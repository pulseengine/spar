//! Bidirectional transform between WAC compositions and AADL system implementations.
//!
//! # WAC -> AADL mapping
//!
//! | WAC construct             | AADL equivalent                                 |
//! |---------------------------|-------------------------------------------------|
//! | `package ns:name`         | `package Ns_Name_WAC`                           |
//! | `let x = new pkg:comp`    | `x: system pkg::comp` (subcomponent)            |
//! | `import name: iface`      | `requires subprogram_group access` (on type)    |
//! | `export expr`             | `provides subprogram_group access` (on type)    |
//! | `key: value.field` (arg)  | access connection in implementation             |
//! | composition as a whole    | system type + system implementation             |
//!
//! # AADL -> WAC mapping
//!
//! | AADL construct                  | WAC equivalent            |
//! |---------------------------------|---------------------------|
//! | `system implementation`         | `let` + argument wiring   |
//! | `subcomponent`                  | `let x = new ...`         |
//! | `requires subprogram_group`     | `import`                  |
//! | `provides subprogram_group`     | `export`                  |
//! | access connection               | named argument            |

use crate::wac_parser::{self, WacArg, WacDocument, WacExpr, WacStatement};
use crate::wit_parser::kebab_to_pascal;
use spar_hir_def::item_tree::{
    AccessKind, ComponentCategory, ComponentImplItem, ComponentTypeItem, ConnectedElementRef,
    ConnectionItem, ConnectionKind, Feature, FeatureKind, ItemRef, ItemTree, Package,
    SubcomponentItem,
};
use spar_hir_def::name::{ClassifierRef, Name};

/// Bidirectional WAC <-> AADL transform.
pub struct WacTransform;

impl crate::Transform for WacTransform {
    type External = WacDocument;

    fn parse_external(source: &str) -> Result<Self::External, Vec<String>> {
        wac_parser::parse_wac(source)
    }

    fn to_aadl(external: &Self::External) -> ItemTree {
        lower_wac(external)
    }

    fn from_aadl(tree: &ItemTree) -> String {
        generate_wac(tree)
    }
}

// ── WAC -> AADL lowering ───────────────────────────────────────────

