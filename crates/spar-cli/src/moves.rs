//! `spar moves verify` and `spar moves enumerate` — hypothetical-rebinding
//! oracle (Track E commits 3/8 and 4/8).
//!
//! Per the v0.8.0 migration design research §6.3, this module exposes the
//! first two user-facing surfaces of the migration oracle:
//!
//! - `spar moves verify --component X --to Y` (commit 3): single
//!   hypothetical-move pass/fail report.
//! - `spar moves enumerate --component X` (commit 4): design-space
//!   exploration listing every legal target with verification status
//!   and an optional slack-rank metric.
//!
//! Both subcommands share the same parse / resolve / overlay / validate
//! / render scaffolding. Verify is the canonical single-shot pipeline;
//! enumerate fans the same pipeline out across a candidate-target set
//! (either `Spar_Migration::Allowed_Targets` or every Processor /
//! VirtualProcessor in the instance).
//!
//! # Pipeline
//!
//! 1. Parse the AADL model files and instantiate the requested root.
//! 2. Resolve `--component` and `--to` to [`ComponentInstanceIdx`] values
//!    by FQN matching against the instance hierarchy. Errors out with a
//!    clear message when either name fails to resolve, when `--to` does
//!    not name a processor, or when the names match nothing.
//! 3. Build a single-move [`BindingOverlay`] and run
//!    [`BindingOverlay::validate`] to surface
//!    [`OverlayDiagnostic::Frozen`] / [`OverlayDiagnostic::AllowedTargets`]
//!    constraint-layer rejections.
//! 4. Run the standard analysis-pass suite on the un-overlayed instance —
//!    commit 3 only widens the overlay-aware property lookup at the HIR
//!    level (see [`spar_hir_def::actual_processor_binding_with_overlay`]);
//!    the analyses themselves are not yet overlay-aware. Commit 4 widens
//!    that surface to RTA, latency, bandwidth, EMV2, and ARINC653.
//! 5. Render a [`MoveVerifyReport`] in either `text` or `json` form.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | Move is admissible: no overlay violations, no error-severity diagnostics |
//! | 1 | One or more analysis diagnostics at `Severity::Error` |
//! | 2 | Overlay violations (frozen / allowed-targets) |
//!
//! Overlay violations dominate analysis errors for exit-code purposes
//! because they are *constraint-layer* rejections — the move would never
//! be considered valid regardless of analysis results.
//!
//! # FQN resolution
//!
//! `--component` and `--to` accept three shapes:
//!
//! - A bare name (`handler_brake`) — case-insensitive, matched against
//!   any component anywhere in the hierarchy. First match wins.
//! - A path with `/` separators (`root/subsys/handler_brake`) — the
//!   component-path string from each component is matched as a suffix.
//! - A path with `.` separators (`subsys.handler_brake`) — same as above
//!   with `.` translated to `/` for matching, mirroring the AADL
//!   `applies to` shape.
//!
//! This permissive matching aligns with the existing
//! `spar-analysis::arinc653` pattern; v0.9.0 may tighten to fully
//! qualified `Pkg::Type.Impl/sub/sub` once the MCP surface lands.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::process;
use std::sync::Arc;

use serde::Serialize;

use spar_analysis::{AnalysisDiagnostic, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, ItemTree};
use spar_hir_def::{AllowedTargetsViolation, BindingOverlay, FrozenViolation, OverlayDiagnostic};
use spar_solver::enumerate::rank_candidate;
pub use spar_solver::enumerate::{CandidateRank, EnumerationObjective};
use spar_variants::{ContextError, VariantContext};

use crate::variants_bridge::{SourcePathMap, VariantScope};

/// Parsed CLI arguments for `spar moves verify`.
///
/// Populated by the manual-arg-parsing path in [`run_verify`]; mirrors
/// the design-research-style clap struct in track-e-migration-research §6.3
/// without dragging clap into spar-cli (which still uses hand-rolled
/// `args[i]` matching for every other subcommand).
#[derive(Debug, Default)]
pub struct VerifyArgs {
    /// Path(s) to the AADL model file(s) to load.
    pub model_files: Vec<String>,
    /// Root system implementation in `Pkg::Type.Impl` form.
    pub root: String,
    /// FQN (or suffix / bare name) of the component to (hypothetically) move.
    pub component: String,
    /// FQN (or suffix / bare name) of the target processor.
    pub target: String,
    /// Output format: `text` (default) or `json`.
    pub format: String,
    /// Implicit-form variant name. When set (and `variant_context` is
    /// not), spar shells out to `rivet resolve --variant <name>
    /// --format spar-context-json` per the v1 contract. Mutually
    /// exclusive with [`Self::variant_context`].
    pub variant: Option<String>,
    /// Explicit-form variant-context source. `Some("-")` reads stdin;
    /// any other path is read from the filesystem. Mutually exclusive
    /// with [`Self::variant`].
    pub variant_context: Option<String>,
}

/// All ways `spar moves verify` can fail before producing a report.
///
/// Distinct from the [`Violation`] enum that appears *inside* a report —
/// these are CLI-level errors (bad inputs, parse failures, unresolved
/// names) that prevent a verification run from completing at all.
#[derive(Debug)]
pub enum MovesError {
    /// A model file could not be read.
    Io(String, std::io::Error),
    /// A model file failed to parse.
    Parse(String, String),
    /// `--root Pkg::Type.Impl` is not present in the parsed package set.
    UnknownRoot(String),
    /// `--component` does not match any component in the instance.
    UnknownComponent(String),
    /// `--to` does not match any component in the instance.
    UnknownTarget(String),
    /// `--to` matched a non-processor component.
    TargetNotProcessor {
        target: String,
        category: ComponentCategory,
    },
    /// `--format` is neither `text` nor `json`.
    UnknownFormat(String),
    /// `--objective` does not match any of the recognised modes
    /// (`max-response`, `total-load`, `total-power`, `total-weight`,
    /// `balanced`).
    UnknownObjective(String),
    /// `--variant` and `--variant-context` were both supplied. The v1
    /// contract specifies they are mutually exclusive.
    VariantArgsConflict,
    /// `--variant-context` could not be read from the named file or stdin.
    VariantContextIo(String, std::io::Error),
    /// The variant context blob failed schema validation. Wrapped from
    /// [`spar_variants::ContextError`] so unknown-version refusal and
    /// JSON-parse failures are reported with their original message.
    VariantContextSchema(ContextError),
    /// `--variant NAME` was supplied but rivet could not be located on
    /// `$PATH` (and `$RIVET_BIN` was unset). Per the v1 contract we
    /// point the user at the explicit form.
    RivetNotFound,
    /// rivet was located but exited non-zero. The captured stderr is
    /// surfaced to the user.
    RivetFailed { stderr: String, code: Option<i32> },
    /// rivet emitted output we could not capture or decode.
    RivetIo(std::io::Error),
    /// A user-supplied `--component` value resolves to a component that
    /// the variant filter dropped. Per the contract — and the
    /// commit-spec — the move-oracle scopes its analysis to the kept
    /// subset only.
    ComponentNotInVariant { name: String, variant: String },
    /// As above but for `--to`.
    TargetNotInVariant { name: String, variant: String },
}

