//! Integration tests for `spar moves enumerate` (Track E commit 4/8).
//!
//! Each test builds a small inline AADL model, drops it into a per-test
//! temp file (mirroring the `moves_verify.rs` pattern), and shells out
//! to the `spar` binary. Tests assert exit codes, parse JSON / text
//! output, and verify the candidate list shape.
//!
//! Test inventory (10 cases per the commit-4 spec):
//!
//!  1. `enumerate_lists_all_processors_when_no_allowed_targets`
//!  2. `enumerate_respects_allowed_targets`
//!  3. `enumerate_marks_frozen_target_as_invalid`
//!  4. `enumerate_marks_unallowed_target_as_invalid`
//!  5. `enumerate_propagates_analysis_diagnostics`
//!  6. `enumerate_target_filter_narrows_candidates`
//!  7. `enumerate_text_format_includes_summary_line`
//!  8. `enumerate_json_format_parseable`
//!  9. `enumerate_unknown_component_errors`
//! 10. `enumerate_doesnt_mutate_underlying_model`

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// Per-test temp file: process id + per-test tag, to avoid races between
/// parallel test runners on the same machine.
fn write_model(tag: &str, body: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_moves_enumerate_{}_{}.aadl",
        std::process::id(),
        tag,
    ));
    fs::write(&path, body).expect("write temp AADL");
    path
}

fn cleanup(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

/// Two-processor model: thread t1 with no Allowed_Targets restriction.
const TWO_CPU_MODEL: &str = "\
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

/// Three-processor model with Allowed_Targets => (cpu1, cpu2). cpu3
/// exists in the instance but is *not* in the allowed list, so
/// enumerate must skip it.
const ALLOWED_TWO_OF_THREE: &str = "\
package Allowed
public
  processor CPU
  end CPU;

  thread Worker
    properties
      Spar_Migration::Mobile => true;
      Spar_Migration::Allowed_Targets => (reference (cpu1), reference (cpu2));
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
      cpu3: processor CPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu1)) applies to app.t1;
  end Sys.Impl;
end Allowed;
";

/// Frozen-thread model with two processors. Every move attempt must
/// raise a Frozen violation.
const FROZEN_MODEL: &str = "\
package Frozen
public
  processor CPU
  end CPU;

  thread Plat
    properties
      Spar_Migration::Frozen => true;
      Spar_Migration::Pinned_Reason => \"ASIL-D platform partition\";
  end Plat;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      t1: thread Plat;
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
end Frozen;
";

/// Periodic-overrun model: the thread asks for far more compute than
/// any processor can supply, so RTA produces an error-severity
/// diagnostic for every candidate target. Used to verify that
/// enumerate surfaces analysis diagnostics into per-candidate counts.
const RTA_FAIL_MODEL: &str = "\
package RtaFail
public
  processor CPU
    properties
      Scheduling_Protocol => (RMS);
  end CPU;

  thread T
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 50 ms .. 50 ms;
      Deadline => 10 ms;
  end T;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      t1: thread T;
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
end RtaFail;
";

/// Three-processor model (cpu_x86_a, cpu_x86_b, cpu_arm) used for the
/// `--target-filter` test. No Allowed_Targets restriction → all three
/// would be candidates without a filter.
const FILTER_MODEL: &str = "\
package Filter
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
      cpu_x86_a: processor CPU;
      cpu_x86_b: processor CPU;
      cpu_arm:   processor CPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu_x86_a)) applies to app.t1;
  end Sys.Impl;
end Filter;
";

// ── 1. enumerate_lists_all_processors_when_no_allowed_targets ─────────

#[test]
fn enumerate_lists_all_processors_when_no_allowed_targets() {
    // Two processors, no Allowed_Targets restriction: the candidate
    // list must include cpu1 and cpu2.
    let path = write_model("two_cpu_no_allowed", TWO_CPU_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Migrate::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected ok exit, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code(),
    );

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");
    let candidates = v["candidates"]
        .as_array()
        .expect("candidates must be an array");
    let target_names: Vec<String> = candidates
        .iter()
        .filter_map(|c| c["target"].as_str().map(String::from))
        .collect();
    assert!(
        target_names.iter().any(|t| t.ends_with("cpu1")),
        "expected cpu1 in candidates; got {target_names:?}",
    );
    assert!(
        target_names.iter().any(|t| t.ends_with("cpu2")),
        "expected cpu2 in candidates; got {target_names:?}",
    );
    assert_eq!(v["total"].as_u64(), Some(2));
    cleanup(&path);
}

// ── 2. enumerate_respects_allowed_targets ─────────────────────────────

