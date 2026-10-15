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
//!   "variant":         "diesel-eu5",          // NOT YET SUPPORTED — rejected
//!   "variant_context": "/path/to/ctx.json"    // NOT YET SUPPORTED — rejected
//! }
//! ```
//!
//! Output: a per-flow latency record carrying the source / sink FQNs,
//! the flow name (when found), and the per-pass diagnostics from the
//! `LatencyAnalysis` pass — these already alternate compute (WCET) and
//! network (WCTT) hops in the message stream so an LLM can read the
//! shape without re-running its own walk.
//!
//! # Variant scoping (deferred)
//!
//! Unlike `spar.verify_move` and `spar.enumerate_moves`, this tool does
//! NOT yet apply rivet variant filtering to the analysis. Wiring the
//! `VariantScope` through `LatencyAnalysis` requires the pass to honour
//! "kept" predicates on the instance hierarchy (chains can cross dropped
//! components in surprising ways), which is tracked as a v0.10
//! enhancement. To avoid silently returning an unfiltered chain when an
//! agent asks for a variant-scoped one, we hard-reject the inputs with
//! a `BAD_INPUT` error here — the agent should fall back to
//! `spar.verify_move` / `spar.enumerate_moves` for variant-scoped queries
//! against the same chain participants.
//!
//! # Matching
//!
//! The components a flow passes through are resolved the way the latency
//! pass resolves them ([`spar_analysis::latency::flow_components`]): a
//! `sub.flow` segment is replaced by the components named in `sub`'s
//! implementation of `flow` (so threads inside a process are reached through
//! a flow specification on the process, not by writing `proc.thread.flow`).
//! `source_thread` must name the FIRST component the flow reaches and
//! `sink_thread` the LAST, each as a bare instance name or a dotted/`/`
//! path suffix (`producer`, `app.producer`).
//!
//! An end-to-end flow with a segment reaching more than one level inside a
//! subcomponent is not AADL and is rejected by the `flow_check` analysis; a
//! request that would name such a flow gets `CHAIN_INVALID` carrying that
//! rejection, never a chain. Models without any matching end-to-end flow
//! return `CHAIN_NOT_FOUND`.
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
use spar_analysis::flow_check::FlowCheckAnalysis;
use spar_analysis::latency::{LatencyAnalysis, flow_components};
use spar_analysis::{Analysis, AnalysisDiagnostic, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
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
    let variant = match optional_string(arguments, "variant") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let variant_context = match optional_string(arguments, "variant_context") {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Mutual-exclusion guard, applied before the not-yet-supported
    // refusal so an agent that supplies BOTH gets the more specific
    // error. Mirrors the v1 contract's CLI-side enforcement in
    // `spar_cli::moves::load_variant_context`.
    if variant.is_some() && variant_context.is_some() {
        return ToolResult::Error {
            code: "BAD_INPUT",
            message: "variant and variant_context are mutually exclusive (see \
                 docs/contracts/rivet-spar-variant-v1.md)"
                .to_string(),
        };
    }

    // Variant inputs are accepted in the schema for API symmetry with
    // verify_move / enumerate_moves but are not yet threaded into the
    // latency analysis. Until that wiring lands (tracked as a v0.10
    // enhancement), refuse the call rather than silently return an
    // unfiltered chain — a wrong answer is worse than a clear refusal.
    if variant.is_some() || variant_context.is_some() {
        return ToolResult::Error {
            code: "BAD_INPUT",
            message: "variant and variant_context are not yet supported for check_chain \
                 (tracked as v0.10 enhancement); call spar.verify_move or \
                 spar.enumerate_moves with --variant for variant-scoped queries"
                .to_string(),
        };
    }

    // Parse + instantiate. We reuse the same minimal pipeline as the
    // verify path (single file, named root) without going through
    // `spar_cli::moves` because the check_chain tool does not need
    // overlay scaffolding.
    let instance = match parse_and_instantiate(&model, &root) {
        Ok(i) => i,
        Err(e) => return e,
    };

    // Find a matching end-to-end flow.
    let (flow_name, src_fqn, sink_fqn) = match find_chain(&instance, &source_thread, &sink_thread) {
        ChainMatch::Found {
            flow_name,
            source,
            sink,
        } => (flow_name, source, sink),
        ChainMatch::Rejected { flow_name } => {
            let prefix = format!("end-to-end flow '{flow_name}': ");
            let reasons: Vec<String> = FlowCheckAnalysis
                .analyze(&instance)
                .into_iter()
                .filter(|d| d.severity == Severity::Error && d.message.starts_with(&prefix))
                .map(|d| d.message)
                .collect();
            return ToolResult::Error {
                code: "CHAIN_INVALID",
                message: format!(
                    "end-to-end flow `{flow_name}` between `{source_thread}` and \
                     `{sink_thread}` is not valid AADL and is not analysed: {}",
                    reasons.join("; "),
                ),
            };
        }
        ChainMatch::NotFound => {
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
    let tree: Arc<ItemTree> = spar_hir_def::file_item_tree(&db, sf).clone();

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

/// Outcome of looking for the end-to-end flow a request names.
enum ChainMatch {
    /// A valid flow whose first reached component is the source and last is
    /// the sink; `source` / `sink` are instance paths (`root/app/producer`).
    Found {
        flow_name: String,
        source: String,
        sink: String,
    },
    /// No valid flow matches, but a flow `flow_check` rejects (a segment
    /// reaching inside a subcomponent) mentions both names.
    Rejected {
        flow_name: String,
    },
    NotFound,
}

/// Locate the end-to-end flow from `source` to `sink`. A valid flow is
/// matched on the components it passes through (see the module docs); a
/// valid match always wins over a rejected flow.
fn find_chain(instance: &SystemInstance, source: &str, sink: &str) -> ChainMatch {
    let mut rejected: Option<String> = None;
    for (_idx, e2e) in instance.end_to_end_flows.iter() {
        let (Some(first_seg), Some(last_seg)) = (e2e.segments.first(), e2e.segments.last()) else {
            continue;
        };
        if e2e.nested_segment().is_some() {
            // Not analysable: resolving it would pick the wrong component.
            // Remember it only so the request gets the rejection, not a
            // misleading "not found".
            if rejected.is_none()
                && segment_mentions(first_seg, source)
                && segment_mentions(last_seg, sink)
            {
                rejected = Some(e2e.name.as_str().to_string());
            }
            continue;
        }
        let comps = flow_components(instance, e2e);
        if let (Some(&first), Some(&last)) = (comps.first(), comps.last())
            && component_matches(instance, first, source)
            && component_matches(instance, last, sink)
        {
            return ChainMatch::Found {
                flow_name: e2e.name.as_str().to_string(),
                source: instance.component_instance_path(first),
                sink: instance.component_instance_path(last),
            };
        }
    }
    match rejected {
        Some(flow_name) => ChainMatch::Rejected { flow_name },
        None => ChainMatch::NotFound,
    }
}

/// True when `needle` names component `idx`: its bare instance name, or a
/// `.` / `/` path suffix of its instance path (case-insensitive).
fn component_matches(instance: &SystemInstance, idx: ComponentInstanceIdx, needle: &str) -> bool {
    let needle = needle.replace('.', "/").to_ascii_lowercase();
    let path = instance.component_instance_path(idx).to_ascii_lowercase();
    path == needle || path.ends_with(&format!("/{needle}"))
}

/// True when `needle` is one of the `.`-separated parts of `segment`
/// (case-insensitive). Used only to point a request at a flow that
/// `flow_check` rejects; never to resolve a chain.
fn segment_mentions(segment: &spar_hir_def::Name, needle: &str) -> bool {
    segment
        .as_str()
        .split('.')
        .any(|part| part.eq_ignore_ascii_case(needle))
}
