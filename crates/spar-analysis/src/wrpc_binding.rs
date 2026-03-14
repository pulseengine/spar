//! wRPC binding validation analysis.
//!
//! Checks that connections between components bound to different processors
//! have an `Actual_Connection_Binding` property pointing to a bus.
//!
//! In AADL, when two communicating components are deployed to different
//! processors, the connection between them must be bound to a bus (or
//! virtual bus) to model the transport. This analysis warns when such
//! a binding is missing, which helps catch incomplete deployment models.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates that cross-processor connections have bus bindings.
///
/// For each connection in the instance model:
/// 1. Resolves source and destination subcomponents
/// 2. Checks whether they have different `Actual_Processor_Binding` values
/// 3. If so, checks that the owning component has an `Actual_Connection_Binding`
///    property covering the connection
/// 4. Emits a warning if the binding is missing
pub struct WrpcBindingAnalysis;

impl Analysis for WrpcBindingAnalysis {
    fn name(&self) -> &str {
        "wrpc-binding"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diagnostics = Vec::new();

        // Only run if the model has multiple processors — otherwise cross-processor
        // connections are impossible.
        let processor_count = instance
            .all_components()
            .filter(|(_, c)| {
                matches!(
                    c.category,
                    ComponentCategory::Processor | ComponentCategory::VirtualProcessor
                )
            })
            .count();
        if processor_count < 2 {
            return diagnostics;
        }

        // Walk all connection instances.
        for (_conn_idx, conn) in instance.connections.iter() {
            let owner = conn.owner;
            let owner_comp = instance.component(owner);

            // Resolve source and destination subcomponent instances.
            let src_sub = conn
                .src
                .as_ref()
                .and_then(|end| end.subcomponent.as_ref())
                .and_then(|sub_name| find_child_by_name(instance, owner, sub_name.as_str()));

            let dst_sub = conn
                .dst
                .as_ref()
                .and_then(|end| end.subcomponent.as_ref())
                .and_then(|sub_name| find_child_by_name(instance, owner, sub_name.as_str()));

            // Both endpoints must resolve to subcomponents for this check.
            let (src_idx, dst_idx) = match (src_sub, dst_sub) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Get processor bindings for each endpoint.
            let src_binding = get_processor_binding(instance, src_idx);
            let dst_binding = get_processor_binding(instance, dst_idx);

            // If both have processor bindings and they differ, the connection
            // crosses a processor boundary.
            let crosses_boundary = match (&src_binding, &dst_binding) {
                (Some(src_b), Some(dst_b)) => !src_b.eq_ignore_ascii_case(dst_b),
                _ => false,
            };

            if !crosses_boundary {
                continue;
            }

            // Check if the connection has an Actual_Connection_Binding.
            // Connection bindings are typically set on the owning component
            // via `applies to` or directly on the component's property map.
            let owner_props = instance.properties_for(owner);
            let has_conn_binding = owner_props
                .get("Deployment_Properties", "Actual_Connection_Binding")
                .is_some()
                || owner_props.get("", "Actual_Connection_Binding").is_some();

            if !has_conn_binding {
                let path = component_path(instance, owner);
                diagnostics.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "connection '{}' in '{}' crosses processor boundary \
                         (source bound to {}, destination bound to {}) \
                         but has no Actual_Connection_Binding to a bus",
                        conn.name,
                        owner_comp.name,
                        src_binding.as_deref().unwrap_or("?"),
                        dst_binding.as_deref().unwrap_or("?"),
                    ),
                    path,
                    analysis: "wrpc-binding".to_string(),
                });
            }
        }

        diagnostics
    }
}

/// Find a child component instance by name within a parent.
fn find_child_by_name(
    instance: &SystemInstance,
    parent: ComponentInstanceIdx,
    name: &str,
) -> Option<ComponentInstanceIdx> {
    let parent_comp = instance.component(parent);
    for &child_idx in &parent_comp.children {
        let child = instance.component(child_idx);
        if child.name.as_str().eq_ignore_ascii_case(name) {
            return Some(child_idx);
        }
    }
    None
}

/// Get the Actual_Processor_Binding value for a component, if set.
///
/// Walks up the hierarchy: if a thread has a binding, use it; otherwise
/// check its parent process, etc.
fn get_processor_binding(instance: &SystemInstance, idx: ComponentInstanceIdx) -> Option<String> {
    let mut current = Some(idx);
    while let Some(ci) = current {
        let props = instance.properties_for(ci);
        if let Some(val) = props
            .get("Deployment_Properties", "Actual_Processor_Binding")
            .or_else(|| props.get("", "Actual_Processor_Binding"))
        {
            return Some(val.to_string());
        }
        current = instance.component(ci).parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
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

        fn add_connection(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: &str,
            src_feat: &str,
            dst_sub: &str,
            dst_feat: &str,
        ) -> ConnectionInstanceIdx {
            let conn_idx = self.connections.alloc(ConnectionInstance {
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
            self.components[owner].connections.push(conn_idx);
            conn_idx
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
    fn cross_processor_connection_without_bus_binding_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sender, receiver]);

        // Bind sender to cpu1, receiver to cpu2
        b.set_property(
            sender,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(
            receiver,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Add connection between sender and receiver, no bus binding
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");

        let inst = b.build(root);
        let diags = WrpcBindingAnalysis.analyze(&inst);
        assert_eq!(
            diags.len(),
            1,
            "should warn about missing bus binding: {:?}",
            diags
        );
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("Actual_Connection_Binding"));
        assert!(diags[0].message.contains("c1"));
    }

    #[test]
    fn cross_processor_connection_with_bus_binding_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sender, receiver]);

        b.set_property(
            sender,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(
            receiver,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Connection with bus binding on owner
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (nats_bus)",
        );

        let inst = b.build(root);
        let diags = WrpcBindingAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "should not warn when bus binding exists: {:?}",
            diags
        );
    }

    #[test]
    fn same_processor_connection_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sender, receiver]);

        // Both bound to same processor
        b.set_property(
            sender,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(
            receiver,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");

        let inst = b.build(root);
        let diags = WrpcBindingAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "same processor = no warning: {:?}", diags);
    }

    #[test]
    fn single_processor_model_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu, sender, receiver]);

        b.set_property(
            sender,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu)",
        );
        b.set_property(
            receiver,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu)",
        );

        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");

        let inst = b.build(root);
        let diags = WrpcBindingAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "single processor model = skip: {:?}",
            diags
        );
    }

    #[test]
    fn no_processor_bindings_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("top", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sender, receiver]);

        // No processor bindings set — can't determine cross-processor
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");

        let inst = b.build(root);
        let diags = WrpcBindingAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "no bindings = can't determine boundary: {:?}",
            diags
        );
    }
}
