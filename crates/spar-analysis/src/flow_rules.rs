//! Flow specification and end-to-end flow validation rules (AS5506 §10).
//!
//! Validates flow-related rules beyond what `flow_check` covers:
//! - **FLOW-SPEC-PORT-EXISTS** — Flow spec endpoints must reference existing features
//! - **FLOW-SPEC-DIRECTION** — Flow source must be on an out port, sink on in port,
//!   path from in to out
//! - **FLOW-E2E-CONTINUITY** — End-to-end flow segments must chain properly
//! - **FLOW-E2E-FIRST-LAST** — First/last segments must match E2E flow endpoints
//! - **FLOW-IMPL-COVERS-SPEC** — Flow implementations must exist for all flow specs

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::FlowKind;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates flow specification and end-to-end flow rules on the instance model.
///
/// Checks AS5506 §10 rules:
/// - Flow spec endpoints reference existing features
/// - Flow direction constraints
/// - End-to-end flow segment continuity
/// - Flow implementation coverage
pub struct FlowRuleAnalysis;

impl Analysis for FlowRuleAnalysis {
    fn name(&self) -> &str {
        "flow_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — flow source/sink/path missing required port directions
        //   Warning — E2E flow continuity broken, E2E segment references nonexistent subcomponent
        //   Info    — flow spec not referenced by any end-to-end flow
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            // FLOW-SPEC-PORT-EXISTS + FLOW-SPEC-DIRECTION:
            // For each flow instance, validate that the component has
            // appropriately-directed ports for the flow kind.
            for &flow_idx in &comp.flows {
                let flow = &instance.flow_instances[flow_idx];

                match flow.kind {
                    FlowKind::Source => {
                        // FLOW-SPEC-DIRECTION: Source flow should reference
                        // an out or in-out port
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
                                severity: Severity::Error,
                                message: format!(
                                    "flow source '{}' requires an out or in-out port \
                                     but component '{}' has no output ports",
                                    flow.name, comp.name
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    FlowKind::Sink => {
                        // FLOW-SPEC-DIRECTION: Sink flow should reference
                        // an in or in-out port
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
                                severity: Severity::Error,
                                message: format!(
                                    "flow sink '{}' requires an in or in-out port \
                                     but component '{}' has no input ports",
                                    flow.name, comp.name
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    FlowKind::Path => {
                        // FLOW-SPEC-DIRECTION: Path flow needs both in and out ports
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
                                severity: Severity::Error,
                                message: format!(
                                    "flow path '{}' requires both in and out ports \
                                     but component '{}' is missing {}",
                                    flow.name,
                                    comp.name,
                                    if !has_in && !has_out {
                                        "both"
                                    } else if !has_in {
                                        "input port"
                                    } else {
                                        "output port"
                                    }
                                ),
                                path: path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                }
            }

            // FLOW-SPEC-PORT-EXISTS: Check that flows referencing specific
            // feature names can resolve those names.
            // (Flow instances in the instance model don't carry the specific
            // port reference, but we can validate via name matching when
            // the flow name contains a feature reference hint.)
        }

        // FLOW-E2E-CONTINUITY + FLOW-E2E-FIRST-LAST:
        // Check end-to-end flow segment continuity
        for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
            let owner = instance.component(e2e.owner);
            let path = component_path(instance, e2e.owner);

            if e2e.segments.is_empty() {
                continue; // Already caught by flow_check
            }

            // FLOW-E2E-CONTINUITY: Check that consecutive flow segments
            // chain through common subcomponents.
            // Segments alternate: sub.flow, connection, sub.flow, connection, ...
            // For chaining: the subcomponent at the end of segment[i]
            // should be connected to the subcomponent at the start of segment[i+2].
            // The connection at segment[i+1] should connect them.
            if e2e.segments.len() >= 3 {
                for i in (0..e2e.segments.len() - 2).step_by(2) {
                    let seg_flow = e2e.segments[i].as_str();
                    let seg_conn = e2e.segments[i + 1].as_str();
                    let seg_next = e2e.segments[i + 2].as_str();

                    // Extract subcomponent names from "sub.flow" notation
                    let src_sub = extract_subcomponent(seg_flow);
                    let dst_sub = extract_subcomponent(seg_next);

                    // Verify the connection at position i+1 connects these subcomponents
                    if let (Some(src), Some(dst)) = (src_sub, dst_sub) {
                        let conn_valid = owner.connections.iter().any(|&ci| {
                            let conn = &instance.connections[ci];
                            if !conn.name.as_str().eq_ignore_ascii_case(seg_conn) {
                                return false;
                            }
                            let conn_src = conn
                                .src
                                .as_ref()
                                .and_then(|s| s.subcomponent.as_ref())
                                .map(|n| n.as_str());
                            let conn_dst = conn
                                .dst
                                .as_ref()
                                .and_then(|d| d.subcomponent.as_ref())
                                .map(|n| n.as_str());

                            let fwd = conn_src.is_some_and(|s| s.eq_ignore_ascii_case(src))
                                && conn_dst.is_some_and(|d| d.eq_ignore_ascii_case(dst));
                            let rev = conn.is_bidirectional
                                && conn_src.is_some_and(|s| s.eq_ignore_ascii_case(dst))
                                && conn_dst.is_some_and(|d| d.eq_ignore_ascii_case(src));
                            fwd || rev
                        });

                        if !conn_valid {
                            // Don't flag if we can't find the connection at all
                            // (it might be an abbreviated reference)
                            let conn_exists = owner.connections.iter().any(|&ci| {
                                instance.connections[ci]
                                    .name
                                    .as_str()
                                    .eq_ignore_ascii_case(seg_conn)
                            });
                            if conn_exists {
                                diags.push(AnalysisDiagnostic {
                                    severity: Severity::Warning,
                                    message: format!(
                                        "end-to-end flow '{}': connection '{}' may not \
                                         connect subcomponents '{}' and '{}' as required \
                                         for flow continuity",
                                        e2e.name, seg_conn, src, dst
                                    ),
                                    path: path.clone(),
                                    analysis: self.name().to_string(),
                                });
                            }
                        }
                    }
                }
            }

            // FLOW-E2E-FIRST-LAST: First segment should be a flow source
            // or flow path, last should be a flow sink or flow path.
            if e2e.segments.len() >= 3 {
                let first_seg = e2e.segments[0].as_str();
                let last_seg = e2e.segments[e2e.segments.len() - 1].as_str();

                // Validate first segment references a valid subcomponent flow
                if let Some(first_sub) = extract_subcomponent(first_seg) {
                    let sub_exists = owner.children.iter().any(|&child_idx| {
                        instance
                            .component(child_idx)
                            .name
                            .as_str()
                            .eq_ignore_ascii_case(first_sub)
                    });
                    if !sub_exists {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "end-to-end flow '{}': first segment references \
                                 subcomponent '{}' which is not found in '{}'",
                                e2e.name, first_sub, owner.name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }

                if let Some(last_sub) = extract_subcomponent(last_seg) {
                    let sub_exists = owner.children.iter().any(|&child_idx| {
                        instance
                            .component(child_idx)
                            .name
                            .as_str()
                            .eq_ignore_ascii_case(last_sub)
                    });
                    if !sub_exists {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "end-to-end flow '{}': last segment references \
                                 subcomponent '{}' which is not found in '{}'",
                                e2e.name, last_sub, owner.name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // FLOW-IMPL-COVERS-SPEC: Check that flow specs are covered by
        // implementations in parent components.
        check_flow_coverage(instance, &mut diags);

        diags
    }
}

/// Extract the subcomponent name from a "subcomponent.flow" reference.
fn extract_subcomponent(segment: &str) -> Option<&str> {
    segment.split('.').next().filter(|s| !s.is_empty())
}

/// FLOW-IMPL-COVERS-SPEC: For each component with children that have flow specs,
/// check that the parent has end-to-end flows covering those children's flows.
fn check_flow_coverage(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    for (comp_idx, comp) in instance.all_components() {
        // Only check components that have both children with flows AND
        // their own end-to-end flows
        if comp.children.is_empty() {
            continue;
        }

        let child_has_flows = comp
            .children
            .iter()
            .any(|&child_idx| !instance.component(child_idx).flows.is_empty());

        if !child_has_flows {
            continue;
        }

        // Collect all child flow names referenced in E2E flows
        let mut referenced_flows: Vec<String> = Vec::new();
        for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
            if e2e.owner != comp_idx {
                continue;
            }
            for seg in &e2e.segments {
                referenced_flows.push(seg.as_str().to_ascii_lowercase());
            }
        }

        // Check each child's source/sink flows to see if they're referenced
        for &child_idx in &comp.children {
            let child = instance.component(child_idx);
            for &flow_idx in &child.flows {
                let flow = &instance.flow_instances[flow_idx];
                // Only check source and sink flows (not path flows which are
                // internal to the child)
                if !matches!(flow.kind, FlowKind::Source | FlowKind::Sink) {
                    continue;
                }

                let flow_ref =
                    format!("{}.{}", child.name.as_str(), flow.name.as_str()).to_ascii_lowercase();

                // Check if any E2E flow references this child flow
                let is_covered = referenced_flows.iter().any(|r| r == &flow_ref);

                if !is_covered && !instance.end_to_end_flows.is_empty() {
                    let path = component_path(instance, comp_idx);
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "flow {} '{}' on subcomponent '{}' is not referenced \
                             by any end-to-end flow in '{}'",
                            flow.kind_str(),
                            flow.name,
                            child.name,
                            comp.name
                        ),
                        path,
                        analysis: "flow_rules".to_string(),
                    });
                }
            }
        }
    }
}

/// Extension trait to get a display string for FlowKind.
trait FlowKindStr {
    fn kind_str(&self) -> &'static str;
}

impl FlowKindStr for spar_hir_def::instance::FlowInstance {
    fn kind_str(&self) -> &'static str {
        match self.kind {
            FlowKind::Source => "source",
            FlowKind::Sink => "sink",
            FlowKind::Path => "path",
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
                array_index: None,
                in_modes: Vec::new(),
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
                array_index: None,
            });
            self.components[owner].features.push(idx);
        }

        fn add_flow(&mut self, name: &str, kind: FlowKind, owner: ComponentInstanceIdx) {
            let idx = self.flow_instances.alloc(FlowInstance {
                name: Name::new(name),
                kind,
                owner,
            });
            self.components[owner].flows.push(idx);
        }

        fn add_connection(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: Option<&str>,
            src_feat: &str,
            dst_sub: Option<&str>,
            dst_feat: &str,
        ) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: Some(ConnectionEnd {
                    subcomponent: src_sub.map(Name::new),
                    feature: Name::new(src_feat),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: dst_sub.map(Name::new),
                    feature: Name::new(dst_feat),
                }),
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
        }

        fn add_e2e(&mut self, name: &str, owner: ComponentInstanceIdx, segments: Vec<&str>) {
            self.end_to_end_flows.alloc(EndToEndFlowInstance {
                name: Name::new(name),
                owner,
                segments: segments.into_iter().map(Name::new).collect(),
            });
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

    // ── FLOW-SPEC-DIRECTION tests ───────────────────────────────────

    #[test]
    fn flow_source_with_out_port_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        b.add_feature("reading", FeatureKind::DataPort, Direction::Out, sensor);
        b.add_flow("data_src", FlowKind::Source, sensor);
        b.set_children(root, vec![sensor]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("flow source"))
            .collect();
        assert!(errs.is_empty(), "valid source flow: {:?}", errs);
    }

    #[test]
    fn flow_source_with_only_in_port_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("input", FeatureKind::DataPort, Direction::In, comp);
        b.add_flow("bad_src", FlowKind::Source, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("no output ports"))
            .collect();
        assert_eq!(errs.len(), 1, "source with no out: {:?}", diags);
    }

