//! Hardware topology graph extraction from AADL instance models.
//!
//! Walks a `SystemInstance` and extracts a petgraph `DiGraph` containing
//! only hardware platform components: processors, virtual processors,
//! memories, buses, and virtual buses. Edges represent bus access
//! connectivity derived from access connections and connection bindings.
//!
//! # Properties extracted
//!
//! - **Memory**: `Memory_Size` (converted from bits to bytes)
//! - **Bus**: bandwidth capacity via `SEI::Bandwidth`, `Communication_Properties::Bandwidth`,
//!   or `Data_Rate` (bits per second), plus protocol from `Communication_Properties::Protocol`
//! - **Processor**: `utilization_budget` and `memory_bytes` are placeholders for future work

use petgraph::graph::{DiGraph, NodeIndex};
use rustc_hash::FxHashMap;
use spar_analysis::property_accessors::{extract_reference_target, get_size_property};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind};

/// A hardware node in the topology graph.
#[derive(Debug, Clone)]
pub enum HwNode {
    /// A processor or virtual processor.
    Processor {
        idx: ComponentInstanceIdx,
        name: String,
        utilization_budget: Option<f64>,
        memory_bytes: Option<u64>,
    },
    /// A memory component.
    Memory {
        idx: ComponentInstanceIdx,
        name: String,
        size_bytes: Option<u64>,
    },
    /// A bus or virtual bus.
    Bus {
        idx: ComponentInstanceIdx,
        name: String,
        bandwidth_bps: Option<f64>,
        protocol: Option<String>,
    },
}

impl HwNode {
    /// Return the component instance index for this node.
    pub fn component_idx(&self) -> ComponentInstanceIdx {
        match self {
            HwNode::Processor { idx, .. } => *idx,
            HwNode::Memory { idx, .. } => *idx,
            HwNode::Bus { idx, .. } => *idx,
        }
    }

    /// Return the name of this node.
    pub fn name(&self) -> &str {
        match self {
            HwNode::Processor { name, .. } => name,
            HwNode::Memory { name, .. } => name,
            HwNode::Bus { name, .. } => name,
        }
    }
}

/// An edge representing bus connectivity between hardware nodes.
#[derive(Debug, Clone)]
pub struct BusEdge {
    /// Name of the bus providing the connection.
    pub bus_name: String,
}

/// A hardware topology graph extracted from an AADL system instance.
///
/// Nodes are processors, memories, and buses. Edges represent bus access
/// connectivity: when a processor or memory is connected to a bus via an
/// access connection or binding, an edge is created between them.
#[derive(Debug)]
pub struct TopologyGraph {
    /// The underlying petgraph directed graph.
    pub graph: DiGraph<HwNode, BusEdge>,
    /// Map from AADL component instance index to petgraph node index.
    pub idx_map: FxHashMap<ComponentInstanceIdx, NodeIndex>,
}

