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
         \x20   .node rect {{ stroke: #333; stroke-width: 1.5; }}\n\
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

        writeln!(
            svg,
            "        <rect x=\"{}\" y=\"{}\" width=\"{node_w}\" height=\"{node_h}\" \
             rx=\"4\" ry=\"4\" fill=\"{fill}\" stroke=\"{stroke_c}\" stroke-width=\"{stroke_w}\" />",
            pos.x, pos.y,
        )
        .unwrap();

        // Primary label
        let text_y = if node.sublabel.is_some() {
            pos.y + node_h / 2.0 - 13.0 * 0.45
        } else {
            pos.y + node_h / 2.0
        };
        writeln!(
            svg,
            "        <text x=\"{}\" y=\"{text_y}\">{}</text>",
            pos.x + node_w / 2.0,
            xml_escape(&node.label),
        )
        .unwrap();

        // Sublabel
        if let Some(ref sub) = node.sublabel {
            let sub_y = pos.y + node_h / 2.0 + 13.0 * 0.65;
            writeln!(
                svg,
                "        <text class=\"sublabel\" x=\"{}\" y=\"{sub_y}\">{}</text>",
                pos.x + node_w / 2.0,
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
}
