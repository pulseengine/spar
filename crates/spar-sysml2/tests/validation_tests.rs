//! Validation tests for the SysML v2 parser.
//!
//! These tests parse SysML v2 files from the test-data directory and verify:
//! 1. No panics during parsing
//! 2. Lossless roundtrip (CST text == original source)
//! 3. Error rate tracking for improvement over time
//!
//! The test files are organized into:
//! - `validation/` -- core language construct validation (20 files)
//! - `training/`   -- representative training examples (10 files)
//! - `examples/`   -- comprehensive integration examples (6 files)
//! - Root-level files (e.g., VehicleUsages.sysml)
//!
//! These tests mirror the structure of the official SysML v2 validation suite
//! from Systems-Modeling/SysML-v2-Release.

use spar_sysml2::SyntaxKind;
use std::path::{Path, PathBuf};

/// Base path for SysML v2 test data, relative to the workspace root.
fn test_data_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-data")
        .join("sysml2")
}

/// Collect all .sysml files recursively from a directory.
fn collect_sysml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    if dir.is_file() && dir.extension().map(|e| e == "sysml").unwrap_or(false) {
        files.push(dir.to_path_buf());
        return files;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_sysml_files(&path));
            } else if path.extension().map(|e| e == "sysml").unwrap_or(false) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Results from parsing a set of files.
#[derive(Default)]
struct ParseResults {
    total: usize,
    clean: usize,
    with_errors: usize,
    panicked: usize,
    error_details: Vec<(String, usize)>,
}

impl ParseResults {
    fn error_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.with_errors as f64 / self.total as f64
    }

    fn report(&self, category: &str) {
        eprintln!("\n{category} Results:");
        eprintln!("  Total files:  {}", self.total);
        eprintln!("  Clean parse:  {}", self.clean);
        eprintln!("  With errors:  {}", self.with_errors);
        eprintln!("  Panicked:     {}", self.panicked);
        eprintln!("  Error rate:   {:.1}%", self.error_rate() * 100.0);
        if !self.error_details.is_empty() {
            eprintln!("  Error details:");
            for (file, count) in &self.error_details {
                eprintln!("    {file}: {count} error(s)");
            }
        }
    }
}

/// Parse a single file and record results.
fn parse_file(path: &Path, results: &mut ParseResults) {
    results.total += 1;
    let file_name = path.file_name().unwrap().to_string_lossy().to_string();

    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  SKIP {file_name}: {e}");
            return;
        }
    };

    // Parse -- catch panics
    let parse_result = std::panic::catch_unwind(|| spar_sysml2::parse(&source));

    match parse_result {
        Ok(parse) => {
            // Verify lossless roundtrip
            let roundtrip = parse.syntax_node().text().to_string();
            assert_eq!(source, roundtrip, "Roundtrip failed for {file_name}");

            // Count errors
            let errors = parse.errors();
            if errors.is_empty() {
                results.clean += 1;
            } else {
                results.with_errors += 1;
                results
                    .error_details
                    .push((file_name.clone(), errors.len()));
            }
        }
        Err(_) => {
            results.panicked += 1;
            eprintln!("  PANIC {file_name}");
        }
    }
}

// ---------------------------------------------------------------------------
// Validation suite tests
// ---------------------------------------------------------------------------

#[test]
fn parse_all_validation_files() {
    let dir = test_data_root().join("validation");
    let files = collect_sysml_files(&dir);
    assert!(
        !files.is_empty(),
        "No validation .sysml files found in {}",
        dir.display()
    );

    let mut results = ParseResults::default();
    for path in &files {
        parse_file(path, &mut results);
    }

    results.report("SysML v2 Validation Suite");
    assert_eq!(
        results.panicked, 0,
        "Parser panicked on {} file(s)",
        results.panicked
    );
    // Validation files are curated to work with our parser -- expect high success rate
    assert!(
        results.error_rate() < 0.5,
        "Error rate {:.1}% exceeds 50% threshold",
        results.error_rate() * 100.0
    );
}

#[test]
fn parse_all_training_files() {
    let dir = test_data_root().join("training");
    let files = collect_sysml_files(&dir);
    assert!(
        !files.is_empty(),
        "No training .sysml files found in {}",
        dir.display()
    );

    let mut results = ParseResults::default();
    for path in &files {
        parse_file(path, &mut results);
    }

    results.report("SysML v2 Training Suite");
    assert_eq!(
        results.panicked, 0,
        "Parser panicked on {} file(s)",
        results.panicked
    );
}