/// Convert a WAC document into an AADL ItemTree.
///
/// Creates a package named `{Namespace}_{Name}_WAC` containing:
/// - A system type with features derived from imports/exports
/// - A system implementation with subcomponents from `let` bindings
///   and connections from named arguments
fn lower_wac(doc: &WacDocument) -> ItemTree {
    let mut tree = ItemTree::default();
    let mut public_items = Vec::new();
    let mut type_features = Vec::new();

    // Derive composition name from package declaration
    let (pkg_name, comp_name) = match &doc.package {
        Some(pkg) => {
            let ns = kebab_to_pascal(&pkg.namespace);
            let name = kebab_to_pascal(&pkg.name);
            let pkg_name = if ns.is_empty() {
                format!("{}_WAC", name)
            } else {
                format!("{}_{}_WAC", ns, name)
            };
            let comp_name = if ns.is_empty() {
                name
            } else {
                format!("{}_{}", ns, name)
            };
            (pkg_name, comp_name)
        }
        None => ("WAC_Package".to_string(), "Composition".to_string()),
    };

    // First pass: collect imports/exports for the system type features
    for stmt in &doc.statements {
        match stmt {
            WacStatement::Import {
                name,
                interface_path,
            } => {
                let feat_name = name.replace('-', "_");
                let classifier_name = interface_path_to_classifier(interface_path);
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&feat_name),
                    kind: FeatureKind::SubprogramGroupAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Requires),
                    classifier: Some(ClassifierRef::type_only(Name::new(&classifier_name))),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                type_features.push(feat_idx);
            }
            WacStatement::Export { expr } => {
                let (feat_name, classifier_name) = match expr {
                    WacExpr::Name(n) => {
                        let feat = n.replace('-', "_");
                        (feat.clone(), kebab_to_pascal(n))
                    }
                    WacExpr::Access { base, field } => {
                        let feat = field.replace('-', "_");
                        let cls = format!("{}_{}", kebab_to_pascal(base), kebab_to_pascal(field));
                        (feat, cls)
                    }
                };
                let feat_idx = tree.features.alloc(Feature {
                    name: Name::new(&feat_name),
                    kind: FeatureKind::SubprogramGroupAccess,
                    direction: None,
                    access_kind: Some(AccessKind::Provides),
                    classifier: Some(ClassifierRef::type_only(Name::new(&classifier_name))),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    property_associations: Vec::new(),
                });
                type_features.push(feat_idx);
            }
            WacStatement::Let { .. } => {} // handled in second pass
        }
    }

    // Create the system type
    let type_name = format!("{}", comp_name);
    let sys_type_idx = tree.component_types.alloc(ComponentTypeItem {
        name: Name::new(&type_name),
        category: ComponentCategory::System,
        is_public: true,
        extends: None,
        features: type_features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
    });
    public_items.push(ItemRef::ComponentType(sys_type_idx));

    // Second pass: create subcomponents and connections for the implementation
    let mut subcomponents = Vec::new();
    let mut connections = Vec::new();
    let mut conn_counter = 0u32;

    for stmt in &doc.statements {
        if let WacStatement::Let {
            name,
            component_path,
            args,
        } = stmt
        {
            // Create subcomponent
            let sub_name = name.replace('-', "_");
            let classifier = component_path_to_classifier(component_path);
            let sub_idx = tree.subcomponents.alloc(SubcomponentItem {
                name: Name::new(&sub_name),
                category: ComponentCategory::System,
                classifier: Some(classifier),
                is_refined: false,
                array_dimensions: Vec::new(),
                in_modes: Vec::new(),
                property_associations: Vec::new(),
            });
            subcomponents.push(sub_idx);

            // Create connections from named args
            for arg in args {
                if let WacArg::Named { key, value } = arg {
                    let conn_name = format!("c{}", conn_counter);
                    conn_counter += 1;

                    let dst = ConnectedElementRef {
                        subcomponent: Some(Name::new(&sub_name)),
                        feature: Name::new(&key.replace('-', "_")),
                    };

                    let src = match value {
                        WacExpr::Name(n) => ConnectedElementRef {
                            subcomponent: None,
                            feature: Name::new(&n.replace('-', "_")),
                        },
                        WacExpr::Access { base, field } => ConnectedElementRef {
                            subcomponent: Some(Name::new(&base.replace('-', "_"))),
                            feature: Name::new(&field.replace('-', "_")),
                        },
                    };

                    let conn_idx = tree.connections.alloc(ConnectionItem {
                        name: Name::new(&conn_name),
                        kind: ConnectionKind::Access,
                        is_bidirectional: false,
                        is_refined: false,
                        src: Some(src),
                        dst: Some(dst),
                        in_modes: Vec::new(),
                        property_associations: Vec::new(),
                    });
                    connections.push(conn_idx);
                }
            }
        }
    }

    // Create the system implementation
    let impl_name = "wac";
    let impl_idx = tree.component_impls.alloc(ComponentImplItem {
        type_name: Name::new(&type_name),
        impl_name: Name::new(impl_name),
        category: ComponentCategory::System,
        is_public: true,
        extends: None,
        subcomponents,
        connections,
        end_to_end_flows: Vec::new(),
        flow_impls: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        call_sequences: Vec::new(),
        property_associations: Vec::new(),
    });
    public_items.push(ItemRef::ComponentImpl(impl_idx));

    // Create package
    tree.packages.alloc(Package {
        name: Name::new(&pkg_name),
        with_clauses: Vec::new(),
        public_items,
        private_items: Vec::new(),
        renames: Vec::new(),
    });

    tree
}

/// Convert a WAC component path (e.g. `example:backend`) to a ClassifierRef.
fn component_path_to_classifier(path: &str) -> ClassifierRef {
    if let Some(idx) = path.find(':') {
        let ns = &path[..idx];
        let name = &path[idx + 1..];
        // Strip version if present
        let name = if let Some(at) = name.find('@') {
            &name[..at]
        } else {
            name
        };
        let pkg_name = format!("{}_{}", kebab_to_pascal(ns), kebab_to_pascal(name));
        let type_name = kebab_to_pascal(name);
        ClassifierRef::qualified(Name::new(&pkg_name), Name::new(&type_name))
    } else {
        ClassifierRef::type_only(Name::new(&kebab_to_pascal(path)))
    }
}