impl TopologyGraph {
    /// Extract a hardware topology graph from a system instance.
    ///
    /// Walks all components, filters for hardware platform categories
    /// (Processor, VirtualProcessor, Memory, Bus, VirtualBus), and creates
    /// graph nodes with extracted property values. Bus access connections
    /// and connection bindings create edges between nodes.
    pub fn from_instance(instance: &SystemInstance) -> Self {
        let mut graph = DiGraph::new();
        let mut idx_map = FxHashMap::default();

        // Phase 1: Create nodes for all hardware components.
        for (comp_idx, comp) in instance.all_components() {
            let node = match comp.category {
                ComponentCategory::Processor | ComponentCategory::VirtualProcessor => {
                    Some(HwNode::Processor {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        utilization_budget: None, // placeholder for future work
                        memory_bytes: None,       // placeholder for future work
                    })
                }
                ComponentCategory::Memory => {
                    let props = instance.properties_for(comp_idx);
                    // Memory_Size is in bits per AADL property convention; convert to bytes.
                    let size_bytes = get_size_property(props, "Memory_Size").map(|bits| bits / 8);
                    Some(HwNode::Memory {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        size_bytes,
                    })
                }
                ComponentCategory::Bus | ComponentCategory::VirtualBus => {
                    let props = instance.properties_for(comp_idx);
                    let bandwidth_bps = get_bandwidth_capacity(props);
                    let protocol = props
                        .get("Communication_Properties", "Protocol")
                        .or_else(|| props.get("", "Protocol"))
                        .map(|s| s.to_string());
                    Some(HwNode::Bus {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        bandwidth_bps,
                        protocol,
                    })
                }
                _ => None,
            };

            if let Some(hw_node) = node {
                let node_idx = graph.add_node(hw_node);
                idx_map.insert(comp_idx, node_idx);
            }
        }

        // Phase 2: Create edges from bus access connections.
        //
        // Walk all connection instances. For Access connections involving
        // a bus (identified by either endpoint being a BusAccess feature
        // or connecting to a Bus/VirtualBus component), create an edge
        // between the two hardware nodes (if both are in our graph).
        for (_, conn) in instance.connections.iter() {
            if conn.kind != ConnectionKind::Access {
                continue;
            }

            let (src_end, dst_end) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Resolve component indices for each endpoint.
            let src_comp = resolve_endpoint_component(instance, conn.owner, src_end);
            let dst_comp = resolve_endpoint_component(instance, conn.owner, dst_end);

            let (src_comp_idx, dst_comp_idx) = match (src_comp, dst_comp) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Both endpoints must be hardware nodes in our graph.
            let (src_node, dst_node) =
                match (idx_map.get(&src_comp_idx), idx_map.get(&dst_comp_idx)) {
                    (Some(&s), Some(&d)) => (s, d),
                    _ => continue,
                };

            // Determine bus name: one side should be a bus.
            let bus_name = if is_bus_node(&graph[src_node]) {
                graph[src_node].name().to_string()
            } else if is_bus_node(&graph[dst_node]) {
                graph[dst_node].name().to_string()
            } else {
                continue;
            };

            // Add edge from src to dst (and reverse for bidirectional).
            if !graph.contains_edge(src_node, dst_node) {
                graph.add_edge(
                    src_node,
                    dst_node,
                    BusEdge {
                        bus_name: bus_name.clone(),
                    },
                );
            }
            if conn.is_bidirectional && !graph.contains_edge(dst_node, src_node) {
                graph.add_edge(dst_node, src_node, BusEdge { bus_name });
            }
        }

        // Phase 3: Create edges from Actual_Connection_Binding properties.
        //
        // Components with `Actual_Connection_Binding => reference(bus_name)`
        // indicate that their connections are routed through a specific bus.
        // We create edges between the bound component (if it's a hardware node)
        // and the referenced bus.
        let bus_name_to_idx: FxHashMap<String, ComponentInstanceIdx> = instance
            .all_components()
            .filter(|(_, c)| {
                matches!(
                    c.category,
                    ComponentCategory::Bus | ComponentCategory::VirtualBus
                )
            })
            .map(|(idx, c)| (c.name.as_str().to_lowercase(), idx))
            .collect();

        for (comp_idx, _comp) in instance.all_components() {
            let props = instance.properties_for(comp_idx);
            let binding = props
                .get("Deployment_Properties", "Actual_Connection_Binding")
                .or_else(|| props.get("", "Actual_Connection_Binding"));

            if let Some(val) = binding {
                let target_name = extract_reference_target(val)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| val.trim().to_string());

                if let Some(&bus_comp_idx) = bus_name_to_idx.get(&target_name.to_lowercase()) {
                    // If both the component and the bus are in our HW graph, add an edge.
                    if let (Some(&comp_node), Some(&bus_node)) =
                        (idx_map.get(&comp_idx), idx_map.get(&bus_comp_idx))
                    {
                        let bus_name = instance.component(bus_comp_idx).name.as_str().to_string();
                        if !graph.contains_edge(comp_node, bus_node) {
                            graph.add_edge(
                                comp_node,
                                bus_node,
                                BusEdge {
                                    bus_name: bus_name.clone(),
                                },
                            );
                        }
                        if !graph.contains_edge(bus_node, comp_node) {
                            graph.add_edge(bus_node, comp_node, BusEdge { bus_name });
                        }
                    }
                }
            }
        }

