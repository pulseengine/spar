use la_arena::Arena;
use rustc_hash::FxHashMap;

use spar_hir_def::instance::{
    ComponentInstance, ComponentInstanceIdx, ConnectionEnd, ConnectionInstance,
    ConnectionInstanceIdx, FeatureInstance, FeatureInstanceIdx, SystemInstance,
};
use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind, Direction, FeatureKind};
use spar_hir_def::name::Name;
use spar_hir_def::properties::{PropertyMap, PropertyValue};

use crate::topology::{HwNode, TopologyGraph};

/// Test helper for building `SystemInstance` values in unit tests.
struct TestBuilder {
    components: Arena<ComponentInstance>,
    features: Arena<FeatureInstance>,
    connections: Arena<ConnectionInstance>,
    property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
}

impl TestBuilder {
    fn new() -> Self {
        Self {
            components: Arena::default(),
            features: Arena::default(),
            connections: Arena::default(),
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

    fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
        self.components[parent].children = children;
    }

    fn set_property(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
        let map = self.property_maps.entry(comp).or_default();
        map.add(PropertyValue {
            name: spar_hir_def::name::PropertyRef {
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
            property_maps: self.property_maps,
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn empty_system_has_no_hw_nodes() {
    let mut b = TestBuilder::new();
    // System with only software components — no processors, buses, or memory.
    let root = b.add_component("top", ComponentCategory::System, None);
    let proc = b.add_component("app", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(proc, vec![thr]);
    b.set_children(root, vec![proc]);

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    assert_eq!(topo.processor_count(), 0);
    assert_eq!(topo.bus_count(), 0);
    assert_eq!(topo.memory_count(), 0);
    assert_eq!(topo.graph.node_count(), 0);
    assert_eq!(topo.graph.edge_count(), 0);
}

#[test]
fn extracts_processors() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
    b.set_children(root, vec![cpu1, cpu2]);

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    assert_eq!(topo.processor_count(), 2);
    assert_eq!(topo.bus_count(), 0);
    assert_eq!(topo.memory_count(), 0);

    let procs = topo.processors();
    assert_eq!(procs.len(), 2);

    // Verify names.
    let names: Vec<&str> = procs.iter().map(|&ni| topo.graph[ni].name()).collect();
    assert!(names.contains(&"cpu1"));
    assert!(names.contains(&"cpu2"));

    // Verify idx_map entries.
    assert!(topo.idx_map.contains_key(&cpu1));
    assert!(topo.idx_map.contains_key(&cpu2));
}

#[test]
fn extracts_buses_and_memory() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
    let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));

    // Set properties.
    b.set_property(bus, "SEI", "Bandwidth", "100 Mbitsps");
    b.set_property(mem, "Memory_Properties", "Memory_Size", "8388608 bits"); // 1 MB = 8388608 bits

    b.set_children(root, vec![bus, mem]);

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    assert_eq!(topo.bus_count(), 1);
    assert_eq!(topo.memory_count(), 1);
    assert_eq!(topo.processor_count(), 0);

    // Verify bus properties.
    let bus_node_idx = topo.idx_map[&bus];
    match &topo.graph[bus_node_idx] {
        HwNode::Bus {
            name,
            bandwidth_bps,
            ..
        } => {
            assert_eq!(name, "eth0");
            assert!((bandwidth_bps.unwrap() - 100_000_000.0).abs() < 1.0);
        }
        other => panic!("expected Bus node, got {:?}", other),
    }

    // Verify memory properties: 8388608 bits / 8 = 1048576 bytes.
    let mem_node_idx = topo.idx_map[&mem];
    match &topo.graph[mem_node_idx] {
        HwNode::Memory {
            name, size_bytes, ..
        } => {
            assert_eq!(name, "ram");
            assert_eq!(size_bytes.unwrap(), 1_048_576);
        }
        other => panic!("expected Memory node, got {:?}", other),
    }
}

#[test]
fn extracts_mixed_topology() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let cpu2 = b.add_component("cpu2", ComponentCategory::VirtualProcessor, Some(root));
    let bus1 = b.add_component("can_bus", ComponentCategory::Bus, Some(root));
    let vbus = b.add_component("vlan1", ComponentCategory::VirtualBus, Some(root));
    let mem = b.add_component("flash", ComponentCategory::Memory, Some(root));
    // Add a non-HW component that should be excluded.
    let proc = b.add_component("app", ComponentCategory::Process, Some(root));

    b.set_children(root, vec![cpu1, cpu2, bus1, vbus, mem, proc]);

    // Add a bus access connection: cpu1 -> can_bus.
    b.add_feature("bus_acc", FeatureKind::BusAccess, None, cpu1);
    b.add_connection(
        "c1",
        ConnectionKind::Access,
        root,
        ConnectionEnd {
            subcomponent: Some(Name::new("cpu1")),
            feature: Name::new("bus_acc"),
        },
        ConnectionEnd {
            subcomponent: Some(Name::new("can_bus")),
            feature: Name::new("bus_acc"),
        },
    );

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    // 2 processors, 2 buses, 1 memory = 5 HW nodes.
    assert_eq!(topo.processor_count(), 2);
    assert_eq!(topo.bus_count(), 2);
    assert_eq!(topo.memory_count(), 1);
    assert_eq!(topo.graph.node_count(), 5);

    // Process component should NOT be in the graph.
    assert!(!topo.idx_map.contains_key(&proc));

    // cpu1 and can_bus should be connected via the access connection.
    let cpu1_node = topo.idx_map[&cpu1];
    let bus1_node = topo.idx_map[&bus1];
    assert!(topo.are_connected(cpu1_node, bus1_node));
}

#[test]
fn topology_is_deterministic() {
    // SOLVER-REQ-023: Same input must produce the same output.
    let build_instance = || {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));

        b.set_property(bus, "SEI", "Bandwidth", "1 Gbitsps");
        b.set_property(mem, "Memory_Properties", "Memory_Size", "16777216 bits");

        b.set_children(root, vec![cpu1, cpu2, bus, mem]);
        b.build(root)
    };

