//! SVG architecture visualization for AADL models.
//!
//! Converts an AADL `SystemInstance` into a compound hierarchical graph using
//! the `etch` crate's Sugiyama-based layout, then renders to SVG with
//! AADL-standard category colors and nested container boxes.

use std::collections::HashMap;

use etch::layout::{EdgeInfo, LayoutOptions, NodeInfo};
use etch::svg::{SvgOptions, render_svg};
use petgraph::Graph;
use petgraph::graph::{EdgeIndex, NodeIndex};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

/// Render an AADL system instance to SVG.
///
/// The resulting SVG uses compound layout — containers (systems, processes)
/// visually enclose their children, with connections drawn as edges.
pub fn render_instance(instance: &SystemInstance, options: &RenderOptions) -> String {
    let (graph, node_infos, edge_infos) = build_graph(instance, options);

    let layout_opts = LayoutOptions {
        node_width: options.node_width,
        node_height: options.node_height,
        rank_separation: options.rank_separation,
        node_separation: options.node_separation,
        container_padding: options.container_padding,
        container_header: options.container_header,
        ..Default::default()
    };

    let gl = etch::layout::layout(
        &graph,
        &|idx, _: &ComponentInstanceIdx| node_infos[&idx].clone(),
        &|idx, _: &()| {
            edge_infos
                .get(&idx)
                .cloned()
                .unwrap_or(EdgeInfo { label: String::new() })
        },
        &layout_opts,
    );

    let svg_opts = SvgOptions {
        type_colors: category_colors(),
        interactive: options.interactive,
        base_url: options.base_url.clone(),
        highlight: options.highlight.clone(),
        ..Default::default()
    };

    render_svg(&gl, &svg_opts)
}

/// Options for AADL architecture rendering.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Width of leaf node boxes (px).
    pub node_width: f64,
    /// Height of leaf node boxes (px).
    pub node_height: f64,
    /// Vertical spacing between ranks.
    pub rank_separation: f64,
    /// Horizontal spacing between sibling nodes.
    pub node_separation: f64,
    /// Padding inside container nodes.
    pub container_padding: f64,
    /// Height of container header labels.
    pub container_header: f64,
    /// Emit interactive data-* attributes.
    pub interactive: bool,
    /// Base URL for data-href attributes.
    pub base_url: Option<String>,
    /// Node ID to highlight.
    pub highlight: Option<String>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            node_width: 180.0,
            node_height: 50.0,
            rank_separation: 80.0,
            node_separation: 40.0,
            container_padding: 20.0,
            container_header: 30.0,
            interactive: false,
            base_url: None,
            highlight: None,
        }
    }
}

/// Build a petgraph from the AADL instance model.
fn build_graph(
    instance: &SystemInstance,
    _options: &RenderOptions,
) -> (
    Graph<ComponentInstanceIdx, ()>,
    HashMap<NodeIndex, NodeInfo>,
    HashMap<EdgeIndex, EdgeInfo>,
) {
    let mut graph = Graph::new();
    let mut idx_map: HashMap<ComponentInstanceIdx, NodeIndex> = HashMap::new();
    let mut node_infos: HashMap<NodeIndex, NodeInfo> = HashMap::new();
    let mut edge_infos: HashMap<EdgeIndex, EdgeInfo> = HashMap::new();

    // Add all component instances as nodes.
    for (ci_idx, comp) in instance.all_components() {
        let node_idx = graph.add_node(ci_idx);
        idx_map.insert(ci_idx, node_idx);

        let label = if let Some(arr_idx) = comp.array_index {
            format!("{}[{}]", comp.name, arr_idx)
        } else {
            comp.name.to_string()
        };

        let sublabel = comp
            .impl_name
            .as_ref()
            .map(|impl_name| format!("{}::{}.{}", comp.package, comp.type_name, impl_name));

        // Parent in the containment hierarchy — enables compound layout.
        let parent = if ci_idx == instance.root {
            None
        } else {
            comp.parent.map(|p| {
                let parent_comp = instance.component(p);
                node_id(parent_comp, p)
            })
        };

        let info = NodeInfo {
            id: node_id(comp, ci_idx),
            label,
            node_type: category_type_name(comp.category).to_string(),
            sublabel,
            parent,
        };

        node_infos.insert(node_idx, info);
    }

    // Add connection edges.
    for (_conn_idx, conn) in instance.connections.iter() {
        let src_ci = resolve_connection_end(instance, conn.owner, &conn.src);
        let dst_ci = resolve_connection_end(instance, conn.owner, &conn.dst);

        if let (Some(src), Some(dst)) = (src_ci, dst_ci) {
            if let (Some(&src_node), Some(&dst_node)) = (idx_map.get(&src), idx_map.get(&dst)) {
                if src_node != dst_node {
                    let edge_idx = graph.add_edge(src_node, dst_node, ());
                    edge_infos.insert(
                        edge_idx,
                        EdgeInfo {
                            label: conn.name.to_string(),
                        },
                    );
                }
            }
        }
    }

    (graph, node_infos, edge_infos)
}

