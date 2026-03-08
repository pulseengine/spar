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
// Test data files
// ====================================================================

fn check_file_no_errors(path: &str) {
    let input = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));
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

osate2_test!(osate2_aircraft_safety, "AircraftSafetyExample_AOADiscrepancy.aadl");
osate2_test!(osate2_automated_flight_guidance, "AutomatedFlightGuidance.aadl");
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
osate2_test!(osate2_issue_flow_instantiation, "issue_flow_instantiation.aadl");
osate2_test!(osate2_issue_flow_refined_conn, "issue_flow_refined_conn.aadl");
osate2_test!(osate2_multi_modal_ping_pong, "multiModalPingPong.aadl");
osate2_test!(osate2_navigation, "Navigation.aadl");
osate2_test!(osate2_ports, "ports.aadl");
osate2_test!(osate2_props, "Props.aadl");
osate2_test!(osate2_refinement, "Refinement.aadl");
osate2_test!(osate2_resource_budgets, "resourcebudgets.aadl");
osate2_test!(osate2_simple_control_system, "SimpleControlSystem.aadl");
osate2_test!(osate2_software, "software.aadl");
osate2_test!(osate2_stop_and_go, "StopAndGo.aadl");
osate2_test!(osate2_subprogram_with_subprogram, "SubprogramWithSubprogram.aadl");
osate2_test!(osate2_super_basic, "SuperBasic.aadl");
osate2_test!(osate2_test_abstract_classifier, "TestAbstractClassifier.aadl");
osate2_test!(osate2_test_abstract_feature_refinement, "TestAbstractFeatureRefinement.aadl");
osate2_test!(osate2_wbs_simple, "wbs_simple.aadl");

// ====================================================================
// Negative tests — files that MUST produce at least one parse error
// ====================================================================

/// Read a file and assert the parser produces at least one error.
/// Also verify losslessness (the tree must still round-trip).
fn check_file_has_errors(path: &str) {
    let input = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));
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
negative_test!(neg_missing_colon_in_feature, "missing_colon_in_feature.aadl");
negative_test!(neg_missing_port_keyword, "missing_port_keyword.aadl");
negative_test!(neg_invalid_connection_arrow, "invalid_connection_arrow.aadl");
negative_test!(neg_missing_impl_dot, "missing_impl_dot.aadl");
negative_test!(neg_invalid_flow_kind, "invalid_flow_kind.aadl");
negative_test!(neg_missing_property_arrow, "missing_property_arrow.aadl");
negative_test!(neg_invalid_top_level, "invalid_top_level.aadl");
negative_test!(neg_missing_connection_kind, "missing_connection_kind.aadl");
negative_test!(neg_missing_access_keyword, "missing_access_keyword.aadl");
negative_test!(neg_unclosed_property_block, "unclosed_property_block.aadl");
negative_test!(neg_missing_subcomponent_category, "missing_subcomponent_category.aadl");
negative_test!(neg_missing_mode_keyword, "missing_mode_keyword.aadl");
negative_test!(neg_invalid_virtual_category, "invalid_virtual_category.aadl");
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
inline_test!(inline_duplicate_case_insensitive, "duplicate_case_insensitive.aadl");
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
inline_test!(inline_internal_processor_features, "internal_processor_features.aadl");
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
