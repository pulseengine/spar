//! Public library surface for `spar-cli`.
//!
//! `spar-cli` is primarily a binary crate (`spar`); the library exposes
//! the *internals* needed by sibling crates — currently:
//!
//! - [`moves`] — the verify / enumerate pipelines used by the v0.9.0
//!   MCP tool surface ([`spar-mcp`](../spar_mcp/index.html)). The MCP
//!   tools call [`moves::verify_pipeline`] and
//!   [`moves::enumerate_pipeline`] directly so the wire-format report
//!   shape stays a single source of truth across CLI and MCP transport.
//! - [`load`] — the parse gate every model-loading subcommand goes
//!   through (#552): a model `spar parse` rejects is refused with
//!   `parse`'s own diagnostic.
//!
//! Everything else (LSP, codegen dispatch, refactor, diff, sarif) is
//! still private to the binary and exposed only via `spar <command>`.
//!
//! # Stability
//!
//! This is an internal-use-only library; the public surface only
//! supports the in-tree `spar-mcp` consumer. External users should
//! shell out to `spar` (or, for v0.9.0+, drive the MCP server) rather
//! than depend on this crate as a Rust library.

pub mod load;
pub mod moves;
pub mod variants_bridge;
