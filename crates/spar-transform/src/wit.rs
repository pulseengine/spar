//! Bidirectional transform between WIT and AADL ItemTree.
//!
//! # WIT → AADL mapping
//!
//! | WIT construct      | AADL equivalent                                       |
//! |--------------------|-------------------------------------------------------|
//! | `world Foo`        | `system type FooWorld`                                |
//! | `interface Bar`    | `subprogram group type BarInterface` with subprograms |
//! | `func f(...)`      | `subprogram` with parameter features                  |
//! | `async func f(…)`  | `thread` with event data port features + Aperiodic    |
//! | `stream<T>`        | `event data port` (inner type as classifier)           |
//! | `future<T>`        | `event data port` (inner type as classifier)           |
//! | `record R`         | `data type R`                                         |
//! | `enum E`           | `data type E` (with enumeration property)             |
//! | `variant V`        | `data type V`                                         |
//! | `flags F`          | `data type F`                                         |
//! | `type alias`       | `data type` (with extends)                            |
//! | `import i`         | `requires subprogram_group access`                    |
//! | `export e`         | `provides subprogram_group access`                    |
//!
//! # AADL → WIT mapping
//!
//! | AADL construct         | WIT equivalent         |
//! |------------------------|------------------------|
//! | `system type`          | `world`                |
//! | `subprogram group`     | `interface`            |
//! | `subprogram`           | `func`                 |
//! | `thread (Aperiodic)`   | `async func`           |
//! | `data type`            | `record` (default)     |
//! | `requires access`      | `import`               |
//! | `provides access`      | `export`               |

use spar_hir_def::item_tree::{
    AccessKind, ComponentCategory, ComponentTypeItem, Direction, Feature, FeatureGroupTypeItem,
    FeatureKind, ItemRef, ItemTree, Package, PropertyAssociationItem, PropertyExpr,
};
use spar_hir_def::name::{ClassifierRef, Name, PropertyRef};

use crate::wit_parser::{
    self, WitDocument, WitFunction, WitInterface, WitType, WitTypeDef, WitWorld, WitWorldItem,
};

/// Bidirectional WIT ↔ AADL transform.
pub struct WitTransform;

impl crate::Transform for WitTransform {
    type External = WitDocument;

    fn parse_external(source: &str) -> Result<Self::External, Vec<String>> {
        wit_parser::parse_wit(source)
    }

    fn to_aadl(external: &Self::External) -> ItemTree {
        WitTransform::wit_to_item_tree(external)
    }

    fn from_aadl(tree: &ItemTree) -> String {
        WitTransform::item_tree_to_wit(tree)
    }
}

impl WitTransform {
    // ── WIT → AADL ────────────────────────────────────────────────

    /// Convert a WIT document into an AADL ItemTree.
    ///
    /// Creates a package named `{Namespace}_{Name}_WIT` (or `WIT_Package`
    /// if no package declaration). Each world becomes a system type,
    /// each interface becomes a subprogram group type, and type definitions
    /// become data component types.
    pub fn wit_to_item_tree(doc: &WitDocument) -> ItemTree {
        let mut tree = ItemTree::default();
        let mut public_items = Vec::new();

        // Lower interfaces first so worlds can reference them
        for iface in &doc.interfaces {
            let items = lower_interface(iface, &mut tree);
            public_items.extend(items);
        }

        // Lower worlds
        for world in &doc.worlds {
            let item = lower_world(world, &mut tree);
            public_items.push(item);
        }

        // Create package
        let pkg_name = match &doc.package {
            Some(pkg) => {
                let ns = wit_parser::kebab_to_pascal(&pkg.namespace);
                let name = wit_parser::kebab_to_pascal(&pkg.name);
                if ns.is_empty() {
                    format!("{}_WIT", name)
                } else {
                    format!("{}_{}_WIT", ns, name)
                }
            }
            None => "WIT_Package".to_string(),
        };

        tree.packages.alloc(Package {
            name: Name::new(&pkg_name),
            with_clauses: Vec::new(),
            public_items,
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree
    }

    /// Generate WIT text from an AADL ItemTree.
    ///
    /// System types become worlds, subprogram group types become interfaces,
    /// subprogram types become functions, and data types become records
    /// (or enums if they have an enumeration property).
    pub fn item_tree_to_wit(tree: &ItemTree) -> String {
        let mut out = String::new();

        // Try to derive a package declaration from the first package name
        for (_, pkg) in tree.packages.iter() {
            let pkg_name = pkg.name.as_str();
            // Strip _WIT suffix if present
            let base = pkg_name
                .strip_suffix("_WIT")
                .or_else(|| pkg_name.strip_suffix("_wit"))
                .unwrap_or(pkg_name);

            let kebab = wit_parser::to_kebab_case(base);
            // Try to split on '_' to get namespace:name
            if kebab.contains('-') {
                // Take the first segment as namespace if it looks like one
                // But only if there's a clear split
                let parts: Vec<&str> = kebab.splitn(2, '-').collect();
                if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                    out.push_str(&format!("package {}:{};\n\n", parts[0], parts[1]));
                } else {
                    out.push_str(&format!("package local:{};\n\n", kebab));
                }
            } else {
                out.push_str(&format!("package local:{};\n\n", kebab));
            }
            // Only use the first package
            break;
        }

        // Collect data types first (we'll reference them from interfaces)
        // Also determine which component types are which
        for (_, pkg) in tree.packages.iter() {
            for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
                match item_ref {
                    ItemRef::ComponentType(idx) => {
                        let ct = &tree.component_types[*idx];
                        match ct.category {
                            ComponentCategory::System => {
                                emit_world(ct, tree, &mut out);
                            }
                            ComponentCategory::SubprogramGroup => {
                                emit_interface_from_component(ct, tree, &mut out);
                            }
                            ComponentCategory::Data => {
                                // Data types are emitted inline within interfaces
                            }
                            ComponentCategory::Thread => {
                                // Threads with Aperiodic dispatch are emitted
                                // as async functions within interfaces
                            }
                            ComponentCategory::Subprogram => {
                                // Subprograms are emitted as functions within interfaces
                            }
                            _ => {
                                // Other categories: emit as commented world
                                out.push_str(&format!(
                                    "// AADL {} type {} (no direct WIT mapping)\n",
                                    ct.category, ct.name
                                ));
                            }
                        }
                    }
                    ItemRef::FeatureGroupType(idx) => {
                        let fgt = &tree.feature_group_types[*idx];
                        emit_interface_from_feature_group(fgt, tree, &mut out);
                    }
                    _ => {}
                }
            }
        }

        // If no packages, iterate component types directly
        if tree.packages.iter().count() == 0 {
            for (_, ct) in tree.component_types.iter() {
                match ct.category {
                    ComponentCategory::System => emit_world(ct, tree, &mut out),
                    ComponentCategory::SubprogramGroup => {
                        emit_interface_from_component(ct, tree, &mut out)
                    }
                    _ => {}
                }
            }
            for (_, fgt) in tree.feature_group_types.iter() {
                emit_interface_from_feature_group(fgt, tree, &mut out);
            }
        }

        out
    }