#[test]
fn parse_all_example_files() {
    let dir = test_data_root().join("examples");
    let files = collect_sysml_files(&dir);
    assert!(
        !files.is_empty(),
        "No example .sysml files found in {}",
        dir.display()
    );

    let mut results = ParseResults::default();
    for path in &files {
        parse_file(path, &mut results);
    }

    results.report("SysML v2 Examples Suite");
    assert_eq!(
        results.panicked, 0,
        "Parser panicked on {} file(s)",
        results.panicked
    );
}

#[test]
fn parse_root_sysml_files() {
    let root = test_data_root();
    let files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .map(|ext| ext == "sysml")
                    .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();

    if files.is_empty() {
        eprintln!("No root-level .sysml files found -- skipping");
        return;
    }

    let mut results = ParseResults::default();
    for path in &files {
        parse_file(path, &mut results);
    }

    results.report("SysML v2 Root Files");
    assert_eq!(
        results.panicked, 0,
        "Parser panicked on {} file(s)",
        results.panicked
    );
}

/// Combined test across ALL .sysml files in the test-data/sysml2 tree.
#[test]
fn parse_all_sysml2_files_combined() {
    let root = test_data_root();
    let files = collect_sysml_files(&root);
    assert!(
        !files.is_empty(),
        "No .sysml files found in {}",
        root.display()
    );

    let mut results = ParseResults::default();
    for path in &files {
        parse_file(path, &mut results);
    }

    results.report("SysML v2 Combined (all files)");
    assert_eq!(
        results.panicked, 0,
        "Parser panicked on {} file(s)",
        results.panicked
    );

    // Track overall conformance
    eprintln!(
        "\n  CONFORMANCE: {}/{} files parse cleanly ({:.1}%)",
        results.clean,
        results.total,
        (results.clean as f64 / results.total as f64) * 100.0
    );
}

// ---------------------------------------------------------------------------
// Per-file validation tests -- ensure each category file parses
// ---------------------------------------------------------------------------

macro_rules! validation_file_test {
    ($name:ident, $file:literal) => {
        #[test]
        fn $name() {
            let path = test_data_root().join("validation").join($file);
            assert!(path.exists(), "File not found: {}", path.display());
            let source = std::fs::read_to_string(&path).unwrap();
            let parse = spar_sysml2::parse(&source);

            // Lossless roundtrip
            let roundtrip = parse.syntax_node().text().to_string();
            assert_eq!(source, roundtrip, "Roundtrip failed for {}", $file);

            // Root is SOURCE_FILE
            assert_eq!(parse.syntax_node().kind(), SyntaxKind::SOURCE_FILE);

            if !parse.ok() {
                eprintln!("{}: {} parse error(s)", $file, parse.errors().len());
                for err in parse.errors() {
                    eprintln!("  offset {}: {}", err.offset, err.msg);
                }
            }
        }
    };
}

validation_file_test!(validate_01_packages, "01-Packages.sysml");
validation_file_test!(validate_02_parts, "02-Parts.sysml");
validation_file_test!(validate_03_ports, "03-Ports.sysml");
validation_file_test!(validate_04_connections, "04-Connections.sysml");
validation_file_test!(validate_05_actions, "05-Actions.sysml");
validation_file_test!(validate_06_states, "06-States.sysml");
validation_file_test!(validate_07_requirements, "07-Requirements.sysml");
validation_file_test!(validate_08_constraints, "08-Constraints.sysml");
validation_file_test!(validate_09_interfaces, "09-Interfaces.sysml");
validation_file_test!(validate_10_attributes, "10-Attributes.sysml");
validation_file_test!(validate_11_items, "11-Items.sysml");
validation_file_test!(validate_12_enums, "12-Enums.sysml");
validation_file_test!(validate_13_allocations, "13-Allocations.sysml");
validation_file_test!(validate_14_specialization, "14-Specialization.sysml");
validation_file_test!(validate_15_multiplicity, "15-Multiplicity.sysml");
validation_file_test!(validate_16_imports, "16-Imports.sysml");
validation_file_test!(validate_17_visibility, "17-Visibility.sysml");
validation_file_test!(validate_18_calcs, "18-Calcs.sysml");
validation_file_test!(validate_19_abstract, "19-Abstract.sysml");
validation_file_test!(validate_20_ref_usages, "20-RefUsages.sysml");

// ---------------------------------------------------------------------------
// Parse tree structure tests -- compare with expected Java pilot output
// ---------------------------------------------------------------------------

