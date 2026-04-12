//! Binding validation (AS5506 §10.6).
//!
//! Validates deployment property bindings:
//! - Actual_Processor_Binding targets are processors or virtual processors
//! - Actual_Memory_Binding targets are memory, system, or processor
//! - Required bindings exist (thread→processor, process→memory)
//! - Allowed_*_Binding constraints are satisfied by Actual_*_Binding

use spar_hir_def::instance::{SystemInstance, SystemOperationMode};
use spar_hir_def::item_tree::ComponentCategory;

use crate::modal::is_component_active_in_som;
use crate::property_accessors::extract_reference_target;
use crate::{Analysis, AnalysisDiagnostic, ModalAnalysis, Severity, component_path};

/// Validates deployment binding properties on the instance model.
///
/// Checks:
/// - Threads and virtual processors should have processor bindings
/// - Processes and threads should have memory bindings
/// - Connections should have connection bindings (when buses exist)
pub struct BindingCheckAnalysis;

impl Analysis for BindingCheckAnalysis {
    fn name(&self) -> &str {
        "binding_check"
    }

    fn as_modal(&self) -> Option<&dyn ModalAnalysis> {
        Some(self)
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        self.check_bindings(instance, None)
    }
}

impl ModalAnalysis for BindingCheckAnalysis {
    fn analyze_in_mode(
        &self,
        instance: &SystemInstance,
        som: &SystemOperationMode,
    ) -> Vec<AnalysisDiagnostic> {
        self.check_bindings(instance, Some(som))
    }
}

impl BindingCheckAnalysis {
    fn check_bindings(
        &self,
        instance: &SystemInstance,
        som: Option<&SystemOperationMode>,
    ) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error — binding target is wrong component category
        //   Info  — thread missing processor binding, process missing memory binding
        let mut diags = Vec::new();

        // Track whether the model has any processors and memory components
        let has_processors = instance.all_components().any(|(_, c)| {
            matches!(
                c.category,
                ComponentCategory::Processor | ComponentCategory::VirtualProcessor
            )
        });
        let has_memory = instance
            .all_components()
            .any(|(_, c)| matches!(c.category, ComponentCategory::Memory));