    /// Convert WIT source text into AADL text (for display/debugging).
    pub fn wit_to_aadl_text(wit_source: &str) -> Result<String, Vec<String>> {
        let doc = wit_parser::parse_wit(wit_source)?;
        let tree = Self::wit_to_item_tree(&doc);
        Ok(item_tree_to_aadl_text(&tree))
    }

    /// Convert an AADL ItemTree (provided as text that is parsed externally)
    /// into WIT text. This takes a pre-built ItemTree rather than parsing AADL
    /// text directly (to avoid depending on the parser crate).
    pub fn aadl_to_wit_text(tree: &ItemTree) -> String {
        Self::item_tree_to_wit(tree)
    }
}

// ── WIT → AADL lowering helpers ────────────────────────────────────

/// Lower a WIT interface into AADL items: a subprogram group type
/// plus individual subprogram types for each function, and data types
/// for each type definition.
fn lower_interface(iface: &WitInterface, tree: &mut ItemTree) -> Vec<ItemRef> {
    let mut items = Vec::new();

    // Create data types for type definitions
    for typedef in &iface.types {
        if let Some(item) = lower_type_def(typedef, tree) {
            items.push(item);
        }
    }

    // Create subprogram/thread types for functions
    let mut subprogram_features = Vec::new();
    for func in &iface.functions {
        let component_name = wit_parser::kebab_to_pascal(&func.name);
        if func.is_async {
            let ct_idx = lower_async_function(func, &component_name, tree);
            items.push(ItemRef::ComponentType(ct_idx));
        } else {
            let sub_idx = lower_function(func, &component_name, tree);
            items.push(ItemRef::ComponentType(sub_idx));
        }

        // Also create a feature on the interface's subprogram group
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new(&wit_parser::kebab_to_snake(&func.name)),
            kind: FeatureKind::SubprogramAccess,
            direction: None,
            access_kind: Some(AccessKind::Provides),
            classifier: Some(ClassifierRef::type_only(Name::new(&component_name))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        subprogram_features.push(feat_idx);
    }

    // Create the subprogram group type for the interface
    let iface_name = wit_parser::kebab_to_pascal(&iface.name);
    let sg_idx = tree.component_types.alloc(ComponentTypeItem {
        name: Name::new(&iface_name),
        category: ComponentCategory::SubprogramGroup,
        extends: None,
        features: subprogram_features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        is_public: true,
    });
    items.push(ItemRef::ComponentType(sg_idx));

    items
}

/// Lower a WIT function into an AADL subprogram component type.
fn lower_function(
    func: &WitFunction,
    aadl_name: &str,
    tree: &mut ItemTree,
) -> spar_hir_def::item_tree::ComponentTypeIdx {
    let mut features = Vec::new();

    // Parameters → in parameter features
    for (pname, ptype) in &func.params {
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new(&wit_parser::kebab_to_snake(pname)),
            kind: FeatureKind::Parameter,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: wit_type_to_classifier(ptype),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        features.push(feat_idx);
    }

    // Return type → out parameter feature
    if let Some(ret_type) = &func.result {
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new("return_value"),
            kind: FeatureKind::Parameter,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: wit_type_to_classifier(ret_type),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        features.push(feat_idx);
    }

    tree.component_types.alloc(ComponentTypeItem {
        name: Name::new(aadl_name),
        category: ComponentCategory::Subprogram,
        extends: None,
        features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        is_public: true,
    })
}

/// Lower an async WIT function into an AADL thread component type.
///
/// Async functions map to threads with `Dispatch_Protocol => Aperiodic`.
/// `stream<T>` and `future<T>` parameters become `event data port` features.
fn lower_async_function(
    func: &WitFunction,
    aadl_name: &str,
    tree: &mut ItemTree,
) -> spar_hir_def::item_tree::ComponentTypeIdx {
    let mut features = Vec::new();

    // Parameters → in event data port features
    for (pname, ptype) in &func.params {
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new(&wit_parser::kebab_to_snake(pname)),
            kind: FeatureKind::EventDataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: wit_type_to_classifier(ptype),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        features.push(feat_idx);
    }

    // Return type → out event data port feature
    if let Some(ret_type) = &func.result {
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new("return_value"),
            kind: FeatureKind::EventDataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: wit_type_to_classifier(ret_type),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        features.push(feat_idx);
    }

    // Add Dispatch_Protocol => Aperiodic property
    let dispatch_prop_idx =
        tree.property_associations
            .alloc(PropertyAssociationItem {
                name: PropertyRef {
                    property_set: Some(Name::new("Timing_Properties")),
                    property_name: Name::new("Dispatch_Protocol"),
                },
                value: "Aperiodic".to_string(),
                typed_value: Some(PropertyExpr::Enum(Name::new("Aperiodic"))),
                is_append: false,
                applies_to: None,
                in_modes: Vec::new(),
            });

    tree.component_types.alloc(ComponentTypeItem {
        name: Name::new(aadl_name),
        category: ComponentCategory::Thread,
        extends: None,
        features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: vec![dispatch_prop_idx],
        is_public: true,
    })
}

/// Lower a WIT world into an AADL system component type.
fn lower_world(world: &WitWorld, tree: &mut ItemTree) -> ItemRef {
    let mut features = Vec::new();

    // Imports → requires subprogram_group access (or event data port for async)
    for import in &world.imports {
        match import {
            WitWorldItem::Interface(name) => {
                let short_name = extract_short_name(name);
                let aadl_name = wit_parser::kebab_to_snake(&short_name);
                let classifier_name = wit_parser::kebab_to_pascal(&short_name);
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&aadl_name),
                    kind: FeatureKind::SubprogramGroupAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Requires),
                    classifier: Some(ClassifierRef::type_only(Name::new(&classifier_name))),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }
            WitWorldItem::Function(func) => {
                let func_name = wit_parser::kebab_to_snake(&func.name);
                if func.is_async {
                    let feat_idx = tree.features.alloc(Feature {
                        name: Name::new(&func_name),
                        kind: FeatureKind::EventDataPort,
                        direction: Some(Direction::In),
                        access_kind: None,
                        classifier: None,
                        is_refined: false,
                        array_dimensions: Vec::new(),
                        property_associations: Vec::new(),
                    });
                    features.push(feat_idx);
                } else {
                    let feat_idx = tree.features.alloc(Feature {
                        name: Name::new(&func_name),
                        kind: FeatureKind::SubprogramAccess,
                        direction: None,
                        access_kind: Some(AccessKind::Requires),
                        classifier: None,
                        is_refined: false,
                        array_dimensions: Vec::new(),
                        property_associations: Vec::new(),
                    });
                    features.push(feat_idx);
                }
            }
            WitWorldItem::Type(name) => {
                let aadl_name = wit_parser::kebab_to_snake(name);
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&aadl_name),
                    kind: FeatureKind::DataAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Requires),
                    classifier: None,
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }
        }
    }

    // Exports → provides subprogram_group access (or event data port for async)
    for export in &world.exports {
        match export {
            WitWorldItem::Interface(name) => {
                let short_name = extract_short_name(name);
                let aadl_name = wit_parser::kebab_to_snake(&short_name);
                let classifier_name = wit_parser::kebab_to_pascal(&short_name);
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&aadl_name),
                    kind: FeatureKind::SubprogramGroupAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Provides),
                    classifier: Some(ClassifierRef::type_only(Name::new(&classifier_name))),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }
            WitWorldItem::Function(func) => {
                let func_name = wit_parser::kebab_to_snake(&func.name);
                if func.is_async {
                    let feat_idx = tree.features.alloc(Feature {
                        name: Name::new(&func_name),
                        kind: FeatureKind::EventDataPort,
                        direction: Some(Direction::Out),
                        access_kind: None,
                        classifier: None,
                        is_refined: false,
                        array_dimensions: Vec::new(),
                        property_associations: Vec::new(),
                    });
                    features.push(feat_idx);
                } else {
                    let feat_idx = tree.features.alloc(Feature {
                        name: Name::new(&func_name),
                        kind: FeatureKind::SubprogramAccess,
                        direction: None,
                        access_kind: Some(AccessKind::Provides),
                        classifier: None,
                        is_refined: false,
                        array_dimensions: Vec::new(),
                        property_associations: Vec::new(),
                    });
                    features.push(feat_idx);
                }
            }
            WitWorldItem::Type(name) => {
                let aadl_name = wit_parser::kebab_to_snake(name);
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&aadl_name),
                    kind: FeatureKind::DataAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Provides),
                    classifier: None,
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }
        }
    }

    let world_name = format!("{}World", wit_parser::kebab_to_pascal(&world.name));
    let ct_idx = tree.component_types.alloc(ComponentTypeItem {
        name: Name::new(&world_name),
        category: ComponentCategory::System,
        extends: None,
        features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        is_public: true,
    });
    ItemRef::ComponentType(ct_idx)
}