impl std::fmt::Display for MovesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MovesError::Io(path, err) => write!(f, "Cannot read {path}: {err}"),
            MovesError::Parse(path, msg) => write!(f, "Parse error in {path}: {msg}"),
            MovesError::UnknownRoot(r) => {
                write!(f, "Root {r} did not instantiate (0 components)")
            }
            MovesError::UnknownComponent(c) => {
                write!(f, "--component {c} did not match any component")
            }
            MovesError::UnknownTarget(t) => {
                write!(f, "--to {t} did not match any component")
            }
            MovesError::TargetNotProcessor { target, category } => write!(
                f,
                "--to {target} resolved to a {category}; expected processor"
            ),
            MovesError::UnknownFormat(fmt_) => {
                write!(f, "--format {fmt_} is not recognised (expected text|json)")
            }
            MovesError::UnknownObjective(o) => write!(
                f,
                "--objective {o} is not recognised (expected max-response | total-load | \
                 total-power | total-weight | balanced)",
            ),
            MovesError::VariantArgsConflict => write!(
                f,
                "--variant and --variant-context are mutually exclusive (see docs/contracts/rivet-spar-variant-v1.md)",
            ),
            MovesError::VariantContextIo(path, err) => {
                write!(f, "Cannot read variant context from {path}: {err}")
            }
            MovesError::VariantContextSchema(err) => {
                write!(f, "Variant context: {err}")
            }
            MovesError::RivetNotFound => write!(
                f,
                "rivet not found on $PATH and $RIVET_BIN is unset; \
                 either install rivet or use the explicit form: \
                 `rivet resolve --variant <name> --format spar-context-json > ctx.json` \
                 then pass `--variant-context ctx.json` \
                 (see docs/contracts/rivet-spar-variant-v1.md)",
            ),
            MovesError::RivetFailed { stderr, code } => {
                let suffix = code
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "killed".into());
                write!(f, "rivet resolve failed ({suffix}): {stderr}")
            }
            MovesError::RivetIo(err) => write!(f, "Cannot run rivet: {err}"),
            MovesError::ComponentNotInVariant { name, variant } => {
                write!(f, "--component {name} is not part of variant {variant}",)
            }
            MovesError::TargetNotInVariant { name, variant } => {
                write!(f, "--to {name} is not part of variant {variant}",)
            }
        }
    }
}

/// Structured rendering of a single overlay or analysis violation.
///
/// Mirrored to JSON via serde; the `kind` tag drives discrimination on
/// the consumer side (LLM tool surface in v0.9.0).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum Violation {
    /// Overlay tried to move a `Spar_Migration::Frozen` component.
    Frozen {
        /// FQN of the component the overlay attempted to move.
        component: String,
        /// Reason from `Spar_Migration::Pinned_Reason`, or a default.
        reason: String,
    },
    /// Overlay's target is not in `Spar_Migration::Allowed_Targets`.
    AllowedTargets {
        /// FQN of the component being moved.
        component: String,
        /// FQN of the proposed target (the offending value).
        target: String,
        /// FQNs of the targets the component declared as legal.
        allowed: Vec<String>,
    },
    /// Analysis pass produced an error-severity diagnostic.
    AnalysisError {
        /// The analysis name (e.g., `RtaAnalysis`).
        pass: String,
        /// The diagnostic message.
        message: String,
        /// The severity reported by the analysis.
        severity: SerSeverity,
        /// Element path where the diagnostic was raised.
        path: Vec<String>,
    },
}

/// Wire-format mirror of [`spar_analysis::Severity`].
///
/// We define our own copy so the `Violation::AnalysisError` variant can
/// be serialized with the same lower-case shape that already exists on
/// `AnalysisDiagnostic` without requiring a custom serializer for the
/// upstream type.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SerSeverity {
    /// Error-severity diagnostic — fails the move.
    Error,
    /// Warning-severity diagnostic — logged but does not fail the move.
    Warning,
    /// Informational diagnostic — logged but does not fail the move.
    Info,
}

impl From<Severity> for SerSeverity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Error => SerSeverity::Error,
            Severity::Warning => SerSeverity::Warning,
            Severity::Info => SerSeverity::Info,
        }
    }
}

/// Output shape for `spar moves verify --format json`.
///
/// Documented in the Track E design research §6.3; the JSON shape is the
/// canonical machine-readable contract consumed later by the v0.9.0 MCP
/// `spar.verify_move` tool surface.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MoveVerifyReport {
    /// True if the move is admissible: no overlay violations and no
    /// error-severity diagnostics from the analysis suite.
    pub ok: bool,
    /// FQN of the component being (hypothetically) moved.
    pub component: String,
    /// FQN of the proposed target processor.
    pub target: String,
    /// Overlay + analysis violations, in the order they were detected.
    pub violations: Vec<Violation>,
    /// Per-pass diagnostic stream from the analysis suite, keyed by pass
    /// name. Empty when there were no analysis diagnostics for a pass.
    pub diagnostics_by_pass: BTreeMap<String, Vec<DiagnosticOut>>,
    /// Resolved-variant name when the run was scoped by a rivet
    /// variant context per the v1 contract, otherwise `None`.
    /// Promoted to a top-level field so MCP consumers can route a
    /// follow-up call back to the same variant without parsing the
    /// audit trail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Stable hash of the feature model that produced the variant
    /// resolution. Used as a salsa cache key; surfaced here so audit
    /// trails can pin the exact feature model the analysis was run
    /// against. `None` when no variant was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature_model_hash: Option<String>,
}

/// Wire-format mirror of [`AnalysisDiagnostic`].
///
/// The upstream type already derives `Serialize`, but we re-shape into a
/// flat record keyed `severity / message / path / analysis` so the JSON
/// stream has a stable shape across the MCP transition (where the
/// upstream serde shape may evolve independently).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiagnosticOut {
    /// Severity bucket: `error` / `warning` / `info`.
    pub severity: SerSeverity,
    /// Diagnostic message.
    pub message: String,
    /// Element path (e.g. `["root", "fw", "firmware"]`).
    pub path: Vec<String>,
}

impl From<&AnalysisDiagnostic> for DiagnosticOut {
    fn from(d: &AnalysisDiagnostic) -> Self {
        DiagnosticOut {
            severity: d.severity.into(),
            message: d.message.clone(),
            path: d.path.clone(),
        }
    }
}

