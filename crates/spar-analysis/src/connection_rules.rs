//! Connection legality rules (AS5506 §9).
//!
//! Validates connection declarations in the instance model:
//! - **CONN-TYPE**: Port connections must connect compatible feature kinds
//!   (data port to data port, event port to event port, etc.)
//! - **CONN-SELF**: A component cannot connect a port to itself
//!   (source and destination must differ)

use spar_hir_def::instance::{
    ComponentInstance, ComponentInstanceIdx, ConnectionInstance, SystemInstance,
};
use spar_hir_def::item_tree::FeatureKind;
use spar_hir_def::name::Name;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Validates connection legality rules on the instance model.
///
/// Checks AS5506 §9 rules:
/// - Feature kind compatibility between connection endpoints
/// - No self-loop connections (same subcomponent and feature)
pub struct ConnectionRuleAnalysis;

impl Analysis for ConnectionRuleAnalysis {
    fn name(&self) -> &str {
        "connection_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            for &conn_idx in &comp.connections {
                let conn = &instance.connections[conn_idx];
                check_feature_kind_compatibility(instance, conn, comp_idx, comp, &mut diags);
                check_connection_self_loop(conn, &mut diags, instance, comp_idx);
            }
        }

        diags
    }
}

/// CONN-TYPE: Check that source and destination feature kinds are compatible.
///
/// Port connections must connect compatible feature kinds:
/// - DataPort ↔ DataPort
/// - EventPort ↔ EventPort
/// - EventDataPort ↔ EventDataPort
/// - DataAccess ↔ DataAccess
/// - BusAccess ↔ BusAccess
/// - SubprogramAccess ↔ SubprogramAccess
/// - SubprogramGroupAccess ↔ SubprogramGroupAccess
/// - FeatureGroup ↔ FeatureGroup
/// - AbstractFeature is compatible with any feature kind
fn check_feature_kind_compatibility(
    instance: &SystemInstance,
    conn: &ConnectionInstance,
    owner_idx: ComponentInstanceIdx,
    _owner: &ComponentInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let (src_end, dst_end) = match (&conn.src, &conn.dst) {
        (Some(s), Some(d)) => (s, d),
        _ => return, // incomplete connection, skip
    };

    let src_kind = find_feature_kind(instance, owner_idx, &src_end.subcomponent, &src_end.feature);
    let dst_kind = find_feature_kind(instance, owner_idx, &dst_end.subcomponent, &dst_end.feature);

    let (src_kind, dst_kind) = match (src_kind, dst_kind) {
        (Some(s), Some(d)) => (s, d),
        _ => return, // can't resolve, skip
    };

    // AbstractFeature is compatible with anything
    if src_kind == FeatureKind::AbstractFeature || dst_kind == FeatureKind::AbstractFeature {
        return;
    }

    if !are_feature_kinds_compatible(src_kind, dst_kind) {
        let path = component_path(instance, owner_idx);
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "connection '{}': source feature '{}' is {} but destination \
                 feature '{}' is {} — feature kinds must match",
                conn.name, src_end.feature, src_kind, dst_end.feature, dst_kind
            ),
            path,
            analysis: "connection_rules".to_string(),
        });
    }
}

/// CONN-SELF: Check that a connection does not loop back to the same
/// subcomponent and feature.
fn check_connection_self_loop(
    conn: &ConnectionInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
    instance: &SystemInstance,
    owner_idx: ComponentInstanceIdx,
) {
    let (src_end, dst_end) = match (&conn.src, &conn.dst) {
        (Some(s), Some(d)) => (s, d),
        _ => return,
    };

    // Both endpoints must reference the same subcomponent (or both be on the
    // enclosing component) AND the same feature name.
    let same_subcomponent = match (&src_end.subcomponent, &dst_end.subcomponent) {
        (Some(s), Some(d)) => s.eq_ci(d),
        (None, None) => true,
        _ => false,
    };

    if same_subcomponent && src_end.feature.eq_ci(&dst_end.feature) {
        let path = component_path(instance, owner_idx);
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "connection '{}': source and destination are the same \
                 (self-loop on feature '{}')",
                conn.name, src_end.feature
            ),
            path,
            analysis: "connection_rules".to_string(),
        });
    }
}

/// Check if two feature kinds are compatible for connection.
fn are_feature_kinds_compatible(src: FeatureKind, dst: FeatureKind) -> bool {
    src == dst
}

