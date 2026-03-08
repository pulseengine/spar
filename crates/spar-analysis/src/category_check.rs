//! Category restriction analysis (AS5506 §5-6).
//!
//! Validates that features and subcomponents respect the containment
//! rules for their parent component category. Uses the restriction
//! tables from `spar_hir_def::category_rules`.

use spar_hir_def::item_tree::ItemTree;

use crate::{AnalysisDiagnostic, Severity};

/// Check all category restriction rules on an ItemTree.
///
/// This is a declarative-model analysis that runs before instantiation.
/// It validates:
/// - Every feature in a component type is allowed for that category
/// - Every subcomponent in an implementation has an allowed category
pub fn check_category_rules(tree: &ItemTree) -> Vec<AnalysisDiagnostic> {
    let mut diags = Vec::new();

    // Check component types: features must be allowed
    for (_idx, ct) in tree.component_types.iter() {
        for &feat_idx in &ct.features {
            let feat = &tree.features[feat_idx];
            if !spar_hir_def::category_rules::is_feature_allowed(ct.category, feat.kind) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "{} component type '{}' cannot have {} feature '{}'",
                        ct.category, ct.name, feat.kind, feat.name
                    ),
                    path: vec![ct.name.as_str().to_string()],
                    analysis: "category_check".to_string(),
                });
            }
        }
    }

    // Check component implementations: subcomponent categories must be allowed
    for (_idx, ci) in tree.component_impls.iter() {
        for &sub_idx in &ci.subcomponents {
            let sub = &tree.subcomponents[sub_idx];
            if !spar_hir_def::category_rules::is_subcomponent_allowed(ci.category, sub.category) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "{} implementation '{}.{}' cannot contain {} subcomponent '{}'",
                        ci.category, ci.type_name, ci.impl_name, sub.category, sub.name
                    ),
                    path: vec![
                        format!("{}.{}", ci.type_name, ci.impl_name),
                    ],
                    analysis: "category_check".to_string(),
                });
            }
        }
    }

    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::Name;

    fn make_tree_with_feature(
        category: ComponentCategory,
        feat_kind: FeatureKind,
    ) -> ItemTree {
        let mut tree = ItemTree::default();
        let feat_idx = tree.features.alloc(Feature {
            name: Name::new("test_feat"),
            kind: feat_kind,
            direction: None,
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("TestType"),
            category,
            extends: None,
            features: vec![feat_idx],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });
        tree
    }

    fn make_tree_with_subcomponent(
        parent_category: ComponentCategory,
        child_category: ComponentCategory,
    ) -> ItemTree {
        let mut tree = ItemTree::default();
        let sub_idx = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("test_sub"),
            category: child_category,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });
        tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("TestType"),
            impl_name: Name::new("impl"),
            category: parent_category,
            extends: None,
            subcomponents: vec![sub_idx],
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
        tree
    }

    #[test]
    fn system_allows_data_port() {
        let tree = make_tree_with_feature(ComponentCategory::System, FeatureKind::DataPort);
        let diags = check_category_rules(&tree);
        assert!(diags.is_empty(), "system allows data port: {:?}", diags);
    }

    #[test]
    fn data_disallows_bus_access() {
        let tree = make_tree_with_feature(ComponentCategory::Data, FeatureKind::BusAccess);
        let diags = check_category_rules(&tree);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "data cannot have bus access: {:?}", diags);
        assert!(errors[0].message.contains("bus access"));
    }

    #[test]
    fn thread_allows_parameter() {
        let tree = make_tree_with_feature(ComponentCategory::Thread, FeatureKind::Parameter);
        let diags = check_category_rules(&tree);
        assert!(diags.is_empty(), "thread allows parameter: {:?}", diags);
    }

    #[test]
    fn system_disallows_parameter() {
        let tree = make_tree_with_feature(ComponentCategory::System, FeatureKind::Parameter);
        let diags = check_category_rules(&tree);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "system cannot have parameter: {:?}", diags);
    }

    #[test]
    fn abstract_allows_everything() {
        for kind in [
            FeatureKind::DataPort, FeatureKind::EventPort, FeatureKind::EventDataPort,
            FeatureKind::Parameter, FeatureKind::DataAccess, FeatureKind::BusAccess,
            FeatureKind::SubprogramAccess, FeatureKind::SubprogramGroupAccess,
            FeatureKind::FeatureGroup, FeatureKind::AbstractFeature,
        ] {
            let tree = make_tree_with_feature(ComponentCategory::Abstract, kind);
            let diags = check_category_rules(&tree);
            assert!(diags.is_empty(), "abstract allows {:?}: {:?}", kind, diags);
        }
    }

    #[test]
    fn system_allows_process_subcomponent() {
        let tree = make_tree_with_subcomponent(ComponentCategory::System, ComponentCategory::Process);
        let diags = check_category_rules(&tree);
        assert!(diags.is_empty(), "system allows process: {:?}", diags);
    }

    #[test]
    fn system_disallows_thread_subcomponent() {
        let tree = make_tree_with_subcomponent(ComponentCategory::System, ComponentCategory::Thread);
        let diags = check_category_rules(&tree);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "system cannot contain thread: {:?}", diags);
    }

    #[test]
    fn process_allows_thread_subcomponent() {
        let tree = make_tree_with_subcomponent(ComponentCategory::Process, ComponentCategory::Thread);
        let diags = check_category_rules(&tree);
        assert!(diags.is_empty(), "process allows thread: {:?}", diags);
    }

    #[test]
    fn bus_disallows_thread_subcomponent() {
        let tree = make_tree_with_subcomponent(ComponentCategory::Bus, ComponentCategory::Thread);
        let diags = check_category_rules(&tree);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "bus cannot contain thread: {:?}", diags);
    }
}
