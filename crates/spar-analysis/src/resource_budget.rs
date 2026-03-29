//! Resource budget analysis (memory, MIPS, bandwidth).
//!
//! Checks that memory, processing, and communication resources
//! are not exceeded:
//!
//! - **Memory budget**: For each memory component, sums the memory demands
//!   (Source_Code_Size + Data_Size + Stack_Size) of bound software components
//!   and compares against Memory_Size capacity.
//! - **Bandwidth budget**: For each bus, checks if total Data_Rate of bound
//!   connections exceeds bus bandwidth.
//!
//! This is a v1 implementation focusing on memory budgets since those
//! properties are most commonly specified in AADL models.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{get_memory_binding, get_size_property};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Resource budget analysis (memory, MIPS, bandwidth).
pub struct ResourceBudgetAnalysis;

impl Analysis for ResourceBudgetAnalysis {
    fn name(&self) -> &str {
        "resource_budget"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — memory budget exceeded
        //   Warning — bus bandwidth exceeded
        //   Info    — memory utilization within budget, modal awareness note
        let mut diags = Vec::new();

        check_memory_budgets(instance, &mut diags);
        check_bandwidth_budgets(instance, &mut diags);

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

/// Check memory budgets: compare software memory demands against memory capacity.
fn check_memory_budgets(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    for (mem_idx, mem_comp) in instance.all_components() {
        if mem_comp.category != ComponentCategory::Memory {
            continue;
        }

        let mem_props = instance.properties_for(mem_idx);
        let mem_path = component_path(instance, mem_idx);

        // Get memory capacity (Memory_Size property)
        let capacity_bits = get_size_property(mem_props, "Memory_Size");

        if capacity_bits.is_none() {
            continue; // No capacity specified, skip budget check
        }
        let capacity_bits = capacity_bits.unwrap();

        // Find all software components bound to this memory
        let mut total_demand_bits: u64 = 0;
        let mut bound_components: Vec<(String, u64)> = Vec::new();

        for (comp_idx, _comp) in instance.all_components() {
            let comp_props = instance.properties_for(comp_idx);

            // Check if this component is bound to this memory
            let binding = get_memory_binding(comp_props);
            if let Some(ref target) = binding {
                if !target.eq_ignore_ascii_case(mem_comp.name.as_str()) {
                    continue;
                }
            } else {
                continue;
            }

            let demand = compute_memory_demand(comp_props);
            if demand > 0 {
                let comp = instance.component(comp_idx);
                bound_components.push((comp.name.as_str().to_string(), demand));
                total_demand_bits = total_demand_bits.saturating_add(demand);
            }
        }

        if bound_components.is_empty() {
            continue; // No bound components to check
        }

        let demand_kb = total_demand_bits as f64 / (8.0 * 1024.0);
        let capacity_kb = capacity_bits as f64 / (8.0 * 1024.0);

        if total_demand_bits > capacity_bits {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "memory '{}' budget exceeded: {:.1} KB used of {:.1} KB capacity \
                     ({} bound components, {:.1}% utilization)",
                    mem_comp.name,
                    demand_kb,
                    capacity_kb,
                    bound_components.len(),
                    (total_demand_bits as f64 / capacity_bits as f64) * 100.0,
                ),
                path: mem_path.clone(),
                analysis: "resource_budget".to_string(),
            });
        } else {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "memory '{}' utilization: {:.1} KB of {:.1} KB ({:.1}%, {} bound components)",
                    mem_comp.name,
                    demand_kb,
                    capacity_kb,
                    (total_demand_bits as f64 / capacity_bits as f64) * 100.0,
                    bound_components.len(),
                ),
                path: mem_path,
                analysis: "resource_budget".to_string(),
            });
        }
    }
}

