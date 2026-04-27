//! Integration tests for `spar moves verify --variant` and
//! `spar moves enumerate --variant` (Track E commit 6/8).
//!
//! Each test builds a small inline AADL model, drops it (and an
//! accompanying rivet variant-context blob when needed) into a per-test
//! temp file, and shells out to the `spar` binary. Tests assert exit
//! codes, parse the JSON output to verify the variant-aware report
//! shape, and exercise both the explicit (`--variant-context PATH`) and
//! implicit (`--variant NAME`) forms of the contract's CLI.
//!
//! Test inventory (8 cases per the commit-6 spec):
//!
//!  1. `verify_with_variant_filters_components`
//!  2. `verify_unknown_component_in_variant_errors`
//!  3. `enumerate_with_variant_filters_targets`
//!  4. `verify_explicit_context_file_and_stdin`
//!  5. `verify_implicit_variant_shells_out_to_rivet`
//!  6. `verify_no_rivet_on_path_clear_error`
//!  7. `verify_unknown_version_blob_rejected`
//!  8. `verify_output_includes_variant_metadata`

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// Per-test temp file: process id + per-test tag, to avoid races between
/// parallel test runners on the same machine.
fn write_file(prefix: &str, tag: &str, ext: &str, body: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_moves_variant_{}_{}_{}.{}",
        prefix,
        std::process::id(),
        tag,
        ext,
    ));
    fs::write(&path, body).expect("write temp file");
    path
}

fn cleanup(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

/// Two-cpu-two-thread model split across two declared classifier sources.
/// In a real rivet project the artifact split would be at the file
/// boundary; here we use a single AADL file and rely on symbol bindings
/// instead — keeping the test fixture lean while still exercising the
/// keep_in_variant pipeline end-to-end.
const PETROL_DIESEL_MODEL: &str = "\
package Engines
public
  processor CPU
  end CPU;

  thread Petrol
    properties
      Spar_Migration::Mobile => true;
  end Petrol;

  thread Diesel
    properties
      Spar_Migration::Mobile => true;
  end Diesel;

  process Engine
  end Engine;

  process implementation Engine.Impl
    subcomponents
      petrol: thread Petrol;
      diesel: thread Diesel;
  end Engine.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu1: processor CPU;
      cpu2: processor CPU;
      eng: process Engine.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu1)) applies to eng.petrol;
      Actual_Processor_Binding => (reference (cpu1)) applies to eng.diesel;
  end Sys.Impl;
end Engines;
";

/// Build a minimal v1 variant-context blob for the petrol-only variant.
///
/// The binding gates `Engines::Sys.Impl.eng.diesel` (the FQN spar's
/// instance-path adapter reports for the diesel thread instance) on
/// the `engine_diesel` feature. The variant declares only
/// `engine_petrol` as active, so the diesel instance is dropped from
/// the analysis surface while the petrol instance is kept.
fn petrol_only_blob() -> &'static str {
    r#"{
        "rivet_spar_context_version": "1",
        "variant": "petrol_only",
        "features": ["engine_petrol"],
        "bindings": [
            { "symbol": "Engines::Sys.Impl.eng.diesel", "requires": ["engine_diesel"] }
        ],
        "feature_model_hash": "sha256:petrol",
        "resolved_at": "2026-04-23T12:00:00Z",
        "generated_by": "spar test harness"
    }"#
}

/// Build a v2 (unknown) blob for the strict-version-rejection test.
fn unknown_version_blob() -> &'static str {
    r#"{
        "rivet_spar_context_version": "2",
        "variant": "future",
        "features": [],
        "bindings": [],
        "feature_model_hash": "sha256:0",
        "resolved_at": "2026-04-23T12:00:00Z",
        "generated_by": "future-emitter"
    }"#
}

// ── 1. verify_with_variant_filters_components ─────────────────────────

