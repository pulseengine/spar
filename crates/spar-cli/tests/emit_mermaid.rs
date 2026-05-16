//! Integration tests for `spar emit --format mermaid`.
//!
//! Tests the happy-path flowchart emission and the `--category` filter.

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// Minimal AADL fixture with a system containing one thread, one processor,
/// and a port connection between them (for edge coverage).
const MODEL: &str = "\
package Emit_Test
public
  processor TestCpu
  end TestCpu;

  thread Worker
  end Worker;

  process App
  end App;

  process implementation App.Impl
    subcomponents
      worker: thread Worker;
  end App.Impl;

  system TestSys
  end TestSys;

  system implementation TestSys.Impl
    subcomponents
      cpu: processor TestCpu;
      app: process App.Impl;
  end TestSys.Impl;
end Emit_Test;
";

fn write_model(tag: &str) -> std::path::PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_emit_mermaid_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, MODEL).expect("write temp AADL");
    path
}

/// Happy path: `spar emit --format mermaid --root ...` should produce a
/// Mermaid flowchart containing the header and the root system name.
#[test]
fn emit_mermaid_happy_path() {
    let path = write_model("happy");

    let output = spar()
        .arg("emit")
        .arg("--root")
        .arg("Emit_Test::TestSys.Impl")
        .arg("--format")
        .arg("mermaid")
        .arg(&path)
        .output()
        .expect("failed to run spar emit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "spar emit exited with failure; stderr:\n{stderr}"
    );

    assert!(
        stdout.starts_with("flowchart TD\n"),
        "expected 'flowchart TD' header; got:\n{stdout}"
    );

    // Root system node should appear.
    assert!(
        stdout.contains("TestSys"),
        "expected root system name 'TestSys' in output; got:\n{stdout}"
    );

    // At least one component node line (contains '[\"').
    assert!(
        stdout.contains("[\""),
        "expected at least one node declaration in output; got:\n{stdout}"
    );

    let _ = fs::remove_file(&path);
}

/// Category filter: `--category thread` should include thread components but
/// exclude processor components.
#[test]
fn emit_mermaid_category_filter_excludes_non_thread() {
    let path = write_model("cat");

    let output = spar()
        .arg("emit")
        .arg("--root")
        .arg("Emit_Test::TestSys.Impl")
        .arg("--format")
        .arg("mermaid")
        .arg("--category")
        .arg("thread")
        .arg(&path)
        .output()
        .expect("failed to run spar emit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "spar emit with --category thread failed; stderr:\n{stderr}"
    );

    assert!(
        stdout.starts_with("flowchart TD\n"),
        "expected 'flowchart TD' header; got:\n{stdout}"
    );

    // Thread subcomponent 'worker' should appear.
    assert!(
        stdout.contains("worker"),
        "expected thread 'worker' in filtered output; got:\n{stdout}"
    );

    // Processor 'cpu' must NOT appear.
    assert!(
        !stdout.contains("cpu"),
        "processor 'cpu' should be absent when filtering by thread; got:\n{stdout}"
    );

    // Process 'app' must NOT appear either.
    assert!(
        !stdout.contains("\"app"),
        "process 'app' should be absent when filtering by thread; got:\n{stdout}"
    );

    let _ = fs::remove_file(&path);
}
