//! `spar emit --format architecture-v1` against the #664 fixture
//! (REQ-EMIT-ARCHITECTURE-V1-001).
//!
//! The fixture under `tests/fixtures/architecture_v1/` is the one in #664,
//! agreed with the warte side: the PE_Arch property set, a synthetic model,
//! and the expected document. The expected document names spar 0.49.0; the
//! test puts this crate's version in its place, so the comparison is exact
//! for every other byte.
//!
//! Schema validation and `warte arch check` run in CI (the Architecture
//! export job in ci.yml), with the schema and the warte binary pinned by
//! sha256.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/architecture_v1")
        .join(name)
}

fn emit(extra: &[&str], files: &[PathBuf]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["emit", "--format", "architecture-v1"])
        .args(["--root", "Example_Security::Security.impl"])
        .args(extra)
        .args(files)
        .output()
        .expect("run spar")
}

#[test]
fn fixture_matches_expected_document() {
    let out = emit(
        &["--generated-at", "2026-01-01T00:00:00Z"],
        &[fixture("pe_arch.aadl"), fixture("example.aadl")],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(fixture("expected.architecture.json"))
        .unwrap()
        .replace(
            "\"version\": \"0.49.0\"",
            &format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION")),
        );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
}

#[test]
fn commit_is_written_when_given() {
    let out = emit(
        &[
            "--generated-at",
            "2026-01-01T00:00:00Z",
            "--commit",
            "abc123",
        ],
        &[fixture("pe_arch.aadl"), fixture("example.aadl")],
    );
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["source"]["commit"], "abc123");
    assert_eq!(v["source"]["model"], "example.aadl");
}

/// A component without `PE_Arch::Zone` fails with exit 1 and names the
/// subcomponent, rather than writing a document the schema rejects.
#[test]
fn missing_required_property_is_an_error() {
    let model = std::fs::read_to_string(fixture("example.aadl"))
        .unwrap()
        .replace(" PE_Arch::Zone => \"z-cloud\";", "");
    let dir = std::env::temp_dir().join(format!("spar_arch_v1_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("example.aadl");
    std::fs::write(&path, model).unwrap();
    let out = emit(&[], &[fixture("pe_arch.aadl"), path]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("subcomponent cloud_api: PE_Arch::Zone is missing"),
        "{err}"
    );
}

/// A protocol outside the schema's enum is refused.
#[test]
fn protocol_outside_enum_is_an_error() {
    let model = std::fs::read_to_string(fixture("example.aadl"))
        .unwrap()
        .replace(
            "PE_Arch::Protocol => \"udp\"",
            "PE_Arch::Protocol => \"coap\"",
        );
    let dir = std::env::temp_dir().join(format!("spar_arch_v1_p_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("example.aadl");
    std::fs::write(&path, model).unwrap();
    let out = emit(&[], &[fixture("pe_arch.aadl"), path]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("connection c_sensor: PE_Arch::Protocol 'coap'"),
        "{err}"
    );
}
