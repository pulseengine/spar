//! Convert a SystemInstance into a petgraph for layout.
//!
//! This module provides the bridge between the AADL instance model
//! (arena-indexed, non-serializable) and a petgraph representation
//! suitable for graph layout, visualization, and export to rivet.

use std::collections::HashMap;

use petgraph::graph::{Graph, NodeIndex};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

/// A node in the architecture graph, representing a component instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchNode {
    /// Unique artifact identifier, e.g. `AADL-FlightSystem-GPS`.
    pub id: String,
    /// Human-readable label (the component instance name).
    pub label: String,
    /// AADL component category.
    pub category: ComponentCategory,
    /// Optional secondary label (e.g. classifier reference).
    pub sublabel: Option<String>,
}

/// An edge in the architecture graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchEdge {
    /// Edge label describing the relationship.
    pub label: String,
}

/// Build a petgraph from a `SystemInstance`.
///
/// Returns the directed graph and a mapping from arena component indices
/// to petgraph `NodeIndex` values.
///
/// The graph contains:
/// - One node per component instance (root + all descendants)
/// - "contains" edges from parent to child
/// - Connection edges using the connection name as label
pub fn build_graph(
    instance: &SystemInstance,
) -> (Graph<ArchNode, ArchEdge>, HashMap<ComponentInstanceIdx, NodeIndex>) {
    let mut graph = Graph::new();
    let mut index_map: HashMap<ComponentInstanceIdx, NodeIndex> = HashMap::new();

    // Recursively add all component nodes starting from the root.
    add_component_recursive(instance, instance.root, &mut graph, &mut index_map);

    // Add connection edges.
    for (_conn_idx, conn) in instance.connections.iter() {
        let owner_comp = &instance.components[conn.owner];

        // Resolve source component: if subcomponent is Some, find the child;
        // otherwise the endpoint is on the owner itself.
        let src_comp_idx = conn.src.as_ref().and_then(|end| {
            match &end.subcomponent {
                Some(sub_name) => {
                    owner_comp.children.iter().find(|&&child_idx| {
                        instance.components[child_idx].name.eq_ci(sub_name)
                    }).copied()
                }
                None => Some(conn.owner),
            }
        });

        // Resolve destination component similarly.
        let dst_comp_idx = conn.dst.as_ref().and_then(|end| {
            match &end.subcomponent {
                Some(sub_name) => {
                    owner_comp.children.iter().find(|&&child_idx| {
                        instance.components[child_idx].name.eq_ci(sub_name)
                    }).copied()
                }
                None => Some(conn.owner),
            }
        });

        if let (Some(src_idx), Some(dst_idx)) = (src_comp_idx, dst_comp_idx) {
            if let (Some(&src_node), Some(&dst_node)) =
                (index_map.get(&src_idx), index_map.get(&dst_idx))
            {
                graph.add_edge(
                    src_node,
                    dst_node,
                    ArchEdge {
                        label: conn.name.to_string(),
                    },
                );
            }
        }
    }

    (graph, index_map)
}