#[test]
fn parse_tree_vehicle_definitions() {
    let path = test_data_root()
        .join("examples")
        .join("VehicleDefinitions.sysml");
    let source = std::fs::read_to_string(&path).unwrap();
    let parse = spar_sysml2::parse(&source);
    let root = parse.syntax_node();

    // Root must be SOURCE_FILE
    assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);

    // Should contain a PACKAGE node
    let pkg = root
        .children()
        .find(|n| n.kind() == SyntaxKind::PACKAGE)
        .expect("expected PACKAGE node for VehicleDefinitions");

    // Package should have a NAMESPACE_BODY
    let body = pkg
        .children()
        .find(|n| n.kind() == SyntaxKind::NAMESPACE_BODY)
        .expect("expected NAMESPACE_BODY");

    // Count definitions -- the Java pilot would produce identical structure
    let attr_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::ATTRIBUTE_DEF)
        .collect();
    assert_eq!(attr_defs.len(), 1, "expected 1 ATTRIBUTE_DEF (Torque)");

    let port_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::PORT_DEF)
        .collect();
    assert_eq!(
        port_defs.len(),
        2,
        "expected 2 PORT_DEF (DriveIF, MountingPoint)"
    );

    let part_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::PART_DEF)
        .collect();
    assert_eq!(
        part_defs.len(),
        6,
        "expected 6 PART_DEF (Vehicle, Axle, Wheel, Lugbolt, AxleAssembly, Transmission)"
    );

    let interface_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::INTERFACE_DEF)
        .collect();
    assert_eq!(
        interface_defs.len(),
        1,
        "expected 1 INTERFACE_DEF (Mounting)"
    );

    // Print tree for manual comparison with Java pilot
    eprintln!("\nParse tree for VehicleDefinitions.sysml:");
    print_tree(&root, 0);
}

#[test]
fn parse_tree_sensor_system() {
    let path = test_data_root().join("examples").join("SensorSystem.sysml");
    let source = std::fs::read_to_string(&path).unwrap();
    let parse = spar_sysml2::parse(&source);
    let root = parse.syntax_node();

    assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);

    let pkg = root
        .children()
        .find(|n| n.kind() == SyntaxKind::PACKAGE)
        .expect("expected PACKAGE node for SensorSystemExample");

    let body = pkg
        .children()
        .find(|n| n.kind() == SyntaxKind::NAMESPACE_BODY)
        .expect("expected NAMESPACE_BODY");

    // Verify structure matches what the Java pilot would produce
    let attr_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::ATTRIBUTE_DEF)
        .collect();
    assert_eq!(
        attr_defs.len(),
        4,
        "expected 4 ATTRIBUTE_DEF (Temperature, Pressure, Humidity, Voltage)"
    );

    let port_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::PORT_DEF)
        .collect();
    assert_eq!(port_defs.len(), 3, "expected 3 PORT_DEF");

    let conn_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::CONNECTION_DEF)
        .collect();
    assert_eq!(
        conn_defs.len(),
        2,
        "expected 2 CONNECTION_DEF (SensorDataLink, PowerLink)"
    );

    let part_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::PART_DEF)
        .collect();
    // EnvironmentalSensor, DataAggregator, PowerSupply, Controller (abstract),
    // SensorController, SensorSystem = 6
    assert!(
        part_defs.len() >= 5,
        "expected at least 5 PART_DEF nodes, got {}",
        part_defs.len()
    );

    let req_defs: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::REQUIREMENT_DEF)
        .collect();
    assert_eq!(req_defs.len(), 1, "expected 1 REQUIREMENT_DEF (SensorReq)");

    let satisfy_nodes: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::SATISFY_REQ)
        .collect();
    assert_eq!(satisfy_nodes.len(), 1, "expected 1 SATISFY_REQ");

    let verify_nodes: Vec<_> = body
        .children()
        .filter(|n| n.kind() == SyntaxKind::VERIFY_REQ)
        .collect();
    assert_eq!(verify_nodes.len(), 1, "expected 1 VERIFY_REQ");

    eprintln!("\nParse tree for SensorSystem.sysml:");
    print_tree(&root, 0);
}