/// Run the verify pipeline and return the structured report plus the
/// desired CLI exit code, without printing anything.
///
/// This is the load-bearing function shared between the CLI driver
/// (`run_verify`, which prints + exits) and the MCP `spar.verify_move`
/// tool (which returns the report as JSON over stdio). The pure-data
/// shape is documented under [`MoveVerifyReport`]; the exit code follows
/// the table in the module docs.
///
/// The function is `#[allow(clippy::result_large_err)]` because the
/// error type is shared with [`run_verify`] and downstream callers
/// expect the same type.
pub fn verify_pipeline(args: &VerifyArgs) -> Result<(MoveVerifyReport, i32), MovesError> {
    if args.format != "text" && args.format != "json" {
        return Err(MovesError::UnknownFormat(args.format.clone()));
    }
    if args.model_files.is_empty() {
        return Err(MovesError::Parse(
            "(no files)".to_string(),
            "spar moves verify requires at least one .aadl file".to_string(),
        ));
    }

    // 1. Parse + instantiate.
    let (inst, source_paths) = parse_and_instantiate(&args.model_files, &args.root)?;

    // 2. Optional variant scope.
    let variant_ctx =
        load_variant_context(args.variant.as_deref(), args.variant_context.as_deref())?;
    let scope_holder = variant_ctx
        .as_ref()
        .map(|ctx| VariantScope::new(&inst, ctx, &source_paths));

    // 3. Resolve component + target FQNs (variant-aware).
    let comp_idx = resolve_component_in_scope(&inst, scope_holder.as_ref(), &args.component)
        .ok_or_else(|| component_not_found_error(&args.component, scope_holder.as_ref()))?;
    let target_idx = resolve_component_in_scope(&inst, scope_holder.as_ref(), &args.target)
        .ok_or_else(|| target_not_found_error(&args.target, scope_holder.as_ref()))?;
    let target_cat = inst.component(target_idx).category;
    if target_cat != ComponentCategory::Processor
        && target_cat != ComponentCategory::VirtualProcessor
    {
        return Err(MovesError::TargetNotProcessor {
            target: args.target.clone(),
            category: target_cat,
        });
    }

    // 4. Overlay + validate.
    let mut overlay = BindingOverlay::new();
    overlay.add_move(comp_idx, target_idx);
    let overlay_diags = overlay.validate(&inst);

    // 5. Analysis suite.
    let analysis_diags = run_all_analyses(&inst);

    // 6. Build the structured report.
    let mut report = build_report(&inst, comp_idx, target_idx, &overlay_diags, &analysis_diags);
    if let Some(scope) = scope_holder.as_ref() {
        report.variant = Some(scope.variant_name().to_string());
        report.feature_model_hash = Some(scope.feature_model_hash().to_string());
    }

    let code = exit_code_for(&report, &overlay_diags);
    Ok((report, code))
}

/// Run `spar moves verify`, returning the desired process exit code.
///
/// See module docs for the full pipeline; a zero return from this
/// function means the move is admissible. The caller in `main.rs`
/// passes the return through `process::exit` directly so behaviour is
/// observable to a shell or harness.
pub fn run_verify(args: VerifyArgs) -> Result<i32, MovesError> {
    let format = args.format.clone();
    let (report, code) = verify_pipeline(&args)?;

    // Render.
    match format.as_str() {
        "json" => render_json(&report),
        _ => render_text(&report),
    }

    Ok(code)
}

/// Translate a populated [`MoveVerifyReport`] back into the Unix exit
/// code documented in the module-level table.
fn exit_code_for(report: &MoveVerifyReport, overlay_diags: &[OverlayDiagnostic]) -> i32 {
    if !overlay_diags.is_empty() {
        return 2;
    }
    let any_error = report.violations.iter().any(|v| {
        matches!(
            v,
            Violation::AnalysisError {
                severity: SerSeverity::Error,
                ..
            }
        )
    });
    if any_error { 1 } else { 0 }
}

/// Build a [`MoveVerifyReport`] from the raw overlay + analysis outputs.
fn build_report(
    instance: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    target_idx: ComponentInstanceIdx,
    overlay_diags: &[OverlayDiagnostic],
    analysis_diags: &[AnalysisDiagnostic],
) -> MoveVerifyReport {
    let mut violations = Vec::new();

    for d in overlay_diags {
        match d {
            OverlayDiagnostic::Frozen(FrozenViolation { component, reason }) => {
                violations.push(Violation::Frozen {
                    component: fqn(instance, *component),
                    reason: reason.clone(),
                });
            }
            OverlayDiagnostic::AllowedTargets(AllowedTargetsViolation {
                component,
                target,
                allowed,
            }) => {
                violations.push(Violation::AllowedTargets {
                    component: fqn(instance, *component),
                    target: fqn(instance, *target),
                    allowed: allowed.iter().map(|i| fqn(instance, *i)).collect(),
                });
            }
        }
    }

    let mut by_pass: BTreeMap<String, Vec<DiagnosticOut>> = BTreeMap::new();
    for d in analysis_diags {
        by_pass
            .entry(d.analysis.clone())
            .or_default()
            .push(d.into());
        if d.severity == Severity::Error {
            violations.push(Violation::AnalysisError {
                pass: d.analysis.clone(),
                message: d.message.clone(),
                severity: d.severity.into(),
                path: d.path.clone(),
            });
        }
    }

    let ok = violations.is_empty();
    MoveVerifyReport {
        ok,
        component: fqn(instance, comp_idx),
        target: fqn(instance, target_idx),
        violations,
        diagnostics_by_pass: by_pass,
        variant: None,
        feature_model_hash: None,
    }
}

/// Render a [`MoveVerifyReport`] as canonical pretty-printed JSON.
fn render_json(report: &MoveVerifyReport) {
    println!("{}", serde_json::to_string_pretty(report).unwrap());
}

/// Render a [`MoveVerifyReport`] in human-readable form.
///
/// Layout: a single `OK` / `FAIL` summary line, the component / target
/// pair, the violation list (one per line, prefixed by kind), and a
/// terse per-pass diagnostic summary so users can chase the underlying
/// analysis output when an `AnalysisError` is reported.
fn render_text(report: &MoveVerifyReport) {
    let status = if report.ok { "OK" } else { "FAIL" };
    let variant_prefix = match &report.variant {
        Some(v) => format!("(variant={v}) "),
        None => String::new(),
    };
    println!(
        "{}{} move {} -> {}",
        variant_prefix, status, report.component, report.target,
    );

    if report.violations.is_empty() {
        println!("  no violations");
    } else {
        println!("  violations:");
        for v in &report.violations {
            match v {
                Violation::Frozen { component, reason } => {
                    println!("    [Frozen]         {component}: {reason}");
                }
                Violation::AllowedTargets {
                    component,
                    target,
                    allowed,
                } => {
                    println!(
                        "    [AllowedTargets] {component} -> {target} not in [{}]",
                        allowed.join(", "),
                    );
                }
                Violation::AnalysisError {
                    pass,
                    message,
                    severity,
                    path,
                } => {
                    let path_str = if path.is_empty() {
                        "(no path)".to_string()
                    } else {
                        path.join("/")
                    };
                    println!(
                        "    [{}] {pass}: {message} (at {path_str})",
                        format_severity(*severity)
                    );
                }
            }
        }
    }

    if !report.diagnostics_by_pass.is_empty() {
        println!("  diagnostics by pass:");
        for (pass, diags) in &report.diagnostics_by_pass {
            println!("    {pass}: {} diagnostic(s)", diags.len());
        }
    }
}

/// Capitalised severity tag for the text-format renderer.
fn format_severity(s: SerSeverity) -> &'static str {
    match s {
        SerSeverity::Error => "Error",
        SerSeverity::Warning => "Warning",
        SerSeverity::Info => "Info",
    }
}

