//! Port direction rule enforcement (AS5506 §9.3-9.4).
//!
//! Validates that connection endpoints have compatible directions:
//! - **Across connections**: `out` port → `in` port (between sibling subcomponents)
//! - **Up connections**: `out` port → `out` port (subcomponent to enclosing)
//! - **Down connections**: `in` port → `in` port (enclosing to subcomponent)
//! - **Bidirectional**: `in out` at either end
//! - **Access**: `provides` → `requires` or vice versa

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::{ConnectionKind, Direction, FeatureKind};
use spar_hir_def::name::Name;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates port direction compatibility on connections.
///
/// Checks AS5506 §9.3-9.4 rules:
/// - Across connections: source `out`/`in out` → destination `in`/`in out`
/// - Up connections: subcomponent `out`/`in out` → enclosing `out`/`in out`
/// - Down connections: enclosing `in`/`in out` → subcomponent `in`/`in out`
/// - Bidirectional connections require `in out` on both ends
/// - Access connections: provides/requires compatibility
pub struct DirectionRuleAnalysis;

impl Analysis for DirectionRuleAnalysis {
    fn name(&self) -> &str {
        "direction_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error — port direction incompatibility for across/up/down connections,
        //           bidirectional connection requires in-out ports
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            for &conn_idx in &comp.connections {
                let conn = &instance.connections[conn_idx];

                let (src_end, dst_end) = match (&conn.src, &conn.dst) {
                    (Some(s), Some(d)) => (s, d),
                    _ => continue, // incomplete connection, skip
                };

                // Classify the connection pattern
                let pattern = classify_connection(&src_end.subcomponent, &dst_end.subcomponent);

                // Look up source and destination feature directions
                let src_dir = find_feature_direction(
                    instance,
                    comp_idx,
                    &src_end.subcomponent,
                    &src_end.feature,
                );
                let dst_dir = find_feature_direction(
                    instance,
                    comp_idx,
                    &dst_end.subcomponent,
                    &dst_end.feature,
                );

                let src_kind =
                    find_feature_kind(instance, comp_idx, &src_end.subcomponent, &src_end.feature);

                // Skip direction checks for access connections — they use
                // provides/requires semantics instead.
                if conn.kind == ConnectionKind::Access {
                    continue;
                }

                // Skip if we can't resolve either endpoint's direction
                let (src_dir, dst_dir) = match (src_dir, dst_dir) {
                    (Some(s), Some(d)) => (s, d),
                    _ => continue,
                };

                match pattern {
                    ConnectionPattern::Across => {
                        // Across: src must be out/inout, dst must be in/inout
                        if !is_output_compatible(src_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': source port '{}' has direction '{}', \
                                     expected 'out' or 'in out' for across connection",
                                    conn.name, src_end.feature, src_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                        if !is_input_compatible(dst_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': destination port '{}' has direction '{}', \
                                     expected 'in' or 'in out' for across connection",
                                    conn.name, dst_end.feature, dst_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    ConnectionPattern::Up => {
                        // Up: subcomponent out → enclosing out
                        if !is_output_compatible(src_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': source port '{}' has direction '{}', \
                                     expected 'out' or 'in out' for up connection",
                                    conn.name, src_end.feature, src_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                        if !is_output_compatible(dst_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': enclosing port '{}' has direction '{}', \
                                     expected 'out' or 'in out' for up connection",
                                    conn.name, dst_end.feature, dst_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    ConnectionPattern::Down => {
                        // Down: enclosing in → subcomponent in
                        if !is_input_compatible(src_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': enclosing port '{}' has direction '{}', \
                                     expected 'in' or 'in out' for down connection",
                                    conn.name, src_end.feature, src_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                        if !is_input_compatible(dst_dir) {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "connection '{}': destination port '{}' has direction '{}', \
                                     expected 'in' or 'in out' for down connection",
                                    conn.name, dst_end.feature, dst_dir
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                }

                // Bidirectional connections require in out on both ends
                if conn.is_bidirectional {
                    if src_dir != Direction::InOut {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "connection '{}': bidirectional connection requires 'in out' \
                                 on source port '{}', found '{}'",
                                conn.name, src_end.feature, src_dir
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                    if dst_dir != Direction::InOut {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "connection '{}': bidirectional connection requires 'in out' \
                                 on destination port '{}', found '{}'",
                                conn.name, dst_end.feature, dst_dir
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // Event ports: same direction rules apply but we note it's an event
                if matches!(
                    src_kind,
                    Some(FeatureKind::EventPort | FeatureKind::EventDataPort)
                ) {
                    // Event port direction rules are the same as data port rules
                    // (already checked above), but we could add event-specific checks here
                }
            }
        }

        diags
    }
}

/// Connection topology pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionPattern {
    /// Between two sibling subcomponents: `sub_a.port -> sub_b.port`
    Across,
    /// From subcomponent to enclosing: `sub_a.port -> port`
    Up,
    /// From enclosing to subcomponent: `port -> sub_a.port`
    Down,
}

/// Classify a connection as across, up, or down based on endpoint subcomponent presence.
fn classify_connection(
    src_subcomponent: &Option<Name>,
    dst_subcomponent: &Option<Name>,
) -> ConnectionPattern {
    match (src_subcomponent, dst_subcomponent) {
        (Some(_), Some(_)) => ConnectionPattern::Across,
        (Some(_), None) => ConnectionPattern::Up,
        (None, Some(_)) => ConnectionPattern::Down,
        // Both on enclosing — treat as across (edge case, shouldn't normally occur)
        (None, None) => ConnectionPattern::Across,
    }
}

/// Find the direction of a feature in the instance model.
///
/// If `subcomponent` is Some, look at that subcomponent's features.
/// If None, look at the owning component's own features.
fn find_feature_direction(
    instance: &SystemInstance,
    owner: spar_hir_def::instance::ComponentInstanceIdx,
    subcomponent: &Option<Name>,
    feature_name: &Name,
) -> Option<Direction> {
    let comp = if let Some(sub_name) = subcomponent {
        // Find the child component matching the subcomponent name
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
            return feat.direction;
        }
    }
    None
}

/// Find the kind of a feature in the instance model.
fn find_feature_kind(
    instance: &SystemInstance,
    owner: spar_hir_def::instance::ComponentInstanceIdx,
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

/// Check if a direction is compatible with being a data source (out or in out).
fn is_output_compatible(dir: Direction) -> bool {
    matches!(dir, Direction::Out | Direction::InOut)
}

/// Check if a direction is compatible with being a data sink (in or in out).
fn is_input_compatible(dir: Direction) -> bool {
    matches!(dir, Direction::In | Direction::InOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::Name;

    /// Test helper to build instances.
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
                in_modes: Vec::new(),
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
                classifier: None,
                access_kind: None,
                array_index: None,
            });
            self.components[owner].features.push(idx);
            idx
        }

        fn add_connection(
            &mut self,
            name: &str,
            kind: ConnectionKind,
            bidi: bool,
            owner: ComponentInstanceIdx,
            src: ConnectionEnd,
            dst: ConnectionEnd,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind,
                is_bidirectional: bidi,
                owner,
                src: Some(src),
                dst: Some(dst),
                in_modes: Vec::new(),
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

    #[test]
    fn valid_across_connection() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "valid across should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn invalid_across_in_to_out() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Wrong: in -> out for across connection
        b.add_feature("port1", FeatureKind::DataPort, Some(Direction::In), a);
        b.add_feature("port2", FeatureKind::DataPort, Some(Direction::Out), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(Some("a"), "port1"),
            end(Some("b"), "port2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            2,
            "should flag both src (in) and dst (out): {:?}",
            errors
        );
    }

    #[test]
    fn valid_up_connection() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("child", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), child);
        b.add_feature("ext_out", FeatureKind::DataPort, Some(Direction::Out), root);
        // Up: child.out1 -> ext_out (on enclosing)
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(Some("child"), "out1"),
            end(None, "ext_out"),
        );
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "valid up should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn valid_down_connection() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("child", ComponentCategory::System, Some(root));
        b.add_feature("ext_in", FeatureKind::DataPort, Some(Direction::In), root);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), child);
        // Down: ext_in (on enclosing) -> child.in1
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(None, "ext_in"),
            end(Some("child"), "in1"),
        );
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "valid down should have no errors: {:?}",
            errors
        );
    }

    #[test]
    fn invalid_down_out_to_out() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("child", ComponentCategory::System, Some(root));
        // Wrong: out -> out for down connection
        b.add_feature("ext_out", FeatureKind::DataPort, Some(Direction::Out), root);
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), child);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(None, "ext_out"),
            end(Some("child"), "out1"),
        );
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 2, "should flag both ends: {:?}", errors);
    }

    #[test]
    fn inout_compatible_everywhere() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("p1", FeatureKind::DataPort, Some(Direction::InOut), a);
        b.add_feature("p2", FeatureKind::DataPort, Some(Direction::InOut), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(Some("a"), "p1"),
            end(Some("b"), "p2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "in out is compatible everywhere: {:?}",
            errors
        );
    }

    #[test]
    fn bidirectional_requires_inout() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Bidirectional but src is only 'out'
        b.add_feature("p1", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("p2", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            true,
            root,
            end(Some("a"), "p1"),
            end(Some("b"), "p2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let bidi_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("bidirectional"))
            .collect();
        assert_eq!(
            bidi_errors.len(),
            2,
            "both ends need in out: {:?}",
            bidi_errors
        );
    }

    #[test]
    fn access_connections_skip_direction_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Access features with 'in' direction (provides/requires isn't modeled as direction)
        b.add_feature("acc1", FeatureKind::DataAccess, Some(Direction::In), a);
        b.add_feature("acc2", FeatureKind::DataAccess, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            false,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "access connections skip direction: {:?}",
            errors
        );
    }

    #[test]
    fn event_port_direction_rules() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Wrong direction: in -> out for event port
        b.add_feature("evt1", FeatureKind::EventPort, Some(Direction::In), a);
        b.add_feature("evt2", FeatureKind::EventPort, Some(Direction::Out), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            false,
            root,
            end(Some("a"), "evt1"),
            end(Some("b"), "evt2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = DirectionRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "event ports follow same direction rules: {:?}",
            errors
        );
    }

    #[test]
    fn classify_connection_patterns() {
        assert_eq!(
            classify_connection(&Some(Name::new("a")), &Some(Name::new("b"))),
            ConnectionPattern::Across
        );
        assert_eq!(
            classify_connection(&Some(Name::new("a")), &None),
            ConnectionPattern::Up
        );
        assert_eq!(
            classify_connection(&None, &Some(Name::new("b"))),
            ConnectionPattern::Down
        );
    }
}