#[test]
fn enumerate_respects_allowed_targets() {
    // Allowed_Targets => (cpu1, cpu2): cpu3 must be absent from the
    // candidate list even though it exists in the model.
    let path = write_model("allowed_two_of_three", ALLOWED_TWO_OF_THREE);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Allowed::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0; stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    let names: Vec<String> = candidates
        .iter()
        .filter_map(|c| c["target"].as_str().map(String::from))
        .collect();
    assert_eq!(
        candidates.len(),
        2,
        "expected exactly two candidates from Allowed_Targets, got {names:?}",
    );
    assert!(names.iter().any(|n| n.ends_with("cpu1")));
    assert!(names.iter().any(|n| n.ends_with("cpu2")));
    assert!(
        names.iter().all(|n| !n.ends_with("cpu3")),
        "cpu3 is not in Allowed_Targets but appeared in candidates: {names:?}",
    );
    cleanup(&path);
}

// ── 3. enumerate_marks_frozen_target_as_invalid ───────────────────────

#[test]
fn enumerate_marks_frozen_target_as_invalid() {
    // For a Frozen thread, every candidate target produces a Frozen
    // overlay violation → all candidates have ok=false. The total
    // and valid counts must reflect that.
    let path = write_model("frozen_invalid", FROZEN_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Frozen::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");

    let candidates = v["candidates"].as_array().expect("candidates");
    assert!(
        !candidates.is_empty(),
        "expected at least one candidate target",
    );
    for c in candidates {
        assert_eq!(
            c["ok"].as_bool(),
            Some(false),
            "every Frozen-thread candidate must be ok=false; got {c}",
        );
        let viols = c["violations"].as_array().expect("violations");
        assert!(
            viols.iter().any(|v| v["kind"] == "Frozen"),
            "expected Frozen violation on candidate {}; got {viols:?}",
            c["target"],
        );
    }
    assert_eq!(v["valid"].as_u64(), Some(0));
    // Exit 1: no admissible candidate.
    assert_eq!(out.status.code(), Some(1));
    cleanup(&path);
}

// ── 4. enumerate_marks_unallowed_target_as_invalid ────────────────────

#[test]
fn enumerate_marks_unallowed_target_as_invalid() {
    // When `--target-filter` is supplied with a name that *does* match
    // a processor but the component restricts allowed targets, the
    // intersection still respects Allowed_Targets — i.e. the filter
    // narrows the already-allowed set; it does not bypass the
    // platform/application split. With ALLOWED_TWO_OF_THREE we filter
    // for "cpu3" and must get zero candidates because cpu3 is not in
    // Allowed_Targets.
    let path = write_model("filter_outside_allowed", ALLOWED_TWO_OF_THREE);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Allowed::Sys.Impl",
            "--component",
            "t1",
            "--target-filter",
            "cpu3",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    assert_eq!(
        candidates.len(),
        0,
        "filter on cpu3 must yield no candidates given Allowed_Targets => (cpu1, cpu2); got {candidates:?}",
    );
    assert_eq!(v["total"].as_u64(), Some(0));
    assert_eq!(v["valid"].as_u64(), Some(0));
    // Zero admissible candidates → exit 1.
    assert_eq!(out.status.code(), Some(1));
    cleanup(&path);
}

// ── 5. enumerate_propagates_analysis_diagnostics ──────────────────────

#[test]
fn enumerate_propagates_analysis_diagnostics() {
    // Periodic-overrun model: every candidate produces an
    // error-severity RTA diagnostic. Each candidate must therefore
    // have diagnostics_count > 0 and ok=false.
    let path = write_model("rta_fail", RTA_FAIL_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "RtaFail::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    assert!(
        !candidates.is_empty(),
        "expected at least one candidate (cpu1, cpu2)",
    );
    for c in candidates {
        let diag_count = c["diagnostics_count"].as_u64().unwrap_or(0);
        assert!(
            diag_count > 0,
            "expected diagnostics_count > 0 on RTA-fail candidate {}, got {}",
            c["target"],
            diag_count,
        );
        assert_eq!(
            c["ok"].as_bool(),
            Some(false),
            "RTA-fail candidate {} must be ok=false",
            c["target"],
        );
    }
    cleanup(&path);
}

// ── 6. enumerate_target_filter_narrows_candidates ─────────────────────

#[test]
fn enumerate_target_filter_narrows_candidates() {
    // No --target-filter: 3 processors. With --target-filter cpu_x86:
    // 2 processors. The filter must shrink the set without altering
    // the order or evaluation of each remaining candidate.
    let path = write_model("filter_narrow", FILTER_MODEL);

    let unfiltered = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Filter::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar (no filter)");
    let unfiltered_stdout = String::from_utf8_lossy(&unfiltered.stdout);
    let unfiltered_json: serde_json::Value =
        serde_json::from_str(&unfiltered_stdout).expect("unfiltered JSON");
    assert_eq!(
        unfiltered_json["total"].as_u64(),
        Some(3),
        "expected three candidates without --target-filter; got {unfiltered_json}",
    );

    let filtered = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Filter::Sys.Impl",
            "--component",
            "t1",
            "--target-filter",
            "cpu_x86",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar (filter)");
    let filtered_stdout = String::from_utf8_lossy(&filtered.stdout);
    let filtered_json: serde_json::Value =
        serde_json::from_str(&filtered_stdout).expect("filtered JSON");
    assert_eq!(
        filtered_json["total"].as_u64(),
        Some(2),
        "expected two candidates with --target-filter cpu_x86; got {filtered_json}",
    );

    // And confirm the filtered set is a strict subset.
    let filtered_names: Vec<String> = filtered_json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["target"].as_str().map(String::from))
        .collect();
    assert!(
        filtered_names.iter().all(|n| n.contains("cpu_x86")),
        "every filtered candidate FQN must contain 'cpu_x86'; got {filtered_names:?}",
    );

    cleanup(&path);
}