/// Resolve a user-supplied component name (FQN, dotted path, or bare
/// name) to a [`ComponentInstanceIdx`].
///
/// Matching rules (case-insensitive, first match wins):
///
/// 1. `name` is a bare identifier → match `component.name == name`.
/// 2. `name` contains `/` or `.` → translate `.` to `/`, then match the
///    component's instance path (`root/sub1/sub2`) by suffix.
///
/// Returns `None` if no component matches; resolves ties by preferring
/// matches deeper in the hierarchy (more specific paths win), which is
/// the common case for `--component` arguments naming a leaf thread or
/// process.
pub fn resolve_component(instance: &SystemInstance, name: &str) -> Option<ComponentInstanceIdx> {
    let needle = name.replace('.', "/");
    let needle_lower = needle.to_ascii_lowercase();
    let is_path = needle.contains('/');

    // Path matching: suffix-match against the component's full path.
    if is_path {
        // Prefer the deepest (most specific) match.
        let mut best: Option<(ComponentInstanceIdx, usize)> = None;
        for (idx, _comp) in instance.all_components() {
            let path = component_instance_path(instance, idx);
            let path_lower = path.to_ascii_lowercase();
            if path_lower.ends_with(&needle_lower) {
                let depth = path.matches('/').count();
                if best.map(|(_, d)| depth >= d).unwrap_or(true) {
                    best = Some((idx, depth));
                }
            }
        }
        return best.map(|(idx, _)| idx);
    }

    // Bare-name matching: exact name, deepest match wins.
    let mut best: Option<(ComponentInstanceIdx, usize)> = None;
    for (idx, comp) in instance.all_components() {
        if comp.name.as_str().eq_ignore_ascii_case(name) {
            let depth = component_instance_path(instance, idx).matches('/').count();
            if best.map(|(_, d)| depth >= d).unwrap_or(true) {
                best = Some((idx, depth));
            }
        }
    }
    best.map(|(idx, _)| idx)
}

/// Build a `/`-separated instance path for a component (root first).
///
/// Mirrors the `spar-analysis::component_path` helper but returns a
/// joined string suitable for FQN matching, not a Vec.
fn component_instance_path(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = Some(idx);
    while let Some(ci) = current {
        let comp = instance.component(ci);
        parts.push(comp.name.as_str().to_string());
        current = comp.parent;
    }
    parts.reverse();
    parts.join("/")
}

/// FQN-style render of a component for report output.
///
/// Uses the same `/`-separated path as the resolver so users can
/// round-trip a report's component name back into a follow-up
/// `--component` argument.
fn fqn(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    component_instance_path(instance, idx)
}

/// Parse a `Pkg::Type.Impl` root reference. Returns a [`MovesError`] on
/// malformed input rather than calling `process::exit`, so the test
/// harness can observe the failure shape.
fn parse_root_ref(s: &str) -> Result<(String, String, String), MovesError> {
    let parts: Vec<&str> = s.splitn(2, "::").collect();
    if parts.len() != 2 {
        return Err(MovesError::UnknownRoot(s.to_string()));
    }
    let pkg = parts[0].to_string();
    let type_impl: Vec<&str> = parts[1].splitn(2, '.').collect();
    if type_impl.len() != 2 {
        return Err(MovesError::UnknownRoot(s.to_string()));
    }
    Ok((pkg, type_impl[0].to_string(), type_impl[1].to_string()))
}

/// Run the full analysis suite and return its diagnostics.
///
/// Mirrors the `run_all_analyses` helper in `main.rs`; inlined here to
/// avoid a circular module reference.
fn run_all_analyses(inst: &SystemInstance) -> Vec<AnalysisDiagnostic> {
    let mut runner = spar_analysis::AnalysisRunner::new();
    runner.register_all();
    runner.run_all(inst)
}

/// Parse the model files, build the global scope, instantiate the root,
/// and return the instance plus a `(package, type) -> path` map for the
/// variant-bridge layer.
///
/// Centralised so verify and enumerate share the exact same parse +
/// instantiate pipeline; differences live in the variant scope and the
/// candidate-target enumeration respectively.
fn parse_and_instantiate(
    model_files: &[String],
    root: &str,
) -> Result<(SystemInstance, SourcePathMap), MovesError> {
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();
    let mut path_pairs: Vec<(String, Arc<ItemTree>)> = Vec::new();
    for file_path in model_files {
        let source =
            fs::read_to_string(file_path).map_err(|e| MovesError::Io(file_path.clone(), e))?;
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            let mut msg = String::new();
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                msg.push_str(&format!("{file_path}:{line}:{col}: {}\n", err.msg));
            }
            return Err(MovesError::Parse(file_path.clone(), msg));
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        let tree = spar_hir_def::file_item_tree(&db, sf);
        path_pairs.push((file_path.clone(), tree.clone()));
        trees.push(tree);
    }

    let (pkg_name, type_name, impl_name) = parse_root_ref(root)?;
    let scope = spar_hir_def::GlobalScope::from_trees(trees);
    let inst = SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );
    if inst.component_count() == 0 {
        return Err(MovesError::UnknownRoot(root.to_string()));
    }
    let source_paths = SourcePathMap::from_trees(&path_pairs);
    Ok((inst, source_paths))
}

/// Resolve the variant-context source — either implicit (`--variant
/// NAME`, shells out to rivet) or explicit (`--variant-context PATH`,
/// where `PATH` is `-` for stdin or a filesystem path) — and return
/// the parsed [`VariantContext`].
///
/// `None` is returned when neither flag was supplied, in which case
/// the run is a no-op variant-pass-through (the legacy v0.7.x path).
///
/// Mutual exclusion is enforced: passing both flags is a hard error
/// per the v1 contract's CLI section.
///
/// # Test override
///
/// The `SPAR_VARIANT_TEST_RIVET_OUTPUT` environment variable, if set
/// when `--variant NAME` is in play, replaces the rivet shell-out with
/// a direct read of the variable's value. This is the seam the
/// integration test uses to exercise the implicit-form path without
/// requiring a real rivet binary on the test runner.
fn load_variant_context(
    variant: Option<&str>,
    variant_context: Option<&str>,
) -> Result<Option<VariantContext>, MovesError> {
    match (variant, variant_context) {
        (Some(_), Some(_)) => Err(MovesError::VariantArgsConflict),
        (None, None) => Ok(None),
        (None, Some(path)) => {
            let blob = read_variant_context_file(path)?;
            VariantContext::from_json(&blob)
                .map(Some)
                .map_err(MovesError::VariantContextSchema)
        }
        (Some(name), None) => {
            // Test seam: the integration tests set this to the JSON
            // payload they want spar to read. In production this is
            // never set, so we fall through to shelling out.
            if let Ok(payload) = std::env::var("SPAR_VARIANT_TEST_RIVET_OUTPUT") {
                return VariantContext::from_json(&payload)
                    .map(Some)
                    .map_err(MovesError::VariantContextSchema);
            }
            let blob = shell_out_to_rivet(name)?;
            VariantContext::from_json(&blob)
                .map(Some)
                .map_err(MovesError::VariantContextSchema)
        }
    }
}

/// Read a `--variant-context` payload from the named source.
///
/// The path `-` reads stdin to EOF. Any other path is a filesystem
/// path; failures are reported with a context-rich error.
fn read_variant_context_file(path: &str) -> Result<String, MovesError> {
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| MovesError::VariantContextIo("<stdin>".to_string(), e))?;
        Ok(buf)
    } else {
        fs::read_to_string(path).map_err(|e| MovesError::VariantContextIo(path.to_string(), e))
    }
}