/// Lower a WIT type definition into an AADL data component type.
fn lower_type_def(
    typedef: &WitTypeDef,
    tree: &mut ItemTree,
) -> Option<ItemRef> {
    match typedef {
        WitTypeDef::Record { name, fields } => {
            let aadl_name = wit_parser::kebab_to_pascal(name);
            let mut features = Vec::new();
            for (fname, ftype) in fields {
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&wit_parser::kebab_to_snake(fname)),
                    kind: FeatureKind::DataAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Provides),
                    classifier: wit_type_to_classifier(ftype),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends: None,
                features,
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: Vec::new(),
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
        WitTypeDef::Enum { name, cases } => {
            let aadl_name = wit_parser::kebab_to_pascal(name);
            // Add an enumeration property association
            let enum_values = cases
                .iter()
                .map(|c| wit_parser::kebab_to_pascal(c))
                .collect::<Vec<_>>()
                .join(", ");

            let prop_idx =
                tree.property_associations
                    .alloc(PropertyAssociationItem {
                        name: PropertyRef {
                            property_set: Some(Name::new("Data_Model")),
                            property_name: Name::new("Data_Representation"),
                        },
                        value: "Enum".to_string(),
                        typed_value: Some(PropertyExpr::Enum(Name::new("Enum"))),
                        is_append: false,
                        applies_to: None,
                        in_modes: Vec::new(),
                    });

            let enum_prop_idx =
                tree.property_associations
                    .alloc(PropertyAssociationItem {
                        name: PropertyRef {
                            property_set: Some(Name::new("Data_Model")),
                            property_name: Name::new("Enumerators"),
                        },
                        value: format!("({})", enum_values),
                        typed_value: Some(PropertyExpr::List(
                            cases
                                .iter()
                                .map(|c| {
                                    PropertyExpr::StringLit(wit_parser::kebab_to_pascal(c))
                                })
                                .collect(),
                        )),
                        is_append: false,
                        applies_to: None,
                        in_modes: Vec::new(),
                    });

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends: None,
                features: Vec::new(),
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: vec![prop_idx, enum_prop_idx],
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
        WitTypeDef::Variant { name, cases } => {
            let aadl_name = wit_parser::kebab_to_pascal(name);
            let mut features = Vec::new();
            for (cname, payload) in cases {
                let classifier = payload.as_ref().and_then(|t| wit_type_to_classifier(t));
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&wit_parser::kebab_to_snake(cname)),
                    kind: FeatureKind::DataAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Provides),
                    classifier,
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                features.push(feat_idx);
            }

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends: None,
                features,
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: Vec::new(),
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
        WitTypeDef::Flags { name, flags } => {
            let aadl_name = wit_parser::kebab_to_pascal(name);

            let enum_values = flags
                .iter()
                .map(|f| wit_parser::kebab_to_pascal(f))
                .collect::<Vec<_>>()
                .join(", ");

            let prop_idx =
                tree.property_associations
                    .alloc(PropertyAssociationItem {
                        name: PropertyRef {
                            property_set: Some(Name::new("Data_Model")),
                            property_name: Name::new("Data_Representation"),
                        },
                        value: "Enum".to_string(),
                        typed_value: Some(PropertyExpr::Enum(Name::new("Enum"))),
                        is_append: false,
                        applies_to: None,
                        in_modes: Vec::new(),
                    });

            let flags_prop_idx =
                tree.property_associations
                    .alloc(PropertyAssociationItem {
                        name: PropertyRef {
                            property_set: Some(Name::new("Data_Model")),
                            property_name: Name::new("Enumerators"),
                        },
                        value: format!("({})", enum_values),
                        typed_value: Some(PropertyExpr::List(
                            flags
                                .iter()
                                .map(|f| {
                                    PropertyExpr::StringLit(wit_parser::kebab_to_pascal(f))
                                })
                                .collect(),
                        )),
                        is_append: false,
                        applies_to: None,
                        in_modes: Vec::new(),
                    });

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends: None,
                features: Vec::new(),
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: vec![prop_idx, flags_prop_idx],
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
        WitTypeDef::TypeAlias { name, target } => {
            let aadl_name = wit_parser::kebab_to_pascal(name);
            let extends = wit_type_to_classifier(target);

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends,
                features: Vec::new(),
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: Vec::new(),
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
        WitTypeDef::Resource { name } => {
            // Resources map to abstract data types
            let aadl_name = wit_parser::kebab_to_pascal(name);
            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&aadl_name),
                category: ComponentCategory::Data,
                extends: None,
                features: Vec::new(),
                flow_specs: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: Vec::new(),
                is_public: true,
            });
            Some(ItemRef::ComponentType(ct_idx))
        }
    }
}

