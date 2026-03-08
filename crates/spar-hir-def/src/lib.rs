//! HIR definitions and name resolution for AADL analysis.
//!
//! This crate provides the semantic model layer between the CST
//! (spar-syntax) and high-level analysis (spar-analysis):
//!
//! - [`ItemTree`] — condensed per-file representation of declarations
//! - [`Name`], [`ClassifierRef`] — interned name types
//! - [`GlobalScope`] — cross-file name resolution
//!
//! # Architecture
//!
//! ```text
//! spar-base-db (parse_file) ──▶ spar-hir-def (item tree + names)
//!                                     │
//!                                     ▼
//!                               spar-analysis (type checking, etc.)
//! ```

pub mod category_rules;
pub mod feature_group;
pub mod instance;
pub mod item_tree;
pub mod name;
pub mod properties;
pub mod property_value;
pub mod prototype;
pub mod resolver;
pub mod standard_properties;

use std::sync::Arc;

pub use item_tree::ItemTree;
pub use name::{ClassifierRef, Name, PropertyRef};
pub use resolver::{GlobalScope, ItemLoc, ResolvedClassifier, ResolvedProperty};

/// The salsa database trait for HIR definitions.
///
/// Extends the base database with item tree computation.
#[salsa::db]
pub trait Db: spar_base_db::Db {}

/// Compute the item tree for a source file.
///
/// This is a salsa tracked function: it memoizes the result and
/// recomputes only when the file's parse result changes.
#[salsa::tracked]
pub fn file_item_tree(db: &dyn Db, file: spar_base_db::SourceFile) -> Arc<ItemTree> {
    let parse_result = spar_base_db::parse_file(db, file);
    let root = parse_result.syntax_node();
    Arc::new(item_tree::lower::lower_file(&root))
}

// ── Default database implementation ──────────────────────────────

/// A database that supports both base-db and hir-def queries.
#[salsa::db]
#[derive(Default)]
pub struct HirDefDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for HirDefDatabase {}

#[salsa::db]
impl spar_base_db::Db for HirDefDatabase {}

#[salsa::db]
impl Db for HirDefDatabase {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> HirDefDatabase {
        HirDefDatabase::default()
    }

    #[test]
    fn item_tree_simple_package() {
        let db = make_db();
        let file = spar_base_db::SourceFile::new(
            &db,
            "test.aadl".to_string(),
            "package TestPkg\npublic\n  system Sensor\n  end Sensor;\nend TestPkg;".to_string(),
        );

        let tree = file_item_tree(&db, file);
        assert_eq!(tree.packages.len(), 1);
        let pkg = &tree.packages[tree.packages.iter().next().unwrap().0];
        assert_eq!(pkg.name.as_str(), "TestPkg");
        assert_eq!(pkg.public_items.len(), 1);

        // Should have one component type
        assert_eq!(tree.component_types.len(), 1);
        let ct = &tree.component_types[tree.component_types.iter().next().unwrap().0];
        assert_eq!(ct.name.as_str(), "Sensor");
        assert_eq!(ct.category, item_tree::ComponentCategory::System);
    }

    #[test]
    fn item_tree_with_implementation() {
        let db = make_db();
        let src = r#"package P
public
  system S
    features
      inp : in data port;
  end S;
  system implementation S.impl
    subcomponents
      sub1 : system;
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "p.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        assert_eq!(tree.component_types.len(), 1);
        assert_eq!(tree.component_impls.len(), 1);

        let ci = &tree.component_impls[tree.component_impls.iter().next().unwrap().0];
        assert_eq!(ci.type_name.as_str(), "S");
        assert_eq!(ci.impl_name.as_str(), "impl");
        assert_eq!(ci.subcomponents.len(), 1);

        // Check feature
        assert_eq!(tree.features.len(), 1);
        let feat = &tree.features[tree.features.iter().next().unwrap().0];
        assert_eq!(feat.name.as_str(), "inp");
        assert_eq!(feat.kind, item_tree::FeatureKind::DataPort);
        assert_eq!(feat.direction, Some(item_tree::Direction::In));
    }

    #[test]
    fn item_tree_with_clause() {
        let db = make_db();
        let src = r#"package Consumer
public
  with DataTypes;
  system Controller
  end Controller;
end Consumer;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "consumer.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        let pkg = &tree.packages[tree.packages.iter().next().unwrap().0];
        assert_eq!(pkg.name.as_str(), "Consumer");
        assert!(
            pkg.with_clauses
                .iter()
                .any(|n| n.as_str() == "DataTypes"),
            "with_clauses: {:?}",
            pkg.with_clauses
        );
    }

