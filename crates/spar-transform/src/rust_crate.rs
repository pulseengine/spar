//! Transform Rust crate metadata into virtual AADL packages.
//!
//! Maps cargo workspace concepts to AADL constructs:
//!
//! | Cargo concept          | AADL equivalent                           |
//! |------------------------|-------------------------------------------|
//! | workspace              | system type + impl (with subcomponents)   |
//! | crate (lib target)     | subprogram group type                     |
//! | crate (bin target)     | process type                              |
//! | dependency (workspace) | with clause                               |
//! | dependency (requires)  | requires subprogram group access feature  |
//! | feature flag           | mode (default feature → initial mode)     |

use crate::cargo_metadata::CargoMetadata;
use crate::wit_parser::kebab_to_pascal;
use spar_hir_def::item_tree::*;
use spar_hir_def::name::{ClassifierRef, Name};
use std::collections::HashSet;

/// Bidirectional Rust crate ↔ AADL transform.
pub struct RustCrateTransform;

impl crate::Transform for RustCrateTransform {
    type External = CargoMetadata;

    fn parse_external(source: &str) -> Result<Self::External, Vec<String>> {
        crate::cargo_metadata::parse_cargo_metadata(source).map_err(|e| vec![e])
    }

    fn to_aadl(metadata: &Self::External) -> ItemTree {
        lower_metadata(metadata)
    }

    fn from_aadl(_tree: &ItemTree) -> String {
        // Reverse transform not meaningful for cargo metadata
        String::from("// Reverse transform from AADL to cargo metadata not supported\n")
    }
}

/// Convert a crate name (kebab-case) to AADL PascalCase identifier.
fn crate_name_to_aadl(name: &str) -> String {
    kebab_to_pascal(name)
}

/// Extract workspace member names from the `workspace_members` list.
///
/// Cargo encodes workspace members as strings like:
///   `"my-crate 0.1.0 (path+file:///...)"`
/// We extract just the first whitespace-delimited token (the crate name).
fn workspace_member_names(members: &[String]) -> HashSet<&str> {
    members
        .iter()
        .filter_map(|m| m.split_whitespace().next())
        .collect()
}

