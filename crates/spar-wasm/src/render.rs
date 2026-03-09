//! Top-level render function: AADL source to SVG.
//!
//! Parses AADL source, instantiates a system implementation, builds a
//! petgraph via [`build_graph`], and renders the graph to SVG with a
//! simple topological grid layout.

use std::fmt;
use std::fmt::Write as FmtWrite;

use petgraph::graph::Graph;
use petgraph::visit::Topo;

use spar_hir::Database;
use spar_hir_def::item_tree::ComponentCategory;

use crate::graph::{ArchEdge, ArchNode, build_graph};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during AADL-to-SVG rendering.
#[derive(Debug)]
pub enum RenderError {
    /// The AADL source could not be parsed.
    ParseError(String),
    /// The requested root implementation was not found.
    NoRoot(String),
    /// The layout algorithm failed.
    LayoutError(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::ParseError(msg) => write!(f, "parse error: {msg}"),
            RenderError::NoRoot(msg) => write!(f, "root not found: {msg}"),
            RenderError::LayoutError(msg) => write!(f, "layout error: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render AADL from filesystem (for WASM component use).
///
/// Reads all `.aadl` files in the current directory, parses them,
/// instantiates from the given `root`, and renders to SVG.
pub fn render_aadl_from_fs(root: &str, highlight: &[String]) -> Result<String, RenderError> {
    let mut sources = Vec::new();
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "aadl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    sources.push((path.display().to_string(), content));
                }
            }
        }
    }
    if sources.is_empty() {
        return Err(RenderError::ParseError("no .aadl files found".into()));
    }

    let db = Database::from_aadl(
        &sources.iter().map(|(f, c)| (f.clone(), c.clone())).collect::<Vec<_>>(),
    );

    let instance = db
        .instantiate(root)
        .ok_or_else(|| RenderError::NoRoot(format!("cannot instantiate {}", root)))?;

    if instance.diagnostics().iter().any(|d| d.contains("Unresolved")) {
        return Err(RenderError::NoRoot(format!(
            "root {} has unresolved components",
            root
        )));
    }

    let (graph, _) = build_graph(instance.inner());
    render_graph_to_svg(&graph, highlight)
}

/// Run all analyses on the AADL model from filesystem.
///
/// Reads `.aadl` files from the current directory, instantiates the given
/// root, and runs all registered analysis passes.
pub fn analyze_aadl_from_fs(
    root: &str,
) -> Result<Vec<spar_analysis::AnalysisDiagnostic>, RenderError> {
    let mut sources = Vec::new();
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "aadl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    sources.push((path.display().to_string(), content));
                }
            }
        }
    }
    if sources.is_empty() {
        return Err(RenderError::ParseError("no .aadl files found".into()));
    }

    let db = Database::from_aadl(
        &sources.iter().map(|(f, c)| (f.clone(), c.clone())).collect::<Vec<_>>(),
    );

    let instance = db
        .instantiate(root)
        .ok_or_else(|| RenderError::NoRoot(format!("cannot instantiate {}", root)))?;

    if instance.diagnostics().iter().any(|d| d.contains("Unresolved")) {
        return Err(RenderError::NoRoot(format!(
            "root {} has unresolved components",
            root
        )));
    }

    let mut runner = spar_analysis::AnalysisRunner::new();
    runner.register(Box::new(spar_analysis::connectivity::ConnectivityAnalysis));
    runner.register(Box::new(spar_analysis::hierarchy::HierarchyAnalysis));
    runner.register(Box::new(spar_analysis::completeness::CompletenessAnalysis));
    runner.register(Box::new(spar_analysis::flow_check::FlowCheckAnalysis));
    runner.register(Box::new(spar_analysis::mode_check::ModeCheckAnalysis));
    runner.register(Box::new(spar_analysis::binding_check::BindingCheckAnalysis));
    runner.register(Box::new(spar_analysis::scheduling::SchedulingAnalysis));
    runner.register(Box::new(spar_analysis::latency::LatencyAnalysis));
    runner.register(Box::new(spar_analysis::resource_budget::ResourceBudgetAnalysis));
    runner.register(Box::new(spar_analysis::direction_rules::DirectionRuleAnalysis));
    runner.register(Box::new(spar_analysis::connection_rules::ConnectionRuleAnalysis));
    runner.register(Box::new(spar_analysis::classifier_match::ClassifierMatchAnalysis));
    runner.register(Box::new(spar_analysis::mode_rules::ModeRuleAnalysis));
    runner.register(Box::new(spar_analysis::subcomponent_rules::SubcomponentRuleAnalysis));
    runner.register(Box::new(spar_analysis::emv2_analysis::Emv2Analysis));

    Ok(runner.run_all(instance.inner()))
}

