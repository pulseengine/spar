//! SysML v2 parser conformance tests using official examples.
//!
//! Files from Systems-Modeling/SysML-v2-Release repository.

use spar_sysml2::parse;

fn parse_file(name: &str) -> spar_sysml2::Parse {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = manifest.replace("crates/spar-sysml2", "").replace("/crates/spar-sysml2", "");
    let path = format!("{}/test-data/sysml2/{}", root.trim_end_matches('/'), name);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Cannot read {}: {}", path, e));
    parse(&source)
}

#[test]
fn parse_package_example() {
    let result = parse_file("Package_Example.sysml");
    assert!(!result.syntax_node().text().is_empty());
}

#[test]
fn parse_part_definition_example() {
    let result = parse_file("Part_Definition_Example.sysml");
    let text = result.syntax_node().text().to_string();
    assert!(text.contains("part def Vehicle"));
}

#[test]
fn parse_parts_example() {
    let result = parse_file("Parts_Example.sysml");
    assert!(!result.syntax_node().text().is_empty());
}

#[test]
fn parse_connections_example() {
    let result = parse_file("Connections_Example.sysml");
    assert!(!result.syntax_node().text().is_empty());
}

#[test]
fn parse_port_example() {
    let result = parse_file("Port_Example.sysml");
    let text = result.syntax_node().text().to_string();
    assert!(text.contains("port"));
}

#[test]
fn parse_vehicle_usages() {
    let result = parse_file("VehicleUsages.sysml");
    let text = result.syntax_node().text().to_string();
    assert!(text.contains("Vehicle"));
}

#[test]
fn parse_annex_a_simple_vehicle() {
    let result = parse_file("SysML_v2_Spec_Annex_A_SimpleVehicleModel.sysml");
    assert!(u32::from(result.syntax_node().text().len()) > 50000);
}

#[test]
fn lossless_roundtrip_all_files() {
    for file in &[
        "Package_Example.sysml",
        "Part_Definition_Example.sysml",
        "Parts_Example.sysml",
        "Connections_Example.sysml",
        "Port_Example.sysml",
        "VehicleUsages.sysml",
        "SysML_v2_Spec_Annex_A_SimpleVehicleModel.sysml",
    ] {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = manifest.replace("crates/spar-sysml2", "").replace("/crates/spar-sysml2", "");
        let path = format!("{}/test-data/sysml2/{}", root.trim_end_matches('/'), file);
        let source = std::fs::read_to_string(&path).unwrap();
        let result = parse(&source);
        let roundtrip = result.syntax_node().text().to_string();
        assert_eq!(source, roundtrip, "Lossless roundtrip failed for {}", file);
    }
}
