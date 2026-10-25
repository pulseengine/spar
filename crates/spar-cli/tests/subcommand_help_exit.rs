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
    // Negative control: a misspelled flag must still fail, with 2, on every
    // command whose option parser this change touched. The in-diff mutation
    // gate (#400) showed that items, modes, diff, render, dbc, sysml2
    // generate and insight verify-trace had NO test at all: replacing each
    // command with `()` survived. Reaching their unknown-option exit kills it.
    let commands: &[&[&str]] = &[
        &["parse"],
        &["instance"],
        &["analyze"],
        &["allocate"],
        &["verify"],
        &["codegen"],
        &["items"],
        &["modes"],
        &["diff"],
        &["render"],
        &["dbc"],
        &["sysml2", "generate"],
        &["insight", "verify-trace"],
    ];
    for cmd in commands {
        let mut args: Vec<&str> = cmd.to_vec();
        args.extend(["--no-such-flag", "x.aadl"]);
        let out = spar(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "spar {} --no-such-flag",
            cmd.join(" ")
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("Unknown option: --no-such-flag"),
            "spar {}: {}",
            cmd.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
