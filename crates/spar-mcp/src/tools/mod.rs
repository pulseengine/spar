//! Tool dispatcher and per-tool input/output handlers.
//!
//! Each tool is implemented in its own submodule and exposes a single
//! entry point of the form `pub fn call(args: &Value) -> ToolResult`.
//! The dispatcher in [`dispatch_tool`] routes by tool name and returns
//! a typed [`ToolResult`] which the JSON-RPC layer turns into either a
//! `result` payload or a structured error.

pub mod check_chain;
pub mod enumerate;
pub mod verify;

use serde::Serialize;
use serde_json::Value;

use crate::schema;

/// Outcome of a single MCP tool call.
#[derive(Debug, Clone, Serialize)]
pub enum ToolResult {
    /// Tool ran to completion. Payload is the structured JSON the
    /// agent will see in its `tools/call` response.
    Ok(Value),
    /// Tool refused the call (bad inputs, model not found, …). The
    /// JSON-RPC layer renders this as an MCP-style error result with
    /// `isError: true` so the agent can react to the failure without
    /// a transport-level abort.
    Error {
        /// Stable error code (e.g., `"BAD_INPUT"`, `"MODEL_NOT_FOUND"`).
        code: &'static str,
        /// Human-readable message for the agent / log.
        message: String,
    },
}

/// Dispatch a tools/call by name. Unknown tool names return a
/// JSON-RPC `MethodNotFound` (-32601) at the server layer; this
/// function only routes the *known* tool surface.
pub fn dispatch_tool(name: &str, arguments: &Value) -> Option<ToolResult> {
    match name {
        schema::VERIFY_MOVE => Some(verify::call(arguments)),
        schema::ENUMERATE_MOVES => Some(enumerate::call(arguments)),
        schema::CHECK_CHAIN => Some(check_chain::call(arguments)),
        _ => None,
    }
}

/// Read a required string argument, returning a `BAD_INPUT`
/// [`ToolResult::Error`] when the field is missing or not a string.
pub(crate) fn required_string(args: &Value, key: &str) -> Result<String, ToolResult> {
    match args.get(key).and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(ToolResult::Error {
            code: "BAD_INPUT",
            message: format!("missing or non-string required argument: {key}"),
        }),
    }
}

/// Read an optional string argument; returns `None` when the field is
/// absent, `Some(s)` when present and a string. A non-string value
/// surfaces as a `BAD_INPUT` error to keep the contract strict.
pub(crate) fn optional_string(args: &Value, key: &str) -> Result<Option<String>, ToolResult> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ToolResult::Error {
            code: "BAD_INPUT",
            message: format!("argument `{key}` must be a string when present"),
        }),
    }
}
