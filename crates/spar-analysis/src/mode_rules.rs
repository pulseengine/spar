//! Mode legality rules (AS5506 §12).
//!
//! Validates mode declarations beyond what `mode_check` covers:
//! - **MODE-UNIQUE**: Mode names must be unique within a component
//! - **MODE-TRANS-TRIGGER-KIND**: Mode transition triggers should reference
//!   event ports or event data ports (not data ports or access features)

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::FeatureKind;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Validates mode legality rules on the instance model.
///
/// Checks AS5506 §12 rules:
/// - Mode names must be unique within a component
/// - Mode transition triggers should be event or event data ports
pub struct ModeRuleAnalysis;

impl Analysis for ModeRuleAnalysis {
    fn name(&self) -> &str {
        "mode_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let modes = instance.modes_for(comp_idx);
            let transitions = instance.mode_transitions_for(comp_idx);
            let path = component_path(instance, comp_idx);

            // MODE-UNIQUE: check for duplicate mode names
            check_unique_mode_names(&modes, comp, &path, &mut diags);

            // MODE-TRANS-TRIGGER-KIND: triggers should be event/event data ports
            check_transition_trigger_kinds(
                instance,
                comp_idx,
                &transitions,
                comp,
                &path,
                &mut diags,
            );
        }

        diags
    }
}

/// MODE-UNIQUE: Mode names must be unique within a component.
fn check_unique_mode_names(
    modes: &[&spar_hir_def::instance::ModeInstance],
    comp: &spar_hir_def::instance::ComponentInstance,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for mode in modes {
        let name_lower = mode.name.as_str();
        if seen.iter().any(|s| s.eq_ignore_ascii_case(name_lower)) {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "component '{}': duplicate mode name '{}'",
                    comp.name, mode.name
                ),
                path: path.to_vec(),
                analysis: "mode_rules".to_string(),
            });
        } else {
            seen.push(name_lower);
        }
    }
}

/// MODE-TRANS-TRIGGER-KIND: Mode transition triggers should reference
/// event ports or event data ports, not data ports or other features.
fn check_transition_trigger_kinds(
    instance: &SystemInstance,
    _comp_idx: spar_hir_def::instance::ComponentInstanceIdx,
    transitions: &[&spar_hir_def::instance::ModeTransitionInstance],
    comp: &spar_hir_def::instance::ComponentInstance,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for mt in transitions {
        let mt_label = mt
            .name
            .as_ref()
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| {
                format!(
                    "{}-[]->{}",
                    mt.source.as_str(),
                    mt.destination.as_str()
                )
            });

        for trigger in &mt.triggers {
            // Find the feature matching this trigger name
            let feat_kind = comp.features.iter().find_map(|&fi| {
                let feat = &instance.features[fi];
                if feat.name.as_str().eq_ignore_ascii_case(trigger.as_str()) {
                    Some(feat.kind)
                } else {
                    None
                }
            });

            if let Some(kind) = feat_kind {
                // Trigger must be an event port or event data port
                if !matches!(kind, FeatureKind::EventPort | FeatureKind::EventDataPort) {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "mode transition '{}' in '{}': trigger '{}' is a {} \
                             but should be an event port or event data port",
                            mt_label, comp.name, trigger, kind
                        ),
                        path: path.to_vec(),
                        analysis: "mode_rules".to_string(),
                    });
                }
            }
            // If the trigger doesn't match any feature, mode_check already warns about that.
        }
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
                system_operation_modes: Vec::new(),
            }
        }
    }

    // ── MODE-UNIQUE tests ───────────────────────────────────────────

    #[test]
    fn unique_mode_names_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "unique mode names should produce no errors: {:?}",
            errors
        );
    }

    #[test]
    fn duplicate_mode_names_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, child);
        b.add_mode("idle", false, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("duplicate mode name"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "duplicate mode names should produce an error: {:?}",
            diags
        );
        assert!(errors[0].message.contains("idle"));
    }

    #[test]
    fn duplicate_mode_names_case_insensitive() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("Idle", true, child);
        b.add_mode("idle", false, child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("duplicate mode name"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "case-insensitive duplicate should be caught: {:?}",
            diags
        );
    }

    // ── MODE-TRANS-TRIGGER-KIND tests ───────────────────────────────

    #[test]
    fn trigger_is_event_port_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("start_cmd", FeatureKind::EventPort, Direction::In, child);
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["start_cmd"], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("should be an event port"))
            .collect();
        assert!(
            warnings.is_empty(),
            "event port trigger should produce no warning: {:?}",
            warnings
        );
    }

    #[test]
    fn trigger_is_data_port_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("data_in", FeatureKind::DataPort, Direction::In, child);
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["data_in"], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("data_in")
                    && d.message.contains("should be an event port")
            })
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "data port trigger should produce a warning: {:?}",
            diags
        );
    }

    #[test]
    fn trigger_is_event_data_port_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("cmd", FeatureKind::EventDataPort, Direction::In, child);
        b.add_mode("idle", true, child);
        b.add_mode("active", false, child);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["cmd"], child);
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("should be an event port"))
            .collect();
        assert!(
            warnings.is_empty(),
            "event data port trigger should produce no warning: {:?}",
            warnings
        );
    }

    // ── No modes: clean ────────────────────────────────────────────

    #[test]
    fn component_without_modes_no_diagnostics() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("simple", ComponentCategory::System, Some(root));
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = ModeRuleAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "no modes = no diagnostics: {:?}", diags);
    }
}
