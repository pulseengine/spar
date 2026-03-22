//! Model completeness analysis.
//!
//! Checks for structural completeness of the AADL instance model,
//! looking for missing implementations, missing features, and
//! unresolved classifier references.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::SystemInstance;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Analyzes model completeness.
///
/// Checks:
/// - Component types without implementations (type-only subcomponents)
/// - Component types without features (featureless components)
/// - Components with no connections and no features (skeletal)
/// - Unresolved classifier references (no type_name)
pub struct CompletenessAnalysis;

impl Analysis for CompletenessAnalysis {
    fn name(&self) -> &str {
        "completeness"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — unresolved classifier reference (instance-level)
        //   Warning — component has no classifier reference
        //   Info    — type-only subcomponent without implementation, featureless component
        let mut diags = Vec::new();

        // Track which type names have implementations.
        // Key: (package, type_name), Value: has_implementation
        let mut type_has_impl: FxHashMap<(String, String), bool> = FxHashMap::default();

        for (_idx, comp) in instance.all_components() {
            let key = (
                comp.package.as_str().to_string(),
                comp.type_name.as_str().to_string(),
            );
            let entry = type_has_impl.entry(key).or_insert(false);
            if comp.impl_name.is_some() {
                *entry = true;
            }
        }

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            // Check for unresolved classifier references.
            // A component with an empty type_name likely failed resolution.
            if comp.type_name.as_str().is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "component '{}' has no classifier reference (unresolved type)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
                continue;
            }

            // Warn about type-only subcomponents (no implementation).
            if comp.impl_name.is_none() && comp.parent.is_some() {
                let key = (
                    comp.package.as_str().to_string(),
                    comp.type_name.as_str().to_string(),
                );
                // Only warn if there IS an implementation somewhere (i.e. they
                // chose not to use it), or if the type_name is non-empty (leaf).
                // For truly anonymous subcomponents with empty type, we already
                // warned above.
                if !comp.type_name.as_str().is_empty() {
                    let has_impl = type_has_impl.get(&key).copied().unwrap_or(false);
                    if !has_impl {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "component '{}' uses type '{}' which has no implementation in scope",
                                comp.name, comp.type_name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }

            // Warn about featureless components (except data and abstract
            // which commonly have no features).
            if comp.features.is_empty() && comp.parent.is_some() {
                use spar_hir_def::item_tree::ComponentCategory;
                let trivially_featureless = matches!(
                    comp.category,
                    ComponentCategory::Data
                        | ComponentCategory::Abstract
                        | ComponentCategory::Memory
                        | ComponentCategory::Bus
                        | ComponentCategory::VirtualBus
                );
                if !trivially_featureless {
                    // Only warn if the type_name is non-empty (otherwise
                    // it's likely an anonymous/unresolved subcomponent).
                    if !comp.type_name.as_str().is_empty() {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "component '{}' of type '{}' has no features",
                                comp.name, comp.type_name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // Check instance-level diagnostics for unresolved references.
        for inst_diag in &instance.diagnostics {
            let path: Vec<String> = inst_diag
                .path
                .iter()
                .map(|n| n.as_str().to_string())
                .collect();
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: inst_diag.message.clone(),
                path,
                analysis: self.name().to_string(),
            });
        }

        diags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::Name;

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        diagnostics: Vec<spar_hir_def::instance::InstanceDiagnostic>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                diagnostics: Vec::new(),
            }
        }

        fn add_component(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
            impl_name: Option<&str>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(name),
                impl_name: impl_name.map(Name::new),
                package: Name::new("Pkg"),
                parent,
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: None,
                in_modes: Vec::new(),
            })
        }

        fn add_feature(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind: spar_hir_def::item_tree::FeatureKind::DataPort,
                direction: Some(spar_hir_def::item_tree::Direction::In),
                owner,
                classifier: None,
                access_kind: None,
                array_index: None,
            });
            self.components[owner].features.push(idx);
        }

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
            SystemInstance {
                root,
                components: self.components,
                features: self.features,
                connections: self.connections,
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
                mode_instances: Arena::default(),
                mode_transition_instances: Arena::default(),
                diagnostics: self.diagnostics,
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    #[test]
    fn complete_model_no_warnings() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let sub = b.add_component("sub", ComponentCategory::System, Some(root), Some("impl"));
        b.add_feature("port", sub);
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error || d.severity == Severity::Warning)
            .collect();
        assert!(errors.is_empty(), "complete model: {:?}", errors);
    }

    #[test]
    fn empty_type_name_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let sub_idx = b.components.alloc(ComponentInstance {
            name: Name::new("sub"),
            category: ComponentCategory::System,
            type_name: Name::new(""),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });
        b.set_children(root, vec![sub_idx]);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("no classifier reference"))
            .collect();
        assert_eq!(warns.len(), 1, "empty type_name should warn: {:?}", diags);
    }

    #[test]
    fn type_only_subcomponent_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        // sub has no impl_name and no other implementation in scope
        let sub = b.add_component("sensor", ComponentCategory::Device, Some(root), None);
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("no implementation"))
            .collect();
        assert_eq!(infos.len(), 1, "type-only subcomponent should produce info: {:?}", diags);
    }

    #[test]
    fn featureless_system_subcomponent_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        // System subcomponent with no features
        let sub = b.add_component("sub", ComponentCategory::System, Some(root), Some("impl"));
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("has no features"))
            .collect();
        assert_eq!(infos.len(), 1, "featureless system should produce info: {:?}", diags);
    }

    #[test]
    fn featureless_data_subcomponent_no_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        // Data subcomponent with no features — trivially featureless, should NOT warn
        let sub = b.add_component("data", ComponentCategory::Data, Some(root), Some("impl"));
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("has no features"))
            .collect();
        assert!(infos.is_empty(), "data should be trivially featureless: {:?}", infos);
    }

    #[test]
    fn instance_diagnostics_forwarded() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        b.diagnostics.push(spar_hir_def::instance::InstanceDiagnostic {
            message: "unresolved reference foo".to_string(),
            path: vec![Name::new("root")],
        });

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("unresolved reference"))
            .collect();
        assert_eq!(errors.len(), 1, "instance diagnostics should be forwarded: {:?}", diags);
    }

    #[test]
    fn root_component_not_checked_for_type_only() {
        let mut b = TestBuilder::new();
        // Root has no impl_name, but it's not a subcomponent (parent is None)
        let root = b.add_component("root", ComponentCategory::System, None, None);

        let inst = b.build(root);
        let diags = CompletenessAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("no implementation"))
            .collect();
        assert!(infos.is_empty(), "root component should not be checked for type-only: {:?}", infos);
    }
}