/// Lower a `CargoMetadata` into an AADL `ItemTree`.
fn lower_metadata(metadata: &CargoMetadata) -> ItemTree {
    let mut tree = ItemTree::default();

    let workspace_names = workspace_member_names(&metadata.workspace_members);

    // Track per-crate package indices so we can build the workspace system
    let mut crate_packages: Vec<(String, String)> = Vec::new(); // (crate_name, aadl_name)

    for pkg in &metadata.packages {
        if !workspace_names.contains(pkg.name.as_str()) {
            continue;
        }

        let aadl_name = crate_name_to_aadl(&pkg.name);

        // Build with-clauses from workspace-internal normal dependencies
        let with_clauses: Vec<Name> = pkg
            .dependencies
            .iter()
            .filter(|d| d.kind.is_none() || d.kind.as_deref() == Some(""))
            .filter(|d| {
                let dep_name = d.rename.as_deref().unwrap_or(&d.name);
                workspace_names.contains(dep_name)
            })
            .map(|d| {
                let dep_name = d.rename.as_deref().unwrap_or(&d.name);
                Name::new(&crate_name_to_aadl(dep_name))
            })
            .collect();

        // Create component types for each target
        let mut public_items = Vec::new();

        for target in &pkg.targets {
            let category = if target.kind.contains(&"bin".to_string()) {
                ComponentCategory::Process
            } else if target.kind.contains(&"lib".to_string()) {
                ComponentCategory::SubprogramGroup
            } else {
                // Skip proc-macro, custom-build, test, bench, example targets
                continue;
            };

            let type_name = crate_name_to_aadl(&target.name);

            // Build features for external (non-workspace) dependencies
            let dep_features: Vec<FeatureIdx> = pkg
                .dependencies
                .iter()
                .filter(|d| d.kind.is_none() || d.kind.as_deref() == Some(""))
                .filter(|d| {
                    let dep_name = d.rename.as_deref().unwrap_or(&d.name);
                    !workspace_names.contains(dep_name)
                })
                .map(|d| {
                    let dep_name = d.rename.as_deref().unwrap_or(&d.name);
                    let feat_aadl_name = crate_name_to_aadl(dep_name);
                    tree.features.alloc(Feature {
                        name: Name::new(&dep_name.replace('-', "_")),
                        kind: FeatureKind::SubprogramGroupAccess,
                        direction: None,
                        access_kind: Some(AccessKind::Requires),
                        classifier: Some(ClassifierRef::type_only(Name::new(&feat_aadl_name))),
                        is_refined: false,
                        array_dimensions: Vec::new(),
                        property_associations: Vec::new(),
                    })
                })
                .collect();

            // Create modes from feature flags
            let modes: Vec<ModeIdx> = pkg
                .features
                .keys()
                .map(|feat_name| {
                    let is_initial = feat_name == "default";
                    tree.modes.alloc(ModeItem {
                        name: Name::new(&feat_name.replace('-', "_")),
                        is_initial,
                    })
                })
                .collect();

            let ct_idx = tree.component_types.alloc(ComponentTypeItem {
                name: Name::new(&type_name),
                category,
                is_public: true,
                extends: None,
                features: dep_features,
                flow_specs: Vec::new(),
                modes,
                mode_transitions: Vec::new(),
                prototypes: Vec::new(),
                property_associations: Vec::new(),
            });
            public_items.push(ItemRef::ComponentType(ct_idx));
        }

        // Create the package for this crate
        tree.packages.alloc(Package {
            name: Name::new(&aadl_name),
            with_clauses,
            public_items,
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        crate_packages.push((pkg.name.clone(), aadl_name));
    }

    // If there are multiple workspace members, create a workspace system package
    if crate_packages.len() > 1 {
        let ws_name = if metadata.workspace_root.is_empty() {
            "Workspace".to_string()
        } else {
            // Use the last path component as the workspace name
            let base = metadata
                .workspace_root
                .rsplit('/')
                .next()
                .unwrap_or("Workspace");
            crate_name_to_aadl(base)
        };

        let ws_pkg_name = format!("{}_Workspace", ws_name);

        // Create with-clauses referencing all crate packages
        let ws_with_clauses: Vec<Name> = crate_packages
            .iter()
            .map(|(_, aadl_name)| Name::new(aadl_name))
            .collect();

        // Create a system type for the workspace
        let ws_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new(&ws_name),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Create subcomponents for each crate in the implementation
        let subcomponents: Vec<SubcomponentIdx> = crate_packages
            .iter()
            .map(|(crate_name, aadl_name)| {
                // Determine the category from the first target of this crate
                let pkg_data = metadata.packages.iter().find(|p| p.name == *crate_name);
                let category = pkg_data
                    .and_then(|p| p.targets.first())
                    .map(|t| {
                        if t.kind.contains(&"bin".to_string()) {
                            ComponentCategory::Process
                        } else {
                            ComponentCategory::SubprogramGroup
                        }
                    })
                    .unwrap_or(ComponentCategory::SubprogramGroup);

                let target_name = pkg_data
                    .and_then(|p| p.targets.first())
                    .map(|t| crate_name_to_aadl(&t.name))
                    .unwrap_or_else(|| aadl_name.clone());

                tree.subcomponents.alloc(SubcomponentItem {
                    name: Name::new(&crate_name.replace('-', "_")),
                    category,
                    classifier: Some(ClassifierRef::qualified(
                        Name::new(aadl_name),
                        Name::new(&target_name),
                    )),
                    is_refined: false,
                    array_dimensions: Vec::new(),
                    in_modes: Vec::new(),
                    property_associations: Vec::new(),
                })
            })
            .collect();

        // Create a system implementation
        let ws_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new(&ws_name),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents,
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new(&ws_pkg_name),
            with_clauses: ws_with_clauses,
            public_items: vec![
                ItemRef::ComponentType(ws_type_idx),
                ItemRef::ComponentImpl(ws_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });
    }

    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;

    /// Build a minimal 2-crate workspace metadata JSON for testing.
    fn two_crate_workspace_json() -> &'static str {
        r#"{
            "packages": [
                {
                    "name": "my-core",
                    "version": "0.1.0",
                    "dependencies": [],
                    "targets": [{"name": "my-core", "kind": ["lib"]}],
                    "features": {
                        "default": ["std"],
                        "std": [],
                        "no_std": []
                    },
                    "manifest_path": "/tmp/ws/core/Cargo.toml"
                },
                {
                    "name": "my-app",
                    "version": "0.1.0",
                    "dependencies": [
                        {"name": "my-core", "kind": null},
                        {"name": "serde", "kind": null},
                        {"name": "tokio", "kind": "dev"}
                    ],
                    "targets": [
                        {"name": "my-app", "kind": ["bin"]}
                    ],
                    "features": {},
                    "manifest_path": "/tmp/ws/app/Cargo.toml"
                },
                {
                    "name": "serde",
                    "version": "1.0.0",
                    "dependencies": [],
                    "targets": [{"name": "serde", "kind": ["lib"]}],
                    "features": {},
                    "manifest_path": "/tmp/serde/Cargo.toml"
                }
            ],
            "workspace_members": [
                "my-core 0.1.0 (path+file:///tmp/ws/core)",
                "my-app 0.1.0 (path+file:///tmp/ws/app)"
            ],
            "workspace_root": "/tmp/ws",
            "resolve": null,
            "target_directory": "/tmp/ws/target",
            "version": 1
        }"#
    }

    #[test]
    fn parse_external_roundtrip() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        assert_eq!(meta.packages.len(), 3);
        assert_eq!(meta.workspace_members.len(), 2);
    }

    #[test]
    fn generates_correct_packages() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Should have 3 packages: MyCore, MyApp, Ws_Workspace
        assert_eq!(tree.packages.iter().count(), 3);

        let pkg_names: Vec<&str> = tree.packages.iter().map(|(_, p)| p.name.as_str()).collect();
        assert!(pkg_names.contains(&"MyCore"), "missing MyCore: {:?}", pkg_names);
        assert!(pkg_names.contains(&"MyApp"), "missing MyApp: {:?}", pkg_names);
        // Workspace package
        assert!(
            pkg_names.iter().any(|n| n.contains("Workspace")),
            "missing workspace package: {:?}",
            pkg_names
        );
    }

    #[test]
    fn with_clauses_for_workspace_deps() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Find MyApp package — it depends on my-core (workspace) and serde (external)
        let app_pkg = tree
            .packages
            .iter()
            .find(|(_, p)| p.name.as_str() == "MyApp")
            .map(|(_, p)| p)
            .expect("MyApp package not found");

        // with-clauses should include MyCore (workspace dep) but not Serde (external)
        let with_names: Vec<&str> = app_pkg.with_clauses.iter().map(|n| n.as_str()).collect();
        assert!(
            with_names.contains(&"MyCore"),
            "MyApp should have MyCore with-clause: {:?}",
            with_names
        );
        assert!(
            !with_names.contains(&"Serde"),
            "MyApp should NOT have Serde with-clause (external): {:?}",
            with_names
        );
    }

    #[test]
    fn bin_target_becomes_process() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Find the component type for my-app (bin target)
        let app_ct = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "MyApp")
            .map(|(_, ct)| ct)
            .expect("MyApp component type not found");
        assert_eq!(app_ct.category, ComponentCategory::Process);
    }

    #[test]
    fn lib_target_becomes_subprogram_group() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Find the component type for my-core (lib target)
        let core_ct = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "MyCore")
            .map(|(_, ct)| ct)
            .expect("MyCore component type not found");
        assert_eq!(core_ct.category, ComponentCategory::SubprogramGroup);
    }

    #[test]
    fn feature_flags_become_modes() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Find the MyCore component type — it has 3 features: default, std, no_std
        let core_ct = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "MyCore")
            .map(|(_, ct)| ct)
            .expect("MyCore component type not found");

        assert_eq!(
            core_ct.modes.len(),
            3,
            "expected 3 modes, got {}",
            core_ct.modes.len()
        );

        let mode_names: Vec<&str> = core_ct
            .modes
            .iter()
            .map(|idx| tree.modes[*idx].name.as_str())
            .collect();
        assert!(mode_names.contains(&"default"), "missing default mode: {:?}", mode_names);
        assert!(mode_names.contains(&"std"), "missing std mode: {:?}", mode_names);
        assert!(mode_names.contains(&"no_std"), "missing no_std mode: {:?}", mode_names);

        // "default" should be the initial mode
        let default_mode = core_ct
            .modes
            .iter()
            .map(|idx| &tree.modes[*idx])
            .find(|m| m.name.as_str() == "default")
            .expect("default mode not found");
        assert!(default_mode.is_initial);

        // Other modes should not be initial
        let std_mode = core_ct
            .modes
            .iter()
            .map(|idx| &tree.modes[*idx])
            .find(|m| m.name.as_str() == "std")
            .expect("std mode not found");
        assert!(!std_mode.is_initial);
    }

    #[test]
    fn external_deps_become_requires_features() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Find the MyApp component type — serde is an external dep
        let app_ct = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "MyApp")
            .map(|(_, ct)| ct)
            .expect("MyApp component type not found");

        // Should have a requires feature for serde (external normal dep)
        // tokio is dev dep, so excluded
        assert_eq!(
            app_ct.features.len(),
            1,
            "expected 1 feature (serde), got {}",
            app_ct.features.len()
        );

        let feat = &tree.features[app_ct.features[0]];
        assert_eq!(feat.name.as_str(), "serde");
        assert_eq!(feat.kind, FeatureKind::SubprogramGroupAccess);
        assert_eq!(feat.access_kind, Some(AccessKind::Requires));
    }

    #[test]
    fn workspace_system_has_subcomponents() {
        let meta = RustCrateTransform::parse_external(two_crate_workspace_json()).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Should have a system implementation with subcomponents
        assert!(
            tree.component_impls.iter().count() > 0,
            "expected at least one component impl for workspace system"
        );

        let ws_impl = tree
            .component_impls
            .iter()
            .next()
            .map(|(_, ci)| ci)
            .expect("workspace impl not found");

        assert_eq!(ws_impl.category, ComponentCategory::System);
        assert_eq!(ws_impl.subcomponents.len(), 2);

        let sub_names: Vec<&str> = ws_impl
            .subcomponents
            .iter()
            .map(|idx| tree.subcomponents[*idx].name.as_str())
            .collect();
        assert!(sub_names.contains(&"my_core"), "missing my_core subcomponent: {:?}", sub_names);
        assert!(sub_names.contains(&"my_app"), "missing my_app subcomponent: {:?}", sub_names);
    }

    #[test]
    fn single_crate_no_workspace_system() {
        let json = r#"{
            "packages": [{
                "name": "solo",
                "version": "0.1.0",
                "dependencies": [],
                "targets": [{"name": "solo", "kind": ["lib"]}],
                "features": {},
                "manifest_path": "/tmp/solo/Cargo.toml"
            }],
            "workspace_members": ["solo 0.1.0 (path+file:///tmp/solo)"],
            "workspace_root": "/tmp/solo",
            "resolve": null,
            "target_directory": "/tmp/solo/target",
            "version": 1
        }"#;

        let meta = RustCrateTransform::parse_external(json).unwrap();
        let tree = RustCrateTransform::to_aadl(&meta);

        // Single crate: only one package, no workspace system
        assert_eq!(tree.packages.iter().count(), 1);
        assert_eq!(tree.component_impls.iter().count(), 0);
    }

    #[test]
    fn from_aadl_returns_not_supported() {
        let tree = ItemTree::default();
        let result = RustCrateTransform::from_aadl(&tree);
        assert!(result.contains("not supported"));
    }
}
