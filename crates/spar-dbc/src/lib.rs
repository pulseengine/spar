//! CAN `.dbc` ingest for spar.
//!
//! This crate reads a [Vector CAN database] (`.dbc`) file and emits **AADL
//! v2.3 source text** describing the same network. The emitted text is meant
//! to flow through spar's normal AADL pipeline
//! (`parse → ItemTree → GlobalScope → SystemInstance`), so the model the rest
//! of the toolchain analyses is an ordinary AADL instance — there is no
//! special-case DBC path downstream.
//!
//! # Why emit text instead of building an `ItemTree` directly
//!
//! Constructing `spar-hir-def`'s arena structures by hand would couple this
//! crate to internal, release-volatile representation details. Emitting AADL
//! *source* keeps the coupling at the stable language surface: spar's own
//! parser is the contract, and the round-trip test
//! (`tests/roundtrip.rs`) uses that parser as a mechanical oracle — if the
//! emitted text does not parse and instantiate with zero diagnostics, the
//! test fails. A useful side effect is a human-inspectable
//! "transpile DBC → AADL" artifact.
//!
//! # Mapping (v0.17.0 scope — `REQ-INGEST-DBC-001`)
//!
//! | DBC concept            | AADL construct                                  |
//! |------------------------|-------------------------------------------------|
//! | CAN bus (the network)  | a single `bus CAN_Bus`                           |
//! | node / ECU             | `device <Node>` with `requires bus access`      |
//! | message (frame)        | `data Msg_<Name>` with `Data_Size => <n> Bytes` |
//! | the whole network      | `system CAN_Network` + its implementation        |
//!
//! Message *flows* (which node transmits/receives which frame, modelled as
//! ports and data connections across the bus) are deliberately **out of
//! scope** for this first cut — that is the broadcast-bus equivalent of the
//! port-connection modelling the network-calculus track will need, and is
//! tracked separately as a follow-up requirement. What ships here is a
//! structurally complete, instantiable model: bus + devices + message data
//! types, with each device wired to the bus.
//!
//! [Vector CAN database]: https://en.wikipedia.org/wiki/CAN_bus

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fmt;

use can_dbc::{Dbc, Transmitter};

/// Error raised while ingesting a `.dbc` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestError {
    /// The `can-dbc` parser rejected the input. The wrapped string is the
    /// parser's diagnostic (its error type is not `'static`-cloneable, so we
    /// render it eagerly).
    Parse(String),
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestError::Parse(msg) => write!(f, "failed to parse DBC: {msg}"),
        }
    }
}

impl core::error::Error for IngestError {}

/// Parse `dbc_text` and emit an AADL v2.3 package named `package_name`
/// describing the CAN network.
///
/// The returned string is a complete, self-contained AADL package: it parses
/// and instantiates (root `CAN_Network.impl`) with zero diagnostics through
/// spar's pipeline. `package_name` is sanitised to a legal AADL identifier; an
/// empty or all-symbol name falls back to `Can_Network`.
pub fn dbc_to_aadl(dbc_text: &str, package_name: &str) -> Result<String, IngestError> {
    let dbc = Dbc::try_from(dbc_text).map_err(|e| IngestError::Parse(format!("{e:?}")))?;
    Ok(emit_aadl(&dbc, package_name))
}

/// The fixed bus classifier name. Not derived from the DBC (a `.dbc` describes
/// exactly one CAN network), so it is a constant rather than a sanitised name.
const BUS_NAME: &str = "CAN_Bus";
/// The fixed root system classifier name.
const SYSTEM_NAME: &str = "CAN_Network";

