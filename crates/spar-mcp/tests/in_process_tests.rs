//! In-process tests for the spar-mcp tool surface.
//!
//! These tests call each tool's `call(arguments)` entry point directly
//! through the Rust API, bypassing the JSON-RPC stdio transport. The
//! stdio path is exercised separately in `stdio_tests.rs`; both paths
//! end up routing through the same `dispatch_tool` machinery, so the
//! split keeps the JSON-RPC envelope concerns out of these tests.

use std::path::PathBuf;

use serde_json::{Value, json};

use spar_mcp::tools::{ToolResult, check_chain, enumerate, verify};

/// Per-test temp file: process id + per-test tag, mirroring the
/// pattern in `crates/spar-cli/tests/moves_*.rs`.
fn write_model(tag: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("spar_mcp_{}_{}.aadl", std::process::id(), tag,));
    std::fs::write(&path, body).expect("write temp AADL");
    path
}

fn cleanup(p: &PathBuf) {
    let _ = std::fs::remove_file(p);
}

/// Mirrors `MOBILE_MODEL` in `moves_verify.rs`: one thread `t1`, two
/// processors `cpu1` (declared) + `cpu2` (legal target).
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

/// A two-thread chain on two processors with one connection segment.
/// `producer` runs on cpu1, `consumer` on cpu2; the end-to-end flow
/// `chain` connects them via `c1`. Compute hops carry an explicit
/// `Compute_Execution_Time` so the latency pass can produce a bound.
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
    flows
      src_flow: flow source out_p;
      sink_flow: flow sink in_p;
  end Proc;

  process implementation Proc.Impl
    subcomponents
      producer: thread Producer;
      consumer: thread Consumer;
    connections
      c_out: port producer.out_p -> out_p;
      c_in:  port in_p -> consumer.in_p;
    flows
      src_flow: flow source producer.f1 -> c_out -> out_p;
      sink_flow: flow sink in_p -> c_in -> consumer.f2;
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
      chain: end to end flow app.src_flow -> loop_back -> app.sink_flow;
    properties
      Actual_Processor_Binding => (reference (cpu1)) applies to app.producer;
      Actual_Processor_Binding => (reference (cpu2)) applies to app.consumer;
  end Sys.Impl;
end Chain;
";

// A flat latency chain: source and sink are DIRECT subcomponents of the
// flow's owner, each carrying `Compute_Execution_Time`. CHAIN_MODEL reaches
// timed threads inside a `process` through flow specifications on the process
// (descent); this one needs no descent at all.
const LATENCY_CHAIN_MODEL: &str = "\
package LatChain
public
  device Producer
    features
      out_p: out data port;
    flows
      f1: flow source out_p;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end Producer;

  device Consumer
    features
      in_p: in data port;
    flows
      f2: flow sink in_p;
    properties
      Compute_Execution_Time => 1 ms .. 3 ms;
  end Consumer;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      producer: device Producer;
      consumer: device Consumer;
    connections
      c: port producer.out_p -> consumer.in_p;
    flows
      chain: end to end flow producer.f1 -> c -> consumer.f2;
  end Sys.Impl;
end LatChain;
";

// ── 1. verify_tool_returns_ok_when_move_is_valid ─────────────────────

#[test]
fn verify_tool_returns_ok_when_move_is_valid() {
    let path = write_model("ok", MOBILE_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
        "target":    "cpu2",
    });
    let result = verify::call(&args);
    let payload = match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => {
            cleanup(&path);
            panic!("expected Ok, got Error[{code}] {message}");
        }
    };
    assert_eq!(payload["ok"].as_bool(), Some(true), "payload was {payload}");
    assert_eq!(
        payload["violations"].as_array().map(|a| a.len()),
        Some(0),
        "expected no violations; got {payload}",
    );
    assert_eq!(
        payload["cli_exit_code"].as_i64(),
        Some(0),
        "exit code should be 0 for an admissible move",
    );
    cleanup(&path);
}

// ── 2. enumerate_tool_lists_all_processors_no_allowed_targets ────────

