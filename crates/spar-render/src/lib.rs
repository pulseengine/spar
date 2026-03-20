//! SVG and interactive HTML architecture visualization for AADL models.
//!
//! Converts an AADL `SystemInstance` into a compound hierarchical graph using
//! the `etch` crate's Sugiyama-based layout, then renders to SVG or
//! interactive HTML with AADL-standard category colors and port visualization.

use std::collections::HashMap;

use etch::layout::{
    EdgeInfo, LayoutOptions, NodeInfo, PortDirection, PortInfo, PortSide, PortType,
};
use etch::svg::{SvgOptions, render_svg};
use petgraph::Graph;
use petgraph::graph::{EdgeIndex, NodeIndex};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};

/// Render an AADL system instance to SVG.
pub fn render_instance(instance: &SystemInstance, options: &RenderOptions) -> String {
    let (graph, node_infos, edge_infos) = build_graph(instance, options);
    let layout_opts = make_layout_opts(options);

    let gl = etch::layout::layout(
        &graph,
        &|idx, _: &ComponentInstanceIdx| node_infos[&idx].clone(),
        &|idx, _: &()| {
            edge_infos.get(&idx).cloned().unwrap_or(EdgeInfo {
                label: String::new(),
                source_port: None,
                target_port: None,
            })
        },
        &layout_opts,
    );

    render_svg(&gl, &make_svg_opts(options))
}

/// Render an AADL system instance to interactive HTML.
pub fn render_instance_html(
    instance: &SystemInstance,
    options: &RenderOptions,
    html_options: &etch::html::HtmlOptions,
) -> String {
    let (graph, node_infos, edge_infos) = build_graph(instance, options);
    let layout_opts = make_layout_opts(options);

    let gl = etch::layout::layout(
        &graph,
        &|idx, _: &ComponentInstanceIdx| node_infos[&idx].clone(),
        &|idx, _: &()| {
            edge_infos.get(&idx).cloned().unwrap_or(EdgeInfo {
                label: String::new(),
                source_port: None,
                target_port: None,
            })
        },
        &layout_opts,
    );

    etch::html::render_html(&gl, &make_svg_opts(options), html_options)
}

fn make_layout_opts(options: &RenderOptions) -> LayoutOptions {
    LayoutOptions {
        node_width: options.node_width,
        node_height: options.node_height,
        rank_separation: options.rank_separation,
        node_separation: options.node_separation,
        container_padding: options.container_padding,
        container_header: options.container_header,
        ..Default::default()
    }
}

fn make_svg_opts(options: &RenderOptions) -> SvgOptions {
    SvgOptions {
        type_colors: category_colors(),
        interactive: options.interactive,
        base_url: options.base_url.clone(),
        highlight: options.highlight.clone(),
        ..Default::default()
    }
}

/// Options for AADL architecture rendering.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub node_width: f64,
    pub node_height: f64,
    pub rank_separation: f64,
    pub node_separation: f64,
    pub container_padding: f64,
    pub container_header: f64,
    pub interactive: bool,
    pub base_url: Option<String>,
    pub highlight: Option<String>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            node_width: 220.0,
            node_height: 60.0,
            rank_separation: 70.0,
            node_separation: 40.0,
            container_padding: 30.0,
            container_header: 40.0,
            interactive: false,
            base_url: None,
            highlight: None,
        }
    }
}

