//! WASM component for AADL architecture visualization.
//!
//! Provides two capabilities as a WASI component:
//! 1. `adapter` — import/export AADL artifacts (same as CLI JSON output)
//! 2. `renderer` — parse AADL, instantiate, and render SVG via graph layout
//!
//! The component reads `.aadl` files via WASI filesystem and uses the full
//! spar-hir pipeline (including salsa) for semantic analysis.

mod graph;
mod render;

pub use graph::{ArchEdge, ArchNode, build_graph};
pub use render::{RenderError, analyze_aadl_from_fs, render_aadl, render_aadl_from_fs};

// ---------------------------------------------------------------------------
// WIT guest bindings (WASM component only)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        world: "spar-component",
        path: "wit/adapter.wit",
    });
}

#[cfg(target_arch = "wasm32")]
use bindings::exports::pulseengine::rivet::adapter::{self as wit_adapter, Guest as AdapterGuest};
#[cfg(target_arch = "wasm32")]
use bindings::exports::pulseengine::rivet::renderer::{
    AnalysisDiagnostic as WitDiagnostic, Guest as RendererGuest, RenderError as WitRenderError,
};

#[cfg(target_arch = "wasm32")]
struct SparComponent;

#[cfg(target_arch = "wasm32")]
fn map_render_err(e: render::RenderError) -> WitRenderError {
    match e {
        render::RenderError::ParseError(s) => WitRenderError::ParseError(s),
        render::RenderError::NoRoot(s) => WitRenderError::NoRoot(s),
        render::RenderError::LayoutError(s) => WitRenderError::LayoutError(s),
    }
}

#[cfg(target_arch = "wasm32")]
impl RendererGuest for SparComponent {
    fn render(root: String, highlight: Vec<String>) -> Result<String, WitRenderError> {
        render::render_aadl_from_fs(&root, &highlight).map_err(map_render_err)
    }

    fn analyze(root: String) -> Result<Vec<WitDiagnostic>, WitRenderError> {
        let diags = render::analyze_aadl_from_fs(&root).map_err(map_render_err)?;
        Ok(diags
            .into_iter()
            .map(|d| WitDiagnostic {
                severity: format!("{:?}", d.severity).to_lowercase(),
                message: d.message,
                component_path: d.path.join("."),
                analysis_name: d.analysis,
            })
            .collect())
    }
}

#[cfg(target_arch = "wasm32")]
impl AdapterGuest for SparComponent {
    fn id() -> String {
        "aadl".into()
    }

    fn name() -> String {
        "AADL (spar)".into()
    }

    fn supported_types() -> Vec<String> {
        vec![
            "aadl-component".into(),
            "aadl-analysis-result".into(),
            "aadl-flow".into(),
        ]
    }

    fn import(
        _source: Vec<u8>,
        _config: wit_adapter::AdapterConfig,
    ) -> Result<Vec<wit_adapter::Artifact>, wit_adapter::AdapterError> {
        Err(wit_adapter::AdapterError::NotSupported(
            "import not yet implemented in WASM".into(),
        ))
    }

    fn export(
        _artifacts: Vec<wit_adapter::Artifact>,
        _config: wit_adapter::AdapterConfig,
    ) -> Result<Vec<u8>, wit_adapter::AdapterError> {
        Err(wit_adapter::AdapterError::NotSupported(
            "export not supported".into(),
        ))
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(SparComponent with_types_in bindings);
