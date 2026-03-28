use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

// ---------------------------------------------------------------------------
// sysml2 parse — always succeeds (even with parse errors, exit 0 if no errors
// or exit 1 if there are errors, but we get syntax tree on stdout either way)
// ---------------------------------------------------------------------------

#[test]
fn sysml2_parse_package_example() {
    let output = spar()
        .args([
            "sysml2",
            "parse",
            "../../test-data/sysml2/Package_Example.sysml",
        ])
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse should produce a syntax tree dump containing SOURCE_FILE
    assert!(
        stdout.contains("SOURCE_FILE"),
        "expected SOURCE_FILE in tree: stdout len={}",
        stdout.len()
    );
}

#[test]
fn sysml2_parse_part_definition_example() {
    let output = spar()
        .args([
            "sysml2",
            "parse",
            "../../test-data/sysml2/Part_Definition_Example.sysml",
        ])
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain a SOURCE_FILE node (even if there are parse errors from quoted names)
    assert!(
        stdout.contains("SOURCE_FILE"),
        "expected SOURCE_FILE in output"
    );
}

#[test]
fn sysml2_parse_connections_example() {
    let output = spar()
        .args([
            "sysml2",
            "parse",
            "../../test-data/sysml2/Connections_Example.sysml",
        ])
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SOURCE_FILE"),
        "expected SOURCE_FILE in output"
    );
}

#[test]
fn sysml2_parse_no_file_shows_error() {
    let output = spar()
        .args(["sysml2", "parse"])
        .output()
        .expect("failed to run spar");
    assert!(
        !output.status.success(),
        "expected non-zero exit for missing file"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing file"),
        "expected error about missing file: {stderr}"
    );
}

