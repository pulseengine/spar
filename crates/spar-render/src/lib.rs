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

    let svg = render_svg(&gl, &make_svg_opts(options));
    postprocess_svg(&svg)
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

    let html = etch::html::render_html(&gl, &make_svg_opts(options), html_options);
    postprocess_svg(&html)
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
// SVG post-processing: AADL-standard shapes and visual improvements
// ---------------------------------------------------------------------------

/// AADL component categories that use dashed borders.
const DASHED_CATEGORIES: &[&str] = &[
    "thread-group",
    "virtual-processor",
    "virtual-bus",
    "subprogram-group",
];

/// Post-process SVG output from etch to apply AADL-standard component shapes,
/// a drop shadow filter, and improved CSS styling.
///
/// This replaces generic `<rect>` elements inside node groups with category-
/// specific `<path>` or shape elements per AS5506 Appendix A, and patches
/// the embedded `<style>` and `<defs>` for a refined visual appearance.
fn postprocess_svg(svg: &str) -> String {
    let mut result = svg.to_string();

    // 1. Inject drop shadow filter into <defs>.
    result = inject_drop_shadow(result);

    // 2. Patch CSS style block for improved typography and softer strokes.
    result = patch_css_style(result);

    // 3. Replace <rect> inside node groups with AADL-standard shapes.
    result = replace_node_shapes(result);

    result
}

/// Inject a subtle drop shadow SVG filter into the `<defs>` block.
fn inject_drop_shadow(svg: String) -> String {
    let shadow_filter = "\
    <filter id=\"shadow\" x=\"-4%\" y=\"-4%\" width=\"108%\" height=\"112%\">\n\
      <feDropShadow dx=\"1\" dy=\"2\" stdDeviation=\"2\" flood-color=\"#00000018\" />\n\
    </filter>";

    // Insert before </defs>
    if let Some(pos) = svg.find("</defs>") {
        let mut result = String::with_capacity(svg.len() + shadow_filter.len() + 10);
        result.push_str(&svg[..pos]);
        result.push_str("    ");
        result.push_str(shadow_filter);
        result.push('\n');
        result.push_str("  ");
        result.push_str(&svg[pos..]);
        result
    } else {
        svg
    }
}

/// Patch the CSS `<style>` block for softer strokes and improved typography.
fn patch_css_style(svg: String) -> String {
    // Replace the node rect stroke color from #333 to #555
    let svg = svg.replace(
        ".node rect { stroke: #333;",
        ".node rect, .node path, .node ellipse { stroke: #555;",
    );
    // Add drop shadow filter reference to nodes via additional CSS-like approach:
    // Since SVG CSS `filter:` works, we add it to .node styling.
    // But we need to do this via the attribute on each node group instead.
    // We'll add filter="url(#shadow)" to each node <g>.
    let svg = svg.replace(
        ".node:hover rect { filter: brightness(0.92); }",
        ".node:hover rect, .node:hover path, .node:hover ellipse { filter: brightness(0.92); }",
    );
    // Update container CSS to apply to path elements too
    let svg = svg.replace(
        ".node.container rect { stroke-dasharray: 4 2; }",
        ".node.container rect, .node.container path { stroke-dasharray: 4 2; }",
    );
    // Also fix .node.selected rect in HTML mode
    let svg = svg.replace(
        ".node.selected rect {",
        ".node.selected rect, .node.selected path, .node.selected ellipse {",
    );
    svg
}

