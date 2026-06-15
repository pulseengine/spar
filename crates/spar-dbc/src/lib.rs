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
//! # Mapping (`REQ-INGEST-DBC-001` + `REQ-INGEST-DBC-FLOWS-001`)
//!
//! | DBC concept                | AADL construct                                  |
//! |----------------------------|-------------------------------------------------|
//! | CAN bus (the network)      | a single `bus CAN_Bus`                           |
//! | node / ECU                 | `device <Node>` with `requires bus access`      |
//! | message (frame)            | `data Msg_<Name>` with `Data_Size => <n> Bytes` |
//! | message tx (the `BO_` node)| an `out event data <Msg>` port on that device   |
//! | message rx (a `SG_` receiver) | an `in event data <Msg>` port on that device |
//! | a tx→rx pairing            | a `port` connection bound to the CAN bus        |
//! | the whole network          | `system CAN_Network` + its implementation        |
//!
//! # Message flows (`REQ-INGEST-DBC-FLOWS-001`)
//!
//! A CAN frame is a discrete, data-carrying event, so each modelled message
//! becomes an `out event data Msg_<Name>` port on its transmitting node and an
//! `in event data Msg_<Name>` port on every receiving node, joined by `port`
//! connections. Each connection carries
//! `Deployment_Properties::Actual_Connection_Binding => (reference (can_bus))`
//! — the faithful AADL statement that this message is carried over the CAN
//! bus. The single broadcast `out` port fanning out to several `in` ports
//! mirrors CAN's one-to-many broadcast.
//!
//! Transmitter comes from the `BO_` line; receivers are the union of the
//! frame's `SG_` signal receiver lists. A message is wired as a flow only when
//! it has **both** a known transmitter node and at least one known receiver
//! node (the `Vector__XXX` placeholder and unknown names are dropped), so every
//! emitted port participates in a connection — no dangling features. Messages
//! lacking either end still emit their `data` type, as before.
//!
//! **Scope note (not a network-calculus feeder).** These connections model the
//! CAN broadcast for dataflow, traceability and codegen. They are *not* wired
//! into spar's FIFO TFA/PLP network-calculus engine: CAN is priority-arbitrated,
//! not FIFO, so a FIFO bound over it would be unsound. The CAN bus carries no
//! `Spar_Network::Switch_Type`, so `spar_network::extract_network_graph`
//! correctly ignores it (buses without that property are skipped). CAN
//! response-time analysis is a separate (Tindell) track behind its own
//! soundness gate.
//!
//! [Vector CAN database]: https://en.wikipedia.org/wiki/CAN_bus

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
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

/// A modelled message flow: the message's index, its transmitting device, and
/// the port names on each end of the resulting `port` connections.
struct MessageFlow {
    /// Index into `messages` (and `dbc.messages`).
    msg: usize,
    /// Transmitting device index + its `out event data` port name.
    tx: (usize, String),
    /// Receiving device indices + their `in event data` port names.
    rx: Vec<(usize, String)>,
}

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

    // Raw DBC node name -> device index, for resolving transmitters/receivers.
    let node_index: HashMap<&str, usize> = dbc
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.0.as_str(), i))
        .collect();

    // -- flow model (REQ-INGEST-DBC-FLOWS-001) ---------------------------
    // For each message with a known transmitter AND >=1 known receiver, allocate
    // a typed `out`/`in event data` port on each participating device and record
    // the flow for connection emission. Port names live in a per-device
    // namespace seeded with the reserved `can` bus-access feature.
    let mut dev_port_alloc: Vec<NameAllocator> = (0..devices.len())
        .map(|_| {
            let mut a = NameAllocator::new();
            a.reserve("can");
            a
        })
        .collect();
    // Extra feature lines (typed ports) per device, in message order.
    let mut dev_features: Vec<Vec<String>> = vec![Vec::new(); devices.len()];
    let mut flows: Vec<MessageFlow> = Vec::new();

    for (j, m) in dbc.messages.iter().enumerate() {
        let tx_dev = match &m.transmitter {
            Transmitter::NodeName(n) => node_index.get(n.as_str()).copied(),
            Transmitter::VectorXXX => None,
        };
        // Receivers: union over the frame's signals, in first-seen order,
        // dropping the placeholder, the transmitter itself, and unknown nodes.
        let mut rx_devs: Vec<usize> = Vec::new();
        for sig in &m.signals {
            for r in &sig.receivers {
                if !is_real_node(r) {
                    continue;
                }
                if let Some(&ri) = node_index.get(r.as_str())
                    && Some(ri) != tx_dev
                    && !rx_devs.contains(&ri)
                {
                    rx_devs.push(ri);
                }
            }
        }

        let Some(txi) = tx_dev else { continue };
        if rx_devs.is_empty() {
            continue;
        }

        let msg_type = messages[j].0.clone();
        let base = sanitize_ident(&m.name, "Frame");
        let tx_port = dev_port_alloc[txi].alloc(&format!("{base}_tx"), "frame_tx");
        dev_features[txi].push(format!("{tx_port}: out event data port {msg_type};"));
        let rx: Vec<(usize, String)> = rx_devs
            .iter()
            .map(|&ri| {
                let p = dev_port_alloc[ri].alloc(&format!("{base}_rx"), "frame_rx");
                dev_features[ri].push(format!("{p}: in event data port {msg_type};"));
                (ri, p)
            })
            .collect();
        flows.push(MessageFlow {
            msg: j,
            tx: (txi, tx_port),
            rx,
        });
    }

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
    for (i, dev) in devices.iter().enumerate() {
        push_line(&mut out, 1, &format!("device {dev}"));
        push_line(&mut out, 2, "features");
        push_line(
            &mut out,
            3,
            &format!("can: requires bus access {BUS_NAME};"),
        );
        // Message-flow ports (transmitted out, received in).
        for feature in &dev_features[i] {
            push_line(&mut out, 3, feature);
        }
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
        // Bus-access connections (every device shares the broadcast bus).
        for inst in &dev_insts {
            let c = conns.alloc(&format!("{inst}_access"), "access");
            push_line(
                &mut out,
                3,
                &format!("{c}: bus access {inst}.can <-> {bus_inst};"),
            );
        }
        // Message-flow port connections, each bound to the CAN bus. A single
        // `out` port may source several connections (CAN broadcast fan-out).
        for flow in &flows {
            let (txi, tx_port) = &flow.tx;
            let tx_inst = &dev_insts[*txi];
            let base = sanitize_ident(&dbc.messages[flow.msg].name, "frame").to_lowercase();
            for (ri, rx_port) in &flow.rx {
                let rx_inst = &dev_insts[*ri];
                let c = conns.alloc(&format!("{tx_inst}_{base}_to_{rx_inst}"), "flow");
                push_line(
                    &mut out,
                    3,
                    &format!(
                        "{c}: port {tx_inst}.{tx_port} -> {rx_inst}.{rx_port} \
                         {{Deployment_Properties::Actual_Connection_Binding => (reference ({bus_inst}));}};"
                    ),
                );
            }
        }
    }

    push_line(&mut out, 1, &format!("end {SYSTEM_NAME}.impl;"));
    out.push('\n');

    push_line(&mut out, 0, &format!("end {pkg};"));
    out
}

/// Whether `name` denotes a real DBC node, excluding the `Vector__XXX`
/// "no node" placeholder (and its variants) and the empty string.
fn is_real_node(name: &str) -> bool {
    !matches!(name, "Vector__XXX" | "VectorXXX" | "")
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
