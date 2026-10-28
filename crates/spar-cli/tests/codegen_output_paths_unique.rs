//! REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001 (#543) — every file `spar codegen`
//! reports is a distinct file on disk.
//!
//! Two subsystems `left` and `right` each hold a processor `cpu` and a
//! process `app` with a thread `t`, bound by `applies to`. On origin/main the
//! WIT, crate and config outputs were named by process NAME, so both `app`s
//! wrote `wit/app.wit`, `crates/app/*` and `config/app.toml`: the summary said
//! "wrote 16 files" and 9 reached disk, and the generated workspace listed
//! `crates/app` twice (cargo: "duplicate key").
//!
//! The compile half of the oracle (the generated workspace `cargo check`s) is
//! `emitted_two_subsystem_workspace_cargo_checks` in
//! `crates/spar-codegen/tests/lifecycle_bindings.rs`, which the "Codegen
//! compile oracle" CI job runs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const TWO_SUBSYSTEMS: &str = r#"package TwoSub
public
  processor Cpu
  end Cpu;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end W;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread W;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
  end Top.impl;
end TwoSub;
"#;

/// A flat model with one process: no name collides, so its output must be
/// exactly what origin/main wrote.
const SINGLE_PROCESS: &str = r#"package Single
public
  data Reading
    properties
      Data_Size => 4 Bytes;
  end Reading;
  processor Cpu
  end Cpu;
  thread Filter
    features
      raw : in data port Reading;
      clean : out data port Reading;
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 2 ms .. 3 ms;
  end Filter;
  process Sensor
    features
      raw : in data port Reading;
      clean : out data port Reading;
  end Sensor;
  process implementation Sensor.impl
    subcomponents
      filter : thread Filter;
    connections
      c_in : port raw -> filter.raw;
      c_out : port filter.clean -> clean;
  end Sensor.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      sensor_app : process Sensor.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to sensor_app.filter;
  end Top.impl;
end Single;
"#;

/// A fresh scratch directory holding `src` as `m.aadl`.
fn scratch(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-543-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("m.aadl"), src).unwrap();
    dir
}

/// Run `spar codegen` with `args`; returns (stdout, stderr). Asserts success.
fn codegen(dir: &Path, root: &str, out: &Path, args: &[&str]) -> (String, String) {
    let model = dir.join("m.aadl");
    let output = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["codegen", "--root", root, "--output", out.to_str().unwrap()])
        .args(args)
        .arg(model.to_str().unwrap())
        .output()
        .expect("spar binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "codegen {args:?} failed:\n{stderr}"
    );
    (stdout, stderr)
}

