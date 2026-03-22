//! Extends/inheritance validation rules (AS5506 §4.4).
//!
//! Validates `extends` clauses on component types and implementations:
//! - **EXT-CATEGORY-MATCH** — Extended type/impl must have the same
//!   component category as the extender
//! - **EXT-FEATURE-COMPAT** — Features in extending type must be
//!   compatible with features in the base type
//! - **EXT-NO-SELF** — A component type/impl cannot extend itself
//! - **EXT-IMPL-TYPE-MATCH** — A component implementation's category
//!   must match its type's category

use spar_hir_def::item_tree::ItemTree;

use crate::{AnalysisDiagnostic, Severity};

/// The analysis name used in all diagnostics produced by this module.
const ANALYSIS_NAME: &str = "extends_rules";

/// Check all extends/inheritance rules on an ItemTree. Returns diagnostics.
pub fn check_extends_rules(tree: &ItemTree) -> Vec<AnalysisDiagnostic> {
    // Severity rationale (STPA-REQ-016):
    //   Error — self-extension cycle, category mismatch in extends, incompatible feature
    //           kind refinement, implementation/type category mismatch
    let mut diags = Vec::new();

    for (_idx, pkg) in tree.packages.iter() {
        let pkg_name = pkg.name.as_str().to_string();

        for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
            match item_ref {
                spar_hir_def::item_tree::ItemRef::ComponentType(ct_idx) => {
                    let ct = &tree.component_types[*ct_idx];
                    check_type_extends(tree, ct, &pkg_name, &mut diags);
                }
                spar_hir_def::item_tree::ItemRef::ComponentImpl(ci_idx) => {
                    let ci = &tree.component_impls[*ci_idx];
                    check_impl_extends(tree, ci, &pkg_name, &mut diags);
                    check_impl_type_category_match(tree, ci, &pkg_name, &mut diags);
                }
                _ => {}
            }
        }
    }

    diags
}

/// EXT-NO-SELF + EXT-CATEGORY-MATCH + EXT-FEATURE-COMPAT for component types.
fn check_type_extends(
    tree: &ItemTree,
    ct: &spar_hir_def::item_tree::ComponentTypeItem,
    pkg_name: &str,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let extends = match &ct.extends {
        Some(e) => e,
        None => return,
    };

    let type_name = ct.name.as_str();
    let path = vec![pkg_name.to_string(), type_name.to_string()];

    // EXT-NO-SELF: Check for self-extension
    let ext_type = extends.type_name.as_str();
    if ext_type.eq_ignore_ascii_case(type_name) {
        // Also check package: if no package qualifier, it's in the same package
        let same_package = extends.package.is_none()
            || extends
                .package
                .as_ref()
                .is_some_and(|p| p.as_str().eq_ignore_ascii_case(pkg_name));

        if same_package {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "component type '{}' extends itself (direct cycle)",
                    type_name
                ),
                path: path.clone(),
                analysis: ANALYSIS_NAME.to_string(),
            });
        }
    }

    // EXT-CATEGORY-MATCH: Find the extended type and check category
    let base_ct = find_component_type(tree, extends);
    if let Some(base) = base_ct {
        if base.category != ct.category {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "component type '{}' ({}) extends '{}' ({}) \
                     — categories must match",
                    type_name, ct.category, extends, base.category
                ),
                path: path.clone(),
                analysis: ANALYSIS_NAME.to_string(),
            });
        }

        // EXT-FEATURE-COMPAT: Check that extending type's features are
        // compatible with base type's features (refined features must
        // have the same kind)
        check_feature_compatibility(tree, ct, base, &path, diags);
    }
}

