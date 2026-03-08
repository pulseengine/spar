//! Mode consistency analysis (AS5506 §12).
//!
//! Validates mode declarations and transitions:
//! - Components with modes have exactly one initial mode
//! - Mode transition source and destination reference declared modes
//! - Mode transition triggers reference declared features (ports)
//! - Components with mode transitions but no modes are flagged

use spar_hir_def::instance::SystemInstance;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Validates mode declarations and mode transitions in the instance model.
///
/// Checks:
/// - Exactly one initial mode per component that declares modes
/// - Mode transition source/destination reference declared modes
/// - Mode transition triggers reference component features
/// - Components with transitions but no modes are warned
pub struct ModeCheckAnalysis;

impl Analysis for ModeCheckAnalysis {
    fn name(&self) -> &str {
        "mode_check"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let modes = instance.modes_for(comp_idx);
            let transitions = instance.mode_transitions_for(comp_idx);
            let path = component_path(instance, comp_idx);

            let has_modes = !modes.is_empty();
            let has_transitions = !transitions.is_empty();

            // Check: transitions without modes
            if has_transitions && !has_modes {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "component '{}' has {} mode transition(s) but no modes declared",
                        comp.name,
                        transitions.len()
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Initial mode validation: exactly one initial mode required
            if has_modes {
                let initial_count = modes.iter().filter(|m| m.is_initial).count();
                if initial_count == 0 {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "component '{}' has {} mode(s) but no initial mode \
                             (exactly one mode must be declared initial)",
                            comp.name,
                            modes.len()
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                } else if initial_count > 1 {
                    let initial_names: Vec<&str> =
                        modes.iter().filter(|m| m.is_initial).map(|m| m.name.as_str()).collect();
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "component '{}' has {} initial modes ({}) \
                             but exactly one is required",
                            comp.name,
                            initial_count,
                            initial_names.join(", ")
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Collect declared mode names for transition validation
            let mode_names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();

            // Mode transition endpoint validation
            for mt in &transitions {
                let mt_label = mt
                    .name
                    .as_ref()
                    .map(|n| n.as_str().to_string())
                    .unwrap_or_else(|| {
                        format!("{}-[]->{}",
                            mt.source.as_str(),
                            mt.destination.as_str())
                    });

                // Source must reference a declared mode
                if has_modes
                    && !mode_names.iter().any(|n| n.eq_ignore_ascii_case(mt.source.as_str()))
                {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "mode transition '{}' in '{}': source mode '{}' \
                             is not declared on this component",
                            mt_label, comp.name, mt.source
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }

                // Destination must reference a declared mode
                if has_modes
                    && !mode_names.iter().any(|n| n.eq_ignore_ascii_case(mt.destination.as_str()))
                {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "mode transition '{}' in '{}': destination mode '{}' \
                             is not declared on this component",
                            mt_label, comp.name, mt.destination
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }

                // Trigger validation: each trigger should name a feature on this component
                let feature_names: Vec<String> = comp
                    .features
                    .iter()
                    .map(|&fi| instance.features[fi].name.as_str().to_string())
                    .collect();

                for trigger in &mt.triggers {
                    if !feature_names
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case(trigger.as_str()))
                    {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "mode transition '{}' in '{}': trigger '{}' \
                                 does not match any feature on this component",
                                mt_label, comp.name, trigger
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
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
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::Name;

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        mode_instances: Arena<ModeInstance>,
        mode_transition_instances: Arena<ModeTransitionInstance>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                mode_instances: Arena::default(),
                mode_transition_instances: Arena::default(),
            }
        }

        fn add_component(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(name),
                impl_name: Some(Name::new("impl")),
                package: Name::new("Pkg"),
                parent,
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
            })
        }

        fn add_feature(
            &mut self,
            name: &str,
            kind: FeatureKind,
            dir: Direction,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction: Some(dir),
                owner,
            });
            self.components[owner].features.push(idx);
        }

        fn add_mode(
            &mut self,
            name: &str,
            is_initial: bool,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.mode_instances.alloc(ModeInstance {
                name: Name::new(name),
                is_initial,
                owner,
            });
            self.components[owner].modes.push(idx);
        }

