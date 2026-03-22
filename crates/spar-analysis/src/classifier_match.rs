//! Connection classifier type matching (AS5506 §9).
//!
//! Validates that connection endpoints have compatible classifier types:
//! - **CONN-CLASSIFIER-MATCH** — For port connections (DataPort, EventDataPort),
//!   both endpoint classifiers must reference the same data type.
//! - **CONN-ACCESS-MATCH** — For access connections, classifiers must match AND
//!   access_kind must be compatible (Provides ↔ Requires).
//! - **CONN-CLASSIFIER-MISSING** — Info-level: one endpoint has a classifier but
//!   the other doesn't (potential type safety gap).

use spar_hir_def::instance::{ComponentInstanceIdx, FeatureInstance, SystemInstance};
use spar_hir_def::item_tree::{ConnectionKind, FeatureKind};
use spar_hir_def::name::Name;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates connection classifier type matching on the instance model.
pub struct ClassifierMatchAnalysis;

impl Analysis for ClassifierMatchAnalysis {
    fn name(&self) -> &str {
        "classifier_match"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error — mismatching classifiers between endpoints, same access kind on both ends
        //   Info  — one endpoint has classifier but other does not (type safety gap)
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            for &conn_idx in &comp.connections {
                let conn = &instance.connections[conn_idx];

                let (src_end, dst_end) = match (&conn.src, &conn.dst) {
                    (Some(s), Some(d)) => (s, d),
                    _ => continue, // incomplete connection, skip
                };

                let src_feat =
                    find_feature(instance, comp_idx, &src_end.subcomponent, &src_end.feature);
                let dst_feat =
                    find_feature(instance, comp_idx, &dst_end.subcomponent, &dst_end.feature);

                let (src_feat, dst_feat) = match (src_feat, dst_feat) {
                    (Some(s), Some(d)) => (s, d),
                    _ => continue, // can't resolve, skip
                };

                let path = component_path(instance, comp_idx);

                // Check classifier matching for port-like connections
                if conn.kind == ConnectionKind::Port || conn.kind == ConnectionKind::Feature {
                    check_port_classifier_match(
                        &conn.name,
                        src_feat,
                        dst_feat,
                        &src_end.feature,
                        &dst_end.feature,
                        &path,
                        &mut diags,
                    );
                }

                // Check access connection classifier + provides/requires compatibility
                if conn.kind == ConnectionKind::Access {
                    check_access_match(
                        &conn.name,
                        src_feat,
                        dst_feat,
                        &src_end.feature,
                        &dst_end.feature,
                        &path,
                        &mut diags,
                    );
                }
            }
        }

        diags
    }
}

/// CONN-CLASSIFIER-MATCH / CONN-CLASSIFIER-MISSING for port connections.
///
/// For DataPort, EventDataPort, and Parameter features: if both endpoints
/// have classifiers, they must reference the same data type (case-insensitive).
/// If only one has a classifier, emit an info-level diagnostic.
fn check_port_classifier_match(
    conn_name: &Name,
    src_feat: &FeatureInstance,
    dst_feat: &FeatureInstance,
    src_name: &Name,
    dst_name: &Name,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // Only check features that carry data classifiers
    let carries_data = |kind: FeatureKind| {
        matches!(
            kind,
            FeatureKind::DataPort | FeatureKind::EventDataPort | FeatureKind::Parameter
        )
    };

    if !carries_data(src_feat.kind) && !carries_data(dst_feat.kind) {
        return;
    }

    match (&src_feat.classifier, &dst_feat.classifier) {
        (Some(src_cls), Some(dst_cls)) => {
            // Both have classifiers — compare type names case-insensitively
            if !classifiers_match(src_cls, dst_cls) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "connection '{}': source feature '{}' has classifier '{}' but \
                         destination feature '{}' has classifier '{}' — data types must match",
                        conn_name, src_name, src_cls, dst_name, dst_cls
                    ),
                    path: path.to_vec(),
                    analysis: "classifier_match".to_string(),
                });
            }
        }
        (Some(cls), None) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "connection '{}': source feature '{}' has classifier '{}' but \
                     destination feature '{}' has no classifier — potential type safety gap",
                    conn_name, src_name, cls, dst_name
                ),
                path: path.to_vec(),
                analysis: "classifier_match".to_string(),
            });
        }
        (None, Some(cls)) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "connection '{}': destination feature '{}' has classifier '{}' but \
                     source feature '{}' has no classifier — potential type safety gap",
                    conn_name, dst_name, cls, src_name
                ),
                path: path.to_vec(),
                analysis: "classifier_match".to_string(),
            });
        }
        (None, None) => {
            // Neither has a classifier — nothing to check
        }
    }
}

