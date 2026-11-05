//! REQ-ANALYSIS-LATENCY-BEST-CASE-INCOMPLETE-001 (#516): a best-case latency
//! summed while some flow elements carry no timing does not raise the
//! below-minimum Error.
//!
//! In test-data/osate2/CasePositionControl.aadl, flow `smcmdflow` has a
//! `50 ms .. 100 ms` budget, and four of its components (ECS, SM_PCS, SM_ACT,
//! SM) have no timing. Up to v0.48.1 the best case summed to 0.000 ms and
//! spar reported `[ERROR] ... best-case latency 0.000 ms is below the declared
//! minimum 50.000 ms`, although the flow's real best case is unknown.

use std::path::PathBuf;
use std::process::Command;

fn latency_lines() -> Vec<String> {
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/osate2/CasePositionControl.aadl");
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "CasePositionControl::SMS_Operational_Environment.Position",
            model.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Instantiated "), "must instantiate: {text}");
    text.lines()
        .filter(|l| l.contains("latency:") && l.contains("'smcmdflow'"))
        .map(str::to_string)
        .collect()
}

// rivet: verifies REQ-ANALYSIS-LATENCY-BEST-CASE-INCOMPLETE-001
#[test]
fn an_incomplete_best_case_is_not_a_below_minimum_error() {
    let lines = latency_lines();
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("[ERROR]") && l.contains("below the declared minimum")),
        "{lines:#?}"
    );
}

// rivet: verifies REQ-ANALYSIS-LATENCY-BEST-CASE-INCOMPLETE-001
#[test]
fn an_incomplete_best_case_is_a_warning_naming_the_untimed_components() {
    let lines = latency_lines();
    let warning: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("[WARNING]") && l.contains("INCOMPLETE"))
        .collect();
    assert_eq!(warning.len(), 1, "{lines:#?}");
    assert!(
        warning[0].contains(
            "best-case latency 0.000 ms is below the declared minimum 50.000 ms, but it is \
             INCOMPLETE: ECS, SM_PCS, SM_ACT, SM carry no timing"
        ),
        "{lines:#?}"
    );
}