        fn add_mode_transition(
            &mut self,
            name: Option<&str>,
            source: &str,
            destination: &str,
            triggers: Vec<&str>,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.mode_transition_instances.alloc(ModeTransitionInstance {
                name: name.map(Name::new),
                source: Name::new(source),
                destination: Name::new(destination),
                triggers: triggers.into_iter().map(Name::new).collect(),
                owner,
            });
            self.components[owner].mode_transitions.push(idx);
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
                mode_instances: self.mode_instances,
                mode_transition_instances: self.mode_transition_instances,
                diagnostics: Vec::new(),
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
            }
        }
    }

    // ── Initial mode validation ─────────────────────────────────────

    #[test]
    fn valid_component_one_initial_mode() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "valid modes should produce no errors: {:?}", errors);
    }

    #[test]
    fn component_with_two_initial_modes_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", true, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("2 initial modes"))
            .collect();
        assert_eq!(errors.len(), 1, "should error on two initial modes: {:?}", diags);
    }

    #[test]
    fn component_with_modes_but_no_initial_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", false, child);
        b.add_mode("active", false, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("no initial mode"))
            .collect();
        assert_eq!(errors.len(), 1, "should error on zero initial modes: {:?}", diags);
    }

    // ── Mode transition endpoint validation ─────────────────────────

    #[test]
    fn mode_transition_referencing_undeclared_mode_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        // Transition references "running" which does not exist
        b.add_mode_transition(Some("t1"), "idle", "running", vec![], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("destination mode 'running'"))
            .collect();
        assert_eq!(errors.len(), 1, "should error on undeclared destination mode: {:?}", diags);
    }

    #[test]
    fn mode_transition_undeclared_source_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        // Source "nonexistent" is not declared
        b.add_mode_transition(Some("t_bad"), "nonexistent", "active", vec![], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("source mode 'nonexistent'"))
            .collect();
        assert_eq!(errors.len(), 1, "should error on undeclared source mode: {:?}", diags);
    }

    #[test]
    fn valid_mode_transition_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.add_mode_transition(Some("activate"), "idle", "active", vec![], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "valid transition should have no errors: {:?}", errors);
    }

    // ── Mode transition trigger validation ──────────────────────────

    #[test]
    fn mode_transition_trigger_matches_feature_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("start_cmd", FeatureKind::EventPort, Direction::In, child);
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["start_cmd"], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("trigger"))
            .collect();
        assert!(warnings.is_empty(), "matching trigger should produce no warning: {:?}", warnings);
    }

    #[test]
    fn mode_transition_trigger_no_matching_feature_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("data_in", FeatureKind::DataPort, Direction::In, child);
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        // Trigger "go" does not match any feature
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["go"], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("trigger 'go'"))
            .collect();
        assert_eq!(warnings.len(), 1, "unmatched trigger should warn: {:?}", diags);
    }

    // ── Transitions without modes ───────────────────────────────────

    #[test]
    fn transitions_without_modes_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        // Add a transition but no modes
        b.add_mode_transition(Some("t1"), "a", "b", vec![], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("no modes declared"))
            .collect();
        assert_eq!(warnings.len(), 1, "transitions without modes should warn: {:?}", diags);
    }

    // ── No modes at all: clean ──────────────────────────────────────

    #[test]
    fn component_without_modes_no_diagnostics() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("simple", ComponentCategory::System, Some(root));
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "no modes = no diagnostics: {:?}", diags);
    }

    // ── Unnamed transition label ────────────────────────────────────

    #[test]
    fn unnamed_transition_with_undeclared_source() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        // Unnamed transition (name=None), source "missing" not declared
        b.add_mode_transition(None, "missing", "idle", vec![], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("source mode 'missing'"))
            .collect();
        assert_eq!(errors.len(), 1, "unnamed transition should still report errors: {:?}", diags);
        // The label should use the fallback format
        assert!(
            errors[0].message.contains("missing-[]->idle"),
            "fallback label expected: {:?}",
            errors[0].message
        );
    }
}