/// Every regular file under `root`, as `/`-separated paths relative to it.
fn files_on_disk(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                let parts: Vec<_> = rel.iter().map(|p| p.to_string_lossy()).collect();
                out.insert(parts.join("/"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

/// The N of "wrote N files" in the codegen summary.
fn reported_count(stderr: &str) -> usize {
    let line = stderr
        .lines()
        .find(|l| l.contains("wrote "))
        .unwrap_or_else(|| panic!("no summary line in:\n{stderr}"));
    let after = line.split("wrote ").nth(1).unwrap();
    after.split_whitespace().next().unwrap().parse().unwrap()
}

/// For `args`: the dry-run path list has no duplicate, a real run puts every
/// listed path on disk and nothing else, and the summary's N is that count.
/// Returns the files on disk.
fn assert_paths_unique_and_on_disk(name: &str, args: &[&str]) -> BTreeSet<String> {
    let dir = scratch(name, TWO_SUBSYSTEMS);
    let out = dir.join("out");

    let mut dry_args = args.to_vec();
    dry_args.push("--dry-run");
    let (listed, _) = codegen(&dir, "TwoSub::Top.impl", &out, &dry_args);
    let prefix = format!("{}/", out.display());
    let listed: Vec<String> = listed
        .lines()
        .map(|l| l.strip_prefix(&prefix).unwrap_or(l).to_string())
        .collect();
    let unique: BTreeSet<String> = listed.iter().cloned().collect();
    assert_eq!(
        listed.len(),
        unique.len(),
        "{args:?}: a generated path repeats, so one file overwrites another: {listed:#?}"
    );

    let (_, stderr) = codegen(&dir, "TwoSub::Top.impl", &out, args);
    let on_disk = files_on_disk(&out);
    assert_eq!(
        on_disk, unique,
        "{args:?}: files on disk != files generated"
    );
    assert_eq!(
        reported_count(&stderr),
        on_disk.len(),
        "{args:?}: the summary count is not the number of files on disk:\n{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    on_disk
}

// rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
#[test]
fn two_same_named_processes_each_keep_their_wit_crate_and_config() {
    let on_disk = assert_paths_unique_and_on_disk("both", &["--format", "both"]);
    let expected: BTreeSet<String> = [
        "BUILD.bazel",
        "Cargo.toml",
        "config/left_app.toml",
        "config/right_app.toml",
        "crates/left_app/BUILD.bazel",
        "crates/left_app/Cargo.toml",
        "crates/left_app/src/lib.rs",
        "crates/left_app/src/t.rs",
        "crates/left_app/src/types.rs",
        "crates/right_app/BUILD.bazel",
        "crates/right_app/Cargo.toml",
        "crates/right_app/src/lib.rs",
        "crates/right_app/src/t.rs",
        "crates/right_app/src/types.rs",
        "wit/left-app.wit",
        "wit/right-app.wit",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(on_disk, expected);
}

// rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
#[test]
fn every_generator_keeps_both_processes_apart() {
    // Every generator at once: tests, the build contract, proofs and the
    // rivet design docs are per-process or per-thread too.
    let on_disk =
        assert_paths_unique_and_on_disk("all", &["--format", "both", "--verify", "all", "--rivet"]);
    for side in ["left", "right"] {
        for path in [
            format!("tests/{side}_app/t_test.rs"),
            format!("docs/design/{side}_app.md"),
            format!("verification/{side}_app.yaml"),
            format!("proofs/{side}__cpu_scheduling.lean"),
        ] {
            assert!(on_disk.contains(&path), "missing {path}: {on_disk:#?}");
        }
    }
}

// rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
#[test]
fn a_single_process_model_is_byte_identical_to_origin_main() {
    // The golden file is the output of the origin/main binary (b2225ea) for
    // this model with `--format both --verify all --rivet`, every file but
    // `proofs/` (whose header #510 changed on purpose). A model with no name
    // collision keeps its plain names, so nothing else may change.
    let dir = scratch("flat", SINGLE_PROCESS);
    let out = dir.join("out");
    codegen(
        &dir,
        "Single::Top.impl",
        &out,
        &["--format", "both", "--verify", "all", "--rivet"],
    );
    let mut got = Vec::new();
    for path in files_on_disk(&out) {
        if path.starts_with("proofs/") {
            continue;
        }
        let data = std::fs::read(out.join(&path)).unwrap();
        got.extend_from_slice(format!("==> {path} ({} bytes) <==\n", data.len()).as_bytes());
        got.extend_from_slice(&data);
        got.push(b'\n');
    }
    let want = include_bytes!("fixtures/codegen_flat_single_process.golden");
    assert!(
        got == want,
        "flat single-process output changed from origin/main:\n--- got ---\n{}",
        String::from_utf8_lossy(&got)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
#[test]
fn a_duplicate_output_path_fails_the_command_naming_both_sources() {
    // Identifiers are truncated to 64 characters, so two threads whose names
    // share their first 64 characters map to one module file: a collision no
    // naming table removes. On origin/main codegen exited 0 and wrote one
    // file for both; now it exits non-zero, names both threads, and writes
    // nothing (generation fails before any file is written).
    let long = "a".repeat(64);
    let src = format!(
        "package Trunc\npublic\n  thread T\n  end T;\n  process P\n  end P;\n\
         process implementation P.impl\n    subcomponents\n      {long}x : thread T;\n\
         {long}y : thread T;\n  end P.impl;\n  system Top\n  end Top;\n\
         system implementation Top.impl\n    subcomponents\n      app : process P.impl;\n\
         end Top.impl;\nend Trunc;\n"
    );
    let dir = scratch("dup", &src);
    let out = dir.join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["codegen", "--root", "Trunc::Top.impl", "--format", "rust"])
        .args(["--output", out.to_str().unwrap()])
        .arg(dir.join("m.aadl"))
        .output()
        .expect("spar binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "must fail:\n{stderr}");
    assert!(
        stderr.contains(&format!("`crates/app/src/{long}.rs`")),
        "{stderr}"
    );
    for side in ["x", "y"] {
        assert!(
            stderr.contains(&format!("Top.impl/app/{long}{side}")),
            "{stderr}"
        );
    }
    assert!(!out.exists(), "nothing may be written on a duplicate path");
    let _ = std::fs::remove_dir_all(&dir);
}