/// Shell out to `rivet resolve --variant NAME --format spar-context-json`
/// and return its stdout.
///
/// The rivet binary is located via `$RIVET_BIN` first, then via the
/// host `$PATH`. Failures map to typed [`MovesError`] variants so the
/// CLI surface emits actionable messages rather than raw OS errors.
fn shell_out_to_rivet(variant: &str) -> Result<String, MovesError> {
    let bin = match std::env::var_os("RIVET_BIN") {
        Some(v) => std::path::PathBuf::from(v),
        None => match which_rivet() {
            Some(p) => p,
            None => return Err(MovesError::RivetNotFound),
        },
    };

    let output = process::Command::new(&bin)
        .args([
            "resolve",
            "--variant",
            variant,
            "--format",
            "spar-context-json",
        ])
        .output()
        .map_err(|e| {
            // `not found` -> RivetNotFound; everything else -> IO error
            // bubble.
            if e.kind() == std::io::ErrorKind::NotFound {
                MovesError::RivetNotFound
            } else {
                MovesError::RivetIo(e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(MovesError::RivetFailed {
            stderr,
            code: output.status.code(),
        });
    }
    String::from_utf8(output.stdout).map_err(|e| {
        MovesError::RivetIo(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("rivet stdout was not UTF-8: {e}"),
        ))
    })
}

/// Best-effort lookup of `rivet` on `$PATH`. Returns `None` when no
/// `rivet` (or `rivet.exe`) is found in any `$PATH` entry.
fn which_rivet() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["rivet", "rivet.exe"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Variant-aware component resolution.
///
/// When a [`VariantScope`] is supplied, the resolver first tries to
/// match against the kept subset only — so `--component X` resolving to
/// a dropped component is reported as "not in variant" rather than
/// silently snapping to a same-named-but-kept sibling. When no scope is
/// in play we fall through to the v0.7.x [`resolve_component`] path
/// untouched.
fn resolve_component_in_scope(
    inst: &SystemInstance,
    scope: Option<&VariantScope<'_>>,
    name: &str,
) -> Option<ComponentInstanceIdx> {
    let raw = resolve_component(inst, name)?;
    if let Some(scope) = scope {
        if scope.is_kept(raw) {
            Some(raw)
        } else {
            // Dropped by the variant. The resolver's caller turns this
            // into a typed error (`ComponentNotInVariant` /
            // `TargetNotInVariant`) — we just signal "not findable".
            None
        }
    } else {
        Some(raw)
    }
}

/// Lift a "name not found" failure into the right [`MovesError`]
/// variant: when a variant scope is active the dropped-by-variant case
/// gets a more specific diagnostic so users know to check the variant
/// definition rather than the model.
fn component_not_found_error(name: &str, scope: Option<&VariantScope<'_>>) -> MovesError {
    match scope {
        Some(scope) if resolve_component(scope.instance, name).is_some() => {
            MovesError::ComponentNotInVariant {
                name: name.to_string(),
                variant: scope.variant_name().to_string(),
            }
        }
        _ => MovesError::UnknownComponent(name.to_string()),
    }
}

/// As [`component_not_found_error`] but for `--to`.
fn target_not_found_error(name: &str, scope: Option<&VariantScope<'_>>) -> MovesError {
    match scope {
        Some(scope) if resolve_component(scope.instance, name).is_some() => {
            MovesError::TargetNotInVariant {
                name: name.to_string(),
                variant: scope.variant_name().to_string(),
            }
        }
        _ => MovesError::UnknownTarget(name.to_string()),
    }
}

// ── CLI dispatch helpers ─────────────────────────────────────────────

/// Print top-level `spar moves` usage to stderr and exit non-zero.
pub fn print_moves_usage() {
    eprintln!("Usage: spar moves <subcommand> [options]");
    eprintln!();
    eprintln!("Subcommands:");
    eprintln!("  verify     Verify a hypothetical component move under the migration overlay.");
    eprintln!("  enumerate  List every valid hypothetical rebinding target for a component, with");
    eprintln!("             per-candidate verification status and optional slack ranking.");
    eprintln!();
    eprintln!("`spar moves optimize` lands in a later commit.");
}

/// Print `spar moves verify` usage to stderr and exit non-zero.
pub fn print_verify_usage() {
    eprintln!(
        "Usage: spar moves verify --root Pkg::Type.Impl --component <fqn> --to <processor> \\"
    );
    eprintln!(
        "                         [--variant NAME | --variant-context PATH] [--format text|json] \\"
    );
    eprintln!("                         <model.aadl>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --root             Root system implementation in Pkg::Type.Impl form");
    eprintln!(
        "  --component        FQN (or suffix / bare name) of the component to (hypothetically) move"
    );
    eprintln!("  --to               FQN (or suffix / bare name) of the target processor");
    eprintln!("  --format           Output format: text (default) or json");
    eprintln!(
        "  --variant          Variant NAME; spar shells out to `rivet resolve` (see contract docs)"
    );
    eprintln!(
        "  --variant-context  PATH (or '-' for stdin) of an explicit rivet variant-context blob"
    );
    eprintln!();
    eprintln!("Exit codes:");
    eprintln!("  0  move is admissible (no violations, no analysis errors)");
    eprintln!("  1  one or more analysis-error-severity diagnostics");
    eprintln!("  2  overlay violations (Frozen / Allowed_Targets)");
}

/// Manual-arg parser for `spar moves` matching the rest of `main.rs`'s
/// hand-rolled style. Returns the desired exit code.
pub fn cmd_moves(args: &[String]) -> i32 {
    if args.is_empty() {
        print_moves_usage();
        return 1;
    }
    match args[0].as_str() {
        "verify" => cmd_moves_verify(&args[1..]),
        "enumerate" => cmd_moves_enumerate(&args[1..]),
        other => {
            eprintln!("Unknown moves subcommand: {other}");
            print_moves_usage();
            1
        }
    }
}

/// Manual-arg parser for `spar moves verify`.
fn cmd_moves_verify(args: &[String]) -> i32 {
    let mut root = None;
    let mut component = None;
    let mut target = None;
    let mut format: Option<String> = None;
    let mut variant: Option<String> = None;
    let mut variant_context: Option<String> = None;
    let mut model_files = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    return 1;
                }
                root = Some(args[i].clone());
            }
            "--component" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--component requires a value");
                    return 1;
                }
                component = Some(args[i].clone());
            }
            "--to" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--to requires a value");
                    return 1;
                }
                target = Some(args[i].clone());
            }
            "--format" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--format requires a value (text|json)");
                    return 1;
                }
                format = Some(args[i].clone());
            }
            "--variant" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--variant requires a value (variant name)");
                    return 1;
                }
                variant = Some(args[i].clone());
            }
            "--variant-context" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--variant-context requires a value (path or '-' for stdin)");
                    return 1;
                }
                variant_context = Some(args[i].clone());
            }
            "--help" | "-h" => {
                print_verify_usage();
                return 0;
            }
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                print_verify_usage();
                return 1;
            }
            s => model_files.push(s.to_string()),
        }
        i += 1;
    }

    let Some(root) = root else {
        eprintln!("--root Package::Type.Impl is required");
        return 1;
    };
    let Some(component) = component else {
        eprintln!("--component is required");
        return 1;
    };
    let Some(target) = target else {
        eprintln!("--to is required");
        return 1;
    };
    if model_files.is_empty() {
        eprintln!("at least one .aadl file is required");
        return 1;
    }

    let args = VerifyArgs {
        model_files,
        root,
        component,
        target,
        format: format.unwrap_or_else(|| "text".to_string()),
        variant,
        variant_context,
    };

    match run_verify(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            // Distinguish "input does not resolve" (1) from "invalid CLI shape"
            // (also 1): both are user-fixable, so a single non-zero suffices.
            1
        }
    }
}

