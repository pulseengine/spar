//! Integration tests for the EMV2 parser against real-world OSATE2 test files.

use spar_annex::emv2;

fn parse_ok(source: &str) -> emv2::Emv2Parse {
    let result = emv2::parse(source);
    assert!(
        result.ok(),
        "parse errors: {:?}",
        result.errors()
    );
    // Verify lossless round-trip
    let root = result.syntax_node();
    assert_eq!(root.text().to_string(), source, "lossless round-trip failed");
    result
}

fn parse_file(name: &str) -> emv2::Emv2Parse {
    let path = format!(
        "{}/test-data/emv2/{}",
        env!("CARGO_MANIFEST_DIR").trim_end_matches("/crates/spar-annex"),
        name
    );
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    parse_ok(&source)
}

// ── Library files ────────────────────────────────────────────────

#[test]
fn error_library() {
    let result = parse_file("error_library.emv2");
    let root = result.syntax_node();
    assert_eq!(root.kind(), emv2::Emv2Kind::EMV2_ROOT);
}

#[test]
fn type_mappings() {
    parse_file("type_mappings.emv2");
}

#[test]
fn type_transformations() {
    parse_file("type_transformations.emv2");
}

// ── Subclause files ──────────────────────────────────────────────

#[test]
fn common_error_sensor() {
    parse_file("common_error_sensor.emv2");
}

#[test]
fn common_error_computing() {
    parse_file("common_error_computing.emv2");
}

#[test]
fn common_error_actuator() {
    parse_file("common_error_actuator.emv2");
}

#[test]
fn common_error_main() {
    parse_file("common_error_main.emv2");
}

#[test]
fn fta2_system() {
    parse_file("fta2_system.emv2");
}

#[test]
fn fta2_impl() {
    parse_file("fta2_impl.emv2");
}

#[test]
fn fta2_sensor() {
    parse_file("fta2_sensor.emv2");
}

// ── Branching transitions with properties ────────────────────────

#[test]
fn branch_btcu_error_root() {
    parse_file("branch_btcu_error_root.emv2");
}

#[test]
fn branch_btcu_impl() {
    parse_file("branch_btcu_impl.emv2");
}

#[test]
fn branch_io() {
    parse_file("branch_io.emv2");
}

// ── Composite state (multi-section file, split for testing) ──────

#[test]
fn composite_state_behavior_definition() {
    // Section 1: error behavior definition (library form)
    let src = "\
error behavior EB
use types ErrorLibrary;
\tevents
\t\tPoorValue: error event;
\t\tNoValue: error event;
\tstates
\t\tOperational: initial state;
\t\tOperationalNonCritical: state {CommonErrors};
\t\tFailedState: state {CommonErrors};
\ttransitions
\t\ttran1: Operational -[NoValue]->FailedState{ItemOmission};
end behavior;";
    parse_ok(src);
}

#[test]
fn composite_state_subclause() {
    // Section 2: system implementation subclause with composite error behavior
    let src = "\
use types ErrorLibrary;
use behavior composite_state::EB;

composite error behavior
\tstates
\t\t[s1.OperationalNonCritical]->Operational{ServiceError};
\t\t[s1.OperationalNonCritical and s2.Operational]->Operational{ServiceError};
\t\t[s1.Operational or s2.Operational]->Operational{ServiceError};
end composite;";
    parse_ok(src);
}

#[test]
fn composite_state_with_type_sets() {
    // Composite states can target states with type set constraints
    let src = "\
use types ErrorLibrary;
use behavior composite_state::EB;

composite error behavior
\tstates
\t\t[s1.OperationalNonCritical]->Operational{CommonErrors};
\t\t[s2.OperationalNonCritical]->Operational{ServiceError, EarlyService};
\t\t[s1.OperationalNonCritical and s2.OperationalNonCritical]->Operational{ServiceError, EarlyService};
\t\t[s1.Operational or s2.Operational]->Operational{ServiceError, EarlyService};
end composite;";
    parse_ok(src);
}
