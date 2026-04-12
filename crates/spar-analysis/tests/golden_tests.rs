//! Golden / snapshot tests for spar-analysis diagnostic output.
//!
//! Each test loads an AADL model from `test-data/golden/`, instantiates
//! the root system, runs all analyses, and verifies that the expected
//! diagnostic messages appear.

use spar_analysis::{AnalysisDiagnostic, AnalysisRunner, Severity};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

// ── Helpers ──────────────────────────────────��─────────────────────

/// Build a `SystemInstance` from inline AADL text.
fn build_instance(aadl: &str, pkg: &str, typ: &str, imp: &str) -> SystemInstance {
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), aadl.to_string());
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    SystemInstance::instantiate(&scope, &Name::new(pkg), &Name::new(typ), &Name::new(imp))
}

/// Run all analyses on a `SystemInstance` and return diagnostics.
fn run_all_analyses(instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
    let mut runner = AnalysisRunner::new();
    runner.register_all();
    runner.run_all_per_som(instance)
}

/// Filter diagnostics by severity.
fn by_severity(diags: &[AnalysisDiagnostic], sev: Severity) -> Vec<&AnalysisDiagnostic> {
    diags.iter().filter(|d| d.severity == sev).collect()
}

/// Filter diagnostics whose message contains a substring.
fn containing<'a>(diags: &'a [AnalysisDiagnostic], sub: &str) -> Vec<&'a AnalysisDiagnostic> {
    diags.iter().filter(|d| d.message.contains(sub)).collect()
}

/// Filter diagnostics from a specific analysis.
fn from_analysis<'a>(
    diags: &'a [AnalysisDiagnostic],
    analysis: &str,
) -> Vec<&'a AnalysisDiagnostic> {
    diags.iter().filter(|d| d.analysis == analysis).collect()
}

// ── Golden test: simple_timing.aadl ────────────────────────────���──

#[test]
fn golden_simple_timing_instantiates() {
    let aadl = include_str!("../../../test-data/golden/simple_timing.aadl");
    let inst = build_instance(aadl, "SimpleTiming", "Top", "impl");
    let root = inst.component(inst.root);
    assert_eq!(
        root.category,
        spar_hir_def::item_tree::ComponentCategory::System
    );
    assert!(
        inst.component_count() >= 4,
        "expected at least 4 components (top, cpu, app, threads), got {}",
        inst.component_count()
    );
}

#[test]
fn golden_simple_timing_scheduling_diagnostics() {
    let aadl = include_str!("../../../test-data/golden/simple_timing.aadl");
    let inst = build_instance(aadl, "SimpleTiming", "Top", "impl");
    let diags = run_all_analyses(&inst);

    let sched = from_analysis(&diags, "scheduling");
    assert!(
        !sched.is_empty(),
        "scheduling analysis should produce diagnostics for timing model"
    );

    // Logger_Thread has no Period -- should produce a warning.
    let no_period = containing(&diags, "no Period");
    assert!(
        !no_period.is_empty(),
        "expected warning about missing Period property, got: {:?}",
        sched,
    );
}

#[test]
fn golden_simple_timing_no_false_errors() {
    let aadl = include_str!("../../../test-data/golden/simple_timing.aadl");
    let inst = build_instance(aadl, "SimpleTiming", "Top", "impl");
    let diags = run_all_analyses(&inst);

    // Sensor_Thread has valid timing -- should not cause scheduling errors.
    let sched_errors: Vec<_> = diags
        .iter()
        .filter(|d| d.analysis == "scheduling" && d.severity == Severity::Error)
        .collect();
    // No overload errors expected (utilization is well within bounds).
    assert!(
        sched_errors.is_empty(),
        "scheduling should have no errors for well-defined threads: {:?}",
        sched_errors,
    );
}

// ── Golden test: memory_budget.aadl ───────────────────────────────

#[test]
fn golden_memory_budget_instantiates() {
    let aadl = include_str!("../../../test-data/golden/memory_budget.aadl");
    let inst = build_instance(aadl, "MemoryBudget", "Top", "impl");
    let root = inst.component(inst.root);
    assert_eq!(
        root.category,
        spar_hir_def::item_tree::ComponentCategory::System
    );
}

