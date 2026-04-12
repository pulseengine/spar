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
pub mod property_check;
pub mod property_eval;
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
        let file = spar_base_db::SourceFile::new(&db, "consumer.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);

        let pkg = &tree.packages[tree.packages.iter().next().unwrap().0];
        assert_eq!(pkg.name.as_str(), "Consumer");
        assert!(
            pkg.with_clauses.iter().any(|n| n.as_str() == "DataTypes"),
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
        let file = spar_base_db::SourceFile::new(&db, "conn.aadl".to_string(), src.to_string());
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
        let file = spar_base_db::SourceFile::new(&db, "flow.aadl".to_string(), src.to_string());
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
        let f1 =
            spar_base_db::SourceFile::new(&db, "datatypes.aadl".to_string(), types_src.to_string());
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
        let reference = ClassifierRef::qualified(Name::new("DataTypes"), Name::new("Temperature"));
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
        let file = spar_base_db::SourceFile::new(&db, "flight.aadl".to_string(), src.to_string());
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
        let file = spar_base_db::SourceFile::new(&db, "nested.aadl".to_string(), src.to_string());
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
        let f1 =
            spar_base_db::SourceFile::new(&db, "sensor.aadl".to_string(), types_src.to_string());
        let f2 =
            spar_base_db::SourceFile::new(&db, "vehicle.aadl".to_string(), main_src.to_string());
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
        assert_eq!(
            pa0.name.property_set.as_ref().unwrap().as_str(),
            "Deployment"
        );
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
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

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
        let f1 =
            spar_base_db::SourceFile::new(&db, "timing.aadl".to_string(), props_src.to_string());
        let f2 =
            spar_base_db::SourceFile::new(&db, "vehicle.aadl".to_string(), main_src.to_string());
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
        use name::PropertyRef;
        use properties::{PropertyMap, PropertyValue};

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
            typed_value: None,
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
            typed_value: None,
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
            typed_value: None,
            is_append: true,
        });

        let all = map.get_all("Timing", "Period");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], "200 ms");
        assert_eq!(all[1], "300 ms");
    }

    #[test]
    fn typed_value_survives_instantiation() {
        // A typed property on a component type should be visible via get_typed()
        // after instantiation.
        let db = make_db();
        let src = r#"package P
public
  system Sensor
    properties
      Period => 10 ms;
  end Sensor;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      s : system Sensor;
  end Top.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "typed_inst.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let s_idx = root.children[0];
        let props = inst.properties_for(s_idx);

        // The lowering should produce a typed value for "10 ms"
        assert_eq!(props.get("", "Period"), Some("10 ms"));

        // Verify typed_value is propagated (the lowerer should produce an
        // Integer(10, Some("ms")) expression for "10 ms").
        let typed = props.get_typed("", "Period");
        assert!(
            typed.is_some(),
            "typed_value should survive instantiation; props: {:?}",
            props
        );
    }

    #[test]
    fn typed_value_survives_extends_chain() {
        // Parent type has a typed property. The child extends the parent.
        // After instantiation, the inherited property should have its typed_value.
        let db = make_db();
        let src = r#"package P
public
  system Base
    properties
      Period => 10 ms;
  end Base;

  system Child extends Base
    properties
      Priority => 7;
  end Child;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      c : system Child;
  end Top.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "typed_extends.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let c_idx = root.children[0];
        let props = inst.properties_for(c_idx);

        // Period inherited from Base should still have typed_value
        assert_eq!(props.get("", "Period"), Some("10 ms"));
        let typed_period = props.get_typed("", "Period");
        assert!(
            typed_period.is_some(),
            "typed_value for Period should survive extends chain; props: {:?}",
            props
        );

        // Priority on Child should also have typed_value
        assert_eq!(props.get("", "Priority"), Some("7"));
        let typed_priority = props.get_typed("", "Priority");
        assert!(
            typed_priority.is_some(),
            "typed_value for Priority should be present; props: {:?}",
            props
        );
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
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let s1_idx = root.children[0];
        let props = inst.properties_for(s1_idx);

        // Period: type says 10ms, impl says 20ms -> impl wins
        assert_eq!(props.get("", "Period"), Some("20 ms"), "props: {:?}", props);

        // Deadline: type says 5ms, subcomponent says 15ms -> subcomponent wins
        assert_eq!(
            props.get("", "Deadline"),
            Some("15 ms"),
            "props: {:?}",
            props
        );

        // Priority: only on impl -> inherited
        assert_eq!(props.get("", "Priority"), Some("3"), "props: {:?}", props);
    }

    // ── Property inheritance through extends chain ─────────────────────

    #[test]
    fn property_type_extends_chain_inheritance() {
        // Parent type has Property A, child type extends parent and adds Property B.
        // Child instance should inherit both A and B.
        let db = make_db();
        let src = r#"package P
public
  system Base
    properties
      Period => 10 ms;
      Deadline => 5 ms;
  end Base;

  system Child extends Base
    properties
      Priority => 7;
      Deadline => 15 ms;
  end Child;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      c : system Child;
  end Top.impl;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "extends.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let child_idx = root.children[0];
        let props = inst.properties_for(child_idx);

        // Period: inherited from Base (parent type)
        assert_eq!(
            props.get("", "Period"),
            Some("10 ms"),
            "Period should be inherited from Base; props: {:?}",
            props
        );

        // Deadline: Base says 5ms, Child says 15ms -> Child wins
        assert_eq!(
            props.get("", "Deadline"),
            Some("15 ms"),
            "Deadline should be overridden by Child; props: {:?}",
            props
        );

        // Priority: only on Child -> inherited
        assert_eq!(
            props.get("", "Priority"),
            Some("7"),
            "Priority should come from Child; props: {:?}",
            props
        );
    }

    #[test]
    fn property_impl_extends_chain_inheritance() {
        // Parent impl has a property, child impl extends it and adds another.
        let db = make_db();
        let src = r#"package P
public
  system S
  end S;

  system implementation S.base
    properties
      Period => 10 ms;
      Deadline => 5 ms;
  end S.base;

  system implementation S.child extends S.base
    properties
      Priority => 3;
      Deadline => 20 ms;
  end S.child;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      s1 : system S.child;
  end Top.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "impl_extends.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let s1_idx = root.children[0];
        let props = inst.properties_for(s1_idx);

        // Period: inherited from S.base (parent impl)
        assert_eq!(
            props.get("", "Period"),
            Some("10 ms"),
            "Period should be inherited from S.base; props: {:?}",
            props
        );

        // Deadline: S.base says 5ms, S.child says 20ms -> S.child wins
        assert_eq!(
            props.get("", "Deadline"),
            Some("20 ms"),
            "Deadline should be overridden by S.child; props: {:?}",
            props
        );

        // Priority: only on S.child -> inherited
        assert_eq!(
            props.get("", "Priority"),
            Some("3"),
            "Priority should come from S.child; props: {:?}",
            props
        );
    }

    #[test]
    fn property_grandparent_type_chain() {
        // Three-level chain: Grandparent -> Parent -> Child, each with properties.
        let db = make_db();
        let src = r#"package P
public
  system Grandparent
    properties
      Period => 100 ms;
      Deadline => 50 ms;
      Priority => 1;
  end Grandparent;

  system Parent extends Grandparent
    properties
      Deadline => 25 ms;
  end Parent;

  system Child extends Parent
    properties
      Priority => 9;
  end Child;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      c : system Child;
  end Top.impl;
end P;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "grandparent.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let root = inst.component(inst.root);
        assert_eq!(
            root.children.len(),
            1,
            "diagnostics: {:?}",
            inst.diagnostics
        );

        let child_idx = root.children[0];
        let props = inst.properties_for(child_idx);

        // Period: from Grandparent, never overridden
        assert_eq!(
            props.get("", "Period"),
            Some("100 ms"),
            "Period should be inherited from Grandparent; props: {:?}",
            props
        );

        // Deadline: Grandparent 50ms, Parent 25ms -> Parent wins
        assert_eq!(
            props.get("", "Deadline"),
            Some("25 ms"),
            "Deadline should be overridden by Parent; props: {:?}",
            props
        );

        // Priority: Grandparent 1, Child 9 -> Child wins (skipping Parent)
        assert_eq!(
            props.get("", "Priority"),
            Some("9"),
            "Priority should be overridden by Child; props: {:?}",
            props
        );
    }

    // ── Flow instance tests ────────────────────────────────────��──────

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
        let file =
            spar_base_db::SourceFile::new(&db, "flow_inst.aadl".to_string(), src.to_string());
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
        assert!(
            summary.contains("End-to-end flows: 1"),
            "summary: {}",
            summary
        );
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
        let file =
            spar_base_db::SourceFile::new(&db, "flight_system.aadl".to_string(), src.to_string());
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
        assert!(
            summary.contains("End-to-end flows: 1"),
            "summary:\n{}",
            summary
        );
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
        let file = spar_base_db::SourceFile::new(&db, "modes.aadl".to_string(), src.to_string());
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
        assert!(
            summary.contains("Mode transitions: 0"),
            "summary:\n{}",
            summary
        );
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
        let file =
            spar_base_db::SourceFile::new(&db, "transitions.aadl".to_string(), src.to_string());
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
        assert_eq!(
            root.mode_transitions.len(),
            2,
            "expected 2 mode transitions"
        );

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
        assert!(
            summary.contains("Mode transitions: 2"),
            "summary:\n{}",
            summary
        );
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
        let file =
            spar_base_db::SourceFile::new(&db, "impl_modes.aadl".to_string(), src.to_string());
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
        let file =
            spar_base_db::SourceFile::new(&db, "sub_modes.aadl".to_string(), src.to_string());
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
        let file = spar_base_db::SourceFile::new(&db, "sem_conn.aadl".to_string(), src.to_string());
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
        let file =
            spar_base_db::SourceFile::new(&db, "multi_conn.aadl".to_string(), src.to_string());
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
    fn semantic_connections_root_down_and_across() {
        let db = make_db();
        // Model with a down-connection (ext_in -> a.in1) and one across (a.out1 -> b.in1).
        // At the root level, both produce semantic connections:
        // - c1 is across (a.out1 -> b.in1)
        // - c2 is a down connection at root (ext_in -> a.in1)
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
        let file =
            spar_base_db::SourceFile::new(&db, "skip_conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("impl"),
        );

        // Both c1 (across) and c2 (down at root) produce semantic connections
        assert_eq!(
            inst.semantic_connection_count(),
            2,
            "expected 2 semantic connections, got {}",
            inst.semantic_connection_count()
        );
        // c1: across connection
        assert_eq!(inst.semantic_connections[0].name.as_str(), "c1");
        // c2: down connection at root
        assert_eq!(inst.semantic_connections[1].name.as_str(), "c2");

        // Verify c2 endpoints: source is the root component's ext_in,
        // destination is subcomponent 'a'
        let sc2 = &inst.semantic_connections[1];
        let (src_idx, ref src_feat) = sc2.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "S.impl");
        assert_eq!(src_feat.as_str(), "ext_in");
        let (dst_idx, ref dst_feat) = sc2.ultimate_destination;
        assert_eq!(inst.component(dst_idx).name.as_str(), "a");
        assert_eq!(dst_feat.as_str(), "in1");
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
        let file =
            spar_base_db::SourceFile::new(&db, "summary_conn.aadl".to_string(), src.to_string());
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

    #[test]
    fn semantic_connections_simple_up() {
        let db = make_db();
        // Inner has an up connection: inner_sub.sensor_out -> reading
        // Since Inner.i is instantiated as a child of Top.impl, the up connection
        // is NOT at the root level — it's consumed when Top traces across connections.
        // But here Inner.i IS the root, so the up connection produces a standalone
        // semantic connection.
        let src = r#"package UpPkg
public
  system Probe
    features
      sensor_out : out data port;
  end Probe;
  system implementation Probe.i
  end Probe.i;

  system Inner
    features
      reading : out data port;
  end Inner;
  system implementation Inner.i
    subcomponents
      probe : system Probe.i;
    connections
      c_up : port probe.sensor_out -> reading;
  end Inner.i;
end UpPkg;
"#;
        let file = spar_base_db::SourceFile::new(&db, "up_conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("UpPkg"),
            &Name::new("Inner"),
            &Name::new("i"),
        );

        // The up connection at root produces 1 semantic connection
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection for root up connection, got {}",
            inst.semantic_connection_count()
        );
        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.name.as_str(), "c_up");

        // Source is the probe subcomponent's sensor_out
        let (src_idx, ref src_feat) = sc.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "probe");
        assert_eq!(src_feat.as_str(), "sensor_out");

        // Destination is the root component's reading port
        let (dst_idx, ref dst_feat) = sc.ultimate_destination;
        assert_eq!(inst.component(dst_idx).name.as_str(), "Inner.i");
        assert_eq!(dst_feat.as_str(), "reading");

        // Connection path has 1 entry
        assert_eq!(sc.connection_path.len(), 1);
    }

    #[test]
    fn semantic_connections_simple_down() {
        let db = make_db();
        // Root has a down connection: cmd_in -> actuator.command
        let src = r#"package DownPkg
public
  system Actuator
    features
      command : in data port;
  end Actuator;
  system implementation Actuator.i
  end Actuator.i;

  system Controller
    features
      cmd_in : in data port;
  end Controller;
  system implementation Controller.i
    subcomponents
      actuator : system Actuator.i;
    connections
      c_down : port cmd_in -> actuator.command;
  end Controller.i;
end DownPkg;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "down_conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("DownPkg"),
            &Name::new("Controller"),
            &Name::new("i"),
        );

        // The down connection at root produces 1 semantic connection
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection for root down connection, got {}",
            inst.semantic_connection_count()
        );
        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.name.as_str(), "c_down");

        // Source is the root component's cmd_in port
        let (src_idx, ref src_feat) = sc.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "Controller.i");
        assert_eq!(src_feat.as_str(), "cmd_in");

        // Destination is the actuator subcomponent's command port
        let (dst_idx, ref dst_feat) = sc.ultimate_destination;
        assert_eq!(inst.component(dst_idx).name.as_str(), "actuator");
        assert_eq!(dst_feat.as_str(), "command");

        // Connection path has 1 entry
        assert_eq!(sc.connection_path.len(), 1);
    }

    #[test]
    fn semantic_connections_multi_level_tracing() {
        let db = make_db();
        // Three-level hierarchy:
        //   Top.impl
        //     inner_a : Inner.i
        //       probe : Probe.i          (leaf, has sensor_out)
        //       c_up : probe.sensor_out -> reading   (up connection)
        //     inner_b : Inner2.i
        //       handler : Handler.i       (leaf, has data_in)
        //       c_down : data_in -> handler.data_in  (down connection)
        //     c_across : inner_a.reading -> inner_b.data_in  (across connection)
        //
        // The semantic connection should trace:
        //   probe.sensor_out -> (up) -> inner_a.reading -> (across) -> inner_b.data_in -> (down) -> handler.data_in
        // Ultimate source: probe.sensor_out
        // Ultimate destination: handler.data_in
        let src = r#"package MultiPkg
public
  system Probe
    features
      sensor_out : out data port;
  end Probe;
  system implementation Probe.i
  end Probe.i;

  system Handler
    features
      data_in : in data port;
  end Handler;
  system implementation Handler.i
  end Handler.i;

  system Inner
    features
      reading : out data port;
  end Inner;
  system implementation Inner.i
    subcomponents
      probe : system Probe.i;
    connections
      c_up : port probe.sensor_out -> reading;
  end Inner.i;

  system Inner2
    features
      data_in : in data port;
  end Inner2;
  system implementation Inner2.i
    subcomponents
      handler : system Handler.i;
    connections
      c_down : port data_in -> handler.data_in;
  end Inner2.i;

  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      inner_a : system Inner.i;
      inner_b : system Inner2.i;
    connections
      c_across : port inner_a.reading -> inner_b.data_in;
  end Top.impl;
end MultiPkg;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "multi_conn.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("MultiPkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // Should have exactly 1 semantic connection (the across at root, traced
        // through up and down connections in children).
        // The up/down connections inside inner_a and inner_b are not at the root
        // level, so they don't produce standalone semantic connections.
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection (multi-level traced), got {}; diagnostics: {:?}",
            inst.semantic_connection_count(),
            inst.diagnostics
        );

        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.name.as_str(), "c_across");

        // Ultimate source: probe.sensor_out (deepest source via up connection)
        let (src_idx, ref src_feat) = sc.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "probe");
        assert_eq!(src_feat.as_str(), "sensor_out");

        // Ultimate destination: handler.data_in (deepest destination via down connection)
        let (dst_idx, ref dst_feat) = sc.ultimate_destination;
        assert_eq!(inst.component(dst_idx).name.as_str(), "handler");
        assert_eq!(dst_feat.as_str(), "data_in");

        // Connection path: c_across + c_up + c_down = 3 connections
        assert_eq!(
            sc.connection_path.len(),
            3,
            "expected 3 connections in path (across + up + down), got {}",
            sc.connection_path.len()
        );
    }

    #[test]
    fn semantic_connections_up_not_at_root_skipped() {
        let db = make_db();
        // Up connection inside a non-root component should NOT produce a standalone
        // semantic connection (it's consumed by the parent's across tracing).
        let src = r#"package UpSkipPkg
public
  system Sensor
    features
      raw : out data port;
  end Sensor;
  system implementation Sensor.i
  end Sensor.i;

  system Wrapper
    features
      output : out data port;
  end Wrapper;
  system implementation Wrapper.i
    subcomponents
      s : system Sensor.i;
    connections
      c_up : port s.raw -> output;
  end Wrapper.i;

  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      w : system Wrapper.i;
      other : system;
    connections
      c_across : port w.output -> other.in1;
  end Top.impl;
end UpSkipPkg;
"#;
        let file = spar_base_db::SourceFile::new(&db, "up_skip.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("UpSkipPkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // Only 1 semantic connection: the across connection c_across, which
        // traces through the up connection inside Wrapper to find the ultimate
        // source (s.raw). The up connection c_up inside Wrapper does not produce
        // its own standalone semantic connection.
        assert_eq!(
            inst.semantic_connection_count(),
            1,
            "expected 1 semantic connection, got {}",
            inst.semantic_connection_count()
        );

        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.name.as_str(), "c_across");

        // Ultimate source should be s.raw (traced through the up connection)
        let (src_idx, ref src_feat) = sc.ultimate_source;
        assert_eq!(inst.component(src_idx).name.as_str(), "s");
        assert_eq!(src_feat.as_str(), "raw");

        // Connection path should be 2: c_across + c_up
        assert_eq!(
            sc.connection_path.len(),
            2,
            "expected 2 connections in path, got {}",
            sc.connection_path.len()
        );
    }

    // ── System Operation Mode (SOM) tests ────────────────────────────

    #[test]
    fn som_no_modes_yields_zero_soms() {
        let db = make_db();
        let src = r#"package NoModePkg
public
  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      sub1 : system Top;
  end Top.impl;
end NoModePkg;
"#;
        let file = spar_base_db::SourceFile::new(&db, "no_mode.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("NoModePkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        assert_eq!(
            inst.som_count(),
            0,
            "system with no modal components should have 0 SOMs"
        );
        let summary = inst.summary();
        assert!(
            summary.contains("System operation modes: 0"),
            "summary should show 0 SOMs: {}",
            summary
        );
    }

    #[test]
    fn som_single_modal_component_three_modes() {
        let db = make_db();
        let src = r#"package SingleModalPkg
public
  system Controller
    modes
      standby : initial mode;
      active : mode;
      shutdown : mode;
  end Controller;

  system implementation Controller.impl
  end Controller.impl;
end SingleModalPkg;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "single_modal.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("SingleModalPkg"),
            &Name::new("Controller"),
            &Name::new("impl"),
        );

        assert_eq!(
            inst.som_count(),
            3,
            "single component with 3 modes should produce 3 SOMs; diagnostics: {:?}",
            inst.diagnostics
        );

        // Each SOM should have exactly one mode selection and its name should match the mode.
        let names: Vec<&str> = inst
            .system_operation_modes
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["standby", "active", "shutdown"]);

        for som in &inst.system_operation_modes {
            assert_eq!(som.mode_selections.len(), 1);
        }
    }

    #[test]
    fn som_two_modal_subcomponents_cartesian_product() {
        let db = make_db();
        let src = r#"package TwoModalPkg
public
  system SensorA
    modes
      active : initial mode;
      standby : mode;
  end SensorA;

  system implementation SensorA.impl
  end SensorA.impl;

  system SensorB
    modes
      fast : initial mode;
      slow : mode;
  end SensorB;

  system implementation SensorB.impl
  end SensorB.impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      sub_a : system SensorA.impl;
      sub_b : system SensorB.impl;
  end Top.impl;
end TwoModalPkg;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "two_modal.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("TwoModalPkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        assert_eq!(
            inst.som_count(),
            4,
            "2x2 modes should produce 4 SOMs; diagnostics: {:?}",
            inst.diagnostics
        );

        // Verify names: cartesian product should give 4 combinations.
        let mut names: Vec<String> = inst
            .system_operation_modes
            .iter()
            .map(|s| s.name.clone())
            .collect();
        names.sort();
        let mut expected = vec![
            "active_fast".to_string(),
            "active_slow".to_string(),
            "standby_fast".to_string(),
            "standby_slow".to_string(),
        ];
        expected.sort();
        assert_eq!(names, expected);

        // Each SOM should have exactly 2 mode selections (one per modal component).
        for som in &inst.system_operation_modes {
            assert_eq!(
                som.mode_selections.len(),
                2,
                "each SOM should select a mode for each of the 2 modal components"
            );
        }

        let summary = inst.summary();
        assert!(
            summary.contains("System operation modes: 4"),
            "summary should show 4 SOMs: {}",
            summary
        );
    }

    #[test]
    fn som_names_concatenated_with_underscores() {
        let db = make_db();
        let src = r#"package SomNamePkg
public
  system M1
    modes
      a : initial mode;
      b : mode;
  end M1;

  system implementation M1.impl
  end M1.impl;

  system M2
    modes
      x : initial mode;
      y : mode;
      z : mode;
  end M2;

  system implementation M2.impl
  end M2.impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      m1 : system M1.impl;
      m2 : system M2.impl;
  end Top.impl;
end SomNamePkg;
"#;
        let file =
            spar_base_db::SourceFile::new(&db, "som_names.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);

        let inst = instance::SystemInstance::instantiate(
            &scope,
            &Name::new("SomNamePkg"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // 2 * 3 = 6 SOMs
        assert_eq!(inst.som_count(), 6);

        // Verify every name is "modeA_modeB" format with underscore separator.
        for som in &inst.system_operation_modes {
            let parts: Vec<&str> = som.name.split('_').collect();
            assert_eq!(
                parts.len(),
                2,
                "SOM name '{}' should have exactly 2 underscore-separated parts",
                som.name
            );
        }
    }

    // ── PropertyExpr lowering tests ─────────────────────────────────

    /// Helper: parse AADL source and return the typed_value of the first property
    /// association found in the first component type.
    fn lower_first_typed_value(src: &str) -> Option<item_tree::PropertyExpr> {
        let db = make_db();
        let file = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        // Get the first property association from the first component type
        let ct = &tree.component_types[tree.component_types.iter().next()?.0];
        let pa_idx = ct.property_associations.first()?;
        let pa = &tree.property_associations[*pa_idx];
        pa.typed_value.clone()
    }

    #[test]
    fn lower_integer_value() {
        let src = r#"package P
public
  system S
    properties
      MyProp => 42;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Integer(42, None)),
            "expected Integer(42, None), got {:?}",
            val
        );
    }

    #[test]
    fn lower_integer_with_unit() {
        let src = r#"package P
public
  system S
    properties
      Period => 10 ms;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Integer(10, Some(Name::new("ms")))),
            "expected Integer(10, Some(ms)), got {:?}",
            val
        );
    }

    #[test]
    fn lower_real_value() {
        let src = r#"package P
public
  system S
    properties
      Weight => 3.14;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Real("3.14".to_string(), None)),
            "expected Real(3.14, None), got {:?}",
            val
        );
    }

    #[test]
    fn lower_real_with_unit() {
        let src = r#"package P
public
  system S
    properties
      Latency => 1.5 ms;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Real(
                "1.5".to_string(),
                Some(Name::new("ms"))
            )),
            "expected Real(1.5, Some(ms)), got {:?}",
            val
        );
    }

    #[test]
    fn lower_string_value() {
        let src = r#"package P
public
  system S
    properties
      Source_Name => "main.c";
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::StringLit("main.c".to_string())),
            "expected StringLit(main.c), got {:?}",
            val
        );
    }

    #[test]
    fn lower_boolean_true() {
        let src = r#"package P
public
  system S
    properties
      Active => true;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Boolean(true)),
            "expected Boolean(true), got {:?}",
            val
        );
    }

    #[test]
    fn lower_boolean_false() {
        let src = r#"package P
public
  system S
    properties
      Active => false;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Boolean(false)),
            "expected Boolean(false), got {:?}",
            val
        );
    }

    #[test]
    fn lower_enum_value() {
        let src = r#"package P
public
  system S
    properties
      Dispatch_Protocol => Periodic;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        assert_eq!(
            val,
            Some(item_tree::PropertyExpr::Enum(Name::new("Periodic"))),
            "expected Enum(Periodic), got {:?}",
            val
        );
    }

    #[test]
    fn lower_list_value() {
        let src = r#"package P
public
  system S
    properties
      Allowed_Periods => (10, 20, 30);
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::List(items)) => {
                assert_eq!(items.len(), 3, "expected 3 list items, got {}", items.len());
                assert_eq!(items[0], item_tree::PropertyExpr::Integer(10, None));
                assert_eq!(items[1], item_tree::PropertyExpr::Integer(20, None));
                assert_eq!(items[2], item_tree::PropertyExpr::Integer(30, None));
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn lower_record_value() {
        let src = r#"package P
public
  system S
    properties
      MyRecord => [x => 10; y => 20;];
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::Record(fields)) => {
                assert_eq!(
                    fields.len(),
                    2,
                    "expected 2 record fields, got {}",
                    fields.len()
                );
                assert_eq!(fields[0].0.as_str(), "x");
                assert_eq!(fields[0].1, item_tree::PropertyExpr::Integer(10, None));
                assert_eq!(fields[1].0.as_str(), "y");
                assert_eq!(fields[1].1, item_tree::PropertyExpr::Integer(20, None));
            }
            other => panic!("expected Record, got {:?}", other),
        }
    }

    #[test]
    fn lower_range_value() {
        let src = r#"package P
public
  system S
    properties
      Size_Range => 1 .. 100;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::Range { min, max, delta }) => {
                assert_eq!(*min, item_tree::PropertyExpr::Integer(1, None));
                assert_eq!(*max, item_tree::PropertyExpr::Integer(100, None));
                assert!(delta.is_none(), "expected no delta");
            }
            other => panic!("expected Range, got {:?}", other),
        }
    }

    #[test]
    fn lower_classifier_value() {
        let src = r#"package P
public
  system S
    properties
      Classifier_Ref => classifier (P::MyType);
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::ClassifierValue(cr)) => {
                assert_eq!(cr.type_name.as_str(), "MyType");
            }
            other => panic!("expected ClassifierValue, got {:?}", other),
        }
    }

    #[test]
    fn lower_reference_value() {
        let src = r#"package P
public
  system S
    properties
      Actual_Processor_Binding => reference (cpu1);
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::ReferenceValue(path)) => {
                assert_eq!(path, "cpu1");
            }
            other => panic!("expected ReferenceValue, got {:?}", other),
        }
    }

    #[test]
    fn lower_compute_value() {
        let src = r#"package P
public
  system S
    properties
      Compute_Deadline => compute (calc_deadline);
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::ComputedValue(name)) => {
                assert_eq!(name.as_str(), "calc_deadline");
            }
            other => panic!("expected ComputedValue, got {:?}", other),
        }
    }

    #[test]
    fn lower_list_of_strings() {
        let src = r#"package P
public
  system S
    properties
      Source_Text => ("file1.c", "file2.c");
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::List(items)) => {
                assert_eq!(items.len(), 2);
                assert_eq!(
                    items[0],
                    item_tree::PropertyExpr::StringLit("file1.c".to_string())
                );
                assert_eq!(
                    items[1],
                    item_tree::PropertyExpr::StringLit("file2.c".to_string())
                );
            }
            other => panic!("expected List of strings, got {:?}", other),
        }
    }

    #[test]
    fn lower_empty_list() {
        let src = r#"package P
public
  system S
    properties
      EmptyList => ();
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::List(items)) => {
                assert!(items.is_empty(), "expected empty list, got {:?}", items);
            }
            other => panic!("expected empty List, got {:?}", other),
        }
    }

    #[test]
    fn lower_preserves_raw_value_text() {
        // Verify that the raw value string is still populated alongside typed_value
        let db = make_db();
        let src = r#"package P
public
  system S
    properties
      Period => 10 ms;
  end S;
end P;
"#;
        let file = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let ct = &tree.component_types[tree.component_types.iter().next().unwrap().0];
        let pa_idx = ct.property_associations.first().unwrap();
        let pa = &tree.property_associations[*pa_idx];

        // Raw value should still be set
        assert!(!pa.value.is_empty(), "raw value string should not be empty");
        // Typed value should also be set
        assert!(pa.typed_value.is_some(), "typed_value should be Some");
    }

    #[test]
    fn lower_qualified_enum_value() {
        let src = r#"package P
public
  system S
    properties
      Protocol => MyProps::Custom;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::Enum(name)) => {
                assert_eq!(name.as_str(), "MyProps::Custom");
            }
            other => panic!("expected Enum(MyProps::Custom), got {:?}", other),
        }
    }

    // ── Property type declaration lowering tests ───────────────

    /// Helper to get a property set from parsed source.
    fn lower_first_property_set(src: &str) -> std::sync::Arc<item_tree::ItemTree> {
        let db = make_db();
        let file = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), src.to_string());
        file_item_tree(&db, file)
    }

    #[test]
    fn property_type_def_aadlinteger() {
        let src = r#"property set MyProps is
  MyInt : aadlinteger applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        assert_eq!(tree.property_sets.len(), 1);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.property_defs.len(), 1);
        let def = &ps.property_defs[0];
        assert_eq!(def.name.as_str(), "MyInt");
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::AadlInteger { range, units }) => {
                assert!(range.is_none());
                assert!(units.is_none());
            }
            other => panic!("expected AadlInteger, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_aadlinteger_with_units() {
        let src = r#"property set MyProps is
  MyInt : aadlinteger units Time_Units applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::AadlInteger { range, units }) => {
                assert!(range.is_none());
                assert_eq!(units.as_ref().unwrap().as_str(), "Time_Units");
            }
            other => panic!("expected AadlInteger with units, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_aadlboolean() {
        let src = r#"property set MyProps is
  MyBool : aadlboolean applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::AadlBoolean) => {}
            other => panic!("expected AadlBoolean, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_aadlstring() {
        let src = r#"property set MyProps is
  MyStr : aadlstring applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::AadlString) => {}
            other => panic!("expected AadlString, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_aadlreal() {
        let src = r#"property set MyProps is
  MyReal : aadlreal applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::AadlReal { range, units }) => {
                assert!(range.is_none());
                assert!(units.is_none());
            }
            other => panic!("expected AadlReal, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_enumeration() {
        let src = r#"property set MyProps is
  MyEnum : enumeration (Periodic, Sporadic, Aperiodic) applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::Enumeration(variants)) => {
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0].as_str(), "Periodic");
                assert_eq!(variants[1].as_str(), "Sporadic");
                assert_eq!(variants[2].as_str(), "Aperiodic");
            }
            other => panic!("expected Enumeration, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_list_of_aadlstring() {
        let src = r#"property set MyProps is
  MyList : list of aadlstring applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::ListOf(inner)) => {
                assert!(matches!(
                    inner.as_ref(),
                    item_tree::PropertyTypeDef::AadlString
                ));
            }
            other => panic!("expected ListOf(AadlString), got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_range_of_aadlinteger() {
        let src = r#"property set MyProps is
  MyRange : range of aadlinteger applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::Range(inner)) => {
                assert!(matches!(
                    inner.as_ref(),
                    item_tree::PropertyTypeDef::AadlInteger { .. }
                ));
            }
            other => panic!("expected Range(AadlInteger), got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_classifier() {
        let src = r#"property set MyProps is
  MyClass : classifier applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::Classifier(_)) => {}
            other => panic!("expected Classifier, got {:?}", other),
        }
    }

    #[test]
    fn property_type_def_reference() {
        let src = r#"property set MyProps is
  MyRef : reference applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::Reference(_)) => {}
            other => panic!("expected Reference, got {:?}", other),
        }
    }

    // ── Units declaration lowering tests ───────────────────────

    #[test]
    fn property_type_decl_units() {
        let src = r#"property set MyProps is
  Size_Units : type units (bits, Bytes => bits * 8, KByte => Bytes * 1024);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.property_type_defs.len(), 1);
        let td = &ps.property_type_defs[0];
        assert_eq!(td.name.as_str(), "Size_Units");
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::UnitsType(units)) => {
                assert_eq!(units.len(), 3);
                // Base unit: bits (no conversion)
                assert_eq!(units[0].0.as_str(), "bits");
                assert!(units[0].1.is_none());
                // Bytes => bits * 8
                assert_eq!(units[1].0.as_str(), "Bytes");
                let (base, factor) = units[1].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "bits");
                assert_eq!(*factor, 8);
                // KByte => Bytes * 1024
                assert_eq!(units[2].0.as_str(), "KByte");
                let (base2, factor2) = units[2].1.as_ref().unwrap();
                assert_eq!(base2.as_str(), "Bytes");
                assert_eq!(*factor2, 1024);
            }
            other => panic!("expected UnitsType, got {:?}", other),
        }
    }

    #[test]
    fn property_type_decl_units_time_chain() {
        // Verify full AADL time units chain: each derived unit references
        // its predecessor, with the correct factor.
        let src = r#"property set TimeProps is
  Time_Units : type units (ps, ns => ps * 1000, us => ns * 1000, ms => us * 1000, sec => ms * 1000, min => sec * 60, hr => min * 60);
end TimeProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.property_type_defs.len(), 1);
        let td = &ps.property_type_defs[0];
        assert_eq!(td.name.as_str(), "Time_Units");
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::UnitsType(units)) => {
                assert_eq!(units.len(), 7);
                // ps — base unit
                assert_eq!(units[0].0.as_str(), "ps");
                assert!(units[0].1.is_none());
                // ns => ps * 1000
                assert_eq!(units[1].0.as_str(), "ns");
                let (base, factor) = units[1].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "ps");
                assert_eq!(*factor, 1000);
                // us => ns * 1000
                assert_eq!(units[2].0.as_str(), "us");
                let (base, factor) = units[2].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "ns");
                assert_eq!(*factor, 1000);
                // ms => us * 1000
                assert_eq!(units[3].0.as_str(), "ms");
                let (base, factor) = units[3].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "us");
                assert_eq!(*factor, 1000);
                // sec => ms * 1000
                assert_eq!(units[4].0.as_str(), "sec");
                let (base, factor) = units[4].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "ms");
                assert_eq!(*factor, 1000);
                // min => sec * 60
                assert_eq!(units[5].0.as_str(), "min");
                let (base, factor) = units[5].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "sec");
                assert_eq!(*factor, 60);
                // hr => min * 60
                assert_eq!(units[6].0.as_str(), "hr");
                let (base, factor) = units[6].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "min");
                assert_eq!(*factor, 60);
            }
            other => panic!("expected UnitsType, got {:?}", other),
        }
    }

    #[test]
    fn property_type_decl_units_simple_conversion() {
        // Simple case: only one derived unit
        let src = r#"property set P is
  MyUnits : type units (base, derived => base * 10);
end P;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let td = &ps.property_type_defs[0];
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::UnitsType(units)) => {
                assert_eq!(units.len(), 2);
                assert_eq!(units[0].0.as_str(), "base");
                assert!(units[0].1.is_none());
                assert_eq!(units[1].0.as_str(), "derived");
                let (base, factor) = units[1].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "base");
                assert_eq!(*factor, 10);
            }
            other => panic!("expected UnitsType, got {:?}", other),
        }
    }

    #[test]
    fn property_type_decl_units_underscore_factor() {
        // AADL allows underscores in numeric literals (e.g., 1_000).
        let src = r#"property set P is
  MyUnits : type units (base, kilo => base * 1_000, mega => kilo * 1_000);
end P;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let td = &ps.property_type_defs[0];
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::UnitsType(units)) => {
                assert_eq!(units.len(), 3);
                assert_eq!(units[0].0.as_str(), "base");
                assert!(units[0].1.is_none());
                // kilo => base * 1_000 — factor must be parsed as 1000
                let (base, factor) = units[1].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "base");
                assert_eq!(*factor, 1000);
                // mega => kilo * 1_000
                let (base2, factor2) = units[2].1.as_ref().unwrap();
                assert_eq!(base2.as_str(), "kilo");
                assert_eq!(*factor2, 1000);
            }
            other => panic!("expected UnitsType, got {:?}", other),
        }
    }

    #[test]
    fn property_type_decl_units_based_literal_factor() {
        // Based literals like 16#FF# are valid AADL integer literals.
        let src = r#"property set P is
  MyUnits : type units (base, hex => base * 16#100#);
end P;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let td = &ps.property_type_defs[0];
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::UnitsType(units)) => {
                assert_eq!(units.len(), 2);
                // hex => base * 16#100# — 16#100# is 256 in decimal
                let (base, factor) = units[1].1.as_ref().unwrap();
                assert_eq!(base.as_str(), "base");
                assert_eq!(*factor, 256);
            }
            other => panic!("expected UnitsType, got {:?}", other),
        }
    }

    #[test]
    fn property_type_decl_enumeration() {
        let src = r#"property set MyProps is
  MyStatus : type enumeration (Active, Inactive, Error);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.property_type_defs.len(), 1);
        let td = &ps.property_type_defs[0];
        assert_eq!(td.name.as_str(), "MyStatus");
        match &td.type_def {
            Some(item_tree::PropertyTypeDef::Enumeration(variants)) => {
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0].as_str(), "Active");
                assert_eq!(variants[1].as_str(), "Inactive");
                assert_eq!(variants[2].as_str(), "Error");
            }
            other => panic!("expected Enumeration, got {:?}", other),
        }
    }

    // ── Applies to validation tests ────────────────────────────

    #[test]
    fn applies_to_all() {
        let src = r#"property set MyProps is
  MyProp : aadlinteger applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        assert_eq!(def.applies_to.len(), 1);
        assert!(matches!(def.applies_to[0], item_tree::AppliesToKind::All));
    }

    #[test]
    fn applies_to_specific_categories() {
        let src = r#"property set MyProps is
  MyProp : aadlinteger applies to (thread, process);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        // The parser produces COMPONENT_CATEGORY nodes for component keywords
        // or IDENT tokens that get mapped via parse_applies_to_name
        assert!(
            def.applies_to.len() >= 2,
            "expected at least 2 applies_to entries, got {:?}",
            def.applies_to
        );
    }

    // ── Property constant lowering tests ───────────────────────

    #[test]
    fn property_constant_integer() {
        let src = r#"property set MyProps is
  MaxSize : constant aadlinteger => 1024;
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        assert_eq!(ps.property_constants.len(), 1);
        let c = &ps.property_constants[0];
        assert_eq!(c.name.as_str(), "MaxSize");
        match &c.type_def {
            Some(item_tree::PropertyTypeDef::AadlInteger { .. }) => {}
            other => panic!("expected AadlInteger type, got {:?}", other),
        }
        match &c.value {
            Some(item_tree::PropertyExpr::Integer(1024, None)) => {}
            other => panic!("expected Integer(1024), got {:?}", other),
        }
    }

    #[test]
    fn property_constant_string() {
        let src = r#"property set MyProps is
  DefaultName : constant aadlstring => "hello";
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let c = &ps.property_constants[0];
        assert_eq!(c.name.as_str(), "DefaultName");
        match &c.type_def {
            Some(item_tree::PropertyTypeDef::AadlString) => {}
            other => panic!("expected AadlString type, got {:?}", other),
        }
        match &c.value {
            Some(item_tree::PropertyExpr::StringLit(s)) => {
                assert_eq!(s, "hello");
            }
            other => panic!("expected StringLit(hello), got {:?}", other),
        }
    }

    // ── Default value lowering tests ───────────────────────────

    #[test]
    fn property_def_default_value() {
        let src = r#"property set MyProps is
  MyInt : aadlinteger => 42 applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.default_value {
            Some(item_tree::PropertyExpr::Integer(42, None)) => {}
            other => panic!("expected default Integer(42), got {:?}", other),
        }
    }

    #[test]
    fn property_def_no_default_value() {
        let src = r#"property set MyProps is
  MyInt : aadlinteger applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        assert!(def.default_value.is_none());
    }

    // ── Negative numeric value tests ───────────────────────────

    #[test]
    fn lower_negative_integer_value() {
        let src = r#"package P
public
  system S
    properties
      Offset => -10;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::Integer(v, None)) => {
                assert_eq!(v, -10);
            }
            other => panic!("expected Integer(-10), got {:?}", other),
        }
    }

    #[test]
    fn lower_negative_real_value() {
        let src = r#"package P
public
  system S
    properties
      Scale => -3.14;
  end S;
end P;
"#;
        let val = lower_first_typed_value(src);
        match val {
            Some(item_tree::PropertyExpr::Real(s, None)) => {
                assert!(s.starts_with('-'), "expected negative real, got {}", s);
            }
            other => panic!("expected Real(-3.14), got {:?}", other),
        }
    }

    // ── Contained property association tests ───────────────────

    #[test]
    fn contained_property_association() {
        let src = r#"package P
public
  system S
    properties
      Timing_Properties::Period => 10 ms applies to sub1;
  end S;
end P;
"#;
        let db = make_db();
        let file = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let ct = &tree.component_types[tree.component_types.iter().next().unwrap().0];
        let pa_idx = ct.property_associations.first().unwrap();
        let pa = &tree.property_associations[*pa_idx];
        assert!(pa.applies_to.is_some(), "expected applies_to to be set");
        let at = pa.applies_to.as_ref().unwrap();
        assert!(
            at.contains("sub1"),
            "expected applies_to to contain 'sub1', got '{}'",
            at
        );
    }

    #[test]
    fn contained_property_association_dotted_path() {
        let src = r#"package P
public
  system S
    properties
      Timing_Properties::Period => 10 ms applies to sub1.feat1;
  end S;
end P;
"#;
        let db = make_db();
        let file = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let ct = &tree.component_types[tree.component_types.iter().next().unwrap().0];
        let pa_idx = ct.property_associations.first().unwrap();
        let pa = &tree.property_associations[*pa_idx];
        assert!(pa.applies_to.is_some(), "expected applies_to to be set");
        let at = pa.applies_to.as_ref().unwrap();
        assert!(
            at.contains("sub1"),
            "expected path to contain 'sub1', got '{}'",
            at
        );
        assert!(
            at.contains("feat1"),
            "expected path to contain 'feat1', got '{}'",
            at
        );
    }

    // ── Record type definition test ────────────────────────────

    #[test]
    fn property_type_def_record() {
        let src = r#"property set MyProps is
  MyRec : record (x : aadlinteger; y : aadlstring;) applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::RecordType(fields)) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0.as_str(), "x");
                assert!(matches!(
                    fields[0].1,
                    item_tree::PropertyTypeDef::AadlInteger { .. }
                ));
                assert_eq!(fields[1].0.as_str(), "y");
                assert!(matches!(
                    fields[1].1,
                    item_tree::PropertyTypeDef::AadlString
                ));
            }
            other => panic!("expected RecordType, got {:?}", other),
        }
    }

    // ── Type reference test ────────────────────────────────────

    #[test]
    fn property_type_def_type_ref() {
        let src = r#"property set MyProps is
  MyProp : Time_Range applies to (all);
end MyProps;
"#;
        let tree = lower_first_property_set(src);
        let ps = &tree.property_sets[tree.property_sets.iter().next().unwrap().0];
        let def = &ps.property_defs[0];
        match &def.type_def {
            Some(item_tree::PropertyTypeDef::TypeRef(name)) => {
                assert_eq!(name.as_str(), "Time_Range");
            }
            other => panic!("expected TypeRef(Time_Range), got {:?}", other),
        }
    }
}
