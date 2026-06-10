//! Integration tests for `spar emit --format mermaid` (M2) and the M3
//! extensions `mermaid-class` and `mermaid-req`.

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

// ── M3: mermaid-class ───────────────────────────────────────────────────────

/// `spar emit --format mermaid-class --root ...` should produce a Mermaid
/// classDiagram containing the header and at least one stereotype.
#[test]
fn emit_mermaid_class_happy_path() {
    let path = write_model("class_happy");

    let output = spar()
        .arg("emit")
        .arg("--root")
        .arg("Emit_Test::TestSys.Impl")
        .arg("--format")
        .arg("mermaid-class")
        .arg(&path)
        .output()
        .expect("failed to run spar emit --format mermaid-class");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "spar emit --format mermaid-class failed; stderr:\n{stderr}"
    );

    assert!(
        stdout.starts_with("classDiagram\n"),
        "expected 'classDiagram' header; got:\n{stdout}"
    );

    // At least one stereotype should be present.
    assert!(
        stdout.contains("<<"),
        "expected at least one stereotype (<<...>>) in output; got:\n{stdout}"
    );

    let _ = fs::remove_file(&path);
}

// ── M3: mermaid-req ─────────────────────────────────────────────────────────

/// `spar emit --format mermaid-req` should produce a Mermaid requirementDiagram
/// containing the header and at least one of the well-known REQ-* IDs from
/// artifacts/requirements.yaml (present in repo root).
#[test]
fn emit_mermaid_req_happy_path() {
    // mermaid-req does not require AADL files or --root.
    let output = spar()
        .arg("emit")
        .arg("--format")
        .arg("mermaid-req")
        .current_dir(
            // Run from the workspace root so "artifacts/requirements.yaml" resolves.
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .output()
        .expect("failed to run spar emit --format mermaid-req");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "spar emit --format mermaid-req failed; stderr:\n{stderr}"
    );

    assert!(
        stdout.starts_with("requirementDiagram\n"),
        "expected 'requirementDiagram' header; got:\n{stdout}"
    );

    // At least one well-known REQ should appear (sanitised hyphens → underscores).
    let has_known_req = stdout.contains("REQ_PARSE_001")
        || stdout.contains("REQ_MODEL_001")
        || stdout.contains("REQ_MERMAID");
    assert!(
        has_known_req,
        "expected at least one well-known REQ_* block; got:\n{stdout}"
    );
}
