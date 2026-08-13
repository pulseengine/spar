//! A `--root` that names no component implementation must FAIL, on every path.
//!
//! REGRESSION #417.
//!
//! `spar instance --root Totally::Bogus.Impl --format json` used to print a
//! fabricated node — the requested package/type/impl echoed back, empty
//! children, empty diagnostics — and exit 0. The human-readable path printed
//! "Instance model: Totally::Bogus. / 1 component instances" and also exited 0.
//!
//! Two independent causes, both fixed:
//!   * `SystemInstance::instantiate` returns `Self` and cannot fail; for an
//!     unresolved classifier the builder fabricates a `System` root and records
//!     "unresolved implementation: …" in a diagnostics list nothing reads.
//!   * `Database::instantiate` returned `Some` unconditionally, contradicting
//!     its own doc comment, and a test named `instantiate_not_found` ASSERTED
//!     that — pinning the defect as intended behaviour.
//!
//! Why this test exists at the CLI level rather than only in spar-hir: the unit
//! pair proves `instantiate` returns `None`, but the property users depend on is
//! the EXIT CODE. Guarding the JSON path alone left the human path fabricating,
//! which a library-level test cannot see.

use std::process::Command;

const BOGUS: &str = "Totally::Bogus.Impl";
const REAL: &str = "ConnectionInstantiationExample::sys.impl";

fn model() -> String {
    // The corpus file is two directories up from this crate.
    format!(
        "{}/../../test-data/osate2/BasicHierarchy.aadl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .arg(model())
        .output()
        .expect("spar binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn bogus_root_fails_on_the_json_path() {
    let (code, stdout, stderr) = run(&["instance", "--root", BOGUS, "--format", "json"]);
    assert_ne!(code, 0, "an unresolvable root must not exit 0");
    assert!(
        stdout.trim().is_empty(),
        "no model may be printed for an unresolvable root; got: {stdout}"
    );
    assert!(
        stderr.contains("cannot instantiate root"),
        "the error must name the failure; got: {stderr}"
    );
}

#[test]
fn bogus_root_fails_on_the_human_path() {
    // The path that kept fabricating after the JSON path was fixed.
    let (code, stdout, _stderr) = run(&["instance", "--root", BOGUS]);
    assert_ne!(code, 0, "an unresolvable root must not exit 0");
    assert!(
        !stdout.contains("component instances"),
        "no instance model may be printed for an unresolvable root; got: {stdout}"
    );
}

#[test]
fn the_error_lists_what_is_available() {
    // Absence must be actionable, not merely reported: a typo'd root is the
    // common case and the candidate list is what makes it self-correcting.
    let (_code, _stdout, stderr) = run(&["instance", "--root", BOGUS, "--format", "json"]);
    assert!(
        stderr.contains(REAL),
        "the error should list real implementations, including {REAL}; got: {stderr}"
    );
}

#[test]
fn a_real_root_still_succeeds() {
    // The discriminating partner. A guard that rejected everything would pass
    // all three tests above and fail this one: distinct inputs, distinct
    // outputs.
    let (code, stdout, stderr) = run(&["instance", "--root", REAL, "--format", "json"]);
    assert_eq!(code, 0, "a resolvable root must succeed; stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON instance");
    assert_eq!(v["name"], "sys.impl");
    assert!(
        v["children"].as_array().is_some_and(|c| !c.is_empty()),
        "the real model must have children, else this test would pass on an empty one"
    );
}