/// CONN-ACCESS-MATCH for access connections.
///
/// For access connections (DataAccess, BusAccess, SubprogramAccess):
/// - Classifiers must match (same as port connections)
/// - Access kinds must be complementary (Provides ↔ Requires)
fn check_access_match(
    conn_name: &Name,
    src_feat: &FeatureInstance,
    dst_feat: &FeatureInstance,
    src_name: &Name,
    dst_name: &Name,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let is_access = |kind: FeatureKind| {
        matches!(
            kind,
            FeatureKind::DataAccess
                | FeatureKind::BusAccess
                | FeatureKind::SubprogramAccess
                | FeatureKind::SubprogramGroupAccess
        )
    };

    if !is_access(src_feat.kind) && !is_access(dst_feat.kind) {
        return;
    }

    // Check classifier matching (same logic as ports)
    match (&src_feat.classifier, &dst_feat.classifier) {
        (Some(src_cls), Some(dst_cls)) => {
            if !classifiers_match(src_cls, dst_cls) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "connection '{}': source feature '{}' has classifier '{}' but \
                         destination feature '{}' has classifier '{}' — access types must match",
                        conn_name, src_name, src_cls, dst_name, dst_cls
                    ),
                    path: path.to_vec(),
                    analysis: "classifier_match".to_string(),
                });
            }
        }
        (Some(cls), None) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "connection '{}': source feature '{}' has classifier '{}' but \
                     destination feature '{}' has no classifier — potential type safety gap",
                    conn_name, src_name, cls, dst_name
                ),
                path: path.to_vec(),
                analysis: "classifier_match".to_string(),
            });
        }
        (None, Some(cls)) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "connection '{}': destination feature '{}' has classifier '{}' but \
                     source feature '{}' has no classifier — potential type safety gap",
                    conn_name, dst_name, cls, src_name
                ),
                path: path.to_vec(),
                analysis: "classifier_match".to_string(),
            });
        }
        (None, None) => {}
    }

    // Check access kind compatibility: Provides ↔ Requires
    match (&src_feat.access_kind, &dst_feat.access_kind) {
        (Some(src_ak), Some(dst_ak)) if src_ak == dst_ak => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "connection '{}': both source '{}' and destination '{}' are '{}' — \
                     access connections require provides ↔ requires pairing",
                    conn_name, src_name, dst_name, src_ak
                ),
                path: path.to_vec(),
                analysis: "classifier_match".to_string(),
            });
        }
        _ => {
            // One or both missing access_kind — skip (may not be resolvable)
        }
    }
}

/// Compare two classifier references for type-level equality.
///
/// Two classifiers match if their type names match (case-insensitive),
/// and if both have package qualifiers, those must also match.
fn classifiers_match(
    a: &spar_hir_def::name::ClassifierRef,
    b: &spar_hir_def::name::ClassifierRef,
) -> bool {
    // Type names must match
    if !a.type_name.eq_ci(&b.type_name) {
        return false;
    }

    // If both have package qualifiers, those must match too
    match (&a.package, &b.package) {
        (Some(pa), Some(pb)) => pa.eq_ci(pb),
        // If only one has a package qualifier, we treat it as a match
        // (the unqualified reference might resolve to the same package).
        _ => true,
    }
}