/// Convert an interface path (e.g. `test:config/settings`) to a classifier name.
fn interface_path_to_classifier(path: &str) -> String {
    // Extract the last segment after '/' or ':'
    let short = if let Some(idx) = path.rfind('/') {
        &path[idx + 1..]
    } else if let Some(idx) = path.rfind(':') {
        &path[idx + 1..]
    } else {
        path
    };
    // Strip version
    let short = if let Some(at) = short.find('@') {
        &short[..at]
    } else {
        short
    };
    kebab_to_pascal(short)
}

// ── AADL -> WAC generation ─────────────────────────────────────────

/// Generate WAC text from an AADL ItemTree.
///
/// Looks for system implementations and converts them to WAC compositions.
fn generate_wac(tree: &ItemTree) -> String {
    let mut out = String::new();

    // Derive package declaration from the first AADL package name
    for (_, pkg) in tree.packages.iter() {
        let pkg_name = pkg.name.as_str();
        let base = pkg_name
            .strip_suffix("_WAC")
            .or_else(|| pkg_name.strip_suffix("_wac"))
            .unwrap_or(pkg_name);

        let kebab = to_wac_kebab(base);
        // Try to split on first '_' to get namespace:name
        if let Some(idx) = kebab.find('-') {
            let ns = &kebab[..idx];
            let name = &kebab[idx + 1..];
            if !ns.is_empty() && !name.is_empty() {
                out.push_str(&format!("package {}:{};\n\n", ns, name));
            } else {
                out.push_str(&format!("package local:{};\n\n", kebab));
            }
        } else {
            out.push_str(&format!("package local:{};\n\n", kebab));
        }
        break;
    }

    // Emit imports from system type features (requires access)
    for (_, ct) in tree.component_types.iter() {
        if ct.category != ComponentCategory::System {
            continue;
        }
        for &feat_idx in &ct.features {
            let feat = &tree.features[feat_idx];
            if feat.access_kind == Some(AccessKind::Requires) {
                let name = feat.name.as_str().replace('_', "-");
                let iface = feat
                    .classifier
                    .as_ref()
                    .map(|c| to_wac_kebab(c.type_name.as_str()))
                    .unwrap_or_else(|| name.clone());
                out.push_str(&format!("import {}: local:{};\n", name, iface));
            }
        }
    }

    // Emit let bindings from subcomponents in system implementations
    for (_, ci) in tree.component_impls.iter() {
        if ci.category != ComponentCategory::System {
            continue;
        }

        for &sub_idx in &ci.subcomponents {
            let sub = &tree.subcomponents[sub_idx];
            let sub_name = sub.name.as_str().replace('_', "-");
            let comp_path = sub
                .classifier
                .as_ref()
                .map(|c| classifier_to_wac_path(c))
                .unwrap_or_else(|| format!("local:{}", sub_name));

            // Collect connections that target this subcomponent
            let mut args = Vec::new();
            for &conn_idx in &ci.connections {
                let conn = &tree.connections[conn_idx];
                if let Some(ref dst) = conn.dst {
                    if let Some(ref dst_sub) = dst.subcomponent {
                        if dst_sub.as_str() == sub.name.as_str() {
                            // This connection targets our subcomponent
                            let key = dst.feature.as_str().replace('_', "-");
                            let value = if let Some(ref src) = conn.src {
                                if let Some(ref src_sub) = src.subcomponent {
                                    format!(
                                        "{}.{}",
                                        src_sub.as_str().replace('_', "-"),
                                        src.feature.as_str().replace('_', "-")
                                    )
                                } else {
                                    src.feature.as_str().replace('_', "-")
                                }
                            } else {
                                "unknown".to_string()
                            };
                            args.push(format!("    {}: {}", key, value));
                        }
                    }
                }
            }

            if args.is_empty() {
                out.push_str(&format!("let {} = new {};\n", sub_name, comp_path));
            } else {
                out.push_str(&format!("let {} = new {} {{\n", sub_name, comp_path));
                out.push_str(&args.join(",\n"));
                out.push_str("\n};\n");
            }
        }
    }

    // Emit exports from system type features (provides access)
    for (_, ct) in tree.component_types.iter() {
        if ct.category != ComponentCategory::System {
            continue;
        }
        for &feat_idx in &ct.features {
            let feat = &tree.features[feat_idx];
            if feat.access_kind == Some(AccessKind::Provides) {
                let name = feat.name.as_str().replace('_', "-");
                out.push_str(&format!("export {};\n", name));
            }
        }
    }

    out
}