/// Parse AADL source, instantiate the given root, and render to SVG.
///
/// `source` is the raw AADL text.  `root` is a qualified name such as
/// `"Pkg::Type.Impl"`.  `highlight` is a list of node IDs (e.g.
/// `"AADL-Pkg-sub1"`) that should be visually emphasized.
pub fn render_aadl(
    source: &str,
    root: &str,
    highlight: &[String],
) -> Result<String, RenderError> {
    let db = Database::from_aadl(&[("input.aadl".into(), source.into())]);

    let instance = db
        .instantiate(root)
        .ok_or_else(|| RenderError::NoRoot(format!("cannot instantiate '{root}'")))?;

    // If the root could not be resolved, the instance will contain diagnostics
    // about unresolved implementations. Treat that as a NoRoot error.
    let diags = instance.diagnostics();
    if !diags.is_empty() && diags.iter().any(|d| d.contains("unresolved")) {
        return Err(RenderError::NoRoot(format!(
            "cannot resolve root '{root}': {}",
            diags[0]
        )));
    }

    let (graph, _index_map) = build_graph(instance.inner());

    render_graph_to_svg(&graph, highlight)
}

/// Render a petgraph of [`ArchNode`]/[`ArchEdge`] to an SVG string.
///
/// Uses a simple topological grid layout (one rank per topological
/// depth level, nodes centered within each rank).
pub fn render_graph_to_svg(
    graph: &Graph<ArchNode, ArchEdge>,
    highlight: &[String],
) -> Result<String, RenderError> {
    // ── Layout constants ───────────────────────────────────────────
    let node_w: f64 = 180.0;
    let node_h: f64 = 50.0;
    let rank_sep: f64 = 80.0;
    let node_sep: f64 = 40.0;

    // ── Assign ranks via topological order ─────────────────────────
    // Compute the depth (longest path from any root) for each node.
    let node_count = graph.node_count();
    if node_count == 0 {
        return Ok(minimal_svg());
    }

    let mut depth: Vec<usize> = vec![0; node_count];
    let mut topo = Topo::new(graph);
    let mut topo_order = Vec::with_capacity(node_count);
    while let Some(nx) = topo.next(graph) {
        topo_order.push(nx);
        for neighbor in graph.neighbors(nx) {
            let d = depth[nx.index()] + 1;
            if d > depth[neighbor.index()] {
                depth[neighbor.index()] = d;
            }
        }
    }

    // If topo didn't visit all nodes (cycle), fall back to index order.
    if topo_order.len() < node_count {
        topo_order.clear();
        for i in 0..node_count {
            topo_order.push(petgraph::graph::NodeIndex::new(i));
        }
        for (i, nx) in topo_order.iter().enumerate() {
            depth[nx.index()] = i;
        }
    }

    // ── Group nodes by rank ────────────────────────────────────────
    let max_rank = depth.iter().copied().max().unwrap_or(0);
    let mut ranks: Vec<Vec<petgraph::graph::NodeIndex>> = vec![Vec::new(); max_rank + 1];
    for &nx in &topo_order {
        ranks[depth[nx.index()]].push(nx);
    }

    // ── Compute positions ──────────────────────────────────────────
    #[derive(Clone)]
    struct PositionedNode {
        x: f64,
        y: f64,
    }
    let mut positions: Vec<PositionedNode> = vec![PositionedNode { x: 0.0, y: 0.0 }; node_count];

    let max_rank_width = ranks
        .iter()
        .map(|r| {
            if r.is_empty() {
                0.0
            } else {
                r.len() as f64 * node_w + (r.len() as f64 - 1.0) * node_sep
            }
        })
        .fold(0.0_f64, f64::max);

    for (rank_idx, rank_nodes) in ranks.iter().enumerate() {
        let rank_width = if rank_nodes.is_empty() {
            0.0
        } else {
            rank_nodes.len() as f64 * node_w + (rank_nodes.len() as f64 - 1.0) * node_sep
        };
        let x_offset = (max_rank_width - rank_width) / 2.0;
        let y = rank_idx as f64 * (node_h + rank_sep);

        for (col, &nx) in rank_nodes.iter().enumerate() {
            positions[nx.index()] = PositionedNode {
                x: x_offset + col as f64 * (node_w + node_sep),
                y,
            };
        }
    }

    // ── Compute SVG dimensions ─────────────────────────────────────
    let padding = 20.0;
    let svg_w = max_rank_width + padding * 2.0;
    let svg_h = (max_rank + 1) as f64 * (node_h + rank_sep) - rank_sep + padding * 2.0;

    // ── Build SVG ──────────────────────────────────────────────────
    let mut svg = String::with_capacity(4096);

    // Root element
    writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {svg_w} {svg_h}\" width=\"{svg_w}\" height=\"{svg_h}\">"
    )
    .unwrap();

    // Defs: arrowhead marker
    write!(
        svg,
        "  <defs>\n\
         \x20   <marker id=\"arrowhead\" markerWidth=\"8\" markerHeight=\"8\" \
         refX=\"8\" refY=\"4\" orient=\"auto\" markerUnits=\"strokeWidth\">\n\
         \x20     <path d=\"M 0 0 L 8 4 L 0 8\" fill=\"#666\" />\n\
         \x20   </marker>\n\
         \x20 </defs>\n"
    )
    .unwrap();

    // Style
    write!(
        svg,
        "  <style>\n\
         \x20   .node rect, .node polygon, .node path, .node ellipse {{ stroke: #333; stroke-width: 1.5; }}\n\
         \x20   .node text {{ font-family: system-ui, -apple-system, sans-serif; font-size: 13px; \
         fill: #222; text-anchor: middle; dominant-baseline: central; }}\n\
         \x20   .node .sublabel {{ font-size: 11px; fill: #666; }}\n\
         \x20   .edge path {{ fill: none; stroke: #666; stroke-width: 1.4; \
         marker-end: url(#arrowhead); }}\n\
         \x20 </style>\n"
    )
    .unwrap();

    // Translation group
    writeln!(svg, "  <g transform=\"translate({padding},{padding})\">").unwrap();

    // ── Edges ──────────────────────────────────────────────────────
    svg.push_str("    <g class=\"edges\">\n");
    for edge_ref in graph.edge_indices() {
        let (src_nx, dst_nx) = graph.edge_endpoints(edge_ref).unwrap();
        let src_node = &graph[src_nx];
        let dst_node = &graph[dst_nx];
        let src_pos = &positions[src_nx.index()];
        let dst_pos = &positions[dst_nx.index()];

        // Path from bottom-center of source to top-center of destination
        let x1 = src_pos.x + node_w / 2.0;
        let y1 = src_pos.y + node_h;
        let x2 = dst_pos.x + node_w / 2.0;
        let y2 = dst_pos.y;

        let cy1 = y1 + (y2 - y1) * 0.5;
        let cy2 = y2 - (y2 - y1) * 0.5;
        let path_d = format!("M {x1} {y1} C {x1} {cy1}, {x2} {cy2}, {x2} {y2}");

        writeln!(
            svg,
            "      <g class=\"edge\" data-source=\"{}\" data-target=\"{}\">",
            xml_escape(&src_node.id),
            xml_escape(&dst_node.id),
        )
        .unwrap();
        writeln!(svg, "        <path d=\"{path_d}\" />").unwrap();
        svg.push_str("      </g>\n");
    }
    svg.push_str("    </g>\n");

    // ── Nodes ──────────────────────────────────────────────────────
    svg.push_str("    <g class=\"nodes\">\n");
    for nx in graph.node_indices() {
        let node = &graph[nx];
        let pos = &positions[nx.index()];
        let fill = category_color(node.category);
        let css_cat = category_css_class(node.category);
        let is_highlighted = highlight.iter().any(|h| h == &node.id);
        let (stroke_c, stroke_w) = if is_highlighted {
            ("#ff6600", "3")
        } else {
            ("#333", "1.5")
        };

        writeln!(
            svg,
            "      <g class=\"node type-{css_cat}\" data-id=\"{}\">",
            xml_escape(&node.id),
        )
        .unwrap();

        render_node_shape(
            &mut svg, pos.x, pos.y, node_w, node_h,
            node.category, fill, stroke_c, stroke_w,
        );

        // Adjust text center for 3D shapes (processor, device)
        let (text_dx, text_dy) = match node.category {
            ComponentCategory::Processor | ComponentCategory::VirtualProcessor => (-5.0, 5.0),
            ComponentCategory::Device => (-4.0, 4.0),
            _ => (0.0, 0.0),
        };
        let text_cx = pos.x + node_w / 2.0 + text_dx;
        let text_cy = pos.y + node_h / 2.0 + text_dy;

        // Primary label
        let text_y = if node.sublabel.is_some() {
            text_cy - 13.0 * 0.45
        } else {
            text_cy
        };
        writeln!(
            svg,
            "        <text x=\"{text_cx}\" y=\"{text_y}\">{}</text>",
            xml_escape(&node.label),
        )
        .unwrap();

        // Sublabel
        if let Some(ref sub) = node.sublabel {
            let sub_y = text_cy + 13.0 * 0.65;
            writeln!(
                svg,
                "        <text class=\"sublabel\" x=\"{text_cx}\" y=\"{sub_y}\">{}</text>",
                xml_escape(sub),
            )
            .unwrap();
        }

        // Tooltip
        writeln!(svg, "        <title>{}</title>", xml_escape(&node.id)).unwrap();

        svg.push_str("      </g>\n");
    }
    svg.push_str("    </g>\n");

    svg.push_str("  </g>\n");
    svg.push_str("</svg>\n");

    Ok(svg)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Produce a minimal empty SVG when the graph has no nodes.