    #[test]
    fn item_tree_property_set() {
        let db = make_db();
        let src = r#"property set MyProps is
  Timeout : aadlinteger applies to (all);
  MaxRetries : constant aadlinteger => 3;
end MyProps;
"#;
        let file = spar_base_db::SourceFile::new(&db, "props.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        assert_eq!(tree.property_sets.len(), 1);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.name.as_str(), "MyProps");
    }

    #[test]
    fn item_tree_feature_group_type() {
        let db = make_db();
        let src = r#"package P
public
  feature group SensorData
    features
      temperature : out data port;
      pressure : out data port;
  end SensorData;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "fg.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        assert_eq!(tree.feature_group_types.len(), 1);
        let fgt = &tree.feature_group_types[tree.feature_group_types.iter().next().unwrap().0];
        assert_eq!(fgt.name.as_str(), "SensorData");
        assert_eq!(fgt.features.len(), 2);
    }

    #[test]
    fn item_tree_connections() {
        let db = make_db();
        let src = r#"package P
public
  system S
  end S;
  system implementation S.impl
    subcomponents
      a : system;
      b : system;
    connections
      c1 : port a.out1 -> b.in1;
      c2 : port b.out1 <-> a.in1;
  end S.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        assert_eq!(tree.connections.len(), 2);
        let c1 = &tree.connections[tree.connections.iter().next().unwrap().0];
        assert_eq!(c1.name.as_str(), "c1");
        assert_eq!(c1.kind, item_tree::ConnectionKind::Port);
        assert!(!c1.is_bidirectional);

        let c2_idx = tree.connections.iter().nth(1).unwrap().0;
        let c2 = &tree.connections[c2_idx];
        assert_eq!(c2.name.as_str(), "c2");
        assert!(c2.is_bidirectional);
    }

    #[test]
    fn item_tree_flow_specs() {
        let db = make_db();
        let src = r#"package P
public
  system S
    features
      sensor_in : in data port;
      cmd_out : out data port;
    flows
      data_flow : flow source cmd_out;
      sense : flow sink sensor_in;
  end S;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "flow.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        assert_eq!(tree.flow_specs.len(), 2);
        let f0 = &tree.flow_specs[tree.flow_specs.iter().next().unwrap().0];
        assert_eq!(f0.name.as_str(), "data_flow");
        assert_eq!(f0.kind, item_tree::FlowKind::Source);
    }

    #[test]
    fn cross_file_name_resolution() {
        let db = make_db();
        let types_src = r#"package DataTypes
public
  data Temperature
  end Temperature;
end DataTypes;
"#;
        let consumer_src = r#"package Consumer
public
  with DataTypes;
  system Controller
    features
      temp : in data port DataTypes::Temperature;
  end Controller;
end Consumer;
"#;
        let f1 = spar_base_db::SourceFile::new(
            &db,
            "datatypes.aadl".to_string(),
            types_src.to_string(),
        );
        let f2 = spar_base_db::SourceFile::new(
            &db,
            "consumer.aadl".to_string(),
            consumer_src.to_string(),
        );

        let tree1 = file_item_tree(&db, f1);
        let tree2 = file_item_tree(&db, f2);

        // Build global scope from both files
        let scope = GlobalScope::from_trees(vec![tree1, tree2]);

        // Resolve DataTypes::Temperature from Consumer
        let reference = ClassifierRef::qualified(
            Name::new("DataTypes"),
            Name::new("Temperature"),
        );
        let resolved = scope.resolve_classifier(&Name::new("Consumer"), &reference);
        assert!(
            matches!(resolved, ResolvedClassifier::ComponentType { .. }),
            "expected ComponentType, got {:?}",
            resolved
        );
    }

    #[test]
    fn incremental_item_tree() {
        use salsa::Setter;
        let mut db = make_db();

        let file = spar_base_db::SourceFile::new(
            &db,
            "test.aadl".to_string(),
            "package V1\npublic\n  system A\n  end A;\nend V1;".to_string(),
        );

        let tree1 = file_item_tree(&db, file);
        assert_eq!(tree1.component_types.len(), 1);

        // Change the file — salsa should recompute
        file.set_text(&mut db).to(
            "package V1\npublic\n  system A\n  end A;\n  system B\n  end B;\nend V1;".to_string(),
        );

        let tree2 = file_item_tree(&db, file);
        assert_eq!(tree2.component_types.len(), 2);
    }

    #[test]
    fn instance_model_basic() {
        let db = make_db();
        let src = r#"package FlightControl
public
  system Controller
    features
      sensor_in : in data port;
      cmd_out : out data port;
  end Controller;

  system implementation Controller.basic
    subcomponents
      nav : system;
      guidance : system;
    connections
      c1 : port nav.out1 -> guidance.in1;
  end Controller.basic;
end FlightControl;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "flight.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("FlightControl"),
            &Name::new("Controller"),
            &Name::new("basic"),
        );

        // Root + 2 subcomponents = 3 component instances
        assert_eq!(
            inst.component_count(),
            3,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        // Root has 2 features from the type
        let root = inst.component(inst.root);
        assert_eq!(root.features.len(), 2);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.connections.len(), 1);

        // Check feature names
        let f0 = &inst.features[root.features[0]];
        let f1 = &inst.features[root.features[1]];
        assert_eq!(f0.name.as_str(), "sensor_in");
        assert_eq!(f1.name.as_str(), "cmd_out");
    }

