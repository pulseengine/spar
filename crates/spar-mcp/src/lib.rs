//! MCP (Model Context Protocol) server for spar's verification oracles.
//!
//! This crate exposes three read-only / idempotent tools to AI agents
//! over the standard MCP stdio JSON-RPC transport:
//!
//! - `spar.verify_move` — wraps `spar moves verify`.
//! - `spar.enumerate_moves` — wraps `spar moves enumerate`.
//! - `spar.check_chain` — end-to-end latency analysis for a thread chain.
//!
//! All three are *read-only*: they do not mutate the model file on
//! disk, do not return a binding overlay that the LLM can apply
//! directly, and do not interact with any persistent state. The
//! deterministic apply path (`spar moves apply` in a future release)
//! stays *CLI-exclusive* — there is, by certification design, no
//! `spar.apply_move` tool over MCP.
//!
//! # Why read-only?
//!
//! The certification chain Track E targets (DO-178C / ISO 26262) does
//! not allow an LLM to write into the verified-binding state directly.
//! The MCP surface is the LLM's *proposal* lane: the agent enumerates,
//! verifies, and inspects; a human (or higher-trust automation) calls
//! the deterministic apply path with the trace ID returned by the
//! verification oracle. See `docs/designs/track-e-migration-research.md`
//! §6.5 for the canonical design.
//!
//! # Crate layout
//!
//! - [`schema`] — JSON Schema declarations for each tool.
//! - [`tools`] — per-tool handlers calling into `spar-cli`'s pipelines.
//! - [`server`] — MCP stdio JSON-RPC dispatcher.
//!
//! # In-process API
//!
//! Tests and embedded callers can drive each tool directly via the
//! `*_call` helpers in [`tools`] without going through the JSON-RPC
//! server; [`server::handle_request`] is the same dispatch path the
//! stdio loop uses, but accepts a single message at a time.

pub mod schema;
pub mod server;
pub mod tools;

/// Re-exports of the report types exposed over the MCP wire so
/// downstream consumers can deserialise responses without depending on
/// `spar-cli` directly.
pub use spar_cli::moves::{
    EnumerateArgs, EnumerationObjective, MoveCandidate, MoveEnumerateReport, MoveVerifyReport,
    MovesError, VerifyArgs, Violation,
};
