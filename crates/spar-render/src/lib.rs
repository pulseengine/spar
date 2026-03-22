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
        type_shapes: aadl_shapes(),
        interactive: options.interactive,
        base_url: options.base_url.clone(),
        highlight: options.highlight.clone(),
        font_family: "'Inter', 'SF Pro', system-ui, sans-serif".into(),
        edge_color: "#888".into(),
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
        ("system", "#dce8f5"),          // Soft blue
        ("process", "#d5edd8"),         // Sage green
        ("thread", "#fef3d0"),          // Warm cream
        ("thread-group", "#fef3d0"),    // Same as thread
        ("processor", "#fde2e2"),       // Soft rose
        ("virtual-processor", "#fde2e2"),
        ("memory", "#e8dff0"),          // Lavender
        ("bus", "#f0ece4"),             // Warm gray
        ("virtual-bus", "#f0ece4"),
        ("device", "#ddf0ee"),          // Teal tint
        ("data", "#fff8e1"),            // Pale gold
        ("subprogram", "#e8e8ef"),      // Cool gray
        ("subprogram-group", "#e8e8ef"),
        ("abstract", "#f5f5f5"),        // Near white
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

// ---------------------------------------------------------------------------
// AADL-standard shape providers for etch's type_shapes API
// ---------------------------------------------------------------------------

