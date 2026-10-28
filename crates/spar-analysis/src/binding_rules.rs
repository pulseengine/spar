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
                    instance.reference_context(
                        comp_idx,
                        "Deployment_Properties",
                        "Actual_Processor_Binding",
                    ),
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
                    instance.reference_context(
                        comp_idx,
                        "Deployment_Properties",
                        "Actual_Memory_Binding",
                    ),
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

        // BIND-TARGET-EXISTS + BIND-TARGET-CATEGORY for connections (#539,
        // REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001). A connection's own
        // `{ … }` value and an `applies to <conn>` value from an enclosing
        // implementation both live in the connection's property map; each
        // resolves from the component that declared it
        // (connection_reference_context), never by a model-wide name search.
        for (conn_idx, conn) in instance.connections.iter() {
            let props = instance.properties_for_connection(conn_idx);
            let Some(binding_val) = props
                .get("Deployment_Properties", ACB)
                .or_else(|| props.get("", ACB))
            else {
                continue;
            };
            check_binding_target(
                instance,
                instance.connection_reference_context(conn_idx, "Deployment_Properties", ACB),
                binding_val,
                &format!("{ACB} of connection '{}'", conn.name),
                CONNECTION_BINDING_CATEGORIES,
                &component_path(instance, conn.owner),
                &mut diags,
            );
        }

        diags
    }
}

const ACB: &str = "Actual_Connection_Binding";

/// The categories `Actual_Connection_Binding` may reference:
/// Deployment_Properties.aadl:96-97 (osate2 @ 87468992) declares it
/// `list of reference (processor, virtual processor, bus, virtual bus,
/// device, memory, system, abstract)`.
const CONNECTION_BINDING_CATEGORIES: &[ComponentCategory] = &[
    ComponentCategory::Processor,
    ComponentCategory::VirtualProcessor,
    ComponentCategory::Bus,
    ComponentCategory::VirtualBus,
    ComponentCategory::Device,
    ComponentCategory::Memory,
    ComponentCategory::System,
    ComponentCategory::Abstract,
];