#[test]
fn enumerate_tool_lists_all_processors_no_allowed_targets() {
    let path = write_model("enum_all", MOBILE_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
    });
    let result = enumerate::call(&args);
    let payload = match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => {
            cleanup(&path);
            panic!("expected Ok, got Error[{code}] {message}");
        }
    };
    let candidates = payload["candidates"]
        .as_array()
        .expect("candidates must be array");
    assert_eq!(
        candidates.len(),
        2,
        "expected cpu1 + cpu2 in candidate set; got {payload}",
    );
    let names: Vec<&str> = candidates
        .iter()
        .filter_map(|c| c["target"].as_str())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("cpu1")),
        "expected cpu1 candidate; got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.ends_with("cpu2")),
        "expected cpu2 candidate; got {names:?}",
    );
    cleanup(&path);
}

// ── 3. check_chain_tool_returns_latency_breakdown ────────────────────

#[test]
fn check_chain_tool_returns_latency_breakdown() {
    // The flat LATENCY_CHAIN_MODEL: both flow ends are direct subcomponents,
    // so a real breakdown is derived without any flow-implementation descent.
    let path = write_model("lat_chain", LATENCY_CHAIN_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "LatChain::Sys.Impl",
        "source_thread": "producer",
        "sink_thread":   "consumer",
    });
    let result = check_chain::call(&args);
    let payload = match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => {
            cleanup(&path);
            panic!("expected Ok, got Error[{code}] {message}");
        }
    };
    assert_eq!(payload["flow_name"].as_str(), Some("chain"));
    let diags = payload["diagnostics"]
        .as_array()
        .expect("diagnostics must be array");
    assert!(
        !diags.is_empty(),
        "expected at least one latency diagnostic for the chain; got {payload}",
    );
    // The latency pass emits an Info diagnostic of the form
    // `end-to-end flow 'chain' latency: [a ms .. b ms]` — surface that
    // the bounds are present.
    let has_bounds = diags.iter().any(|d| {
        d["message"]
            .as_str()
            .map(|m| m.contains("latency:") && m.contains("ms"))
            .unwrap_or(false)
    });
    assert!(
        has_bounds,
        "expected a latency-bounds diagnostic; got {diags:?}",
    );
    // And no "not computed" provenance error, since this flow IS derivable.
    let not_computed = diags.iter().any(|d| {
        d["message"]
            .as_str()
            .map(|m| m.contains("not computed"))
            .unwrap_or(false)
    });
    assert!(
        !not_computed,
        "a flow with timing on both ends must derive a figure, not error: {diags:?}",
    );
    cleanup(&path);
}

// ── 4. verify_tool_with_variant_filters_components ───────────────────

#[test]
fn verify_tool_with_variant_filters_components() {
    // Drive the implicit-variant path through the SPAR_VARIANT_TEST_RIVET_OUTPUT
    // test seam. The variant context resolves to a name that the
    // verify pipeline materialises into the report's `variant` field —
    // the agent uses this to route follow-up calls to the same
    // variant resolution.
    let path = write_model("variant", MOBILE_MODEL);

    // Minimal v1 variant context payload — see
    // `crates/spar-variants/src/context.rs` for the canonical schema.
    let payload = serde_json::json!({
        "rivet_spar_context_version": "1",
        "variant": "test-variant",
        "features": [],
        "bindings": [],
        "feature_model_hash": "deadbeef",
        "resolved_at": "2026-04-23T00:00:00Z",
        "generated_by": "spar-mcp-test",
    });
    // Safety: the test seam is deliberately a process-level env var,
    // mirroring how the moves CLI's variant tests drive the same
    // flag. Tests in this file do not set it concurrently with the
    // moves-side integration tests because each test crate has its
    // own process.
    unsafe {
        std::env::set_var("SPAR_VARIANT_TEST_RIVET_OUTPUT", payload.to_string());
    }

    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
        "target":    "cpu2",
        "variant":   "test-variant",
    });
    let result = verify::call(&args);
    unsafe {
        std::env::remove_var("SPAR_VARIANT_TEST_RIVET_OUTPUT");
    }
    let payload = match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => {
            cleanup(&path);
            panic!("expected Ok, got Error[{code}] {message}");
        }
    };
    assert_eq!(
        payload["variant"].as_str(),
        Some("test-variant"),
        "expected variant audit-trail field to be propagated; got {payload}",
    );
    assert_eq!(payload["feature_model_hash"].as_str(), Some("deadbeef"));
    cleanup(&path);
}

// ── 5. enumerate_tool_with_objective_picks_least_loaded ──────────────