fn minimal_svg() -> String {
    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 0 0\"></svg>\n".into()
}

/// Map AADL component category to a fill colour matching etch conventions.
fn category_color(cat: ComponentCategory) -> &'static str {
    match cat {
        ComponentCategory::System => "#b3d9ff",
        ComponentCategory::Process => "#d4edda",
        ComponentCategory::Thread | ComponentCategory::ThreadGroup => "#fff3cd",
        ComponentCategory::Processor | ComponentCategory::VirtualProcessor => "#f8d7da",
        ComponentCategory::Device => "#e2d5f1",
        ComponentCategory::Data => "#fce4ec",
        ComponentCategory::Memory => "#e8e8e8",
        ComponentCategory::Bus | ComponentCategory::VirtualBus => "#e8e8e8",
        ComponentCategory::Subprogram | ComponentCategory::SubprogramGroup => "#e8e8e8",
        ComponentCategory::Abstract => "#e8e8e8",
    }
}

/// Map AADL component category to a CSS-class-safe identifier.
fn category_css_class(cat: ComponentCategory) -> &'static str {
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

/// Whether a component category uses a dashed outline (virtual/group/abstract variants).
fn category_is_dashed(cat: ComponentCategory) -> bool {
    matches!(
        cat,
        ComponentCategory::ThreadGroup
            | ComponentCategory::VirtualProcessor
            | ComponentCategory::VirtualBus
            | ComponentCategory::SubprogramGroup
            | ComponentCategory::Abstract
    )
}

