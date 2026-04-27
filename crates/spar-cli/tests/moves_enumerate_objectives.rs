//! Integration tests for `spar moves enumerate --objective <mode>`
//! (Track E commit 5/8).
//!
//! Each test builds a small inline AADL model, drops it into a per-test
//! temp file (mirroring the `moves_enumerate.rs` pattern), and shells
//! out to the `spar` binary. Tests assert exit codes, parse JSON / text
//! output, and verify the multi-objective ranking shape.
//!
//! Test inventory (10 cases per the commit-5 spec):
//!
//!  1. `default_objective_is_max_response`
//!  2. `total_load_objective_picks_least_loaded_cpu`
//!  3. `total_power_objective_reads_spar_power`
//!  4. `total_weight_objective_reads_weight_property`
//!  5. `balanced_objective_aggregates_all_four`
//!  6. `unknown_objective_errors_clearly`
//!  7. `objective_doesnt_change_validity`
//!  8. `score_zero_when_no_objective_data`
//!  9. `negative_max_response_indicates_deadline_miss`
//! 10. `enumerate_text_format_includes_score_column`

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

fn write_model(tag: &str, body: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_moves_enumerate_objectives_{}_{}.aadl",
        std::process::id(),
        tag,
    ));
    fs::write(&path, body).expect("write temp AADL");
    path
}

fn cleanup(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

/// Two-CPU baseline: minimal model used wherever the test is asserting
/// orthogonal behaviour and the actual analysis numbers don't matter.
const BASELINE_MODEL: &str = "\
package Obj
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
end Obj;
";

/// Two CPUs with very different power budgets attached.
///
/// Test 3 asserts that under `--objective total-power` the candidate
/// with the lower-power CPU sorts ahead of the one with the
/// higher-power CPU. Spar_Power::Power_Budget is read directly from
/// the candidate processor.
const POWER_MODEL: &str = "\
package PowerObj
public
  processor LowPowerCPU
    properties
      Spar_Power::Power_Budget => 100 ms;
  end LowPowerCPU;

  processor HighPowerCPU
    properties
      Spar_Power::Power_Budget => 5000 ms;
  end HighPowerCPU;

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
      cpu_low:  processor LowPowerCPU;
      cpu_high: processor HighPowerCPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu_low)) applies to app.t1;
  end Sys.Impl;
end PowerObj;
";

/// Two CPUs with very different declared weights.
const WEIGHT_MODEL: &str = "\
package WeightObj
public
  processor LightCPU
    properties
      Weight_Properties::Weight => 1 ms;
  end LightCPU;

  processor HeavyCPU
    properties
      Weight_Properties::Weight => 1000 ms;
  end HeavyCPU;

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
      cpu_light: processor LightCPU;
      cpu_heavy: processor HeavyCPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu_light)) applies to app.t1;
  end Sys.Impl;
end WeightObj;
";

/// Two CPUs with one already loaded by an existing thread (`busy`)
/// while the other (`empty`) is unbound.
///
/// Under `--objective total-load`, ranking the candidate that lands
/// `t1` on `cpu_busy` must score higher (worse) than landing on
/// `cpu_empty` because cpu_busy already has a thread costing wcet/period
/// = 0.5 of utilisation under the (un-overlayed) declared binding.
const LOAD_MODEL: &str = "\
package LoadObj
public
  processor CPU
  end CPU;

  thread Worker
    properties
      Dispatch_Protocol => Periodic;
      Period => 100 ms;
      Compute_Execution_Time => 50 ms .. 50 ms;
      Deadline => 100 ms;
  end Worker;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      t1: thread Worker;
      t_existing: thread Worker;
  end Proc.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu_empty: processor CPU;
      cpu_busy:  processor CPU;
      app: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu_empty)) applies to app.t1;
      Actual_Processor_Binding => (reference (cpu_busy))  applies to app.t_existing;
  end Sys.Impl;
end LoadObj;
";

/// Same shape as `LOAD_MODEL` but with execution times that *miss*
/// the deadline on every candidate, exercising the deadline-miss path.
const DEADLINE_MISS_MODEL: &str = "\
package MissObj
public
  processor CPU
    properties
      Scheduling_Protocol => (RMS);
  end CPU;

  thread Worker
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 50 ms .. 50 ms;
      Deadline => 10 ms;
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
end MissObj;
";