#[test]
fn enumerate_tool_with_objective_picks_least_loaded() {
    // With objective=total-load, the candidate with the lowest
    // post-move utilisation should sort first. Both processors are
    // empty in the MOBILE_MODEL fixture, but the ranker's score is
    // still well-defined (zero or near-zero) — we confirm the
    // payload's candidates order respects the score ordering and that
    // no candidate has an `<missed>` sentinel.
    let path = write_model("enum_obj", MOBILE_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
        "objective": "total-load",
    });
    let result = enumerate::call(&args);
    let payload = match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => {
            cleanup(&path);
            panic!("expected Ok, got Error[{code}] {message}");
        }
    };
    let candidates = payload["candidates"].as_array().expect("array");
    assert!(!candidates.is_empty(), "expected at least one candidate");

    // Scores must be monotone non-decreasing across the candidates
    // (admissible-first sort, then score ascending).
    let scores: Vec<f64> = candidates
        .iter()
        .map(|c| c["rank"]["score"].as_f64().unwrap_or(f64::NAN))
        .collect();
    for w in scores.windows(2) {
        // Reject NaNs only; equal scores are fine.
        assert!(
            !w[0].is_nan() && !w[1].is_nan(),
            "rank score should be finite for a well-formed model; got {scores:?}",
        );
        assert!(
            w[0] <= w[1] + f64::EPSILON,
            "candidates should be sorted by score ascending; got {scores:?}",
        );
    }

    // Unknown objective string surfaces a BAD_INPUT error.
    let bad = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
        "objective": "not-a-real-mode",
    });
    let bad_result = enumerate::call(&bad);
    match bad_result {
        ToolResult::Error { code, .. } => {
            assert_eq!(code, "BAD_INPUT", "expected BAD_INPUT for bogus objective");
        }
        ToolResult::Ok(v) => {
            cleanup(&path);
            panic!("expected error for bogus objective, got Ok({v})");
        }
    }
    cleanup(&path);
}

// ── 6. check_chain_rejects_variant_input_with_bad_input ──────────────

#[test]
fn check_chain_rejects_variant_input_with_bad_input() {
    // Tier A #7: check_chain previously accepted `variant` and silently
    // discarded it, returning unfiltered chain results. Until variant
    // scoping is wired through `LatencyAnalysis` (v0.10 enhancement),
    // the tool refuses the input cleanly so an agent cannot mistake an
    // unfiltered answer for a variant-scoped one.
    let path = write_model("chain_variant_reject", CHAIN_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Chain::Sys.Impl",
        "source_thread": "producer",
        "sink_thread":   "consumer",
        "variant":       "test-variant",
    });
    let result = check_chain::call(&args);
    match result {
        ToolResult::Error { code, message } => {
            assert_eq!(
                code, "BAD_INPUT",
                "expected BAD_INPUT for variant on check_chain; got {code}: {message}",
            );
            assert!(
                message.contains("not yet supported"),
                "error message should explain the limitation; got {message}",
            );
            assert!(
                message.contains("verify_move") || message.contains("enumerate_moves"),
                "error message should suggest the variant-aware tools; got {message}",
            );
        }
        ToolResult::Ok(v) => {
            cleanup(&path);
            panic!("expected BAD_INPUT for variant input on check_chain, got Ok({v})");
        }
    }
    cleanup(&path);
}

// ── 7. tools_reject_both_variant_and_variant_context ─────────────────

#[test]
fn check_chain_rejects_both_variant_and_variant_context() {
    // Tier C #50: mutual exclusion was previously enforced only at the
    // pipeline layer (verify/enumerate); check_chain now applies it
    // before the not-yet-supported refusal so an agent supplying both
    // gets a stable BAD_INPUT mentioning the conflict.
    let path = write_model("chain_both_variant", CHAIN_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Chain::Sys.Impl",
        "source_thread": "producer",
        "sink_thread":   "consumer",
        "variant":         "test-variant",
        "variant_context": "/tmp/ctx.json",
    });
    let result = check_chain::call(&args);
    match result {
        ToolResult::Error { code, message } => {
            assert_eq!(
                code, "BAD_INPUT",
                "expected BAD_INPUT; got {code}: {message}"
            );
            assert!(
                message.contains("mutually exclusive"),
                "error message should mention mutual exclusion; got {message}",
            );
        }
        ToolResult::Ok(v) => {
            cleanup(&path);
            panic!("expected BAD_INPUT for variant+variant_context, got Ok({v})");
        }
    }
    cleanup(&path);
}