/// Resolve a connection end to a component instance index.
fn resolve_connection_end(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    end: &Option<spar_hir_def::instance::ConnectionEnd>,
) -> Option<ComponentInstanceIdx> {
    match end {
        Some(conn_end) => {
            if let Some(ref sub_name) = conn_end.subcomponent {
                let owner_comp = instance.component(owner);
                owner_comp.children.iter().find_map(|&child_idx| {
                    let child = instance.component(child_idx);
                    if child.name.as_str() == sub_name.as_str() {
                        Some(child_idx)
                    } else {
                        None
                    }
                })
            } else {
                Some(owner)
            }
        }
        None => None,
    }
}

/// Generate a stable node ID for a component instance.
fn node_id(
    comp: &spar_hir_def::instance::ComponentInstance,
    _idx: ComponentInstanceIdx,
) -> String {
    if let Some(arr_idx) = comp.array_index {
        format!("AADL-{}-{}_{}", comp.package, comp.name, arr_idx)
    } else {
        format!("AADL-{}-{}", comp.package, comp.name)
    }
}

/// Map ComponentCategory to a node type string for etch theming.
fn category_type_name(cat: ComponentCategory) -> &'static str {
    match cat {
        ComponentCategory::System => "system",
        ComponentCategory::Process => "process",
        ComponentCategory::Thread => "thread",
        ComponentCategory::ThreadGroup => "thread-group",
        ComponentCategory::Processor => "processor",
        ComponentCategory::VirtualProcessor => "virtual-processor",
        ComponentCategory::Memory => "memory",
        ComponentCategory::Bus => "bus",
        ComponentCategory::VirtualBus => "virtual-bus",
        ComponentCategory::Device => "device",
        ComponentCategory::Data => "data",
        ComponentCategory::Subprogram => "subprogram",
        ComponentCategory::SubprogramGroup => "subprogram-group",
        ComponentCategory::Abstract => "abstract",
    }
}

/// Standard AADL category colors.
fn category_colors() -> HashMap<String, String> {
    [
        ("system", "#b3d9ff"),
        ("process", "#d4edda"),
        ("thread", "#fff3cd"),
        ("thread-group", "#fff3cd"),
        ("processor", "#f8d7da"),
        ("virtual-processor", "#f8d7da"),
        ("memory", "#e8e8e8"),
        ("bus", "#e8e8e8"),
        ("virtual-bus", "#e8e8e8"),
        ("device", "#e2d5f1"),
        ("data", "#fce4ec"),
        ("subprogram", "#e8e8e8"),
        ("subprogram-group", "#e8e8e8"),
        ("abstract", "#e8e8e8"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_colors_has_all_categories() {
        let colors = category_colors();
        assert!(colors.contains_key("system"));
        assert!(colors.contains_key("process"));
        assert!(colors.contains_key("thread"));
        assert!(colors.contains_key("processor"));
        assert!(colors.contains_key("device"));
        assert!(colors.contains_key("memory"));
        assert!(colors.contains_key("bus"));
        assert!(colors.contains_key("data"));
    }

    #[test]
    fn category_type_names_are_kebab_case() {
        assert_eq!(
            category_type_name(ComponentCategory::VirtualProcessor),
            "virtual-processor"
        );
        assert_eq!(
            category_type_name(ComponentCategory::ThreadGroup),
            "thread-group"
        );
        assert_eq!(
            category_type_name(ComponentCategory::VirtualBus),
            "virtual-bus"
        );
    }

    #[test]
    fn default_render_options() {
        let opts = RenderOptions::default();
        assert_eq!(opts.node_width, 180.0);
        assert!(!opts.interactive);
    }
}
