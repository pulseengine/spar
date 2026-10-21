//! #508 — `spar <command> --help` is a request, not an unknown option.
//!
//! The PulseEngine CLI baseline: `--help` exits 0, and an unknown flag exits 2
//! with usage on stderr. Before this, every subcommand parser treated `--help`
//! as an unknown option and exited 1 — `spar allocate --help` printed
//! `Unknown option: --help` — so a request for help and a misspelled flag were
//! the same failure.

use std::process::Command;

/// Every command in the top-level dispatch table (17, measured against
/// v0.46.0: 14 exited non-zero on `--help`; dbc, mcp and lsp exited 0).
const COMMANDS: &[&str] = &[
    "parse", "items", "instance", "analyze", "allocate", "diff", "modes", "emit", "render",
    "verify", "codegen", "insight", "moves", "dbc", "mcp", "sysml2", "lsp",
];

fn spar(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar binary runs")
}

#[test]
fn every_command_help_exits_zero() {
    for cmd in COMMANDS {
        for flag in ["--help", "-h"] {
            let out = spar(&[cmd, flag]);
            assert_eq!(
                out.status.code(),
                Some(0),
                "spar {cmd} {flag}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            assert!(
                !text.contains("Unknown option"),
                "spar {cmd} {flag}: {text}"
            );
        }
    }
}

#[test]
fn an_unknown_option_is_a_usage_error() {
    // Negative control: a misspelled flag must still fail, with 2.
    for cmd in [
        // `insight` takes a subcommand first, so a bare flag is an unknown
        // SUBCOMMAND there — a different path, not changed here.
        "parse", "instance", "analyze", "allocate", "verify", "codegen",
    ] {
        let out = spar(&[cmd, "--no-such-flag", "x.aadl"]);
        assert_eq!(out.status.code(), Some(2), "spar {cmd} --no-such-flag");
        assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown option: --no-such-flag"));
    }
}