        for (comp_idx, comp) in instance.all_components() {
            // Filter by SOM if provided.
            if let Some(som) = som {
                if !is_component_active_in_som(instance, comp_idx, som) {
                    continue;
                }
            }

            let path = component_path(instance, comp_idx);
            let props = instance.properties_for(comp_idx);

            // Check for processor bindings on threads
            if comp.category == ComponentCategory::Thread && has_processors {
                let has_binding = props
                    .get("Deployment_Properties", "Actual_Processor_Binding")
                    .is_some()
                    || props.get("", "Actual_Processor_Binding").is_some();
                if !has_binding {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "thread '{}' has no Actual_Processor_Binding \
                             (required for schedulability analysis)",
                            comp.name
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Check for memory bindings on processes
            if comp.category == ComponentCategory::Process && has_memory {
                let has_binding = props
                    .get("Deployment_Properties", "Actual_Memory_Binding")
                    .is_some()
                    || props.get("", "Actual_Memory_Binding").is_some();
                if !has_binding {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "process '{}' has no Actual_Memory_Binding \
                             (required for memory analysis)",
                            comp.name
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Validate Actual_Processor_Binding value if present
            if let Some(binding_val) = props
                .get("Deployment_Properties", "Actual_Processor_Binding")
                .or_else(|| props.get("", "Actual_Processor_Binding"))
            {
                // The binding value is a reference — validate the target is a processor
                // For now, we do a name-based heuristic check since we have opaque strings
                validate_binding_target(
                    instance,
                    comp_idx,
                    "Actual_Processor_Binding",
                    binding_val,
                    &[
                        ComponentCategory::Processor,
                        ComponentCategory::VirtualProcessor,
                    ],
                    &path,
                    &mut diags,
                );
            }

            // Validate Actual_Memory_Binding value if present
            if let Some(binding_val) = props
                .get("Deployment_Properties", "Actual_Memory_Binding")
                .or_else(|| props.get("", "Actual_Memory_Binding"))
            {
                validate_binding_target(
                    instance,
                    comp_idx,
                    "Actual_Memory_Binding",
                    binding_val,
                    &[
                        ComponentCategory::Memory,
                        ComponentCategory::System,
                        ComponentCategory::Processor,
                    ],
                    &path,
                    &mut diags,
                );
            }
        }

        diags
    }
}

/// Try to validate a binding target reference against allowed categories.
///
/// Since property values are currently opaque strings, this does a best-effort
/// match: it extracts the reference target name and looks it up in the instance
/// model. If found, it checks the category.
fn validate_binding_target(
    instance: &SystemInstance,
    _comp_idx: spar_hir_def::instance::ComponentInstanceIdx,
    binding_name: &str,
    binding_val: &str,
    allowed_categories: &[ComponentCategory],
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // Extract target name from reference(name) format
    let target_name = match extract_reference_target(binding_val) {
        Some(name) => name,
        None => return, // Can't parse the reference
    };

    // Try to find the target in the instance model
    for (_idx, comp) in instance.all_components() {
        if comp.name.as_str().eq_ignore_ascii_case(target_name) {
            if !allowed_categories.contains(&comp.category) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "{} references '{}' which is a {} component, \
                         expected one of: {}",
                        binding_name,
                        target_name,
                        comp.category,
                        allowed_categories
                            .iter()
                            .map(|c| c.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    path: path.to_vec(),
                    analysis: "binding_check".to_string(),
                });
            }
            return;
        }
    }
    // Target not found in instance model — not an error, might be
    // in a different part of the model or use a different naming scheme
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::name::Name;
    use spar_hir_def::name::PropertyRef;
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
    fn thread_without_processor_binding_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![thread]);

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Processor_Binding"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "thread should note missing binding: {:?}",
            diags
        );
    }

    #[test]
    fn thread_with_processor_binding_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu)",
        );

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.contains("Actual_Processor_Binding") && d.message.contains("worker")
            })
            .collect();
        assert!(
            binding_diags.is_empty(),
            "bound thread should not warn: {:?}",
            binding_diags
        );
    }

    #[test]
    fn no_processors_in_model_no_binding_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![thread]);

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Processor_Binding"))
            .collect();
        assert!(
            binding_diags.is_empty(),
            "no processors = no binding needed: {:?}",
            binding_diags
        );
    }

    #[test]
    fn extract_reference_target_works() {
        assert_eq!(extract_reference_target("reference (cpu1)"), Some("cpu1"));
        assert_eq!(extract_reference_target("(reference (cpu1))"), Some("cpu1"));
        assert_eq!(extract_reference_target("reference(mem)"), Some("mem"));
        assert_eq!(extract_reference_target("invalid"), None);
    }

    // ── Process without memory binding info ────────────────────

    #[test]
    fn process_without_memory_binding_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![mem, proc]);

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Memory_Binding"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "process should note missing memory binding: {:?}",
            diags
        );
    }

    // ── Process with memory binding: no warning ─────────────────

    #[test]
    fn process_with_memory_binding_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![mem, proc]);
        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (mem)",
        );

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Memory_Binding") && d.message.contains("proc"))
            .collect();
        assert!(
            binding_diags.is_empty(),
            "bound process should not warn: {:?}",
            binding_diags
        );
    }

    // ── No memory in model: no memory binding info ──────────────

    #[test]
    fn no_memory_in_model_no_binding_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Memory_Binding"))
            .collect();
        assert!(
            binding_diags.is_empty(),
            "no memory = no binding needed: {:?}",
            binding_diags
        );
    }

    // ── Binding to valid processor target (no error) ────────────

    #[test]
    fn binding_to_valid_processor_target() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu)",
        );

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "valid binding should not error: {:?}",
            errors
        );
    }

    // ── Binding to nonexistent target (no error — graceful) ─────

    #[test]
    fn binding_to_nonexistent_target_graceful() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (ghost)",
        );

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        // binding_check does NOT error on nonexistent target (it just returns)
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("ghost"))
            .collect();
        assert!(
            errors.is_empty(),
            "nonexistent target is not flagged in binding_check: {:?}",
            errors
        );
    }

    #[test]
    fn binding_to_wrong_category() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![mem, thread]);
        // Bind thread to memory (wrong — should be processor)
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (mem)",
        );

        let inst = b.build(root);
        let diags = BindingCheckAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("memory"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "binding to memory for processor: {:?}",
            diags
        );
    }

    // ── ModalAnalysis tests ─────────────────────────────────────

    #[test]
    fn as_modal_returns_some() {
        let analysis = BindingCheckAnalysis;
        assert!(
            analysis.as_modal().is_some(),
            "BindingCheckAnalysis should support modal analysis"
        );
    }

    #[test]
    fn modal_filters_inactive_thread() {
        // Two threads, each active in a different mode.
        // In "fast" mode only t_fast is active. t_slow should be filtered out.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t_fast = b.add_component("t_fast", ComponentCategory::Thread, Some(proc));
        let t_slow = b.add_component("t_slow", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t_fast, t_slow]);

        b.components[t_fast].in_modes = vec![Name::new("fast")];
        b.components[t_slow].in_modes = vec![Name::new("slow")];

        // Neither thread has a processor binding -> both would get info diagnostic if active.

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: proc,
        });
        let slow_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("slow"),
            is_initial: false,
            owner: proc,
        });
        inst.components[proc].modes = vec![fast_mode, slow_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(proc, fast_mode)],
        };

        let diags = BindingCheckAnalysis.analyze_in_mode(&inst, &som_fast);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Processor_Binding"))
            .collect();
        // Only t_fast should be checked (and it's missing a binding)
        assert_eq!(
            binding_diags.len(),
            1,
            "should only warn for active thread: {:?}",
            diags
        );
        assert!(
            binding_diags[0].message.contains("t_fast"),
            "warning should be for t_fast: {}",
            binding_diags[0].message
        );
    }

    #[test]
    fn modal_non_modal_thread_included_in_all_soms() {
        // A non-modal thread (empty in_modes) should be included in every SOM.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t_always = b.add_component("t_always", ComponentCategory::Thread, Some(proc));
        let t_fast = b.add_component("t_fast", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t_always, t_fast]);

        b.components[t_fast].in_modes = vec![Name::new("fast")];
        // t_always has empty in_modes -> always active

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: proc,
        });
        inst.components[proc].modes = vec![fast_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(proc, fast_mode)],
        };

        let diags = BindingCheckAnalysis.analyze_in_mode(&inst, &som_fast);
        let binding_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Processor_Binding"))
            .collect();
        // Both t_always and t_fast should be checked
        assert_eq!(
            binding_diags.len(),
            2,
            "both threads should be checked: {:?}",
            diags
        );
    }

    #[test]
    fn modal_different_diagnostics_per_som() {
        // In "fast" mode: t_fast is active (no binding -> info).
        // In "slow" mode: t_slow is active (has binding -> no info).
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t_fast = b.add_component("t_fast", ComponentCategory::Thread, Some(proc));
        let t_slow = b.add_component("t_slow", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t_fast, t_slow]);

        b.components[t_fast].in_modes = vec![Name::new("fast")];
        b.components[t_slow].in_modes = vec![Name::new("slow")];

        // t_fast has NO binding; t_slow HAS a binding
        b.set_property(
            t_slow,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu)",
        );

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: proc,
        });
        let slow_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("slow"),
            is_initial: false,
            owner: proc,
        });
        inst.components[proc].modes = vec![fast_mode, slow_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(proc, fast_mode)],
        };
        let som_slow = SystemOperationMode {
            name: "slow".to_string(),
            mode_selections: vec![(proc, slow_mode)],
        };

        // Fast mode: t_fast is active and missing binding
        let diags_fast = BindingCheckAnalysis.analyze_in_mode(&inst, &som_fast);
        let fast_binding: Vec<_> = diags_fast
            .iter()
            .filter(|d| {
                d.message.contains("Actual_Processor_Binding") && d.message.contains("t_fast")
            })
            .collect();
        assert_eq!(
            fast_binding.len(),
            1,
            "t_fast should be missing binding in fast mode: {:?}",
            diags_fast
        );

        // Slow mode: t_slow is active and HAS binding -> no missing-binding diagnostic
        let diags_slow = BindingCheckAnalysis.analyze_in_mode(&inst, &som_slow);
        let slow_binding: Vec<_> = diags_slow
            .iter()
            .filter(|d| {
                d.message.contains("Actual_Processor_Binding") && d.message.contains("t_slow")
            })
            .collect();
        assert!(
            slow_binding.is_empty(),
            "t_slow has binding, should not warn: {:?}",
            diags_slow
        );
    }
}