fn emit_aadl(dbc: &Dbc, package_name: &str) -> String {
    // One global namespace for *classifier* names (bus, data types, devices,
    // system) so a node accidentally named like a message can never collide.
    let mut classifiers = NameAllocator::new();
    // The two fixed classifiers are reserved first so user names yield to them.
    classifiers.reserve(BUS_NAME);
    classifiers.reserve(SYSTEM_NAME);

    let pkg = {
        let mut a = NameAllocator::new();
        a.alloc(package_name, "Can_Network")
    };

    // Message data type names, in DBC order, paired with byte sizes.
    let messages: Vec<(String, u64)> = dbc
        .messages
        .iter()
        .map(|m| {
            let raw = format!("Msg_{}", m.name);
            (classifiers.alloc(&raw, "Msg_Frame"), m.size)
        })
        .collect();

    // Device names, in DBC node order.
    let devices: Vec<String> = dbc
        .nodes
        .iter()
        .map(|n| classifiers.alloc(&n.0, "Ecu"))
        .collect();

    let mut out = String::new();
    push_line(&mut out, 0, &format!("package {pkg}"));
    push_line(&mut out, 0, "public");
    out.push('\n');

    // -- the bus ---------------------------------------------------------
    push_line(&mut out, 1, &format!("bus {BUS_NAME}"));
    push_line(&mut out, 1, &format!("end {BUS_NAME};"));
    out.push('\n');

    // -- message frames as data types ------------------------------------
    for (name, size) in &messages {
        push_line(&mut out, 1, &format!("data {name}"));
        push_line(&mut out, 2, "properties");
        // A CAN frame payload is 0..=8 bytes (classic) or up to 64 (CAN FD);
        // `size` is already the DBC byte count. AADL `Data_Size` wants a
        // positive size, so clamp a degenerate 0 up to 1 byte.
        let bytes = (*size).max(1);
        push_line(&mut out, 3, &format!("Data_Size => {bytes} Bytes;"));
        push_line(&mut out, 1, &format!("end {name};"));
        out.push('\n');
    }

    // -- nodes as devices ------------------------------------------------
    for dev in &devices {
        push_line(&mut out, 1, &format!("device {dev}"));
        push_line(&mut out, 2, "features");
        push_line(
            &mut out,
            3,
            &format!("can: requires bus access {BUS_NAME};"),
        );
        push_line(&mut out, 1, &format!("end {dev};"));
        out.push('\n');
    }

    // -- the network system + implementation -----------------------------
    push_line(&mut out, 1, &format!("system {SYSTEM_NAME}"));
    push_line(&mut out, 1, &format!("end {SYSTEM_NAME};"));
    out.push('\n');

    push_line(
        &mut out,
        1,
        &format!("system implementation {SYSTEM_NAME}.impl"),
    );

    // Subcomponent names are a *separate* namespace (scoped to the impl).
    let mut subs = NameAllocator::new();
    let bus_inst = subs.alloc("can_bus", "can_bus");
    let dev_insts: Vec<String> = devices
        .iter()
        .map(|d| subs.alloc(&d.to_lowercase(), "ecu"))
        .collect();

    push_line(&mut out, 2, "subcomponents");
    push_line(&mut out, 3, &format!("{bus_inst}: bus {BUS_NAME};"));
    for (inst, dev) in dev_insts.iter().zip(&devices) {
        push_line(&mut out, 3, &format!("{inst}: device {dev};"));
    }

    // Connection names are likewise scoped to the impl.
    if !dev_insts.is_empty() {
        push_line(&mut out, 2, "connections");
        let mut conns = NameAllocator::new();
        for inst in &dev_insts {
            let c = conns.alloc(&format!("{inst}_access"), "access");
            push_line(
                &mut out,
                3,
                &format!("{c}: bus access {inst}.can <-> {bus_inst};"),
            );
        }
    }

    push_line(&mut out, 1, &format!("end {SYSTEM_NAME}.impl;"));
    out.push('\n');

    push_line(&mut out, 0, &format!("end {pkg};"));
    out
}

fn push_line(out: &mut String, indent: usize, text: &str) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

/// Allocates legal, unique AADL identifiers.
///
/// AADL identifiers are `letter ( [underscore] (letter | digit) )*` — they must
/// start with a letter, contain no consecutive or trailing underscores, and
/// must not be a reserved word. Names are compared case-insensitively for both
/// keyword and uniqueness checks (AADL identifiers are case-insensitive).
struct NameAllocator {
    used: HashSet<String>,
}

impl NameAllocator {
    fn new() -> Self {
        Self {
            used: HashSet::new(),
        }
    }

    /// Reserve a known-legal name verbatim (used for the fixed classifiers).
    fn reserve(&mut self, name: &str) {
        self.used.insert(name.to_lowercase());
    }

    /// Sanitise `raw` to a legal AADL identifier (using `fallback` if `raw`
    /// has no usable characters) and make it unique within this allocator.
    fn alloc(&mut self, raw: &str, fallback: &str) -> String {
        let base = sanitize_ident(raw, fallback);
        let mut candidate = base.clone();
        let mut n = 2u32;
        while self.used.contains(&candidate.to_lowercase()) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        self.used.insert(candidate.to_lowercase());
        candidate
    }
}

