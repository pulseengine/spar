//! Tests for constraint extraction from AADL instance model properties.

use la_arena::Arena;
use rustc_hash::FxHashMap;

use spar_hir_def::instance::*;
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::{Name, PropertyRef};
use spar_hir_def::properties::{PropertyMap, PropertyValue};

use crate::constraints::ModelConstraints;

// ── Test builder ────────────────────────────────────────────────────

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

    fn set_property(
        &mut self,
        comp: ComponentInstanceIdx,
        set: &str,
        name: &str,
        value: &str,
    ) {
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

// ── Tests ───────────────────────────────────────────────────────────

#[test]
fn extract_thread_timing() {
    // Thread with all timing properties should extract correctly.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("sensor_reader", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![t1]);

    b.set_property(t1, "Timing_Properties", "Period", "10 ms");
    b.set_property(
        t1,
        "Timing_Properties",
        "Compute_Execution_Time",
        "1 ms .. 3 ms",
    );
    b.set_property(t1, "Timing_Properties", "Deadline", "8 ms");
    b.set_property(t1, "Deployment_Properties", "Priority", "5");

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.threads.len(), 1);
    let tc = &mc.threads[0];
    assert_eq!(tc.period_ps, 10_000_000_000); // 10 ms
    assert_eq!(tc.wcet_ps, 3_000_000_000); // worst-case 3 ms
    assert_eq!(tc.deadline_ps, 8_000_000_000); // 8 ms
    assert_eq!(tc.priority, Some(5));
    assert!(tc.current_binding.is_none());
    assert!(mc.warnings.is_empty(), "no warnings expected: {:?}", mc.warnings);
}

#[test]
fn extract_thread_with_binding() {
    // Thread already bound to a processor should have current_binding set.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![cpu, proc]);
    b.set_children(proc, vec![t1]);

    b.set_property(t1, "Timing_Properties", "Period", "20 ms");
    b.set_property(
        t1,
        "Timing_Properties",
        "Compute_Execution_Time",
        "5 ms",
    );
    b.set_property(
        t1,
        "Deployment_Properties",
        "Actual_Processor_Binding",
        "reference (cpu1)",
    );

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.threads.len(), 1);
    assert_eq!(mc.threads[0].current_binding.as_deref(), Some("cpu1"));
    assert!(mc.warnings.is_empty());
}

#[test]
fn missing_wcet_produces_warning() {
    // SOLVER-REQ-020: thread with Period but no WCET must warn.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("incomplete", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![t1]);

    b.set_property(t1, "Timing_Properties", "Period", "50 ms");
    // No Compute_Execution_Time set.

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.threads.len(), 1);
    assert_eq!(mc.threads[0].wcet_ps, 0);

    let wcet_warnings: Vec<_> = mc
        .warnings
        .iter()
        .filter(|w| w.contains("Compute_Execution_Time"))
        .collect();
    assert_eq!(
        wcet_warnings.len(),
        1,
        "expected exactly one WCET warning: {:?}",
        mc.warnings,
    );
    assert!(
        wcet_warnings[0].contains("UNSAFE"),
        "warning must flag as UNSAFE: {}",
        wcet_warnings[0],
    );
}

#[test]
fn missing_period_produces_warning() {
    // Thread without Period should warn about missing scheduling info.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("sporadic", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![t1]);

    // No properties at all.
    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.threads.len(), 1);
    assert_eq!(mc.threads[0].period_ps, 0);

    let period_warnings: Vec<_> = mc
        .warnings
        .iter()
        .filter(|w| w.contains("missing Period"))
        .collect();
    assert_eq!(
        period_warnings.len(),
        1,
        "expected one Period warning: {:?}",
        mc.warnings,
    );
}

#[test]
fn extract_processor_constraints() {
    // Processor with Memory_Size should extract memory_bytes.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    b.set_children(root, vec![cpu]);

    // 256 KByte = 256 * 1024 bytes = 262144 bytes
    b.set_property(cpu, "Memory_Properties", "Memory_Size", "256 KByte");

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.processors.len(), 1);
    assert_eq!(mc.processors[0].name, "root.cpu1");
    // get_size_property returns bits: 256 * 8 * 1024 = 2_097_152 bits
    // We convert to bytes: 2_097_152 / 8 = 262_144 bytes
    assert_eq!(mc.processors[0].memory_bytes, Some(262_144));
}

#[test]
fn constraints_are_sorted_deterministically() {
    // SOLVER-REQ-023: output must be sorted by component name.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    // Add threads in reverse alphabetical order.
    let t_z = b.add_component("zebra", ComponentCategory::Thread, Some(proc));
    let t_a = b.add_component("alpha", ComponentCategory::Thread, Some(proc));
    let t_m = b.add_component("middle", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![t_z, t_a, t_m]);

    // Give all threads Period + WCET to avoid warnings cluttering the test.
    for t in [t_z, t_a, t_m] {
        b.set_property(t, "Timing_Properties", "Period", "10 ms");
        b.set_property(t, "Timing_Properties", "Compute_Execution_Time", "1 ms");
    }

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    let names: Vec<&str> = mc.threads.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["root.proc.alpha", "root.proc.middle", "root.proc.zebra"],
        "threads must be sorted by name",
    );
}

#[test]
fn deadline_defaults_to_period() {
    // When no Deadline is set, it should default to the Period value.
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("worker", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![t1]);

    b.set_property(t1, "Timing_Properties", "Period", "25 ms");
    b.set_property(
        t1,
        "Timing_Properties",
        "Compute_Execution_Time",
        "2 ms",
    );
    // No Deadline property set.

    let inst = b.build(root);
    let mc = ModelConstraints::from_instance(&inst);

    assert_eq!(mc.threads.len(), 1);
    // Deadline should equal Period (25 ms = 25_000_000_000 ps).
    assert_eq!(mc.threads[0].deadline_ps, 25_000_000_000);
    assert_eq!(mc.threads[0].period_ps, mc.threads[0].deadline_ps);
    assert!(mc.warnings.is_empty(), "no warnings expected: {:?}", mc.warnings);
}
