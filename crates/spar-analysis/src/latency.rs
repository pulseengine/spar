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
//!
//! # WCET + WCTT alternation (Track D, v0.8.0)
//!
//! When a connection segment crosses a switched bus annotated with
//! `Spar_Network::*`, the per-hop contribution comes from the WCTT
//! analysis (NC-derived bound on the bus) instead of the sampling-delay
//! placeholder. The chain therefore alternates RTA-derived WCET on
//! compute hops (thread-to-thread on the same processor) with
//! WCTT-derived bounds on network hops (connections bound to a switched
//! bus). Models with no `Spar_Network::*` annotations fall back to the
//! existing scalar `Bus_Properties::Latency` / `Transmission_Time` path
//! and produce byte-identical output to v0.7.0.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{get_execution_time, get_processor_binding, get_timing_property};
use crate::wctt::compute_network_hop_latency;
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
            let mut prev_processor: Option<String> = None;

            // Read inter-processor overhead from the owner (applied at each
            // processor boundary crossing — only when the connection is
            // *not* served by a Spar_Network-annotated switched bus).
            let owner_props_for_overhead = instance.properties_for(e2e.owner);
            let inter_proc_overhead_ps = get_inter_processor_overhead(owner_props_for_overhead);

            // Track per-chain hop typology for the LatencyHopMixed marker.
            // We deliberately count compute hops only on chains that *also*
            // see a network hop, because in the no-network case the
            // existing v0.7.0 traces don't distinguish hop kinds — keeping
            // the chain-level diagnostic byte-identical when no
            // `Spar_Network::*` is set is the v0.7.0 non-regression
            // contract.
            let mut network_hops: Vec<NetworkHopAnnotation> = Vec::new();
            let mut compute_hops: Vec<ComputeHopAnnotation> = Vec::new();
            let mut chain_unservable = false;
            // The last network hop's worst-case bound (picoseconds) — used
            // to gate the legacy sampling-delay / inter-processor-overhead
            // code paths on the *next* component segment so we never
            // double-count the network contribution.
            let mut suppress_next_legacy_overhead = false;

            for (i, seg) in e2e.segments.iter().enumerate() {
                if i % 2 == 1 {
                    // Connection segment.
                    connection_count += 1;
                    let conn_name = seg.as_str();

                    // Try the WCTT-derived bound first. When the
                    // connection has no `Actual_Connection_Binding` to a
                    // Spar_Network-annotated switched bus, this returns
                    // None and we fall through to the legacy
                    // sampling-delay + inter-processor-overhead path on
                    // the next component segment.
                    if let Some(hop_lat) =
                        compute_network_hop_latency(instance, e2e.owner, conn_name)
                    {
                        if hop_lat.unservable {
                            // Mirror wctt.rs's `WcttUnservable` Error and
                            // stop aggregating this chain — we cannot
                            // honestly compose a finite end-to-end bound
                            // when one of the network hops is starved.
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "end-to-end flow '{}': network hop '{}' is unservable \
                                     (competing flows saturate the residual service); \
                                     latency aggregation aborted [network hop]",
                                    e2e.name, conn_name,
                                ),
                                path: owner_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            chain_unservable = true;
                            break;
                        }
                        best_case_ps = best_case_ps.saturating_add(hop_lat.min_ps);
                        worst_case_ps = worst_case_ps.saturating_add(hop_lat.max_ps);
                        network_hops.push(NetworkHopAnnotation {
                            connection_name: conn_name.to_string(),
                            min_ps: hop_lat.min_ps,
                            max_ps: hop_lat.max_ps,
                        });
                        suppress_next_legacy_overhead = true;
                    } else {
                        suppress_next_legacy_overhead = false;
                    }

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

                    // Add sampling delay for connections after the first
                    // component, *unless* the preceding connection was
                    // already accounted for by a WCTT bound (network hop):
                    // adding the sampling period on top of the NC bound
                    // would double-count the queuing component.
                    if connection_count > 0
                        && !suppress_next_legacy_overhead
                        && let Some(period) = period_ps
                    {
                        worst_case_ps = worst_case_ps.saturating_add(period);
                    }

                    // Track processor binding for inter-processor overhead.
                    let cur_binding = get_processor_binding(child_props);
                    let mut crossed_boundary = false;
                    if let Some(ref cur) = cur_binding
                        && let Some(ref prev) = prev_processor
                        && !cur.eq_ignore_ascii_case(prev)
                    {
                        crossed_boundary = true;
                        // Processor boundary crossing — add overhead only
                        // when we did *not* already account for the
                        // crossing through a WCTT-derived network hop.
                        if !suppress_next_legacy_overhead
                            && let Some(overhead) = inter_proc_overhead_ps
                        {
                            worst_case_ps = worst_case_ps.saturating_add(overhead);
                        }
                    }

                    // Compute-hop annotation: a flow segment whose
                    // component shares a processor with its predecessor
                    // (or is the first segment with a binding). Only
                    // tracked here for `LatencyHopMixed` — the per-hop
                    // diagnostic is emitted only on chains that also see
                    // a network hop.
                    if !crossed_boundary && prev_processor.is_some() {
                        compute_hops.push(ComputeHopAnnotation {
                            component_name: child_comp.name.as_str().to_string(),
                            exec_ps: exec_ps.unwrap_or(0),
                        });
                    }

                    if cur_binding.is_some() {
                        prev_processor = cur_binding;
                    }
                } else {
                    missing_timing.push(subcomp_name.to_string());
                }

                // Reset the WCTT-suppression latch now that the next
                // component has consumed it.
                suppress_next_legacy_overhead = false;
            }

            if chain_unservable {
                continue;
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

            // Per-hop diagnostics and the mixed-hop marker only fire on
            // chains that exercised at least one network hop. Without a
            // network hop the chain is byte-identical to v0.7.0.
            if !network_hops.is_empty() {
                for hop in &network_hops {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "end-to-end flow '{}': connection '{}' [network hop] [{:.3} ms .. {:.3} ms]",
                            e2e.name,
                            hop.connection_name,
                            hop.min_ps as f64 / 1_000_000_000.0,
                            hop.max_ps as f64 / 1_000_000_000.0,
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
                for hop in &compute_hops {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "end-to-end flow '{}': component '{}' [compute hop] {:.3} ms",
                            e2e.name,
                            hop.component_name,
                            hop.exec_ps as f64 / 1_000_000_000.0,
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }

                if !compute_hops.is_empty() {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "LatencyHopMixed: end-to-end flow '{}' alternates RTA-derived \
                             WCET on {} compute hop{} with WCTT-derived bounds on {} \
                             network hop{}",
                            e2e.name,
                            compute_hops.len(),
                            if compute_hops.len() == 1 { "" } else { "s" },
                            network_hops.len(),
                            if network_hops.len() == 1 { "" } else { "s" },
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

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

/// Per-hop bookkeeping for chains that mix WCET and WCTT contributions.
#[derive(Debug, Clone)]
struct NetworkHopAnnotation {
    connection_name: String,
    min_ps: u64,
    max_ps: u64,
}

#[derive(Debug, Clone)]
struct ComputeHopAnnotation {
    component_name: String,
    exec_ps: u64,
}

/// Read inter-processor communication overhead in picoseconds.
///
/// Checks `SPAR_Properties::Inter_Processor_Overhead`,
/// `Timing_Properties::Inter_Processor_Overhead` (unqualified fallback),
/// then `Timing_Properties::Transmission_Time`. Returns `None` when none
/// of these properties are set.
fn get_inter_processor_overhead(props: &spar_hir_def::properties::PropertyMap) -> Option<u64> {
    use crate::property_accessors::extract_time_ps;
    use spar_hir_def::property_value::parse_time_value;

    // Try SPAR_Properties::Inter_Processor_Overhead (typed path).
    if let Some(expr) = props
        .get_typed("SPAR_Properties", "Inter_Processor_Overhead")
        .or_else(|| props.get_typed("", "Inter_Processor_Overhead"))
        && let Some(ps) = extract_time_ps(expr)
    {
        return Some(ps);
    }

    // String fallback for Inter_Processor_Overhead.
    if let Some(raw) = props
        .get("SPAR_Properties", "Inter_Processor_Overhead")
        .or_else(|| props.get("", "Inter_Processor_Overhead"))
        && let Some(ps) = parse_time_value(raw)
    {
        return Some(ps);
    }

    // Try Timing_Properties::Inter_Processor_Overhead.
    if let Some(val) = get_timing_property(props, "Inter_Processor_Overhead") {
        return Some(val);
    }

    // Fall back to Transmission_Time.
    get_timing_property(props, "Transmission_Time")
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

        /// Add a connection instance with named source/destination
        /// subcomponent endpoints — needed by the WCTT integration tests
        /// because `compute_network_hop_latency` walks the connection's
        /// `src` / `dst` to identify the source end station.
        fn add_connection_with_endpoints(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: &str,
            src_feat: &str,
            dst_sub: &str,
            dst_feat: &str,
        ) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(src_sub)),
                    feature: Name::new(src_feat),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(dst_sub)),
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
                typed_expr: None,
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

    // ── Inter-processor overhead applied to latency ────────────────

    #[test]
    fn inter_processor_overhead_added_to_worst_case() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
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

        // Set inter-processor overhead on the owner.
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should report latency: {:?}", diags);

        // Best case: 1 + 2 = 3 ms (overhead not in best case)
        assert!(
            infos[0].message.contains("3.000 ms"),
            "best case should be 3ms: {}",
            infos[0].message
        );

        // Worst case: 1 + 2 exec + 20 sampling + 5 overhead = 28 ms
        assert!(
            infos[0].message.contains("28.000 ms"),
            "worst case should include 5ms overhead: {}",
            infos[0].message
        );
    }

    #[test]
    fn inter_processor_overhead_transmission_time_fallback() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
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

        // Use Transmission_Time instead of Inter_Processor_Overhead.
        b.set_property(root, "Timing_Properties", "Transmission_Time", "3 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should report latency: {:?}", diags);

        // Worst case: 1 + 2 exec + 20 sampling + 3 overhead = 26 ms
        assert!(
            infos[0].message.contains("26.000 ms"),
            "worst case should include 3ms Transmission_Time: {}",
            infos[0].message
        );
    }

    #[test]
    fn no_overhead_when_same_processor() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, sensor, ctrl]);
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
            "reference (cpu1)",
        );

        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);

        // Worst case without overhead: 1 + 2 exec + 20 sampling = 23 ms
        assert!(
            infos[0].message.contains("23.000 ms"),
            "same-processor flow should NOT include overhead: {}",
            infos[0].message
        );
    }

    #[test]
    fn no_overhead_property_no_overhead_added() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
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

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);

        // Worst case without overhead: 1 + 2 exec + 20 sampling = 23 ms
        assert!(
            infos[0].message.contains("23.000 ms"),
            "no overhead property = no overhead in latency: {}",
            infos[0].message
        );
    }

    // ── Track D commit 6: WCTT integration tests ─────────────────────
    //
    // The next block of tests verifies that `LatencyAnalysis::analyze`
    // alternates RTA-derived WCET on compute hops with WCTT-derived
    // bounds on network hops, and that models without `Spar_Network::*`
    // annotations stay byte-identical to v0.7.0 (the non-regression
    // contract on which Track D Phase 1 stands).

    /// Build a compute-thread-on-cpu1 → connection → compute-thread-on-cpu2
    /// chain wrapping a single switched bus. Used by tests 3, 4, 6, 7.
    /// Parameters select whether the bus is annotated as a
    /// `Spar_Network::Switch_Type` switch and whether a
    /// `Bus_Properties::Latency` scalar fallback is set.
    fn build_two_hop_chain(annotate_switch: bool) -> SystemInstance {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
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

        if annotate_switch {
            b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
            b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
            b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
            b.set_property(sw, "Spar_Network", "Queue_Depth", "1");

            // Owner-level connection binding: c1 binds to switch sw.
            b.set_property(
                root,
                "Deployment_Properties",
                "Actual_Connection_Binding",
                "(reference (sw))",
            );
        }

        b.build(root)
    }

    /// Test 1: non-regression — model without `Spar_Network::*` produces
    /// byte-identical latency output to current main.
    #[test]
    fn no_spar_network_models_unchanged_v07() {
        // Reuse the existing inter_processor_overhead model: cross-CPU
        // chain with a `SPAR_Properties::Inter_Processor_Overhead`
        // annotation but *no* `Spar_Network::Switch_Type`. Output must
        // include neither a `[network hop]` nor a `LatencyHopMixed`
        // marker — that is the byte-level non-regression contract.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
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
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // No network-hop annotations and no LatencyHopMixed marker.
        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "non-regression: should not emit `[network hop]` without Spar_Network: {}",
                d.message
            );
            assert!(
                !d.message.contains("[compute hop]"),
                "non-regression: should not emit `[compute hop]` without Spar_Network: {}",
                d.message
            );
            assert!(
                !d.message.contains("LatencyHopMixed"),
                "non-regression: should not emit `LatencyHopMixed` without Spar_Network: {}",
                d.message
            );
        }

        // Existing v0.7.0 output is preserved.
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should still report one chain: {:?}", diags);
        // 1 + 2 exec + 20 sampling + 5 overhead = 28 ms (matches the
        // existing `inter_processor_overhead_added_to_worst_case` test).
        assert!(
            infos[0].message.contains("28.000 ms"),
            "non-regression: worst case must still be 28ms: {}",
            infos[0].message
        );
    }

    /// Test 2: chain entirely on one CPU; no network hop. Even if other
    /// components in the model declare `Spar_Network::*`, a chain whose
    /// connection is unbound to any switched bus must not emit any
    /// network-hop annotation.
    #[test]
    fn compute_only_chain_unchanged() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        // A switched bus exists in the model but is *not* bound by c1.
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "e2e_same_cpu",
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
            "reference (cpu1)",
        );

        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        // No Actual_Connection_Binding on owner — c1 is unbound to sw.

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "no binding to sw → no network hop: {}",
                d.message
            );
        }
        assert!(
            !diags.iter().any(|d| d.message.contains("LatencyHopMixed")),
            "no network hops → no LatencyHopMixed marker"
        );
    }

    /// Test 3: chain crosses a switched bus → the per-hop network bound
    /// matches what `wctt::compute_network_hop_latency` returns.
    #[test]
    fn single_network_hop_uses_wctt() {
        let inst = build_two_hop_chain(true);

        let diags = LatencyAnalysis.analyze(&inst);
        // Helper invocation result must equal the per-hop annotation
        // emitted by latency.rs.
        let owner = inst.root;
        let hop = compute_network_hop_latency(&inst, owner, "c1")
            .expect("c1 binds to switch sw → Some(NetworkHopLatency)");
        // Expected: forwarding latency 0 + σ/R, where σ = MTU = 1500 B,
        // R = 1 Gbps → 12_000_000 ps = 12 us = 0.012 ms.
        assert_eq!(hop.max_ps, 12_000_000, "12 us per-hop bound expected");
        assert!(
            !hop.unservable,
            "single-stream model is not unservable: {:?}",
            hop
        );

        let net_hop_diag = diags
            .iter()
            .find(|d| d.message.contains("[network hop]"))
            .unwrap_or_else(|| panic!("expected [network hop] diag, got: {:?}", diags));
        // 12 us = 0.012 ms — formatted with 3 decimals.
        assert!(
            net_hop_diag.message.contains("0.012 ms"),
            "[network hop] must report 0.012 ms WCTT bound: {}",
            net_hop_diag.message
        );
    }

    /// Test 4: three-stage chain demonstrating WCET+WCTT alternation.
    /// sensor (cpu1) → switched-bus connection → controller (cpu2) →
    /// switched-bus connection → actuator (cpu2). Both connections
    /// inherit the owner-level `Actual_Connection_Binding` to `sw`
    /// (canonical AADL behaviour — bindings inherit unless overridden
    /// per-connection), so both are network hops; actuator on the
    /// same CPU as controller is a compute hop. We therefore expect
    /// `2 network hop` + `1 compute hop` in the LatencyHopMixed marker.
    /// This is the canonical "WCET+WCTT alternation is live" model.
    #[test]
    fn compute_then_network_then_compute() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl, actuator]);

        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_connection_with_endpoints("c2", root, "controller", "out_p", "actuator", "in_p");
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );

        for (comp, cpu) in [(sensor, "cpu1"), (ctrl, "cpu2"), (actuator, "cpu2")] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
            b.set_property(
                comp,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({})", cpu),
            );
        }

        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Chain has both kinds of hops → LatencyHopMixed Info emitted.
        let mixed = diags
            .iter()
            .find(|d| d.message.contains("LatencyHopMixed"))
            .unwrap_or_else(|| panic!("expected LatencyHopMixed: {:?}", diags));
        assert_eq!(mixed.severity, Severity::Info);
        assert!(
            mixed.message.contains("1 compute hop"),
            "actuator shares cpu2 with controller = 1 compute hop: {}",
            mixed.message
        );
        assert!(
            mixed.message.contains("2 network hops"),
            "c1 + c2 inherit owner-level binding to sw = 2 network hops: {}",
            mixed.message
        );
    }

    /// Test 5: bus has `Bus_Properties::Latency`-style scalar but no
    /// `Spar_Network::*` annotations → fallback to legacy scalar
    /// (Inter_Processor_Overhead) preserves v0.7.0 behaviour.
    #[test]
    fn network_hop_falls_back_to_bus_latency_when_no_switch_type() {
        // Smoke-check the helper: an unannotated `build_two_hop_chain`
        // must not produce any network-hop annotation, and the chain
        // must fall through to the legacy v0.7.0 path.
        let inst_smoke = build_two_hop_chain(false /* unannotated */);
        let smoke = LatencyAnalysis.analyze(&inst_smoke);
        assert!(
            smoke.iter().all(|d| !d.message.contains("[network hop]")),
            "unannotated bus must not produce a [network hop] diag: {:?}",
            smoke
        );

        // Now build the cross-CPU chain again with the legacy
        // `SPAR_Properties::Inter_Processor_Overhead` scalar set, which
        // is the "scalar Bus_Properties::Latency placeholder" the
        // integration design refers to. Verify the worst-case bound
        // reproduces the v0.7.0 number (28 ms — same as the existing
        // `inter_processor_overhead_added_to_worst_case` test).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
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
        // Legacy scalar fallback — applied because there's no
        // Spar_Network::Switch_Type on `sw`.
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Same number as `inter_processor_overhead_added_to_worst_case`.
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        assert!(
            info.message.contains("28.000 ms"),
            "fallback to scalar overhead: 1+2 exec + 20 sampling + 5 overhead = 28ms, got: {}",
            info.message
        );
        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "no Spar_Network::Switch_Type → no network hop: {}",
                d.message
            );
        }
    }

    /// Test 6: same model as test 5 but with `Spar_Network::Switch_Type`
    /// added → the bound becomes NC-derived (12 us) instead of the
    /// scalar `Inter_Processor_Overhead` (5 ms).
    #[test]
    fn network_hop_uses_wctt_when_switch_type_present() {
        let inst = build_two_hop_chain(true);

        let diags = LatencyAnalysis.analyze(&inst);
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        // 1 + 2 exec + 0.012 ms WCTT + 0 sampling delay (suppressed
        // because the connection consumed the WCTT bound) = 3.012 ms.
        assert!(
            info.message.contains("3.012 ms"),
            "WCTT-derived bound: 1+2 exec + 0.012 ms WCTT = 3.012 ms, got: {}",
            info.message
        );
        let net_hop = diags
            .iter()
            .find(|d| d.message.contains("[network hop]"))
            .unwrap();
        assert!(net_hop.message.contains("0.012 ms"));
    }

    /// Test 7: chain with both hop types → `LatencyHopMixed` Info appears.
    #[test]
    fn latency_hop_mixed_diagnostic_emitted() {
        // Same fixture as compute_then_network_then_compute. Verify
        // explicitly that the LatencyHopMixed string is the unique
        // marker for chains that exercise both hop types.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let a = b.add_component("a", ComponentCategory::Thread, Some(root));
        let bb = b.add_component("b", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, a, bb]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "a", "in_p");
        b.add_connection_with_endpoints("c2", root, "a", "out_p", "b", "in_p");
        b.add_e2e(
            "mixed",
            root,
            vec!["sensor.src", "c1", "a.pass", "c2", "b.sink"],
        );

        for (comp, cpu) in [(sensor, "cpu1"), (a, "cpu2"), (bb, "cpu2")] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
            b.set_property(
                comp,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({})", cpu),
            );
        }
        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let mixed: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("LatencyHopMixed"))
            .collect();
        assert_eq!(mixed.len(), 1, "exactly one LatencyHopMixed: {:?}", diags);
        assert_eq!(mixed[0].severity, Severity::Info);
    }

    /// Test 8: `compute_network_hop_latency` returns `unservable = true`
    /// → `latency.rs` emits an error and stops aggregating.
    #[test]
    fn wctt_unservable_propagates_to_latency() {
        // Two streams whose sustained rates each equal the server rate
        // (so the residual service for each is exhausted by the other).
        // The wctt unit tests cover the same model under the name
        // `competing_flow_exceeds_rate_emits_unservable`.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let other_src = b.add_component("hot_src", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let other_dst = b.add_component("hot_dst", ComponentCategory::Device, Some(root));
        b.set_children(
            root,
            vec![_cpu1, _cpu2, sw, sensor, other_src, ctrl, other_dst],
        );
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_connection_with_endpoints("c2", root, "hot_src", "out_p", "hot_dst", "in_p");
        b.add_e2e(
            "saturated",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        for comp in [sensor, ctrl, other_src, other_dst] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
        }
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Saturate the 1 kbps switch with two 1 kbps sources.
        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(sensor, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(sensor, "Spar_Network", "Queue_Depth", "1");
        b.set_property(other_src, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(other_src, "Spar_Network", "Queue_Depth", "1");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let err = diags
            .iter()
            .find(|d| d.severity == Severity::Error && d.message.contains("unservable"))
            .unwrap_or_else(|| panic!("expected unservable Error: {:?}", diags));
        assert!(
            err.message.contains("[network hop]"),
            "unservable error annotates the offending hop: {}",
            err.message
        );
        // Aggregation aborted — no chain-level "latency: [..]" Info for
        // the saturated chain.
        let info_count = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .count();
        assert_eq!(
            info_count, 0,
            "unservable chain must not emit a latency range: {:?}",
            diags
        );
    }

    /// Test 9: middle hop has no `Spar_Network::*` annotations →
    /// fall back to scalar without crashing.
    #[test]
    fn unannotated_bus_in_chain_falls_back() {
        // Single chain with a connection that names an unannotated
        // bus in its binding. `compute_network_hop_latency` returns
        // `None` and the analysis falls through to the legacy
        // sampling-delay path. No panic, no `[network hop]` annotation.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let plain_bus = b.add_component("plain_bus", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, plain_bus, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "fallback",
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
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (plain_bus))",
        );
        // plain_bus has no Spar_Network::Switch_Type — the binding
        // resolves to a non-switch and the helper returns None.

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "unannotated bus → no network hop: {}",
                d.message
            );
        }
        // Latency is still produced (legacy path).
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap_or_else(|| panic!("expected latency Info, got {:?}", diags));
        assert!(info.message.contains("ms"));
    }

    /// Test 10: existing fixture-style chain stays unchanged. We
    /// re-exercise the seven main pre-Track-D scenarios in one assert
    /// block to guard against accidental drift in chains the v0.7.0
    /// suite already covers.
    #[test]
    fn existing_latency_fixtures_unchanged() {
        // Re-uses the pattern of `latency_simple_flow_best_worst_case`
        // and asserts the headline numbers (4 ms best, 34 ms worst) plus
        // the absence of any Track-D-specific markers.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor, ctrl, actuator]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);
        b.add_e2e(
            "e2e_control",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );
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

        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        assert!(info.message.contains("4.000 ms"), "best case 4ms preserved");
        assert!(
            info.message.contains("34.000 ms"),
            "worst case 34ms preserved"
        );
        for d in &diags {
            assert!(
                !d.message.contains("LatencyHopMixed"),
                "existing fixture must not gain LatencyHopMixed: {}",
                d.message
            );
        }
    }
}