/// Replace `<rect>` elements inside `<g class="node type-XYZ ...">` groups
/// with AADL-standard shapes based on the component category.
fn replace_node_shapes(svg: String) -> String {
    let mut result = String::with_capacity(svg.len() + 1024);
    let mut remaining = svg.as_str();

    while let Some(g_start) = remaining.find("<g class=\"node type-") {
        // Copy everything before this node group.
        result.push_str(&remaining[..g_start]);

        let after_g = &remaining[g_start..];

        // Extract the node type from the class attribute.
        let category = extract_node_category(after_g);

        // Check if this is a container node.
        let is_container = after_g
            .get(..200)
            .unwrap_or(after_g)
            .contains(" container");

        // Find the <rect .../>  inside this <g>.
        if let Some(rect_start_rel) = after_g.find("<rect ") {
            // Only process if the rect is within this node group (before next </g>)
            let g_end = after_g.find("</g>").unwrap_or(after_g.len());
            if rect_start_rel < g_end {
                // Copy up to the <rect
                let g_tag_and_prefix = &after_g[..rect_start_rel];

                // Add filter="url(#shadow)" to the <g> opening tag
                let g_tag_with_shadow = add_shadow_to_g_tag(g_tag_and_prefix);
                result.push_str(&g_tag_with_shadow);

                let rect_str = &after_g[rect_start_rel..];

                // Find the end of the <rect .../> element
                if let Some(rect_end) = rect_str.find("/>") {
                    let rect_full = &rect_str[..rect_end + 2];

                    // Parse rect attributes.
                    if let Some(dims) = parse_rect_attrs(rect_full) {
                        // Generate the replacement shape.
                        let shape = generate_aadl_shape(
                            &category,
                            is_container,
                            dims.x,
                            dims.y,
                            dims.width,
                            dims.height,
                            &dims.fill,
                            &dims.stroke,
                            &dims.stroke_width,
                        );
                        result.push_str(&shape);
                    } else {
                        // Could not parse; keep original rect.
                        result.push_str(rect_full);
                    }

                    // Continue after the rect element.
                    remaining = &after_g[rect_start_rel + rect_end + 2..];
                } else {
                    // Malformed rect; keep as-is.
                    result.push_str(after_g);
                    remaining = "";
                }
            } else {
                // Rect not inside this group; copy the whole group start.
                result.push_str(&after_g[..g_end + 4]);
                remaining = &after_g[g_end + 4..];
            }
        } else {
            // No rect found; copy rest as-is.
            result.push_str(after_g);
            remaining = "";
        }
    }

    // Copy any remaining content.
    result.push_str(remaining);
    result
}

/// Add `filter="url(#shadow)"` to the `<g class="node ...">` opening tag.
fn add_shadow_to_g_tag(g_prefix: &str) -> String {
    // The g_prefix contains `<g class="node type-XYZ...">` followed by whitespace/newline.
    // We want to inject filter="url(#shadow)" before the closing >.
    if let Some(close_pos) = g_prefix.find(">\n") {
        let mut result = String::with_capacity(g_prefix.len() + 30);
        result.push_str(&g_prefix[..close_pos]);
        result.push_str(" filter=\"url(#shadow)\"");
        result.push_str(&g_prefix[close_pos..]);
        result
    } else if let Some(close_pos) = g_prefix.rfind('>') {
        let mut result = String::with_capacity(g_prefix.len() + 30);
        result.push_str(&g_prefix[..close_pos]);
        result.push_str(" filter=\"url(#shadow)\"");
        result.push_str(&g_prefix[close_pos..]);
        result
    } else {
        g_prefix.to_string()
    }
}

/// Extract the AADL category type from a `<g class="node type-XYZ ...">` tag.
fn extract_node_category(g_tag: &str) -> String {
    // Pattern: class="node type-XYZ" or class="node type-XYZ container"
    let prefix = "type-";
    if let Some(start) = g_tag.find(prefix) {
        let after = &g_tag[start + prefix.len()..];
        // The category ends at the next space or quote.
        let end = after
            .find(|c: char| c == '"' || c == ' ')
            .unwrap_or(after.len());
        after[..end].to_string()
    } else {
        String::new()
    }
}

/// Parsed rectangle attributes.
struct RectDims {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fill: String,
    stroke: String,
    stroke_width: String,
}

/// Parse x, y, width, height, fill, stroke, stroke-width from a `<rect .../>` element.
fn parse_rect_attrs(rect: &str) -> Option<RectDims> {
    let x = parse_attr_f64(rect, "x")?;
    let y = parse_attr_f64(rect, "y")?;
    let width = parse_attr_f64(rect, "width")?;
    let height = parse_attr_f64(rect, "height")?;
    let fill = parse_attr_str(rect, "fill").unwrap_or_else(|| "#e8e8e8".to_string());
    let stroke = parse_attr_str(rect, "stroke").unwrap_or_else(|| "#555".to_string());
    let stroke_width =
        parse_attr_str(rect, "stroke-width").unwrap_or_else(|| "1.5".to_string());

    Some(RectDims {
        x,
        y,
        width,
        height,
        fill,
        stroke,
        stroke_width,
    })
}