/// Recursively add a component instance and all its children as graph nodes,
/// plus "contains" edges from parent to child.
fn add_component_recursive(
    instance: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    graph: &mut Graph<ArchNode, ArchEdge>,
    index_map: &mut HashMap<ComponentInstanceIdx, NodeIndex>,
) {
    let comp = &instance.components[comp_idx];

    // Build the artifact ID: AADL-{package}-{name}
    let id = format!("AADL-{}-{}", comp.package, comp.name);

    // Build sublabel from classifier reference.
    let sublabel = {
        let mut s = format!("{}::{}", comp.package, comp.type_name);
        if let Some(ref impl_name) = comp.impl_name {
            s.push('.');
            s.push_str(impl_name.as_str());
        }
        Some(s)
    };

    let node = ArchNode {
        id,
        label: comp.name.to_string(),
        category: comp.category,
        sublabel,
    };

    let node_index = graph.add_node(node);
    index_map.insert(comp_idx, node_index);

    // Add "contains" edge from parent to this node.
    if let Some(parent_idx) = comp.parent {
        if let Some(&parent_node) = index_map.get(&parent_idx) {
            graph.add_edge(
                parent_node,
                node_index,
                ArchEdge {
                    label: "contains".to_string(),
                },
            );
        }
    }

    // Recurse into children.
    let children: Vec<ComponentInstanceIdx> = comp.children.clone();
    for child_idx in children {
        add_component_recursive(instance, child_idx, graph, index_map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::{
        ComponentInstance, ConnectionEnd, ConnectionInstance,
    };
    use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind};
    use spar_hir_def::name::Name;

    /// Helper to build a minimal SystemInstance for testing.
    fn make_test_instance() -> SystemInstance {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let connections: Arena<ConnectionInstance> = Arena::default();

        // Root system
        let root_idx = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        // Child process
        let child_idx = components.alloc(ComponentInstance {
            name: Name::new("sensor"),
            category: ComponentCategory::Process,
            type_name: Name::new("Sensor"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        // Link child to root
        components[root_idx].children.push(child_idx);

        SystemInstance {
            root: root_idx,
            components,
            features: Arena::default(),
            connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }

    #[test]
    fn test_basic_graph_structure() {
        let instance = make_test_instance();
        let (graph, index_map) = build_graph(&instance);

        // 2 components → 2 nodes
        assert_eq!(graph.node_count(), 2);
        // 1 "contains" edge (root → child)
        assert_eq!(graph.edge_count(), 1);

        // Both components are in the index map
        assert_eq!(index_map.len(), 2);

        // Verify root node
        let root_node_idx = index_map[&instance.root];
        let root_node = &graph[root_node_idx];
        assert_eq!(root_node.id, "AADL-Pkg-top");
        assert_eq!(root_node.label, "top");
        assert_eq!(root_node.category, ComponentCategory::System);
        assert_eq!(root_node.sublabel.as_deref(), Some("Pkg::Top.impl"));
    }

    #[test]
    fn test_graph_with_connections() {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        // Root system
        let root_idx = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        // Two child components
        let sensor_idx = components.alloc(ComponentInstance {
            name: Name::new("sensor"),
            category: ComponentCategory::Device,
            type_name: Name::new("Sensor"),
            impl_name: None,
            package: Name::new("HW"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        let controller_idx = components.alloc(ComponentInstance {
            name: Name::new("ctrl"),
            category: ComponentCategory::Process,
            type_name: Name::new("Controller"),
            impl_name: Some(Name::new("basic")),
            package: Name::new("SW"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        components[root_idx].children.push(sensor_idx);
        components[root_idx].children.push(controller_idx);

        // Connection: sensor.out_port -> ctrl.in_port
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c1"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root_idx,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("sensor")),
                feature: Name::new("out_port"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("ctrl")),
                feature: Name::new("in_port"),
            }),
        });
        components[root_idx].connections.push(conn_idx);

        let instance = SystemInstance {
            root: root_idx,
            components,
            features: Arena::default(),
            connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };

        let (graph, index_map) = build_graph(&instance);

        // 3 nodes: root, sensor, ctrl
        assert_eq!(graph.node_count(), 3);
        // 2 "contains" edges + 1 connection edge = 3
        assert_eq!(graph.edge_count(), 3);

        // Verify the connection edge exists between sensor and ctrl
        let sensor_node = index_map[&sensor_idx];
        let ctrl_node = index_map[&controller_idx];

        let conn_edge = graph
            .edges_connecting(sensor_node, ctrl_node)
            .find(|e| e.weight().label == "c1");
        assert!(conn_edge.is_some(), "expected connection edge c1 between sensor and ctrl");

        // Verify node IDs follow rivet format
        assert_eq!(graph[sensor_node].id, "AADL-HW-sensor");
        assert_eq!(graph[ctrl_node].id, "AADL-SW-ctrl");
        assert_eq!(
            graph[ctrl_node].sublabel.as_deref(),
            Some("SW::Controller.basic")
        );
    }

    #[test]
    fn test_connection_to_owner_port() {
        // Test a connection where one end is on the containing component (no subcomponent).
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root_idx = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        let child_idx = components.alloc(ComponentInstance {
            name: Name::new("proc"),
            category: ComponentCategory::Process,
            type_name: Name::new("Proc"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        components[root_idx].children.push(child_idx);

        // Connection from root's own port to child's port
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c_in"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root_idx,
            src: Some(ConnectionEnd {
                subcomponent: None, // port on the owner itself
                feature: Name::new("ext_in"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("proc")),
                feature: Name::new("in_port"),
            }),
        });
        components[root_idx].connections.push(conn_idx);

        let instance = SystemInstance {
            root: root_idx,
            components,
            features: Arena::default(),
            connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };

        let (graph, index_map) = build_graph(&instance);

        // 2 nodes, 1 contains + 1 connection = 2 edges
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 2);

        // Connection should go from root (owner) to child
        let root_node = index_map[&root_idx];
        let child_node = index_map[&child_idx];

        let conn_edge = graph
            .edges_connecting(root_node, child_node)
            .find(|e| e.weight().label == "c_in");
        assert!(
            conn_edge.is_some(),
            "expected connection edge c_in from root to child"
        );
    }
}
