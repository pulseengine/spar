//! `spar --version` must work, and must agree with the crate version (#422).
//!
//! # Why this is not cosmetic
//!
//! A tool result cited as qualification evidence has to carry the identity of
//! the binary that produced it. The maintainer's example: `rivet validate` on
//! one unchanged tree returns FAIL/exit 1 under rivet 0.19.0 and PASS/exit 0
//! under 0.32.0. An analysis result without a version is not reproducible
//! evidence, it is a number with no provenance.
//!
//! spar exposed its version **nowhere** — not via `--version`, not in
//! `--help`, not in any subcommand. `spar --version` printed
//! `Unknown command: --version` and exited 1.
//!
//! # Exit 2, not 1
//!
//! An unknown command now exits 2, matching ordeal — one failure, one exit
//! code across the toolchain. spar also uses 2 for an unresolvable `--root`
//! (#417).
//!
//! What is NOT true, and was claimed here before an audit caught it: that 2
//! means "usage error" throughout the binary. `spar moves verify` returns 2
//! for a FINDING and 1 for an argument error — the inverse convention, in the
//! same executable. #422 asked for the top-level dispatcher to match the org
//! baseline, and that is all this change does.
//!
//! # What the assertions actually pin
//!
//! Not merely "some output appeared": the version must EQUAL
//! `CARGO_PKG_VERSION`. A test asserting only non-empty output would pass on a
//! hardcoded string that drifts from the crate version at the next release —
//! which is the failure mode that makes a version field worse than useless,
//! because it is then confidently wrong.

use std::process::Command;

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

#[test]
fn version_flags_print_the_crate_version_and_exit_zero() {
    for flag in ["--version", "-V", "version"] {
        let (code, stdout, stderr) = run(&[flag]);
        assert_eq!(code, 0, "`spar {flag}` must exit 0; stderr: {stderr}");
        // Equality with the crate version, not merely "looks like a version".
        // A hardcoded string would satisfy a laxer check and then drift.
        assert_eq!(
            stdout.trim(),
            format!("spar {}", env!("CARGO_PKG_VERSION")),
            "`spar {flag}` must print `spar <crate version>` on stdout"
        );
    }
}

#[test]
fn the_version_goes_to_stdout_so_it_can_be_captured() {
    // `spar --version > version.txt` has to work: this output exists to be
    // recorded as evidence, and evidence on stderr is evidence a pipeline
    // silently drops.
    let (_code, stdout, stderr) = run(&["--version"]);
    assert!(!stdout.trim().is_empty(), "version must be on stdout");
    assert!(
        !stderr.contains(env!("CARGO_PKG_VERSION")),
        "version must not go to stderr instead; stderr was: {stderr}"
    );
}

#[test]
fn help_carries_the_version_too() {
    // So a pasted help dump identifies the binary even when the reporter did
    // not think to run --version.
    let (code, _stdout, stderr) = run(&["--help"]);
    assert_eq!(code, 0, "`spar --help` must exit 0");
    assert!(
        stderr.contains(env!("CARGO_PKG_VERSION")),
        "help output should name the version; got: {}",
        stderr.lines().next().unwrap_or("")
    );
}

#[test]
fn an_unknown_command_exits_two() {
    let (code, _stdout, stderr) = run(&["definitely-not-a-command"]);
    assert_eq!(
        code, 2,
        "an unknown command must exit 2, not 1 (#422): one failure, one exit \
         code across the toolchain, matching ordeal"
    );
    assert!(
        stderr.contains("Unknown command"),
        "the error must say what went wrong; got: {stderr}"
    );
}

#[test]
fn a_real_command_still_succeeds() {
    // The discriminating partner. A dispatcher that exited 2 for everything
    // would satisfy every assertion above and fail this one.
    let model = format!(
        "{}/../../test-data/osate2/BasicHierarchy.aadl",
        env!("CARGO_MANIFEST_DIR")
    );
    let (code, _stdout, stderr) = run(&["parse", &model]);
    assert_eq!(
        code, 0,
        "`spar parse` on a valid model must exit 0: {stderr}"
    );
}