fn run_enumerate(model_path: &PathBuf, root: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = spar();
    cmd.args(["moves", "enumerate", "--root", root, "--component", "t1"]);
    cmd.args(args);
    cmd.arg(model_path);
    cmd.output().expect("failed to run spar")
}

// ── 1. default_objective_is_max_response ─────────────────────────────

#[test]
fn default_objective_is_max_response() {
    // Without --objective, the score must reflect the
    // max-response single-objective metric. With BASELINE_MODEL
    // (no period / wcet) the max_response_ns should be None and
    // hence score = 0 for every candidate. We verify the JSON shape
    // carries `rank.max_response_ns` and `rank.score` keys.
    let path = write_model("default_obj", BASELINE_MODEL);
    let out = run_enumerate(&path, "Obj::Sys.Impl", &["--format", "json"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout was not valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty(), "expected at least one candidate");
    for c in candidates {
        let rank = &c["rank"];
        assert!(
            rank.is_object(),
            "rank must be an object on every candidate"
        );
        assert!(rank["score"].is_number(), "rank.score must be a number");
        // max_response_ns is null or i64 (None or a value)
        assert!(
            rank["max_response_ns"].is_null() || rank["max_response_ns"].is_number(),
            "rank.max_response_ns must be null or number; got {rank}"
        );
    }
    cleanup(&path);
}

// ── 2. total_load_objective_picks_least_loaded_cpu ───────────────────

#[test]
fn total_load_objective_picks_least_loaded_cpu() {
    // Two CPUs: cpu_empty has no other thread; cpu_busy carries
    // t_existing. Under --objective total-load, the candidate moving
    // t1 to cpu_empty must rank ahead of (lower score than) the one
    // moving t1 to cpu_busy.
    let path = write_model("total_load", LOAD_MODEL);
    let out = run_enumerate(
        &path,
        "LoadObj::Sys.Impl",
        &["--objective", "total-load", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");

    // Find the cpu_empty and cpu_busy entries.
    let by_target = |needle: &str| {
        candidates
            .iter()
            .find(|c| c["target"].as_str().unwrap_or("").ends_with(needle))
            .unwrap_or_else(|| panic!("no candidate matched '{needle}': {candidates:?}"))
    };
    let empty = by_target("cpu_empty");
    let busy = by_target("cpu_busy");

    let s_empty = empty["rank"]["score"].as_f64().unwrap();
    let s_busy = busy["rank"]["score"].as_f64().unwrap();

    assert!(
        s_empty <= s_busy,
        "expected cpu_empty score ({s_empty}) <= cpu_busy score ({s_busy})",
    );

    // Sorting should also reflect this: first ok=true candidate is cpu_empty.
    let first_ok = candidates
        .iter()
        .find(|c| c["ok"].as_bool() == Some(true))
        .expect("at least one ok candidate");
    assert!(
        first_ok["target"].as_str().unwrap().ends_with("cpu_empty"),
        "expected the lowest-load candidate to sort first; got {}",
        first_ok["target"],
    );
    cleanup(&path);
}

// ── 3. total_power_objective_reads_spar_power ────────────────────────

#[test]
fn total_power_objective_reads_spar_power() {
    let path = write_model("total_power", POWER_MODEL);
    let out = run_enumerate(
        &path,
        "PowerObj::Sys.Impl",
        &["--objective", "total-power", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");

    let by_target = |needle: &str| {
        candidates
            .iter()
            .find(|c| c["target"].as_str().unwrap_or("").ends_with(needle))
            .unwrap_or_else(|| panic!("no candidate matched '{needle}'"))
    };
    let low = by_target("cpu_low");
    let high = by_target("cpu_high");

    // Both candidates must have a populated total_power_mw value
    // (read from Spar_Power::Power_Budget on the target processor).
    let low_power = low["rank"]["total_power_mw"].as_u64();
    let high_power = high["rank"]["total_power_mw"].as_u64();
    assert!(
        low_power.is_some() && high_power.is_some(),
        "expected total_power_mw on both candidates; got {low_power:?} / {high_power:?}",
    );
    assert!(
        low_power.unwrap() < high_power.unwrap(),
        "low-power CPU must report less power; got {low_power:?} vs {high_power:?}",
    );

    let s_low = low["rank"]["score"].as_f64().unwrap();
    let s_high = high["rank"]["score"].as_f64().unwrap();
    assert!(
        s_low < s_high,
        "expected cpu_low score ({s_low}) < cpu_high score ({s_high})",
    );
    cleanup(&path);
}

// ── 4. total_weight_objective_reads_weight_property ──────────────────

#[test]
fn total_weight_objective_reads_weight_property() {
    let path = write_model("total_weight", WEIGHT_MODEL);
    let out = run_enumerate(
        &path,
        "WeightObj::Sys.Impl",
        &["--objective", "total-weight", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");

    let by_target = |needle: &str| {
        candidates
            .iter()
            .find(|c| c["target"].as_str().unwrap_or("").ends_with(needle))
            .unwrap_or_else(|| panic!("no candidate matched '{needle}'"))
    };
    let light = by_target("cpu_light");
    let heavy = by_target("cpu_heavy");

    let light_w = light["rank"]["total_weight_g"].as_u64();
    let heavy_w = heavy["rank"]["total_weight_g"].as_u64();
    assert!(
        light_w.is_some() && heavy_w.is_some(),
        "expected total_weight_g on both candidates; got {light_w:?} / {heavy_w:?}",
    );
    assert!(
        light_w.unwrap() < heavy_w.unwrap(),
        "light CPU must report less weight; got {light_w:?} vs {heavy_w:?}",
    );

    let s_light = light["rank"]["score"].as_f64().unwrap();
    let s_heavy = heavy["rank"]["score"].as_f64().unwrap();
    assert!(
        s_light < s_heavy,
        "expected cpu_light score ({s_light}) < cpu_heavy score ({s_heavy})",
    );
    cleanup(&path);
}

// ── 5. balanced_objective_aggregates_all_four ────────────────────────

#[test]
fn balanced_objective_aggregates_all_four() {
    // Use POWER_MODEL: with --objective balanced, the score still
    // differentiates the two candidates because total-power varies
    // across targets, but each axis carries 1/4 of the total weight.
    // Verify (a) score is non-zero on at least one candidate, and
    // (b) the cpu_high candidate scores higher than cpu_low.
    let path = write_model("balanced", POWER_MODEL);
    let out = run_enumerate(
        &path,
        "PowerObj::Sys.Impl",
        &["--objective", "balanced", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");

    let by_target = |needle: &str| {
        candidates
            .iter()
            .find(|c| c["target"].as_str().unwrap_or("").ends_with(needle))
            .unwrap_or_else(|| panic!("no candidate matched '{needle}'"))
    };
    let low = by_target("cpu_low");
    let high = by_target("cpu_high");

    let s_low = low["rank"]["score"].as_f64().unwrap();
    let s_high = high["rank"]["score"].as_f64().unwrap();
    assert!(
        s_high > s_low,
        "balanced score should still penalise high-power candidate; got low={s_low} high={s_high}",
    );
    // Score must be > 0 since power values are non-zero.
    assert!(
        s_high > 0.0,
        "expected non-zero balanced score on the high-power candidate; got {s_high}",
    );
    cleanup(&path);
}

// ── 6. unknown_objective_errors_clearly ──────────────────────────────

#[test]
fn unknown_objective_errors_clearly() {
    let path = write_model("unknown_obj", BASELINE_MODEL);
    let out = run_enumerate(
        &path,
        "Obj::Sys.Impl",
        &["--objective", "not-a-real-mode", "--format", "json"],
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.code() != Some(0),
        "expected non-zero exit on bad --objective"
    );
    assert!(
        stderr.contains("not-a-real-mode") || stderr.contains("--objective"),
        "stderr should mention the offending objective or --objective; got: {stderr}",
    );
    cleanup(&path);
}

// ── 7. objective_doesnt_change_validity ──────────────────────────────

#[test]
fn objective_doesnt_change_validity() {
    // For the same model, the set of `ok=true` candidates must be
    // identical regardless of which objective drives the ranking.
    // Only the order changes.
    let path = write_model("validity", POWER_MODEL);

    let valid_set = |args: &[&str]| -> std::collections::BTreeSet<String> {
        let out = run_enumerate(&path, "PowerObj::Sys.Impl", args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        v["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .filter(|c| c["ok"].as_bool() == Some(true))
            .map(|c| c["target"].as_str().unwrap_or("").to_string())
            .collect()
    };

    let s_default = valid_set(&["--format", "json"]);
    let s_load = valid_set(&["--objective", "total-load", "--format", "json"]);
    let s_power = valid_set(&["--objective", "total-power", "--format", "json"]);
    let s_balanced = valid_set(&["--objective", "balanced", "--format", "json"]);

    assert_eq!(s_default, s_load, "objective change altered validity set");
    assert_eq!(s_default, s_power, "objective change altered validity set");
    assert_eq!(
        s_default, s_balanced,
        "objective change altered validity set"
    );
    cleanup(&path);
}

// ── 8. score_zero_when_no_objective_data ─────────────────────────────

#[test]
fn score_zero_when_no_objective_data() {
    // BASELINE_MODEL has no Period / Compute_Execution_Time, no
    // Spar_Power, and no Weight properties on either CPU. Under
    // any objective other than balanced, the score collapses to 0
    // for every candidate.
    let path = write_model("no_data", BASELINE_MODEL);
    let out = run_enumerate(
        &path,
        "Obj::Sys.Impl",
        &["--objective", "total-power", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty());
    for c in candidates {
        let s = c["rank"]["score"].as_f64().unwrap();
        assert_eq!(
            s, 0.0,
            "expected score=0 on every candidate when no data is available; got {s}",
        );
    }
    cleanup(&path);
}

// ── 9. negative_max_response_indicates_deadline_miss ─────────────────

#[test]
fn negative_max_response_indicates_deadline_miss() {
    // The deadline-miss model produces an RTA Error on every
    // candidate. The corresponding rank.max_response_ns must be
    // negative (sentinel) on those candidates and the score must
    // reflect the inflated contribution (10.0 under
    // --objective max-response).
    let path = write_model("miss", DEADLINE_MISS_MODEL);
    let out = run_enumerate(
        &path,
        "MissObj::Sys.Impl",
        &["--objective", "max-response", "--format", "json"],
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let candidates = v["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty());
    let mut had_miss = false;
    for c in candidates {
        let mrs = c["rank"]["max_response_ns"].as_i64();
        if let Some(n) = mrs
            && n < 0
        {
            had_miss = true;
            let s = c["rank"]["score"].as_f64().unwrap();
            assert!(
                s >= 9.99,
                "deadline-miss candidate score should be inflated; got {s} for target {}",
                c["target"],
            );
        }
    }
    assert!(
        had_miss,
        "expected at least one candidate to have a deadline-miss sentinel",
    );

    // Such candidates must rank *last* — i.e., the last candidate's
    // score is at least the first candidate's score.
    let first_score = candidates[0]["rank"]["score"].as_f64().unwrap();
    let last_score = candidates[candidates.len() - 1]["rank"]["score"]
        .as_f64()
        .unwrap();
    assert!(
        last_score >= first_score,
        "deadline-miss candidate must not sort ahead of an admissible candidate; first={first_score}, last={last_score}",
    );
    cleanup(&path);
}

// ── 10. enumerate_text_format_includes_score_column ──────────────────

#[test]
fn enumerate_text_format_includes_score_column() {
    let path = write_model("text_score", BASELINE_MODEL);
    let out = run_enumerate(&path, "Obj::Sys.Impl", &["--format", "text"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    // The header row must mention "score" (not "slack"). The first
    // header line begins with "  ok" and ends with "violations" per
    // render_enumerate_text.
    let header = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("ok") && l.contains("target"))
        .unwrap_or_else(|| panic!("no enumerate-table header found in output:\n{stdout}"));
    assert!(
        header.contains("score"),
        "expected `score` column header; got: {header:?}\nfull stdout: {stdout}"
    );
    assert!(
        !header.contains("slack"),
        "text format should no longer carry a `slack` column; got: {header:?}",
    );
    cleanup(&path);
}