/// Render the AADL-standard graphical shape for a component category.
///
/// Writes SVG elements (path, polygon, rect, ellipse) into `svg`, positioned
/// at `(x, y)` within a bounding box of `(w, h)`.  Each AADL component
/// category has a distinctive shape per SAE AS5506:
///
/// - System: rounded rectangle
/// - Process/Thread: parallelogram
/// - ThreadGroup: rounded rectangle (dashed)
/// - Processor: 3D isometric box
/// - Memory: cylinder
/// - Bus: double-headed arrow
/// - Device: 3D raised rectangle
/// - Data: rectangle with separator line
/// - Subprogram: ellipse
/// - Abstract: rectangle (dashed)
fn render_node_shape(
    svg: &mut String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    cat: ComponentCategory,
    fill: &str,
    stroke: &str,
    stroke_w: &str,
) {
    let dash = if category_is_dashed(cat) {
        " stroke-dasharray=\"6 3\""
    } else {
        ""
    };
    let base = format!(
        "fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_w}\"{dash}"
    );

    match cat {
        // System: rounded rectangle (large radius)
        ComponentCategory::System => {
            writeln!(
                svg,
                "        <rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                 rx=\"12\" ry=\"12\" {base} />"
            )
            .unwrap();
        }

        // Process / Thread: parallelogram (thread is solid, same shape)
        ComponentCategory::Process | ComponentCategory::Thread => {
            let s = 15.0;
            writeln!(
                svg,
                "        <polygon points=\"{},{} {},{} {},{} {},{}\" {base} />",
                x + s, y,
                x + w, y,
                x + w - s, y + h,
                x, y + h,
            )
            .unwrap();
        }

        // Thread Group: rounded rectangle, dashed
        ComponentCategory::ThreadGroup => {
            writeln!(
                svg,
                "        <rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                 rx=\"12\" ry=\"12\" {base} />"
            )
            .unwrap();
        }

        // Processor / Virtual Processor: 3D isometric box (hexagon outline)
        ComponentCategory::Processor | ComponentCategory::VirtualProcessor => {
            let d = 10.0;
            // Outer hexagon
            writeln!(
                svg,
                "        <path d=\"M {},{} L {},{} L {},{} L {},{} L {},{} L {},{} Z\" {base} />",
                x, y + d,
                x + d, y,
                x + w, y,
                x + w, y + h - d,
                x + w - d, y + h,
                x, y + h,
            )
            .unwrap();
            // Internal 3D edges (lighter)
            writeln!(
                svg,
                "        <path d=\"M {},{} L {},{} L {},{} M {},{} L {},{}\" \
                 fill=\"none\" stroke=\"{stroke}\" stroke-width=\"0.8\" opacity=\"0.5\"{dash} />",
                x, y + d,
                x + w - d, y + d,
                x + w, y,
                x + w - d, y + d,
                x + w - d, y + h,
            )
            .unwrap();
        }

        // Memory: cylinder (body + top ellipse cap)
        ComponentCategory::Memory => {
            let ry = 8.0;
            // Body: left side + bottom arc + right side + close
            writeln!(
                svg,
                "        <path d=\"M {},{} L {},{} A {} {} 0 0 0 {},{} L {},{} Z\" {base} />",
                x, y + ry,
                x, y + h - ry,
                w / 2.0, ry, x + w, y + h - ry,
                x + w, y + ry,
            )
            .unwrap();
            // Top ellipse cap
            writeln!(
                svg,
                "        <ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{ry}\" {base} />",
                x + w / 2.0,
                y + ry,
                w / 2.0,
            )
            .unwrap();
        }

        // Bus / Virtual Bus: double-headed arrow
        ComponentCategory::Bus | ComponentCategory::VirtualBus => {
            let aw = 15.0; // arrowhead depth
            let m = h * 0.25; // body inset
            writeln!(
                svg,
                "        <polygon points=\"{},{} {},{} {},{} {},{} {},{} {},{} {},{} {},{} {},{} {},{}\" {base} />",
                x, y + h / 2.0,
                x + aw, y,
                x + aw, y + m,
                x + w - aw, y + m,
                x + w - aw, y,
                x + w, y + h / 2.0,
                x + w - aw, y + h,
                x + w - aw, y + h - m,
                x + aw, y + h - m,
                x + aw, y + h,
            )
            .unwrap();
        }

        // Device: 3D raised rectangle (front face + top face + right face)
        ComponentCategory::Device => {
            let d = 8.0;
            // Front face
            writeln!(
                svg,
                "        <rect x=\"{x}\" y=\"{}\" width=\"{}\" height=\"{}\" {base} />",
                y + d, w - d, h - d,
            )
            .unwrap();
            // Top face (slightly transparent fill)
            writeln!(
                svg,
                "        <polygon points=\"{},{} {},{} {},{} {},{}\" \
                 fill=\"{fill}\" fill-opacity=\"0.85\" stroke=\"{stroke}\" stroke-width=\"{stroke_w}\"{dash} />",
                x, y + d,
                x + d, y,
                x + w, y,
                x + w - d, y + d,
            )
            .unwrap();
            // Right face (more transparent)
            writeln!(
                svg,
                "        <polygon points=\"{},{} {},{} {},{} {},{}\" \
                 fill=\"{fill}\" fill-opacity=\"0.7\" stroke=\"{stroke}\" stroke-width=\"{stroke_w}\"{dash} />",
                x + w - d, y + d,
                x + w, y,
                x + w, y + h - d,
                x + w - d, y + h,
            )
            .unwrap();
        }

        // Data: plain rectangle with separator line near top
        ComponentCategory::Data => {
            writeln!(
                svg,
                "        <rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" {base} />"
            )
            .unwrap();
            let sep_y = y + 14.0;
            writeln!(
                svg,
                "        <line x1=\"{x}\" y1=\"{sep_y}\" x2=\"{}\" y2=\"{sep_y}\" \
                 stroke=\"{stroke}\" stroke-width=\"0.8\" opacity=\"0.4\" />",
                x + w,
            )
            .unwrap();
        }

        // Subprogram / Subprogram Group: ellipse
        ComponentCategory::Subprogram | ComponentCategory::SubprogramGroup => {
            writeln!(
                svg,
                "        <ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" {base} />",
                x + w / 2.0,
                y + h / 2.0,
                w / 2.0,
                h / 2.0,
            )
            .unwrap();
        }

        // Abstract: plain rectangle, dashed (dash applied via base attrs)
        ComponentCategory::Abstract => {
            writeln!(
                svg,
                "        <rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                 rx=\"2\" ry=\"2\" {base} />"
            )
            .unwrap();
        }
    }
}

