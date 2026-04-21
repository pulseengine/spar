use spar_syntax::SyntaxKind;
use spar_syntax::parse;

/// Parse an AADL string and assert it produces no errors.
fn check_no_errors(input: &str) {
    let result = parse(input);
    let errors = result.errors();
    assert!(
        errors.is_empty(),
        "expected no errors, got:\n{}",
        errors
            .iter()
            .map(|e| format!("  offset {}: {}", e.offset, e.msg))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Parse an AADL string and assert it produces at least one error.
fn check_has_errors(input: &str) {
    let result = parse(input);
    assert!(
        !result.errors().is_empty(),
        "expected parse errors but got none"
    );
}

/// Parse and verify the syntax tree is lossless (round-trips to original text).
fn check_lossless(input: &str) {
    let result = parse(input);
    let root = result.syntax_node();
    assert_eq!(
        root.text().to_string(),
        input,
        "syntax tree does not round-trip to original text"
    );
}

// ====================================================================
// Basic structure tests
// ====================================================================

#[test]
fn empty_package() {
    let input = "package Empty\npublic\nend Empty;\n";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn package_with_private_section() {
    let input = "\
package P
public
private
end P;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn with_clauses() {
    let input = "\
package P
public
  with A;
  with B, C;
end P;
";
    check_no_errors(input);
}

#[test]
fn property_set() {
    let input = "\
property set MyProps is
  MaxThreads : aadlinteger applies to (all);
end MyProps;
";
    check_no_errors(input);
}

// ====================================================================
// Component categories
// ====================================================================

#[test]
fn all_component_categories() {
    let cats = [
        "abstract",
        "system",
        "process",
        "thread",
        "thread group",
        "subprogram",
        "subprogram group",
        "processor",
        "virtual processor",
        "memory",
        "bus",
        "virtual bus",
        "device",
        "data",
    ];
    for cat in &cats {
        let name = cat.replace(' ', "_") + "_T";
        let input = format!("package P\npublic\n  {cat} {name}\n  end {name};\nend P;\n");
        check_no_errors(&input);
    }
}

// ====================================================================
// Features
// ====================================================================

#[test]
fn data_ports() {
    let input = "\
package P
public
  system S
    features
      p_in : in data port;
      p_out : out data port;
      p_inout : in out data port;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn event_ports() {
    let input = "\
package P
public
  system S
    features
      e_in : in event port;
      e_out : out event port;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn event_data_ports() {
    let input = "\
package P
public
  system S
    features
      edp : in event data port;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn access_features() {
    let input = "\
package P
public
  system S
    features
      ba : requires bus access;
      da : provides data access;
      spa : requires subprogram access;
      spga : provides subprogram group access;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn feature_group() {
    let input = "\
package P
public
  feature group FG
    features
      p1 : out data port;
      p2 : in event port;
  end FG;

  system S
    features
      fg : feature group FG;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn feature_group_inverse() {
    let input = "\
package P
public
  feature group FG
    features
      p1 : out data port;
  end FG;

  system S
    features
      fg : feature group inverse of FG;
  end S;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Connections
// ====================================================================

#[test]
fn port_connections() {
    let input = "\
package P
public
  system S
    features
      p_in : in data port;
      p_out : out data port;
  end S;
  system implementation S.i
    subcomponents
      sub1 : process Pr;
    connections
      c1 : port p_in -> sub1.inp;
      c2 : port sub1.outp -> p_out;
  end S.i;
  process Pr
    features
      inp : in data port;
      outp : out data port;
  end Pr;
end P;
";
    check_no_errors(input);
}

#[test]
fn bus_access_connection() {
    let input = "\
package P
public
  system S
    features
      ba : requires bus access;
  end S;
  system implementation S.i
    subcomponents
      sub1 : process Pr;
    connections
      c1 : bus access ba -> sub1.ba;
  end S.i;
  process Pr
    features
      ba : requires bus access;
  end Pr;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Flows
// ====================================================================

#[test]
fn flow_spec() {
    let input = "\
package P
public
  system S
    features
      p_in : in data port;
      p_out : out data port;
    flows
      f_path : flow path p_in -> p_out;
      f_src : flow source p_out;
      f_sink : flow sink p_in;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn flow_implementation() {
    let input = "\
package P
public
  system S
    features
      p_in : in data port;
      p_out : out data port;
    flows
      f : flow path p_in -> p_out;
  end S;
  system implementation S.i
    subcomponents
      sub : process Pr;
    connections
      c1 : port p_in -> sub.inp;
      c2 : port sub.outp -> p_out;
    flows
      f : flow path p_in -> c1 -> sub.compute -> c2 -> p_out;
  end S.i;
  process Pr
    features
      inp : in data port;
      outp : out data port;
    flows
      compute : flow path inp -> outp;
  end Pr;
end P;
";
    check_no_errors(input);
}

#[test]
fn end_to_end_flow() {
    let input = "\
package P
public
  system S
    features
      p_in : in data port;
      p_out : out data port;
  end S;
  system implementation S.i
    subcomponents
      sub : process Pr;
    connections
      c1 : port p_in -> sub.inp;
      c2 : port sub.outp -> p_out;
    flows
      e2e : end to end flow sub.f1 -> c2 -> p_out;
  end S.i;
  process Pr
    features
      inp : in data port;
      outp : out data port;
    flows
      f1 : flow source outp;
  end Pr;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Modes
// ====================================================================

#[test]
fn modes_and_transitions() {
    let input = "\
package P
public
  system S
    features
      trigger : in event port;
    modes
      nominal : initial mode;
      degraded : mode;
      t1 : nominal -[trigger]-> degraded;
  end S;
end P;
";
    check_no_errors(input);
}

#[test]
fn subcomponents_in_modes() {
    let input = "\
package P
public
  system S
    features
      e : in event port;
    modes
      m1 : initial mode;
      m2 : mode;
      t : m1 -[e]-> m2;
  end S;
  system implementation S.i
    subcomponents
      sub1 : process Pr in modes (m1);
      sub2 : process Pr in modes (m2);
  end S.i;
  process Pr
  end Pr;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Properties
// ====================================================================

#[test]
fn basic_properties() {
    let input = "\
package P
public
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Period => 20 ms;
      Deadline => 20 ms;
      Priority => 5;
  end T;
end P;
";
    check_no_errors(input);
}

#[test]
fn property_with_units_and_range() {
    let input = "\
package P
public
  thread T
    properties
      Compute_Execution_Time => 2 ms .. 5 ms;
  end T;
end P;
";
    check_no_errors(input);
}

#[test]
fn property_reference_and_classifier() {
    let input = "\
package P
public
  thread T
    properties
      Actual_Processor_Binding => (reference (cpu));
      Allowed_Processor_Binding_Class => (classifier (processor));
  end T;
end P;
";
    check_no_errors(input);
}

#[test]
fn property_applies_to() {
    let input = "\
package P
public
  system S
  end S;
  system implementation S.i
    subcomponents
      sub : process Pr;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to sub;
  end S.i;
  process Pr
  end Pr;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Annexes
// ====================================================================

#[test]
fn annex_subclause() {
    let input = "\
package P
public
  system S
    annex EMV2 {**
      use types ErrorLibrary;
    **};
  end S;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Extensions
// ====================================================================

#[test]
fn type_extension() {
    let input = "\
package P
public
  system Base
    features
      p : in data port;
  end Base;
  system Extended extends Base
    features
      q : out data port;
  end Extended;
end P;
";
    check_no_errors(input);
}

#[test]
fn impl_extension() {
    let input = "\
package P
public
  system S
  end S;
  system implementation S.base
  end S.base;
  system implementation S.extended extends S.base
  end S.extended;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Keywords as identifiers
// ====================================================================

#[test]
fn keyword_as_flow_name() {
    let input = "\
package P
public
  process Pr
    features
      p_in : in data port;
      p_out : out data port;
    flows
      compute : flow path p_in -> p_out;
  end Pr;
end P;
";
    check_no_errors(input);
}

#[test]
fn keyword_in_classifier_ref() {
    let input = "\
package P
public
  thread T
    properties
      Allowed_Processor_Binding_Class => (classifier (processor));
  end T;
end P;
";
    check_no_errors(input);
}

// ====================================================================
// Losslessness
// ====================================================================

#[test]
fn lossless_round_trip_complex() {
    let input = "\
-- A comment
package P
public
  with A;

  system S
    features
      p : in data port;
    properties
      Period => 10 ms;
  end S;

  system implementation S.i
    subcomponents
      sub : process Pr;
    connections
      c1 : port p -> sub.inp;
  end S.i;

  process Pr
    features
      inp : in data port;
  end Pr;
end P;
";
    check_lossless(input);
}

// ====================================================================
// Error recovery
// ====================================================================

#[test]
fn error_recovery_missing_semicolon() {
    // Parser should produce errors but not panic
    let input = "\
package P
public
  system S
  end S
end P;
";
    check_has_errors(input);
    // Should still produce a syntax tree
    let result = parse(input);
    let root = result.syntax_node();
    assert!(!root.text().is_empty());
}

#[test]
fn error_recovery_garbage_tokens() {
    let input = "\
package P
public
  @@@ garbage
end P;
";
    check_has_errors(input);
    let result = parse(input);
    let _root = result.syntax_node();
}

// ====================================================================
// STPA-REQ-001: Error handling for invalid input
// ====================================================================

#[test]
fn unrecognized_token_produces_error() {
    let input = "package P\npublic\n  @@@ garbage\nend P;";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    let errors: Vec<_> = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ERROR)
        .collect();
    assert!(
        !errors.is_empty(),
        "expected parse errors for invalid tokens"
    );
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

#[test]
fn missing_end_keyword_produces_error() {
    let input = "package P\npublic\n  system S\n  system S;";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    assert!(
        !parsed.errors().is_empty() || root.descendants().any(|n| n.kind() == SyntaxKind::ERROR),
        "expected parse errors for missing end keyword"
    );
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

#[test]
fn invalid_component_category_produces_error() {
    let input = "package P\npublic\n  widget W\n  end W;\nend P;";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    assert!(
        !parsed.errors().is_empty() || root.descendants().any(|n| n.kind() == SyntaxKind::ERROR),
        "expected parse errors for invalid component category"
    );
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

#[test]
fn incomplete_feature_declaration_produces_error() {
    let input = "package P\npublic\n  system S\n    features\n      p : in;\n  end S;\nend P;";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    assert!(
        !parsed.errors().is_empty() || root.descendants().any(|n| n.kind() == SyntaxKind::ERROR),
        "expected parse errors for incomplete feature"
    );
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

#[test]
fn recovery_after_error() {
    // An error inside a component (missing semicolon) — the parser should
    // still produce a COMPONENT_TYPE node and an AADL_PACKAGE wrapping it,
    // alongside parse error diagnostics.
    let input = "\
package P
public
  system S
    features
      p : in data port
  end S;
end P;
";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    let has_error =
        !parsed.errors().is_empty() || root.descendants().any(|n| n.kind() == SyntaxKind::ERROR);
    let has_component = root
        .descendants()
        .any(|n| n.kind() == SyntaxKind::COMPONENT_TYPE);
    assert!(has_error, "expected parse errors for missing semicolon");
    assert!(has_component, "expected COMPONENT_TYPE node despite error");
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

#[test]
fn error_preserves_source_text() {
    let inputs = [
        "package P\npublic\n  @@@ garbage\nend P;",
        "package P\npublic\n  system S\n  end S\nend P;",
        "package P\npublic\n  widget W\n  end W;\nend P;",
        "package\npublic\nend;",
        "system S\nend S;",
    ];
    for input in &inputs {
        let parsed = parse(input);
        let root = parsed.syntax_node();
        assert_eq!(
            root.text().to_string(),
            *input,
            "CST must be lossless even with errors"
        );
    }
}

#[test]
fn multiple_errors_collected() {
    let input = "\
package P
public
  @@@ first_garbage
  $$$ second_garbage
end P;
";
    let parsed = parse(input);
    let root = parsed.syntax_node();
    let error_nodes: Vec<_> = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ERROR)
        .collect();
    assert!(
        error_nodes.len() >= 2 || !parsed.errors().is_empty(),
        "expected multiple errors, got {} ERROR nodes and {} parse errors",
        error_nodes.len(),
        parsed.errors().len()
    );
    assert_eq!(root.text().to_string(), input, "CST must be lossless");
}

// ====================================================================
// Test data files
// ====================================================================

fn check_file_no_errors(path: &str) {
    let input =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));
    let result = parse(&input);
    let errors = result.errors();
    assert!(
        errors.is_empty(),
        "{path}: expected no errors, got:\n{}",
        errors
            .iter()
            .map(|e| format!("  offset {}: {}", e.offset, e.msg))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Verify losslessness
    let root = result.syntax_node();
    assert_eq!(
        root.text().to_string(),
        input,
        "{path}: syntax tree does not round-trip"
    );
}

#[test]
fn test_data_simple_system() {
    check_file_no_errors("../../test-data/parser/simple_system.aadl");
}

#[test]
fn test_data_empty_package() {
    check_file_no_errors("../../test-data/parser/empty_package.aadl");
}

#[test]
fn test_data_all_categories() {
    check_file_no_errors("../../test-data/parser/all_categories.aadl");
}

#[test]
fn test_data_features_all() {
    check_file_no_errors("../../test-data/parser/features_all.aadl");
}

#[test]
fn test_data_connections_all() {
    check_file_no_errors("../../test-data/parser/connections_all.aadl");
}

#[test]
fn test_data_flows_test() {
    check_file_no_errors("../../test-data/parser/flows_test.aadl");
}

#[test]
fn test_data_modes_test() {
    check_file_no_errors("../../test-data/parser/modes_test.aadl");
}

#[test]
fn test_data_properties_test() {
    check_file_no_errors("../../test-data/parser/properties_test.aadl");
}

#[test]
fn test_data_annex_test() {
    check_file_no_errors("../../test-data/parser/annex_test.aadl");
}

#[test]
fn test_data_private_section() {
    check_file_no_errors("../../test-data/parser/private_section.aadl");
}

#[test]
fn test_data_extends_test() {
    check_file_no_errors("../../test-data/parser/extends_test.aadl");
}

#[test]
fn test_data_property_set() {
    check_file_no_errors("../../test-data/parser/property_set.aadl");
}

#[test]
fn test_data_feature_group_type() {
    check_file_no_errors("../../test-data/parser/feature_group_type.aadl");
}

#[test]
fn test_data_complex_system() {
    check_file_no_errors("../../test-data/parser/complex_system.aadl");
}

#[test]
fn test_data_with_clauses() {
    check_file_no_errors("../../test-data/parser/with_clauses.aadl");
}

// Regression for #125: enumeration property definition followed by another
// property must not break the property-set loop.
#[test]
fn test_data_property_set_enum_sequenced() {
    check_file_no_errors("../../test-data/parser/property_set_enum_sequenced.aadl");
}

// Regression for #126: inline `units (...)` on aadlreal/aadlinteger.
#[test]
fn test_data_property_set_inline_units() {
    check_file_no_errors("../../test-data/parser/property_set_inline_units.aadl");
}

// Regression for #127: binary arithmetic in property values.
#[test]
fn test_data_property_value_arithmetic() {
    check_file_no_errors("../../test-data/parser/property_value_arithmetic.aadl");
}

// ====================================================================
// OSATE2 test files
// ====================================================================

macro_rules! osate2_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            check_file_no_errors(concat!("../../test-data/osate2/", $file));
        }
    };
}

osate2_test!(
    osate2_aircraft_safety,
    "AircraftSafetyExample_AOADiscrepancy.aadl"
);
osate2_test!(
    osate2_automated_flight_guidance,
    "AutomatedFlightGuidance.aadl"
);
osate2_test!(osate2_bad_access_connections, "BadAccessConnections.aadl");
osate2_test!(osate2_basic_access, "BasicAccess.aadl");
osate2_test!(osate2_basic_binding, "BasicBinding.aadl");
osate2_test!(osate2_basic_end_to_end_flow, "BasicEndToEndFlow.aadl");
osate2_test!(osate2_basic_hierarchy, "BasicHierarchy.aadl");
osate2_test!(osate2_bus_access_example, "BusAccessExample.aadl");
osate2_test!(osate2_case_position_control, "CasePositionControl.aadl");
osate2_test!(osate2_combined_etef, "CombinedETEF.aadl");
osate2_test!(osate2_complicated, "Complicated.aadl");
osate2_test!(osate2_connection_in_bus, "ConnectionInBus.aadl");
osate2_test!(osate2_declarative_tests, "DeclarativeTests.aadl");
osate2_test!(osate2_devices, "devices.aadl");
osate2_test!(osate2_digital_control_system, "DigitalControlSystem.aadl");
osate2_test!(osate2_dual_fgs, "DualFGS.aadl");
osate2_test!(osate2_exhaustive, "Exhaustive.aadl");
osate2_test!(osate2_explicit_mapping, "ExplicitMapping.aadl");
osate2_test!(osate2_feature_arrays, "FeatureArrays.aadl");
osate2_test!(osate2_flight_system, "FlightSystem.aadl");
osate2_test!(osate2_flow_order, "flow_order_test.aadl");
osate2_test!(osate2_gps_system, "GPSSystem.aadl");
osate2_test!(osate2_initial_scs, "InitialSCS.aadl");
osate2_test!(osate2_integration, "integration.aadl");
osate2_test!(osate2_issue626, "Issue626.aadl");
osate2_test!(osate2_issue818, "Issue818.aadl");
osate2_test!(osate2_issue931, "issue931.aadl");
osate2_test!(osate2_issue1233, "issue1233.aadl");
osate2_test!(osate2_issue1564, "issue1564.aadl");
osate2_test!(osate2_issue1616, "Issue1616.aadl");
osate2_test!(osate2_issue1954, "Issue1954_test.aadl");
osate2_test!(osate2_issue2722c, "Issue2722C.aadl");
osate2_test!(
    osate2_issue_flow_instantiation,
    "issue_flow_instantiation.aadl"
);
osate2_test!(
    osate2_issue_flow_refined_conn,
    "issue_flow_refined_conn.aadl"
);
osate2_test!(osate2_multi_modal_ping_pong, "multiModalPingPong.aadl");
osate2_test!(osate2_navigation, "Navigation.aadl");
osate2_test!(osate2_ports, "ports.aadl");
osate2_test!(osate2_props, "Props.aadl");
osate2_test!(osate2_refinement, "Refinement.aadl");
osate2_test!(osate2_resource_budgets, "resourcebudgets.aadl");
osate2_test!(osate2_simple_control_system, "SimpleControlSystem.aadl");
osate2_test!(osate2_software, "software.aadl");
osate2_test!(osate2_stop_and_go, "StopAndGo.aadl");
osate2_test!(
    osate2_subprogram_with_subprogram,
    "SubprogramWithSubprogram.aadl"
);
osate2_test!(osate2_super_basic, "SuperBasic.aadl");
osate2_test!(
    osate2_test_abstract_classifier,
    "TestAbstractClassifier.aadl"
);
osate2_test!(
    osate2_test_abstract_feature_refinement,
    "TestAbstractFeatureRefinement.aadl"
);
osate2_test!(osate2_wbs_simple, "wbs_simple.aadl");

// ====================================================================
// Negative tests — files that MUST produce at least one parse error
// ====================================================================

/// Read a file and assert the parser produces at least one error.
/// Also verify losslessness (the tree must still round-trip).
fn check_file_has_errors(path: &str) {
    let input =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));
    let result = parse(&input);
    let errors = result.errors();
    assert!(
        !errors.is_empty(),
        "{path}: expected parse errors but got none"
    );
    // Even with errors, the tree must be lossless
    let root = result.syntax_node();
    assert_eq!(
        root.text().to_string(),
        input,
        "{path}: syntax tree does not round-trip despite errors"
    );
}

macro_rules! negative_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            check_file_has_errors(concat!("../../test-data/negative/", $file));
        }
    };
}

negative_test!(neg_missing_package_name, "missing_package_name.aadl");
negative_test!(neg_missing_end_semicolon, "missing_end_semicolon.aadl");
negative_test!(neg_missing_end_keyword, "missing_end_keyword.aadl");
negative_test!(neg_missing_component_end, "missing_component_end.aadl");
negative_test!(
    neg_missing_colon_in_feature,
    "missing_colon_in_feature.aadl"
);
negative_test!(neg_missing_port_keyword, "missing_port_keyword.aadl");
negative_test!(
    neg_invalid_connection_arrow,
    "invalid_connection_arrow.aadl"
);
negative_test!(neg_missing_impl_dot, "missing_impl_dot.aadl");
negative_test!(neg_invalid_flow_kind, "invalid_flow_kind.aadl");
negative_test!(neg_missing_property_arrow, "missing_property_arrow.aadl");
negative_test!(neg_invalid_top_level, "invalid_top_level.aadl");
negative_test!(neg_missing_connection_kind, "missing_connection_kind.aadl");
negative_test!(neg_missing_access_keyword, "missing_access_keyword.aadl");
negative_test!(neg_unclosed_property_block, "unclosed_property_block.aadl");
negative_test!(
    neg_missing_subcomponent_category,
    "missing_subcomponent_category.aadl"
);
negative_test!(neg_missing_mode_keyword, "missing_mode_keyword.aadl");
negative_test!(
    neg_invalid_virtual_category,
    "invalid_virtual_category.aadl"
);
negative_test!(neg_missing_with_semicolon, "missing_with_semicolon.aadl");
negative_test!(neg_missing_property_set_is, "missing_property_set_is.aadl");
negative_test!(neg_missing_call_subprogram, "missing_call_subprogram.aadl");

// ====================================================================
// OSATE2 inline extracted tests
// ====================================================================

macro_rules! inline_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            check_file_no_errors(concat!("../../test-data/osate2-inline/", $file));
        }
    };
}

inline_test!(inline_all_14_categories, "all_14_categories.aadl");
inline_test!(inline_array_size_property, "array_size_property.aadl");
inline_test!(inline_array_subcomponents, "array_subcomponents.aadl");
inline_test!(inline_deep_fg_connections, "deep_fg_connections.aadl");
inline_test!(
    inline_duplicate_case_insensitive,
    "duplicate_case_insensitive.aadl"
);
inline_test!(inline_e2e_flow_fg_members, "e2e_flow_fg_members.aadl");
inline_test!(inline_fg_flow_impl, "fg_flow_impl.aadl");
inline_test!(inline_fg_flows_inverse, "fg_flows_inverse.aadl");
inline_test!(inline_fg_inverse_extends, "fg_inverse_extends.aadl");
inline_test!(inline_fg_inverse_features, "fg_inverse_features.aadl");
inline_test!(inline_fg_inverse_type, "fg_inverse_type.aadl");
inline_test!(inline_fg_subset_matching, "fg_subset_matching.aadl");
inline_test!(inline_fg_unique_names, "fg_unique_names.aadl");
inline_test!(inline_flow_path_fg, "flow_path_fg.aadl");
inline_test!(inline_flow_segments_calls, "flow_segments_calls.aadl");
inline_test!(inline_impl_reference_list, "impl_reference_list.aadl");
inline_test!(inline_in_modes_and_flows, "in_modes_and_flows.aadl");
inline_test!(
    inline_internal_processor_features,
    "internal_processor_features.aadl"
);
inline_test!(inline_modal_property_values, "modal_property_values.aadl");
inline_test!(inline_mode_transitions_scope, "mode_transitions_scope.aadl");
inline_test!(inline_nested_fg_connections, "nested_fg_connections.aadl");
inline_test!(inline_property_set_units, "property_set_units.aadl");
inline_test!(inline_prototype_bindings, "prototype_bindings.aadl");
inline_test!(inline_refined_elements, "refined_elements.aadl");
inline_test!(inline_renames_all_kinds, "renames_all_kinds.aadl");
inline_test!(inline_requires_modes, "requires_modes.aadl");
inline_test!(inline_thread_group_e2e_flow, "thread_group_e2e_flow.aadl");
inline_test!(inline_type_unique_names, "type_unique_names.aadl");

// ====================================================================
// AADL2Rust example tests
// ====================================================================

macro_rules! aadl2rust_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            check_file_no_errors(concat!("../../test-data/aadl2rust/", $file));
        }
    };
}

aadl2rust_test!(a2r_arrays, "arrays.aadl");
aadl2rust_test!(a2r_base_types, "base_types.aadl");
aadl2rust_test!(a2r_bit_codec, "bit_codec.aadl");
aadl2rust_test!(a2r_building_control, "building_control.aadl");
aadl2rust_test!(a2r_car_devices, "car_devices.aadl");
aadl2rust_test!(a2r_car_icd, "car_icd.aadl");
aadl2rust_test!(a2r_car_integration, "car_integration.aadl");
aadl2rust_test!(a2r_composite_types, "composite_types.aadl");
aadl2rust_test!(a2r_flight_control, "flight_control_system.aadl");
aadl2rust_test!(a2r_minepump, "minepump.aadl");
aadl2rust_test!(a2r_modevva_data_access, "modevva_data_access.aadl");
aadl2rust_test!(a2r_obstacle_detection_ba, "obstacle_detection_ba.aadl");
aadl2rust_test!(a2r_partitioned_system, "partitioned_system.aadl");
aadl2rust_test!(a2r_pathfinder_software, "pathfinder_software.aadl");
aadl2rust_test!(a2r_producer_consumer, "producer_consumer.aadl");
aadl2rust_test!(a2r_radar_system, "radar_system.aadl");
aadl2rust_test!(a2r_radar_types, "radar_types.aadl");
aadl2rust_test!(a2r_ravenscar, "ravenscar_example.aadl");
aadl2rust_test!(a2r_robotv2_ba, "robotv2_behavior_annex.aadl");
aadl2rust_test!(a2r_rpc, "rpc.aadl");
aadl2rust_test!(a2r_satellite_software, "satellite_software.aadl");
aadl2rust_test!(a2r_sunseeker, "sunseeker.aadl");
aadl2rust_test!(a2r_testsubprogram, "testsubprogram.aadl");
aadl2rust_test!(a2r_wbs_tire_monitor, "wbs_tire_monitor.aadl");

// ====================================================================
// STPA-REQ-003: Production coverage — untested grammar productions
// ====================================================================

// --- Type extensions ---
aadl2rust_test!(a2r_component_type_extends, "component_type_extends.aadl");
aadl2rust_test!(a2r_process_type_extends, "process_type_extends.aadl");

// --- Feature group type variants ---
aadl2rust_test!(
    a2r_feature_group_with_properties,
    "feature_group_with_properties.aadl"
);
aadl2rust_test!(a2r_feature_group_extends, "feature_group_extends.aadl");
aadl2rust_test!(
    a2r_feature_group_inverse_of,
    "feature_group_inverse_of.aadl"
);

// --- Prototypes ---
aadl2rust_test!(a2r_prototype_component, "prototype_component.aadl");
aadl2rust_test!(a2r_prototype_feature, "prototype_feature.aadl");

// --- Call sequences ---
aadl2rust_test!(a2r_call_sequence_basic, "call_sequence_basic.aadl");
aadl2rust_test!(
    a2r_call_sequence_multiple_calls,
    "call_sequence_multiple_calls.aadl"
);

// --- Connection kinds ---
aadl2rust_test!(a2r_parameter_connection, "parameter_connection.aadl");
aadl2rust_test!(a2r_data_access_connection, "data_access_connection.aadl");
aadl2rust_test!(
    a2r_feature_group_connection,
    "feature_group_connection.aadl"
);
aadl2rust_test!(
    a2r_bidirectional_connection,
    "bidirectional_connection.aadl"
);

// --- Property value kinds ---
aadl2rust_test!(a2r_property_string_value, "property_string_value.aadl");
aadl2rust_test!(a2r_property_record_value, "property_record_value.aadl");
aadl2rust_test!(a2r_property_list_value, "property_list_value.aadl");
aadl2rust_test!(a2r_property_range_value, "property_range_value.aadl");
aadl2rust_test!(a2r_property_computed_value, "property_computed_value.aadl");
aadl2rust_test!(a2r_property_boolean_values, "property_boolean_values.aadl");
aadl2rust_test!(
    a2r_property_append_operator,
    "property_append_operator.aadl"
);

// --- Annexes ---
aadl2rust_test!(a2r_annex_library_package, "annex_library_package.aadl");
aadl2rust_test!(
    a2r_annex_subclause_component,
    "annex_subclause_component.aadl"
);

// --- Abstract features ---
aadl2rust_test!(
    a2r_abstract_feature_declaration,
    "abstract_feature_declaration.aadl"
);

// --- Modal property values ---
aadl2rust_test!(a2r_modal_property_value, "modal_property_value.aadl");

// --- Renames ---
aadl2rust_test!(a2r_renames_package, "renames_package.aadl");
aadl2rust_test!(a2r_renames_component, "renames_component.aadl");
aadl2rust_test!(a2r_renames_all, "renames_all.aadl");

// --- Arrays ---
aadl2rust_test!(a2r_subcomponent_array, "subcomponent_array.aadl");
aadl2rust_test!(
    a2r_feature_with_array_dimension,
    "feature_with_array_dimension.aadl"
);

// --- Property set type declarations ---
aadl2rust_test!(
    a2r_property_set_with_enumeration,
    "property_set_with_enumeration.aadl"
);
aadl2rust_test!(a2r_property_set_with_units, "property_set_with_units.aadl");
aadl2rust_test!(
    a2r_property_set_with_record_type,
    "property_set_with_record_type.aadl"
);

// --- Miscellaneous productions ---
aadl2rust_test!(a2r_none_sections, "none_sections.aadl");
aadl2rust_test!(a2r_refined_feature, "refined_feature.aadl");
aadl2rust_test!(a2r_requires_modes_section, "requires_modes_section.aadl");
aadl2rust_test!(a2r_connection_in_modes, "connection_in_modes.aadl");
aadl2rust_test!(a2r_flow_spec_in_modes, "flow_spec_in_modes.aadl");
aadl2rust_test!(a2r_virtual_bus_type, "virtual_bus_type.aadl");
aadl2rust_test!(a2r_virtual_processor_type, "virtual_processor_type.aadl");

// ====================================================================
// AADL v2.3 (AS5506D) parser extension tests
// ====================================================================

#[test]
fn v23_abstract_feature_with_classifier() {
    let input = "\
package V23_Test
public
  system S
    features
      f1 : feature;
      f2 : feature classifier(Pkg::DataType);
      f3 : in feature classifier(Pkg::DataType);
  end S;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_abstract_feature_with_plain_ref() {
    // v2.2 style: abstract feature with plain classifier reference still works
    let input = "\
package V23_Test
public
  system S
    features
      f1 : feature Pkg::DataType;
  end S;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_range_expression_real() {
    let input = "\
package V23_Test
public
  thread T
    properties
      Compute_Execution_Time => 1.0 ms .. 10.0 ms;
  end T;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_range_with_delta() {
    let input = "\
package V23_Test
public
  thread T
    properties
      Compute_Execution_Time => 1 ms .. 10 ms delta 1 ms;
  end T;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_compute_property_expression() {
    let input = "\
package V23_Test
public
  thread T
    properties
      Compute_Execution_Time => compute (CompTime);
  end T;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_annex_file_reference() {
    let input = "\
package V23_Test
public
  system S
    annex EMV2 {** file(\"errors.emv2\") **};
  end S;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_annex_library_file_reference() {
    let input = "\
package V23_Test
public
  annex EMV2 {** file(\"error_library.emv2\") **};
  system S
  end S;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_interface_feature_group_type() {
    let input = "\
package V23_Test
public
  interface feature group IFG
    features
      data_out : out data port;
      status : in event port;
  end IFG;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_record_property_value() {
    let input = "\
package V23_Test
public
  thread T
    properties
      SEI::GrossWeight => [ value => 100; unit => kg; ];
  end T;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_identifier_range_expression() {
    // Range expression with identifier-based values
    let input = "\
package V23_Test
public
  thread T
    properties
      Compute_Execution_Time => Low .. High;
  end T;
end V23_Test;
";
    check_no_errors(input);
    check_lossless(input);
}

#[test]
fn v23_existing_v22_still_works() {
    // Comprehensive v2.2 model that must still parse correctly
    let input = "\
package Regression
public
  with Base_Types;

  system Controller
    features
      sensor_in : in data port;
      actuator_out : out data port;
      status : out event port;
      bus_access : requires bus access;
    flows
      ctrl_flow : flow path sensor_in -> actuator_out;
    modes
      nominal : initial mode;
      degraded : mode;
      t1 : nominal -[status]-> degraded;
    properties
      Period => 20 ms;
  end Controller;

  system implementation Controller.impl
    subcomponents
      cpu : processor;
      proc : process Proc.impl;
    connections
      c1 : port sensor_in -> proc.inp;
      c2 : port proc.outp -> actuator_out;
    flows
      ctrl_flow : flow path sensor_in -> c1 -> proc.f -> c2 -> actuator_out;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to proc;
  end Controller.impl;

  process Proc
    features
      inp : in data port;
      outp : out data port;
    flows
      f : flow path inp -> outp;
  end Proc;

  process implementation Proc.impl
    subcomponents
      t : thread Worker;
    connections
      c1 : port inp -> t.inp;
      c2 : port t.outp -> outp;
  end Proc.impl;

  thread Worker
    features
      inp : in data port;
      outp : out data port;
    properties
      Dispatch_Protocol => Periodic;
      Period => 20 ms;
      Compute_Execution_Time => 2 ms .. 5 ms;
  end Worker;
end Regression;
";
    check_no_errors(input);
    check_lossless(input);
}