    let instance_a = build_instance();
    let instance_b = build_instance();

    let topo_a = TopologyGraph::from_instance(&instance_a);
    let topo_b = TopologyGraph::from_instance(&instance_b);

    // Same node and edge counts.
    assert_eq!(topo_a.graph.node_count(), topo_b.graph.node_count());
    assert_eq!(topo_a.graph.edge_count(), topo_b.graph.edge_count());
    assert_eq!(topo_a.processor_count(), topo_b.processor_count());
    assert_eq!(topo_a.bus_count(), topo_b.bus_count());
    assert_eq!(topo_a.memory_count(), topo_b.memory_count());

    // Same node names in the same order (since arenas are deterministic).
    let names_a: Vec<&str> = topo_a.graph.node_weights().map(|n| n.name()).collect();
    let names_b: Vec<&str> = topo_b.graph.node_weights().map(|n| n.name()).collect();
    assert_eq!(names_a, names_b);

    // Same property values.
    for (a, b) in topo_a.graph.node_weights().zip(topo_b.graph.node_weights()) {
        match (a, b) {
            (
                HwNode::Bus {
                    bandwidth_bps: bw_a,
                    ..
                },
                HwNode::Bus {
                    bandwidth_bps: bw_b,
                    ..
                },
            ) => {
                assert_eq!(bw_a.is_some(), bw_b.is_some());
                if let (Some(a), Some(b)) = (bw_a, bw_b) {
                    assert!((a - b).abs() < f64::EPSILON);
                }
            }
            (
                HwNode::Memory {
                    size_bytes: sz_a, ..
                },
                HwNode::Memory {
                    size_bytes: sz_b, ..
                },
            ) => {
                assert_eq!(sz_a, sz_b);
            }
            (HwNode::Processor { .. }, HwNode::Processor { .. }) => {}
            _ => panic!("node type mismatch between runs"),
        }
    }
}

#[test]
fn virtual_processor_is_extracted() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let vp = b.add_component("vp0", ComponentCategory::VirtualProcessor, Some(root));
    b.set_children(root, vec![vp]);

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    assert_eq!(topo.processor_count(), 1);
    let procs = topo.processors();
    assert_eq!(topo.graph[procs[0]].name(), "vp0");
}

#[test]
fn bus_bandwidth_property_extraction() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let bus = b.add_component("can", ComponentCategory::Bus, Some(root));
    b.set_property(bus, "Communication_Properties", "Data_Rate", "500 KBytesps");
    b.set_children(root, vec![bus]);

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    let bus_node = topo.idx_map[&bus];
    match &topo.graph[bus_node] {
        HwNode::Bus { bandwidth_bps, .. } => {
            // 500 KBytesps = 500 * 8000 = 4_000_000 bps
            assert!((bandwidth_bps.unwrap() - 4_000_000.0).abs() < 1.0);
        }
        other => panic!("expected Bus, got {:?}", other),
    }
}