/// Minimal XML escaping for attribute values and text content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_basic_aadl() {
        let source = "package Pkg\npublic\n  system S\n  end S;\n  system implementation S.I\n    subcomponents\n      sub1: process P;\n  end S.I;\n  process P\n  end P;\nend Pkg;";
        let svg = render_aadl(source, "Pkg::S.I", &[]).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("data-id"));
    }

    #[test]
    fn render_with_highlight() {
        let source = "package Pkg\npublic\n  system S\n  end S;\n  system implementation S.I\n    subcomponents\n      sub1: process P;\n  end S.I;\n  process P\n  end P;\nend Pkg;";
        let svg = render_aadl(source, "Pkg::S.I", &["AADL-Pkg-sub1".into()]).unwrap();
        assert!(svg.contains("#ff6600")); // highlight color
    }

    #[test]
    fn render_invalid_root() {
        let source = "package Pkg\npublic\n  system S\n  end S;\nend Pkg;";
        let result = render_aadl(source, "Pkg::Nonexistent.Impl", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn aadl_standard_shapes() {
        // System: rounded rectangle
        let mut svg = String::new();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::System, "#b3d9ff", "#333", "1.5");
        assert!(svg.contains("<rect") && svg.contains("rx=\"12\""), "system should be rounded rect");

        // Process: parallelogram (solid)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Process, "#d4edda", "#333", "1.5");
        assert!(svg.contains("<polygon"), "process should be polygon");
        assert!(!svg.contains("stroke-dasharray"), "process should be solid");

        // Thread: parallelogram (solid)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Thread, "#fff3cd", "#333", "1.5");
        assert!(svg.contains("<polygon"), "thread should be polygon");
        assert!(!svg.contains("stroke-dasharray"), "thread should be solid");

        // ThreadGroup: rounded rect, dashed
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::ThreadGroup, "#fff3cd", "#333", "1.5");
        assert!(svg.contains("<rect") && svg.contains("rx=\"12\""), "thread group should be rounded rect");
        assert!(svg.contains("stroke-dasharray"), "thread group should be dashed");

        // Processor: 3D isometric box
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Processor, "#f8d7da", "#333", "1.5");
        assert!(svg.contains("<path"), "processor should use path");
        assert!(svg.contains("opacity=\"0.5\""), "processor should have 3D internal lines");

        // VirtualProcessor: 3D box, dashed
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::VirtualProcessor, "#f8d7da", "#333", "1.5");
        assert!(svg.contains("stroke-dasharray"), "virtual processor should be dashed");

        // Memory: cylinder (path + ellipse)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Memory, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("<path") && svg.contains("<ellipse"), "memory should be cylinder");

        // Bus: double-headed arrow
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Bus, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("<polygon"), "bus should be polygon");
        assert!(!svg.contains("stroke-dasharray"), "bus should be solid");

        // VirtualBus: dashed
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::VirtualBus, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("stroke-dasharray"), "virtual bus should be dashed");

        // Device: 3D raised rectangle (rect + polygons)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Device, "#e2d5f1", "#333", "1.5");
        assert!(svg.contains("<rect") && svg.contains("<polygon"), "device should have rect + polygons");

        // Data: rectangle with separator line
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Data, "#fce4ec", "#333", "1.5");
        assert!(svg.contains("<rect") && svg.contains("<line"), "data should be rect with separator");

        // Subprogram: ellipse (solid)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Subprogram, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("<ellipse"), "subprogram should be ellipse");
        assert!(!svg.contains("stroke-dasharray"), "subprogram should be solid");

        // SubprogramGroup: ellipse (dashed)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::SubprogramGroup, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("<ellipse"), "subprogram group should be ellipse");
        assert!(svg.contains("stroke-dasharray"), "subprogram group should be dashed");

        // Abstract: rectangle (dashed)
        svg.clear();
        render_node_shape(&mut svg, 0.0, 0.0, 180.0, 50.0, ComponentCategory::Abstract, "#e8e8e8", "#333", "1.5");
        assert!(svg.contains("<rect"), "abstract should be rect");
        assert!(svg.contains("stroke-dasharray"), "abstract should be dashed");
    }
}
