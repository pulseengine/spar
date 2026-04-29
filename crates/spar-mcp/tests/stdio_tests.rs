//! Stdio JSON-RPC integration tests for spar-mcp.
//!
//! These tests drive the server one message at a time via
//! [`spar_mcp::server::handle_request_line`]. The same dispatch path
//! is what the stdio loop uses; testing it message-at-a-time keeps the
//! tests deterministic and free of background-thread / pipe-buffer
//! synchronisation.

use std::path::PathBuf;

use serde_json::{Value, json};

use spar_mcp::server::handle_request_line;

fn write_model(tag: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "spar_mcp_stdio_{}_{}.aadl",
        std::process::id(),
        tag,
    ));
    std::fs::write(&path, body).expect("write temp AADL");
    path
}

fn cleanup(p: &PathBuf) {
    let _ = std::fs::remove_file(p);
}

const MOBILE_MODEL: &str = "\
package Migrate
public
  processor CPU
  end CPU;

  thread Worker
  end Worker;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      t1: thread Worker;
  end Proc.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu1: processor CPU;
      cpu2: processor CPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu1)) applies to app.t1;
  end Sys.Impl;
end Migrate;
";

const CHAIN_MODEL: &str = "\
package Chain
public
  processor CPU
  end CPU;

  thread Producer
    features
      out_p: out data port;
    flows
      f1: flow source out_p;
    properties
      Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 2 ms;
      Dispatch_Protocol => Periodic;
  end Producer;

  thread Consumer
    features
      in_p: in data port;
    flows
      f2: flow sink in_p;
    properties
      Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 3 ms;
      Dispatch_Protocol => Periodic;
  end Consumer;

  process Proc
    features
      out_p: out data port;
      in_p:  in data port;
  end Proc;

  process implementation Proc.Impl
    subcomponents
      producer: thread Producer;
      consumer: thread Consumer;
    connections
      c_out: port producer.out_p -> out_p;
      c_in:  port in_p -> consumer.in_p;
  end Proc.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu1: processor CPU;
      cpu2: processor CPU;
      app: process Proc.Impl;
    connections
      loop_back: port app.out_p -> app.in_p;
    flows
      chain: end to end flow app.producer.f1 -> loop_back -> app.consumer.f2;
    properties
      Actual_Processor_Binding => (reference (cpu1)) applies to app.producer;
      Actual_Processor_Binding => (reference (cpu2)) applies to app.consumer;
  end Sys.Impl;
end Chain;
";

/// Send a single line through the server and return its parsed
/// response payload (the JSON-RPC envelope). Panics if the server
/// returns no response (notifications) — none of these tests exercise
/// that path.
fn drive(line: &str) -> Value {
    let resp = handle_request_line(line).expect("expected a response, got None (notification?)");
    serde_json::to_value(&resp).expect("response serialises")
}

// ── 6. tools_list_returns_three_tools ────────────────────────────────

#[test]
fn tools_list_returns_three_tools() {
    let resp = drive(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    assert_eq!(resp["jsonrpc"].as_str(), Some("2.0"));
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("result.tools must be array; got {resp}");
    assert_eq!(tools.len(), 3, "expected exactly three tools; got {resp}");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"spar.verify_move"), "names={names:?}");
    assert!(names.contains(&"spar.enumerate_moves"), "names={names:?}");
    assert!(names.contains(&"spar.check_chain"), "names={names:?}");
    // Annotations: every tool is read-only / idempotent.
    for t in tools {
        assert_eq!(
            t["annotations"]["readOnlyHint"].as_bool(),
            Some(true),
            "tool {t} must declare readOnlyHint=true",
        );
        assert_eq!(
            t["annotations"]["idempotentHint"].as_bool(),
            Some(true),
            "tool {t} must declare idempotentHint=true",
        );
    }
}

// ── 7. tools_call_verify_returns_valid_json_result ───────────────────

#[test]
fn tools_call_verify_returns_valid_json_result() {
    let path = write_model("verify_call", MOBILE_MODEL);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "spar.verify_move",
            "arguments": {
                "model": path.to_string_lossy(),
                "root": "Migrate::Sys.Impl",
                "component": "t1",
                "target": "cpu2",
            }
        }
    });
    let resp = drive(&req.to_string());
    assert_eq!(
        resp["id"].as_i64(),
        Some(7),
        "id must round-trip; got {resp}"
    );
    let result = &resp["result"];
    assert_eq!(
        result["isError"].as_bool(),
        Some(false),
        "isError should be false on a successful tool call; got {resp}",
    );
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["ok"].as_bool(),
        Some(true),
        "structured content must include ok=true; got {resp}",
    );
    assert!(
        structured["component"].is_string(),
        "structured content must include component; got {resp}",
    );
    cleanup(&path);
}

// ── 8. tools_call_enumerate_returns_valid_json_result ────────────────

#[test]
fn tools_call_enumerate_returns_valid_json_result() {
    let path = write_model("enumerate_call", MOBILE_MODEL);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "spar.enumerate_moves",
            "arguments": {
                "model": path.to_string_lossy(),
                "root": "Migrate::Sys.Impl",
                "component": "t1",
            }
        }
    });
    let resp = drive(&req.to_string());
    let result = &resp["result"];
    assert_eq!(result["isError"].as_bool(), Some(false));
    let structured = &result["structuredContent"];
    assert!(
        structured["candidates"].is_array(),
        "structured content must include candidates array; got {resp}",
    );
    assert!(
        structured["total"].as_u64().is_some(),
        "structured content must include total; got {resp}",
    );
    cleanup(&path);
}