#[test]
fn connection_binding_creates_edges() {
    let mut b = TestBuilder::new();
    let root = b.add_component("top", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
    b.set_children(root, vec![cpu, bus]);

    // Set Actual_Connection_Binding on the processor pointing to the bus.
    b.set_property(
        cpu,
        "Deployment_Properties",
        "Actual_Connection_Binding",
        "reference(eth0)",
    );

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    let cpu_node = topo.idx_map[&cpu];
    let bus_node = topo.idx_map[&bus];
    assert!(topo.are_connected(cpu_node, bus_node));
}

// ── Data rate parsing tests (all unit suffixes) ─────────────────────

#[test]
fn bandwidth_gbitsps() {
    let bps = crate::topology::parse_data_rate("1 Gbitsps").unwrap();
    assert!((bps - 1_000_000_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_mbitsps() {
    let bps = crate::topology::parse_data_rate("100 Mbitsps").unwrap();
    assert!((bps - 100_000_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_kbitsps() {
    let bps = crate::topology::parse_data_rate("100 Kbitsps").unwrap();
    assert!((bps - 100_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_bitsps() {
    let bps = crate::topology::parse_data_rate("9600 bitsps").unwrap();
    assert!((bps - 9600.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_gbytesps() {
    let bps = crate::topology::parse_data_rate("1 GBytesps").unwrap();
    assert!((bps - 8_000_000_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_mbytesps() {
    let bps = crate::topology::parse_data_rate("100 MBytesps").unwrap();
    assert!((bps - 800_000_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_kbytesps() {
    let bps = crate::topology::parse_data_rate("500 KBytesps").unwrap();
    assert!((bps - 4_000_000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_bytesps() {
    let bps = crate::topology::parse_data_rate("1000 Bytesps").unwrap();
    assert!((bps - 8000.0).abs() < f64::EPSILON);
}

#[test]
fn bandwidth_plain_number() {
    let bps = crate::topology::parse_data_rate("1000000").unwrap();
    assert!((bps - 1_000_000.0).abs() < f64::EPSILON);
}

// ── Impact analysis tests ───────────────────────────────────────────

#[test]
fn impact_reports_feasible_allocation() {
    use crate::allocate::Allocator;
    use crate::constraints::ModelConstraints;

    let instance = build_schedulable_system();
    let constraints = ModelConstraints::from_instance(&instance);
    let result = Allocator::ffd(&constraints);
    let impact = result.impact(&constraints);

    assert!(impact.schedulable);
    assert!(impact.deadline_violations.is_empty());
    for pi in &impact.processor_utilization {
        assert!(pi.feasible);
        assert!(pi.utilization <= 1.0);
    }
}

#[test]
fn impact_detects_overloaded_processor() {
    use crate::allocate::{AllocationResult, Binding};
    use crate::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};

    // Manually create an allocation that overloads a processor
    let mut components: Arena<ComponentInstance> = Arena::default();
    let dummy = components.alloc(ComponentInstance {
        name: Name::new("dummy"),
        category: ComponentCategory::System,
        type_name: Name::new("D"),
        impl_name: None,
        package: Name::new("Pkg"),
        parent: None,
        children: Vec::new(),
        features: Vec::new(),
        connections: Vec::new(),
        flows: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        array_index: None,
        in_modes: Vec::new(),
    });

    let constraints = ModelConstraints {
        threads: vec![
            ThreadConstraint {
                idx: dummy,
                name: "t1".to_string(),
                period_ps: 10_000_000_000,
                wcet_ps: 6_000_000_000,
                deadline_ps: 10_000_000_000,
                current_binding: None,
                priority: None,
            },
            ThreadConstraint {
                idx: dummy,
                name: "t2".to_string(),
                period_ps: 10_000_000_000,
                wcet_ps: 6_000_000_000,
                deadline_ps: 10_000_000_000,
                current_binding: None,
                priority: None,
            },
        ],
        processors: vec![ProcessorConstraint {
            idx: dummy,
            name: "cpu1".to_string(),
            memory_bytes: None,
        }],
        warnings: vec![],
    };

    // Force both threads onto one processor (120% utilization)
    let result = AllocationResult {
        bindings: vec![
            Binding {
                thread: "t1".to_string(),
                processor: "cpu1".to_string(),
                utilization: 0.6,
            },
            Binding {
                thread: "t2".to_string(),
                processor: "cpu1".to_string(),
                utilization: 0.6,
            },
        ],
        unallocated: vec![],
        per_processor_utilization: vec![("cpu1".to_string(), 1.2)],
        warnings: vec![],
    };

    let impact = result.impact(&constraints);
    assert!(!impact.schedulable);
    assert!(!impact.processor_utilization[0].feasible);
}

// ── Constraint extraction tests ──────────────────────────────────────

#[test]
fn constraints_extract_thread_timing() {
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thr]);

    b.set_property(thr, "Timing_Properties", "Period", "10 ms");
    b.set_property(
        thr,
        "Timing_Properties",
        "Compute_Execution_Time",
        "2 ms",
    );
    b.set_property(thr, "Timing_Properties", "Deadline", "5 ms");

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    let tc = &constraints.threads[0];
    assert_eq!(tc.period_ps, 10_000_000_000); // 10 ms in ps
    assert_eq!(tc.wcet_ps, 2_000_000_000); // 2 ms in ps
    assert_eq!(tc.deadline_ps, 5_000_000_000); // 5 ms in ps
    assert!(constraints.warnings.is_empty());
}

#[test]
fn constraints_missing_wcet_warns() {
    // SOLVER-REQ-020: periodic thread without WCET must produce a warning.
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thr]);

    b.set_property(thr, "Timing_Properties", "Period", "10 ms");
    // No Compute_Execution_Time set.

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    assert_eq!(constraints.threads[0].wcet_ps, 0);
    assert!(
        constraints
            .warnings
            .iter()
            .any(|w| w.contains("missing Compute_Execution_Time")),
        "expected warning about missing WCET, got: {:?}",
        constraints.warnings
    );
}

#[test]
fn constraints_missing_period_warns() {
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thr]);

    b.set_property(
        thr,
        "Timing_Properties",
        "Compute_Execution_Time",
        "2 ms",
    );
    // No Period set.

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    assert_eq!(constraints.threads[0].period_ps, 0);
    assert!(
        constraints
            .warnings
            .iter()
            .any(|w| w.contains("missing Period")),
        "expected warning about missing Period, got: {:?}",
        constraints.warnings
    );
}

#[test]
fn constraints_deadline_defaults_to_period() {
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thr]);

    b.set_property(thr, "Timing_Properties", "Period", "10 ms");
    b.set_property(
        thr,
        "Timing_Properties",
        "Compute_Execution_Time",
        "2 ms",
    );
    // No Deadline set — should default to Period.

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    let tc = &constraints.threads[0];
    assert_eq!(tc.deadline_ps, tc.period_ps);
    assert_eq!(tc.deadline_ps, 10_000_000_000);
}

#[test]
fn constraints_extract_processor() {
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    b.set_children(root, vec![cpu]);

    // 8388608 bits = 1 MB = 1_048_576 bytes
    b.set_property(cpu, "Memory_Properties", "Memory_Size", "8388608 bits");

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.processors.len(), 1);
    let pc = &constraints.processors[0];
    assert!(pc.name.contains("cpu1"));
    assert_eq!(pc.memory_bytes, Some(1_048_576));
}

#[test]
fn constraints_extract_binding() {
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let thr = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![cpu, proc]);
    b.set_children(proc, vec![thr]);

    b.set_property(thr, "Timing_Properties", "Period", "10 ms");
    b.set_property(
        thr,
        "Timing_Properties",
        "Compute_Execution_Time",
        "2 ms",
    );
    b.set_property(
        thr,
        "Deployment_Properties",
        "Actual_Processor_Binding",
        "reference(cpu1)",
    );

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    assert_eq!(
        constraints.threads[0].current_binding,
        Some("cpu1".to_string())
    );
}

#[test]
fn constraints_sorted_deterministically() {
    // SOLVER-REQ-023: threads and processors must be sorted by name.
    use crate::constraints::ModelConstraints;

    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    // Create threads in non-alphabetical order.
    let z = b.add_component("z_thread", ComponentCategory::Thread, Some(proc));
    let a = b.add_component("a_thread", ComponentCategory::Thread, Some(proc));
    let m = b.add_component("m_thread", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![z, a, m]);

    // Give all threads a Period so they don't produce warnings about missing period
    // that would clutter the assertion.
    for &thr in &[z, a, m] {
        b.set_property(thr, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            thr,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
    }

    let instance = b.build(root);
    let constraints = ModelConstraints::from_instance(&instance);

    let names: Vec<&str> = constraints.threads.iter().map(|t| t.name.as_str()).collect();
    // Names must contain the thread name as the last segment.
    assert!(names[0].ends_with("a_thread"), "first should be a_thread, got {}", names[0]);
    assert!(names[1].ends_with("m_thread"), "second should be m_thread, got {}", names[1]);
    assert!(names[2].ends_with("z_thread"), "third should be z_thread, got {}", names[2]);
}

// ── Impact analysis: deadline violation ─────────────────────────────

#[test]
fn impact_detects_deadline_violation() {
    use crate::allocate::{AllocationResult, Binding};
    use crate::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};

    // Create a dummy ComponentInstanceIdx for test structs.
    let mut components: Arena<ComponentInstance> = Arena::default();
    let dummy = components.alloc(ComponentInstance {
        name: Name::new("dummy"),
        category: ComponentCategory::System,
        type_name: Name::new("D"),
        impl_name: None,
        package: Name::new("Pkg"),
        parent: None,
        children: Vec::new(),
        features: Vec::new(),
        connections: Vec::new(),
        flows: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        array_index: None,
        in_modes: Vec::new(),
    });

    // Thread t1: period=10ms, wcet=5ms, deadline=7ms → util=0.5
    // Thread t2: period=10ms, wcet=4ms, deadline=7ms → util=0.4
    // Total utilization = 0.9
    // RMA bound for n=2 = 2*(2^(1/2)-1) ≈ 0.828
    // 0.9 > 0.828 AND deadline < period → triggers deadline violation path
    let constraints = ModelConstraints {
        threads: vec![
            ThreadConstraint {
                idx: dummy,
                name: "t1".to_string(),
                period_ps: 10_000_000_000,
                wcet_ps: 5_000_000_000,
                deadline_ps: 7_000_000_000, // deadline < period
                current_binding: None,
                priority: None,
            },
            ThreadConstraint {
                idx: dummy,
                name: "t2".to_string(),
                period_ps: 10_000_000_000,
                wcet_ps: 4_000_000_000,
                deadline_ps: 7_000_000_000, // deadline < period
                current_binding: None,
                priority: None,
            },
        ],
        processors: vec![ProcessorConstraint {
            idx: dummy,
            name: "cpu1".to_string(),
            memory_bytes: None,
        }],
        warnings: vec![],
    };

    // Force both threads onto one processor (0.9 total utilization).
    let result = AllocationResult {
        bindings: vec![
            Binding {
                thread: "t1".to_string(),
                processor: "cpu1".to_string(),
                utilization: 0.5,
            },
            Binding {
                thread: "t2".to_string(),
                processor: "cpu1".to_string(),
                utilization: 0.4,
            },
        ],
        unallocated: vec![],
        per_processor_utilization: vec![("cpu1".to_string(), 0.9)],
        warnings: vec![],
    };

    let impact = result.impact(&constraints);
    assert!(
        !impact.deadline_violations.is_empty(),
        "expected deadline violations for constrained-deadline threads exceeding RMA bound"
    );
    // Both threads have deadline < period and are on the overloaded processor.
    assert_eq!(
        impact.deadline_violations.len(),
        2,
        "both threads should have deadline violations, got: {:?}",
        impact.deadline_violations
    );
    // The allocation is still "feasible" in utilization terms (0.9 <= 1.0),
    // but not schedulable due to deadline violations.
    assert!(
        !impact.schedulable,
        "should not be schedulable with deadline violations"
    );
}

/// Build a simple schedulable system for impact tests.
fn build_schedulable_system() -> SystemInstance {
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
    let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![cpu1, cpu2, proc]);
    b.set_children(proc, vec![t1, t2]);

    b.set_property(t1, "Timing_Properties", "Period", "10 ms");
    b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "2 ms");
    b.set_property(t2, "Timing_Properties", "Period", "20 ms");
    b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "3 ms");

    b.build(root)
}