/// Convert a PascalCase or snake_case name to WAC kebab-case.
fn to_wac_kebab(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' {
            result.push('-');
            prev_lower = false;
        } else if ch.is_uppercase() {
            if prev_lower {
                result.push('-');
            }
            result.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_lower = false;
        } else {
            result.push(ch);
            prev_lower = ch.is_lowercase();
        }
    }
    result
}

/// Convert a ClassifierRef back to a WAC-style component path.
fn classifier_to_wac_path(cref: &ClassifierRef) -> String {
    if let Some(ref pkg) = cref.package {
        let pkg_kebab = to_wac_kebab(pkg.as_str());
        // Try to split the package name into ns-name format
        if let Some(idx) = pkg_kebab.find('-') {
            let ns = &pkg_kebab[..idx];
            let _rest = &pkg_kebab[idx + 1..];
            let type_kebab = to_wac_kebab(cref.type_name.as_str());
            format!("{}:{}", ns, type_kebab)
        } else {
            let type_kebab = to_wac_kebab(cref.type_name.as_str());
            format!("{}:{}", pkg_kebab, type_kebab)
        }
    } else {
        let type_kebab = to_wac_kebab(cref.type_name.as_str());
        format!("local:{}", type_kebab)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;

    fn parse_and_lower(wac_src: &str) -> ItemTree {
        let doc = wac_parser::parse_wac(wac_src).unwrap();
        lower_wac(&doc)
    }

    #[test]
    fn wac_to_aadl_creates_package() {
        let tree = parse_and_lower("package example:my-app;");
        assert_eq!(tree.packages.iter().count(), 1);
        let (_, pkg) = tree.packages.iter().next().unwrap();
        assert_eq!(pkg.name.as_str(), "Example_MyApp_WAC");
    }

    #[test]
    fn wac_to_aadl_no_package() {
        let tree = parse_and_lower("let x = new test:comp;");
        let (_, pkg) = tree.packages.iter().next().unwrap();
        assert_eq!(pkg.name.as_str(), "WAC_Package");
    }

    #[test]
    fn wac_to_aadl_creates_system_type_and_impl() {
        let src = r#"
            package example:my-app;
            let backend = new example:backend;
            export backend.api;
        "#;
        let tree = parse_and_lower(src);

        // Should have a system type
        let sys_type = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System);
        assert!(sys_type.is_some(), "should have a system type");
        let (_, st) = sys_type.unwrap();
        assert_eq!(st.name.as_str(), "Example_MyApp");

        // System type should have an export feature
        assert_eq!(st.features.len(), 1);
        let feat = &tree.features[st.features[0]];
        assert_eq!(feat.access_kind, Some(AccessKind::Provides));
        assert_eq!(feat.name.as_str(), "api");

        // Should have a system implementation
        let sys_impl = tree.component_impls.iter().next();
        assert!(sys_impl.is_some(), "should have a system implementation");
        let (_, si) = sys_impl.unwrap();
        assert_eq!(si.type_name.as_str(), "Example_MyApp");
        assert_eq!(si.impl_name.as_str(), "wac");
        assert_eq!(si.category, ComponentCategory::System);
    }

    #[test]
    fn wac_to_aadl_subcomponents() {
        let src = r#"
            package example:composed;
            let alpha = new example:component-a;
            let beta = new example:component-b;
        "#;
        let tree = parse_and_lower(src);

        let (_, si) = tree.component_impls.iter().next().unwrap();
        assert_eq!(si.subcomponents.len(), 2);

        let sub0 = &tree.subcomponents[si.subcomponents[0]];
        assert_eq!(sub0.name.as_str(), "alpha");
        assert_eq!(sub0.category, ComponentCategory::System);
        let cls0 = sub0.classifier.as_ref().unwrap();
        assert_eq!(cls0.type_name.as_str(), "ComponentA");

        let sub1 = &tree.subcomponents[si.subcomponents[1]];
        assert_eq!(sub1.name.as_str(), "beta");
    }

    #[test]
    fn wac_to_aadl_connections_from_args() {
        let src = r#"
            package example:composed;
            let a = new example:source;
            let b = new example:sink {
                input: a.output
            };
        "#;
        let tree = parse_and_lower(src);

        let (_, si) = tree.component_impls.iter().next().unwrap();
        assert_eq!(si.connections.len(), 1);

        let conn = &tree.connections[si.connections[0]];
        assert_eq!(conn.kind, ConnectionKind::Access);

        let src_ref = conn.src.as_ref().unwrap();
        assert_eq!(src_ref.subcomponent.as_ref().unwrap().as_str(), "a");
        assert_eq!(src_ref.feature.as_str(), "output");

        let dst_ref = conn.dst.as_ref().unwrap();
        assert_eq!(dst_ref.subcomponent.as_ref().unwrap().as_str(), "b");
        assert_eq!(dst_ref.feature.as_str(), "input");
    }

    #[test]
    fn wac_to_aadl_import_becomes_requires_access() {
        let src = r#"
            package test:app;
            import config: test:config/settings;
        "#;
        let tree = parse_and_lower(src);

        let (_, st) = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System)
            .unwrap();

        assert_eq!(st.features.len(), 1);
        let feat = &tree.features[st.features[0]];
        assert_eq!(feat.name.as_str(), "config");
        assert_eq!(feat.access_kind, Some(AccessKind::Requires));
        assert_eq!(feat.kind, FeatureKind::SubprogramGroupAccess);
        let cls = feat.classifier.as_ref().unwrap();
        assert_eq!(cls.type_name.as_str(), "Settings");
    }

    #[test]
    fn wac_transform_trait_roundtrip() {
        let src = r#"
            package example:my-app;
            let backend = new example:backend;
            export backend.api;
        "#;
        let doc = WacTransform::parse_external(src).unwrap();
        let tree = WacTransform::to_aadl(&doc);

        // Verify the tree is well-formed
        assert_eq!(tree.packages.iter().count(), 1);
        assert_eq!(tree.component_types.iter().count(), 1);
        assert_eq!(tree.component_impls.iter().count(), 1);

        // Generate WAC text back
        let wac_text = WacTransform::from_aadl(&tree);
        assert!(wac_text.contains("package "));
        assert!(wac_text.contains("export "));
    }

    #[test]
    fn wac_full_composition() {
        let src = r#"
            package example:full-app;
            import logger: example:logging/logger;
            let auth = new example:auth-service;
            let api = new example:api-gateway {
                auth: auth.verify,
                ...
            };
            export api.http;
        "#;
        let tree = parse_and_lower(src);

        // System type has 1 import feature + 1 export feature
        let (_, st) = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.category == ComponentCategory::System)
            .unwrap();
        assert_eq!(st.features.len(), 2);

        // Implementation has 2 subcomponents
        let (_, si) = tree.component_impls.iter().next().unwrap();
        assert_eq!(si.subcomponents.len(), 2);

        // 1 connection (from auth.verify -> api.auth)
        assert_eq!(si.connections.len(), 1);
        let conn = &tree.connections[si.connections[0]];
        let src_ref = conn.src.as_ref().unwrap();
        assert_eq!(src_ref.subcomponent.as_ref().unwrap().as_str(), "auth");
        assert_eq!(src_ref.feature.as_str(), "verify");
        let dst_ref = conn.dst.as_ref().unwrap();
        assert_eq!(dst_ref.subcomponent.as_ref().unwrap().as_str(), "api");
        assert_eq!(dst_ref.feature.as_str(), "auth");
    }
}