/// Wrapper that calls [`cmd_moves`] and exits the process with the
/// returned code. Keeps `main.rs` symmetrical with the other subcommands
/// that all end with `process::exit`.
pub fn cmd_moves_dispatch(args: &[String]) {
    process::exit(cmd_moves(args));
}

// ── enumerate (Track E commit 4/8) ──────────────────────────────────

/// Parsed CLI arguments for `spar moves enumerate`.
///
/// Same shape as [`VerifyArgs`] minus `--to` (the target is *derived*
/// per-candidate, not supplied by the user) plus an optional
/// `--target-filter` to narrow the candidate set when the model has
/// many processors and `Spar_Migration::Allowed_Targets` is absent,
/// and a multi-objective `--objective` selector that drives the
/// candidate ranking score (Track E commit 5/8).
#[derive(Debug)]
pub struct EnumerateArgs {
    /// Path(s) to the AADL model file(s) to load.
    pub model_files: Vec<String>,
    /// Root system implementation in `Pkg::Type.Impl` form.
    pub root: String,
    /// FQN (or suffix / bare name) of the component to enumerate
    /// candidates for.
    pub component: String,
    /// Optional substring (case-insensitive) that a candidate's FQN
    /// must contain to be included. Useful when `Allowed_Targets` is
    /// absent and the model has many processors.
    pub target_filter: Option<String>,
    /// Output format: `text` (default) or `json`.
    pub format: String,
    /// Multi-objective ranking spec. Default
    /// `EnumerationObjective::max_response()` — equivalent to commit 4's
    /// slack-only ranking on single-CPU models.
    pub objective: EnumerationObjective,
    /// Implicit-form variant name. See [`VerifyArgs::variant`].
    pub variant: Option<String>,
    /// Explicit-form variant-context source. See
    /// [`VerifyArgs::variant_context`].
    pub variant_context: Option<String>,
}

/// Per-candidate verification record produced by `spar moves enumerate`.
///
/// One [`MoveCandidate`] is emitted per (component, target) pair the
/// algorithm considers — the target list comes from
/// `Spar_Migration::Allowed_Targets` when set or from every Processor /
/// VirtualProcessor in the instance otherwise. The fields capture
/// whether the move would be admissible (`ok`), the structured
/// violation list, and the multi-objective rank score (Track E commit 5/8).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MoveCandidate {
    /// Fully-qualified name of the candidate target processor.
    pub target: String,
    /// True if the move is admissible: no overlay violations and no
    /// error-severity diagnostics from the analysis suite.
    pub ok: bool,
    /// Overlay + analysis violations. Same shape as
    /// [`MoveVerifyReport::violations`] for one-to-one round-trip
    /// with `spar moves verify` output.
    pub violations: Vec<Violation>,
    /// Total error-severity analysis diagnostics for this candidate
    /// (kept separately from the violation count so consumers can
    /// rank "worst offenders" without traversing `violations`).
    pub diagnostics_count: usize,
    /// Multi-objective rank computed by the solver-driven ranker
    /// (commit 5/8). Lower `rank.score` is better; the per-axis values
    /// (`max_response_ns`, `total_load`, …) stay accessible for
    /// breakdown rendering and post-hoc resorting.
    pub rank: CandidateRank,
}

/// Output shape for `spar moves enumerate --format json`.
///
/// The canonical JSON contract for the v0.9.0 MCP
/// `spar.enumerate_moves` tool surface; consumers downstream of the
/// LLM tool will sort `candidates` by `rank.score` ascending (with
/// `ok=true` first) to surface the most attractive moves.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MoveEnumerateReport {
    /// FQN of the component being enumerated.
    pub component: String,
    /// All candidate targets considered (post target-filter).
    pub candidates: Vec<MoveCandidate>,
    /// Total candidates evaluated.
    pub total: usize,
    /// Number of `ok=true` candidates (admissible moves).
    pub valid: usize,
    /// Resolved-variant name when the run was scoped by a rivet
    /// variant context per the v1 contract, otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Stable hash of the feature model that produced the variant
    /// resolution. `None` when no variant was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature_model_hash: Option<String>,
}

/// Run `spar moves enumerate`, returning the desired process exit code.
///
/// Pipeline:
///
/// 1. Parse + instantiate the model (mirrors `run_verify`).
/// 2. Resolve `--component` to a [`ComponentInstanceIdx`].
/// 3. Build the candidate-target set from
///    `Spar_Migration::Allowed_Targets` (when set) or from every
///    Processor / VirtualProcessor in the instance otherwise.
/// 4. Apply the optional `--target-filter` substring (case-insensitive
///    against the candidate's FQN).
/// 5. For each candidate: build a [`BindingOverlay::add_move`] overlay,
///    run [`BindingOverlay::validate`], run the analysis suite, and
///    derive a slack metric from RTA's info diagnostics.
/// 6. Emit a [`MoveEnumerateReport`] in `text` or `json` form.
///
/// Exit codes mirror `verify` semantics:
///
/// | Code | Meaning |
/// |---|---|
/// | 0 | At least one admissible candidate (`valid >= 1`) |
/// | 1 | All candidates failed analysis or produced no admissible move |
/// | 2 | Input resolution failed (component / model / format) |
pub fn run_enumerate(args: EnumerateArgs) -> Result<i32, MovesError> {
    let format = args.format.clone();
    let report = enumerate_pipeline(&args)?;
    let code = if report.valid > 0 { 0 } else { 1 };
    match format.as_str() {
        "json" => render_enumerate_json(&report),
        _ => render_enumerate_text(&report),
    }
    Ok(code)
}