/// Map a WIT type to an AADL classifier reference.
fn wit_type_to_classifier(ty: &WitType) -> Option<ClassifierRef> {
    let name = wit_type_to_aadl_name(ty);
    if name == "unknown" {
        None
    } else {
        Some(ClassifierRef::type_only(Name::new(&name)))
    }
}

/// Map a WIT type to an AADL type name string.
fn wit_type_to_aadl_name(ty: &WitType) -> String {
    match ty {
        WitType::Bool => "Base_Types::Boolean".into(),
        WitType::U8 => "Base_Types::Unsigned_8".into(),
        WitType::U16 => "Base_Types::Unsigned_16".into(),
        WitType::U32 => "Base_Types::Unsigned_32".into(),
        WitType::U64 => "Base_Types::Unsigned_64".into(),
        WitType::S8 => "Base_Types::Integer_8".into(),
        WitType::S16 => "Base_Types::Integer_16".into(),
        WitType::S32 => "Base_Types::Integer_32".into(),
        WitType::S64 => "Base_Types::Integer_64".into(),
        WitType::F32 => "Base_Types::Float_32".into(),
        WitType::F64 => "Base_Types::Float_64".into(),
        WitType::Char => "Base_Types::Character".into(),
        WitType::String_ => "Base_Types::String".into(),
        WitType::List(inner) => format!("List_{}", wit_type_to_aadl_name(inner)),
        WitType::Option_(inner) => format!("Option_{}", wit_type_to_aadl_name(inner)),
        WitType::Stream(inner) => wit_type_to_aadl_name(inner),
        WitType::Future(inner) => wit_type_to_aadl_name(inner),
        WitType::Result { .. } => "WIT_Result".into(),
        WitType::Tuple(elems) => {
            let parts: Vec<_> = elems.iter().map(wit_type_to_aadl_name).collect();
            format!("Tuple_{}", parts.join("_"))
        }
        WitType::Named(n) => {
            if n == "_" {
                "WIT_Unit".into()
            } else {
                wit_parser::kebab_to_pascal(n)
            }
        }
    }
}

/// Extract the short name from a possibly qualified WIT path.
/// E.g., `wasi:clocks/monotonic-clock@0.2.0` → `monotonic-clock`
fn extract_short_name(path: &str) -> String {
    // Strip version (@...)
    let no_version = path.split('@').next().unwrap_or(path);
    // Take part after last '/'
    let after_slash = no_version.rsplit('/').next().unwrap_or(no_version);
    // Take part after last ':'
    after_slash.rsplit(':').next().unwrap_or(after_slash).to_string()
}

// ── AADL → WIT emission helpers ───────────────────────────────────

/// Emit a WIT world from an AADL system type.
fn emit_world(ct: &ComponentTypeItem, tree: &ItemTree, out: &mut String) {
    let name = ct.name.as_str();
    // Strip "World" suffix if present
    let base = name.strip_suffix("World").unwrap_or(name);
    let kebab = wit_parser::to_kebab_case(base);

    out.push_str(&format!("world {} {{\n", kebab));

    for &feat_idx in &ct.features {
        let feat = &tree.features[feat_idx];
        let feat_kebab = wit_parser::to_kebab_case(feat.name.as_str());

        match feat.access_kind {
            Some(AccessKind::Requires) => {
                out.push_str(&format!("    import {};\n", feat_kebab));
            }
            Some(AccessKind::Provides) => {
                match feat.kind {
                    FeatureKind::SubprogramAccess => {
                        // Inline function export — emit as func signature
                        out.push_str(&format!("    export {}: func();\n", feat_kebab));
                    }
                    _ => {
                        out.push_str(&format!("    export {};\n", feat_kebab));
                    }
                }
            }
            None => {
                // Port-style features: in → import, out → export
                match feat.direction {
                    Some(Direction::In) => {
                        out.push_str(&format!("    import {};\n", feat_kebab));
                    }
                    Some(Direction::Out) => {
                        out.push_str(&format!("    export {};\n", feat_kebab));
                    }
                    _ => {
                        out.push_str(&format!("    // {}: {} (unmapped)\n", feat_kebab, feat.kind));
                    }
                }
            }
        }
    }

    out.push_str("}\n\n");
}

/// Emit a WIT interface from an AADL subprogram group component type.
fn emit_interface_from_component(ct: &ComponentTypeItem, tree: &ItemTree, out: &mut String) {
    let name = ct.name.as_str();
    let kebab = wit_parser::to_kebab_case(name);

    out.push_str(&format!("interface {} {{\n", kebab));

    for &feat_idx in &ct.features {
        let feat = &tree.features[feat_idx];
        if feat.kind == FeatureKind::SubprogramAccess {
            let func_name = wit_parser::to_kebab_case(feat.name.as_str());
            // Try to find the corresponding subprogram type
            if let Some(ref classifier) = feat.classifier {
                if let Some(func_sig) =
                    find_subprogram_signature(&classifier.type_name, tree)
                {
                    out.push_str(&format!("    {}", func_sig));
                    continue;
                }
            }
            out.push_str(&format!("    {}: func();\n", func_name));
        }
    }

    out.push_str("}\n\n");
}

