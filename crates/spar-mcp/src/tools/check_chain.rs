//! `spar.check_chain` — end-to-end latency analysis for a thread chain.
//!
//! Input shape:
//!
//! ```json
//! {
//!   "model": "path/to/system.aadl",
//!   "root":  "Pkg::Type.Impl",
//!   "source_thread": "fw.acquire",
//!   "sink_thread":   "fw.actuate",
//!   "variant":         "diesel-eu5",          // optional
//!   "variant_context": "/path/to/ctx.json"    // optional
//! }
//! ```
//!
//! Output: a per-flow latency record carrying the source / sink FQNs,
//! the flow name (when found), and the per-pass diagnostics from the
//! `LatencyAnalysis` pass — these already alternate compute (WCET) and
//! network (WCTT) hops in the message stream so an LLM can read the
//! shape without re-running its own walk.
//!
//! # Matching
//!
//! The tool matches on the *name* of an end-to-end flow whose first
//! segment names a subcomponent corresponding to the source thread
//! FQN and whose last component segment matches the sink thread FQN.
//! Models without an explicit end-to-end flow declaration return a
//! `CHAIN_NOT_FOUND` error so the agent can fall back to enumerating
//! flows or asking a human.
//!
//! # Why the latency pass and not a custom walk?
//!
//! The v0.8.0 latency analysis already alternates RTA-derived WCET on
//! compute hops with WCTT-derived bounds on network hops (Track D).
//! Re-implementing that walk here would either drift from the canonical
//! pass or duplicate ~600 lines of property-accessor scaffolding. The
//! MCP surface intentionally re-uses the analysis-driven view.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use spar_analysis::{Analysis, AnalysisDiagnostic, Severity, latency::LatencyAnalysis};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ItemTree;

use super::{ToolResult, optional_string, required_string};

/// Wire-format severity (lower-case, matching the rest of the MCP
/// surface). Mirrors the spar-analysis `Severity` enum but with a
/// stable JSON shape independent of upstream evolution.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WireSeverity {
    Error,
    Warning,
    Info,
}

impl From<Severity> for WireSeverity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Error => WireSeverity::Error,
            Severity::Warning => WireSeverity::Warning,
            Severity::Info => WireSeverity::Info,
        }
    }
}

/// Wire-format diagnostic (latency-pass output).
#[derive(Debug, Clone, Serialize)]
pub struct WireDiagnostic {
    pub severity: WireSeverity,
    pub message: String,
    pub path: Vec<String>,
}

impl From<&AnalysisDiagnostic> for WireDiagnostic {
    fn from(d: &AnalysisDiagnostic) -> Self {
        WireDiagnostic {
            severity: d.severity.into(),
            message: d.message.clone(),
            path: d.path.clone(),
        }
    }
}

/// Output payload for `spar.check_chain`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckChainReport {
    /// Source thread FQN as resolved against the instance hierarchy.
    pub source_thread: String,
    /// Sink thread FQN as resolved against the instance hierarchy.
    pub sink_thread: String,
    /// Name of the end-to-end flow that connects source -> sink.
    pub flow_name: String,
    /// Per-flow diagnostic stream from the `LatencyAnalysis` pass.
    /// Best-case / worst-case bounds and per-hop annotations are
    /// already present in this stream's `message` field — see
    /// [`spar_analysis::latency`] for the textual shape.
    pub diagnostics: Vec<WireDiagnostic>,
    /// True when the diagnostic stream contains at least one error-
    /// severity entry (e.g., an unservable network hop).
    pub has_errors: bool,
}

