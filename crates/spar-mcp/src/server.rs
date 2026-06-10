//! Minimal MCP stdio JSON-RPC server.
//!
//! MCP's wire format is JSON-RPC 2.0 over stdio. Each message is a
//! single-line JSON object; the server reads one message per line and
//! writes one response per line. This is sufficient for the v0.9.0
//! oracle surface (no streaming progress, no notifications back to the
//! client beyond the initialise handshake).
//!
//! Supported methods (per MCP 2025-11-25):
//!
//! - `initialize` → returns server capabilities and protocol version.
//! - `ping` → no-op acknowledgement.
//! - `tools/list` → returns the three oracle tools with schemas.
//! - `tools/call` → invokes a named tool with `arguments`.
//!
//! Any other method returns `MethodNotFound` (-32601).

use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{self, ToolDescriptor};
use crate::tools::{ToolResult, dispatch_tool};

/// JSON-RPC 2.0 request envelope. `id` is `null` for notifications.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC error codes used by the spar MCP server.
pub mod error_codes {
    /// Standard JSON-RPC: invalid JSON.
    pub const PARSE_ERROR: i32 = -32700;
    /// Standard JSON-RPC: not a valid Request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Standard JSON-RPC: method does not exist.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Standard JSON-RPC: invalid method parameter(s).
    pub const INVALID_PARAMS: i32 = -32602;
    /// Standard JSON-RPC: server-side internal error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// MCP protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Server name advertised in the initialise handshake.
pub const SERVER_NAME: &str = "spar-mcp";

/// Server version advertised in the initialise handshake (mirrors the
/// crate version).
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Drive the stdio loop: read one JSON-RPC message per line from
/// `reader`, dispatch, write one response per line to `writer`. Returns
/// when the input EOFs or fails. Notifications (id == null) do not
/// produce a response.
pub fn run_stdio<R: Read, W: Write>(reader: R, mut writer: W) -> std::io::Result<()> {
    let buf = BufReader::new(reader);
    for line in buf.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_request_line(&line);
        if let Some(resp) = response {
            let serialized =
                serde_json::to_string(&resp).unwrap_or_else(|e| {
                    format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":-32603,\"message\":\"serialise failed: {e}\"}}}}"
                    )
                });
            writeln!(writer, "{serialized}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Run the server reading from stdin and writing to stdout. The
/// canonical entry point for `spar mcp serve`.
pub fn run() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_stdio(stdin.lock(), stdout.lock())
}

/// Parse and handle a single JSON-RPC line, returning the response (if
/// any). Notifications return `None`; everything else returns `Some`.
pub fn handle_request_line(line: &str) -> Option<JsonRpcResponse> {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(JsonRpcResponse {
                jsonrpc: "2.0",
                id: Value::Null,
                result: None,
                error: Some(JsonRpcError {
                    code: error_codes::PARSE_ERROR,
                    message: format!("invalid JSON: {e}"),
                    data: None,
                }),
            });
        }
    };
    let id = req.id.clone();
    if id.is_none() {
        // Notification — no response.
        let _ = handle_request(req);
        return None;
    }
    Some(handle_request(req))
}

/// Dispatch a single parsed JSON-RPC request to the appropriate
/// handler. Always returns a response object; the caller decides
/// whether to suppress it (notifications).
pub fn handle_request(req: JsonRpcRequest) -> JsonRpcResponse {
    let id = req.id.clone().unwrap_or(Value::Null);
    match req.method.as_str() {
        "initialize" => initialize(id),
        "ping" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        },
        "tools/list" => tools_list(id),
        "tools/call" => tools_call(id, &req.params),
        other => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("method `{other}` is not supported by spar-mcp"),
                data: None,
            }),
        },
    }
}

fn initialize(id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            },
            // Track E §6.5: the apply path is CLI-exclusive. Surface
            // this fact in the initialise handshake so an agent that
            // expects a write tool can fail loudly rather than silently
            // skip the write step.
            "instructions": "Read-only verification oracle. The deterministic apply path \
                             is CLI-exclusive (`spar moves apply`); no write tools are \
                             exposed over MCP.",
        })),
        error: None,
    }
}

fn tools_list(id: Value) -> JsonRpcResponse {
    let descriptors: Vec<ToolDescriptor> = schema::all_descriptors();
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({ "tools": descriptors })),
        error: None,
    }
}

fn tools_call(id: Value, params: &Value) -> JsonRpcResponse {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: error_codes::INVALID_PARAMS,
                    message: "tools/call: missing `name`".to_string(),
                    data: None,
                }),
            };
        }
    };
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    match dispatch_tool(name, &arguments) {
        Some(ToolResult::Ok(payload)) => {
            // Per the MCP spec, tools/call returns
            // { content: [{ type, text|json, ... }], isError: false }.
            // We follow the recommended shape and embed the structured
            // payload as a JSON-text content block so any compliant MCP
            // client can stream it through.
            let text = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({
                    "content": [
                        { "type": "text", "text": text }
                    ],
                    "structuredContent": payload,
                    "isError": false,
                })),
                error: None,
            }
        }
        Some(ToolResult::Error { code, message }) => {
            // Tool-level errors map to a result with isError=true,
            // *not* a JSON-RPC error — per MCP 2025-11-25 the agent
            // wants to see the tool's output even on failure.
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({
                    "content": [
                        { "type": "text", "text": format!("[{code}] {message}") }
                    ],
                    "structuredContent": { "code": code, "message": message },
                    "isError": true,
                })),
                error: None,
            }
        }
        None => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("tool `{name}` is not exposed by spar-mcp"),
                data: None,
            }),
        },
    }
}
