//! REQ-HIR-FEATUREGROUP-PATH-001 — a connection endpoint resolves through the
//! containing component's feature-group members.
//!
//! The repo's own showcase model `test-data/parser/complex_system.aadl` has
//! `FMC.dual` with `c1 : port sensor_input.nav_data -> primary.input`, where
//! `sensor_input` is a FEATURE GROUP of FMC, not a subcomponent. `spar analyze`
//! used to report `connection 'c1': unresolved source subcomponent
//! 'sensor_input'` (and the same for `c2`) on it.
//!
//! The same fix covers the banner: `Instantiated Pkg::Type.` dropped the
//! implementation name and left a trailing dot.
//!
//! NON-VACUITY: the positive observable is `spar verify` of a requirement
//! "no error-level `completeness` diagnostics" — it FAILS before the fix (the
//! two unresolved-source errors) and PASSES after. A genuinely unresolvable
//! endpoint, written to a temp file, must still make the same requirement fail,
//! so a build that merely silenced the check cannot pass.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ROOT_PLATFORM: &str = "Avionics_System::Avionics_Platform.deployed";
const ROOT_FMC: &str = "Avionics_System::FMC.dual";

fn complex_system() -> String {
    format!(
        "{}/../../test-data/parser/complex_system.aadl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-fg-endpoint-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

const NO_COMPLETENESS_ERRORS: &str = r#"
[[requirement]]
id = "REQ-NO-UNRESOLVED"
description = "no error-level completeness diagnostics"
analysis = "completeness"
severity = "error"
max_count = 0
"#;

// rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
#[test]
fn analyze_no_longer_reports_feature_group_source_as_unresolved() {
    for root in [ROOT_PLATFORM, ROOT_FMC] {
        let (_code, stdout, stderr) = run(&["analyze", "--root", root, &complex_system()]);
        let all = format!("{stdout}{stderr}");
        assert!(
            !all.contains("unresolved source subcomponent 'sensor_input'"),
            "root {root}: still reports the feature group as unresolved:\n{all}"
        );
        assert!(
            !all.contains("unresolved destination"),
            "root {root}:\n{all}"
        );
    }
}

// rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
#[test]
fn verify_completeness_requirement_passes_on_the_showcase_model() {
    let req = tmp("req_pass.toml");
    fs::write(&req, NO_COMPLETENESS_ERRORS).unwrap();
    for root in [ROOT_PLATFORM, ROOT_FMC] {
        let (code, stdout, stderr) = run(&[
            "verify",
            "--root",
            root,
            req.to_str().unwrap(),
            &complex_system(),
        ]);
        assert_eq!(code, 0, "root {root}:\n{stdout}\n{stderr}");
        assert!(
            !format!("{stdout}{stderr}").contains("unresolved"),
            "root {root}: evidence still lists an unresolved endpoint:\n{stdout}{stderr}"
        );
    }
}

// rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
#[test]
fn a_truly_unresolvable_endpoint_still_fails_the_same_requirement() {
    // Same showcase model, with the feature-group name replaced by a name that
    // is neither a subcomponent nor a feature group.
    let src = fs::read_to_string(complex_system()).unwrap();
    let broken = src.replace("sensor_input.nav_data", "no_such_thing.nav_data");
    assert_ne!(src, broken);
    let model = tmp("broken.aadl");
    fs::write(&model, broken).unwrap();
    let req = tmp("req_fail.toml");
    fs::write(&req, NO_COMPLETENESS_ERRORS).unwrap();

    let (code, stdout, stderr) = run(&[
        "verify",
        "--root",
        ROOT_FMC,
        req.to_str().unwrap(),
        model.to_str().unwrap(),
    ]);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    assert!(
        format!("{stdout}{stderr}").contains("unresolved source subcomponent 'no_such_thing'"),
        "{stdout}\n{stderr}"
    );
}

#[test]
fn instantiated_banner_carries_the_implementation_name() {
    let f = complex_system();
    let want = format!("Instantiated {ROOT_PLATFORM} (11 components)");

    let (_c, _o, err) = run(&["analyze", "--root", ROOT_PLATFORM, &f]);
    assert!(err.contains(&want), "analyze banner:\n{err}");
    assert!(!err.contains("Avionics_Platform. ("), "{err}");

    let (_c, out, _e) = run(&["instance", "--root", ROOT_PLATFORM, &f]);
    assert!(
        out.contains(&format!("Instance model: {ROOT_PLATFORM}\n")),
        "instance banner:\n{out}"
    );
}

#[test]
fn instantiated_banner_carries_the_implementation_name_on_allocate_and_verify() {
    let f = complex_system();
    let want = format!("Instantiated {ROOT_PLATFORM} (");

    let (_c, _o, err) = run(&["allocate", "--root", ROOT_PLATFORM, &f]);
    assert!(err.contains(&want), "allocate banner:\n{err}");

    let req = tmp("req_banner.toml");
    fs::write(&req, NO_COMPLETENESS_ERRORS).unwrap();
    let (_c, _o, err) = run(&["verify", "--root", ROOT_PLATFORM, req.to_str().unwrap(), &f]);
    assert!(err.contains(&want), "verify banner:\n{err}");
}