#[test]
fn verify_rejects_both_variant_and_variant_context() {
    // The verify pipeline already maps the conflict to
    // `MovesError::VariantArgsConflict`, which classifies as
    // BAD_INPUT. This test pins the contract so future refactors can't
    // regress it without breaking the agent-visible error code.
    let path = write_model("verify_both_variant", MOBILE_MODEL);
    let args = json!({
        "model": path.to_string_lossy(),
        "root":  "Migrate::Sys.Impl",
        "component": "t1",
        "target":    "cpu2",
        "variant":         "test-variant",
        "variant_context": "/tmp/ctx.json",
    });
    let result = verify::call(&args);
    match result {
        ToolResult::Error { code, message } => {
            assert_eq!(
                code, "BAD_INPUT",
                "expected BAD_INPUT; got {code}: {message}"
            );
            assert!(
                message.contains("mutually exclusive"),
                "error message should mention mutual exclusion; got {message}",
            );
        }
        ToolResult::Ok(v) => {
            cleanup(&path);
            panic!("expected BAD_INPUT for variant+variant_context, got Ok({v})");
        }
    }
    cleanup(&path);
}

// ── Validation helper used by stdio tests too ─────────────────────────

/// Used by both in-process and stdio tests: accept either Ok(payload)
/// or panic. Kept here so the stdio tests can re-import via
/// `#[path = "in_process_tests.rs"] mod fixtures;` if needed; for now
/// each test crate maintains its own fixtures.
#[allow(dead_code)]
pub(crate) fn unwrap_ok(r: ToolResult) -> Value {
    match r {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => panic!("expected Ok, got Error[{code}] {message}"),
    }
}

// ── check_chain resolves flows the way latency does (#455) ───────────

const LEGAL_DECL: &str = "chain: end to end flow app.src_flow -> loop_back -> app.sink_flow;";
const NESTED_DECL: &str = "chain: end to end flow app.producer.f1 -> loop_back -> app.consumer.f2;";

/// CHAIN_MODEL with its single end-to-end flow declaration replaced.
fn chain_model_with(decls: &str) -> String {
    let model = CHAIN_MODEL.replace(LEGAL_DECL, decls);
    assert_ne!(model, CHAIN_MODEL, "declaration not found");
    model
}

fn chain_args(path: &std::path::Path, source: &str, sink: &str) -> Value {
    json!({
        "model": path.to_string_lossy(),
        "root":  "Chain::Sys.Impl",
        "source_thread": source,
        "sink_thread":   sink,
    })
}

fn ok_payload(result: ToolResult) -> Value {
    match result {
        ToolResult::Ok(v) => v,
        ToolResult::Error { code, message } => panic!("expected Ok, got Error[{code}] {message}"),
    }
}

fn error_of(result: ToolResult) -> (&'static str, String) {
    match result {
        ToolResult::Error { code, message } => (code, message),
        ToolResult::Ok(v) => panic!("expected an Error, got Ok({v})"),
    }
}

/// A flow reaching the threads through the process's flow implementation
/// (`app.src_flow` / `app.sink_flow`) reports the THREADS it passes through.
#[test]
fn check_chain_reports_threads_reached_through_a_flow_implementation() {
    let path = write_model("legal_threads", CHAIN_MODEL);
    for (source, sink) in [
        ("producer", "consumer"),
        ("app.producer", "app.consumer"),
        ("PRODUCER", "Consumer"),
    ] {
        let p = ok_payload(check_chain::call(&chain_args(&path, source, sink)));
        assert_eq!(p["flow_name"].as_str(), Some("chain"), "{p}");
        assert!(
            p["source_thread"]
                .as_str()
                .unwrap()
                .ends_with("/app/producer"),
            "{p}"
        );
        assert!(
            p["sink_thread"]
                .as_str()
                .unwrap()
                .ends_with("/app/consumer"),
            "{p}"
        );
        // The figure is derived from the threads' Compute_Execution_Time.
        let diags = p["diagnostics"].as_array().unwrap();
        assert!(
            diags.iter().any(|d| {
                let m = d["message"].as_str().unwrap();
                m.contains("'chain' latency:") && !m.contains("not computed")
            }),
            "{p}"
        );
        assert_eq!(p["has_errors"].as_bool(), Some(false), "{p}");
    }
    cleanup(&path);
}