/// Build a petgraph from the AADL instance model with ports.
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

        let parent = if ci_idx == instance.root {
            None
        } else {
            comp.parent.map(|p| {
                let parent_comp = instance.component(p);
                node_id(parent_comp, p)
            })
        };

        // Build ports from AADL features
        let ports: Vec<PortInfo> = comp
            .features
            .iter()
            .map(|&fi| {
                let f = &instance.features[fi];
                feature_to_port(f)
            })
            .collect();

        let info = NodeInfo {
            id: node_id(comp, ci_idx),
            label,
            node_type: category_type_name(comp.category).to_string(),
            sublabel,
            parent,
            ports,
        };

        node_infos.insert(node_idx, info);
    }

    // Add connection edges with port references.
    for (_conn_idx, conn) in instance.connections.iter() {
        let src_ci = resolve_connection_end(instance, conn.owner, &conn.src);
        let dst_ci = resolve_connection_end(instance, conn.owner, &conn.dst);

        let (Some(src), Some(dst)) = (src_ci, dst_ci) else {
            continue;
        };
        let (Some(&src_node), Some(&dst_node)) = (idx_map.get(&src), idx_map.get(&dst)) else {
            continue;
        };
        if src_node != dst_node {
            // Resolve port IDs from connection ends
            let source_port = conn.src.as_ref().map(|e| e.feature.to_string());
            let target_port = conn.dst.as_ref().map(|e| e.feature.to_string());

            let edge_idx = graph.add_edge(src_node, dst_node, ());
            edge_infos.insert(
                edge_idx,
                EdgeInfo {
                    label: conn.name.to_string(),
                    source_port,
                    target_port,
                },
            );
        }
    }

    (graph, node_infos, edge_infos)
}

/// Convert an AADL FeatureInstance to an etch PortInfo.
fn feature_to_port(feature: &spar_hir_def::instance::FeatureInstance) -> PortInfo {
    let port_type = match feature.kind {
        FeatureKind::DataPort | FeatureKind::Parameter => PortType::Data,
        FeatureKind::EventPort => PortType::Event,
        FeatureKind::EventDataPort => PortType::EventData,
        FeatureKind::DataAccess
        | FeatureKind::BusAccess
        | FeatureKind::SubprogramAccess
        | FeatureKind::SubprogramGroupAccess => PortType::Access,
        FeatureKind::FeatureGroup => PortType::Group,
        FeatureKind::AbstractFeature => PortType::Abstract,
    };

    let (direction, side) = match feature.direction {
        Some(Direction::In) => (PortDirection::In, PortSide::Left),
        Some(Direction::Out) => (PortDirection::Out, PortSide::Right),
        Some(Direction::InOut) => (PortDirection::InOut, PortSide::Left),
        None => (PortDirection::In, PortSide::Auto),
    };

    PortInfo {
        id: feature.name.to_string(),
        label: feature.name.to_string(),
        side,
        direction,
        port_type,
    }
}

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

fn node_id(comp: &spar_hir_def::instance::ComponentInstance, _idx: ComponentInstanceIdx) -> String {
    if let Some(arr_idx) = comp.array_index {
        format!("AADL-{}-{}_{}", comp.package, comp.name, arr_idx)
    } else {
        format!("AADL-{}-{}", comp.package, comp.name)
    }
}

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
        assert_eq!(opts.node_width, 220.0);
        assert!(!opts.interactive);
    }

    #[test]
    fn feature_kind_to_port_type_mapping() {
        use spar_hir_def::Name;
        use spar_hir_def::instance::FeatureInstance;

        let f = FeatureInstance {
            name: Name::new("data_in"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            owner: ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(0)),
            classifier: None,
            access_kind: None,
            array_index: None,
        };
        let p = feature_to_port(&f);
        assert_eq!(p.port_type, PortType::Data);
        assert_eq!(p.direction, PortDirection::In);
        assert_eq!(p.side, PortSide::Left);

        let f2 = FeatureInstance {
            name: Name::new("event_out"),
            kind: FeatureKind::EventPort,
            direction: Some(Direction::Out),
            owner: ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(0)),
            classifier: None,
            access_kind: None,
            array_index: None,
        };
        let p2 = feature_to_port(&f2);
        assert_eq!(p2.port_type, PortType::Event);
        assert_eq!(p2.direction, PortDirection::Out);
        assert_eq!(p2.side, PortSide::Right);
    }
}