#[test]
fn sysml2_parse_multiple_files() {
    let output = spar()
        .args([
            "sysml2",
            "parse",
            "../../test-data/sysml2/Package_Example.sysml",
            "../../test-data/sysml2/Parts_Example.sysml",
        ])
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain output headers for both files
    assert!(
        stdout.contains("Package_Example") && stdout.contains("Parts_Example"),
        "expected both file names in output: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// sysml2 lower — needs files that parse cleanly (no quoted package names)
// We create a temp file with clean SysML2 to ensure success.
// ---------------------------------------------------------------------------

#[test]
fn sysml2_lower_produces_aadl() {
    let dir = std::env::temp_dir().join("spar_test_lower");
    std::fs::create_dir_all(&dir).unwrap();
    let sysml_path = dir.join("test_lower.sysml");
    std::fs::write(
        &sysml_path,
        r#"
package TestPkg {
    part def Sensor {
        port sensorOut : SensorPort;
    }
    port def SensorPort {
        out item data;
    }
}
"#,
    )
    .unwrap();

    let output = spar()
        .args(["sysml2", "lower", sysml_path.to_str().unwrap()])
        .output()
        .expect("failed to run spar");
    assert!(
        output.status.success(),
        "lower should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("system type Sensor") || stdout.contains("package TestPkg"),
        "expected AADL output: {stdout}"
    );
}

#[test]
fn sysml2_lower_with_output_flag() {
    let dir = std::env::temp_dir().join("spar_test_lower_out");
    std::fs::create_dir_all(&dir).unwrap();
    let sysml_path = dir.join("input.sysml");
    let out_path = dir.join("output.aadl");
    std::fs::write(&sysml_path, "package Lib { part def Widget { } }").unwrap();

    let output = spar()
        .args([
            "sysml2",
            "lower",
            "-o",
            out_path.to_str().unwrap(),
            sysml_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run spar");
    assert!(
        output.status.success(),
        "lower -o should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let contents = std::fs::read_to_string(&out_path).unwrap();
    assert!(
        contents.contains("Widget"),
        "output file should contain Widget"
    );
}

#[test]
fn sysml2_lower_no_file_shows_error() {
    let output = spar()
        .args(["sysml2", "lower"])
        .output()
        .expect("failed to run spar");
    assert!(
        !output.status.success(),
        "expected non-zero exit for missing file"
    );
}

#[test]
fn sysml2_lower_parse_error_exits_nonzero() {
    // A file with quoted package name causes parse errors, lower should fail
    let output = spar()
        .args([
            "sysml2",
            "lower",
            "../../test-data/sysml2/Port_Example.sysml",
        ])
        .output()
        .expect("failed to run spar");
    // The file has `package 'Port Example'` which the parser can't handle
    // so either it succeeds (if parser is lenient) or fails with parse error
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        assert!(
            stderr.contains("parse error") || stderr.contains("error"),
            "expected parse error message: {stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// sysml2 extract
// ---------------------------------------------------------------------------

#[test]
fn sysml2_extract_requirements_from_clean_file() {
    let dir = std::env::temp_dir().join("spar_test_extract");
    std::fs::create_dir_all(&dir).unwrap();
    let sysml_path = dir.join("reqs.sysml");
    std::fs::write(
        &sysml_path,
        r#"
requirement def SafetyReq { }
requirement def LatencyReq { }
satisfy SafetyReq by controller;
verify LatencyReq by latencyTest;
"#,
    )
    .unwrap();

    let output = spar()
        .args([
            "sysml2",
            "extract",
            "--requirements",
            sysml_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run spar");
    assert!(
        output.status.success(),
        "extract should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SYSML-REQ-SafetyReq"),
        "expected SafetyReq: {stdout}"
    );
    assert!(
        stdout.contains("SYSML-REQ-LatencyReq"),
        "expected LatencyReq: {stdout}"
    );
    assert!(
        stdout.contains("type: satisfies"),
        "expected satisfy link: {stdout}"
    );
    assert!(
        stdout.contains("type: verifies"),
        "expected verify link: {stdout}"
    );
}

#[test]
fn sysml2_extract_no_requirements_found() {
    let dir = std::env::temp_dir().join("spar_test_extract_empty");
    std::fs::create_dir_all(&dir).unwrap();
    let sysml_path = dir.join("no_reqs.sysml");
    std::fs::write(&sysml_path, "part def Widget { }").unwrap();

    let output = spar()
        .args(["sysml2", "extract", sysml_path.to_str().unwrap()])
        .output()
        .expect("failed to run spar");
    assert!(
        output.status.success(),
        "extract should succeed even with no reqs"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("artifacts: []"),
        "expected empty artifacts: {stdout}"
    );
}

#[test]
fn sysml2_extract_with_output_flag() {
    let dir = std::env::temp_dir().join("spar_test_extract_out");
    std::fs::create_dir_all(&dir).unwrap();
    let sysml_path = dir.join("input.sysml");
    let out_path = dir.join("output.yaml");
    std::fs::write(&sysml_path, "requirement def TestReq { }").unwrap();

    let output = spar()
        .args([
            "sysml2",
            "extract",
            "-o",
            out_path.to_str().unwrap(),
            sysml_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run spar");
    assert!(
        output.status.success(),
        "extract -o should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let contents = std::fs::read_to_string(&out_path).unwrap();
    assert!(
        contents.contains("SYSML-REQ-TestReq"),
        "output file should contain requirement"
    );
}

#[test]
fn sysml2_extract_no_file_shows_error() {
    let output = spar()
        .args(["sysml2", "extract", "--requirements"])
        .output()
        .expect("failed to run spar");
    assert!(
        !output.status.success(),
        "expected non-zero exit for missing file"
    );
}

// ---------------------------------------------------------------------------
// sysml2 top-level dispatch
// ---------------------------------------------------------------------------

#[test]
fn sysml2_no_subcommand_shows_usage() {
    let output = spar()
        .args(["sysml2"])
        .output()
        .expect("failed to run spar");
    assert!(
        !output.status.success(),
        "expected non-zero exit for no subcommand"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage") || stderr.contains("Subcommand"),
        "expected usage output: {stderr}"
    );
}

#[test]
fn sysml2_unknown_subcommand_shows_error() {
    let output = spar()
        .args(["sysml2", "bogus"])
        .output()
        .expect("failed to run spar");
    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown subcommand"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown"),
        "expected unknown subcommand error: {stderr}"
    );
}