/// Find the kind of a feature in the instance model.
///
/// If `subcomponent` is Some, look at that subcomponent's features.
/// If None, look at the owning component's own features.
fn find_feature_kind(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    subcomponent: &Option<Name>,
    feature_name: &Name,
) -> Option<FeatureKind> {
    let comp = if let Some(sub_name) = subcomponent {
        let owner_comp = instance.component(owner);
        owner_comp
            .children
            .iter()
            .find(|&&child_idx| instance.component(child_idx).name.eq_ci(sub_name))
            .copied()?
    } else {
        owner
    };

    let comp_inst = instance.component(comp);
    for &feat_idx in &comp_inst.features {
        let feat = &instance.features[feat_idx];
        if feat.name.eq_ci(feature_name) {
            return Some(feat.kind);
        }
    }
    None
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
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
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
                array_index: None,
            })
        }

        fn add_feature(
            &mut self,
            name: &str,
            kind: FeatureKind,
            direction: Option<Direction>,
            owner: ComponentInstanceIdx,
        ) -> FeatureInstanceIdx {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction,
                owner,
                array_index: None,
            });
            self.components[owner].features.push(idx);
            idx
        }

        fn add_connection(
            &mut self,
            name: &str,
            kind: ConnectionKind,
            owner: ComponentInstanceIdx,
            src: ConnectionEnd,
            dst: ConnectionEnd,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind,
                is_bidirectional: false,
                owner,
                src: Some(src),
                dst: Some(dst),
            });
            self.components[owner].connections.push(idx);
            idx
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
                diagnostics: Vec::new(),
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    fn end(sub: Option<&str>, feat: &str) -> ConnectionEnd {
        ConnectionEnd {
            subcomponent: sub.map(Name::new),
            feature: Name::new(feat),
        }
    }

    // ── CONN-TYPE tests ─────────────────────────────────────────────

    #[test]
    fn valid_connection_matching_feature_kinds() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "matching feature kinds should produce no errors: {:?}",
            errors
        );
    }

    #[test]
    fn mismatched_feature_kinds_data_to_event() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Mismatch: data port -> event port
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::EventPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("feature kinds must match"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "data port to event port should produce an error: {:?}",
            diags
        );
        assert!(errors[0].message.contains("data port"));
        assert!(errors[0].message.contains("event port"));
    }

    #[test]
    fn abstract_feature_compatible_with_any() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::AbstractFeature, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "abstract feature should be compatible with any kind: {:?}",
            errors
        );
    }

    #[test]
    fn event_data_port_to_event_port_mismatch() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::EventDataPort, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::EventPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "event data port to event port should error: {:?}",
            diags
        );
    }

    // ── CONN-SELF tests ─────────────────────────────────────────────

    #[test]
    fn self_loop_connection_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        b.add_feature("port1", FeatureKind::DataPort, Some(Direction::InOut), a);
        // Self-loop: same subcomponent, same feature
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "port1"),
            end(Some("a"), "port1"),
        );
        b.set_children(root, vec![a]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("self-loop"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "self-loop should produce an error: {:?}",
            diags
        );
    }

    #[test]
    fn same_subcomponent_different_features_no_self_loop() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), a);
        // Same subcomponent, different features -- not a self-loop
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("a"), "in1"),
        );
        b.set_children(root, vec![a]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let self_loop_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("self-loop"))
            .collect();
        assert!(
            self_loop_errors.is_empty(),
            "different features on same subcomponent should not be a self-loop: {:?}",
            self_loop_errors
        );
    }

    #[test]
    fn different_subcomponents_same_feature_name_no_self_loop() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("port1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("port1", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "port1"),
            end(Some("b"), "port1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        let self_loop_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("self-loop"))
            .collect();
        assert!(
            self_loop_errors.is_empty(),
            "different subcomponents should not be a self-loop: {:?}",
            self_loop_errors
        );
    }

    // ── Incomplete connection skipped ───────────────────────────────

    #[test]
    fn incomplete_connection_skipped() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Connection with no endpoints
        let idx = b.connections.alloc(ConnectionInstance {
            name: Name::new("c_incomplete"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: None,
            dst: None,
        });
        b.components[root].connections.push(idx);

        let inst = b.build(root);
        let diags = ConnectionRuleAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "incomplete connections should be skipped: {:?}",
            diags
        );
    }
}
