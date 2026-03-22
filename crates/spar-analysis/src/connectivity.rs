//! Connection completeness analysis.
//!
//! Checks that ports have connections and that there are no dangling
//! connection endpoints.

use rustc_hash::FxHashSet;

use spar_hir_def::instance::{FeatureInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::Direction;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

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

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — unconnected required/provided port, or featureless component with connections
        //   Info    — feature with no direction and no connections
        let mut diags = Vec::new();

        // Collect all feature indices that participate in connections.
        // In the current instance model, connections are declared at the
        // component level but don't carry endpoint feature indices. We
        // use a heuristic: a feature is "connected" if there is at least
        // one connection on the component or its parent.
        //
        // We gather the set of components that *own* connections.
        let mut components_with_connections: FxHashSet<_> = FxHashSet::default();
        for (_idx, conn) in instance.connections.iter() {
            components_with_connections.insert(conn.owner);
        }

        // For each component, check its features.
        for (comp_idx, comp) in instance.all_components() {
            // A feature is "covered" if:
            // - The owning component has connections, OR
            // - The parent component has connections (connections flow between
            //   parent and child features in AADL)
            let owner_has_conns = components_with_connections.contains(&comp_idx);
            let parent_has_conns = comp
                .parent
                .map(|p| components_with_connections.contains(&p))
                .unwrap_or(false);
            let has_conns = owner_has_conns || parent_has_conns;

            // Only check features that are ports (data, event, event data).
            for &feat_idx in &comp.features {
                let feat = &instance.features[feat_idx];

                // Only check directional port-like features.
                if !is_port_feature(feat_idx, instance) {
                    continue;
                }

                if !has_conns {
                    // Check if this feature is annotated as intentionally unconnected
                    // via the SPAR_Properties::Intentionally_Unconnected property.
                    if is_intentionally_unconnected(instance, comp_idx, feat.name.as_str()) {
                        continue;
                    }

                    // No connections at all on this component or parent.
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
        // Add connection on parent (root) — covers child features
        b.add_connection("c1", root);
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
}