/// Parse a numeric attribute value from an SVG element string.
fn parse_attr_f64(s: &str, attr: &str) -> Option<f64> {
    // Match `attr="value"` — need to be careful with attribute names that are
    // prefixes of other attributes (e.g. "x" vs "rx"). We look for the attribute
    // preceded by a space and followed by `="`.
    let pattern = format!(" {}=\"", attr);
    let start = s.find(&pattern)?;
    let val_start = start + pattern.len();
    let val_end = s[val_start..].find('"')?;
    s[val_start..val_start + val_end].parse().ok()
}

/// Parse a string attribute value from an SVG element string.
fn parse_attr_str(s: &str, attr: &str) -> Option<String> {
    let pattern = format!(" {}=\"", attr);
    let start = s.find(&pattern)?;
    let val_start = start + pattern.len();
    let val_end = s[val_start..].find('"')?;
    Some(s[val_start..val_start + val_end].to_string())
}

/// Generate the AADL-standard SVG shape for a given component category.
///
/// Returns the SVG element(s) that replace the original `<rect>`.
#[allow(clippy::too_many_arguments)]
fn generate_aadl_shape(
    category: &str,
    is_container: bool,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    fill: &str,
    stroke: &str,
    stroke_width: &str,
) -> String {
    let dash = if DASHED_CATEGORIES.contains(&category) {
        " stroke-dasharray=\"6 3\""
    } else {
        ""
    };

    // Containers keep rectangular shapes (with chamfer for system) for clean
    // nesting; only leaf nodes get the full distinctive shapes.
    if is_container {
        return generate_container_shape(category, x, y, w, h, fill, stroke, stroke_width, dash);
    }

    match category {
        // System: rectangle with chamfered (angled) top-left corner.
        "system" => {
            let c = 14.0_f64.min(w * 0.15).min(h * 0.3);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + c, y,           // top-left after chamfer
                x + w, y,           // top-right
                x + w, y + h,       // bottom-right
                x, y + h,           // bottom-left
                x, y + c,           // left side up to chamfer
                fill, stroke, stroke_width, dash,
            )
        }
        // Process: stadium/capsule shape (rectangle with fully rounded left/right sides).
        "process" => {
            let r = (h / 2.0).min(w / 4.0);
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"{}\" ry=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x, y, w, h, r, r, fill, stroke, stroke_width, dash,
            )
        }
        // Thread: parallelogram (slanted right).
        "thread" => {
            let skew = 10.0_f64.min(w * 0.08);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Thread Group: parallelogram with dashed border (dash already applied).
        "thread-group" => {
            let skew = 10.0_f64.min(w * 0.08);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Processor: parallelogram (same shape as thread, different color).
        "processor" => {
            let skew = 10.0_f64.min(w * 0.08);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Virtual Processor: parallelogram with dashed border.
        "virtual-processor" => {
            let skew = 10.0_f64.min(w * 0.08);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + skew, y,
                x + w, y,
                x + w - skew, y + h,
                x, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Memory: trapezoid (wider at top).
        "memory" => {
            let inset = 12.0_f64.min(w * 0.08);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x, y,
                x + w, y,
                x + w - inset, y + h,
                x + inset, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Bus: hexagonal double-arrow/bar shape.
        "bus" => {
            let arrow = 10.0_f64.min(w * 0.06);
            let inset_y = h * 0.25;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + arrow, y + inset_y,
                x + w - arrow, y + inset_y,
                x + w, y + h / 2.0,
                x + w - arrow, y + h - inset_y,
                x + arrow, y + h - inset_y,
                x, y + h / 2.0,
                fill, stroke, stroke_width, dash,
            )
        }
        // Virtual Bus: hexagonal shape with dashed border.
        "virtual-bus" => {
            let arrow = 10.0_f64.min(w * 0.06);
            let inset_y = h * 0.25;
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + arrow, y + inset_y,
                x + w - arrow, y + inset_y,
                x + w, y + h / 2.0,
                x + w - arrow, y + h - inset_y,
                x + arrow, y + h - inset_y,
                x, y + h / 2.0,
                fill, stroke, stroke_width, dash,
            )
        }
        // Device: slightly tilted rectangle.
        "device" => {
            let dx = 4.0_f64.min(w * 0.03);
            let dy = 3.0_f64.min(h * 0.06);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + dx, y + dy,
                x + w - dy, y,
                x + w - dx, y + h - dy,
                x + dy, y + h,
                fill, stroke, stroke_width, dash,
            )
        }
        // Data: rectangle with a horizontal header stripe.
        "data" => {
            let header_h = 14.0_f64.min(h * 0.3);
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"2\" ry=\"2\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />\
                 <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" \
                 stroke=\"{}\" stroke-width=\"1\" />",
                x, y, w, h, fill, stroke, stroke_width, dash,
                x, y + header_h, x + w, y + header_h, stroke,
            )
        }
        // Subprogram: ellipse.
        "subprogram" => {
            format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + w / 2.0,
                y + h / 2.0,
                w / 2.0,
                h / 2.0,
                fill, stroke, stroke_width, dash,
            )
        }
        // Subprogram Group: ellipse with dashed border.
        "subprogram-group" => {
            format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + w / 2.0,
                y + h / 2.0,
                w / 2.0,
                h / 2.0,
                fill, stroke, stroke_width, dash,
            )
        }
        // Abstract: plain rectangle with double border (inner stroke).
        "abstract" => {
            let inset = 3.0;
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"2\" ry=\"2\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />\
                 <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"1\" ry=\"1\" fill=\"none\" stroke=\"{}\" stroke-width=\"0.5\" />",
                x, y, w, h, fill, stroke, stroke_width, dash,
                x + inset, y + inset, w - inset * 2.0, h - inset * 2.0, stroke,
            )
        }
        // Fallback: keep as rectangle.
        _ => {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"4\" ry=\"4\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x, y, w, h, fill, stroke, stroke_width, dash,
            )
        }
    }
}

