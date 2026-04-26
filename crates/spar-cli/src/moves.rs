//! `spar moves verify` — hypothetical-rebinding oracle (Track E commit 3/8).
//!
//! Per the v0.8.0 migration design research §6.3, this module exposes the
//! first user-facing surface of the migration oracle: a CLI that takes a
//! parsed AADL model, a component to (hypothetically) move, and a target
//! processor, and returns a structured pass/fail report describing whether
//! the move is admissible under the platform/application split and the
//! existing analysis-pass suite.
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
use std::process;

use serde::Serialize;

use spar_analysis::{AnalysisDiagnostic, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::{AllowedTargetsViolation, BindingOverlay, FrozenViolation, OverlayDiagnostic};

/// Parsed CLI arguments for `spar moves verify`.
///
/// Populated by the manual-arg-parsing path in [`run_verify`]; mirrors
/// the design-research-style clap struct in track-e-migration-research §6.3
/// without dragging clap into spar-cli (which still uses hand-rolled
/// `args[i]` matching for every other subcommand).
#[derive(Debug)]
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

/// Run `spar moves verify`, returning the desired process exit code.
///
/// See module docs for the full pipeline; a zero return from this
/// function means the move is admissible. The caller in `main.rs`
/// passes the return through `process::exit` directly so behaviour is
/// observable to a shell or harness.
pub fn run_verify(args: VerifyArgs) -> Result<i32, MovesError> {
    if args.format != "text" && args.format != "json" {
        return Err(MovesError::UnknownFormat(args.format));
    }
    if args.model_files.is_empty() {
        return Err(MovesError::Parse(
            "(no files)".to_string(),
            "spar moves verify requires at least one .aadl file".to_string(),
        ));
    }

    // 1. Parse + instantiate. We mirror the path used by `spar analyze`,
    //    short-circuiting the same way on parse errors so users see a
    //    diagnostic rather than a stack trace.
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();
    for file_path in &args.model_files {
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
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    let (pkg_name, type_name, impl_name) = parse_root_ref(&args.root)?;
    let scope = spar_hir_def::GlobalScope::from_trees(trees);
    let inst = SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );
    if inst.component_count() == 0 {
        return Err(MovesError::UnknownRoot(args.root.clone()));
    }

    // 2. Resolve component + target FQNs.
    let comp_idx = resolve_component(&inst, &args.component)
        .ok_or_else(|| MovesError::UnknownComponent(args.component.clone()))?;
    let target_idx = resolve_component(&inst, &args.target)
        .ok_or_else(|| MovesError::UnknownTarget(args.target.clone()))?;
    let target_cat = inst.component(target_idx).category;
    if target_cat != ComponentCategory::Processor
        && target_cat != ComponentCategory::VirtualProcessor
    {
        return Err(MovesError::TargetNotProcessor {
            target: args.target.clone(),
            category: target_cat,
        });
    }

    // 3. Build the overlay and validate against the platform / application split.
    let mut overlay = BindingOverlay::new();
    overlay.add_move(comp_idx, target_idx);
    let overlay_diags = overlay.validate(&inst);

    // 4. Run the analysis suite.
    //
    //    Per commit 3 scope: the suite reads the un-overlayed instance.
    //    The overlay still surfaces its own constraint-layer
    //    diagnostics (frozen / allowed-targets) so a user moving a
    //    pinned component sees an immediate red flag rather than a
    //    silent green from analyses that are not overlay-aware yet.
    //    Commit 4 widens overlay awareness to RTA + bandwidth + latency
    //    + EMV2 + ARINC653 so the diagnostics reflect the hypothetical
    //    binding rather than the declared one.
    let analysis_diags = run_all_analyses(&inst);

    // 5. Build the structured report.
    let report = build_report(&inst, comp_idx, target_idx, &overlay_diags, &analysis_diags);

    // 6. Render.
    match args.format.as_str() {
        "json" => render_json(&report),
        _ => render_text(&report),
    }

    // 7. Compute exit code.
    Ok(exit_code_for(&report, &overlay_diags))
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
    println!("{} move {} -> {}", status, report.component, report.target);

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

// ── CLI dispatch helpers ─────────────────────────────────────────────

/// Print top-level `spar moves` usage to stderr and exit non-zero.
pub fn print_moves_usage() {
    eprintln!("Usage: spar moves <subcommand> [options]");
    eprintln!();
    eprintln!("Subcommands:");
    eprintln!("  verify   Verify a hypothetical component move under the migration overlay.");
    eprintln!();
    eprintln!("`spar moves enumerate` and `spar moves optimize` land in later commits.");
}

/// Print `spar moves verify` usage to stderr and exit non-zero.
pub fn print_verify_usage() {
    eprintln!(
        "Usage: spar moves verify --root Pkg::Type.Impl --component <fqn> --to <processor> \\"
    );
    eprintln!("                         [--format text|json] <model.aadl>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --root        Root system implementation in Pkg::Type.Impl form");
    eprintln!(
        "  --component   FQN (or suffix / bare name) of the component to (hypothetically) move"
    );
    eprintln!("  --to          FQN (or suffix / bare name) of the target processor");
    eprintln!("  --format      Output format: text (default) or json");
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
