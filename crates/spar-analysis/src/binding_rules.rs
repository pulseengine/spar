//! Extended binding validation rules (AS5506 §10.6).
//!
//! Supplements `binding_check.rs` with stricter rules:
//! - **BIND-PROCESSOR-REQUIRED** — Every thread must have Actual_Processor_Binding
//! - **BIND-MEMORY-REQUIRED** — Every process should have Actual_Memory_Binding
//! - **BIND-TARGET-EXISTS** — Binding target path must resolve to an existing component
//! - **BIND-TARGET-CATEGORY** — Binding target must be the correct category

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::extract_reference_target;
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates extended binding rules on the instance model.
///
/// Checks AS5506 §10.6 rules:
/// - Thread instances require processor bindings
/// - Process instances should have memory bindings
/// - Binding targets must resolve to existing components
/// - Binding target categories must be appropriate
pub struct BindingRuleAnalysis;

impl Analysis for BindingRuleAnalysis {
    fn name(&self) -> &str {
        "binding_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — thread missing required processor binding, binding target not found,
        //             binding target wrong category
        //   Warning — process missing memory binding
        let mut diags = Vec::new();

        // Check if the model has any processors and memory
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
            let path = component_path(instance, comp_idx);
            let props = instance.properties_for(comp_idx);

            // BIND-PROCESSOR-REQUIRED: Thread instances must have processor binding
            if comp.category == ComponentCategory::Thread && has_processors {
                let has_binding = props
                    .get("Deployment_Properties", "Actual_Processor_Binding")
                    .is_some()
                    || props.get("", "Actual_Processor_Binding").is_some();

                if !has_binding {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "thread '{}' is missing required Actual_Processor_Binding \
                             (AS5506 §10.6: threads must be bound to a processor)",
                            comp.name
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // BIND-MEMORY-REQUIRED: Process instances should have memory binding
            if comp.category == ComponentCategory::Process && has_memory {
                let has_binding = props
                    .get("Deployment_Properties", "Actual_Memory_Binding")
                    .is_some()
                    || props.get("", "Actual_Memory_Binding").is_some();

                if !has_binding {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "process '{}' is missing Actual_Memory_Binding \
                             (recommended for memory analysis)",
                            comp.name
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // BIND-TARGET-EXISTS + BIND-TARGET-CATEGORY: Validate binding targets
            if let Some(binding_val) = props
                .get("Deployment_Properties", "Actual_Processor_Binding")
                .or_else(|| props.get("", "Actual_Processor_Binding"))
            {
                check_binding_target(
                    instance,
                    binding_val,
                    "Actual_Processor_Binding",
                    &[
                        ComponentCategory::Processor,
                        ComponentCategory::VirtualProcessor,
                    ],
                    &path,
                    &mut diags,
                );
            }

            if let Some(binding_val) = props
                .get("Deployment_Properties", "Actual_Memory_Binding")
                .or_else(|| props.get("", "Actual_Memory_Binding"))
            {
                check_binding_target(
                    instance,
                    binding_val,
                    "Actual_Memory_Binding",
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

/// BIND-TARGET-EXISTS + BIND-TARGET-CATEGORY: Validate that a binding
/// target exists and has an appropriate category.
fn check_binding_target(
    instance: &SystemInstance,
    binding_val: &str,
    binding_name: &str,
    allowed_categories: &[ComponentCategory],
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let target_name = match extract_reference_target(binding_val) {
        Some(name) => name,
        None => return,
    };

    // BIND-TARGET-EXISTS: Try to find the target
    let mut found = false;
    for (_idx, comp) in instance.all_components() {
        if comp.name.as_str().eq_ignore_ascii_case(target_name) {
            found = true;

            // BIND-TARGET-CATEGORY: Check that the category is appropriate
            if !allowed_categories.contains(&comp.category) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "{} target '{}' is a {} component, expected one of: {}",
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
                    analysis: "binding_rules".to_string(),
                });
            }
            break;
        }
    }

    if !found {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "{} references '{}' which does not exist in the instance model",
                binding_name, target_name
            ),
            path: path.to_vec(),
            analysis: "binding_rules".to_string(),
        });
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

    // ── BIND-PROCESSOR-REQUIRED tests ───────────────────────────────

    #[test]
    fn thread_without_processor_binding_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![thread]);

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message
                        .contains("missing required Actual_Processor_Binding")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "thread without binding should error: {:?}",
            diags
        );
    }