/// Build AADL-standard shape providers for all 14 component categories.
///
/// Each closure receives `(node_type, x, y, width, height, fill, stroke)` and
/// returns raw SVG element string per AS5506 Appendix A conventions.
fn aadl_shapes() -> HashMap<String, etch::svg::ShapeProvider> {
    let mut m = HashMap::new();

    // System: chamfered top-left corner
    m.insert(
        "system".into(),
        Box::new(
            |_type: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
                let ch = 12.0;
                format!(
                    "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                     fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                    x + ch, y,
                    x + w, y,
                    x + w, y + h,
                    x, y + h,
                    x, y + ch,
                    fill, stroke,
                )
            },
        ) as etch::svg::ShapeProvider,
    );

    // Process: stadium/capsule (rounded ends)
    m.insert(
        "process".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let r = h / 2.0;
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"{}\" ry=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x, y, w, h, r, r, fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Thread: parallelogram
    m.insert(
        "thread".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let skew = 10.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Thread Group: parallelogram + dashed
    m.insert(
        "thread-group".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let skew = 10.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" stroke-dasharray=\"6 3\" />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Processor: parallelogram (same shape, different color distinguishes)
    m.insert(
        "processor".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let skew = 10.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Virtual Processor: parallelogram + dashed
    m.insert(
        "virtual-processor".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let skew = 10.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" stroke-dasharray=\"6 3\" />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Memory: trapezoid (wider at top)
    m.insert(
        "memory".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let inset = 12.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x, y,
                x + w, y,
                x + w - inset, y + h,
                x + inset, y + h,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Bus: hexagon/double-arrow
    m.insert(
        "bus".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let arrow = 12.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x + arrow, y,
                x + w - arrow, y,
                x + w, y + h / 2.0,
                x + w - arrow, y + h,
                x + arrow, y + h,
                x, y + h / 2.0,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Virtual Bus: hexagon + dashed
    m.insert(
        "virtual-bus".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let arrow = 12.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" stroke-dasharray=\"6 3\" />",
                x + arrow, y,
                x + w - arrow, y,
                x + w, y + h / 2.0,
                x + w - arrow, y + h,
                x + arrow, y + h,
                x, y + h / 2.0,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Device: slightly tilted rectangle
    m.insert(
        "device".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            let tilt = 4.0;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x + tilt, y,
                x + w, y + tilt,
                x + w - tilt, y + h,
                x, y + h - tilt,
                fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Data: rectangle with header stripe
    m.insert(
        "data".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" ry=\"2\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />\
                 <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1\" />",
                x, y, w, h, fill, stroke,
                x, y + 16.0, x + w, y + 16.0, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Subprogram: ellipse
    m.insert(
        "subprogram".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
                x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Subprogram Group: ellipse + dashed
    m.insert(
        "subprogram-group".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" stroke-dasharray=\"6 3\" />",
                x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, fill, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    // Abstract: plain rectangle with double border
    m.insert(
        "abstract".into(),
        Box::new(|_: &str, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str| {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" ry=\"3\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />\
                 <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" ry=\"2\" \
                 fill=\"none\" stroke=\"{}\" stroke-width=\"0.5\" />",
                x, y, w, h, fill, stroke,
                x + 3.0, y + 3.0, w - 6.0, h - 6.0, stroke,
            )
        }) as etch::svg::ShapeProvider,
    );

    m
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

    // -----------------------------------------------------------------------
    // Shape provider tests
    // -----------------------------------------------------------------------

    /// Helper: invoke a shape provider by category name.
    fn call_shape(category: &str, x: f64, y: f64, w: f64, h: f64) -> String {
        let shapes = aadl_shapes();
        let provider = shapes.get(category).unwrap_or_else(|| panic!("no shape for {category}"));
        provider(category, x, y, w, h, "#eee", "#555")
    }

    #[test]
    fn shape_providers_cover_all_categories() {
        let shapes = aadl_shapes();
        let expected = [
            "system", "process", "thread", "thread-group", "processor",
            "virtual-processor", "memory", "bus", "virtual-bus", "device",
            "data", "subprogram", "subprogram-group", "abstract",
        ];
        for cat in expected {
            assert!(shapes.contains_key(cat), "missing shape for {cat}");
        }
        assert_eq!(shapes.len(), 14);
    }

    #[test]
    fn system_shape_is_chamfered_path() {
        let shape = call_shape("system", 10.0, 20.0, 200.0, 60.0);
        assert!(shape.starts_with("<path d=\"M "));
        assert!(shape.contains(" Z\""));
        assert!(shape.contains("fill=\"#eee\""));
        assert!(shape.contains("stroke-width=\"1.5\""));
        assert!(!shape.contains("stroke-dasharray"));
    }

    #[test]
    fn process_shape_is_stadium() {
        let shape = call_shape("process", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<rect "));
        assert!(shape.contains("rx=\"30\"")); // h/2 = 30
    }

    #[test]
    fn thread_shape_is_parallelogram() {
        let shape = call_shape("thread", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<path d=\"M "));
        assert!(shape.contains("M 10,0")); // skew = 10
    }

    #[test]
    fn thread_group_has_dashed_border() {
        let shape = call_shape("thread-group", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.contains("stroke-dasharray=\"6 3\""));
    }

    #[test]
    fn bus_shape_is_hexagonal() {
        let shape = call_shape("bus", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<path d=\"M "));
        // Hexagon: M + 5 L + Z = 6 points
        assert_eq!(shape.matches(" L ").count(), 5);
    }

    #[test]
    fn memory_shape_is_trapezoid() {
        let shape = call_shape("memory", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<path d=\"M "));
        assert!(shape.contains("M 0,0")); // top-left at origin
    }

    #[test]
    fn subprogram_shape_is_ellipse() {
        let shape = call_shape("subprogram", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<ellipse "));
        assert!(shape.contains("cx=\"100\""));
        assert!(shape.contains("cy=\"30\""));
    }

    #[test]
    fn data_shape_has_header_stripe() {
        let shape = call_shape("data", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.contains("<rect "));
        assert!(shape.contains("<line "));
    }

    #[test]
    fn abstract_shape_has_double_border() {
        let shape = call_shape("abstract", 0.0, 0.0, 200.0, 60.0);
        assert_eq!(shape.matches("<rect ").count(), 2);
        assert!(shape.contains("fill=\"none\""));
    }

    #[test]
    fn device_shape_is_tilted() {
        let shape = call_shape("device", 0.0, 0.0, 200.0, 60.0);
        assert!(shape.starts_with("<path d=\"M "));
    }

    #[test]
    fn virtual_categories_are_dashed() {
        for cat in &["virtual-processor", "virtual-bus", "thread-group", "subprogram-group"] {
            let shape = call_shape(cat, 0.0, 0.0, 200.0, 60.0);
            assert!(
                shape.contains("stroke-dasharray"),
                "{cat} should have dashed border"
            );
        }
    }

    #[test]
    fn solid_categories_not_dashed() {
        for cat in &["system", "process", "thread", "processor", "memory", "bus", "device", "data", "subprogram", "abstract"] {
            let shape = call_shape(cat, 0.0, 0.0, 200.0, 60.0);
            assert!(
                !shape.contains("stroke-dasharray"),
                "{cat} should NOT have dashed border"
            );
        }
    }

    #[test]
    fn make_svg_opts_includes_shapes() {
        let opts = make_svg_opts(&RenderOptions::default());
        assert_eq!(opts.type_shapes.len(), 14);
        assert!(opts.type_colors.contains_key("system"));
    }
}
