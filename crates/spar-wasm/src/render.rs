//! Top-level render function: AADL source to interactive HTML.
//!
//! Parses AADL source, instantiates a system implementation, and renders
//! via spar-render/etch for interactive architecture diagrams with ports,
//! orthogonal routing, and pan/zoom/selection.

use std::fmt;

use spar_hir::Database;

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
            if path.extension().is_some_and(|e| e == "aadl")
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                sources.push((path.display().to_string(), content));
            }
        }
    }
    if sources.is_empty() {
        return Err(RenderError::ParseError("no .aadl files found".into()));
    }

    let db = Database::from_aadl(
        &sources
            .iter()
            .map(|(f, c)| (f.clone(), c.clone()))
            .collect::<Vec<_>>(),
    );

    let instance = db
        .instantiate(root)
        .ok_or_else(|| RenderError::NoRoot(format!("cannot instantiate {}", root)))?;

    if instance
        .diagnostics()
        .iter()
        .any(|d| d.contains("Unresolved"))
    {
        return Err(RenderError::NoRoot(format!(
            "root {} has unresolved components",
            root
        )));
    }

    let render_opts = spar_render::RenderOptions {
        interactive: true,
        highlight: highlight.first().cloned(),
        ..Default::default()
    };
    let html_opts = etch::html::HtmlOptions {
        title: root.to_string(),
        ..Default::default()
    };
    Ok(spar_render::render_instance_html(
        instance.inner(),
        &render_opts,
        &html_opts,
    ))
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
            if path.extension().is_some_and(|e| e == "aadl")
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                sources.push((path.display().to_string(), content));
            }
        }
    }
    if sources.is_empty() {
        return Err(RenderError::ParseError("no .aadl files found".into()));
    }

    let db = Database::from_aadl(
        &sources
            .iter()
            .map(|(f, c)| (f.clone(), c.clone()))
            .collect::<Vec<_>>(),
    );

    let instance = db
        .instantiate(root)
        .ok_or_else(|| RenderError::NoRoot(format!("cannot instantiate {}", root)))?;

    if instance
        .diagnostics()
        .iter()
        .any(|d| d.contains("Unresolved"))
    {
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
    runner.register(Box::new(spar_analysis::flow_rules::FlowRuleAnalysis));
    runner.register(Box::new(spar_analysis::mode_check::ModeCheckAnalysis));
    runner.register(Box::new(spar_analysis::modal_rules::ModalRuleAnalysis));
    runner.register(Box::new(spar_analysis::binding_check::BindingCheckAnalysis));
    runner.register(Box::new(spar_analysis::binding_rules::BindingRuleAnalysis));
    runner.register(Box::new(
        spar_analysis::property_rules::PropertyRuleAnalysis,
    ));
    runner.register(Box::new(spar_analysis::scheduling::SchedulingAnalysis));
    runner.register(Box::new(spar_analysis::latency::LatencyAnalysis));
    runner.register(Box::new(
        spar_analysis::resource_budget::ResourceBudgetAnalysis,
    ));
    runner.register(Box::new(
        spar_analysis::direction_rules::DirectionRuleAnalysis,
    ));
    runner.register(Box::new(
        spar_analysis::connection_rules::ConnectionRuleAnalysis,
    ));
    runner.register(Box::new(
        spar_analysis::classifier_match::ClassifierMatchAnalysis,
    ));
    runner.register(Box::new(spar_analysis::mode_rules::ModeRuleAnalysis));
    runner.register(Box::new(
        spar_analysis::subcomponent_rules::SubcomponentRuleAnalysis,
    ));
    runner.register(Box::new(spar_analysis::emv2_analysis::Emv2Analysis));

    Ok(runner.run_all(instance.inner()))
}

/// Parse AADL source, instantiate the given root, and render to SVG.
///
/// `source` is the raw AADL text.  `root` is a qualified name such as
/// `"Pkg::Type.Impl"`.  `highlight` is a list of node IDs (e.g.
/// `"AADL-Pkg-sub1"`) that should be visually emphasized.
pub fn render_aadl(source: &str, root: &str, highlight: &[String]) -> Result<String, RenderError> {
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

    let render_opts = spar_render::RenderOptions {
        interactive: true,
        highlight: highlight.first().cloned(),
        ..Default::default()
    };
    let html_opts = etch::html::HtmlOptions {
        title: root.to_string(),
        ..Default::default()
    };
    Ok(spar_render::render_instance_html(
        instance.inner(),
        &render_opts,
        &html_opts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_basic_aadl() {
        let source = "package Pkg\npublic\n  system S\n  end S;\n  system implementation S.I\n    subcomponents\n      sub1: process P;\n  end S.I;\n  process P\n  end P;\nend Pkg;";
        let html = render_aadl(source, "Pkg::S.I", &[]).unwrap();
        assert!(html.contains("<!DOCTYPE html>"), "should be HTML");
        assert!(html.contains("<svg"), "should contain SVG");
        assert!(html.contains("<script>"), "should have interactivity");
    }

    #[test]
    fn render_with_highlight() {
        let source = "package Pkg\npublic\n  system S\n  end S;\n  system implementation S.I\n    subcomponents\n      sub1: process P;\n  end S.I;\n  process P\n  end P;\nend Pkg;";
        let html = render_aadl(source, "Pkg::S.I", &["AADL-Pkg-sub1".into()]).unwrap();
        assert!(html.contains("#ff6600")); // highlight color
    }

    #[test]
    fn render_invalid_root() {
        let source = "package Pkg\npublic\n  system S\n  end S;\nend Pkg;";
        let result = render_aadl(source, "Pkg::Nonexistent.Impl", &[]);
        assert!(result.is_err());
    }
}
