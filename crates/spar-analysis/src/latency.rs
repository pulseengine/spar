//! End-to-end flow latency analysis.
//!
//! Computes latency bounds for end-to-end flows in the instance model.
//! For each E2E flow, traces through the flow segments and computes
//! best-case and worst-case latency based on execution times and
//! sampling delays (periods) at connection crossings.
//!
//! Follows the OSATE flow latency analysis approach:
//! - Best case: sum of execution times only
//! - Worst case: sum of execution times + sum of periods (sampling delays)

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{get_execution_time, get_processor_binding, get_timing_property};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// End-to-end flow latency analysis.
pub struct LatencyAnalysis;

impl Analysis for LatencyAnalysis {
    fn name(&self) -> &str {
        "latency"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — component without timing properties, worst-case latency exceeds bound
        //   Info    — end-to-end flow latency range, modal awareness note
        let mut diags = Vec::new();

        for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
            let owner_path = component_path(instance, e2e.owner);
            let owner = instance.component(e2e.owner);

            if e2e.segments.is_empty() {
                continue; // flow_check already reports this
            }

            // Walk segments: odd indices are connections, even indices are
            // subcomponent flow references ("subcomp.flow_name")
            let mut best_case_ps: u64 = 0;
            let mut worst_case_ps: u64 = 0;
            let mut missing_timing = Vec::new();
            let mut connection_count: u64 = 0;

            for (i, seg) in e2e.segments.iter().enumerate() {
                if i % 2 == 1 {
                    // Connection segment — adds sampling delay in worst case.
                    connection_count += 1;
                    continue;
                }

                // Flow segment: "subcomponent.flow_name" or just a name
                let seg_str = seg.as_str();
                let subcomp_name = seg_str.split('.').next().unwrap_or(seg_str);

                // Find the subcomponent in the owner's children
                let child = owner.children.iter().find(|&&child_idx| {
                    instance
                        .component(child_idx)
                        .name
                        .as_str()
                        .eq_ignore_ascii_case(subcomp_name)
                });

                if let Some(&child_idx) = child {
                    let child_comp = instance.component(child_idx);
                    let child_props = instance.properties_for(child_idx);

                    // Get execution time contribution
                    let exec_ps = get_execution_time(child_props);
                    // Get period for sampling delay
                    let period_ps = get_timing_property(child_props, "Period");

                    if let Some(exec) = exec_ps {
                        best_case_ps = best_case_ps.saturating_add(exec);
                        worst_case_ps = worst_case_ps.saturating_add(exec);
                    } else {
                        missing_timing.push(child_comp.name.as_str().to_string());
                    }

                    // Add sampling delay for connections after the first component
                    if connection_count > 0
                        && let Some(period) = period_ps
                    {
                        worst_case_ps = worst_case_ps.saturating_add(period);
                    }
                } else {
                    missing_timing.push(subcomp_name.to_string());
                }
            }

            // Report missing timing properties
            if !missing_timing.is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "end-to-end flow '{}': components without timing properties: {}",
                        e2e.name,
                        missing_timing.join(", ")
                    ),
                    path: owner_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Report latency range
            let best_ms = best_case_ps as f64 / 1_000_000_000.0;
            let worst_ms = worst_case_ps as f64 / 1_000_000_000.0;

            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "end-to-end flow '{}' latency: [{:.3} ms .. {:.3} ms]",
                    e2e.name, best_ms, worst_ms,
                ),
                path: owner_path.clone(),
                analysis: self.name().to_string(),
            });

            // Check against Latency property if set on the E2E flow
            // The Latency property might be on the owner component for the flow
            let owner_props = instance.properties_for(e2e.owner);
            if let Some(latency_bound) = get_timing_property(owner_props, "Latency")
                && worst_case_ps > latency_bound
            {
                let bound_ms = latency_bound as f64 / 1_000_000_000.0;
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "end-to-end flow '{}' worst-case latency {:.3} ms exceeds bound {:.3} ms",
                        e2e.name, worst_ms, bound_ms,
                    ),
                    path: owner_path,
                    analysis: self.name().to_string(),
                });
            }
        }

        // STPA-REQ-016: Multi-processor awareness — check if E2E flows cross
        // processor boundaries without communication overhead properties.
        // When a flow path crosses a processor boundary, inter-processor
        // communication adds latency that may not be captured by the basic
        // execution_time + sampling_delay model.
        let has_multi_processors = {
            let proc_count = instance
                .all_components()
                .filter(|(_, c)| {
                    matches!(
                        c.category,
                        ComponentCategory::Processor | ComponentCategory::VirtualProcessor
                    )
                })
                .count();
            proc_count >= 2
        };

        if has_multi_processors {
            for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
                let owner = instance.component(e2e.owner);
                let owner_path = component_path(instance, e2e.owner);

                // Collect processor bindings for each flow segment component
                let mut prev_binding: Option<String> = None;
                let mut crosses_boundary = false;

                for (i, seg) in e2e.segments.iter().enumerate() {
                    if i % 2 == 1 {
                        continue; // skip connection segments
                    }
                    let subcomp_name = seg.as_str().split('.').next().unwrap_or(seg.as_str());
                    let child = owner.children.iter().find(|&&child_idx| {
                        instance
                            .component(child_idx)
                            .name
                            .as_str()
                            .eq_ignore_ascii_case(subcomp_name)
                    });

                    if let Some(&child_idx) = child {
                        let child_props = instance.properties_for(child_idx);
                        let binding = get_processor_binding(child_props);

                        if let Some(ref cur) = binding
                            && let Some(ref prev) = prev_binding
                            && !cur.eq_ignore_ascii_case(prev)
                        {
                            crosses_boundary = true;
                        }
                        if binding.is_some() {
                            prev_binding = binding;
                        }
                    }
                }

                if crosses_boundary {
                    // Check if the owner has a communication overhead property
                    let owner_props = instance.properties_for(e2e.owner);
                    let has_comm_overhead = owner_props
                        .get("Timing_Properties", "Transmission_Time")
                        .is_some()
                        || owner_props.get("", "Transmission_Time").is_some()
                        || owner_props
                            .get("SPAR_Properties", "Inter_Processor_Overhead")
                            .is_some()
                        || owner_props.get("", "Inter_Processor_Overhead").is_some();

                    if !has_comm_overhead {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "end-to-end flow '{}' crosses processor boundaries but no \
                                 Transmission_Time or Inter_Processor_Overhead property is set; \
                                 latency estimate may understate actual inter-processor \
                                 communication delay",
                                e2e.name,
                            ),
                            path: owner_path,
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // STPA-REQ-017: Note modal awareness
        if !instance.system_operation_modes.is_empty() {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "{} analysis used default property values; {} system operation mode(s) exist but modal property evaluation is not yet fully supported",
                    self.name(),
                    instance.system_operation_modes.len()
                ),
                path: vec!["root".to_string()],
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
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        flow_instances: Arena<FlowInstance>,
        end_to_end_flows: Arena<EndToEndFlowInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
                property_maps: FxHashMap::default(),
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

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        fn add_connection_inst(&mut self, name: &str, owner: ComponentInstanceIdx) {
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

        fn add_e2e(&mut self, name: &str, owner: ComponentInstanceIdx, segments: Vec<&str>) {
            self.end_to_end_flows.alloc(EndToEndFlowInstance {
                name: Name::new(name),
                owner,
                segments: segments.into_iter().map(Name::new).collect(),
            });
        }

        fn set_property(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
            let map = self.property_maps.entry(comp).or_default();
            map.add(PropertyValue {
                name: PropertyRef {
                    property_set: if set.is_empty() {
                        None
                    } else {
                        Some(Name::new(set))
                    },
                    property_name: Name::new(name),
                },
                value: value.to_string(),
                is_append: false,
            });
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
                property_maps: self.property_maps,
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    #[test]
    fn latency_simple_flow_best_worst_case() {
        // sensor -> c1 -> controller -> c2 -> actuator
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor, ctrl, actuator]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);

        // E2E flow: sensor.src -> c1 -> controller.pass -> c2 -> actuator.sink
        b.add_e2e(
            "e2e_control",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );

        // Set timing properties
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        b.set_property(
            actuator,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(actuator, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report one latency range: {:?}",
            diags
        );

        // Best case: 1 + 2 + 1 = 4 ms
        assert!(
            infos[0].message.contains("4.000 ms"),
            "best case should be 4ms: {}",
            infos[0].message
        );

        // Worst case: 4 ms exec + 20 ms (controller period after c1) + 10 ms (actuator period after c2) = 34 ms
        assert!(
            infos[0].message.contains("34.000 ms"),
            "worst case should be 34ms: {}",
            infos[0].message
        );
    }

    #[test]
    fn latency_flow_missing_timing_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // Only set properties on sensor, not controller
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("without timing"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "should warn about missing timing: {:?}",
            diags
        );
        assert!(
            warnings[0].message.contains("controller"),
            "warning should mention controller: {}",
            warnings[0].message
        );
    }

    #[test]
    fn latency_exceeds_bound_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set a latency bound of 10ms on the root (owner of E2E flow)
        b.set_property(root, "Timing_Properties", "Latency", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Worst case: 5 + 5 exec + 20 sampling = 30ms > 10ms bound
        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeds bound"))
            .collect();
        assert_eq!(
            bound_warns.len(),
            1,
            "should warn about exceeding bound: {:?}",
            diags
        );
    }

    #[test]
    fn latency_no_e2e_flows_no_diags() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "no E2E flows should produce no diagnostics: {:?}",
            diags
        );
    }

    // ── STPA-REQ-016: Inter-processor communication overhead ─────

    #[test]
    fn cross_processor_flow_without_overhead_info() {
        // STPA-REQ-016: E2E flow crossing processor boundary without overhead prop
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert_eq!(
            cross_proc.len(),
            1,
            "should info about cross-processor flow: {:?}",
            diags
        );
        assert_eq!(cross_proc[0].severity, Severity::Info);
        assert!(
            cross_proc[0].message.contains("Inter_Processor_Overhead"),
            "should mention overhead property: {}",
            cross_proc[0].message
        );
    }

    #[test]
    fn same_processor_flow_no_overhead_info() {
        // No inter-processor advisory when all on same processor
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_same",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)", // Same processor
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert!(
            cross_proc.is_empty(),
            "same-processor flow should not trigger: {:?}",
            cross_proc
        );
    }

    #[test]
    fn cross_processor_flow_with_overhead_no_info() {
        // STPA-REQ-016: No info when overhead property is set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Set transmission time on owner to indicate overhead is accounted for
        b.set_property(root, "Timing_Properties", "Transmission_Time", "1 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert!(
            cross_proc.is_empty(),
            "overhead property set — no advisory: {:?}",
            cross_proc
        );
    }

    #[test]
    fn latency_single_component_flow() {
        // E2E flow with a single segment (no connections)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "2 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency"))
            .collect();
        assert_eq!(infos.len(), 1);
        // Single component, no connections: best = worst = 2ms
        assert!(
            infos[0].message.contains("2.000 ms"),
            "should be 2ms: {}",
            infos[0].message
        );
    }

    // ── Boundary tests for latency bound checking ─────────────────

    #[test]
    fn latency_exactly_at_bound_no_warning() {
        // Worst-case latency exactly equals bound — should NOT warn
        // Kills `>` → `>=` mutant on `worst_case_ps > latency_bound`
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // sensor: exec=3ms, period=10ms; controller: exec=2ms, period=20ms
        // Worst case: 3 + 2 exec + 20 sampling (controller after c1) = 25ms
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "3 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set bound exactly equal to worst case: 25ms
        b.set_property(root, "Timing_Properties", "Latency", "25 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeds bound"))
            .collect();
        assert!(
            bound_warns.is_empty(),
            "latency exactly at bound should NOT warn (only > bound): {:?}",
            bound_warns
        );
    }

    #[test]
    fn latency_one_unit_over_bound_warns() {
        // Worst-case latency is 1ms over bound — should warn.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // sensor: exec=3ms, period=10ms; controller: exec=2ms, period=20ms
        // Worst case: 3 + 2 exec + 20 sampling = 25ms
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "3 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set bound 1ms under worst case: 24ms < 25ms
        b.set_property(root, "Timing_Properties", "Latency", "24 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeds bound"))
            .collect();
        assert_eq!(
            bound_warns.len(),
            1,
            "latency 1ms over bound should warn: {:?}",
            diags
        );
        assert!(
            bound_warns[0].message.contains("25.000 ms"),
            "should show worst-case latency: {}",
            bound_warns[0].message
        );
        assert!(
            bound_warns[0].message.contains("24.000 ms"),
            "should show bound: {}",
            bound_warns[0].message
        );
    }

    #[test]
    fn latency_sampling_delay_formula() {
        // Verify that sampling delay is added for connections AFTER the first component.
        // 3-component flow: A -> c1 -> B -> c2 -> C
        // Best case = exec_A + exec_B + exec_C
        // Worst case = exec_A + exec_B + period_B + exec_C + period_C
        // (period_B added because c1 is before B, period_C because c2 is before C)
        // Sensor (first component) does NOT get sampling delay.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("comp_a", ComponentCategory::Device, Some(root));
        let bb = b.add_component("comp_b", ComponentCategory::Thread, Some(root));
        let c = b.add_component("comp_c", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![a, bb, c]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);

        b.add_e2e(
            "e2e_abc",
            root,
            vec!["comp_a.src", "c1", "comp_b.pass", "c2", "comp_c.sink"],
        );

        // A: exec=2ms, period=5ms
        b.set_property(a, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(a, "Timing_Properties", "Period", "5 ms");

        // B: exec=3ms, period=10ms
        b.set_property(bb, "Timing_Properties", "Compute_Execution_Time", "3 ms");
        b.set_property(bb, "Timing_Properties", "Period", "10 ms");

        // C: exec=1ms, period=8ms
        b.set_property(c, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(c, "Timing_Properties", "Period", "8 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Best case: 2 + 3 + 1 = 6ms
        // Worst case: 2 + 3 + 10 (B sampling) + 1 + 8 (C sampling) = 24ms
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report one latency range: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("6.000 ms"),
            "best case should be 6ms: {}",
            infos[0].message
        );
        assert!(
            infos[0].message.contains("24.000 ms"),
            "worst case should be 24ms (exec + sampling for B and C): {}",
            infos[0].message
        );
    }

    #[test]
    fn latency_within_bound_no_warning() {
        // Worst-case latency well under bound — should NOT warn
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        // Bound = 100ms, worst case = 1ms — no warning
        b.set_property(root, "Timing_Properties", "Latency", "100 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeds bound"))
            .collect();
        assert!(
            bound_warns.is_empty(),
            "latency well within bound should not warn: {:?}",
            bound_warns
        );
    }

    #[test]
    fn latency_no_sampling_delay_for_first_component() {
        // Verify the first component in a flow does NOT get sampling delay added.
        // Single component flow: best = worst = exec only, no period added.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "100 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);
        // Both best and worst case should be 5ms (period NOT added for first component)
        assert!(
            infos[0].message.contains("[5.000 ms .. 5.000 ms]"),
            "first component should not get sampling delay: {}",
            infos[0].message
        );
    }
}