// ── 7. enumerate_text_format_includes_summary_line ────────────────────

#[test]
fn enumerate_text_format_includes_summary_line() {
    let path = write_model("text_summary", TWO_CPU_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Migrate::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "text",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Summary line of the form `total=2 valid=...`. Last
    // non-empty line.
    let last_line = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_default();
    assert!(
        last_line.starts_with("total=") && last_line.contains(" valid="),
        "expected text output to end with `total=N valid=K`; got last line: {last_line:?}\nfull stdout: {stdout}",
    );
    cleanup(&path);
}

// ── 8. enumerate_json_format_parseable ────────────────────────────────

#[test]
fn enumerate_json_format_parseable() {
    let path = write_model("json_parseable", TWO_CPU_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Migrate::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");

    assert!(
        v["component"].is_string(),
        "component must be string; got {v}"
    );
    assert!(v["total"].is_number(), "total must be number; got {v}");
    assert!(v["valid"].is_number(), "valid must be number; got {v}");
    let candidates = v["candidates"].as_array().expect("candidates is array");
    for c in candidates {
        assert!(c["target"].is_string(), "target must be string; got {c}");
        assert!(c["ok"].is_boolean(), "ok must be boolean; got {c}");
        assert!(
            c["violations"].is_array(),
            "violations must be array; got {c}"
        );
        assert!(
            c["diagnostics_count"].is_number(),
            "diagnostics_count must be number; got {c}",
        );
        // slack_ns is either null or a number.
        assert!(
            c["slack_ns"].is_null() || c["slack_ns"].is_number(),
            "slack_ns must be null or number; got {c}",
        );
    }
    cleanup(&path);
}

// ── 9. enumerate_unknown_component_errors ─────────────────────────────

#[test]
fn enumerate_unknown_component_errors() {
    let path = write_model("unknown_comp", TWO_CPU_MODEL);

    let out = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Migrate::Sys.Impl",
            "--component",
            "definitely_not_a_real_thread",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar");

    assert!(
        out.status.code() != Some(0),
        "expected non-zero exit on unknown component",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("definitely_not_a_real_thread"),
        "stderr should mention the unresolved component name; got: {stderr}",
    );
    cleanup(&path);
}

// ── 10. enumerate_doesnt_mutate_underlying_model ──────────────────────

#[test]
fn enumerate_doesnt_mutate_underlying_model() {
    let path = write_model("nomutate_enum", TWO_CPU_MODEL);

    // Snapshot analyze output before enumerate.
    let before = spar()
        .args(["analyze", "--root", "Migrate::Sys.Impl", "--format", "json"])
        .arg(&path)
        .output()
        .expect("failed to run spar analyze (before)");
    let stdout_before = String::from_utf8_lossy(&before.stdout).to_string();

    // Run enumerate.
    let _ = spar()
        .args([
            "moves",
            "enumerate",
            "--root",
            "Migrate::Sys.Impl",
            "--component",
            "t1",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("failed to run spar moves enumerate");

    // Snapshot analyze output after enumerate.
    let after = spar()
        .args(["analyze", "--root", "Migrate::Sys.Impl", "--format", "json"])
        .arg(&path)
        .output()
        .expect("failed to run spar analyze (after)");
    let stdout_after = String::from_utf8_lossy(&after.stdout).to_string();

    assert_eq!(
        stdout_before, stdout_after,
        "spar analyze output diverged across spar moves enumerate; overlay is no longer non-mutating",
    );

    // And the file on disk must be untouched.
    let on_disk = fs::read_to_string(&path).expect("read AADL");
    assert_eq!(on_disk, TWO_CPU_MODEL, "AADL file was modified on disk");

    cleanup(&path);
}