/// BIND-TARGET-EXISTS + BIND-TARGET-CATEGORY: Validate that a binding
/// target exists and has an appropriate category.
///
/// `ctx` is the component the value's `reference (...)` resolves from — the
/// one that DECLARED the association (REQ-AADL-REF-CONTEXT-001). `label`
/// opens each message: the property name, plus the connection for a
/// connection binding.
fn check_binding_target(
    instance: &SystemInstance,
    ctx: spar_hir_def::instance::ComponentInstanceIdx,
    binding_val: &str,
    label: &str,
    allowed_categories: &[ComponentCategory],
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let target_name = match extract_reference_target(binding_val) {
        Some(name) => name,
        None => return,
    };

    // BIND-TARGET-EXISTS: resolve the reference by dotted instance path
    // (`reference (cvc.soc.helix_linux)`) as well as by bare name. A
    // bare-name-only match wrongly reports multi-level subcomponent
    // references as "does not exist" — they name a component by its
    // containment path, not a top-level identifier. See REQ-BINDING-002.
    // Resolved from the component that DECLARED the association, walking
    // child subcomponents exactly, as OSATE does — never the deepest
    // same-named component anywhere in the model (#460,
    // REQ-AADL-REF-CONTEXT-001).
    match instance.resolve_reference(ctx, target_name) {
        Some(idx) => {
            // BIND-TARGET-CATEGORY: Check that the category is appropriate
            let comp = instance.component(idx);
            if !allowed_categories.contains(&comp.category) {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "{} target '{}' is a {} component, expected one of: {}",
                        label,
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
        }
        None => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "{} references '{}' which does not exist in the instance model",
                    label, target_name
                ),
                path: path.to_vec(),
                analysis: "binding_rules".to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ConnectionKind;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
        connection_property_maps: FxHashMap<ConnectionInstanceIdx, PropertyMap>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                property_maps: FxHashMap::default(),
                connection_property_maps: FxHashMap::default(),
            }
        }

        /// A port connection `name` owned by `owner`.
        fn add_connection(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
        ) -> ConnectionInstanceIdx {
            let ci = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: None,
                dst: None,
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(ci);
            ci
        }

        /// Set `Actual_Connection_Binding` on connection `conn`: declared on
        /// the connection itself when `ctx` is `None`, otherwise through
        /// `applies to` from `ctx`.
        fn set_connection_binding(
            &mut self,
            conn: ConnectionInstanceIdx,
            ctx: Option<ComponentInstanceIdx>,
            value: &str,
        ) {
            let pv = PropertyValue {
                name: PropertyRef {
                    property_set: Some(Name::new("Deployment_Properties")),
                    property_name: Name::new("Actual_Connection_Binding"),
                },
                value: value.to_string(),
                typed_expr: None,
                is_append: false,
            };
            let map = self.connection_property_maps.entry(conn).or_default();
            match ctx {
                Some(ctx) => map.add_with_reference_context(pv, ctx),
                None => map.add(pv),
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

        /// Set a property as an `applies to` association declared on `ctx`
        /// would: its `reference (...)` terms resolve from `ctx`, not from
        /// `comp` (REQ-AADL-REF-CONTEXT-001). A binding to a component
        /// elsewhere in the tree is written this way in real AADL —
        /// `Actual_Processor_Binding => (reference (cpu)) applies to worker;`
        /// in the enclosing implementation.
        fn set_property_from(
            &mut self,
            ctx: ComponentInstanceIdx,
            comp: ComponentInstanceIdx,
            set: &str,
            name: &str,
            value: &str,
        ) {
            let map = self.property_maps.entry(comp).or_default();
            map.add_with_reference_context(
                PropertyValue {
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
                },
                ctx,
            );
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
                feature_property_maps: Default::default(),
                connection_property_maps: self.connection_property_maps,
                emv2_models: Default::default(),
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
        b.set_property_from(
            root,
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
        b.set_property_from(
            root,
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

    // ── Memory binding to thread (wrong category) ─────────────────

    #[test]
    fn memory_binding_to_thread_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mem = b.add_component("mem", ComponentCategory::Memory, Some(root));
        let worker = b.add_component("worker", ComponentCategory::Thread, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![mem, worker, proc]);
        b.set_property_from(
            root,
            proc,
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (worker)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("Actual_Memory_Binding")
                    && d.message.contains("thread")
            })
            .collect();
        assert_eq!(
            cat_errs.len(),
            1,
            "memory binding to thread should error: {:?}",
            diags
        );
    }

    // ── Virtual processor binding accepted ───────────────────────────

    #[test]
    fn processor_binding_to_virtual_processor_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let vp = b.add_component("vp", ComponentCategory::VirtualProcessor, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![vp, thread]);
        b.set_property(
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (vp)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("expected one of"))
            .collect();
        assert!(
            cat_errs.is_empty(),
            "virtual processor binding should be valid: {:?}",
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

    // ── BIND-TARGET-EXISTS: multi-level dotted reference paths ──────

    #[test]
    fn nested_dotted_processor_binding_resolves() {
        // Regression (ee_system.poc user report): `reference
        // (cvc.soc.helix_linux)` names a processor by its multi-level
        // containment path, not a top-level identifier. A bare-name match
        // wrongly reported it as "does not exist". It must resolve via the
        // dotted instance path.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cvc = b.add_component("cvc", ComponentCategory::System, Some(root));
        let soc = b.add_component("soc", ComponentCategory::System, Some(cvc));
        let helix = b.add_component("helix_linux", ComponentCategory::Processor, Some(soc));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cvc, proc]);
        b.set_children(cvc, vec![soc]);
        b.set_children(soc, vec![helix]);
        b.set_children(proc, vec![thread]);
        b.set_property_from(
            root,
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cvc.soc.helix_linux)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let not_found: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("does not exist in the instance model"))
            .collect();
        assert!(
            not_found.is_empty(),
            "nested dotted binding path should resolve, got: {diags:?}"
        );
    }

    #[test]
    fn nested_dotted_binding_to_nonprocessor_still_flags_category() {
        // Dotted-path resolution must not lose category checking: binding a
        // thread to a nested *memory* component is still a category error.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cvc = b.add_component("cvc", ComponentCategory::System, Some(root));
        let mem = b.add_component("dram", ComponentCategory::Memory, Some(cvc));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let thread = b.add_component("worker", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cvc, proc]);
        b.set_children(cvc, vec![mem]);
        b.set_children(proc, vec![thread]);
        b.set_property_from(
            root,
            thread,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cvc.dram)",
        );

        let inst = b.build(root);
        let diags = BindingRuleAnalysis.analyze(&inst);
        let cat_err: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message
                        .contains("Actual_Processor_Binding target 'cvc.dram'")
            })
            .collect();
        assert_eq!(
            cat_err.len(),
            1,
            "binding a thread to a nested memory component should flag category: {diags:?}"
        );
    }

    // ── Actual_Connection_Binding (#539) ────────────────────────────

    /// `root` holds `net` (a bus) and the system `node`; `node` holds `local`
    /// (a bus), the process `proc`, and owns connection `c`.
    fn conn_base() -> (
        TestBuilder,
        ComponentInstanceIdx,
        ComponentInstanceIdx,
        ConnectionInstanceIdx,
    ) {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let net = b.add_component("net", ComponentCategory::Bus, Some(root));
        let node = b.add_component("node", ComponentCategory::System, Some(root));
        let local = b.add_component("local", ComponentCategory::Bus, Some(node));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(node));
        b.set_children(root, vec![net, node]);
        b.set_children(node, vec![local, proc]);
        let c = b.add_connection("c", node);
        (b, root, node, c)
    }

    fn acb_diags(b: TestBuilder, root: ComponentInstanceIdx) -> Vec<AnalysisDiagnostic> {
        BindingRuleAnalysis
            .analyze(&b.build(root))
            .into_iter()
            .filter(|d| d.message.contains("Actual_Connection_Binding"))
            .collect()
    }

    // rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
    #[test]
    fn connection_binding_to_a_missing_name_is_an_error() {
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, None, "(reference (no_such_thing))");
        let diags = acb_diags(b, root);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[0].message,
            "Actual_Connection_Binding of connection 'c' references 'no_such_thing' \
             which does not exist in the instance model"
        );
        assert_eq!(diags[0].path, vec!["root".to_string(), "node".to_string()]);
        assert_eq!(diags[0].analysis, "binding_rules");
    }

    // rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
    #[test]
    fn connection_own_binding_resolves_from_its_owner() {
        // `local` is a child of the owner `node`: resolves, a bus, silent.
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, None, "(reference (local))");
        assert!(acb_diags(b, root).is_empty());
        // `net` is a child of `root`, not of the owner: it does not resolve
        // from `node`, and no model-wide name search finds it.
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, None, "(reference (net))");
        let diags = acb_diags(b, root);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("'net' which does not exist"));
    }

    // rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
    #[test]
    fn applies_to_connection_binding_resolves_from_its_declarer() {
        // Declared in `root` (applies to node.c): `net` resolves from root.
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, Some(root), "(reference (net))");
        assert!(acb_diags(b, root).is_empty());
        // ...and `local`, a child of the owner but not of the declarer, does not.
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, Some(root), "(reference (local))");
        let diags = acb_diags(b, root);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("'local' which does not exist"));
        // A dotted path from the declarer resolves.
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, Some(root), "(reference (node.local))");
        assert!(acb_diags(b, root).is_empty());
    }

    // rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
    #[test]
    fn connection_binding_to_a_process_is_the_wrong_category() {
        let (mut b, root, _, c) = conn_base();
        b.set_connection_binding(c, None, "(reference (proc))");
        let diags = acb_diags(b, root);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[0].message,
            "Actual_Connection_Binding of connection 'c' target 'proc' is a process component, \
             expected one of: processor, virtual processor, bus, virtual bus, device, memory, \
             system, abstract"
        );
    }

    // rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
    #[test]
    fn connection_binding_accepts_every_deployment_properties_category() {
        for cat in [
            ComponentCategory::Processor,
            ComponentCategory::VirtualProcessor,
            ComponentCategory::Bus,
            ComponentCategory::VirtualBus,
            ComponentCategory::Device,
            ComponentCategory::Memory,
            ComponentCategory::System,
            ComponentCategory::Abstract,
        ] {
            let (mut b, root, node, c) = conn_base();
            let t = b.add_component("t", cat, Some(node));
            b.components[node].children.push(t);
            b.set_connection_binding(c, None, "(reference (t))");
            let diags = acb_diags(b, root);
            assert!(diags.is_empty(), "{cat}: {diags:?}");
        }
        for cat in [
            ComponentCategory::Process,
            ComponentCategory::Thread,
            ComponentCategory::Data,
            ComponentCategory::Subprogram,
        ] {
            let (mut b, root, node, c) = conn_base();
            let t = b.add_component("t", cat, Some(node));
            b.components[node].children.push(t);
            b.set_connection_binding(c, None, "(reference (t))");
            assert_eq!(acb_diags(b, root).len(), 1, "{cat} is not a legal target");
        }
    }
}
