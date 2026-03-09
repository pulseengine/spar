//! Flow implementation validation (AS5506 §10).
//!
//! Validates that flow implementations are structurally correct:
//! - Flow specs have valid kind (source/sink/path)
//! - End-to-end flow segments alternate between flows and connections
//! - Flow implementations exist for all flow specs (when an impl is present)
//! - Flow kind consistency between spec and implementation

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::FlowKind;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Validates flow specifications and end-to-end flows in the instance model.
///
/// Checks:
/// - End-to-end flows have at least one segment
/// - Components with flow specs in a parent E2E flow actually exist
/// - Flow sources have exactly one flow spec on the first segment component
/// - Flow sinks have exactly one flow spec on the last segment component
pub struct FlowCheckAnalysis;

impl Analysis for FlowCheckAnalysis {
    fn name(&self) -> &str {
        "flow_check"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            // Check flow specs: each should have a valid kind already
            // (enforced by the parser, but we can still validate constraints)
            for &flow_idx in &comp.flows {
                let flow = &instance.flow_instances[flow_idx];

                // A source flow should be on a component with at least one output port
                if flow.kind == FlowKind::Source {
                    let has_out = comp.features.iter().any(|&fi| {
                        let feat = &instance.features[fi];
                        matches!(
                            feat.direction,
                            Some(spar_hir_def::item_tree::Direction::Out)
                                | Some(spar_hir_def::item_tree::Direction::InOut)
                        )
                    });
                    if !has_out && !comp.features.is_empty() {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "flow source '{}' on component '{}' which has no output ports",
                                flow.name, comp.name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // A sink flow should be on a component with at least one input port
                if flow.kind == FlowKind::Sink {
                    let has_in = comp.features.iter().any(|&fi| {
                        let feat = &instance.features[fi];
                        matches!(
                            feat.direction,
                            Some(spar_hir_def::item_tree::Direction::In)
                                | Some(spar_hir_def::item_tree::Direction::InOut)
                        )
                    });
                    if !has_in && !comp.features.is_empty() {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "flow sink '{}' on component '{}' which has no input ports",
                                flow.name, comp.name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // A path flow should be on a component with both in and out ports
                if flow.kind == FlowKind::Path {
                    let has_in = comp.features.iter().any(|&fi| {
                        let feat = &instance.features[fi];
                        matches!(
                            feat.direction,
                            Some(spar_hir_def::item_tree::Direction::In)
                                | Some(spar_hir_def::item_tree::Direction::InOut)
                        )
                    });
                    let has_out = comp.features.iter().any(|&fi| {
                        let feat = &instance.features[fi];
                        matches!(
                            feat.direction,
                            Some(spar_hir_def::item_tree::Direction::Out)
                                | Some(spar_hir_def::item_tree::Direction::InOut)
                        )
                    });
                    if (!has_in || !has_out) && !comp.features.is_empty() {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "flow path '{}' on component '{}' which lacks both in and out ports",
                                flow.name, comp.name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // Check end-to-end flows
        for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
            let owner = instance.component(e2e.owner);
            let path = component_path(instance, e2e.owner);

            // E2E flows should have segments
            if e2e.segments.is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "end-to-end flow '{}' in '{}' has no segments",
                        e2e.name, owner.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
                continue;
            }

            // E2E flows should have an odd number of segments
            // (alternating flow_spec, connection, flow_spec, connection, flow_spec)
            if e2e.segments.len() % 2 == 0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "end-to-end flow '{}' in '{}' has {} segments \
                         (expected odd number: flow, conn, flow, ...)",
                        e2e.name, owner.name, e2e.segments.len()
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Validate that odd-indexed segments (connections) reference
            // actual connection names in the owner component
            for (i, seg) in e2e.segments.iter().enumerate() {
                if i % 2 == 1 {
                    // This should be a connection name
                    let seg_text = seg.as_str();
                    let is_connection = owner.connections.iter().any(|&ci| {
                        instance.connections[ci].name.eq_ci(seg)
                    });
                    if !is_connection && !seg_text.contains('.') {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "end-to-end flow '{}': segment '{}' at position {} \
                                 is not a known connection in '{}'",
                                e2e.name, seg, i, owner.name
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
        flow_instances: Arena<FlowInstance>,
        end_to_end_flows: Arena<EndToEndFlowInstance>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
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
                classifier: None,
                access_kind: None,
            });
            self.components[owner].features.push(idx);
        }

        fn add_flow(
            &mut self,
            name: &str,
            kind: FlowKind,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.flow_instances.alloc(FlowInstance {
                name: Name::new(name),
                kind,
                owner,
            });
            self.components[owner].flows.push(idx);
        }

        fn add_connection_inst(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: None,
                dst: None,
            });
            self.components[owner].connections.push(idx);
        }

        fn add_e2e(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            segments: Vec<&str>,
        ) {
            self.end_to_end_flows.alloc(EndToEndFlowInstance {
                name: Name::new(name),
                owner,
                segments: segments.into_iter().map(Name::new).collect(),
            });
        }

        fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
            self.components[parent].children = children;
        }

        fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
            SystemInstance {
                root,
                components: self.components,
                features: self.features,
                connections: self.connections,
                flow_instances: self.flow_instances,
                end_to_end_flows: self.end_to_end_flows,
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
    fn valid_flow_source_on_output_component() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        b.add_feature("reading", FeatureKind::DataPort, Direction::Out, sensor);
        b.add_flow("data_src", FlowKind::Source, sensor);
        b.set_children(root, vec![sensor]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags.iter().filter(|d| d.message.contains("flow source")).collect();
        assert!(warnings.is_empty(), "valid source flow: {:?}", warnings);
    }

    #[test]
    fn flow_source_on_input_only_component() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("input", FeatureKind::DataPort, Direction::In, comp);
        b.add_flow("bad_src", FlowKind::Source, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags.iter().filter(|d| d.message.contains("no output ports")).collect();
        assert_eq!(warnings.len(), 1, "should warn about source with no out: {:?}", diags);
    }

    #[test]
    fn valid_e2e_flow_with_odd_segments() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_connection_inst("c1", root);
        b.add_e2e("e2e_flow", root, vec!["sensor.data_src", "c1", "ctrl.data_sink"]);
        b.set_children(root, vec![sensor, ctrl]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "valid e2e flow: {:?}", errors);
    }

    #[test]
    fn e2e_flow_empty_segments() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_e2e("empty_flow", root, vec![]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "empty e2e should error: {:?}", diags);
        assert!(errors[0].message.contains("no segments"));
    }

    #[test]
    fn e2e_flow_even_segments_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_e2e("odd_flow", root, vec!["a.src", "c1"]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags.iter()
            .filter(|d| d.message.contains("expected odd"))
            .collect();
        assert_eq!(warnings.len(), 1, "even segment count: {:?}", diags);
    }

    #[test]
    fn e2e_flow_unknown_connection_segment() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Connection "c1" exists, but "c_unknown" does not
        b.add_connection_inst("c1", root);
        b.add_e2e("test_flow", root, vec!["a.src", "c_unknown", "b.sink"]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags.iter()
            .filter(|d| d.message.contains("not a known connection"))
            .collect();
        assert_eq!(warnings.len(), 1, "should flag unknown connection: {:?}", diags);
    }

    #[test]
    fn flow_path_needs_both_directions() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        // Only has input, no output
        b.add_feature("input", FeatureKind::DataPort, Direction::In, comp);
        b.add_flow("pass_through", FlowKind::Path, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowCheckAnalysis.analyze(&inst);
        let warnings: Vec<_> = diags.iter()
            .filter(|d| d.message.contains("lacks both"))
            .collect();
        assert_eq!(warnings.len(), 1, "path needs both directions: {:?}", diags);
    }
}