/// Find a feature instance by resolving a connection endpoint.
fn find_feature<'a>(
    instance: &'a SystemInstance,
    owner: ComponentInstanceIdx,
    subcomponent: &Option<Name>,
    feature_name: &Name,
) -> Option<&'a FeatureInstance> {
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
            return Some(feat);
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
    use spar_hir_def::name::{ClassifierRef, Name};

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
            classifier: Option<ClassifierRef>,
            access_kind: Option<AccessKind>,
        ) -> FeatureInstanceIdx {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction,
                owner,
                classifier,
                access_kind,
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

    fn cls(pkg: &str, typ: &str) -> Option<ClassifierRef> {
        Some(ClassifierRef::qualified(Name::new(pkg), Name::new(typ)))
    }

    // ── CONN-CLASSIFIER-MATCH tests ──────────────────────────────────

    #[test]
    fn matching_classifiers_no_diagnostic() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "matching classifiers should produce no diagnostics: {:?}",
            diags
        );
    }

    #[test]
    fn mismatching_classifiers_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "CommandData"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("data types must match")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "mismatching classifiers should produce one error: {:?}",
            diags
        );
        assert!(errors[0].message.contains("SensorData"));
        assert!(errors[0].message.contains("CommandData"));
    }

    #[test]
    fn one_missing_classifier_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            None,
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("type safety gap"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "one missing classifier should produce info: {:?}",
            diags
        );
    }

    #[test]
    fn no_classifier_on_either_side_no_diagnostic() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            None,
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            None,
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "no classifiers on either side should produce no diagnostics: {:?}",
            diags
        );
    }

    // ── CONN-ACCESS-MATCH tests ──────────────────────────────────────

    #[test]
    fn access_provides_requires_match_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::DataAccess,
            None,
            a,
            cls("DataTypes", "SharedBuffer"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "acc2",
            FeatureKind::DataAccess,
            None,
            bb,
            cls("DataTypes", "SharedBuffer"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "provides/requires pairing should produce no errors: {:?}",
            errors
        );
    }

    #[test]
    fn access_same_direction_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::DataAccess,
            None,
            a,
            cls("DataTypes", "SharedBuffer"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "acc2",
            FeatureKind::DataAccess,
            None,
            bb,
            cls("DataTypes", "SharedBuffer"),
            Some(AccessKind::Provides),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("provides"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "same access direction should produce an error: {:?}",
            diags
        );
    }

    #[test]
    fn event_data_port_matching_classifier_ok() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "evt_out",
            FeatureKind::EventDataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "AlertMsg"),
            None,
        );
        b.add_feature(
            "evt_in",
            FeatureKind::EventDataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "AlertMsg"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "evt_out"),
            end(Some("b"), "evt_in"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "matching EventDataPort classifiers should produce no diagnostics: {:?}",
            diags
        );
    }

    #[test]
    fn cross_component_classifier_match() {
        // Test across a deeper hierarchy: root -> mid -> leaf connections
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mid_a = b.add_component("mid_a", ComponentCategory::System, Some(root));
        let mid_b = b.add_component("mid_b", ComponentCategory::System, Some(root));

        // Features on the mid-level components
        b.add_feature(
            "data_out",
            FeatureKind::DataPort,
            Some(Direction::Out),
            mid_a,
            cls("Pkg", "Telemetry"),
            None,
        );
        b.add_feature(
            "data_in",
            FeatureKind::DataPort,
            Some(Direction::In),
            mid_b,
            cls("pkg", "telemetry"),
            None, // different case — should still match
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("mid_a"), "data_out"),
            end(Some("mid_b"), "data_in"),
        );
        b.set_children(root, vec![mid_a, mid_b]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "case-insensitive classifier match should produce no diagnostics: {:?}",
            diags
        );
    }

    #[test]
    fn access_classifier_mismatch_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "bus_acc",
            FeatureKind::BusAccess,
            None,
            a,
            cls("HW", "PCIBus"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "bus_acc",
            FeatureKind::BusAccess,
            None,
            bb,
            cls("HW", "EthernetBus"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "bus_acc"),
            end(Some("b"), "bus_acc"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("access types must match")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "access classifier mismatch should produce an error: {:?}",
            diags
        );
    }

    #[test]
    fn event_port_no_classifier_check() {
        // Pure event ports don't carry data classifiers — should not trigger diagnostics
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "evt_out",
            FeatureKind::EventPort,
            Some(Direction::Out),
            a,
            None,
            None,
        );
        b.add_feature(
            "evt_in",
            FeatureKind::EventPort,
            Some(Direction::In),
            bb,
            None,
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "evt_out"),
            end(Some("b"), "evt_in"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "pure event ports should not produce classifier diagnostics: {:?}",
            diags
        );
    }

    // ── check_port_classifier_match: guard condition tests ─────────

    #[test]
    fn port_conn_src_carries_data_dst_event_port_checks_classifier() {
        // Only src carries data (DataPort), dst is EventPort (no data).
        // The guard `!carries_data(src) && !carries_data(dst)` is false because
        // src DOES carry data → check proceeds.
        // If `&&` were mutated to `||`, both src-carries and dst-not-carries
        // would short-circuit to return early, missing the info diagnostic.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::EventPort,
            Some(Direction::In),
            bb,
            None,
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        // Src has classifier, dst has None → should emit Info about type safety gap
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "src with classifier, dst EventPort without → should emit info: {:?}",
            diags
        );
    }

    #[test]
    fn port_conn_dst_carries_data_src_event_port_checks_classifier() {
        // Only dst carries data (DataPort), src is EventPort.
        // Guard `!carries_data(src) && !carries_data(dst)` is false because dst carries data.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::EventPort,
            Some(Direction::Out),
            a,
            None,
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "dst with classifier, src EventPort without → should emit info: {:?}",
            diags
        );
    }

    #[test]
    fn port_conn_both_non_data_ports_skipped() {
        // Both are EventPort → neither carries data → guard returns early
        // No diagnostics should be produced.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::EventPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "Alarm"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::EventPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "Alert"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "both EventPort (non-data) should skip classifier check: {:?}",
            diags
        );
    }

    #[test]
    fn port_conn_parameter_features_checked() {
        // Parameter features carry data → should be checked
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "param1",
            FeatureKind::Parameter,
            Some(Direction::Out),
            a,
            cls("DataTypes", "IntType"),
            None,
        );
        b.add_feature(
            "param2",
            FeatureKind::Parameter,
            Some(Direction::In),
            bb,
            cls("DataTypes", "FloatType"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Feature,
            root,
            end(Some("a"), "param1"),
            end(Some("b"), "param2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("data types must match")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "Parameter features with mismatching classifiers should error: {:?}",
            diags
        );
    }

    // ── check_access_match: guard condition tests ───────────────────

    #[test]
    fn access_conn_src_is_access_dst_is_not_still_checks() {
        // Only src is DataAccess, dst is DataPort (not access).
        // Guard `!is_access(src) && !is_access(dst)` is false because src IS access.
        // If `&&` mutated to `||`, this would skip the check.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::DataAccess,
            None,
            a,
            cls("DataTypes", "SharedBuf"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "port1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            None,
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "port1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        // src has classifier, dst has None → should emit Info
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "access src with classifier, non-access dst without → info: {:?}",
            diags
        );
    }

    #[test]
    fn access_conn_neither_is_access_skipped() {
        // Both features are DataPort (not access kind) → guard returns early
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "port1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "TypeA"),
            None,
        );
        b.add_feature(
            "port2",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "TypeB"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "port1"),
            end(Some("b"), "port2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        // Neither is access → should skip, producing no access-related diagnostics
        assert!(
            diags.is_empty(),
            "neither feature is access → should skip access check: {:?}",
            diags
        );
    }

    #[test]
    fn access_subprogram_access_features_checked() {
        // SubprogramAccess is in the is_access set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "sp1",
            FeatureKind::SubprogramAccess,
            None,
            a,
            cls("Code", "Handler"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "sp2",
            FeatureKind::SubprogramAccess,
            None,
            bb,
            cls("Code", "Handler"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "sp1"),
            end(Some("b"), "sp2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "matching SubprogramAccess provides/requires should be clean: {:?}",
            diags
        );
    }

    #[test]
    fn access_subprogram_group_access_features_checked() {
        // SubprogramGroupAccess is in the is_access set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "spg1",
            FeatureKind::SubprogramGroupAccess,
            None,
            a,
            cls("Code", "HandlerGroup"),
            Some(AccessKind::Requires),
        );
        b.add_feature(
            "spg2",
            FeatureKind::SubprogramGroupAccess,
            None,
            bb,
            cls("Code", "HandlerGroup"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "spg1"),
            end(Some("b"), "spg2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("provides"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "SubprogramGroupAccess both requires → same-direction error: {:?}",
            diags
        );
    }

    // ── classifiers_match: package qualifier tests ──────────────────

    #[test]
    fn classifiers_match_both_packages_same() {
        let a = ClassifierRef::qualified(Name::new("DataTypes"), Name::new("Sensor"));
        let b = ClassifierRef::qualified(Name::new("DataTypes"), Name::new("Sensor"));
        assert!(classifiers_match(&a, &b));
    }

    #[test]
    fn classifiers_match_both_packages_different() {
        let a = ClassifierRef::qualified(Name::new("PkgA"), Name::new("Sensor"));
        let b = ClassifierRef::qualified(Name::new("PkgB"), Name::new("Sensor"));
        assert!(
            !classifiers_match(&a, &b),
            "different package qualifiers should NOT match"
        );
    }

    #[test]
    fn classifiers_match_one_unqualified() {
        // One has package, other doesn't → treated as match
        let a = ClassifierRef::qualified(Name::new("Pkg"), Name::new("Sensor"));
        let b = ClassifierRef::type_only(Name::new("Sensor"));
        assert!(
            classifiers_match(&a, &b),
            "one unqualified should still match"
        );
    }

    #[test]
    fn classifiers_match_neither_qualified() {
        let a = ClassifierRef::type_only(Name::new("Sensor"));
        let b = ClassifierRef::type_only(Name::new("Sensor"));
        assert!(classifiers_match(&a, &b));
    }

    #[test]
    fn classifiers_match_type_names_different() {
        let a = ClassifierRef::qualified(Name::new("Pkg"), Name::new("TypeA"));
        let b = ClassifierRef::qualified(Name::new("Pkg"), Name::new("TypeB"));
        assert!(
            !classifiers_match(&a, &b),
            "different type names should NOT match"
        );
    }

    #[test]
    fn classifiers_match_type_names_case_insensitive() {
        let a = ClassifierRef::qualified(Name::new("Pkg"), Name::new("SENSOR"));
        let b = ClassifierRef::qualified(Name::new("pkg"), Name::new("sensor"));
        assert!(
            classifiers_match(&a, &b),
            "case-insensitive type and package names should match"
        );
    }

    // ── Port classifier: both have classifiers, match vs mismatch ──

    #[test]
    fn port_classifier_both_present_matching_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::EventDataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "Msg"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::EventDataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "Msg"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "both classifiers present and matching → no diagnostic: {:?}",
            diags
        );
    }

    #[test]
    fn port_classifier_both_present_mismatching_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::EventDataPort,
            Some(Direction::Out),
            a,
            cls("DataTypes", "MsgA"),
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::EventDataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "MsgB"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("data types must match")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "both classifiers present but mismatching → error: {:?}",
            diags
        );
    }

    #[test]
    fn port_classifier_dst_has_src_none_info() {
        // src None, dst Some → Info
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "out1",
            FeatureKind::DataPort,
            Some(Direction::Out),
            a,
            None,
            None,
        );
        b.add_feature(
            "in1",
            FeatureKind::DataPort,
            Some(Direction::In),
            bb,
            cls("DataTypes", "SensorData"),
            None,
        );
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("a"), "out1"),
            end(Some("b"), "in1"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("destination"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "src None, dst Some → Info about destination having classifier: {:?}",
            diags
        );
    }

    // ── Access: same vs different classifier ────────────────────────

    #[test]
    fn access_same_classifier_provides_requires_clean() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::BusAccess,
            None,
            a,
            cls("HW", "PCI"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "acc2",
            FeatureKind::BusAccess,
            None,
            bb,
            cls("HW", "PCI"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "same classifier + provides/requires → clean: {:?}",
            diags
        );
    }

    #[test]
    fn access_different_classifier_provides_requires_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::BusAccess,
            None,
            a,
            cls("HW", "PCI"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "acc2",
            FeatureKind::BusAccess,
            None,
            bb,
            cls("HW", "USB"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("access types must match")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "different classifier on access → error: {:?}",
            diags
        );
    }

    #[test]
    fn access_both_requires_same_classifier_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::DataAccess,
            None,
            a,
            cls("Data", "Buf"),
            Some(AccessKind::Requires),
        );
        b.add_feature(
            "acc2",
            FeatureKind::DataAccess,
            None,
            bb,
            cls("Data", "Buf"),
            Some(AccessKind::Requires),
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "both requires → same-direction error: {:?}",
            diags
        );
        assert!(errors[0].message.contains("provides"));
    }

    #[test]
    fn access_no_access_kind_on_one_side_no_direction_error() {
        // One side has no access_kind → skip direction check
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bb = b.add_component("b", ComponentCategory::Process, Some(root));
        b.add_feature(
            "acc1",
            FeatureKind::DataAccess,
            None,
            a,
            cls("Data", "Buf"),
            Some(AccessKind::Provides),
        );
        b.add_feature(
            "acc2",
            FeatureKind::DataAccess,
            None,
            bb,
            cls("Data", "Buf"),
            None, // no access_kind
        );
        b.add_connection(
            "c1",
            ConnectionKind::Access,
            root,
            end(Some("a"), "acc1"),
            end(Some("b"), "acc2"),
        );
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "missing access_kind on one side → no direction error: {:?}",
            diags
        );
    }

    #[test]
    fn incomplete_connection_skipped() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let idx = b.connections.alloc(ConnectionInstance {
            name: Name::new("c_incomplete"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: None,
            dst: None,
            in_modes: Vec::new(),
        });
        b.components[root].connections.push(idx);

        let inst = b.build(root);
        let diags = ClassifierMatchAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "incomplete connections should be skipped: {:?}",
            diags
        );
    }
}