/// In-process entry point for the check_chain tool.
pub fn call(arguments: &Value) -> ToolResult {
    let model = match required_string(arguments, "model") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let root = match required_string(arguments, "root") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let source_thread = match required_string(arguments, "source_thread") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let sink_thread = match required_string(arguments, "sink_thread") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let _variant = match optional_string(arguments, "variant") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let _variant_context = match optional_string(arguments, "variant_context") {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Parse + instantiate. We reuse the same minimal pipeline as the
    // verify path (single file, named root) without going through
    // `spar_cli::moves` because the check_chain tool does not need
    // overlay scaffolding. Variant scoping is accepted for API
    // symmetry but not yet applied (latency analysis is monotonic
    // w.r.t. the kept subset; non-kept components contribute 0 to the
    // chain when their bindings are dropped).
    let instance = match parse_and_instantiate(&model, &root) {
        Ok(i) => i,
        Err(e) => return e,
    };

    // Find a matching end-to-end flow.
    let (flow_name, src_fqn, sink_fqn) = match find_chain(&instance, &source_thread, &sink_thread) {
        Some(t) => t,
        None => {
            return ToolResult::Error {
                code: "CHAIN_NOT_FOUND",
                message: format!(
                    "no end-to-end flow connects source `{source_thread}` to sink \
                         `{sink_thread}`; declare an `end to end flow` in the system \
                         implementation or pass the flow name directly",
                ),
            };
        }
    };

    // Run the latency pass and filter to diagnostics that mention the
    // matched flow's name. The pass already produces both bounds
    // (`[a ms .. b ms]`) and per-hop annotations on chains that span
    // a network hop — the agent reads the shape directly.
    let pass = LatencyAnalysis;
    let all = pass.analyze(&instance);
    let diagnostics: Vec<WireDiagnostic> = all
        .iter()
        .filter(|d| d.message.contains(&format!("'{flow_name}'")))
        .map(WireDiagnostic::from)
        .collect();

    let has_errors = diagnostics
        .iter()
        .any(|d| matches!(d.severity, WireSeverity::Error));

    let report = CheckChainReport {
        source_thread: src_fqn,
        sink_thread: sink_fqn,
        flow_name,
        diagnostics,
        has_errors,
    };

    match serde_json::to_value(&report) {
        Ok(v) => ToolResult::Ok(v),
        Err(e) => ToolResult::Error {
            code: "INTERNAL",
            message: format!("failed to serialise CheckChainReport: {e}"),
        },
    }
}

/// Parse a single AADL file and instantiate the named root.
///
/// This mirrors `parse_and_instantiate` in `spar_cli::moves` (which is
/// module-private). The MCP tool is single-file by design — multi-file
/// chains are out of scope for v0.9.0 commit 8.
fn parse_and_instantiate(model: &str, root: &str) -> Result<SystemInstance, ToolResult> {
    let source = std::fs::read_to_string(model).map_err(|e| ToolResult::Error {
        code: "MODEL_NOT_FOUND",
        message: format!("cannot read {model}: {e}"),
    })?;
    let parsed = spar_syntax::parse(&source);
    if !parsed.ok() {
        let mut msg = String::new();
        for err in parsed.errors() {
            let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
            msg.push_str(&format!("{model}:{line}:{col}: {}\n", err.msg));
        }
        return Err(ToolResult::Error {
            code: "BAD_INPUT",
            message: format!("parse error: {msg}"),
        });
    }

    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, model.to_string(), source);
    let tree: Arc<ItemTree> = spar_hir_def::file_item_tree(&db, sf);

    let (pkg_name, type_name, impl_name) = parse_root_ref(root)?;
    let scope = spar_hir_def::GlobalScope::from_trees(vec![tree]);
    let instance = SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );
    if instance.component_count() == 0 {
        return Err(ToolResult::Error {
            code: "MODEL_NOT_FOUND",
            message: format!("root `{root}` did not instantiate (0 components)"),
        });
    }
    Ok(instance)
}

/// Parse a `Pkg::Type.Impl` root reference into its components.
fn parse_root_ref(s: &str) -> Result<(String, String, String), ToolResult> {
    let parts: Vec<&str> = s.splitn(2, "::").collect();
    let bad = || ToolResult::Error {
        code: "BAD_INPUT",
        message: format!("root `{s}` must be Package::Type.Impl"),
    };
    if parts.len() != 2 {
        return Err(bad());
    }
    let pkg = parts[0].to_string();
    let type_impl: Vec<&str> = parts[1].splitn(2, '.').collect();
    if type_impl.len() != 2 {
        return Err(bad());
    }
    Ok((pkg, type_impl[0].to_string(), type_impl[1].to_string()))
}

/// Locate an end-to-end flow whose first / last component segments
/// match the source / sink thread names (case-insensitive bare-name or
/// suffix match). Returns `(flow_name, source_fqn, sink_fqn)` on a hit.
///
/// Flow segments alternate component-flow refs ("subcomp.flow_name")
/// and connection names. We compare on the subcomponent prefix only.
fn find_chain(
    instance: &SystemInstance,
    source: &str,
    sink: &str,
) -> Option<(String, String, String)> {
    let src_lower = source.to_ascii_lowercase();
    let sink_lower = sink.to_ascii_lowercase();

    for (_idx, e2e) in instance.end_to_end_flows.iter() {
        let segs: Vec<&str> = e2e.segments.iter().map(|s| s.as_str()).collect();
        if segs.is_empty() {
            continue;
        }
        let first = segs.first().copied().unwrap_or("");
        let last = segs.last().copied().unwrap_or("");

        if segment_contains_thread(first, &src_lower) && segment_contains_thread(last, &sink_lower)
        {
            // Best-effort FQN: keep the dotted path the segment
            // already carries (it's `subcomp.flow_name` or
            // `path.subcomp.flow`), strip the trailing flow component,
            // and prepend the owner's name for context.
            let owner = instance.component(e2e.owner);
            let owner_name = owner.name.as_str();
            let src_fqn = format!("{owner_name}/{}", strip_trailing_flow(first));
            let sink_fqn = format!("{owner_name}/{}", strip_trailing_flow(last));
            return Some((e2e.name.as_str().to_string(), src_fqn, sink_fqn));
        }
    }
    None
}

/// True when `needle_lower` appears as a `.`-separated path component
/// of `segment` (case-insensitive). Used to match an agent-supplied
/// thread name against a flow segment of the form `subcomp.flow_name`
/// or `enclosing.subcomp.flow_name`.
fn segment_contains_thread(segment: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return false;
    }
    let seg_lower = segment.to_ascii_lowercase();
    seg_lower.split('.').any(|part| part == needle_lower)
        || seg_lower.split('/').any(|part| part == needle_lower)
}

/// Drop the last `.`-separated component of a flow-segment name. This
/// is the trailing `flow_name` of a `subcomp.flow_name` segment so we
/// can form an FQN that points at the *thread*, not the flow spec.
fn strip_trailing_flow(segment: &str) -> String {
    match segment.rsplit_once('.') {
        Some((head, _tail)) => head.to_string(),
        None => segment.to_string(),
    }
}