/// Emit a WIT interface from an AADL feature group type.
fn emit_interface_from_feature_group(
    fgt: &FeatureGroupTypeItem,
    tree: &ItemTree,
    out: &mut String,
) {
    let name = fgt.name.as_str();
    let kebab = wit_parser::to_kebab_case(name);

    out.push_str(&format!("interface {} {{\n", kebab));

    for &feat_idx in &fgt.features {
        let feat = &tree.features[feat_idx];
        let func_name = wit_parser::to_kebab_case(feat.name.as_str());
        out.push_str(&format!("    {}: func();\n", func_name));
    }

    out.push_str("}\n\n");
}

/// Find a subprogram or thread type in the tree and produce a WIT function signature.
fn find_subprogram_signature(name: &Name, tree: &ItemTree) -> Option<String> {
    for (_, ct) in tree.component_types.iter() {
        if (ct.category == ComponentCategory::Subprogram
            || ct.category == ComponentCategory::Thread)
            && ct.name.eq_ci(name)
        {
            return Some(emit_function_signature(ct, tree));
        }
    }
    None
}

/// Check if a component type has `Dispatch_Protocol => Aperiodic`.
fn has_aperiodic_dispatch(ct: &ComponentTypeItem, tree: &ItemTree) -> bool {
    ct.property_associations.iter().any(|&prop_idx| {
        let prop = &tree.property_associations[prop_idx];
        prop.name.property_name.as_str().eq_ignore_ascii_case("Dispatch_Protocol")
            && prop.value.eq_ignore_ascii_case("Aperiodic")
    })
}

/// Emit a WIT function signature from a subprogram or thread component type.
fn emit_function_signature(ct: &ComponentTypeItem, tree: &ItemTree) -> String {
    let func_name = wit_parser::to_kebab_case(ct.name.as_str());
    let is_async = ct.category == ComponentCategory::Thread
        && has_aperiodic_dispatch(ct, tree);
    let mut params = Vec::new();
    let mut ret_type = None;

    let port_kind = if is_async {
        FeatureKind::EventDataPort
    } else {
        FeatureKind::Parameter
    };

    for &feat_idx in &ct.features {
        let feat = &tree.features[feat_idx];
        if feat.kind == port_kind {
            let pname = wit_parser::to_kebab_case(feat.name.as_str());
            let ptype = feat
                .classifier
                .as_ref()
                .map(|c| aadl_type_to_wit(&c.type_name))
                .unwrap_or_else(|| "string".into());

            match feat.direction {
                Some(Direction::Out) => {
                    ret_type = Some(ptype);
                }
                _ => {
                    params.push(format!("{}: {}", pname, ptype));
                }
            }
        }
    }

    let params_str = params.join(", ");
    let async_prefix = if is_async { "async " } else { "" };
    match ret_type {
        Some(ret) => format!("{}: {}func({}) -> {};\n", func_name, async_prefix, params_str, ret),
        None => format!("{}: {}func({});\n", func_name, async_prefix, params_str),
    }
}

/// Map an AADL type name to a WIT type string.
fn aadl_type_to_wit(name: &Name) -> String {
    let s = name.as_str();
    // Handle Base_Types qualified names
    let type_name = s
        .strip_prefix("Base_Types::")
        .or_else(|| s.strip_prefix("Base_Types__"))
        .unwrap_or(s);

    match type_name {
        "Boolean" => "bool".into(),
        "Unsigned_8" => "u8".into(),
        "Unsigned_16" => "u16".into(),
        "Unsigned_32" => "u32".into(),
        "Unsigned_64" => "u64".into(),
        "Integer_8" => "s8".into(),
        "Integer_16" => "s16".into(),
        "Integer_32" => "s32".into(),
        "Integer_64" => "s64".into(),
        "Float_32" => "f32".into(),
        "Float_64" => "f64".into(),
        "Character" => "char".into(),
        "String" => "string".into(),
        other => {
            // Check for List_ prefix
            if let Some(inner) = other.strip_prefix("List_") {
                let inner_wit = aadl_type_to_wit(&Name::new(inner));
                return format!("list<{}>", inner_wit);
            }
            // Check for Option_ prefix
            if let Some(inner) = other.strip_prefix("Option_") {
                let inner_wit = aadl_type_to_wit(&Name::new(inner));
                return format!("option<{}>", inner_wit);
            }
            wit_parser::to_kebab_case(other)
        }
    }
}

// ── ItemTree → AADL text ───────────────────────────────────────────

/// Generate AADL text from an ItemTree (for debugging/display).
fn item_tree_to_aadl_text(tree: &ItemTree) -> String {
    let mut out = String::new();

    for (_, pkg) in tree.packages.iter() {
        out.push_str(&format!("package {}\npublic\n", pkg.name));

        for with in &pkg.with_clauses {
            out.push_str(&format!("  with {};\n", with));
        }

        for item_ref in &pkg.public_items {
            emit_item_ref(item_ref, tree, &mut out, "  ");
        }

        if !pkg.private_items.is_empty() {
            out.push_str("private\n");
            for item_ref in &pkg.private_items {
                emit_item_ref(item_ref, tree, &mut out, "  ");
            }
        }

        out.push_str(&format!("end {};\n\n", pkg.name));
    }

    out
}

fn emit_item_ref(item_ref: &ItemRef, tree: &ItemTree, out: &mut String, indent: &str) {
    match item_ref {
        ItemRef::ComponentType(idx) => {
            let ct = &tree.component_types[*idx];
            out.push_str(&format!(
                "{}{} type {}\n",
                indent, ct.category, ct.name
            ));

            if let Some(ref extends) = ct.extends {
                out.push_str(&format!("{}  extends {}\n", indent, extends));
            }

            if !ct.features.is_empty() {
                out.push_str(&format!("{}  features\n", indent));
                for &feat_idx in &ct.features {
                    let feat = &tree.features[feat_idx];
                    emit_feature(feat, out, &format!("{}    ", indent));
                }
            }

            if !ct.property_associations.is_empty() {
                out.push_str(&format!("{}  properties\n", indent));
                for &prop_idx in &ct.property_associations {
                    let prop = &tree.property_associations[prop_idx];
                    out.push_str(&format!(
                        "{}    {} => {};\n",
                        indent, prop.name, prop.value
                    ));
                }
            }

            out.push_str(&format!("{}end {};\n\n", indent, ct.name));
        }
        ItemRef::FeatureGroupType(idx) => {
            let fgt = &tree.feature_group_types[*idx];
            out.push_str(&format!(
                "{}feature group type {}\n",
                indent, fgt.name
            ));

            if !fgt.features.is_empty() {
                out.push_str(&format!("{}  features\n", indent));
                for &feat_idx in &fgt.features {
                    let feat = &tree.features[feat_idx];
                    emit_feature(feat, out, &format!("{}    ", indent));
                }
            }

            out.push_str(&format!("{}end {};\n\n", indent, fgt.name));
        }
        _ => {}
    }
}