#[test]
fn verify_with_variant_filters_components() {
    // Petrol-only variant: the petrol thread is in the variant, so
    // moving it to cpu2 succeeds; the move should pass without the
    // diesel thread polluting the analysis surface.
    let model = write_file("filter", "model", "aadl", PETROL_DIESEL_MODEL);
    let ctx = write_file("filter", "ctx", "json", petrol_only_blob());

    let out = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--format",
            "json",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected ok exit, stdout: {stdout}\nstderr: {stderr}",
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert_eq!(v["variant"].as_str(), Some("petrol_only"));
    cleanup(&model);
    cleanup(&ctx);
}

// ── 2. verify_unknown_component_in_variant_errors ─────────────────────

#[test]
fn verify_unknown_component_in_variant_errors() {
    // Diesel thread is dropped by the petrol variant — pointing
    // --component at it must produce a clear "not part of variant"
    // diagnostic, not a "no such component" one.
    let model = write_file("notinvariant", "model", "aadl", PETROL_DIESEL_MODEL);
    let ctx = write_file("notinvariant", "ctx", "json", petrol_only_blob());

    let out = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "diesel",
            "--to",
            "cpu2",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.code() != Some(0),
        "expected non-zero exit when --component is dropped by variant",
    );
    assert!(
        stderr.contains("not part of variant") && stderr.contains("petrol_only"),
        "stderr should explain the dropped-by-variant case; got: {stderr}",
    );
    cleanup(&model);
    cleanup(&ctx);
}

// ── 3. enumerate_with_variant_filters_targets ─────────────────────────

#[test]
fn enumerate_with_variant_filters_targets() {
    // Build a model where the second processor is gated on a feature
    // not present in the variant, so the variant filter must drop cpu2
    // from the candidate list.
    let model_src = "\
package Var
public
  processor CPU
  end CPU;

  thread Worker
    properties
      Spar_Migration::Mobile => true;
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
end Var;
";
    // The variant only activates `cpu_a`; the symbol binding gates the
    // entire `Var::Sys.Impl.cpu2` instance on a missing feature so the
    // filter drops it.
    let blob = r#"{
        "rivet_spar_context_version": "1",
        "variant": "single_cpu",
        "features": ["cpu_a"],
        "bindings": [
            { "symbol": "Var::Sys.Impl.cpu2", "requires": ["cpu_b"] }
        ],
        "feature_model_hash": "sha256:single",
        "resolved_at": "2026-04-23T12:00:00Z",
        "generated_by": "spar test harness"
    }"#;
    let model = write_file("enumvar", "model", "aadl", model_src);
    let ctx = write_file("enumvar", "ctx", "json", blob);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Var::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates array");
    // Only cpu1 should remain after the variant filter.
    let targets: Vec<String> = candidates
        .iter()
        .map(|c| c["target"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        targets.iter().any(|t| t.ends_with("cpu1")),
        "expected cpu1 in candidate list; got {targets:?}",
    );
    assert!(
        !targets.iter().any(|t| t.ends_with("cpu2")),
        "expected cpu2 dropped by variant; got {targets:?}",
    );
    assert_eq!(v["variant"].as_str(), Some("single_cpu"));
    cleanup(&model);
    cleanup(&ctx);
}

// ── 4. verify_explicit_context_file_and_stdin ─────────────────────────

#[test]
fn verify_explicit_context_file_and_stdin() {
    let model = write_file("explicit", "model", "aadl", PETROL_DIESEL_MODEL);
    let ctx = write_file("explicit", "ctx", "json", petrol_only_blob());

    // (a) Explicit file path.
    let out = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--format",
            "json",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("file form must produce JSON");
    assert_eq!(v["variant"].as_str(), Some("petrol_only"));

    // (b) Stdin (`-`).
    let mut child = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--format",
            "json",
            "--variant-context",
            "-",
        ])
        .arg(&model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spar");
    {
        let stdin = child.stdin.as_mut().expect("stdin handle");
        stdin
            .write_all(petrol_only_blob().as_bytes())
            .expect("write stdin");
    }
    let out2 = child.wait_with_output().expect("wait spar");
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    let v2: serde_json::Value =
        serde_json::from_str(&stdout2).expect("stdin form must produce JSON");
    assert_eq!(v2["variant"].as_str(), Some("petrol_only"));

    cleanup(&model);
    cleanup(&ctx);
}

