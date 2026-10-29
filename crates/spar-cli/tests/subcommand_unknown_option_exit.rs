//! #508 — an unknown option is a usage error on EVERY subcommand: exit 2,
//! with the offending flag and a usage line on stderr.
//!
//! Measured on v0.47.1 across the 19 subcommands `spar --help` lists:
//! `--help` exited 0 on all 19 (#512), but an unknown option exited 2 on only
//! 14. `sysml2`, `moves`, `insight` and `mcp` exited 1, and `lsp` exited 0: it
//! ignored the flag and started the server.
//!
//! Two of those read stdin (`lsp`, `mcp serve`), so a regression there is a
//! server that starts instead of a usage error. Every invocation here runs
//! with stdin closed and under a deadline, so such a regression is a failing
//! test, not a hung suite.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);
const BOGUS: &str = "--definitely-not-a-flag";

struct Run {
    code: Option<i32>,
    stderr: String,
}

/// Run `spar <args>` with stdin closed; kill it and fail if it outlives
/// [`DEADLINE`].
fn spar(args: &[&str]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spar binary runs");
    // Drain both pipes on their own threads, so a chatty child cannot block
    // on a full pipe while we poll it.
    let mut out = child.stdout.take().expect("stdout piped");
    let mut err = child.stderr.take().expect("stderr piped");
    let out_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
    });
    let err_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
            break st;
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "spar {} still running after {DEADLINE:?} with stdin closed: \
                 it started instead of rejecting its arguments",
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    out_t.join().expect("stdout reader");
    let stderr = String::from_utf8_lossy(&err_t.join().expect("stderr reader")).into_owned();
    Run {
        code: status.code(),
        stderr,
    }
}

/// The subcommands `spar --help` lists, read from its `Commands:` block
/// (stderr), so a subcommand added later is covered without editing this file.
fn listed_commands() -> Vec<String> {
    let help = spar(&["--help"]);
    assert_eq!(help.code, Some(0), "spar --help: {}", help.stderr);
    let cmds: Vec<String> = help
        .stderr
        .lines()
        .skip_while(|l| l.trim_end() != "Commands:")
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .filter(|w| !w.starts_with('-'))
        .map(str::to_string)
        .collect();
    // An empty or truncated parse would make the loop below vacuous.
    assert!(
        cmds.len() >= 19,
        "parsed only {} subcommands from `spar --help`: {cmds:?}\n{}",
        cmds.len(),
        help.stderr
    );
    cmds
}

fn assert_usage_error(args: &[&str], expect: &str, usage: bool) {
    let r = spar(args);
    assert_eq!(
        r.code,
        Some(2),
        "spar {} must exit 2, stderr: {}",
        args.join(" "),
        r.stderr
    );
    assert!(
        r.stderr.contains(expect),
        "spar {}: stderr lacks {expect:?}: {}",
        args.join(" "),
        r.stderr
    );
    if usage {
        assert!(
            r.stderr.contains("Usage: spar"),
            "spar {}: no usage on stderr: {}",
            args.join(" "),
            r.stderr
        );
    }
}

// rivet: verifies REQ-CLI-UNKNOWN-OPTION-001
#[test]
fn every_listed_subcommand_rejects_an_unknown_option_with_2() {
    for cmd in listed_commands() {
        assert_usage_error(&[&cmd, BOGUS], &format!("Unknown option: {BOGUS}"), false);
    }
}

// rivet: verifies REQ-CLI-UNKNOWN-OPTION-001
#[test]
fn the_five_nonconforming_subcommands_print_usage_and_exit_2() {
    let flag = format!("Unknown option: {BOGUS}");
    // Unknown option, at the top level and at every nested level.
    let flagged: &[&[&str]] = &[
        &["sysml2", BOGUS],
        &["sysml2", "parse", BOGUS],
        &["sysml2", "lower", BOGUS],
        &["moves", BOGUS],
        &["moves", "verify", BOGUS],
        &["moves", "enumerate", BOGUS],
        &["insight", BOGUS],
        &["insight", "verify-trace", BOGUS],
        &["lsp", BOGUS],
        &["mcp", BOGUS],
        &["mcp", "serve", BOGUS],
    ];
    for args in flagged {
        assert_usage_error(args, &flag, true);
    }
    // An unknown nested SUBCOMMAND is a usage error too, and names itself as
    // a subcommand, not an option.
    for (cmd, msg) in [
        ("sysml2", "Unknown sysml2 subcommand: bogus"),
        ("moves", "Unknown moves subcommand: bogus"),
        ("insight", "Unknown insight subcommand: bogus"),
        ("mcp", "Unknown mcp subcommand: bogus"),
    ] {
        assert_usage_error(&[cmd, "bogus"], msg, true);
    }
    // lsp takes no positional arguments.
    assert_usage_error(&["lsp", "bogus"], "Unexpected argument: bogus", true);
}

// rivet: verifies REQ-CLI-UNKNOWN-OPTION-001
#[test]
fn the_server_launch_lines_are_not_usage_errors() {
    // The VS Code extension launches `spar lsp` with TransportKind.stdio, and
    // vscode-languageclient 9 appends `--stdio` to an Executable's args. That
    // flag must start the server; with stdin closed it reads EOF and exits 0.
    let r = spar(&["lsp", "--stdio"]);
    assert_eq!(r.code, Some(0), "spar lsp --stdio: {}", r.stderr);
    let r = spar(&["lsp"]);
    assert_eq!(r.code, Some(0), "spar lsp: {}", r.stderr);
    // `spar mcp serve` is accepted: it either execs spar-mcp (EOF, exit 0) or
    // reports that spar-mcp is not installed (1). Never a usage error.
    let r = spar(&["mcp", "serve"]);
    assert_ne!(r.code, Some(2), "spar mcp serve: {}", r.stderr);
    assert!(
        !r.stderr.contains("Unknown") && !r.stderr.contains("Unexpected"),
        "spar mcp serve: {}",
        r.stderr
    );
}
