//! Memory budget analysis pass.
//!
//! For each memory component in the system instance, this pass:
//!
//! 1. Finds all processes and threads with `Actual_Memory_Binding` pointing to it.
//! 2. Reads `Code_Size` and `Data_Size` properties from `Memory_Properties` for
//!    each bound component.
//! 3. Sums the total memory demand.
//! 4. Compares against the `Memory_Size` property on the memory component.
//! 5. Emits an error if demand exceeds capacity.
//! 6. Emits an info diagnostic with utilization percentage otherwise.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{get_memory_binding, get_size_property};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Memory budget analysis.
pub struct MemoryBudgetAnalysis;

impl Analysis for MemoryBudgetAnalysis {
    fn name(&self) -> &str {
        "memory_budget"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (mem_idx, mem_comp) in instance.all_components() {
            if mem_comp.category != ComponentCategory::Memory {
                continue;
            }

            let mem_props = instance.properties_for(mem_idx);
            let mem_path = component_path(instance, mem_idx);

            // Get memory capacity (Memory_Size property); skip if absent.
            let capacity_bits = match get_size_property(mem_props, "Memory_Size") {
                Some(v) => v,
                None => {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "memory '{}' has no Memory_Size property; cannot check budget",
                            mem_comp.name,
                        ),
                        path: mem_path,
                        analysis: self.name().to_string(),
                    });
                    continue;
                }
            };

            // Find processes/threads bound to this memory via Actual_Memory_Binding.
            let mut total_demand_bits: u64 = 0;
            let mut bound_components: Vec<(String, u64)> = Vec::new();

            for (comp_idx, comp) in instance.all_components() {
                // Only consider processes and threads.
                if comp.category != ComponentCategory::Process
                    && comp.category != ComponentCategory::Thread
                {
                    continue;
                }

                let comp_props = instance.properties_for(comp_idx);

                let binding = get_memory_binding(comp_props);
                let bound_to_this = match binding {
                    Some(ref target) => target.eq_ignore_ascii_case(mem_comp.name.as_str()),
                    None => false,
                };
                if !bound_to_this {
                    continue;
                }

                let code_size = get_size_property(comp_props, "Code_Size").unwrap_or(0);
                let data_size = get_size_property(comp_props, "Data_Size").unwrap_or(0);
                let demand = code_size.saturating_add(data_size);

                if demand > 0 {
                    bound_components.push((comp.name.as_str().to_string(), demand));
                    total_demand_bits = total_demand_bits.saturating_add(demand);
                }
            }

            if bound_components.is_empty() {
                continue;
            }

            let demand_kb = total_demand_bits as f64 / (8.0 * 1024.0);
            let capacity_kb = capacity_bits as f64 / (8.0 * 1024.0);
            let utilization = (total_demand_bits as f64 / capacity_bits as f64) * 100.0;

            if total_demand_bits > capacity_bits {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "memory '{}' budget exceeded: {:.1} KB demand vs {:.1} KB capacity \
                         ({} bound component(s), {:.1}% utilization)",
                        mem_comp.name,
                        demand_kb,
                        capacity_kb,
                        bound_components.len(),
                        utilization,
                    ),
                    path: mem_path,
                    analysis: self.name().to_string(),
                });
            } else {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "memory '{}' utilization: {:.1} KB of {:.1} KB ({:.1}%, {} bound component(s))",
                        mem_comp.name,
                        demand_kb,
                        capacity_kb,
                        utilization,
                        bound_components.len(),
                    ),
                    path: mem_path,
                    analysis: self.name().to_string(),
                });
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

    // ── over-capacity ────────────────────────────────────────────

    #[test]
    fn over_capacity_emits_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![t1, t2]);

        // 100 KByte capacity
        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");

        // t1: 60 KByte code + 10 KByte data = 70 KByte
        b.set_property(t1, "Memory_Properties", "Code_Size", "60 KByte");
        b.set_property(t1, "Memory_Properties", "Data_Size", "10 KByte");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        // t2: 40 KByte code + 10 KByte data = 50 KByte  -> total 120 KByte > 100 KByte
        b.set_property(t2, "Memory_Properties", "Code_Size", "40 KByte");
        b.set_property(t2, "Memory_Properties", "Data_Size", "10 KByte");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "expected one error: {:?}", diags);
        assert!(
            errors[0].message.contains("exceeded"),
            "error should mention exceeded: {}",
            errors[0].message,
        );
        assert!(
            errors[0].message.contains("ram"),
            "error should mention memory name: {}",
            errors[0].message,
        );
        assert!(
            errors[0].message.contains("120.0"),
            "error should show total demand ~120 KB: {}",
            errors[0].message,
        );
    }

    // ── normal (within budget) ───────────────────────────────────

    #[test]
    fn within_budget_emits_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // 1 MByte capacity
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 MByte");

        // worker: 100 KByte code + 50 KByte data = 150 KByte
        b.set_property(thread, "Memory_Properties", "Code_Size", "100 KByte");
        b.set_property(thread, "Memory_Properties", "Data_Size", "50 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should have no errors: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "expected one info diagnostic: {:?}", diags);
        assert!(
            infos[0].message.contains("ram"),
            "info should mention ram: {}",
            infos[0].message,
        );
        // 150 KB / 1024 KB = ~14.6%
        assert!(
            infos[0].message.contains("14."),
            "should show ~14% utilization: {}",
            infos[0].message,
        );
    }

    // ── no binding ───────────────────────────────────────────────

    #[test]
    fn no_binding_produces_no_budget_diag() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Memory has capacity but no thread is bound to it.
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 MByte");

        // Thread has sizes but no binding.
        b.set_property(thread, "Memory_Properties", "Code_Size", "100 KByte");
        b.set_property(thread, "Memory_Properties", "Data_Size", "50 KByte");

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let budget_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("utilization") || d.message.contains("exceeded"))
            .collect();
        assert!(
            budget_diags.is_empty(),
            "no binding should produce no budget diagnostic: {:?}",
            budget_diags,
        );
    }

    // ── missing properties ───────────────────────────────────────

    #[test]
    fn missing_memory_size_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        b.set_children(root, vec![mem]);
        // No Memory_Size set on the memory component.

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "should warn about missing Memory_Size: {:?}",
            diags,
        );
        assert!(
            warnings[0].message.contains("Memory_Size"),
            "warning should mention Memory_Size: {}",
            warnings[0].message,
        );
    }

    #[test]
    fn missing_code_and_data_size_skipped() {
        // Thread bound to memory but has no Code_Size or Data_Size.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![mem, thread]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 MByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        // Thread has zero demand so it should not be counted.
        let budget_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("utilization") || d.message.contains("exceeded"))
            .collect();
        assert!(
            budget_diags.is_empty(),
            "thread with no size props should not count: {:?}",
            budget_diags,
        );
    }

    // ── Boundary tests (kill > vs >= mutants) ─────────────────────

    #[test]
    fn memory_budget_exactly_at_capacity() {
        // Demand == capacity must NOT error (boundary: > not >=).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity: 100 KByte = 819200 bits
        b.set_property(mem, "Memory_Properties", "Memory_Size", "100 KByte");
        // Demand: exactly 100 KByte (Code_Size + Data_Size = 50 + 50)
        b.set_property(thread, "Memory_Properties", "Code_Size", "50 KByte");
        b.set_property(thread, "Memory_Properties", "Data_Size", "50 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

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
        b.set_property(t1, "Memory_Properties", "Code_Size", "1 KByte");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );
        // t2: 1 bit (pushes demand to 8193 > 8192)
        b.set_property(t2, "Memory_Properties", "Code_Size", "1 bits");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

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
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![mem, proc]);
        b.set_children(proc, vec![thread]);

        // Capacity: 8192 bits (1 KByte)
        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 KByte");
        // Demand: 8191 bits (1 under capacity)
        b.set_property(thread, "Memory_Properties", "Code_Size", "8191 bits");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

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
    fn memory_demand_is_sum_not_product() {
        // code_size + data_size must use addition, not multiplication.
        // If mutated to *, 100+200=300 would become 100*200=20000.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![mem, thread]);

        // Capacity: 400 bits (above sum 100+200=300, below product 100*200=20000)
        b.set_property(mem, "Memory_Properties", "Memory_Size", "400 bits");
        b.set_property(thread, "Memory_Properties", "Code_Size", "100 bits");
        b.set_property(thread, "Memory_Properties", "Data_Size", "200 bits");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "100+200=300 < 400 should not error (if * were used, 20000 > 400 would error): {:?}",
            errors
        );
    }

    #[test]
    fn memory_budget_analysis_field_matches_name() {
        // Verify every diagnostic has .analysis == self.name().
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![mem, thread]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "1 KByte");
        b.set_property(thread, "Memory_Properties", "Code_Size", "2 KByte");
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let pass = MemoryBudgetAnalysis;
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

    // ── process binding also works ───────────────────────────────

    #[test]
    fn process_binding_counted() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("app", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![mem, proc]);

        b.set_property(mem, "Memory_Properties", "Memory_Size", "512 KByte");
        b.set_property(proc, "Memory_Properties", "Code_Size", "200 KByte");
        b.set_property(proc, "Memory_Properties", "Data_Size", "100 KByte");
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        );

        let inst = b.build(root);
        let diags = MemoryBudgetAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "process should be counted in budget: {:?}",
            diags,
        );
        // 300 / 512 = 58.6%
        assert!(
            infos[0].message.contains("58."),
            "should show ~58% utilization: {}",
            infos[0].message,
        );
    }
}