/// Generate shapes for container nodes. Containers use rectangular outlines
/// with a category-specific top-left treatment for clean child nesting.
fn generate_container_shape(
    category: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    fill: &str,
    stroke: &str,
    stroke_width: &str,
    dash: &str,
) -> String {
    match category {
        // System container: chamfered top-left corner.
        "system" => {
            let c = 16.0_f64.min(w * 0.05).min(h * 0.05);
            format!(
                "<path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} Z\" \
                 fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x + c, y,
                x + w, y,
                x + w, y + h,
                x, y + h,
                x, y + c,
                fill, stroke, stroke_width, dash,
            )
        }
        // Process container: rounded rect with generous radius.
        "process" => {
            let r = 8.0_f64.min(w * 0.05).min(h * 0.1);
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"{}\" ry=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x, y, w, h, r, r, fill, stroke, stroke_width, dash,
            )
        }
        // All other containers: plain rect with standard corner radius.
        _ => {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 rx=\"4\" ry=\"4\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"{} />",
                x, y, w, h, fill, stroke, stroke_width, dash,
            )
        }
    }
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
    // Post-processing tests
    // -----------------------------------------------------------------------

    #[test]
    fn extract_category_from_node_group() {
        assert_eq!(
            extract_node_category(r#"<g class="node type-system container">"#),
            "system"
        );
        assert_eq!(
            extract_node_category(r#"<g class="node type-thread">"#),
            "thread"
        );
        assert_eq!(
            extract_node_category(r#"<g class="node type-virtual-processor">"#),
            "virtual-processor"
        );
        assert_eq!(
            extract_node_category(r#"<g class="node type-subprogram-group container">"#),
            "subprogram-group"
        );
    }

    #[test]
    fn parse_rect_attributes() {
        let rect = r##"<rect x="10" y="20" width="200" height="60" rx="4" ry="4" fill="#dce8f5" stroke="#555" stroke-width="1.5" />"##;
        let dims = parse_rect_attrs(rect).unwrap();
        assert_eq!(dims.x, 10.0);
        assert_eq!(dims.y, 20.0);
        assert_eq!(dims.width, 200.0);
        assert_eq!(dims.height, 60.0);
        assert_eq!(dims.fill, "#dce8f5");
        assert_eq!(dims.stroke, "#555");
        assert_eq!(dims.stroke_width, "1.5");
    }

    #[test]
    fn parse_attr_f64_distinguishes_x_from_rx() {
        let rect = r#"<rect x="10" y="20" width="200" height="60" rx="4" ry="4" />"#;
        assert_eq!(parse_attr_f64(rect, "x"), Some(10.0));
        assert_eq!(parse_attr_f64(rect, "rx"), Some(4.0));
        assert_eq!(parse_attr_f64(rect, "width"), Some(200.0));
    }

    #[test]
    fn system_shape_is_chamfered_path() {
        let shape = generate_aadl_shape("system", false, 10.0, 20.0, 200.0, 60.0, "#dce8f5", "#555", "1.5");
        assert!(shape.starts_with("<path d=\"M "));
        assert!(shape.contains(" Z\""));
        assert!(shape.contains("fill=\"#dce8f5\""));
        assert!(!shape.contains("stroke-dasharray"));
    }

    #[test]
    fn process_shape_is_stadium() {
        let shape = generate_aadl_shape("process", false, 0.0, 0.0, 200.0, 60.0, "#d5edd8", "#555", "1.5");
        assert!(shape.starts_with("<rect "));
        // Stadium has rx = h/2 (capped at w/4).
        assert!(shape.contains("rx=\"30\""));
    }

    #[test]
    fn thread_shape_is_parallelogram() {
        let shape = generate_aadl_shape("thread", false, 0.0, 0.0, 200.0, 60.0, "#fef3d0", "#555", "1.5");
        assert!(shape.starts_with("<path d=\"M "));
        // The first point should be offset (parallelogram skew).
        assert!(shape.contains("M 10,0"));
    }

    #[test]
    fn thread_group_has_dashed_border() {
        let shape = generate_aadl_shape("thread-group", false, 0.0, 0.0, 200.0, 60.0, "#fef3d0", "#555", "1.5");
        assert!(shape.contains("stroke-dasharray=\"6 3\""));
    }

    #[test]
    fn bus_shape_is_hexagonal() {
        let shape = generate_aadl_shape("bus", false, 0.0, 0.0, 200.0, 60.0, "#f0ece4", "#555", "1.5");
        assert!(shape.starts_with("<path d=\"M "));
        // Hexagon has 6 L commands (6 points).
        assert_eq!(shape.matches(" L ").count(), 5); // M + 5 L + Z
    }

    #[test]
    fn memory_shape_is_trapezoid() {
        let shape = generate_aadl_shape("memory", false, 0.0, 0.0, 200.0, 60.0, "#e8dff0", "#555", "1.5");
        assert!(shape.starts_with("<path d=\"M "));
        // Bottom should be narrower: bottom-left x > 0.
        assert!(shape.contains("M 0,0")); // top-left at origin
    }

    #[test]
    fn subprogram_shape_is_ellipse() {
        let shape = generate_aadl_shape("subprogram", false, 0.0, 0.0, 200.0, 60.0, "#e8e8ef", "#555", "1.5");
        assert!(shape.starts_with("<ellipse "));
        assert!(shape.contains("cx=\"100\""));
        assert!(shape.contains("cy=\"30\""));
    }

    #[test]
    fn data_shape_has_header_stripe() {
        let shape = generate_aadl_shape("data", false, 0.0, 0.0, 200.0, 60.0, "#fff8e1", "#555", "1.5");
        assert!(shape.contains("<rect "));
        assert!(shape.contains("<line "));
    }

    #[test]
    fn abstract_shape_has_double_border() {
        let shape = generate_aadl_shape("abstract", false, 0.0, 0.0, 200.0, 60.0, "#f5f5f5", "#555", "1.5");
        // Should have two rect elements.
        assert_eq!(shape.matches("<rect ").count(), 2);
        assert!(shape.contains("fill=\"none\""));
    }

    #[test]
    fn device_shape_is_tilted() {
        let shape = generate_aadl_shape("device", false, 0.0, 0.0, 200.0, 60.0, "#ddf0ee", "#555", "1.5");
        assert!(shape.starts_with("<path d=\"M "));
    }

    #[test]
    fn container_system_is_chamfered() {
        let shape = generate_aadl_shape("system", true, 0.0, 0.0, 400.0, 300.0, "#eef3fa", "#555", "2.0");
        assert!(shape.starts_with("<path d=\"M "));
        assert!(shape.contains(" Z\""));
    }

    #[test]
    fn container_process_is_rounded_rect() {
        let shape = generate_aadl_shape("process", true, 0.0, 0.0, 400.0, 300.0, "#eaf6ec", "#555", "2.0");
        assert!(shape.starts_with("<rect "));
        assert!(shape.contains("rx=\"8\""));
    }

    #[test]
    fn virtual_categories_are_dashed() {
        for cat in &["virtual-processor", "virtual-bus", "thread-group", "subprogram-group"] {
            let shape = generate_aadl_shape(cat, false, 0.0, 0.0, 200.0, 60.0, "#eee", "#555", "1.5");
            assert!(
                shape.contains("stroke-dasharray"),
                "{cat} should have dashed border"
            );
        }
    }

    #[test]
    fn inject_drop_shadow_adds_filter() {
        let svg = "<defs>\n  <marker>...</marker>\n  </defs>";
        let result = inject_drop_shadow(svg.to_string());
        assert!(result.contains("<filter id=\"shadow\""));
        assert!(result.contains("feDropShadow"));
        assert!(result.contains("</defs>"));
    }

    #[test]
    fn patch_css_updates_stroke_selectors() {
        let css = ".node rect { stroke: #333; stroke-width: 1.5; }";
        let result = patch_css_style(css.to_string());
        assert!(result.contains(".node rect, .node path, .node ellipse { stroke: #555;"));
    }

    #[test]
    fn postprocess_replaces_system_rect_with_path() {
        let svg = r##"<svg><defs>
    <marker>...</marker>
  </defs>
  <style>
    .node rect { stroke: #333; stroke-width: 1.5; }
    .node.container rect { stroke-dasharray: 4 2; }
    .node:hover rect { filter: brightness(0.92); }
  </style>
  <g class="node type-system">
        <rect x="10" y="20" width="200" height="60" rx="4" ry="4" fill="#dce8f5" stroke="#555" stroke-width="1.5" />
        <text x="110" y="50">sys</text>
  </g>
</svg>"##;
        let result = postprocess_svg(svg);
        // The rect should be replaced with a path (chamfered system).
        assert!(result.contains("<path d=\"M "), "system should become a <path>");
        assert!(!result.contains(r#"<rect x="10" y="20" width="200" height="60" rx="4""#),
                "original rect should be replaced");
        // Drop shadow filter should be injected.
        assert!(result.contains(r#"<filter id="shadow""#));
        // CSS should be patched.
        assert!(result.contains(".node rect, .node path, .node ellipse"));
        // Shadow filter should be on the node group.
        assert!(result.contains(r#"filter="url(#shadow)""#));
    }

    #[test]
    fn postprocess_preserves_non_node_rects() {
        let svg = r##"<svg><defs></defs><rect width="100" height="100" fill="#fff" /></svg>"##;
        let result = postprocess_svg(svg);
        // The background rect should be preserved.
        assert!(result.contains(r##"width="100" height="100" fill="#fff""##));
    }

    #[test]
    fn postprocess_handles_multiple_nodes() {
        let svg = r##"<svg><defs></defs>
  <g class="node type-system">
        <rect x="0" y="0" width="200" height="60" rx="4" ry="4" fill="#dce8f5" stroke="#555" stroke-width="1.5" />
  </g>
  <g class="node type-thread">
        <rect x="0" y="100" width="200" height="60" rx="4" ry="4" fill="#fef3d0" stroke="#555" stroke-width="1.5" />
  </g>
</svg>"##;
        let result = postprocess_svg(svg);
        // Both should be converted: system -> chamfered path, thread -> parallelogram path.
        assert_eq!(
            result.matches("<path d=\"M ").count(),
            2,
            "both nodes should become paths"
        );
    }
}