/// EXT-NO-SELF + EXT-CATEGORY-MATCH for component implementations.
fn check_impl_extends(
    tree: &ItemTree,
    ci: &spar_hir_def::item_tree::ComponentImplItem,
    pkg_name: &str,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let extends = match &ci.extends {
        Some(e) => e,
        None => return,
    };

    let qualified = format!("{}.{}", ci.type_name, ci.impl_name);
    let path = vec![pkg_name.to_string(), qualified.clone()];

    // EXT-NO-SELF: Check for self-extension
    let ext_type = extends.type_name.as_str();
    let ext_impl = extends.impl_name.as_ref().map(|n| n.as_str());

    let same_type = ext_type.eq_ignore_ascii_case(ci.type_name.as_str());
    let same_impl = match ext_impl {
        Some(ei) => ei.eq_ignore_ascii_case(ci.impl_name.as_str()),
        None => false,
    };
    let same_package = extends.package.is_none()
        || extends
            .package
            .as_ref()
            .is_some_and(|p| p.as_str().eq_ignore_ascii_case(pkg_name));

    if same_type && same_impl && same_package {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "component implementation '{}' extends itself (direct cycle)",
                qualified
            ),
            path: path.clone(),
            analysis: ANALYSIS_NAME.to_string(),
        });
    }

    // EXT-CATEGORY-MATCH: Find the extended implementation and check category
    let base_ci = find_component_impl(tree, extends);
    if let Some(base) = base_ci
        && base.category != ci.category
    {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "component implementation '{}' ({}) extends '{}' ({}) \
                 — categories must match",
                qualified, ci.category, extends, base.category
            ),
            path,
            analysis: ANALYSIS_NAME.to_string(),
        });
    }
}

/// EXT-IMPL-TYPE-MATCH: Check that a component implementation's category
/// matches the category of the type it implements.
fn check_impl_type_category_match(
    tree: &ItemTree,
    ci: &spar_hir_def::item_tree::ComponentImplItem,
    pkg_name: &str,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let qualified = format!("{}.{}", ci.type_name, ci.impl_name);
    let path = vec![pkg_name.to_string(), qualified.clone()];

    // Find the type this implementation claims to implement
    for (_idx, base_ct) in tree.component_types.iter() {
        if base_ct
            .name
            .as_str()
            .eq_ignore_ascii_case(ci.type_name.as_str())
        {
            if base_ct.category != ci.category {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "component implementation '{}' has category {} but its type '{}' \
                         has category {} — they must match",
                        qualified, ci.category, base_ct.name, base_ct.category
                    ),
                    path,
                    analysis: ANALYSIS_NAME.to_string(),
                });
            }
            return;
        }
    }
}

/// EXT-FEATURE-COMPAT: Check that features in the extending type are
/// compatible with features in the base type.
fn check_feature_compatibility(
    tree: &ItemTree,
    extending: &spar_hir_def::item_tree::ComponentTypeItem,
    base: &spar_hir_def::item_tree::ComponentTypeItem,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // For each feature in the extending type that shares a name with a
    // base feature, check that the feature kind matches.
    for &ext_feat_idx in &extending.features {
        let ext_feat = &tree.features[ext_feat_idx];

        for &base_feat_idx in &base.features {
            let base_feat = &tree.features[base_feat_idx];

            if ext_feat
                .name
                .as_str()
                .eq_ignore_ascii_case(base_feat.name.as_str())
            {
                // Feature kinds must match (refinement constraint)
                if ext_feat.kind != base_feat.kind {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "feature '{}' in extending type '{}' is {} but base type '{}' \
                             declares it as {} — refined features must preserve kind",
                            ext_feat.name, extending.name, ext_feat.kind, base.name, base_feat.kind
                        ),
                        path: path.to_vec(),
                        analysis: ANALYSIS_NAME.to_string(),
                    });
                }
            }
        }
    }
}

/// Find a component type in the ItemTree by classifier reference.
fn find_component_type<'a>(
    tree: &'a ItemTree,
    cref: &spar_hir_def::name::ClassifierRef,
) -> Option<&'a spar_hir_def::item_tree::ComponentTypeItem> {
    let target_name = cref.type_name.as_str();
    for (_idx, ct) in tree.component_types.iter() {
        if ct.name.as_str().eq_ignore_ascii_case(target_name) {
            return Some(ct);
        }
    }
    None
}

