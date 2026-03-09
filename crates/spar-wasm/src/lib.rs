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
pub use render::{render_aadl, RenderError};
