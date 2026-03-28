//! Full roundtrip integration test: SysML v2 → AADL → analyze → codegen.
//!
//! Tests the entire spar pipeline end-to-end via the CLI binary.

use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

// ── Step 1: SysML v2 → AADL text (lower) ──────────────────────────

#[test]
fn roundtrip_sysml2_parses_cleanly() {
    let output = spar()
        .args([
            "sysml2",
            "parse",
            "../../test-data/roundtrip/building_control.sysml",
        ])
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SOURCE_FILE"),
        "SysML v2 model should parse to a syntax tree"
    );
    // No errors expected in stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error"),
        "SysML v2 parse should produce no errors: {stderr}"
    );
}

#[test]
fn roundtrip_sysml2_lowers_to_valid_aadl() {
    let tmp = std::env::temp_dir().join("spar-roundtrip");
    std::fs::create_dir_all(&tmp).unwrap();
    let aadl_path = tmp.join("lowered.aadl");

    // Lower SysML v2 → AADL
    let lower = spar()
        .args([
            "sysml2",
            "lower",
            "-o",
            aadl_path.to_str().unwrap(),
            "../../test-data/roundtrip/building_control.sysml",
        ])
        .output()
        .expect("failed to run spar sysml2 lower");
    assert!(
        lower.status.success(),
        "lower should succeed: {}",
        String::from_utf8_lossy(&lower.stderr)
    );

    // Re-parse the lowered AADL
    let parse = spar()
        .args(["parse", aadl_path.to_str().unwrap()])
        .output()
        .expect("failed to run spar parse");
    assert!(
        parse.status.success(),
        "lowered AADL should parse cleanly: {}",
        String::from_utf8_lossy(&parse.stderr)
    );

    // Verify AADL content
    let aadl = std::fs::read_to_string(&aadl_path).unwrap();
    assert!(
        aadl.contains("package BuildingControl"),
        "should have package"
    );
    assert!(
        aadl.contains("system BuildingSystem"),
        "should have BuildingSystem"
    );
    assert!(aadl.contains("system Controller"), "should have Controller");
    assert!(
        aadl.contains("process ControlLoop"),
        "should have ControlLoop as process (has action)"
    );
    assert!(
        aadl.contains("system implementation BuildingSystem.impl"),
        "should have BuildingSystem impl"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn roundtrip_lowered_aadl_instantiates() {
    let tmp = std::env::temp_dir().join("spar-roundtrip-inst");
    std::fs::create_dir_all(&tmp).unwrap();
    let aadl_path = tmp.join("lowered.aadl");

    // Lower
    let lower = spar()
        .args([
            "sysml2",
            "lower",
            "-o",
            aadl_path.to_str().unwrap(),
            "../../test-data/roundtrip/building_control.sysml",
        ])
        .output()
        .expect("lower failed");
    assert!(lower.status.success());

    // Instantiate
    let inst = spar()
        .args([
            "instance",
            "--root",
            "BuildingControl::BuildingSystem.impl",
            aadl_path.to_str().unwrap(),
        ])
        .output()
        .expect("instance failed");
    assert!(
        inst.status.success(),
        "instance should succeed: {}",
        String::from_utf8_lossy(&inst.stderr)
    );
    let stdout = String::from_utf8_lossy(&inst.stdout);
    assert!(
        stdout.contains("component instances"),
        "should report instance count: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn roundtrip_lowered_aadl_analyzes() {
    let tmp = std::env::temp_dir().join("spar-roundtrip-analysis");
    std::fs::create_dir_all(&tmp).unwrap();
    let aadl_path = tmp.join("lowered.aadl");

    // Lower
    let lower = spar()
        .args([
            "sysml2",
            "lower",
            "-o",
            aadl_path.to_str().unwrap(),
            "../../test-data/roundtrip/building_control.sysml",
        ])
        .output()
        .expect("lower failed");
    assert!(lower.status.success());

    // Analyze (may have diagnostics but should not crash)
    let analyze = spar()
        .args([
            "analyze",
            "--root",
            "BuildingControl::BuildingSystem.impl",
            "--format",
            "json",
            aadl_path.to_str().unwrap(),
        ])
        .output()
        .expect("analyze failed");

    // Even if analysis finds errors, the command should produce JSON output
    let stdout = String::from_utf8_lossy(&analyze.stdout);
    assert!(
        stdout.contains("\"root\"") && stdout.contains("\"diagnostics\""),
        "should produce JSON with root and diagnostics: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Step 2: Full AADL → analyze → codegen (using building_control.aadl) ──

#[test]
fn roundtrip_aadl_codegen_produces_all_artifacts() {
    let tmp = std::env::temp_dir().join("spar-roundtrip-codegen");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let out_dir = tmp.join("output");

    // Run codegen on the golden AADL model
    let codegen = spar()
        .args([
            "codegen",
            "--root",
            "BuildingControl::BuildingSystem.impl",
            "--output",
            out_dir.to_str().unwrap(),
            "--format",
            "both",
            "--verify",
            "all",
            "--rivet",
            "../../test-data/codegen/building_control.aadl",
        ])
        .output()
        .expect("codegen failed");
    assert!(
        codegen.status.success(),
        "codegen should succeed: {}",
        String::from_utf8_lossy(&codegen.stderr)
    );

    let stderr = String::from_utf8_lossy(&codegen.stderr);
    assert!(
        stderr.contains("Generated"),
        "should report generation: {stderr}"
    );

    // Verify key files were generated
    assert!(out_dir.join("Cargo.toml").exists(), "Cargo.toml");
    assert!(out_dir.join("BUILD.bazel").exists(), "BUILD.bazel");

    // Verify WIT files
    let wit_files: Vec<_> = walkdir(&out_dir, ".wit");
    assert!(!wit_files.is_empty(), "should generate WIT files");
    let wit_content = std::fs::read_to_string(&wit_files[0]).unwrap();
    assert!(
        wit_content.contains("package"),
        "WIT should have package decl"
    );

    // Verify Rust files
    let rs_files: Vec<_> = walkdir(&out_dir, ".rs");
    assert!(!rs_files.is_empty(), "should generate Rust files");

    // Verify at least one Rust file has timing constants
    let any_has_timing = rs_files.iter().any(|f| {
        let content = std::fs::read_to_string(f).unwrap();
        content.contains("PERIOD_PS") || content.contains("DEADLINE_PS")
    });
    assert!(any_has_timing, "should have timing constants in Rust");

    // Verify config files
    let toml_files: Vec<_> = walkdir(&out_dir, ".toml")
        .into_iter()
        .filter(|p| p.starts_with(out_dir.join("config")))
        .collect();
    assert!(!toml_files.is_empty(), "should generate config TOML files");

    // Verify test harnesses
    let test_files: Vec<_> = walkdir(&out_dir, "_test.rs");
    assert!(!test_files.is_empty(), "should generate test harnesses");

    // Verify Lean4 proofs
    let lean_files: Vec<_> = walkdir(&out_dir, ".lean");
    assert!(!lean_files.is_empty(), "should generate Lean4 proofs");

    // Verify Kani harnesses
    let kani_files: Vec<_> = walkdir(&out_dir, "_harness.rs");
    assert!(!kani_files.is_empty(), "should generate Kani harnesses");

    // Verify design docs
    let doc_files: Vec<_> = walkdir(&out_dir, ".md");
    assert!(!doc_files.is_empty(), "should generate design docs");

    // Verify verification YAML
    let yaml_files: Vec<_> = walkdir(&out_dir, ".yaml");
    assert!(!yaml_files.is_empty(), "should generate verification YAML");

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Step 3: End-to-end: SysML v2 → lower → AADL → codegen ─────────

#[test]
fn roundtrip_full_pipeline_sysml2_to_codegen() {
    let tmp = std::env::temp_dir().join("spar-roundtrip-full");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let aadl_path = tmp.join("lowered.aadl");
    let out_dir = tmp.join("generated");

    // Step 1: Lower SysML v2 → AADL
    let lower = spar()
        .args([
            "sysml2",
            "lower",
            "-o",
            aadl_path.to_str().unwrap(),
            "../../test-data/roundtrip/building_control.sysml",
        ])
        .output()
        .expect("lower failed");
    assert!(
        lower.status.success(),
        "Step 1 (lower) failed: {}",
        String::from_utf8_lossy(&lower.stderr)
    );

    // Step 2: Parse AADL (verify it round-trips)
    let parse = spar()
        .args(["parse", aadl_path.to_str().unwrap()])
        .output()
        .expect("parse failed");
    assert!(
        parse.status.success(),
        "Step 2 (parse) failed: {}",
        String::from_utf8_lossy(&parse.stderr)
    );

    // Step 3: Analyze
    let analyze = spar()
        .args([
            "analyze",
            "--root",
            "BuildingControl::BuildingSystem.impl",
            "--format",
            "json",
            aadl_path.to_str().unwrap(),
        ])
        .output()
        .expect("analyze failed");
    let analyze_stdout = String::from_utf8_lossy(&analyze.stdout);
    assert!(
        analyze_stdout.contains("\"diagnostics\""),
        "Step 3 (analyze) should produce diagnostics: {analyze_stdout}"
    );

    // Step 4: Codegen
    let codegen = spar()
        .args([
            "codegen",
            "--root",
            "BuildingControl::BuildingSystem.impl",
            "--output",
            out_dir.to_str().unwrap(),
            "--dry-run",
            aadl_path.to_str().unwrap(),
        ])
        .output()
        .expect("codegen failed");
    assert!(
        codegen.status.success(),
        "Step 4 (codegen) failed: {}",
        String::from_utf8_lossy(&codegen.stderr)
    );
    let codegen_stderr = String::from_utf8_lossy(&codegen.stderr);
    assert!(
        codegen_stderr.contains("Dry run"),
        "Should report dry run: {codegen_stderr}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Recursively find all files with a given extension under a directory.
fn walkdir(dir: &std::path::Path, ext: &str) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(walkdir(&path, ext));
            } else if path.to_str().is_some_and(|s| s.ends_with(ext)) {
                result.push(path);
            }
        }
    }
    result
}