    #[test]
    fn thread_with_processor_binding_no_error() {
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
        let diags = BindingRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("missing required")
                    && d.message.contains("worker")
            })
            .collect();
        assert!(
            errors.is_empty(),
            "bound thread should not error: {:?}",
            errors
        );
    }

    #[test]
    fn no_processors_thread_no_binding_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![thread]);

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Processor_Binding"))
            .collect();
        assert!(
            errors.is_empty(),
            "no processors = no binding required: {:?}",
            errors
        );
    }

    // ── BIND-MEMORY-REQUIRED tests ──────────────────────────────────

    #[test]
    fn process_without_memory_binding_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![mem, proc]);

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("missing Actual_Memory_Binding")
            })
            .collect();
        assert_eq!(
            warns.len(),
            1,
            "process without memory binding should warn: {:?}",
            diags
        );
    }

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
        let diags = BindingRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("missing Actual_Memory_Binding")
                    && d.message.contains("proc")
            })
            .collect();
        assert!(
            warns.is_empty(),
            "bound process should not warn: {:?}",
            warns
        );
    }

    #[test]
    fn no_memory_process_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Actual_Memory_Binding"))
            .collect();
        assert!(
            warns.is_empty(),
            "no memory = no binding needed: {:?}",
            warns
        );
    }

    // ── BIND-TARGET-EXISTS tests ────────────────────────────────────

    #[test]
    fn binding_target_exists_no_error() {
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
        let diags = BindingRuleAnalysis.analyze(&inst);
        let exists_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("does not exist"))
            .collect();
        assert!(
            exists_errs.is_empty(),
            "existing target should not error: {:?}",
            exists_errs
        );
    }

    #[test]
    fn binding_target_not_found_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (nonexistent)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let exists_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("does not exist"))
            .collect();
        assert_eq!(
            exists_errs.len(),
            1,
            "nonexistent target should error: {:?}",
            diags
        );
    }

    #[test]
    fn binding_target_unparseable_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "something_unparseable",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let exists_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("does not exist"))
            .collect();
        assert!(
            exists_errs.is_empty(),
            "unparseable target should skip check: {:?}",
            exists_errs
        );
    }

    // ── BIND-TARGET-CATEGORY tests ──────────────────────────────────

    #[test]
    fn processor_binding_to_processor_no_error() {
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
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("expected one of"))
            .collect();
        assert!(
            cat_errs.is_empty(),
            "processor binding to processor: {:?}",
            cat_errs
        );
    }

    #[test]
    fn processor_binding_to_memory_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![mem, cpu, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (mem)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("memory")
                    && d.message.contains("expected one of")
            })
            .collect();
        assert_eq!(
            cat_errs.len(),
            1,
            "binding to memory for processor: {:?}",
            diags
        );
    }

    #[test]
    fn memory_binding_to_memory_no_error() {
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
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("Actual_Memory_Binding")
                    && d.message.contains("expected one of")
            })
            .collect();
        assert!(
            cat_errs.is_empty(),
            "memory binding to memory: {:?}",
            cat_errs
        );
    }

    // ── extract_reference_target tests ──────────────────────────────

    #[test]
    fn extract_reference_patterns() {
        assert_eq!(extract_reference_target("reference (cpu1)"), Some("cpu1"));
        assert_eq!(extract_reference_target("(reference (cpu1))"), Some("cpu1"));
        assert_eq!(extract_reference_target("reference(mem)"), Some("mem"));
        assert_eq!(extract_reference_target("invalid"), None);
        assert_eq!(extract_reference_target(""), None);
    }
}