    #[test]
    fn instance_model_nested() {
        let db = make_db();
        let src = r#"package Sys
public
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      sub_a : system Mid.impl;
  end Top.impl;

  system Mid
  end Mid;
  system implementation Mid.impl
    subcomponents
      leaf1 : system;
      leaf2 : system;
  end Mid.impl;
end Sys;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "nested.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("Sys"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // Top + sub_a + leaf1 + leaf2 = 4 component instances
        assert_eq!(
            inst.component_count(),
            4,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        // Root has 1 child (sub_a), which has 2 children (leaf1, leaf2)
        let root = inst.component(inst.root);
        assert_eq!(root.children.len(), 1);

        let sub_a = inst.component(root.children[0]);
        assert_eq!(sub_a.name.as_str(), "sub_a");
        assert_eq!(sub_a.children.len(), 2);
    }

    #[test]
    fn instance_model_cross_package() {
        let db = make_db();
        let types_src = r#"package SensorLib
public
  system TempSensor
    features
      reading : out data port;
  end TempSensor;
  system implementation TempSensor.basic
  end TempSensor.basic;
end SensorLib;
"#;
        let main_src = r#"package Vehicle
public
  with SensorLib;
  system ECU
  end ECU;
  system implementation ECU.impl
    subcomponents
      temp : system SensorLib::TempSensor.basic;
  end ECU.impl;
end Vehicle;
"#;
        let f1 = spar_base_db::SourceFile::new(
            &db,
            "sensor.aadl".to_string(),
            types_src.to_string(),
        );
        let f2 = spar_base_db::SourceFile::new(
            &db,
            "vehicle.aadl".to_string(),
            main_src.to_string(),
        );
        let t1 = file_item_tree(&db, f1);
        let t2 = file_item_tree(&db, f2);
        let scope = GlobalScope::from_trees(vec![t1, t2]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("Vehicle"),
            &Name::new("ECU"),
            &Name::new("impl"),
        );

        // ECU.impl + temp (TempSensor.basic) = 2
        assert_eq!(
            inst.component_count(),
            2,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let root = inst.component(inst.root);
        assert_eq!(root.children.len(), 1);

        let temp = inst.component(root.children[0]);
        assert_eq!(temp.name.as_str(), "temp");
        assert_eq!(temp.type_name.as_str(), "TempSensor");
        assert_eq!(temp.impl_name.as_ref().unwrap().as_str(), "basic");
        // Cross-package reference resolved — temp has 1 feature from SensorLib::TempSensor
        assert_eq!(temp.features.len(), 1);
        let feat = &inst.features[temp.features[0]];
        assert_eq!(feat.name.as_str(), "reading");
    }

    // ── Property evaluation tests ────────────────────────────────────

    #[test]
    fn property_extraction_from_cst() {
        let db = make_db();
        let src = r#"package P
public
  system S
    properties
      Deployment::Priority => 5;
      Period => 10 ms;
  end S;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "props.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        let ct = &tree.component_types[tree.component_types.iter().next().unwrap().0];
        assert_eq!(ct.name.as_str(), "S");
        assert_eq!(
            ct.property_associations.len(),
            2,
            "expected 2 property associations, got {}",
            ct.property_associations.len()
        );

        // Check first property: Deployment::Priority => 5
        let pa0 = &tree.property_associations[ct.property_associations[0]];
        assert_eq!(pa0.name.property_set.as_ref().unwrap().as_str(), "Deployment");
        assert_eq!(pa0.name.property_name.as_str(), "Priority");
        assert_eq!(pa0.value, "5");
        assert!(!pa0.is_append);

        // Check second property: Period => 10 ms
        let pa1 = &tree.property_associations[ct.property_associations[1]];
        assert!(pa1.name.property_set.is_none());
        assert_eq!(pa1.name.property_name.as_str(), "Period");
        assert_eq!(pa1.value, "10 ms");
        assert!(!pa1.is_append);
    }

    #[test]
    fn property_inheritance_type_to_impl() {
        let db = make_db();
        let src = r#"package P
public
  system S
    properties
      Deployment::Priority => 5;
      Period => 10 ms;
  end S;
  system implementation S.impl
    properties
      Period => 20 ms;
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "inh.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        let ct_idx = tree.component_types.iter().next().unwrap().0;
        let ci_idx = tree.component_impls.iter().next().unwrap().0;

        let map = properties::PropertyMap::collect_for_component(&tree, Some(ct_idx), Some(ci_idx));

        // Period should be overridden by impl
        assert_eq!(map.get("", "Period"), Some("20 ms"));
        // Priority should be inherited from type
        assert_eq!(map.get("Deployment", "Priority"), Some("5"));
    }

    #[test]
    fn property_on_subcomponent() {
        let db = make_db();
        let src = r#"package P
public
  system S
    properties
      Period => 10 ms;
  end S;
  system implementation S.impl
  end S.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      child : system S.impl { Period => 50 ms; };
  end Top.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "sub.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // The root has 1 child
        let root = inst.component(inst.root);
        assert_eq!(root.children.len(), 1, "diagnostics: {:?}", inst.diagnostics);

        let child_idx = root.children[0];
        let props = inst.properties_for(child_idx);

        // Subcomponent property should override type property
        assert_eq!(
            props.get("", "Period"),
            Some("50 ms"),
            "property map: {:?}",
            props
        );
    }

    #[test]
    fn property_append() {
        let db = make_db();
        let src = r#"package P
public
  system S
    properties
      Allowed_Processor_Binding => (reference(cpu1));
  end S;
  system implementation S.impl
    properties
      Allowed_Processor_Binding +=> (reference(cpu2));
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "append.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        let ct_idx = tree.component_types.iter().next().unwrap().0;
        let ci_idx = tree.component_impls.iter().next().unwrap().0;

        let map = properties::PropertyMap::collect_for_component(&tree, Some(ct_idx), Some(ci_idx));

        // Both values should be present (append, not override)
        let values = map.get_all("", "Allowed_Processor_Binding");
        assert_eq!(values.len(), 2, "expected 2 values, got: {:?}", values);
        // Property values are extracted as raw text with normalized whitespace
        assert!(values[0].contains("reference") && values[0].contains("cpu1"));
        assert!(values[1].contains("reference") && values[1].contains("cpu2"));
    }

    #[test]
    fn property_cross_package_resolution() {
        let db = make_db();
        let props_src = r#"property set Timing is
  Period : aadlinteger applies to (all);
  Deadline : aadlinteger applies to (all);
end Timing;
"#;
        let main_src = r#"package Vehicle
public
  with Timing;
  system ECU
    properties
      Timing::Period => 100;
      Timing::Deadline => 200;
  end ECU;
  system implementation ECU.impl
  end ECU.impl;
end Vehicle;
"#;
        let f1 = spar_base_db::SourceFile::new(
            &db,
            "timing.aadl".to_string(),
            props_src.to_string(),
        );
        let f2 = spar_base_db::SourceFile::new(
            &db,
            "vehicle.aadl".to_string(),
            main_src.to_string(),
        );
        let t1 = file_item_tree(&db, f1);
        let t2 = file_item_tree(&db, f2);
        let scope = GlobalScope::from_trees(vec![t1, t2]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("Vehicle"),
            &Name::new("ECU"),
            &Name::new("impl"),
        );

        let root_props = inst.properties_for(inst.root);

        assert_eq!(
            root_props.get("Timing", "Period"),
            Some("100"),
            "property map: {:?}",
            root_props
        );
        assert_eq!(
            root_props.get("Timing", "Deadline"),
            Some("200"),
            "property map: {:?}",
            root_props
        );
    }

    #[test]
    fn property_map_basic_operations() {
        use properties::{PropertyMap, PropertyValue};
        use name::PropertyRef;

        let mut map = PropertyMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        // Add a property
        map.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Timing")),
                property_name: Name::new("Period"),
            },
            value: "100 ms".to_string(),
            is_append: false,
        });