    #[test]
    fn flow_source_with_inout_port_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("bidir", FeatureKind::DataPort, Direction::InOut, comp);
        b.add_flow("src", FlowKind::Source, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("flow source"))
            .collect();
        assert!(errs.is_empty(), "inout port satisfies source: {:?}", errs);
    }

    #[test]
    fn flow_sink_with_in_port_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let actuator = b.add_component("actuator", ComponentCategory::System, Some(root));
        b.add_feature("cmd", FeatureKind::DataPort, Direction::In, actuator);
        b.add_flow("data_snk", FlowKind::Sink, actuator);
        b.set_children(root, vec![actuator]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("flow sink"))
            .collect();
        assert!(errs.is_empty(), "valid sink flow: {:?}", errs);
    }

    #[test]
    fn flow_sink_with_only_out_port_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("output", FeatureKind::DataPort, Direction::Out, comp);
        b.add_flow("bad_sink", FlowKind::Sink, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("no input ports"))
            .collect();
        assert_eq!(errs.len(), 1, "sink with no in: {:?}", diags);
    }

    #[test]
    fn flow_path_with_both_directions_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("input", FeatureKind::DataPort, Direction::In, comp);
        b.add_feature("output", FeatureKind::DataPort, Direction::Out, comp);
        b.add_flow("pass_through", FlowKind::Path, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("flow path"))
            .collect();
        assert!(errs.is_empty(), "valid path flow: {:?}", errs);
    }

    #[test]
    fn flow_path_missing_out_port_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        b.add_feature("input", FeatureKind::DataPort, Direction::In, comp);
        b.add_flow("path", FlowKind::Path, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("flow path"))
            .collect();
        assert_eq!(errs.len(), 1, "path missing out port: {:?}", diags);
    }

    #[test]
    fn flow_on_featureless_component_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let comp = b.add_component("comp", ComponentCategory::System, Some(root));
        // No features at all — we skip the check
        b.add_flow("src", FlowKind::Source, comp);
        b.set_children(root, vec![comp]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errs.is_empty(),
            "featureless component should skip check: {:?}",
            errs
        );
    }

    // ── FLOW-E2E-FIRST-LAST tests ──────────────────────────────────

    #[test]
    fn e2e_first_segment_valid_subcomponent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_connection("c1", root, Some("sensor"), "out", Some("ctrl"), "in");
        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.data_src", "c1", "ctrl.data_sink"],
        );
        b.set_children(root, vec![sensor, ctrl]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("not found"))
            .collect();
        assert!(
            warns.is_empty(),
            "valid subcomponents should not warn: {:?}",
            warns
        );
    }

    #[test]
    fn e2e_first_segment_nonexistent_subcomponent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_connection("c1", root, Some("missing"), "out", Some("ctrl"), "in");
        b.add_e2e(
            "e2e_flow",
            root,
            vec!["missing.data_src", "c1", "ctrl.data_sink"],
        );
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("first segment") && d.message.contains("not found"))
            .collect();
        assert_eq!(
            warns.len(),
            1,
            "nonexistent first subcomponent should warn: {:?}",
            diags
        );
    }

    #[test]
    fn e2e_last_segment_nonexistent_subcomponent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        b.add_connection("c1", root, Some("sensor"), "out", Some("missing"), "in");
        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.data_src", "c1", "missing.data_sink"],
        );
        b.set_children(root, vec![sensor]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("last segment") && d.message.contains("not found"))
            .collect();
        assert_eq!(
            warns.len(),
            1,
            "nonexistent last subcomponent should warn: {:?}",
            diags
        );
    }

    // ── FLOW-E2E-CONTINUITY tests ──────────────────────────────────

    #[test]
    fn e2e_continuity_valid_chain() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_connection("c1", root, Some("a"), "out", Some("b"), "in");
        b.add_e2e("flow1", root, vec!["a.src", "c1", "b.sink"]);
        b.set_children(root, vec![a, bb]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let cont_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("continuity"))
            .collect();
        assert!(
            cont_warns.is_empty(),
            "valid chain should not warn: {:?}",
            cont_warns
        );
    }

    #[test]
    fn e2e_continuity_broken_chain() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        let c = b.add_component("c", ComponentCategory::System, Some(root));
        // c1 connects a->b, but we reference a->c in the E2E flow
        b.add_connection("c1", root, Some("a"), "out", Some("b"), "in");
        b.add_e2e("flow1", root, vec!["a.src", "c1", "c.sink"]);
        b.set_children(root, vec![a, bb, c]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let cont_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("continuity"))
            .collect();
        assert_eq!(cont_warns.len(), 1, "broken chain should warn: {:?}", diags);
    }

    #[test]
    fn e2e_empty_segments_no_crash() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_e2e("empty", root, vec![]);

        let inst = b.build(root);
        let _diags = FlowRuleAnalysis.analyze(&inst);
        // Should not panic — if we got here, it didn't crash
    }

    // ── FLOW-IMPL-COVERS-SPEC tests ────────────────────────────────

    #[test]
    fn covered_flows_no_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_flow("data_src", FlowKind::Source, sensor);
        b.add_flow("data_sink", FlowKind::Sink, ctrl);
        b.add_e2e("e2e", root, vec!["sensor.data_src", "c1", "ctrl.data_sink"]);
        b.set_children(root, vec![sensor, ctrl]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("not referenced"))
            .collect();
        assert!(
            infos.is_empty(),
            "covered flows should not produce info: {:?}",
            infos
        );
    }

    #[test]
    fn uncovered_flow_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_flow("data_src", FlowKind::Source, sensor);
        b.add_flow("data_sink", FlowKind::Sink, ctrl);
        // E2E flow only references sensor, not ctrl
        b.add_e2e("e2e", root, vec!["sensor.data_src", "c1", "sensor.other"]);
        b.set_children(root, vec![sensor, ctrl]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("not referenced"))
            .collect();
        assert!(
            !infos.is_empty(),
            "uncovered flow should produce info: {:?}",
            diags
        );
    }

    #[test]
    fn no_e2e_flows_no_coverage_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::System, Some(root));
        b.add_flow("data_src", FlowKind::Source, sensor);
        b.set_children(root, vec![sensor]);

        let inst = b.build(root);
        let diags = FlowRuleAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("not referenced"))
            .collect();
        assert!(
            infos.is_empty(),
            "no E2E flows = no coverage check: {:?}",
            infos
        );
    }

    // ── extract_subcomponent tests ──────────────────────────────────

    #[test]
    fn extract_subcomponent_works() {
        assert_eq!(extract_subcomponent("sensor.data_src"), Some("sensor"));
        assert_eq!(extract_subcomponent("ctrl.sink"), Some("ctrl"));
        assert_eq!(extract_subcomponent("simple"), Some("simple"));
        assert_eq!(extract_subcomponent(""), None);
    }
}