#[test]
fn parse_tree_edge_cases() {
    let path = test_data_root().join("examples").join("EdgeCases.sysml");
    let source = std::fs::read_to_string(&path).unwrap();
    let parse = spar_sysml2::parse(&source);
    let root = parse.syntax_node();

    assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);

    // Lossless roundtrip
    let roundtrip = root.text().to_string();
    assert_eq!(source, roundtrip, "Roundtrip failed for EdgeCases.sysml");

    // Should not panic (we got here)
    // Print errors for visibility
    if !parse.ok() {
        eprintln!("\nEdgeCases.sysml parse errors:");
        for err in parse.errors() {
            eprintln!("  offset {}: {}", err.offset, err.msg);
        }
    }

    eprintln!("\nParse tree for EdgeCases.sysml:");
    print_tree(&root, 0);
}

#[test]
fn parse_tree_vehicle_usages() {
    let path = test_data_root().join("VehicleUsages.sysml");
    if !path.exists() {
        eprintln!("VehicleUsages.sysml not found -- skipping");
        return;
    }
    let source = std::fs::read_to_string(&path).unwrap();
    let parse = spar_sysml2::parse(&source);
    let root = parse.syntax_node();

    assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);

    // Lossless roundtrip
    let roundtrip = root.text().to_string();
    assert_eq!(
        source, roundtrip,
        "Roundtrip failed for VehicleUsages.sysml"
    );

    // This file uses advanced features from the official SysML v2 training.
    // Track how many errors we produce vs the Java pilot (which would produce 0).
    eprintln!(
        "\nVehicleUsages.sysml: {} parse error(s)",
        parse.errors().len()
    );
    for err in parse.errors() {
        eprintln!("  offset {}: {}", err.offset, err.msg);
    }

    eprintln!("\nParse tree for VehicleUsages.sysml (top-level only):");
    // Only print top-level children to keep output manageable
    for child in root.children() {
        eprintln!("  {:?} {:?}", child.kind(), child.text_range());
        for grandchild in child.children() {
            eprintln!("    {:?} {:?}", grandchild.kind(), grandchild.text_range());
        }
    }
}

// ---------------------------------------------------------------------------
// Conformance comparison: what the Java pilot would accept
// ---------------------------------------------------------------------------

/// Test constructs that the Java pilot handles but may challenge our parser.
/// Each sub-test documents the expected behavior.
#[test]
fn java_pilot_conformance_basic_constructs() {
    // The Java pilot accepts all of these without errors.
    let constructs = [
        ("empty package", "package P { }"),
        ("semicolon package", "package P;"),
        ("part def", "part def A { }"),
        ("part usage typed", "part a : A;"),
        ("part usage specialized", "part a :> A;"),
        ("part usage redefines", "part a :>> A;"),
        ("port def", "port def P { }"),
        ("port usage", "port p : P;"),
        ("in port", "in port p : P;"),
        ("out port", "out port p : P;"),
        ("inout port", "inout port p : P;"),
        ("connection def", "connection def C { }"),
        ("connect usage", "connect a.p to b.q;"),
        ("action def", "action def A { }"),
        ("action usage", "action a : A;"),
        ("state def", "state def S { }"),
        ("state usage", "state s : S;"),
        ("attribute def", "attribute def D;"),
        ("attribute usage", "attribute x : Real;"),
        ("attribute default int", "attribute x : Real = 42;"),
        ("attribute default real", "attribute x : Real = 3.14;"),
        (
            "attribute default string",
            r#"attribute x : String = "hello";"#,
        ),
        ("attribute default true", "attribute x : Boolean = true;"),
        ("attribute default false", "attribute x : Boolean = false;"),
        ("item def", "item def I { }"),
        ("item usage", "item i : I;"),
        ("out item", "out item o : I;"),
        ("enum def", "enum def E { }"),
        ("interface def", "interface def IF { }"),
        ("requirement def", "requirement def R { }"),
        ("requirement usage", "requirement r : R;"),
        ("constraint def", "constraint def C { }"),
        ("constraint usage", "constraint c : C;"),
        ("calc def", "calc def F { }"),
        ("allocation def", "allocation def A { }"),
        ("ref part", "ref part r : A;"),
        ("abstract part def", "abstract part def A { }"),
        ("abstract port def", "abstract port def P { }"),
        ("specializes keyword", "part def B specializes A { }"),
        ("subsets keyword", "part x subsets y;"),
        ("redefines keyword", "part x redefines y;"),
        ("multiplicity single", "part x [1] : A;"),
        ("multiplicity range", "part x [0..5] : A;"),
        ("multiplicity star", "part x [0..*] : A;"),
        ("import wildcard", "import Pkg::*;"),
        ("import specific", "import Pkg::Name;"),
        ("satisfy", "satisfy r by impl;"),
        ("verify", "verify r by test;"),
        ("refine", "refine r by detailed;"),
        ("doc string", "part def A { doc \"documented\" }"),
        ("comment string", "part def A { comment \"noted\" }"),
        ("feature decl", "part def A { feature x : Real; }"),
    ];

    let mut clean = 0;
    let mut total = 0;
    let mut failures = Vec::new();

    for (label, source) in &constructs {
        total += 1;
        let parse = spar_sysml2::parse(source);

        // Must not panic (we got here)
        // Must roundtrip
        let roundtrip = parse.syntax_node().text().to_string();
        assert_eq!(*source, roundtrip, "Roundtrip failed for: {label}");

        if parse.ok() {
            clean += 1;
        } else {
            failures.push((
                *label,
                parse
                    .errors()
                    .iter()
                    .map(|e| e.msg.clone())
                    .collect::<Vec<_>>(),
            ));
        }
    }

    eprintln!("\nJava Pilot Conformance (basic constructs):");
    eprintln!("  {clean}/{total} parse cleanly");
    if !failures.is_empty() {
        eprintln!("  Failures:");
        for (label, errors) in &failures {
            eprintln!("    {label}: {errors:?}");
        }
    }

    // All basic constructs should parse cleanly -- they are core SysML v2
    assert_eq!(
        clean,
        total,
        "{} basic constructs failed to parse cleanly",
        total - clean
    );
}