        assert_eq!(map.len(), 1);
        assert!(!map.is_empty());
        assert_eq!(map.get("Timing", "Period"), Some("100 ms"));
        assert_eq!(map.get("timing", "period"), Some("100 ms")); // case-insensitive

        // Override
        map.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Timing")),
                property_name: Name::new("Period"),
            },
            value: "200 ms".to_string(),
            is_append: false,
        });

        assert_eq!(map.len(), 1); // still 1 key
        assert_eq!(map.get("Timing", "Period"), Some("200 ms"));

        // Append
        map.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Timing")),
                property_name: Name::new("Period"),
            },
            value: "300 ms".to_string(),
            is_append: true,
        });

        let all = map.get_all("Timing", "Period");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], "200 ms");
        assert_eq!(all[1], "300 ms");
    }

    #[test]
    fn property_instance_inheritance_chain() {
        // Full chain: type -> impl -> subcomponent, all with properties
        let db = make_db();
        let src = r#"package P
public
  system Sensor
    properties
      Period => 10 ms;
      Deadline => 5 ms;
  end Sensor;
  system implementation Sensor.basic
    properties
      Period => 20 ms;
      Priority => 3;
  end Sensor.basic;
  system Controller
  end Controller;
  system implementation Controller.impl
    subcomponents
      s1 : system Sensor.basic { Deadline => 15 ms; };
  end Controller.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "chain.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Controller"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(root.children.len(), 1, "diagnostics: {:?}", inst.diagnostics);

        let s1_idx = root.children[0];
        let props = inst.properties_for(s1_idx);

        // Period: type says 10ms, impl says 20ms -> impl wins
        assert_eq!(props.get("", "Period"), Some("20 ms"), "props: {:?}", props);

        // Deadline: type says 5ms, subcomponent says 15ms -> subcomponent wins
        assert_eq!(props.get("", "Deadline"), Some("15 ms"), "props: {:?}", props);

        // Priority: only on impl -> inherited
        assert_eq!(props.get("", "Priority"), Some("3"), "props: {:?}", props);
    }

    // ── Flow instance tests ───────────────────────────────────────────

    #[test]
    fn flow_instances_from_type() {
        let db = make_db();
        let src = r#"package P
public
  system S
    features
      sensor_in : in data port;
      cmd_out : out data port;
    flows
      data_flow : flow source cmd_out;
      sense : flow sink sensor_in;
      process_data : flow path sensor_in -> cmd_out;
  end S;
  system implementation S.impl
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "flow_inst.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.flows.len(),
            3,
            "expected 3 flow instances, got {}; diagnostics: {:?}",
            root.flows.len(),
            inst.diagnostics
        );

        // Check flow kinds
        let f0 = &inst.flow_instances[root.flows[0]];
        assert_eq!(f0.name.as_str(), "data_flow");
        assert_eq!(f0.kind, item_tree::FlowKind::Source);

        let f1 = &inst.flow_instances[root.flows[1]];
        assert_eq!(f1.name.as_str(), "sense");
        assert_eq!(f1.kind, item_tree::FlowKind::Sink);

        let f2 = &inst.flow_instances[root.flows[2]];
        assert_eq!(f2.name.as_str(), "process_data");
        assert_eq!(f2.kind, item_tree::FlowKind::Path);

        // Check total flow instance count
        assert_eq!(inst.flow_instances.len(), 3);
    }

    #[test]
    fn connection_endpoints_extracted() {
        let db = make_db();
        let src = r#"package P
public
  system S
    features
      ext_in : in data port;
  end S;
  system implementation S.impl
    subcomponents
      a : system;
      b : system;
    connections
      c1 : port a.out1 -> b.in1;
      c2 : port ext_in -> a.in1;
      c3 : port b.out1 <-> a.in2;
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "conn_end.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        // Check item tree level first
        let conn_items: Vec<_> = tree.connections.iter().collect();
        assert_eq!(conn_items.len(), 3);

        // c1: a.out1 -> b.in1
        let c1 = &tree.connections[conn_items[0].0];
        assert_eq!(c1.name.as_str(), "c1");
        let c1_src = c1.src.as_ref().expect("c1 should have src");
        assert_eq!(c1_src.subcomponent.as_ref().unwrap().as_str(), "a");
        assert_eq!(c1_src.feature.as_str(), "out1");
        let c1_dst = c1.dst.as_ref().expect("c1 should have dst");
        assert_eq!(c1_dst.subcomponent.as_ref().unwrap().as_str(), "b");
        assert_eq!(c1_dst.feature.as_str(), "in1");

        // c2: ext_in -> a.in1 (ext_in is on the containing component)
        let c2 = &tree.connections[conn_items[1].0];
        assert_eq!(c2.name.as_str(), "c2");
        let c2_src = c2.src.as_ref().expect("c2 should have src");
        assert!(c2_src.subcomponent.is_none(), "ext_in has no subcomponent");
        assert_eq!(c2_src.feature.as_str(), "ext_in");
        let c2_dst = c2.dst.as_ref().expect("c2 should have dst");
        assert_eq!(c2_dst.subcomponent.as_ref().unwrap().as_str(), "a");
        assert_eq!(c2_dst.feature.as_str(), "in1");

        // Check at instance level too
        let scope = GlobalScope::from_trees(vec![file_item_tree(&db, file)]);
        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(root.connections.len(), 3);

        let ic1 = &inst.connections[root.connections[0]];
        assert_eq!(ic1.name.as_str(), "c1");
        let ic1_src = ic1.src.as_ref().unwrap();
        assert_eq!(ic1_src.subcomponent.as_ref().unwrap().as_str(), "a");
        assert_eq!(ic1_src.feature.as_str(), "out1");

        let ic2 = &inst.connections[root.connections[1]];
        assert_eq!(ic2.name.as_str(), "c2");
        let ic2_src = ic2.src.as_ref().unwrap();
        assert!(ic2_src.subcomponent.is_none());
        assert_eq!(ic2_src.feature.as_str(), "ext_in");
    }

    #[test]
    fn summary_method() {
        let db = make_db();
        let src = r#"package P
public
  system S
    features
      sensor_in : in data port;
      cmd_out : out data port;
    flows
      data_flow : flow source cmd_out;
      sense : flow sink sensor_in;
  end S;
  system implementation S.impl
    subcomponents
      a : system;
      b : system;
    connections
      c1 : port a.out1 -> b.in1;
    flows
      total : end to end flow a -> c1 -> b;
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "summary.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("impl"),
        );

        let summary = inst.summary();
        assert!(summary.contains("Components: 3"), "summary: {}", summary);
        assert!(summary.contains("Features: 2"), "summary: {}", summary);
        assert!(summary.contains("Connections: 1"), "summary: {}", summary);
        assert!(summary.contains("Flows: 2"), "summary: {}", summary);
        assert!(summary.contains("End-to-end flows: 1"), "summary: {}", summary);
        assert!(summary.contains("Diagnostics: 0"), "summary: {}", summary);
    }

    #[test]
    fn complex_model_with_flows_and_connections() {
        let db = make_db();
        let src = r#"package FlightSystem
public
  system Sensor
    features
      reading : out data port;
    flows
      data_src : flow source reading;
  end Sensor;
  system implementation Sensor.basic
  end Sensor.basic;

  system Actuator
    features
      command : in data port;
    flows
      cmd_sink : flow sink command;
  end Actuator;
  system implementation Actuator.basic
  end Actuator.basic;

  system Controller
    features
      sensor_in : in data port;
      cmd_out : out data port;
    flows
      control_path : flow path sensor_in -> cmd_out;
  end Controller;
  system implementation Controller.basic
  end Controller.basic;

  system FlightControl
  end FlightControl;
  system implementation FlightControl.full
    subcomponents
      sensor : system Sensor.basic;
      ctrl : system Controller.basic;
      actuator : system Actuator.basic;
    connections
      c1 : port sensor.reading -> ctrl.sensor_in;
      c2 : port ctrl.cmd_out -> actuator.command;
    flows
      e2e_control : end to end flow sensor.data_src -> c1 -> ctrl.control_path -> c2 -> actuator.cmd_sink;
  end FlightControl.full;
end FlightSystem;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "flight_system.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("FlightSystem"),
            &Name::new("FlightControl"),
            &Name::new("full"),
        );

        // Root + sensor + ctrl + actuator = 4 components
        assert_eq!(
            inst.component_count(),
            4,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let root = inst.component(inst.root);
        assert_eq!(root.children.len(), 3);
        assert_eq!(root.connections.len(), 2);

        // Sensor has 1 flow (source), Controller has 1 flow (path), Actuator has 1 flow (sink)
        let sensor = inst.component(root.children[0]);
        assert_eq!(sensor.name.as_str(), "sensor");
        assert_eq!(sensor.features.len(), 1);
        assert_eq!(sensor.flows.len(), 1);
        let sensor_flow = &inst.flow_instances[sensor.flows[0]];
        assert_eq!(sensor_flow.name.as_str(), "data_src");
        assert_eq!(sensor_flow.kind, item_tree::FlowKind::Source);

        let ctrl = inst.component(root.children[1]);
        assert_eq!(ctrl.name.as_str(), "ctrl");
        assert_eq!(ctrl.flows.len(), 1);
        let ctrl_flow = &inst.flow_instances[ctrl.flows[0]];
        assert_eq!(ctrl_flow.kind, item_tree::FlowKind::Path);

        let actuator = inst.component(root.children[2]);
        assert_eq!(actuator.name.as_str(), "actuator");
        assert_eq!(actuator.flows.len(), 1);
        let act_flow = &inst.flow_instances[actuator.flows[0]];
        assert_eq!(act_flow.kind, item_tree::FlowKind::Sink);

        // Total flow instances: 3 (one per subcomponent type)
        assert_eq!(inst.flow_instances.len(), 3);

        // 1 end-to-end flow
        assert_eq!(inst.end_to_end_flows.len(), 1);
        let e2e = &inst.end_to_end_flows[inst.end_to_end_flows.iter().next().unwrap().0];
        assert_eq!(e2e.name.as_str(), "e2e_control");
        assert_eq!(
            e2e.segments.len(),
            5,
            "expected 5 segments, got {:?}",
            e2e.segments.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        );

        // Connection endpoints
        let c1 = &inst.connections[root.connections[0]];
        assert_eq!(c1.name.as_str(), "c1");
        let c1_src = c1.src.as_ref().unwrap();
        assert_eq!(c1_src.subcomponent.as_ref().unwrap().as_str(), "sensor");
        assert_eq!(c1_src.feature.as_str(), "reading");
        let c1_dst = c1.dst.as_ref().unwrap();
        assert_eq!(c1_dst.subcomponent.as_ref().unwrap().as_str(), "ctrl");
        assert_eq!(c1_dst.feature.as_str(), "sensor_in");

        let c2 = &inst.connections[root.connections[1]];
        let c2_src = c2.src.as_ref().unwrap();
        assert_eq!(c2_src.subcomponent.as_ref().unwrap().as_str(), "ctrl");
        assert_eq!(c2_src.feature.as_str(), "cmd_out");
        let c2_dst = c2.dst.as_ref().unwrap();
        assert_eq!(c2_dst.subcomponent.as_ref().unwrap().as_str(), "actuator");
        assert_eq!(c2_dst.feature.as_str(), "command");

        // Summary check
        let summary = inst.summary();
        assert!(summary.contains("Components: 4"), "summary:\n{}", summary);
        assert!(summary.contains("End-to-end flows: 1"), "summary:\n{}", summary);
    }

    // ── Mode instance tests ───────────────────────────────────────────

    #[test]
    fn mode_instances_from_type() {
        let db = make_db();
        let src = r#"package ModePkg
public
  system Controller
    modes
      standby : initial mode;
      active : mode;
      shutdown : mode;
  end Controller;

  system implementation Controller.impl
  end Controller.impl;
end ModePkg;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "modes.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("ModePkg"),
            &Name::new("Controller"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.modes.len(),
            3,
            "expected 3 mode instances, got {}; diagnostics: {:?}",
            root.modes.len(),
            inst.diagnostics
        );

        // Check mode names and initial flag
        let m0 = &inst.mode_instances[root.modes[0]];
        assert_eq!(m0.name.as_str(), "standby");
        assert!(m0.is_initial, "standby should be the initial mode");

        let m1 = &inst.mode_instances[root.modes[1]];
        assert_eq!(m1.name.as_str(), "active");
        assert!(!m1.is_initial);

        let m2 = &inst.mode_instances[root.modes[2]];
        assert_eq!(m2.name.as_str(), "shutdown");
        assert!(!m2.is_initial);

        // Check total mode instance count
        assert_eq!(inst.mode_instances.len(), 3);

        // Helper method
        let modes = inst.modes_for(inst.root);
        assert_eq!(modes.len(), 3);
        assert_eq!(modes[0].name.as_str(), "standby");
        assert!(modes[0].is_initial);

        // Summary includes modes
        let summary = inst.summary();
        assert!(summary.contains("Modes: 3"), "summary:\n{}", summary);
        assert!(summary.contains("Mode transitions: 0"), "summary:\n{}", summary);
    }

    #[test]
    fn mode_transitions_from_type() {
        let db = make_db();
        let src = r#"package TransPkg
public
  system Controller
    features
      cmd : in event port;
      reset : in event port;
    modes
      idle : initial mode;
      running : mode;
      idle -[ cmd ]-> running;
      running -[ reset ]-> idle;
  end Controller;

  system implementation Controller.impl
  end Controller.impl;
end TransPkg;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "transitions.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("TransPkg"),
            &Name::new("Controller"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(root.modes.len(), 2, "expected 2 modes");
        assert_eq!(root.mode_transitions.len(), 2, "expected 2 mode transitions");

        let mt0 = &inst.mode_transition_instances[root.mode_transitions[0]];
        assert_eq!(mt0.source.as_str(), "idle");
        assert_eq!(mt0.destination.as_str(), "running");
        assert_eq!(mt0.triggers.len(), 1);
        assert_eq!(mt0.triggers[0].as_str(), "cmd");

        let mt1 = &inst.mode_transition_instances[root.mode_transitions[1]];
        assert_eq!(mt1.source.as_str(), "running");
        assert_eq!(mt1.destination.as_str(), "idle");
        assert_eq!(mt1.triggers.len(), 1);
        assert_eq!(mt1.triggers[0].as_str(), "reset");

        // Helper method
        let mts = inst.mode_transitions_for(inst.root);
        assert_eq!(mts.len(), 2);

        // Summary
        let summary = inst.summary();
        assert!(summary.contains("Mode transitions: 2"), "summary:\n{}", summary);
    }

    #[test]
    fn mode_instances_from_impl() {
        let db = make_db();
        let src = r#"package ImplModePkg
public
  system Controller
  end Controller;

  system implementation Controller.impl
    modes
      off : initial mode;
      on : mode;
  end Controller.impl;
end ImplModePkg;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "impl_modes.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("ImplModePkg"),
            &Name::new("Controller"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.modes.len(),
            2,
            "expected 2 mode instances from implementation, got {}; diagnostics: {:?}",
            root.modes.len(),
            inst.diagnostics
        );

        let m0 = &inst.mode_instances[root.modes[0]];
        assert_eq!(m0.name.as_str(), "off");
        assert!(m0.is_initial);

        let m1 = &inst.mode_instances[root.modes[1]];
        assert_eq!(m1.name.as_str(), "on");
        assert!(!m1.is_initial);
    }

    #[test]
    fn mode_instances_in_subcomponents() {
        let db = make_db();
        let src = r#"package SubModePkg
public
  system Sensor
    modes
      calibrating : initial mode;
      sensing : mode;
  end Sensor;

  system implementation Sensor.impl
  end Sensor.impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      s1 : system Sensor.impl;
  end Top.impl;
end SubModePkg;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "sub_modes.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("SubModePkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // Root has no modes
        let root = inst.component(inst.root);
        assert_eq!(root.modes.len(), 0);

        // Subcomponent s1 has 2 modes from Sensor type
        assert_eq!(root.children.len(), 1);
        let s1 = inst.component(root.children[0]);
        assert_eq!(s1.name.as_str(), "s1");
        assert_eq!(
            s1.modes.len(),
            2,
            "expected 2 modes on subcomponent s1, got {}",
            s1.modes.len()
        );

        let m0 = &inst.mode_instances[s1.modes[0]];
        assert_eq!(m0.name.as_str(), "calibrating");
        assert!(m0.is_initial);

        let m1 = &inst.mode_instances[s1.modes[1]];
        assert_eq!(m1.name.as_str(), "sensing");
        assert!(!m1.is_initial);

        // Total mode instances in the system
        assert_eq!(inst.mode_instances.len(), 2);
    }

    // ── Semantic connection tests ─────────────────────────────────────

    #[test]
    fn semantic_connections_simple_across() {
        let db = make_db();
        let src = r#"package P
public
  system A
    features
      out1 : out data port;
  end A;
  system implementation A.i
  end A.i;

  system B
    features
      in1 : in data port;
  end B;
  system implementation B.i
  end B.i;

  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      sa : system A.i;
      sb : system B.i;
    connections
      c1 : port sa.out1 -> sb.in1;
  end Top.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "sem_conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // Should have exactly 1 semantic connection
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection, got {}; diagnostics: {:?}",
            inst.semantic_connection_count(),
            inst.diagnostics
        );

        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.name.as_str(), "c1");
        assert_eq!(sc.kind, item_tree::ConnectionKind::Port);

        // Verify ultimate source is sa + out1
        let (src_idx, ref src_feat) = sc.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "sa");
        assert_eq!(src_feat.as_str(), "out1");

        // Verify ultimate destination is sb + in1
        let (dst_idx, ref dst_feat) = sc.ultimate_destination;
        assert_eq!(inst.component(dst_idx).name.as_str(), "sb");
        assert_eq!(dst_feat.as_str(), "in1");

        // Connection path has 1 entry
        assert_eq!(sc.connection_path.len(), 1);
    }

    #[test]
    fn semantic_connections_multiple() {
        let db = make_db();
        let src = r#"package FlightSystem
public
  system Sensor
    features
      reading : out data port;
  end Sensor;
  system implementation Sensor.basic
  end Sensor.basic;

  system Actuator
    features
      command : in data port;
  end Actuator;
  system implementation Actuator.basic
  end Actuator.basic;

  system Controller
    features
      sensor_in : in data port;
      cmd_out : out data port;
  end Controller;
  system implementation Controller.basic
  end Controller.basic;

  system FlightControl
  end FlightControl;
  system implementation FlightControl.full
    subcomponents
      sensor : system Sensor.basic;
      ctrl : system Controller.basic;
      actuator : system Actuator.basic;
    connections
      c1 : port sensor.reading -> ctrl.sensor_in;
      c2 : port ctrl.cmd_out -> actuator.command;
  end FlightControl.full;
end FlightSystem;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "multi_conn.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("FlightSystem"),
            &Name::new("FlightControl"),
            &Name::new("full"),
        );

        // 2 across connections => 2 semantic connections
        assert_eq!(
            inst.semantic_connection_count(),
            2,
            "expected 2 semantic connections, got {}; diagnostics: {:?}",
            inst.semantic_connection_count(),
            inst.diagnostics
        );

        // c1: sensor.reading -> ctrl.sensor_in
        let sc1 = &inst.semantic_connections[0];
        assert_eq!(sc1.name.as_str(), "c1");
        let (src1_idx, ref src1_feat) = sc1.ultimate_source;
        assert_eq!(inst.component(src1_idx).name.as_str(), "sensor");
        assert_eq!(src1_feat.as_str(), "reading");
        let (dst1_idx, ref dst1_feat) = sc1.ultimate_destination;
        assert_eq!(inst.component(dst1_idx).name.as_str(), "ctrl");
        assert_eq!(dst1_feat.as_str(), "sensor_in");

        // c2: ctrl.cmd_out -> actuator.command
        let sc2 = &inst.semantic_connections[1];
        assert_eq!(sc2.name.as_str(), "c2");
        let (src2_idx, ref src2_feat) = sc2.ultimate_source;
        assert_eq!(inst.component(src2_idx).name.as_str(), "ctrl");
        assert_eq!(src2_feat.as_str(), "cmd_out");
        let (dst2_idx, ref dst2_feat) = sc2.ultimate_destination;
        assert_eq!(inst.component(dst2_idx).name.as_str(), "actuator");
        assert_eq!(dst2_feat.as_str(), "command");
    }

    #[test]
    fn semantic_connections_skips_incomplete() {
        let db = make_db();
        // Model with a down-connection (ext_in -> a.in1) and one across (a.out1 -> b.in1).
        // Only the across connection should produce a semantic connection.
        let src = r#"package P
public
  system S
    features
      ext_in : in data port;
  end S;
  system implementation S.impl
    subcomponents
      a : system;
      b : system;
    connections
      c1 : port a.out1 -> b.in1;
      c2 : port ext_in -> a.in1;
  end S.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "skip_conn.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("impl"),
        );

        // c2 is a down-connection (no subcomponent on src side), so only c1 is semantic
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection (across only), got {}",
            inst.semantic_connection_count()
        );
        assert_eq!(inst.semantic_connections[0].name.as_str(), "c1");
    }

    #[test]
    fn semantic_connections_in_summary() {
        let db = make_db();
        let src = r#"package P
public
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      a : system;
      b : system;
    connections
      c1 : port a.x -> b.y;
  end Top.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(
            &db,
            "summary_conn.aadl".to_string(),
            src.to_string(),
        );
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let summary = inst.summary();
        assert!(
            summary.contains("Semantic connections: 1"),
            "summary should contain semantic connection count: {}",
            summary
        );
    }
}