#[test]
fn golden_memory_budget_diagnostics() {
    let aadl = include_str!("../../../test-data/golden/memory_budget.aadl");
    let inst = build_instance(aadl, "MemoryBudget", "Top", "impl");
    let diags = run_all_analyses(&inst);

    let mem = from_analysis(&diags, "memory_budget");

    // The model has 100+50+150+80 = 380 KByte demand vs 256 KByte capacity.
    // This should trigger a budget exceeded error.
    let budget_exceeded = mem
        .iter()
        .filter(|d| d.severity == Severity::Error && d.message.contains("exceeded"))
        .count();
    assert!(
        budget_exceeded >= 1,
        "expected memory budget exceeded error (380 KB > 256 KB), got: {:?}",
        mem,
    );
}

#[test]
fn golden_memory_budget_mentions_memory_name() {
    let aadl = include_str!("../../../test-data/golden/memory_budget.aadl");
    let inst = build_instance(aadl, "MemoryBudget", "Top", "impl");
    let diags = run_all_analyses(&inst);

    let mem = from_analysis(&diags, "memory_budget");
    let mentions_ram = mem.iter().any(|d| d.message.contains("ram"));
    assert!(
        mentions_ram,
        "memory budget diagnostics should mention the memory name 'ram': {:?}",
        mem,
    );
}

// ── Golden test: connectivity.aadl ────────────────────────────────

#[test]
fn golden_connectivity_instantiates() {
    let aadl = include_str!("../../../test-data/golden/connectivity.aadl");
    let inst = build_instance(aadl, "Connectivity", "Top", "impl");
    let root = inst.component(inst.root);
    assert_eq!(
        root.category,
        spar_hir_def::item_tree::ComponentCategory::System
    );
}

#[test]
fn golden_connectivity_unconnected_port_warnings() {
    let aadl = include_str!("../../../test-data/golden/connectivity.aadl");
    let inst = build_instance(aadl, "Connectivity", "Top", "impl");
    let diags = run_all_analyses(&inst);

    let conn = from_analysis(&diags, "connectivity");
    let warnings = conn
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect::<Vec<_>>();

    // sensor.status (out, unconnected), controller.cmd_out (out, unconnected),
    // actuator.cmd_in (in, unconnected), actuator.feedback (out, unconnected)
    assert!(
        warnings.len() >= 3,
        "expected at least 3 unconnected port warnings, got {}: {:?}",
        warnings.len(),
        warnings,
    );

    // Check that unconnected ports are specifically named.
    let unconnected_msgs: Vec<&str> = warnings.iter().map(|d| d.message.as_str()).collect();
    let has_input_warning = unconnected_msgs
        .iter()
        .any(|m| m.contains("no incoming connection"));
    let has_output_warning = unconnected_msgs
        .iter()
        .any(|m| m.contains("no outgoing connection"));
    assert!(
        has_input_warning,
        "expected at least one 'no incoming connection' warning: {:?}",
        unconnected_msgs,
    );
    assert!(
        has_output_warning,
        "expected at least one 'no outgoing connection' warning: {:?}",
        unconnected_msgs,
    );
}

#[test]
fn golden_connectivity_connected_ports_not_warned() {
    let aadl = include_str!("../../../test-data/golden/connectivity.aadl");
    let inst = build_instance(aadl, "Connectivity", "Top", "impl");
    let diags = run_all_analyses(&inst);

    let conn = from_analysis(&diags, "connectivity");

    // data_out and data_in are connected via c1 -- should NOT be flagged.
    let false_positive: Vec<_> = conn
        .iter()
        .filter(|d| {
            (d.message.contains("'data_out'") && d.message.contains("no outgoing"))
                || (d.message.contains("'data_in'") && d.message.contains("no incoming"))
        })
        .collect();
    assert!(
        false_positive.is_empty(),
        "connected ports should not be flagged: {:?}",
        false_positive,
    );
}

// ── Golden test: modes.aadl ──────────────────────────────────────

#[test]
fn golden_modes_instantiates() {
    let aadl = include_str!("../../../test-data/golden/modes.aadl");
    let inst = build_instance(aadl, "ModalSystem", "Top", "impl");
    let root = inst.component(inst.root);
    assert_eq!(
        root.category,
        spar_hir_def::item_tree::ComponentCategory::System
    );
}