/// Find a component implementation in the ItemTree by classifier reference.
fn find_component_impl<'a>(
    tree: &'a ItemTree,
    cref: &spar_hir_def::name::ClassifierRef,
) -> Option<&'a spar_hir_def::item_tree::ComponentImplItem> {
    let target_type = cref.type_name.as_str();
    let target_impl = cref.impl_name.as_ref().map(|n| n.as_str());
    for (_idx, ci) in tree.component_impls.iter() {
        let type_match = ci.type_name.as_str().eq_ignore_ascii_case(target_type);
        let impl_match = match target_impl {
            Some(ti) => ci.impl_name.as_str().eq_ignore_ascii_case(ti),
            None => true,
        };
        if type_match && impl_match {
            return Some(ci);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::{ClassifierRef, Name};

    /// Helper: build a tree with two component types for extends testing.
    fn tree_with_extending_type(
        base_name: &str,
        base_category: ComponentCategory,
        ext_name: &str,
        ext_category: ComponentCategory,
    ) -> ItemTree {
        let mut tree = ItemTree::default();

        let base_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new(base_name),
            category: base_category,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ext_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new(ext_name),
            category: ext_category,
            extends: Some(ClassifierRef::type_only(Name::new(base_name))),
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(base_ct),
                ItemRef::ComponentType(ext_ct),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree
    }

    // ── EXT-CATEGORY-MATCH tests ────────────────────────────────────

    #[test]
    fn matching_categories_no_error() {
        let tree = tree_with_extending_type(
            "Base",
            ComponentCategory::System,
            "Extended",
            ComponentCategory::System,
        );
        let diags = check_extends_rules(&tree);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("categories must match"))
            .collect();
        assert!(
            cat_errs.is_empty(),
            "matching categories should not error: {:?}",
            cat_errs
        );
    }

    #[test]
    fn mismatched_categories_error() {
        let tree = tree_with_extending_type(
            "Base",
            ComponentCategory::System,
            "Extended",
            ComponentCategory::Process,
        );
        let diags = check_extends_rules(&tree);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("categories must match")
            })
            .collect();
        assert_eq!(
            cat_errs.len(),
            1,
            "mismatched categories should error: {:?}",
            diags
        );
    }

    #[test]
    fn extends_abstract_to_abstract_no_error() {
        let tree = tree_with_extending_type(
            "AbstractBase",
            ComponentCategory::Abstract,
            "AbstractExt",
            ComponentCategory::Abstract,
        );
        let diags = check_extends_rules(&tree);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("categories must match"))
            .collect();
        assert!(cat_errs.is_empty(), "abstract->abstract ok: {:?}", cat_errs);
    }

    // ── EXT-FEATURE-COMPAT tests ────────────────────────────────────

    #[test]
    fn compatible_features_no_error() {
        let mut tree = ItemTree::default();

        let base_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ext_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::InOut),
            access_kind: None,
            classifier: None,
            is_refined: true,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let base_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Base"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![base_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ext_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Ext"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("Base"))),
            features: vec![ext_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(base_ct),
                ItemRef::ComponentType(ext_ct),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let feat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("refined features must preserve kind"))
            .collect();
        assert!(
            feat_errs.is_empty(),
            "compatible features should not error: {:?}",
            feat_errs
        );
    }

    #[test]
    fn incompatible_feature_kind_error() {
        let mut tree = ItemTree::default();

        let base_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ext_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::EventPort, // incompatible!
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: true,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let base_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Base"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![base_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ext_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Ext"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("Base"))),
            features: vec![ext_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(base_ct),
                ItemRef::ComponentType(ext_ct),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let feat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("refined features must preserve kind"))
            .collect();
        assert_eq!(
            feat_errs.len(),
            1,
            "incompatible feature kind should error: {:?}",
            diags
        );
    }

    #[test]
    fn new_feature_in_extending_no_error() {
        let mut tree = ItemTree::default();

        let base_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ext_f = tree.features.alloc(Feature {
            name: Name::new("port_b"), // new feature, not in base
            kind: FeatureKind::EventPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let base_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Base"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![base_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ext_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Ext"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("Base"))),
            features: vec![ext_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(base_ct),
                ItemRef::ComponentType(ext_ct),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let feat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("refined features"))
            .collect();
        assert!(
            feat_errs.is_empty(),
            "new features should not cause errors: {:?}",
            feat_errs
        );
    }

    // ── EXT-NO-SELF tests ───────────────────────────────────────────

    #[test]
    fn self_extending_type_error() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("SelfRef"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("SelfRef"))),
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert_eq!(
            self_errs.len(),
            1,
            "self-extension should produce error: {:?}",
            diags
        );
    }

    #[test]
    fn self_extending_impl_error() {
        let mut tree = ItemTree::default();

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Top"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::implementation(
                None,
                Name::new("Top"),
                Name::new("impl"),
            )),
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentImpl(ci_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert_eq!(
            self_errs.len(),
            1,
            "self-extending impl should produce error: {:?}",
            diags
        );
    }

    #[test]
    fn different_package_self_ref_no_error() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("SelfRef"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::qualified(
                Name::new("OtherPkg"),
                Name::new("SelfRef"),
            )),
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert!(
            self_errs.is_empty(),
            "different package should not be self-ref: {:?}",
            self_errs
        );
    }

    // ── EXT-IMPL-TYPE-MATCH tests ───────────────────────────────────

    #[test]
    fn impl_matches_type_category_no_error() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyType"),
            category: ComponentCategory::System,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("MyType"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(ct_idx),
                ItemRef::ComponentImpl(ci_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("they must match"))
            .collect();
        assert!(
            cat_errs.is_empty(),
            "matching categories should not error: {:?}",
            cat_errs
        );
    }

    #[test]
    fn impl_mismatches_type_category_error() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyType"),
            category: ComponentCategory::System,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("MyType"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::Process, // wrong!
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(ct_idx),
                ItemRef::ComponentImpl(ci_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("they must match"))
            .collect();
        assert_eq!(
            cat_errs.len(),
            1,
            "mismatched impl/type categories should error: {:?}",
            diags
        );
    }

    #[test]
    fn no_extends_no_diagnostics() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Simple"),
            category: ComponentCategory::System,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        assert!(diags.is_empty(), "no extends = no diagnostics: {:?}", diags);
    }

    // ── EXT-NO-SELF impl: different impl name, same type → no error ──

    #[test]
    fn impl_extends_different_impl_name_no_self_ref() {
        let mut tree = ItemTree::default();

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Top"),
            impl_name: Name::new("impl1"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::implementation(
                None,
                Name::new("Top"),
                Name::new("impl2"),
            )),
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentImpl(ci_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert!(
            self_errs.is_empty(),
            "different impl name should not be self-ref: {:?}",
            self_errs
        );
    }

    // ── EXT-NO-SELF: impl extends type only (no impl_name) → no self ──

    #[test]
    fn impl_extends_type_only_no_self_ref() {
        let mut tree = ItemTree::default();

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Top"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("Top"))),
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentImpl(ci_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert!(
            self_errs.is_empty(),
            "type-only ref from impl should not be self-ref: {:?}",
            self_errs
        );
    }

    // ── EXT-FEATURE-COMPAT: same kind but different direction → no error ──

    #[test]
    fn feature_same_kind_different_direction_no_error() {
        let mut tree = ItemTree::default();

        let base_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ext_f = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: true,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let base_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Base"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![base_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ext_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Ext"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("Base"))),
            features: vec![ext_f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(base_ct),
                ItemRef::ComponentType(ext_ct),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let feat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("refined features must preserve kind"))
            .collect();
        assert!(
            feat_errs.is_empty(),
            "same kind, different direction should be ok: {:?}",
            feat_errs
        );
    }

    // ── Case-insensitive self-ref ────────────────────────────────────

    #[test]
    fn case_insensitive_self_extension_detected() {
        let mut tree = ItemTree::default();

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyType"),
            category: ComponentCategory::System,
            extends: Some(ClassifierRef::type_only(Name::new("mytype"))),
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let diags = check_extends_rules(&tree);
        let self_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("extends itself"))
            .collect();
        assert_eq!(
            self_errs.len(),
            1,
            "case-insensitive self-extension should be caught: {:?}",
            diags
        );
    }
}