        TopologyGraph { graph, idx_map }
    }

    /// Number of processor (and virtual processor) nodes in the graph.
    pub fn processor_count(&self) -> usize {
        self.graph
            .node_weights()
            .filter(|n| matches!(n, HwNode::Processor { .. }))
            .count()
    }

    /// Number of bus (and virtual bus) nodes in the graph.
    pub fn bus_count(&self) -> usize {
        self.graph
            .node_weights()
            .filter(|n| matches!(n, HwNode::Bus { .. }))
            .count()
    }

    /// Number of memory nodes in the graph.
    pub fn memory_count(&self) -> usize {
        self.graph
            .node_weights()
            .filter(|n| matches!(n, HwNode::Memory { .. }))
            .count()
    }

    /// Get the petgraph node indices for all processor nodes.
    pub fn processors(&self) -> Vec<NodeIndex> {
        self.graph
            .node_indices()
            .filter(|&ni| matches!(self.graph[ni], HwNode::Processor { .. }))
            .collect()
    }

    /// Check whether two nodes are connected (in either direction).
    pub fn are_connected(&self, a: NodeIndex, b: NodeIndex) -> bool {
        self.graph.contains_edge(a, b) || self.graph.contains_edge(b, a)
    }
}

/// Resolve a connection endpoint to the component instance it refers to.
///
/// If the endpoint has a subcomponent name, find the child of the owner
/// with that name. Otherwise, the endpoint refers to the owner itself.
fn resolve_endpoint_component(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    end: &spar_hir_def::instance::ConnectionEnd,
) -> Option<ComponentInstanceIdx> {
    match &end.subcomponent {
        Some(sub_name) => {
            let owner_comp = instance.component(owner);
            owner_comp.children.iter().find_map(|&child_idx| {
                let child = instance.component(child_idx);
                if child.name.eq_ci(sub_name) {
                    Some(child_idx)
                } else {
                    None
                }
            })
        }
        None => Some(owner),
    }
}

/// Check if a node is a bus or virtual bus.
fn is_bus_node(node: &HwNode) -> bool {
    matches!(node, HwNode::Bus { .. })
}

/// Get bandwidth capacity of a bus in bits per second.
///
/// Tries the following properties in order:
/// 1. `SEI::Bandwidth`
/// 2. `Communication_Properties::Bandwidth`
/// 3. `Bandwidth` (unqualified)
/// 4. `Communication_Properties::Data_Rate`
/// 5. `Data_Rate` (unqualified)
fn get_bandwidth_capacity(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    let raw = props
        .get("SEI", "Bandwidth")
        .or_else(|| props.get("Communication_Properties", "Bandwidth"))
        .or_else(|| props.get("", "Bandwidth"));

    if let Some(bps) = raw.and_then(parse_bandwidth) {
        return Some(bps);
    }

    let raw = props
        .get("Communication_Properties", "Data_Rate")
        .or_else(|| props.get("", "Data_Rate"));

    if let Some(val) = raw {
        return parse_data_rate(val);
    }

    None
}

/// Parse a bandwidth value string into bits per second.
fn parse_bandwidth(s: &str) -> Option<f64> {
    parse_data_rate(s)
}

/// Parse a data rate value string like "100 KBytesps" into bits per second.
fn parse_data_rate(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(suffix, factor) in DATA_RATE_UNITS {
        if let Some(val) = s
            .strip_suffix(suffix)
            .map(|s| s.trim())
            .and_then(|n| n.parse::<f64>().ok())
        {
            return Some(val * factor);
        }
    }
    // Try plain number (assume bps).
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
