//! Smoke tests for the help dispatch surfaced by the 12-persona audit
//! (Tier B #10): `spar help`, `spar --help`, and `spar -h` must all print
//! the same usage banner the no-args path prints, and must exit 0.

use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// The usage banner all help paths must include. Picking a stable substring
/// (the first line of `print_usage`) keeps this test resilient to the
/// command-list growing over time.
const USAGE_HEADER: &str = "Usage: spar <command> [options] <file...>";

fn assert_help_output(form: &[&str]) {
    let output = spar()
        .args(form)
        .output()
        .unwrap_or_else(|e| panic!("failed to run `spar {}`: {e}", form.join(" ")));

    assert!(
        output.status.success(),
        "`spar {}` should exit 0; got status {:?}\nstderr:\n{}",
        form.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    // The usage banner is currently printed to stderr (matching the
    // existing no-args path). Accept either stream so we don't pin
    // future banner-routing decisions.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");

    assert!(
        combined.contains(USAGE_HEADER),
        "`spar {}` did not print the usage banner.\nstdout:\n{}\nstderr:\n{}",
        form.join(" "),
        stdout,
        stderr,
    );
}

#[test]
fn spar_help_subcommand_prints_usage() {
    assert_help_output(&["help"]);
}

#[test]
fn spar_double_dash_help_prints_usage() {
    assert_help_output(&["--help"]);
}

#[test]
fn spar_dash_h_prints_usage() {
    assert_help_output(&["-h"]);
}