/// Run the enumerate pipeline and return the structured report without
/// printing anything. Shared between the CLI driver and the MCP
/// `spar.enumerate_moves` tool.
pub fn enumerate_pipeline(args: &EnumerateArgs) -> Result<MoveEnumerateReport, MovesError> {
    if args.format != "text" && args.format != "json" {
        return Err(MovesError::UnknownFormat(args.format.clone()));
    }
    if args.model_files.is_empty() {
        return Err(MovesError::Parse(
            "(no files)".to_string(),
            "spar moves enumerate requires at least one .aadl file".to_string(),
        ));
    }

    // 1. Parse + instantiate.
    let (inst, source_paths) = parse_and_instantiate(&args.model_files, &args.root)?;

    // 2. Optional variant scope (see verify pipeline notes).
    let variant_ctx =
        load_variant_context(args.variant.as_deref(), args.variant_context.as_deref())?;
    let scope_holder = variant_ctx
        .as_ref()
        .map(|ctx| VariantScope::new(&inst, ctx, &source_paths));

    // 3. Resolve --component (variant-aware).
    let comp_idx = resolve_component_in_scope(&inst, scope_holder.as_ref(), &args.component)
        .ok_or_else(|| component_not_found_error(&args.component, scope_holder.as_ref()))?;

    // 4. Build the candidate-target set, intersecting with the kept
    //    subset when a variant is in play.
    let mut candidates = candidate_targets(&inst, comp_idx, args.target_filter.as_deref());
    if let Some(scope) = scope_holder.as_ref() {
        candidates.retain(|idx| scope.is_kept(*idx));
    }

    // 5-6. Verify each candidate.
    let mut report = MoveEnumerateReport {
        component: fqn(&inst, comp_idx),
        candidates: Vec::with_capacity(candidates.len()),
        total: 0,
        valid: 0,
        variant: scope_holder.as_ref().map(|s| s.variant_name().to_string()),
        feature_model_hash: scope_holder
            .as_ref()
            .map(|s| s.feature_model_hash().to_string()),
    };

    for target_idx in candidates {
        let candidate = verify_candidate(&inst, comp_idx, target_idx, &args.objective);
        if candidate.ok {
            report.valid += 1;
        }
        report.candidates.push(candidate);
    }
    report.total = report.candidates.len();

    // Sort: ok=true first, then by score ascending (lower = better,
    // matching the convention of all four single-objective modes), then
    // by FQN as a stable tie-breaker. Ranks with NaN sort *last* so a
    // degenerate model never floats above well-defined scores.
    report.candidates.sort_by(|a, b| {
        b.ok.cmp(&a.ok)
            .then_with(|| {
                a.rank
                    .score
                    .partial_cmp(&b.rank.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.target.cmp(&b.target))
    });

    Ok(report)
}

/// Build the candidate-target index list for an enumerate run.
///
/// Resolution order:
///
/// 1. If the component declares a non-empty
///    `Spar_Migration::Allowed_Targets` list, resolve each name to a
///    component index via [`resolve_component`] and keep those.
///    Names that don't resolve are silently dropped (the model would
///    have already produced a property-rule warning).
/// 2. Otherwise: collect every component with category Processor or
///    VirtualProcessor.
///
/// The optional `target_filter` is applied as a case-insensitive
/// substring match against each candidate's FQN.
fn candidate_targets(
    inst: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    target_filter: Option<&str>,
) -> Vec<ComponentInstanceIdx> {
    let props = inst.properties_for(comp_idx);
    let allowed_names = spar_hir_def::read_allowed_targets(props);

    let mut targets: Vec<ComponentInstanceIdx> = if !allowed_names.is_empty() {
        allowed_names
            .iter()
            .filter_map(|n| resolve_component(inst, n))
            .collect()
    } else {
        inst.all_components()
            .filter(|(_, c)| {
                matches!(
                    c.category,
                    ComponentCategory::Processor | ComponentCategory::VirtualProcessor
                )
            })
            .map(|(idx, _)| idx)
            .collect()
    };

    // De-duplicate while preserving order (Allowed_Targets may list
    // the same processor twice, however unlikely).
    let mut seen = std::collections::HashSet::new();
    targets.retain(|idx| seen.insert(*idx));

    if let Some(filter) = target_filter {
        let needle = filter.to_ascii_lowercase();
        targets.retain(|&idx| fqn(inst, idx).to_ascii_lowercase().contains(&needle));
    }

    targets
}

/// Run the verify pipeline for a single (component, target) pair and
/// return a [`MoveCandidate`] record.
///
/// The implementation is structurally identical to the per-move portion
/// of [`run_verify`] — overlay -> validate -> analyses -> build_report
/// — but returns a candidate-shaped record instead of printing a
/// per-move document.
///
/// Track E commit 5/8: ranking is now performed by
/// [`spar_solver::enumerate::rank_candidate`], which runs the same
/// analysis suite and also reads the `Spar_Power::Power_Budget` /
/// `Weight_Properties::Weight` properties to populate the
/// multi-objective score.
fn verify_candidate(
    inst: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    target_idx: ComponentInstanceIdx,
    objective: &EnumerationObjective,
) -> MoveCandidate {
    let mut overlay = BindingOverlay::new();
    overlay.add_move(comp_idx, target_idx);
    let overlay_diags = overlay.validate(inst);
    let analysis_diags = run_all_analyses(inst);

    let report = build_report(inst, comp_idx, target_idx, &overlay_diags, &analysis_diags);
    let diagnostics_count = analysis_diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();

    // Solver-derived multi-objective ranking. The ranker runs its own
    // analysis pass internally (so we double-pay for analyses on each
    // candidate); this matches the commit-5 spec and keeps the score
    // and `violations` lists strictly consistent — both come from the
    // same un-overlayed analysis stream the verify-pipeline produces.
    let rank = rank_candidate(inst, &overlay, objective);

    let ok = overlay_diags.is_empty()
        && !report.violations.iter().any(|v| {
            matches!(
                v,
                Violation::AnalysisError {
                    severity: SerSeverity::Error,
                    ..
                }
            )
        });

    MoveCandidate {
        target: fqn(inst, target_idx),
        ok,
        violations: report.violations,
        diagnostics_count,
        rank,
    }
}

/// Render a [`MoveEnumerateReport`] as canonical pretty-printed JSON.
fn render_enumerate_json(report: &MoveEnumerateReport) {
    println!("{}", serde_json::to_string_pretty(report).unwrap());
}

/// Render a [`MoveEnumerateReport`] in human-readable tabular form.
///
/// Layout: a one-line component header, a fixed-width row per
/// candidate (status / target / score / errs / violation summary),
/// and a `total=N valid=K` summary footer mirroring the JSON shape.
///
/// Track E commit 5/8: the `slack` column from commit 4 is replaced by
/// a `score` column (lower = better) carrying the multi-objective
/// rank value; `<missed>` is emitted for deadline-miss candidates so
/// the column stays one token wide.
fn render_enumerate_text(report: &MoveEnumerateReport) {
    let variant_prefix = match &report.variant {
        Some(v) => format!("(variant={v}) "),
        None => String::new(),
    };
    println!(
        "{}Enumerate {} ({} candidates)",
        variant_prefix, report.component, report.total,
    );
    if report.candidates.is_empty() {
        println!("  (no candidate targets)");
    } else {
        println!(
            "  {:<6} {:<40} {:>10} {:>6}  violations",
            "ok", "target", "score", "errs",
        );
        for c in &report.candidates {
            let status = if c.ok { "OK" } else { "FAIL" };
            let score_str = format_score(&c.rank);
            let viol_summary = summarise_violations(&c.violations);
            println!(
                "  {:<6} {:<40} {:>10} {:>6}  {}",
                status, c.target, score_str, c.diagnostics_count, viol_summary,
            );
        }
    }
    println!("total={} valid={}", report.total, report.valid);
}

/// Render a [`CandidateRank::score`] as a fixed-precision token for the
/// text-format renderer. Deadline-miss candidates (negative
/// `max_response_ns` sentinel) get a `<missed>` token instead so the
/// column stays one word wide and is greppable.
fn format_score(rank: &CandidateRank) -> String {
    if matches!(rank.max_response_ns, Some(n) if n < 0) {
        return "<missed>".to_string();
    }
    format!("{:.4}", rank.score)
}

/// Compress a [`Violation`] vector into a one-line summary for the
/// text-format renderer (so a wide table stays one row per candidate).
fn summarise_violations(violations: &[Violation]) -> String {
    if violations.is_empty() {
        return "none".to_string();
    }
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for v in violations {
        let key = match v {
            Violation::Frozen { .. } => "Frozen",
            Violation::AllowedTargets { .. } => "AllowedTargets",
            Violation::AnalysisError { .. } => "AnalysisError",
        };
        *counts.entry(key).or_default() += 1;
    }
    counts
        .iter()
        .map(|(k, n)| format!("{k}×{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Print `spar moves enumerate` usage to stderr.
pub fn print_enumerate_usage() {
    eprintln!("Usage: spar moves enumerate --root Pkg::Type.Impl --component <fqn> \\");
    eprintln!("                            [--target-filter <substring>] [--objective <mode>] \\");
    eprintln!("                            [--format text|json] \\");
    eprintln!("                            [--variant NAME | --variant-context PATH] \\");
    eprintln!("                            <model.aadl>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --root             Root system implementation in Pkg::Type.Impl form");
    eprintln!("  --component        FQN (or suffix / bare name) of the component to enumerate");
    eprintln!("  --target-filter    Optional case-insensitive substring filter on candidate FQNs");
    eprintln!(
        "  --objective        Ranking objective: max-response (default), total-load, total-power,"
    );
    eprintln!("                     total-weight, or balanced (all four equally weighted)");
    eprintln!("  --format           Output format: text (default) or json");
    eprintln!(
        "  --variant          Variant NAME; spar shells out to `rivet resolve` (see contract docs)"
    );
    eprintln!(
        "  --variant-context  PATH (or '-' for stdin) of an explicit rivet variant-context blob"
    );
    eprintln!();
    eprintln!("Candidate-target set:");
    eprintln!(
        "  - If the component declares Spar_Migration::Allowed_Targets, those names are used."
    );
    eprintln!("  - Otherwise every Processor / VirtualProcessor in the instance is a candidate.");
    eprintln!();
    eprintln!("Exit codes:");
    eprintln!("  0  at least one candidate is admissible (valid >= 1)");
    eprintln!("  1  no admissible candidate (or input-resolution error)");
}

/// Parse the `--objective` CLI value into an [`EnumerationObjective`].
///
/// Recognised modes (case-insensitive, dashed form):
///
/// - `max-response` — single-axis: minimise max response time.
/// - `total-load`   — single-axis: minimise total CPU utilisation.
/// - `total-power`  — single-axis: minimise total power (Spar_Power).
/// - `total-weight` — single-axis: minimise total weight.
/// - `balanced`     — all four axes equally weighted.
///
/// Returns [`MovesError::UnknownObjective`] for any other input so the
/// CLI driver can surface a clear message and a non-zero exit.
pub fn parse_objective(s: &str) -> Result<EnumerationObjective, MovesError> {
    match s.to_ascii_lowercase().as_str() {
        "max-response" => Ok(EnumerationObjective::max_response()),
        "total-load" => Ok(EnumerationObjective::total_load()),
        "total-power" => Ok(EnumerationObjective::total_power()),
        "total-weight" => Ok(EnumerationObjective::total_weight()),
        "balanced" => Ok(EnumerationObjective::balanced()),
        _ => Err(MovesError::UnknownObjective(s.to_string())),
    }
}

/// Manual-arg parser for `spar moves enumerate`.
fn cmd_moves_enumerate(args: &[String]) -> i32 {
    let mut root = None;
    let mut component = None;
    let mut target_filter: Option<String> = None;
    let mut format: Option<String> = None;
    let mut objective_str: Option<String> = None;
    let mut variant: Option<String> = None;
    let mut variant_context: Option<String> = None;
    let mut model_files = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    return 1;
                }
                root = Some(args[i].clone());
            }
            "--component" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--component requires a value");
                    return 1;
                }
                component = Some(args[i].clone());
            }
            "--target-filter" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--target-filter requires a value");
                    return 1;
                }
                target_filter = Some(args[i].clone());
            }
            "--objective" => {
                i += 1;
                if i >= args.len() {
                    eprintln!(
                        "--objective requires a value (max-response | total-load | \
                         total-power | total-weight | balanced)"
                    );
                    return 1;
                }
                objective_str = Some(args[i].clone());
            }
            "--format" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--format requires a value (text|json)");
                    return 1;
                }
                format = Some(args[i].clone());
            }
            "--variant" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--variant requires a value (variant name)");
                    return 1;
                }
                variant = Some(args[i].clone());
            }
            "--variant-context" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--variant-context requires a value (path or '-' for stdin)");
                    return 1;
                }
                variant_context = Some(args[i].clone());
            }
            "--help" | "-h" => {
                print_enumerate_usage();
                return 0;
            }
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                print_enumerate_usage();
                return 1;
            }
            s => model_files.push(s.to_string()),
        }
        i += 1;
    }

    let Some(root) = root else {
        eprintln!("--root Package::Type.Impl is required");
        return 1;
    };
    let Some(component) = component else {
        eprintln!("--component is required");
        return 1;
    };
    if model_files.is_empty() {
        eprintln!("at least one .aadl file is required");
        return 1;
    }

    let objective = match objective_str.as_deref() {
        None => EnumerationObjective::max_response(),
        Some(s) => match parse_objective(s) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        },
    };

    let args = EnumerateArgs {
        model_files,
        root,
        component,
        target_filter,
        format: format.unwrap_or_else(|| "text".to_string()),
        objective,
        variant,
        variant_context,
    };

    match run_enumerate(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod enumerate_tests {
    use super::*;

    #[test]
    fn summarise_violations_groups_by_kind() {
        let v = vec![
            Violation::Frozen {
                component: "t1".to_string(),
                reason: "x".to_string(),
            },
            Violation::Frozen {
                component: "t2".to_string(),
                reason: "y".to_string(),
            },
            Violation::AnalysisError {
                pass: "RTA".to_string(),
                message: "miss".to_string(),
                severity: SerSeverity::Error,
                path: vec![],
            },
        ];
        let s = summarise_violations(&v);
        // Order is BTreeMap so AnalysisError comes before Frozen alphabetically.
        assert!(s.contains("AnalysisError×1"));
        assert!(s.contains("Frozen×2"));
    }

    #[test]
    fn parse_objective_recognises_named_modes() {
        assert!(matches!(
            parse_objective("max-response"),
            Ok(o) if o == EnumerationObjective::max_response()
        ));
        assert!(matches!(
            parse_objective("total-load"),
            Ok(o) if o == EnumerationObjective::total_load()
        ));
        assert!(matches!(
            parse_objective("total-power"),
            Ok(o) if o == EnumerationObjective::total_power()
        ));
        assert!(matches!(
            parse_objective("total-weight"),
            Ok(o) if o == EnumerationObjective::total_weight()
        ));
        assert!(matches!(
            parse_objective("balanced"),
            Ok(o) if o == EnumerationObjective::balanced()
        ));
    }

    #[test]
    fn parse_objective_rejects_unknown() {
        let err = parse_objective("not-a-real-mode").unwrap_err();
        assert!(matches!(err, MovesError::UnknownObjective(s) if s == "not-a-real-mode"));
    }
}