fn emit_feature(feat: &Feature, out: &mut String, indent: &str) {
    out.push_str(indent);
    out.push_str(&format!("{}: ", feat.name));

    if let Some(ref ak) = feat.access_kind {
        out.push_str(&format!("{} ", ak));
    }
    if let Some(ref dir) = feat.direction {
        out.push_str(&format!("{} ", dir));
    }

    out.push_str(&format!("{}", feat.kind));

    if let Some(ref classifier) = feat.classifier {
        out.push_str(&format!(" {}", classifier));
    }

    out.push_str(";\n");
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_convert(wit_src: &str) -> ItemTree {
        let doc = wit_parser::parse_wit(wit_src).unwrap();
        WitTransform::wit_to_item_tree(&doc)
    }

    #[test]
    fn wit_to_aadl_creates_package() {
        let tree = parse_and_convert("package example:sensors@1.0.0;");
        assert_eq!(tree.packages.iter().count(), 1);
        let (_, pkg) = tree.packages.iter().next().unwrap();
        assert_eq!(pkg.name.as_str(), "Example_Sensors_WIT");
    }

    #[test]
    fn wit_to_aadl_no_package() {
        let tree = parse_and_convert("interface foo {}");
        let (_, pkg) = tree.packages.iter().next().unwrap();
        assert_eq!(pkg.name.as_str(), "WIT_Package");
    }

    #[test]
    fn wit_to_aadl_interface_becomes_subprogram_group() {
        let src = r#"
            interface greet {
                hello: func(name: string) -> string;
            }
        "#;
        let tree = parse_and_convert(src);

        // Should have a subprogram group type named "Greet"
        let sg = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::SubprogramGroup);
        assert!(sg.is_some(), "should have a subprogram group type");
        let (_, sg) = sg.unwrap();
        assert_eq!(sg.name.as_str(), "Greet");
        assert_eq!(sg.features.len(), 1); // one function

        // Should have a subprogram type named "Hello"
        let sp = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Subprogram);
        assert!(sp.is_some(), "should have a subprogram type");
        let (_, sp) = sp.unwrap();
        assert_eq!(sp.name.as_str(), "Hello");
        assert_eq!(sp.features.len(), 2); // name (in) + return_value (out)
    }

    #[test]
    fn wit_to_aadl_world_becomes_system() {
        let src = r#"
            world my-app {
                import some-interface;
                export other-interface;
            }
        "#;
        let tree = parse_and_convert(src);

        let sys = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System);
        assert!(sys.is_some());
        let (_, sys) = sys.unwrap();
        assert_eq!(sys.name.as_str(), "MyAppWorld");
        assert_eq!(sys.features.len(), 2); // import + export

        // Check import is requires
        let feat0 = &tree.features[sys.features[0]];
        assert_eq!(feat0.access_kind, Some(AccessKind::Requires));
        assert_eq!(feat0.name.as_str(), "some_interface");

        // Check export is provides
        let feat1 = &tree.features[sys.features[1]];
        assert_eq!(feat1.access_kind, Some(AccessKind::Provides));
        assert_eq!(feat1.name.as_str(), "other_interface");
    }

    #[test]
    fn wit_to_aadl_record_becomes_data_type() {
        let src = r#"
            interface data {
                record point {
                    x: f64,
                    y: f64,
                }
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data);
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.name.as_str(), "Point");
        assert_eq!(data.features.len(), 2); // x, y

        let feat_x = &tree.features[data.features[0]];
        assert_eq!(feat_x.name.as_str(), "x");
        assert!(feat_x.classifier.is_some());
    }

    #[test]
    fn wit_to_aadl_enum_becomes_data_type_with_property() {
        let src = r#"
            interface types {
                enum color {
                    red,
                    green,
                    blue,
                }
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data);
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.name.as_str(), "Color");
        // Should have enumeration property associations
        assert_eq!(data.property_associations.len(), 2);
    }

    #[test]
    fn wit_to_aadl_type_alias_extends() {
        let src = r#"
            interface types {
                type byte-list = list<u8>;
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data);
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.name.as_str(), "ByteList");
        assert!(data.extends.is_some());
    }

    #[test]
    fn wit_to_aadl_text_full() {
        let src = r#"
            package example:sensors@1.0.0;

            interface readings {
                record sensor-data {
                    temperature: f64,
                    pressure: f64,
                    timestamp: u64,
                }

                get-reading: func() -> sensor-data;
                calibrate: func(offset: f64) -> result<_, string>;
            }

            world sensor {
                export readings;
                import wasi:clocks/monotonic-clock@0.2.0;
            }
        "#;
        let aadl_text = WitTransform::wit_to_aadl_text(src).unwrap();
        // Verify it contains expected AADL constructs
        assert!(
            aadl_text.contains("package Example_Sensors_WIT"),
            "should have package: {}",
            aadl_text
        );
        assert!(
            aadl_text.contains("system type SensorWorld"),
            "should have system type: {}",
            aadl_text
        );
        assert!(
            aadl_text.contains("data type SensorData"),
            "should have data type: {}",
            aadl_text
        );
        assert!(
            aadl_text.contains("subprogram type GetReading"),
            "should have subprogram: {}",
            aadl_text
        );
        assert!(
            aadl_text.contains("subprogram group type Readings"),
            "should have subprogram group: {}",
            aadl_text
        );
    }

    #[test]
    fn aadl_to_wit_system_becomes_world() {
        let mut tree = ItemTree::default();

        let feat_idx = tree.features.alloc(Feature {
            name: Name::new("my_import"),
            kind: FeatureKind::SubprogramGroupAccess,
            direction: None,
            access_kind: Some(AccessKind::Requires),
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyAppWorld"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![feat_idx],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Test_WIT"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let wit_text = WitTransform::item_tree_to_wit(&tree);
        assert!(wit_text.contains("world my-app"), "should have world: {}", wit_text);
        assert!(
            wit_text.contains("import my-import"),
            "should have import: {}",
            wit_text
        );
    }

    #[test]
    fn aadl_to_wit_subprogram_group_becomes_interface() {
        let mut tree = ItemTree::default();

        // Create a subprogram type
        let param_feat = tree.features.alloc(Feature {
            name: Name::new("msg"),
            kind: FeatureKind::Parameter,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("Base_Types::String"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let sp_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Greet"),
            category: ComponentCategory::Subprogram,
            extends: None,
            features: vec![param_feat],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        // Create a subprogram group type with a feature referencing the subprogram
        let sp_feat = tree.features.alloc(Feature {
            name: Name::new("greet"),
            kind: FeatureKind::SubprogramAccess,
            direction: None,
            access_kind: Some(AccessKind::Provides),
            classifier: Some(ClassifierRef::type_only(Name::new("Greet"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let sg_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyInterface"),
            category: ComponentCategory::SubprogramGroup,
            extends: None,
            features: vec![sp_feat],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Test_WIT"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(sp_idx),
                ItemRef::ComponentType(sg_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let wit_text = WitTransform::item_tree_to_wit(&tree);
        assert!(
            wit_text.contains("interface my-interface"),
            "should have interface: {}",
            wit_text
        );
        assert!(
            wit_text.contains("greet: func(msg: string)"),
            "should have func: {}",
            wit_text
        );
    }

    #[test]
    fn round_trip_wit_to_aadl_to_wit() {
        let original_wit = r#"
            package example:demo@1.0.0;

            interface operations {
                add: func(a: s32, b: s32) -> s32;
                reset: func();
            }

            world calculator {
                export operations;
            }
        "#;

        // WIT → AADL
        let doc = wit_parser::parse_wit(original_wit).unwrap();
        let tree = WitTransform::wit_to_item_tree(&doc);

        // AADL → WIT
        let roundtrip_wit = WitTransform::item_tree_to_wit(&tree);

        // Verify key constructs survive the round trip
        assert!(
            roundtrip_wit.contains("world calculator"),
            "world should survive round trip: {}",
            roundtrip_wit
        );
        assert!(
            roundtrip_wit.contains("interface operations"),
            "interface should survive round trip: {}",
            roundtrip_wit
        );
        assert!(
            roundtrip_wit.contains("add: func("),
            "function should survive round trip: {}",
            roundtrip_wit
        );
        assert!(
            roundtrip_wit.contains("export operations"),
            "export should survive round trip: {}",
            roundtrip_wit
        );
    }

    #[test]
    fn empty_world() {
        let src = "world empty {}";
        let tree = parse_and_convert(src);

        let sys = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System);
        assert!(sys.is_some());
        let (_, sys) = sys.unwrap();
        assert_eq!(sys.name.as_str(), "EmptyWorld");
        assert!(sys.features.is_empty());
    }

    #[test]
    fn empty_interface() {
        let src = "interface empty {}";
        let tree = parse_and_convert(src);

        let sg = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::SubprogramGroup);
        assert!(sg.is_some());
        let (_, sg) = sg.unwrap();
        assert_eq!(sg.name.as_str(), "Empty");
        assert!(sg.features.is_empty());
    }

    #[test]
    fn wit_primitive_type_mapping() {
        assert_eq!(wit_type_to_aadl_name(&WitType::Bool), "Base_Types::Boolean");
        assert_eq!(wit_type_to_aadl_name(&WitType::U32), "Base_Types::Unsigned_32");
        assert_eq!(wit_type_to_aadl_name(&WitType::S64), "Base_Types::Integer_64");
        assert_eq!(wit_type_to_aadl_name(&WitType::F64), "Base_Types::Float_64");
        assert_eq!(wit_type_to_aadl_name(&WitType::String_), "Base_Types::String");
        assert_eq!(wit_type_to_aadl_name(&WitType::Char), "Base_Types::Character");
    }

    #[test]
    fn wit_complex_type_mapping() {
        assert_eq!(
            wit_type_to_aadl_name(&WitType::List(Box::new(WitType::U8))),
            "List_Base_Types::Unsigned_8"
        );
        assert_eq!(
            wit_type_to_aadl_name(&WitType::Option_(Box::new(WitType::String_))),
            "Option_Base_Types::String"
        );
        assert_eq!(
            wit_type_to_aadl_name(&WitType::Result {
                ok: None,
                err: None
            }),
            "WIT_Result"
        );
    }

    #[test]
    fn extract_short_name_tests() {
        assert_eq!(extract_short_name("wasi:clocks/monotonic-clock@0.2.0"), "monotonic-clock");
        assert_eq!(extract_short_name("wasi:http/types"), "types");
        assert_eq!(extract_short_name("simple-name"), "simple-name");
        assert_eq!(extract_short_name("ns:name"), "name");
    }

    #[test]
    fn aadl_type_to_wit_mapping() {
        assert_eq!(aadl_type_to_wit(&Name::new("Boolean")), "bool");
        assert_eq!(aadl_type_to_wit(&Name::new("Unsigned_32")), "u32");
        assert_eq!(aadl_type_to_wit(&Name::new("Integer_64")), "s64");
        assert_eq!(aadl_type_to_wit(&Name::new("Float_64")), "f64");
        assert_eq!(aadl_type_to_wit(&Name::new("String")), "string");
        assert_eq!(aadl_type_to_wit(&Name::new("Character")), "char");
        assert_eq!(aadl_type_to_wit(&Name::new("List_Unsigned_8")), "list<u8>");
        assert_eq!(
            aadl_type_to_wit(&Name::new("Option_String")),
            "option<string>"
        );
    }

    #[test]
    fn wit_to_aadl_variant() {
        let src = r#"
            interface types {
                variant filter {
                    all,
                    none,
                    some(string),
                }
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data && ct.name.as_str() == "Filter");
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.features.len(), 3); // all, none, some
    }

    #[test]
    fn wit_to_aadl_flags() {
        let src = r#"
            interface perms {
                flags permissions {
                    read,
                    write,
                    exec,
                }
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data);
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.name.as_str(), "Permissions");
        assert_eq!(data.property_associations.len(), 2);
    }

    #[test]
    fn wit_to_aadl_resource() {
        let src = r#"
            interface io {
                resource stream {}
            }
        "#;
        let tree = parse_and_convert(src);

        let data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Data);
        assert!(data.is_some());
        let (_, data) = data.unwrap();
        assert_eq!(data.name.as_str(), "Stream");
    }

    #[test]
    fn multiple_interfaces_and_worlds() {
        let src = r#"
            package test:multi@0.1.0;

            interface api-one {
                do-thing: func() -> bool;
            }

            interface api-two {
                other-thing: func(x: u32);
            }

            world app-one {
                export api-one;
            }

            world app-two {
                import api-one;
                export api-two;
            }
        "#;
        let tree = parse_and_convert(src);

        // Should have 2 subprogram group types + 2 subprogram types + 2 system types
        let sg_count = tree
            .component_types
            .iter()
            .filter(|(_, ct)| ct.category == ComponentCategory::SubprogramGroup)
            .count();
        assert_eq!(sg_count, 2, "should have 2 subprogram group types");

        let sys_count = tree
            .component_types
            .iter()
            .filter(|(_, ct)| ct.category == ComponentCategory::System)
            .count();
        assert_eq!(sys_count, 2, "should have 2 system types");

        let sp_count = tree
            .component_types
            .iter()
            .filter(|(_, ct)| ct.category == ComponentCategory::Subprogram)
            .count();
        assert_eq!(sp_count, 2, "should have 2 subprogram types");
    }

    #[test]
    fn world_with_qualified_imports() {
        let src = r#"
            world http-handler {
                import wasi:http/types@0.2.0;
                import wasi:http/incoming-handler@0.2.0;
                export wasi:http/handler@0.2.0;
            }
        "#;
        let tree = parse_and_convert(src);

        let sys = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System)
            .unwrap()
            .1;

        assert_eq!(sys.features.len(), 3);
        // First import: types
        let feat0 = &tree.features[sys.features[0]];
        assert_eq!(feat0.name.as_str(), "types");
        assert_eq!(feat0.access_kind, Some(AccessKind::Requires));
        // Second import: incoming-handler → incoming_handler
        let feat1 = &tree.features[sys.features[1]];
        assert_eq!(feat1.name.as_str(), "incoming_handler");
    }

    #[test]
    fn function_with_multiple_params_and_complex_return() {
        let src = r#"
            interface math {
                compute: func(a: f64, b: f64, op: string) -> result<f64, string>;
            }
        "#;
        let tree = parse_and_convert(src);

        let sp = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Subprogram)
            .unwrap()
            .1;

        assert_eq!(sp.name.as_str(), "Compute");
        // 3 in params + 1 out return = 4 features
        assert_eq!(sp.features.len(), 4);

        // Verify parameter directions
        let feat_a = &tree.features[sp.features[0]];
        assert_eq!(feat_a.direction, Some(Direction::In));
        assert_eq!(feat_a.name.as_str(), "a");

        let feat_ret = &tree.features[sp.features[3]];
        assert_eq!(feat_ret.direction, Some(Direction::Out));
        assert_eq!(feat_ret.name.as_str(), "return_value");
    }

    #[test]
    fn async_function_becomes_thread() {
        let src = r#"
            interface pipeline {
                process: async func(input: stream<u32>) -> stream<f64>;
            }
        "#;
        let tree = parse_and_convert(src);

        // Should have a thread type named "Process" (not a subprogram)
        let thread = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Thread);
        assert!(thread.is_some(), "should have a thread type");
        let (_, thread) = thread.unwrap();
        assert_eq!(thread.name.as_str(), "Process");

        // Should have event data port features
        assert_eq!(thread.features.len(), 2); // input + return_value
        let feat_input = &tree.features[thread.features[0]];
        assert_eq!(feat_input.kind, FeatureKind::EventDataPort);
        assert_eq!(feat_input.direction, Some(Direction::In));
        assert_eq!(feat_input.name.as_str(), "input");

        let feat_ret = &tree.features[thread.features[1]];
        assert_eq!(feat_ret.kind, FeatureKind::EventDataPort);
        assert_eq!(feat_ret.direction, Some(Direction::Out));
        assert_eq!(feat_ret.name.as_str(), "return_value");

        // Should have Dispatch_Protocol => Aperiodic property
        assert_eq!(thread.property_associations.len(), 1);
        let prop = &tree.property_associations[thread.property_associations[0]];
        assert_eq!(prop.name.property_name.as_str(), "Dispatch_Protocol");
        assert_eq!(prop.value, "Aperiodic");
    }

    #[test]
    fn async_function_roundtrip() {
        let src = r#"
            package example:pipeline@1.0.0;

            interface pipeline {
                process: async func(input: u32) -> f64;
            }
        "#;

        // WIT → AADL
        let doc = wit_parser::parse_wit(src).unwrap();
        let tree = WitTransform::wit_to_item_tree(&doc);

        // AADL → WIT
        let roundtrip_wit = WitTransform::item_tree_to_wit(&tree);

        // Should contain async func in the output
        assert!(
            roundtrip_wit.contains("async func"),
            "should have async func in roundtrip: {}",
            roundtrip_wit
        );
        assert!(
            roundtrip_wit.contains("interface pipeline"),
            "should have interface: {}",
            roundtrip_wit
        );
    }

    #[test]
    fn sync_function_stays_subprogram() {
        let src = r#"
            interface api {
                greet: func(name: string) -> string;
            }
        "#;
        let tree = parse_and_convert(src);

        // Should NOT have a thread type
        let thread = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Thread);
        assert!(thread.is_none(), "should not have a thread type");

        // Should have a subprogram type
        let sp = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::Subprogram);
        assert!(sp.is_some(), "should have a subprogram type");
    }

    #[test]
    fn world_with_async_export_becomes_event_data_port() {
        let src = r#"
            world processor {
                export process: async func(input: stream<u32>) -> stream<f64>;
            }
        "#;
        let tree = parse_and_convert(src);

        let sys = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System)
            .unwrap()
            .1;

        assert_eq!(sys.features.len(), 1);
        let feat = &tree.features[sys.features[0]];
        assert_eq!(feat.kind, FeatureKind::EventDataPort);
        assert_eq!(feat.direction, Some(Direction::Out));
        assert_eq!(feat.name.as_str(), "process");
    }
}