/// Convert an arbitrary string to a legal AADL identifier.
///
/// 1. Split on every non-alphanumeric character; drop empty segments.
/// 2. Join segments with single underscores (collapses runs, drops
///    leading/trailing separators — so no `__` and no edge underscore).
/// 3. If nothing usable remains, use `fallback`.
/// 4. If the result starts with a digit, prefix `n_` (a letter must lead).
fn sanitize_ident(raw: &str, fallback: &str) -> String {
    let segments: Vec<&str> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let mut s = if segments.is_empty() {
        fallback.to_string()
    } else {
        segments.join("_")
    };

    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s = format!("n_{s}");
    }

    if AADL_KEYWORDS.contains(&s.to_lowercase().as_str()) {
        // Trailing underscore is illegal, so append a letter, not `_`.
        s.push_str("_id");
    }

    s
}

/// AADL v2.3 reserved words (lowercase). Sourced from the lexer token set in
/// `spar-parser`'s `syntax_kind.rs`. An emitted identifier matching one of
/// these (case-insensitively) is escaped by [`sanitize_ident`].
const AADL_KEYWORDS: &[&str] = &[
    "aadlboolean",
    "aadlinteger",
    "aadlreal",
    "aadlstring",
    "abstract",
    "access",
    "all",
    "and",
    "annex",
    "applies",
    "binding",
    "bus",
    "calls",
    "classifier",
    "compute",
    "connections",
    "constant",
    "data",
    "delta",
    "device",
    "end",
    "enumeration",
    "event",
    "extends",
    "false",
    "feature",
    "features",
    "file",
    "flow",
    "flows",
    "group",
    "implementation",
    "in",
    "inherit",
    "initial",
    "interface",
    "internal",
    "inverse",
    "is",
    "list",
    "memory",
    "mode",
    "modes",
    "none",
    "not",
    "of",
    "or",
    "out",
    "package",
    "parameter",
    "path",
    "port",
    "private",
    "process",
    "processor",
    "properties",
    "property",
    "prototypes",
    "provides",
    "public",
    "range",
    "record",
    "reference",
    "refined",
    "renames",
    "requires",
    "self",
    "set",
    "sink",
    "source",
    "subcomponents",
    "subprogram",
    "system",
    "thread",
    "to",
    "transition",
    "true",
    "type",
    "units",
    "virtual",
    "with",
];

/// Whether a node actually transmits any message in `dbc`. Exposed for callers
/// (e.g. a future flow-modelling pass) that want to distinguish active
/// transmitters from listen-only nodes; unused by the current emitter.
#[must_use]
pub fn node_is_transmitter(dbc: &Dbc, node_name: &str) -> bool {
    dbc.messages
        .iter()
        .any(|m| matches!(&m.transmitter, Transmitter::NodeName(n) if n == node_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_ident("EngineECU", "x"), "EngineECU");
        assert_eq!(sanitize_ident("Engine ECU", "x"), "Engine_ECU");
        assert_eq!(sanitize_ident("Engine-ECU/1", "x"), "Engine_ECU_1");
    }

    #[test]
    fn sanitize_digit_lead() {
        assert_eq!(sanitize_ident("3Phase", "x"), "n_3Phase");
        assert_eq!(sanitize_ident("123", "x"), "n_123");
    }

    #[test]
    fn sanitize_empty_uses_fallback() {
        assert_eq!(sanitize_ident("", "Ecu"), "Ecu");
        assert_eq!(sanitize_ident("---", "Ecu"), "Ecu");
        assert_eq!(sanitize_ident("...", "Msg_Frame"), "Msg_Frame");
    }

    #[test]
    fn sanitize_collapses_runs_no_edge_underscore() {
        // No `__`, no leading/trailing underscore.
        assert_eq!(sanitize_ident("__a..b__", "x"), "a_b");
        assert_eq!(sanitize_ident("a   b", "x"), "a_b");
    }

    #[test]
    fn sanitize_escapes_keywords() {
        assert_eq!(sanitize_ident("data", "x"), "data_id");
        assert_eq!(sanitize_ident("System", "x"), "System_id"); // case-insensitive
        assert_eq!(sanitize_ident("bus", "x"), "bus_id");
    }

    #[test]
    fn allocator_uniquifies() {
        let mut a = NameAllocator::new();
        assert_eq!(a.alloc("Ecu", "x"), "Ecu");
        assert_eq!(a.alloc("ecu", "x"), "ecu_2"); // case-insensitive collision
        assert_eq!(a.alloc("ECU", "x"), "ECU_3");
    }
}
