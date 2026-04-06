//! Connection completeness analysis.
//!
//! Checks that ports have connections and that there are no dangling
//! connection endpoints.

use rustc_hash::FxHashSet;

use spar_hir_def::instance::{ComponentInstanceIdx, FeatureInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::Direction;

use spar_hir_def::instance::SystemOperationMode;

use crate::{Analysis, AnalysisDiagnostic, ModalAnalysis, Severity, component_path};

/// Analyzes connection completeness across the instance model.
///
/// Checks:
/// - Every required (in/inout) port has at least one incoming connection
/// - Every outgoing (out/inout) port has at least one connection
/// - Warns about features with no connections at all
pub struct ConnectivityAnalysis;

impl Analysis for ConnectivityAnalysis {
    fn name(&self) -> &str {
        "connectivity"
    }

    fn as_modal(&self) -> Option<&dyn ModalAnalysis> {
        Some(self)
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — unconnected required/provided port, or featureless component with connections
        //   Info    — feature with no direction and no connections
        let mut diags = Vec::new();

        // Build a set of (ComponentInstanceIdx, feature_name) pairs that are
        // referenced by at least one connection. This allows per-port precision
        // instead of the old heuristic that marked ALL ports as connected when
        // the component had ANY connection.
        let connected_features = collect_connected_features(instance);

        // For each component, check its features.
        for (comp_idx, comp) in instance.all_components() {
            // Only check features that are ports (data, event, event data).
            for &feat_idx in &comp.features {
                let feat = &instance.features[feat_idx];

                // Only check directional port-like features.
                if !is_port_feature(feat_idx, instance) {
                    continue;
                }

                let feat_name = feat.name.as_str();
                let is_connected = connected_features.contains(&(comp_idx, feat_name.to_string()));

                if !is_connected {
                    // Check if this feature is annotated as intentionally unconnected
                    // via the SPAR_Properties::Intentionally_Unconnected property.
                    if is_intentionally_unconnected(instance, comp_idx, feat_name) {
                        continue;
                    }

                    // This specific port has no connection.
                    let path = component_path(instance, comp_idx);
                    match feat.direction {
                        Some(Direction::In) | Some(Direction::InOut) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "input port '{}' has no incoming connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        Some(Direction::Out) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "output port '{}' has no outgoing connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        None => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "feature '{}' has no direction and no connections",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Check for components that have connections but no features at all.
        // This may indicate dangling connections.
        for (comp_idx, comp) in instance.all_components() {
            if !comp.connections.is_empty() {
                // Check children — if a child has no features, connections
                // referencing it are dangling.
                for &child_idx in &comp.children {
                    let child = instance.component(child_idx);
                    if child.features.is_empty() {
                        let path = component_path(instance, child_idx);
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "component '{}' has no features but parent '{}' declares connections",
                                child.name, comp.name
                            ),
                            path,
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // If the component itself has no features and no parent features
                // to connect to, warn.
                if comp.features.is_empty() && comp.children.is_empty() {
                    let path = component_path(instance, comp_idx);
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "component '{}' declares connections but has no features or subcomponents",
                            comp.name
                        ),
                        path,
                        analysis: self.name().to_string(),
                    });
                }
            }
        }

        diags
    }
}

impl ModalAnalysis for ConnectivityAnalysis {
    fn analyze_in_mode(
        &self,
        instance: &SystemInstance,
        som: &SystemOperationMode,
    ) -> Vec<AnalysisDiagnostic> {
        use crate::modal::is_component_active_in_som;
        use crate::modal::is_connection_active_in_som;

        let mut diags = Vec::new();

        // Build set of connected features considering only connections active in this SOM.
        let connected_features = collect_connected_features_in_som(instance, som);

        for (comp_idx, comp) in instance.all_components() {
            // Skip components that are not active in this SOM.
            if !is_component_active_in_som(instance, comp_idx, som) {
                continue;
            }

            for &feat_idx in &comp.features {
                let feat = &instance.features[feat_idx];

                if !is_port_feature(feat_idx, instance) {
                    continue;
                }

                let feat_name = feat.name.as_str();
                let is_connected = connected_features.contains(&(comp_idx, feat_name.to_string()));

                if !is_connected {
                    if is_intentionally_unconnected(instance, comp_idx, feat_name) {
                        continue;
                    }

                    let path = component_path(instance, comp_idx);
                    match feat.direction {
                        Some(Direction::In) | Some(Direction::InOut) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "input port '{}' has no incoming connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        Some(Direction::Out) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "output port '{}' has no outgoing connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        None => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "feature '{}' has no direction and no connections",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                }
            }
        }

        diags
    }
}

/// Build a set of connected features considering only connections active in a SOM.
fn collect_connected_features_in_som(
    instance: &SystemInstance,
    som: &SystemOperationMode,
) -> FxHashSet<(ComponentInstanceIdx, String)> {
    use crate::modal::is_connection_active_in_som;

    let mut connected: FxHashSet<(ComponentInstanceIdx, String)> = FxHashSet::default();

    for (_idx, conn) in instance.connections.iter() {
        // Skip connections not active in this SOM.
        if !is_connection_active_in_som(instance, conn.owner, &conn.in_modes, som) {
            continue;
        }
        if let Some(ref src) = conn.src
            && let Some(comp_idx) = resolve_subcomponent(instance, conn.owner, &src.subcomponent)
        {
            connected.insert((comp_idx, src.feature.as_str().to_string()));
        }
        if let Some(ref dst) = conn.dst
            && let Some(comp_idx) = resolve_subcomponent(instance, conn.owner, &dst.subcomponent)
        {
            connected.insert((comp_idx, dst.feature.as_str().to_string()));
        }
    }

    // Semantic connections are not mode-filtered (they are already traced).
    for sc in &instance.semantic_connections {
        connected.insert((
            sc.ultimate_source.0,
            sc.ultimate_source.1.as_str().to_string(),
        ));
        connected.insert((
            sc.ultimate_destination.0,
            sc.ultimate_destination.1.as_str().to_string(),
        ));
    }

    connected
}

/// Check if a feature is a port-like feature (data port, event port, event data port).
fn is_port_feature(_feat_idx: FeatureInstanceIdx, instance: &SystemInstance) -> bool {
    use spar_hir_def::item_tree::FeatureKind;
    let feat = &instance.features[_feat_idx];
    matches!(
        feat.kind,
        FeatureKind::DataPort | FeatureKind::EventPort | FeatureKind::EventDataPort
    )
}

/// Check if a feature is annotated as intentionally unconnected.
///
/// Looks for the `SPAR_Properties::Intentionally_Unconnected` property on the
/// component that owns the feature. The property value can be:
/// - `"all"` — all ports on the component are intentionally unconnected
/// - `"true"` — same as "all"
/// - A comma-separated list of feature names (case-insensitive), e.g. `"(feat1, feat2)"`
///   or `"feat1, feat2"`
fn is_intentionally_unconnected(
    instance: &SystemInstance,
    comp_idx: spar_hir_def::instance::ComponentInstanceIdx,
    feature_name: &str,
) -> bool {
    let props = instance.properties_for(comp_idx);
    let value = match props.get("SPAR_Properties", "Intentionally_Unconnected") {
        Some(v) => v,
        None => return false,
    };

    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    // "all" or "true" means every port is intentionally unconnected.
    if lower == "all" || lower == "true" {
        return true;
    }

    // Strip optional surrounding parentheses.
    let inner = lower
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(&lower);

    // Split by comma and check if the feature name appears in the list.
    let feat_lower = feature_name.to_ascii_lowercase();
    inner.split(',').any(|item| item.trim() == feat_lower)
}

/// Build a set of `(ComponentInstanceIdx, feature_name)` pairs for every port
/// that is referenced by at least one connection endpoint.
///
/// When `mode_filter` is `Some(name)`, connections whose `in_modes` list is
/// non-empty and does not contain the given mode are skipped. When
/// `mode_filter` is `None`, all connections are included (existing behaviour).
///
/// We collect from two sources:
/// 1. Raw `ConnectionInstance` endpoints (`src`/`dst`) — resolving the optional
///    subcomponent name to a child `ComponentInstanceIdx`.
/// 2. `SemanticConnection` ultimate endpoints which already carry resolved
///    component indices.
fn collect_connected_features(
    instance: &SystemInstance,
) -> FxHashSet<(ComponentInstanceIdx, String)> {
    collect_connected_features_with_mode(instance, None)
}

/// Mode-aware variant of [`collect_connected_features`].
fn collect_connected_features_with_mode(
    instance: &SystemInstance,
    mode_filter: Option<&str>,
) -> FxHashSet<(ComponentInstanceIdx, String)> {
    use crate::modal::is_active_in_mode;

    let mut connected: FxHashSet<(ComponentInstanceIdx, String)> = FxHashSet::default();

    // 1. Raw connection endpoints.
    for (_idx, conn) in instance.connections.iter() {
        // Skip connections not active in the requested mode.
        if !is_active_in_mode(&conn.in_modes, mode_filter) {
            continue;
        }
        if let Some(ref src) = conn.src
            && let Some(comp_idx) = resolve_subcomponent(instance, conn.owner, &src.subcomponent)
        {
            connected.insert((comp_idx, src.feature.as_str().to_string()));
        }
        if let Some(ref dst) = conn.dst
            && let Some(comp_idx) = resolve_subcomponent(instance, conn.owner, &dst.subcomponent)
        {
            connected.insert((comp_idx, dst.feature.as_str().to_string()));
        }
    }

    // 2. Semantic (traced end-to-end) connections.
    for sc in &instance.semantic_connections {
        connected.insert((
            sc.ultimate_source.0,
            sc.ultimate_source.1.as_str().to_string(),
        ));
        connected.insert((
            sc.ultimate_destination.0,
            sc.ultimate_destination.1.as_str().to_string(),
        ));
    }

    connected
}

/// Resolve a connection endpoint's subcomponent name to a `ComponentInstanceIdx`.
///
/// If `subcomponent` is `None`, the endpoint is on the owner itself.
/// If `Some(name)`, look up the first child of `owner` with that name.
fn resolve_subcomponent(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    subcomponent: &Option<spar_hir_def::name::Name>,
) -> Option<ComponentInstanceIdx> {
    match subcomponent {
        Some(sub_name) => {
            let owner_comp = instance.component(owner);
            owner_comp
                .children
                .iter()
                .find(|&&child_idx| {
                    instance.component(child_idx).name.as_str() == sub_name.as_str()
                })
                .copied()
        }
        None => Some(owner),
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
            dir: Option<Direction>,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction: dir,
                owner,
                classifier: None,
                access_kind: None,
                array_index: None,
            });
            self.components[owner].features.push(idx);
        }

        fn add_connection(&mut self, name: &str, owner: ComponentInstanceIdx) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: None,
                dst: None,
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
        }

        /// Add a connection with explicit source and destination endpoints.
        fn add_connection_with_endpoints(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src: Option<(Option<&str>, &str)>,
            dst: Option<(Option<&str>, &str)>,
        ) {
            let make_end = |end: (Option<&str>, &str)| ConnectionEnd {
                subcomponent: end.0.map(Name::new),
                feature: Name::new(end.1),
            };
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: src.map(make_end),
                dst: dst.map(make_end),
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
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

    #[test]
    fn unconnected_input_port_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("no incoming"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "unconnected input port should warn: {:?}",
            diags
        );
    }

    #[test]
    fn unconnected_output_port_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("no outgoing"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "unconnected output port should warn: {:?}",
            diags
        );
    }

    #[test]
    fn connected_port_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), comp);
        // Connection on parent (root) that explicitly references comp.in1
        b.add_connection_with_endpoints(
            "c1",
            root,
            Some((None, "out1")),        // source: root's own port
            Some((Some("comp"), "in1")), // destination: comp.in1
        );
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("no incoming"))
            .collect();
        assert!(
            warnings.is_empty(),
            "connected port should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn no_direction_feature_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("feat", FeatureKind::DataPort, None, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("no direction"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "no-direction feature should produce info: {:?}",
            diags
        );
    }

    #[test]
    fn non_port_feature_skipped() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        // BusAccess is not a port feature
        b.add_feature("bus", FeatureKind::BusAccess, Some(Direction::In), comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let port_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("no incoming") || d.message.contains("no outgoing"))
            .collect();
        assert!(
            port_warnings.is_empty(),
            "non-port features should be skipped: {:?}",
            port_warnings
        );
    }

    #[test]
    fn featureless_child_with_parent_connections_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_connection("c1", root);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning && d.message.contains("no features but parent")
            })
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "featureless child with parent connections: {:?}",
            diags
        );
    }

    #[test]
    fn component_with_connections_but_no_features_or_children() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Root has a connection but no features and no children
        b.add_connection("c1", root);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("no features or subcomponents")
            })
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "connection but no features/children: {:?}",
            diags
        );
    }

    #[test]
    fn partially_connected_component_flags_unconnected_ports() {
        // Component with 2 ports, only 1 connected — should warn about the unconnected one.
        // This is the regression test for the false-negative bug where ANY connection
        // on a component caused ALL its ports to be considered "covered".
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sender = b.add_component("sender", ComponentCategory::System, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), sender);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), receiver);
        b.add_feature("in2", FeatureKind::DataPort, Some(Direction::In), receiver);
        // Only connect sender.out1 -> receiver.in1; receiver.in2 is unconnected.
        b.add_connection_with_endpoints(
            "c1",
            root,
            Some((Some("sender"), "out1")),
            Some((Some("receiver"), "in1")),
        );
        b.set_children(root, vec![sender, receiver]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);

        // receiver.in2 should be flagged as unconnected.
        let unconnected_in2: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("in2")
                    && d.message.contains("no incoming")
            })
            .collect();
        assert_eq!(
            unconnected_in2.len(),
            1,
            "unconnected port 'in2' should warn: {:?}",
            diags
        );

        // receiver.in1 and sender.out1 should NOT be flagged.
        let false_positive: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && (d.message.contains("in1") || d.message.contains("out1"))
            })
            .collect();
        assert!(
            false_positive.is_empty(),
            "connected ports should not warn: {:?}",
            false_positive
        );
    }

    #[test]
    fn inout_port_unconnected_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("bidir", FeatureKind::DataPort, Some(Direction::InOut), comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = ConnectivityAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("no incoming"))
            .collect();
        assert_eq!(warnings.len(), 1, "inout port counts as input: {:?}", diags);
    }

    #[test]
    fn modal_connection_filtered_by_mode() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let src = b.add_component("src", ComponentCategory::System, Some(root));
        let dst = b.add_component("dst", ComponentCategory::System, Some(root));
        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), src);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), dst);
        b.set_children(root, vec![src, dst]);

        // Create a connection that is active only in "fast" mode.
        let conn_idx = b.connections.alloc(ConnectionInstance {
            name: Name::new("c1"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("src")),
                feature: Name::new("out1"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("dst")),
                feature: Name::new("in1"),
            }),
            in_modes: vec![Name::new("fast")],
        });
        b.components[root].connections.push(conn_idx);

        let inst = b.build(root);

        // When filtering for "fast" mode, both endpoints should appear.
        let fast = collect_connected_features_with_mode(&inst, Some("fast"));
        assert!(
            fast.contains(&(src, "out1".to_string())),
            "src.out1 should be connected in fast mode"
        );
        assert!(
            fast.contains(&(dst, "in1".to_string())),
            "dst.in1 should be connected in fast mode"
        );

        // When filtering for "slow" mode, the connection is not active.
        let slow = collect_connected_features_with_mode(&inst, Some("slow"));
        assert!(
            !slow.contains(&(src, "out1".to_string())),
            "src.out1 should NOT be connected in slow mode"
        );
        assert!(
            !slow.contains(&(dst, "in1".to_string())),
            "dst.in1 should NOT be connected in slow mode"
        );
    }
}