/// Source and sink are the FIRST and LAST component reached, not any
/// component or any dotted part of a segment.
#[test]
fn check_chain_matches_first_and_last_component_only() {
    let path = write_model("first_last", CHAIN_MODEL);
    for (source, sink) in [
        ("consumer", "producer"), // reversed
        ("producer", "producer"), // sink is not the last component
        ("consumer", "consumer"), // source is not the first component
    ] {
        let (code, _) = error_of(check_chain::call(&chain_args(&path, source, sink)));
        assert_eq!(code, "CHAIN_NOT_FOUND", "{source:?} -> {sink:?}");
    }
    cleanup(&path);
}

/// The old matcher accepted any `.`-separated part of a segment, so the
/// subcomponent `app` (a process, never a chain end) and the flow names
/// `src_flow` / `sink_flow` matched. They name no component the flow reaches.
#[test]
fn check_chain_no_longer_matches_any_dotted_part_of_a_segment() {
    let path = write_model("dotted_part", CHAIN_MODEL);
    for (source, sink) in [
        ("app", "app"),
        ("src_flow", "sink_flow"),
        ("app.src_flow", "app.sink_flow"),
    ] {
        let (code, _) = error_of(check_chain::call(&chain_args(&path, source, sink)));
        assert_eq!(code, "CHAIN_NOT_FOUND", "{source:?} -> {sink:?}");
    }
    cleanup(&path);
}

/// A nested (`app.producer.f1`) segment is not AADL: the tool reports the
/// flow as invalid with flow_check's reason, instead of matching a thread.
#[test]
fn check_chain_reports_a_nested_segment_flow_as_invalid() {
    let path = write_model("nested", &chain_model_with(NESTED_DECL));
    let (code, message) = error_of(check_chain::call(&chain_args(
        &path, "producer", "consumer",
    )));
    assert_eq!(code, "CHAIN_INVALID");
    for want in [
        "end-to-end flow `chain`",
        "segment 'app.producer.f1' at position 0 reaches inside subcomponent 'app'",
        "not analysed",
    ] {
        assert!(message.contains(want), "want `{want}` in {message}");
    }
    cleanup(&path);
}

/// Only the requested flow's rejection is reported, and both names must be
/// mentioned for a rejected flow to be reported at all.
#[test]
fn check_chain_invalid_reports_only_the_named_flow() {
    let decls = "bad: end to end flow app.producer.f1 -> loop_back -> app.consumer.f2;\n      \
                 other: end to end flow app.consumer.f2 -> loop_back -> app.producer.f1;";
    let path = write_model("nested_two", &chain_model_with(decls));
    let (code, message) = error_of(check_chain::call(&chain_args(
        &path, "producer", "consumer",
    )));
    assert_eq!(code, "CHAIN_INVALID");
    assert!(message.contains("flow `bad`"), "{message}");
    assert!(message.contains("flow 'bad'"), "{message}");
    assert!(!message.contains("'other'"), "{message}");
    // Case-insensitive mention.
    let (code, _) = error_of(check_chain::call(&chain_args(
        &path, "PRODUCER", "CONSUMER",
    )));
    assert_eq!(code, "CHAIN_INVALID");
    // One name alone does not select a rejected flow.
    for (source, sink) in [("producer", "nobody"), ("nobody", "consumer")] {
        let (code, _) = error_of(check_chain::call(&chain_args(&path, source, sink)));
        assert_eq!(code, "CHAIN_NOT_FOUND", "{source:?} -> {sink:?}");
    }
    cleanup(&path);
}

/// A valid flow wins over a rejected one that mentions the same names,
/// whichever is declared first.
#[test]
fn check_chain_prefers_a_valid_flow_over_a_rejected_one() {
    let bad = "bad: end to end flow app.producer.f1 -> loop_back -> app.consumer.f2;";
    let good = "good: end to end flow app.src_flow -> loop_back -> app.sink_flow;";
    for decls in [
        format!("{bad}\n      {good}"),
        format!("{good}\n      {bad}"),
    ] {
        let path = write_model("both", &chain_model_with(&decls));
        let p = ok_payload(check_chain::call(&chain_args(
            &path, "producer", "consumer",
        )));
        assert_eq!(p["flow_name"].as_str(), Some("good"), "{p}");
        cleanup(&path);
    }
}