/// Check bandwidth budgets: compare connection data rates against bus capacity.
fn check_bandwidth_budgets(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    for (bus_idx, bus_comp) in instance.all_components() {
        if bus_comp.category != ComponentCategory::Bus
            && bus_comp.category != ComponentCategory::VirtualBus
        {
            continue;
        }

        let bus_props = instance.properties_for(bus_idx);
        let bus_path = component_path(instance, bus_idx);

        // Get bus bandwidth capacity (Data_Rate property)
        let capacity_raw = bus_props
            .get("Communication_Properties", "Data_Rate")
            .or_else(|| bus_props.get("", "Data_Rate"));

        if capacity_raw.is_none() {
            continue; // No bandwidth specified, skip
        }
        let capacity_raw = capacity_raw.unwrap();
        let capacity_bps = parse_data_rate(capacity_raw);

        if capacity_bps.is_none() {
            continue;
        }
        let capacity_bps = capacity_bps.unwrap();

        // Find connections bound to this bus
        let mut total_rate: f64 = 0.0;
        let mut connection_count = 0;

        // Check if any components reference this bus in their connection binding
        for (comp_idx, _comp) in instance.all_components() {
            let comp_props = instance.properties_for(comp_idx);

            // Check Actual_Connection_Binding for this bus
            let binding = comp_props
                .get("Deployment_Properties", "Actual_Connection_Binding")
                .or_else(|| comp_props.get("", "Actual_Connection_Binding"));

            if let Some(binding_val) = binding
                && binding_val
                    .to_lowercase()
                    .contains(&bus_comp.name.as_str().to_lowercase())
            {
                // This component's connections use this bus
                if let Some(rate_raw) = comp_props
                    .get("Communication_Properties", "Data_Rate")
                    .or_else(|| comp_props.get("", "Data_Rate"))
                    && let Some(rate) = parse_data_rate(rate_raw)
                {
                    total_rate += rate;
                    connection_count += 1;
                }
            }
        }

        if connection_count == 0 {
            continue;
        }

        if total_rate > capacity_bps {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "bus '{}' bandwidth may be exceeded: {:.1} bps demanded vs {:.1} bps capacity \
                     ({} connections)",
                    bus_comp.name, total_rate, capacity_bps, connection_count,
                ),
                path: bus_path,
                analysis: "resource_budget".to_string(),
            });
        }
    }
}

/// Compute total memory demand from Source_Code_Size + Data_Size + Stack_Size (in bits).
fn compute_memory_demand(props: &spar_hir_def::properties::PropertyMap) -> u64 {
    let code_size = get_size_property(props, "Source_Code_Size").unwrap_or(0);
    let data_size = get_size_property(props, "Data_Size").unwrap_or(0);
    let stack_size = get_size_property(props, "Stack_Size").unwrap_or(0);
    code_size
        .saturating_add(data_size)
        .saturating_add(stack_size)
}

/// Parse a data rate value string like "100 KBytesps" into bits per second.
///
/// Supports common formats: "N bitsps", "N KBytesps", "N MBytesps", "N Bytesps"
/// or just a numeric value assumed to be in bps.
fn parse_data_rate(s: &str) -> Option<f64> {
    let s = s.trim();
    // Try common AADL data rate units
    for &(suffix, factor) in DATA_RATE_UNITS {
        if let Some(num_str) = s.strip_suffix(suffix).map(|s| s.trim())
            && let Ok(val) = num_str.parse::<f64>()
        {
            return Some(val * factor);
        }
    }
    // Try plain number (assume bps)
    s.parse::<f64>().ok()
}

