//! End-to-end tests on synthetic CTF fixtures + a small AADL model
//! that declares `Spar_Trace::Expected_*`.

use pretty_assertions::assert_eq;
use spar_hir_def::GlobalScope;
use spar_hir_def::HirDefDatabase;
use spar_hir_def::Name;
use spar_hir_def::file_item_tree;
use spar_hir_def::instance::SystemInstance;
use spar_insight::{DiscrepancyKind, DiscrepancySeverity, analyze, parse_ctf};

const MODEL_AADL: &str = r#"
package Demo
public

with Spar_Trace;

system Sys
end Sys;

system implementation Sys.Impl
subcomponents
  brake : process Brake.Impl;
end Sys.Impl;

process Brake
end Brake;

process implementation Brake.Impl
properties
  Spar_Trace::Probe_Point => true;
  Spar_Trace::Expected_BCET => 100 ns;
  Spar_Trace::Expected_WCET => 500 ns;
  Spar_Trace::Expected_Mean => 300 ns;
end Brake.Impl;

end Demo;
"#;

fn build_instance() -> SystemInstance {
    let db = HirDefDatabase::default();
    let parsed = spar_syntax::parse(MODEL_AADL);
    assert!(parsed.ok(), "model parse failed: {:?}", parsed.errors());
    let sf = spar_base_db::SourceFile::new(&db, "demo.aadl".to_string(), MODEL_AADL.to_string());
    let tree = file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    SystemInstance::instantiate(
        &scope,
        &Name::new("Demo"),
        &Name::new("Sys"),
        &Name::new("Impl"),
    )
}

#[test]
fn end_to_end_clean_trace_no_discrepancies() {
    let instance = build_instance();
    // Three matched samples around the expected mean (300ns), inside the
    // BCET=100ns / WCET=500ns envelope.
    let stream = "
        1000: probe_point_enter(probe_id=\"brake\")
        1300: probe_point_exit(probe_id=\"brake\")
        2000: probe_point_enter(probe_id=\"brake\")
        2280: probe_point_exit(probe_id=\"brake\")
        3000: probe_point_enter(probe_id=\"brake\")
        3320: probe_point_exit(probe_id=\"brake\")
    ";
    let evs = parse_ctf(stream).unwrap();
    let report = analyze(&evs, &instance);
    assert!(
        report.discrepancies.is_empty(),
        "expected no discrepancies, got: {:#?}",
        report.discrepancies
    );
    assert!(!report.has_errors());
    assert_eq!(report.coverage.matched.len(), 1);
    assert!(report.coverage.matched[0].ends_with("brake"));
}

#[test]
fn end_to_end_wcet_violation_emits_error() {
    let instance = build_instance();
    // 600ns sample violates Expected_WCET=500ns.
    let stream = "
        1000: probe_point_enter(probe_id=\"brake\")
        1600: probe_point_exit(probe_id=\"brake\")
    ";
    let evs = parse_ctf(stream).unwrap();
    let report = analyze(&evs, &instance);
    assert!(report.has_errors(), "expected an Error discrepancy");
    assert!(
        report.discrepancies.iter().any(|d| matches!(
            d.kind,
            DiscrepancyKind::WcetViolated {
                observed_max_ns: 600,
                expected_wcet_ns: 500
            }
        )),
        "expected WcetViolated 600/500, got {:#?}",
        report.discrepancies
    );
    let err = report
        .discrepancies
        .iter()
        .find(|d| matches!(d.kind, DiscrepancyKind::WcetViolated { .. }))
        .unwrap();
    assert_eq!(err.severity, DiscrepancySeverity::Error);
}

#[test]
fn end_to_end_unobserved_probe_emits_warn() {
    let instance = build_instance();
    // Trace exists but never enters `brake`.
    let stream = "
        100: k_sem_give(sem=0x1)
        200: k_sem_take(sem=0x1)
    ";
    let evs = parse_ctf(stream).unwrap();
    let report = analyze(&evs, &instance);
    assert!(
        report
            .discrepancies
            .iter()
            .any(|d| matches!(d.kind, DiscrepancyKind::UnobservedProbe)),
        "expected UnobservedProbe, got {:#?}",
        report.discrepancies
    );
    assert!(
        report
            .coverage
            .unobserved
            .iter()
            .any(|p| p.ends_with("brake")),
        "unobserved={:?}",
        report.coverage.unobserved,
    );
    // Trace summary should pick up the kernel events.
    assert_eq!(report.trace_summary.kernel_event_count, 2);
}

#[test]
fn end_to_end_missing_probe_emits_info() {
    let instance = build_instance();
    // Trace probes a probe-id ("ghost") the model doesn't declare.
    let stream = "
        100: probe_point_enter(probe_id=\"ghost\")
        200: probe_point_exit(probe_id=\"ghost\")
        # also produce one matched sample for `brake` so we don't trip
        # UnobservedProbe.
        300: probe_point_enter(probe_id=\"brake\")
        500: probe_point_exit(probe_id=\"brake\")
    ";
    let evs = parse_ctf(stream).unwrap();
    let report = analyze(&evs, &instance);
    assert!(
        report
            .discrepancies
            .iter()
            .any(|d| matches!(d.kind, DiscrepancyKind::MissingProbe) && d.probe_id == "ghost"),
        "expected MissingProbe for `ghost`, got {:#?}",
        report.discrepancies
    );
    let info = report
        .discrepancies
        .iter()
        .find(|d| matches!(d.kind, DiscrepancyKind::MissingProbe))
        .unwrap();
    assert_eq!(info.severity, DiscrepancySeverity::Info);
}

#[test]
fn report_renders_json_and_text() {
    let instance = build_instance();
    let evs = parse_ctf(
        "
        1000: probe_point_enter(probe_id=\"brake\")
        1600: probe_point_exit(probe_id=\"brake\")
        ",
    )
    .unwrap();
    let report = analyze(&evs, &instance);
    let j = report.to_json();
    assert!(j.contains("\"discrepancies\""));
    assert!(j.contains("wcet_violated"));
    let t = report.to_text();
    assert!(t.contains("error"));
    assert!(t.contains("brake"));
}