#[test]
fn golden_modes_no_initial_mode_error() {
    let aadl = include_str!("../../../test-data/golden/modes.aadl");
    let inst = build_instance(aadl, "ModalSystem", "Top", "impl");
    let diags = run_all_analyses(&inst);

    // BadModal has modes but no initial mode -- should trigger an error.
    let mode_errors: Vec<_> = diags
        .iter()
        .filter(|d| {
            (d.analysis == "mode_check" || d.analysis == "modal_rules")
                && d.severity == Severity::Error
        })
        .collect();
    assert!(
        !mode_errors.is_empty(),
        "expected error about missing initial mode in BadModal, got: {:?}",
        from_analysis(&diags, "mode_check"),
    );

    let mentions_initial = mode_errors.iter().any(|d| d.message.contains("initial"));
    assert!(
        mentions_initial,
        "modal error should mention 'initial': {:?}",
        mode_errors,
    );
}

#[test]
fn golden_modes_valid_controller_no_extra_errors() {
    let aadl = include_str!("../../../test-data/golden/modes.aadl");
    let inst = build_instance(aadl, "ModalSystem", "Top", "impl");
    let diags = run_all_analyses(&inst);

    // Controller has a proper initial mode and valid transitions.
    // Mode check errors should only come from BadModal, not Controller.
    let ctrl_errors: Vec<_> = diags
        .iter()
        .filter(|d| {
            (d.analysis == "mode_check" || d.analysis == "modal_rules")
                && d.severity == Severity::Error
                && d.path.iter().any(|p| p.contains("ctrl"))
        })
        .collect();
    assert!(
        ctrl_errors.is_empty(),
        "Controller with valid modes should have no mode errors: {:?}",
        ctrl_errors,
    );
}

// ── Cross-cutting golden test: all models produce diagnostics ─────

#[test]
fn golden_all_models_produce_nonempty_diagnostics() {
    let models = [
        (
            include_str!("../../../test-data/golden/simple_timing.aadl"),
            "SimpleTiming",
        ),
        (
            include_str!("../../../test-data/golden/memory_budget.aadl"),
            "MemoryBudget",
        ),
        (
            include_str!("../../../test-data/golden/connectivity.aadl"),
            "Connectivity",
        ),
        (
            include_str!("../../../test-data/golden/modes.aadl"),
            "ModalSystem",
        ),
    ];

    for (aadl, pkg) in &models {
        let inst = build_instance(aadl, pkg, "Top", "impl");
        let diags = run_all_analyses(&inst);
        assert!(
            !diags.is_empty(),
            "model '{}' should produce at least one diagnostic",
            pkg,
        );
    }
}

#[test]
fn golden_all_diagnostics_have_analysis_field() {
    let models = [
        (
            include_str!("../../../test-data/golden/simple_timing.aadl"),
            "SimpleTiming",
        ),
        (
            include_str!("../../../test-data/golden/memory_budget.aadl"),
            "MemoryBudget",
        ),
        (
            include_str!("../../../test-data/golden/connectivity.aadl"),
            "Connectivity",
        ),
        (
            include_str!("../../../test-data/golden/modes.aadl"),
            "ModalSystem",
        ),
    ];

    for (aadl, pkg) in &models {
        let inst = build_instance(aadl, pkg, "Top", "impl");
        let diags = run_all_analyses(&inst);
        for diag in &diags {
            assert!(
                !diag.analysis.is_empty(),
                "diagnostic in model '{}' has empty analysis field: {:?}",
                pkg,
                diag,
            );
        }
    }
}

#[test]
fn golden_severity_distribution_sanity() {
    // Quick sanity check that we see a mix of severity levels across all models.
    let models = [
        (
            include_str!("../../../test-data/golden/simple_timing.aadl"),
            "SimpleTiming",
        ),
        (
            include_str!("../../../test-data/golden/memory_budget.aadl"),
            "MemoryBudget",
        ),
        (
            include_str!("../../../test-data/golden/connectivity.aadl"),
            "Connectivity",
        ),
        (
            include_str!("../../../test-data/golden/modes.aadl"),
            "ModalSystem",
        ),
    ];

    let mut all_diags = Vec::new();
    for (aadl, pkg) in &models {
        let inst = build_instance(aadl, pkg, "Top", "impl");
        all_diags.extend(run_all_analyses(&inst));
    }

    let errors = by_severity(&all_diags, Severity::Error);
    let warnings = by_severity(&all_diags, Severity::Warning);

    assert!(
        !errors.is_empty(),
        "expected at least one error across all golden models"
    );
    assert!(
        !warnings.is_empty(),
        "expected at least one warning across all golden models"
    );
}