// ── 9. tools_call_check_chain_returns_valid_json_result ──────────────

#[test]
fn tools_call_check_chain_returns_valid_json_result() {
    let path = write_model("chain_call", CHAIN_MODEL);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "spar.check_chain",
            "arguments": {
                "model": path.to_string_lossy(),
                "root": "Chain::Sys.Impl",
                "source_thread": "producer",
                "sink_thread": "consumer",
            }
        }
    });
    let resp = drive(&req.to_string());
    let result = &resp["result"];
    assert_eq!(result["isError"].as_bool(), Some(false));
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["flow_name"].as_str(),
        Some("chain"),
        "structured content must include flow_name=chain; got {resp}",
    );
    assert!(
        structured["diagnostics"].is_array(),
        "structured content must include diagnostics array; got {resp}",
    );
    cleanup(&path);
}

// ── 10. unknown_tool_returns_method_not_found_error ──────────────────

#[test]
fn unknown_tool_returns_method_not_found_error() {
    let req = r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"spar.nonexistent","arguments":{}}}"#;
    let resp = drive(req);
    assert_eq!(resp["id"].as_i64(), Some(10));
    let err = resp["error"]
        .as_object()
        .expect("missing error object; got {resp}");
    assert_eq!(
        err["code"].as_i64(),
        Some(-32601),
        "expected JSON-RPC code -32601 (MethodNotFound); got {resp}",
    );
    let msg = err["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("spar.nonexistent"),
        "error message should mention the tool name; got {msg}",
    );
}

// ── 11. check_chain_with_variant_returns_bad_input_via_stdio ─────────

#[test]
fn check_chain_with_variant_returns_bad_input_via_stdio() {
    // Tier A #7 reproduced through the stdio JSON-RPC envelope: an
    // agent calling spar.check_chain with a variant must see a tool-
    // level error (isError=true, code=BAD_INPUT) rather than an
    // unfiltered success payload.
    let path = write_model("chain_variant_stdio", CHAIN_MODEL);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "tools/call",
        "params": {
            "name": "spar.check_chain",
            "arguments": {
                "model": path.to_string_lossy(),
                "root": "Chain::Sys.Impl",
                "source_thread": "producer",
                "sink_thread": "consumer",
                "variant": "test-variant",
            }
        }
    });
    let resp = drive(&req.to_string());
    let result = &resp["result"];
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "expected isError=true on BAD_INPUT; got {resp}",
    );
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["code"].as_str(),
        Some("BAD_INPUT"),
        "expected structured code=BAD_INPUT; got {resp}",
    );
    let msg = structured["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("not yet supported"),
        "expected message to explain limitation; got {msg}",
    );
    cleanup(&path);
}

// ── 12. tools_call_rejects_variant_plus_variant_context_via_stdio ────

#[test]
fn check_chain_rejects_both_variant_args_via_stdio() {
    // Tier C #50: mutual exclusion is enforced before the not-yet-
    // supported refusal, so the message points at the conflict rather
    // than the deferred-feature error.
    let path = write_model("chain_both_variant_stdio", CHAIN_MODEL);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 101,
        "method": "tools/call",
        "params": {
            "name": "spar.check_chain",
            "arguments": {
                "model": path.to_string_lossy(),
                "root": "Chain::Sys.Impl",
                "source_thread": "producer",
                "sink_thread": "consumer",
                "variant": "test-variant",
                "variant_context": "/tmp/ctx.json",
            }
        }
    });
    let resp = drive(&req.to_string());
    let result = &resp["result"];
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "expected isError=true on conflict; got {resp}",
    );
    let structured = &result["structuredContent"];
    assert_eq!(
        structured["code"].as_str(),
        Some("BAD_INPUT"),
        "expected structured code=BAD_INPUT; got {resp}",
    );
    let msg = structured["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("mutually exclusive"),
        "expected message to mention mutual exclusion; got {msg}",
    );
    cleanup(&path);
}

// ── 13. tools_list_advertises_mutual_exclusion_in_schema ─────────────

#[test]
fn tools_list_advertises_variant_mutex_in_schema() {
    // Each tool's input schema must encode the variant <-> variant_context
    // mutex via JSON Schema `not` so a strict client can reject the
    // conflicting payload before sending it.
    let resp = drive(r#"{"jsonrpc":"2.0","id":50,"method":"tools/list"}"#);
    let tools = resp["result"]["tools"]
        .as_array()
        .expect("result.tools must be array");
    for t in tools {
        let name = t["name"].as_str().unwrap_or_default();
        let not_clause = &t["inputSchema"]["not"];
        assert!(
            not_clause.is_object(),
            "tool {name} must declare a top-level `not` clause for variant mutex; got {t}",
        );
        let required = not_clause["required"]
            .as_array()
            .unwrap_or_else(|| panic!("tool {name} `not.required` missing"));
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            names.contains(&"variant") && names.contains(&"variant_context"),
            "tool {name} must mark both variant and variant_context as forbidden together; \
             got {names:?}",
        );
    }
}

// ── Bonus: an unknown JSON-RPC method also gets MethodNotFound ───────

#[test]
fn unknown_jsonrpc_method_returns_method_not_found_error() {
    let req = r#"{"jsonrpc":"2.0","id":11,"method":"sampling/createMessage","params":{}}"#;
    let resp = drive(req);
    let err = resp["error"]
        .as_object()
        .expect("missing error object; got {resp}");
    assert_eq!(
        err["code"].as_i64(),
        Some(-32601),
        "expected JSON-RPC code -32601 for unknown method; got {resp}",
    );
}