/// Data rate units and their conversion factors to bits per second.
const DATA_RATE_UNITS: &[(&str, f64)] = &[
    ("Gbitsps", 1_000_000_000.0),
    ("Mbitsps", 1_000_000.0),
    ("Kbitsps", 1_000.0),
    ("bitsps", 1.0),
    ("GBytesps", 8_000_000_000.0),
    ("MBytesps", 8_000_000.0),
    ("KBytesps", 8_000.0),
    ("Bytesps", 8.0),
];

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

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

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
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

    #[test]
    fn memory_budget_within_capacity() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Memory capacity: 1 MByte
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 MByte");

        // Thread demands: 100 KByte code + 50 KByte data + 10 KByte stack = 160 KByte
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "100 KByte");
        b.set_property(thread, "Memory_Properties", "Data_Size", "50 KByte");
        b.set_property(thread, "Memory_Properties", "Stack_Size", "10 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should be within budget: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report memory utilization: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("ram"),
            "should mention ram: {}",
            infos[0].message
        );
    }

    #[test]
    fn memory_budget_exceeded() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![t1, t2]);

        // Memory capacity: 100 KByte
        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");

        // t1: 60 KByte code
        b.set_property(t1, "Memory_Properties", "Source_Code_Size", "60 KByte");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        // t2: 60 KByte code -> total 120 KByte > 100 KByte
        b.set_property(t2, "Memory_Properties", "Source_Code_Size", "60 KByte");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "should error on exceeded budget: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("exceeded"),
            "should mention exceeded: {}",
            errors[0].message
        );
    }

    #[test]
    fn memory_no_capacity_no_check() {
        // Memory without Memory_Size property should not produce diagnostics
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        b.set_children(root, vec![mem]);

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let mem_diags: Vec<_> = diags.iter().filter(|d| d.message.contains("ram")).collect();
        assert!(
            mem_diags.is_empty(),
            "no capacity = no check: {:?}",
            mem_diags
        );
    }

    #[test]
    fn parse_data_rate_values() {
        assert_eq!(parse_data_rate("100 Kbitsps"), Some(100_000.0));
        assert_eq!(parse_data_rate("1 Mbitsps"), Some(1_000_000.0));
        assert_eq!(parse_data_rate("10 KBytesps"), Some(80_000.0));
        assert_eq!(parse_data_rate("1000"), Some(1000.0));
        assert_eq!(parse_data_rate("invalid"), None);
    }

    // ── Boundary tests (kill > vs >= mutants) ─────────────────────

    #[test]
    fn memory_budget_exactly_at_capacity() {
        // Demand == capacity must NOT error (boundary: > not >=).
        // 100 KByte = 819200 bits
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity = 100 KByte = 819200 bits
        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");
        // Demand = exactly 100 KByte via Source_Code_Size
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "100 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "demand == capacity should NOT error (> boundary): {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "demand == capacity should emit info: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("100.0%"),
            "should show 100.0%% utilization: {}",
            infos[0].message
        );
    }

    #[test]
    fn memory_budget_one_bit_over_capacity() {
        // Demand = capacity + 1 bit must error.
        // Capacity: 8192 bits (1 KByte)
        // Demand: 8192 + 1 = 8193 bits
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![t1, t2]);

        // Capacity: 1 KByte = 8192 bits
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 KByte");
        // t1: 1 KByte = 8192 bits
        b.set_property(t1, "Memory_Properties", "Source_Code_Size", "1 KByte");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );
        // t2: 1 bit (the 1-bit that pushes demand over capacity)
        b.set_property(t2, "Memory_Properties", "Source_Code_Size", "1 bits");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "demand > capacity by 1 bit should error: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("exceeded"),
            "should mention exceeded: {}",
            errors[0].message
        );
    }

    #[test]
    fn memory_budget_one_bit_under_capacity() {
        // Demand = capacity - 1 bit must NOT error.
        // Use bit-level precision: capacity=8192, demand=8191.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity: 8192 bits (1 KByte)
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 KByte");
        // Demand: 8191 bits (1 less than capacity)
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "8191 bits");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "demand < capacity by 1 bit should NOT error: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "demand < capacity should emit info: {:?}",
            diags
        );
    }

    #[test]
    fn bandwidth_budget_exactly_at_capacity() {
        // Rate == capacity must NOT warn (boundary: > not >=).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus, proc]);

        // Bus capacity: exactly 1000 bps
        b.set_property(bus, "Communication_Properties", "Data_Rate", "1000 bitsps");
        // Component demand: exactly 1000 bps
        b.set_property(proc, "Communication_Properties", "Data_Rate", "1000 bitsps");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (eth0)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeded"))
            .collect();
        assert!(
            warnings.is_empty(),
            "rate == capacity should NOT warn about exceeded: {:?}",
            warnings
        );
    }

    #[test]
    fn bandwidth_budget_one_bps_over_capacity() {
        // Rate > capacity by 1 bps must warn.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus, proc]);

        // Bus capacity: 1000 bps
        b.set_property(bus, "Communication_Properties", "Data_Rate", "1000 bitsps");
        // Component demand: 1001 bps (1 over)
        b.set_property(proc, "Communication_Properties", "Data_Rate", "1001 bitsps");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (eth0)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeded"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "rate > capacity by 1 bps should warn: {:?}",
            diags
        );
    }

    #[test]
    fn memory_demand_is_sum_not_product() {
        // Verify compute_memory_demand uses addition, not multiplication.
        // If mutated to *, 100+200+300=600 would become 100*200*300=6000000.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity: 700 bits (just above sum of 100+200+300=600)
        b.set_property(mem, "Memory_Properties", "Memory_Size", "700 bits");
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "100 bits");
        b.set_property(thread, "Memory_Properties", "Data_Size", "200 bits");
        b.set_property(thread, "Memory_Properties", "Stack_Size", "300 bits");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "100+200+300=600 < 700 should not error (if * were used, 6M > 700 would error): {:?}",
            errors
        );
    }

    #[test]
    fn resource_budget_analysis_field_matches_name() {
        // Verify every diagnostic has .analysis == self.name().
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 KByte");
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "2 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let pass = ResourceBudgetAnalysis;
        let diags = pass.analyze(&inst);

        assert!(!diags.is_empty(), "should produce diagnostics");
        for diag in &diags {
            assert_eq!(
                diag.analysis,
                pass.name(),
                "diagnostic .analysis must match .name(): {:?}",
                diag,
            );
        }
    }

    #[test]
    fn memory_multiple_properties_summed() {
        // Test that Source_Code_Size + Data_Size + Stack_Size are all summed
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // 512 KByte capacity
        b.set_property(mem, "Memory_Properties", "Memory_Size", "512 KByte");

        // Thread: 100 KByte code + 100 KByte data + 100 KByte stack = 300 KByte
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "100 KByte");
        b.set_property(thread, "Memory_Properties", "Data_Size", "100 KByte");
        b.set_property(thread, "Memory_Properties", "Stack_Size", "100 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "300 KB < 512 KB, should be within budget: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
        // ~58.6% utilization
        assert!(
            infos[0].message.contains("58."),
            "should show ~58% utilization: {}",
            infos[0].message
        );
    }

    // ── Mutant-killing tests ──────────────────────────────────

    #[test]
    fn zero_demand_component_not_counted_in_bound_list() {
        // Kills mutant: line ~93 `if demand > 0` flipped to `>= 0`.
        // A component bound to memory but with zero demand (no size properties)
        // should NOT appear in bound_components, so if it is the only bound
        // component, the memory should produce no utilization diagnostic.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Memory has capacity
        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");
        // Thread is bound to memory but has NO size properties => demand == 0
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        // With `demand > 0`, zero-demand component is skipped, bound_components
        // is empty, so we get no utilization diagnostic for this memory.
        // If mutated to `>= 0`, zero-demand would be included, producing a
        // utilization diagnostic with 0.0 KB.
        let mem_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ram") && d.message.contains("utilization"))
            .collect();
        assert!(
            mem_diags.is_empty(),
            "zero-demand component should not produce utilization diagnostic: {:?}",
            mem_diags
        );
    }

    #[test]
    fn zero_demand_component_alongside_nonzero_demand() {
        // Further kills `demand > 0` -> `>= 0` mutant.
        // One component has demand, one has zero demand.
        // The count of bound components should be 1 (not 2).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![t1, t2]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");
        // t1 has demand
        b.set_property(t1, "Memory_Properties", "Source_Code_Size", "10 KByte");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );
        // t2 is bound but has zero demand (no size properties)
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
        // Should say "1 bound components" not "2 bound components"
        assert!(
            infos[0].message.contains("1 bound"),
            "should count only nonzero-demand components as bound: {}",
            infos[0].message
        );
    }

    #[test]
    fn virtual_bus_bandwidth_checked() {
        // Kills mutant: line ~143-144 `!=` flipped to `==` for Bus/VirtualBus.
        // A VirtualBus with bandwidth and bound connections should produce a
        // warning when exceeded.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let vbus = b.add_component("vnet", ComponentCategory::VirtualBus, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![vbus, proc]);

        // VirtualBus capacity: 500 bps
        b.set_property(vbus, "Communication_Properties", "Data_Rate", "500 bitsps");
        // Component demand: 600 bps (exceeds)
        b.set_property(proc, "Communication_Properties", "Data_Rate", "600 bitsps");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (vnet)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeded"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "VirtualBus should be checked for bandwidth: {:?}",
            diags
        );
        assert!(
            warnings[0].message.contains("vnet"),
            "should reference the virtual bus name: {}",
            warnings[0].message
        );
    }

    #[test]
    fn non_bus_category_not_checked_for_bandwidth() {
        // Kills mutant: line ~143-144 `!=` flipped to `==`.
        // A Processor (non-bus) with Data_Rate should NOT produce bandwidth
        // diagnostics even if another component references it in connection binding.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu, proc]);

        // Processor with Data_Rate (not a bus!)
        b.set_property(cpu, "Communication_Properties", "Data_Rate", "1000 bitsps");
        // Component referencing cpu1 as connection binding
        b.set_property(proc, "Communication_Properties", "Data_Rate", "2000 bitsps");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let bandwidth_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("bandwidth") || d.message.contains("exceeded"))
            .filter(|d| d.message.contains("cpu1"))
            .collect();
        assert!(
            bandwidth_diags.is_empty(),
            "Processor should not be checked for bandwidth: {:?}",
            bandwidth_diags
        );
    }

    #[test]
    fn bus_bandwidth_checked() {
        // Complement to virtual_bus test: plain Bus must also be checked.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus = b.add_component("canbus", ComponentCategory::Bus, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus, proc]);

        // Bus capacity: 100 bps
        b.set_property(bus, "Communication_Properties", "Data_Rate", "100 bitsps");
        // Component demand: 200 bps (exceeds)
        b.set_property(proc, "Communication_Properties", "Data_Rate", "200 bitsps");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (canbus)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeded"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "Bus should be checked for bandwidth: {:?}",
            diags
        );
    }

    #[test]
    fn memory_demand_arithmetic_non_round_numbers() {
        // Kills arithmetic mutants on lines ~104-105 (division / multiplication).
        // Uses non-round bit values where div vs mul would produce very different KB.
        // demand = 13_000 bits, capacity = 15_000 bits
        // Correct: 13000 / (8*1024) = 1.5869 KB, 15000 / (8*1024) = 1.8310 KB
        // If / replaced with *: 13000 * 8192 = huge => would definitely error
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity: 15000 bits
        b.set_property(mem, "Memory_Properties", "Memory_Size", "15000 bits");
        // Demand: 7000 + 3000 + 3000 = 13000 bits (under capacity)
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "7000 bits");
        b.set_property(thread, "Memory_Properties", "Data_Size", "3000 bits");
        b.set_property(thread, "Memory_Properties", "Stack_Size", "3000 bits");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "13000 bits < 15000 bits should not error: {:?}",
            errors
        );

        // Verify correct utilization percentage: 13000/15000 = 86.7%
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
        assert!(
            infos[0].message.contains("86."),
            "should show ~86.7%% utilization: {}",
            infos[0].message
        );
    }

    #[test]
    fn memory_utilization_percentage_correct() {
        // Kills mutants on line ~117 and ~130 where * 100.0 could be mutated.
        // Uses exact numbers to verify the utilization percentage.
        // demand = 1 KByte = 8192 bits, capacity = 4 KByte = 32768 bits
        // utilization = 8192/32768 = 25.0% exactly
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "4 KByte");
        b.set_property(thread, "Memory_Properties", "Source_Code_Size", "1 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
        assert!(
            infos[0].message.contains("25.0%"),
            "should show exactly 25.0%% utilization: {}",
            infos[0].message
        );
        assert!(
            infos[0].message.contains("1.0 KB of 4.0 KB"),
            "should show 1.0 KB of 4.0 KB: {}",
            infos[0].message
        );
    }

    #[test]
    fn bandwidth_multiple_connections_summed() {
        // Kills arithmetic mutant where `total_rate += rate` could become
        // `total_rate -= rate` or `total_rate *= rate`.
        // Two connections each at 400 bps on a 700 bps bus => 800 > 700, should warn.
        // If -= used: 0 + 400 - 400 = 0 < 700, no warning.
        // If *= used: 0 * 400 = 0, no warning.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus = b.add_component("eth0", ComponentCategory::Bus, Some(root));
        let p1 = b.add_component("p1", ComponentCategory::Process, Some(root));
        let p2 = b.add_component("p2", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus, p1, p2]);

        b.set_property(bus, "Communication_Properties", "Data_Rate", "700 bitsps");

        b.set_property(p1, "Communication_Properties", "Data_Rate", "400 bitsps");
        b.set_property(
            p1,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (eth0)",
        );

        b.set_property(p2, "Communication_Properties", "Data_Rate", "400 bitsps");
        b.set_property(
            p2,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (eth0)",
        );

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("exceeded"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "400+400=800 > 700 should warn: {:?}",
            diags
        );
        // Also verify connection count = 2
        assert!(
            warnings[0].message.contains("2 connections"),
            "should report 2 connections: {}",
            warnings[0].message
        );
    }

    #[test]
    fn parse_data_rate_all_units() {
        // Kills mutants on DATA_RATE_UNITS conversion factors.
        assert_eq!(parse_data_rate("1 Gbitsps"), Some(1_000_000_000.0));
        assert_eq!(parse_data_rate("1 Mbitsps"), Some(1_000_000.0));
        assert_eq!(parse_data_rate("1 Kbitsps"), Some(1_000.0));
        assert_eq!(parse_data_rate("1 bitsps"), Some(1.0));
        assert_eq!(parse_data_rate("1 GBytesps"), Some(8_000_000_000.0));
        assert_eq!(parse_data_rate("1 MBytesps"), Some(8_000_000.0));
        assert_eq!(parse_data_rate("1 KBytesps"), Some(8_000.0));
        assert_eq!(parse_data_rate("1 Bytesps"), Some(8.0));
    }

    #[test]
    fn modal_awareness_note_with_modes() {
        // Kills mutant on `system_operation_modes.is_empty()` negation.
        // With modes present, should produce an info diagnostic about modal awareness.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_children(root, vec![]);

        let mut inst = b.build(root);
        // Add a system operation mode
        inst.system_operation_modes.push(SystemOperationMode {
            name: "normal".to_string(),
            mode_selections: Vec::new(),
        });

        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let modal_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("modal"))
            .collect();
        assert_eq!(
            modal_diags.len(),
            1,
            "should produce modal awareness note when modes exist: {:?}",
            diags
        );
        assert!(
            modal_diags[0].message.contains("1 system operation mode"),
            "should report mode count: {}",
            modal_diags[0].message
        );
    }

    #[test]
    fn no_modal_note_without_modes() {
        // Complement: no modes => no modal note.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_children(root, vec![]);

        let inst = b.build(root);
        let diags = ResourceBudgetAnalysis.analyze(&inst);

        let modal_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("modal"))
            .collect();
        assert!(
            modal_diags.is_empty(),
            "should not produce modal note without modes: {:?}",
            modal_diags
        );
    }
}