// ── 5. verify_implicit_variant_shells_out_to_rivet ────────────────────

#[test]
fn verify_implicit_variant_shells_out_to_rivet() {
    // The implicit form normally invokes `rivet resolve --variant
    // <NAME> --format spar-context-json`. To exercise that code path
    // without a real rivet binary on the test runner, the moves
    // pipeline honours an `SPAR_VARIANT_TEST_RIVET_OUTPUT` env-var that
    // short-circuits the shell-out and uses the variable's value as
    // the JSON payload directly. Production builds never set this
    // variable, so the seam is invisible to end users.
    let model = write_file("implicit", "model", "aadl", PETROL_DIESEL_MODEL);

    let out = spar()
        .env("SPAR_VARIANT_TEST_RIVET_OUTPUT", petrol_only_blob())
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--format",
            "json",
            "--variant",
            "petrol_only",
        ])
        .arg(&model)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("implicit form must produce JSON");
    assert_eq!(v["variant"].as_str(), Some("petrol_only"));
    assert_eq!(v["ok"].as_bool(), Some(true));

    cleanup(&model);
}

// ── 6. verify_no_rivet_on_path_clear_error ────────────────────────────

#[test]
fn verify_no_rivet_on_path_clear_error() {
    // Force rivet-not-found by pointing PATH at an empty directory and
    // unsetting RIVET_BIN. The implicit `--variant` form should then
    // emit the documented diagnostic with a pointer back to the
    // explicit form.
    let model = write_file("norivet", "model", "aadl", PETROL_DIESEL_MODEL);
    let empty_dir = env::temp_dir().join(format!("spar_norivet_{}", std::process::id()));
    let _ = fs::create_dir_all(&empty_dir);

    let out = spar()
        .env_remove("SPAR_VARIANT_TEST_RIVET_OUTPUT")
        .env_remove("RIVET_BIN")
        .env("PATH", &empty_dir)
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--variant",
            "petrol_only",
        ])
        .arg(&model)
        .output()
        .expect("failed to run spar");

    assert!(
        out.status.code() != Some(0),
        "expected non-zero exit when rivet is unreachable",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("rivet") && stderr.contains("--variant-context"),
        "stderr should mention rivet and point to the explicit form; got: {stderr}",
    );

    let _ = fs::remove_dir_all(&empty_dir);
    cleanup(&model);
}

// ── 7. verify_unknown_version_blob_rejected ───────────────────────────

#[test]
fn verify_unknown_version_blob_rejected() {
    // v1 readers must refuse v2 (or any non-"1") blobs per the
    // contract's compatibility section.
    let model = write_file("badver", "model", "aadl", PETROL_DIESEL_MODEL);
    let ctx = write_file("badver", "ctx", "json", unknown_version_blob());

    let out = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");

    assert!(
        out.status.code() != Some(0),
        "expected non-zero exit on unknown version",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("rivet_spar_context_version") || stderr.contains("v1 only"),
        "stderr should mention the version mismatch; got: {stderr}",
    );

    cleanup(&model);
    cleanup(&ctx);
}

// ── 8. verify_output_includes_variant_metadata ────────────────────────

#[test]
fn verify_output_includes_variant_metadata() {
    // Top-level JSON must include `variant` and `feature_model_hash`
    // when a variant context is active; both fields are part of the
    // audit trail consumed by MCP / rivet downstream.
    let model = write_file("meta", "model", "aadl", PETROL_DIESEL_MODEL);
    let ctx = write_file("meta", "ctx", "json", petrol_only_blob());

    let out = spar()
        .args([
            "moves",
            "verify",
            "--root",
            "Engines::Sys.Impl",
            "--component",
            "petrol",
            "--to",
            "cpu2",
            "--format",
            "json",
            "--variant-context",
        ])
        .arg(&ctx)
        .arg(&model)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["variant"].as_str(), Some("petrol_only"));
    assert_eq!(
        v["feature_model_hash"].as_str(),
        Some("sha256:petrol"),
        "expected feature_model_hash in output; got {v}",
    );
    cleanup(&model);
    cleanup(&ctx);
}
