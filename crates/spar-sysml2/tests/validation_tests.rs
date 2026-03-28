//! SysML v2 validation model tests.
//!
//! Each test parses a .sysml file from test-data/sysml2/validation/,
//! lowers it to AADL, and verifies the lowering produces the expected output.

use spar_sysml2::lower::{item_tree_to_aadl, lower_to_aadl};
use spar_sysml2::parse;

fn parse_validation_file(name: &str) -> spar_sysml2::Parse {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = manifest
        .replace("crates/spar-sysml2", "")
        .replace("/crates/spar-sysml2", "");
    let path = format!(
        "{}/test-data/sysml2/validation/{}",
        root.trim_end_matches('/'),
        name
    );
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Cannot read {}: {}", path, e));
    parse(&source)
}

fn assert_parses_ok(name: &str) -> spar_sysml2::Parse {
    let result = parse_validation_file(name);
    assert!(result.ok(), "{} parse errors: {:?}", name, result.errors());
    result
}

#[test]
fn val_01_packages_parse_and_lower() {
    let result = assert_parses_ok("01-Packages.sysml");
    let tree = lower_to_aadl(&result);
    assert!(tree.packages.len() >= 2, "expected nested packages");
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("system Sensor"));
    assert!(aadl.contains("system Controller"));
}

#[test]
fn val_02_parts_parse_and_lower() {
    let result = assert_parses_ok("02-Parts.sysml");
    let tree = lower_to_aadl(&result);
    assert!(
        tree.component_impls.len() >= 2,
        "expected impls for Engine and Vehicle"
    );
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("system Engine"));
    assert!(aadl.contains("system Vehicle"));
}

#[test]
fn val_03_ports_parse_and_lower() {
    let result = assert_parses_ok("03-Ports.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data SensorPort"));
    assert!(aadl.contains("system Sensor"));
}

#[test]
fn val_04_connections_parse_and_lower() {
    let result = assert_parses_ok("04-Connections.sysml");
    let tree = lower_to_aadl(&result);
    assert!(!tree.component_impls.is_empty());
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("connections"));
}

#[test]
fn val_05_actions_parse_and_lower() {
    let result = assert_parses_ok("05-Actions.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("subprogram ProcessData"));
}

#[test]
fn val_06_states_parse_and_lower() {
    let result = assert_parses_ok("06-States.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data Operational"));
    assert!(aadl.contains("modes"));
}

#[test]
fn val_07_requirements_parse_and_lower() {
    let result = assert_parses_ok("07-Requirements.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("abstract LatencyReq"));
}

#[test]
fn val_08_constraints_parse_and_lower() {
    let result = assert_parses_ok("08-Constraints.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data TimingConstraint"));
}

#[test]
fn val_09_interfaces_parse_and_lower() {
    let result = assert_parses_ok("09-Interfaces.sysml");
    let tree = lower_to_aadl(&result);
    assert!(!tree.feature_group_types.is_empty());
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("feature group DataLink"));
}

#[test]
fn val_10_attributes_parse_and_lower() {
    let result = assert_parses_ok("10-Attributes.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data Mass"));
    assert!(aadl.contains("data Voltage"));
}

#[test]
fn val_11_items_parse_and_lower() {
    let result = assert_parses_ok("11-Items.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data SensorData"));
}

#[test]
fn val_12_enums_parse_and_lower() {
    let result = assert_parses_ok("12-Enums.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("data Color"));
    assert!(aadl.contains("Enumerators"));
}

#[test]
fn val_13_allocations_parse_and_lower() {
    let result = assert_parses_ok("13-Allocations.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("processor TaskAllocation"));
}

#[test]
fn val_14_specialization_parse_and_lower() {
    let result = assert_parses_ok("14-Specialization.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("extends Vehicle"));
}

#[test]
fn val_15_multiplicity_parse_and_lower() {
    let result = assert_parses_ok("15-Multiplicity.sysml");
    let tree = lower_to_aadl(&result);
    assert!(
        !tree.component_impls.is_empty(),
        "expected Vehicle impl with array subcomponents"
    );
}

#[test]
fn val_16_imports_parse_and_lower() {
    let result = assert_parses_ok("16-Imports.sysml");
    let tree = lower_to_aadl(&result);
    let target = tree
        .packages
        .iter()
        .find(|(_, p)| p.name.as_str() == "ImportTarget");
    assert!(target.is_some());
    let (_, pkg) = target.unwrap();
    assert!(!pkg.with_clauses.is_empty());
}

#[test]
fn val_17_visibility_parse_and_lower() {
    let result = assert_parses_ok("17-Visibility.sysml");
    let tree = lower_to_aadl(&result);
    assert_eq!(tree.packages.len(), 1);
}

#[test]
fn val_18_calcs_parse_and_lower() {
    let result = assert_parses_ok("18-Calcs.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("subprogram Latency"));
}

#[test]
fn val_19_abstract_parse_and_lower() {
    let result = assert_parses_ok("19-Abstract.sysml");
    let tree = lower_to_aadl(&result);
    let aadl = item_tree_to_aadl(&tree);
    assert!(aadl.contains("abstract Component"));
}

#[test]
fn val_20_ref_usages_parse_and_lower() {
    let result = assert_parses_ok("20-RefUsages.sysml");
    let tree = lower_to_aadl(&result);
    assert!(
        !tree.component_impls.is_empty(),
        "expected Controller impl with ref subcomponent"
    );
}

#[test]
fn all_validation_files_parse_losslessly() {
    let files = [
        "01-Packages.sysml",
        "02-Parts.sysml",
        "03-Ports.sysml",
        "04-Connections.sysml",
        "05-Actions.sysml",
        "06-States.sysml",
        "07-Requirements.sysml",
        "08-Constraints.sysml",
        "09-Interfaces.sysml",
        "10-Attributes.sysml",
        "11-Items.sysml",
        "12-Enums.sysml",
        "13-Allocations.sysml",
        "14-Specialization.sysml",
        "15-Multiplicity.sysml",
        "16-Imports.sysml",
        "17-Visibility.sysml",
        "18-Calcs.sysml",
        "19-Abstract.sysml",
        "20-RefUsages.sysml",
    ];
    for file in &files {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = manifest
            .replace("crates/spar-sysml2", "")
            .replace("/crates/spar-sysml2", "");
        let path = format!(
            "{}/test-data/sysml2/validation/{}",
            root.trim_end_matches('/'),
            file
        );
        let source = std::fs::read_to_string(&path).unwrap();
        let result = parse(&source);
        let roundtrip = result.syntax_node().text().to_string();
        assert_eq!(source, roundtrip, "Lossless roundtrip failed for {}", file);
    }
}