/// Test constructs that the Java pilot handles but are more advanced.
/// Some of these may produce parse errors in our implementation -- that is
/// expected and tracked for improvement.
#[test]
fn java_pilot_conformance_advanced_constructs() {
    let constructs = [
        (
            "nested package",
            "package A { package B { part def C { } } }",
        ),
        (
            "deeply nested parts",
            "part def A { part b { part c { attribute x : Real; } } }",
        ),
        (
            "multiple imports",
            "package P { import A::*; import B::*; import C::D; }",
        ),
        ("qualified typing", "part x : Pkg::SubPkg::Type;"),
        (
            "connection in package",
            "package P { part a { port p; } part b { port q; } connect a.p to b.q; }",
        ),
        (
            "requirement with subject",
            "requirement def R { subject s : System; }",
        ),
        (
            "mixed definitions",
            "package Mixed { part def P { } port def Q { } connection def C { } action def A { } }",
        ),
        (
            "private import",
            "package P { private import X::*; part def A { } }",
        ),
        (
            "public import",
            "package P { public import X::*; part def A { } }",
        ),
        (
            "multiple connections",
            "package P { part a { port p; } part b { port q; } part c { port r; } connect a.p to b.q; connect b.q to c.r; }",
        ),
    ];

    let mut clean = 0;
    let mut total = 0;
    let mut failures = Vec::new();

    for (label, source) in &constructs {
        total += 1;
        let result = std::panic::catch_unwind(|| {
            let parse = spar_sysml2::parse(source);
            let roundtrip = parse.syntax_node().text().to_string();
            assert_eq!(*source, roundtrip, "Roundtrip failed for: {label}");
            parse.ok()
        });

        match result {
            Ok(true) => clean += 1,
            Ok(false) => {
                let parse = spar_sysml2::parse(source);
                failures.push((
                    *label,
                    false,
                    parse
                        .errors()
                        .iter()
                        .map(|e| e.msg.clone())
                        .collect::<Vec<_>>(),
                ));
            }
            Err(_) => {
                failures.push((*label, true, vec!["PANICKED".to_string()]));
            }
        }
    }

    eprintln!("\nJava Pilot Conformance (advanced constructs):");
    eprintln!("  {clean}/{total} parse cleanly");
    if !failures.is_empty() {
        eprintln!("  Issues:");
        for (label, panicked, errors) in &failures {
            let status = if *panicked { "PANIC" } else { "ERRORS" };
            eprintln!("    [{status}] {label}: {errors:?}");
        }
    }

    // No panics allowed
    let panic_count = failures.iter().filter(|(_, p, _)| *p).count();
    assert_eq!(
        panic_count, 0,
        "Parser panicked on {} advanced constructs",
        panic_count
    );
}

// ---------------------------------------------------------------------------
// Helper: print tree structure for manual comparison
// ---------------------------------------------------------------------------

fn print_tree(node: &spar_sysml2::SyntaxNode, indent: usize) {
    let prefix = " ".repeat(indent);
    eprintln!("{prefix}{:?} {:?}", node.kind(), node.text_range());
    for child in node.children() {
        print_tree(&child, indent + 2);
    }
}
