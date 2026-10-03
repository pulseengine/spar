//! Every command spar dispatches must be findable in its help.
//!
//! ## The defect
//!
//! `--help` documented 16 commands; `main.rs` dispatched 20. The four missing
//! ones were `sysml2`, `extract`, `generate` and `version` — which meant the
//! entire SysML v2 surface, 8378 LOC and 276 tests, was undiscoverable to
//! anyone who had not read the source. Two hand-maintained lists, drifted.
//!
//! ## Why this file is small
//!
//! Because the fix was not a guard. `main` and `print_usage` now read the same
//! `COMMANDS` table, so a command that dispatches is documented **by
//! construction** and adding one without a description does not compile. There
//! is no longer a second list to diverge from.
//!
//! What is left to test is that the table is actually wired to both ends — that
//! the help really is generated from it, and that the entry points really are
//! reached — plus the short-circuit spellings that sit outside the table. These
//! run the REAL binary (`CARGO_BIN_EXE_spar`) rather than re-deriving anything,
//! for the same reason `cli_version.rs` does: the artifact users get is the one
//! under test.

use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

fn help_text() -> String {
    let out = spar().arg("--help").output().expect("spar --help runs");
    // Usage goes to stderr; the version line goes to stdout. Read both so this
    // test does not silently depend on which stream carries what.
    String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
}

/// The `Commands:` block, as a user would read it.
fn documented() -> Vec<String> {
    let text = help_text();
    let mut names = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_block = true;
            continue;
        }
        if in_block {
            if line.trim().is_empty() {
                break;
            }
            if let Some(first) = line.split_whitespace().next() {
                names.push(first.to_string());
            }
        }
    }
    names
}

#[test]
fn every_dispatched_command_is_documented() {
    let listed = documented();

    // Blindness guard. An empty or unparsed help block agrees with every
    // expectation — zero undocumented commands would be indistinguishable from
    // zero commands read.
    assert!(
        listed.len() >= 15,
        "parsed only {} command(s) out of `--help`; the block format changed and \
         this test would otherwise pass by reading nothing: {listed:?}",
        listed.len()
    );

    // Spot-check the four that were missing when this test was written. Named
    // explicitly rather than derived, because the derivation is exactly what
    // the single-source change removed — there is no second list left to diff.
    for want in ["sysml2", "extract", "generate"] {
        assert!(
            listed.iter().any(|n| n == want),
            "`{want}` dispatches but is absent from `--help`. It is in COMMANDS \
             or it is not dispatched; the two cannot differ."
        );
    }
}

/// The table is wired to the DISPATCHER, not only to the help. A name could be
/// listed and still not route, which would be the same defect facing the other
/// way. Every documented command must be reached — proven by it NOT producing
/// the unknown-command response.
#[test]
fn every_documented_command_actually_dispatches() {
    let mut unreachable = Vec::new();
    for name in documented() {
        // Run with no further arguments. Commands legitimately exit non-zero
        // for missing arguments; what must not happen is "Unknown command".
        let out = spar().arg(&name).output().expect("spar runs");
        let all = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        if all.contains("Unknown command") {
            unreachable.push(name);
        }
    }
    assert!(
        unreachable.is_empty(),
        "documented but not dispatched: {unreachable:?}"
    );
}

/// Non-vacuity for the test above: the unknown-command path must be reachable,
/// or `every_documented_command_actually_dispatches` would pass for a binary
/// that never emits that string at all.
#[test]
fn an_unknown_command_is_reported_as_unknown() {
    let out = spar()
        .arg("definitely-not-a-command")
        .output()
        .expect("spar runs");
    let all =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        all.contains("Unknown command"),
        "the unknown-command path did not fire, so the dispatch test above \
         cannot distinguish routed from unrouted"
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "unknown command must exit 2 (#422)"
    );
}

/// Nothing may dispatch *around* the table.
///
/// Found by mutation, not by design: adding `"secret" => cmd_parse(&args[2..])`
/// to the fallback `match` survived every other test in this file. The table
/// makes the commands *in it* consistent — it cannot stop a hand-written arm
/// being added beside it, which is exactly how `sysml2`, `extract`, `generate`
/// and `version` came to dispatch undocumented in the first place.
///
/// So this reads the source. That is modelling rather than measuring, which
/// this repo normally avoids — but the alternative is enumerating every string
/// a user might type, and the thing being guarded against is a *source edit*.
/// `include_str!` binds it at compile time, so it cannot drift from the file it
/// describes.
#[test]
fn no_command_dispatches_outside_the_table() {
    const SOURCE: &str = include_str!("../src/main.rs");

    // Only these may appear as literal arms in the fallback match: they
    // short-circuit before the table by design and are not model operations.
    const ALLOWED: &[&str] = &["help", "--help", "-h", "--version", "-V", "version"];

    let start = SOURCE
        .find("match args[1].as_str() {")
        .expect("the fallback dispatch match must exist — did the dispatcher move?");
    let block = &SOURCE[start..];
    let end = block
        .find("\n    }\n")
        .expect("could not find the end of the dispatch match");
    let block = &block[..end];

    let mut stray = Vec::new();
    for line in block.lines() {
        let t = line.trim_start();
        if !t.starts_with('"') {
            continue;
        }
        // An arm line looks like:  "a" | "b" => { ...
        let Some(arms) = t.split("=>").next() else {
            continue;
        };
        for lit in arms.split('|') {
            let name = lit.trim().trim_matches('"').trim();
            if name.is_empty() {
                continue;
            }
            if !ALLOWED.contains(&name) {
                stray.push(name.to_string());
            }
        }
    }

    assert!(
        stray.is_empty(),
        "these dispatch around COMMANDS: {stray:?}. A command reached by a \
         hand-written match arm is invisible to `--help`, which is the defect \
         REQ-CLI-SURFACE-001 exists for. Add it to COMMANDS instead — that \
         documents it by construction."
    );
}

/// Non-vacuity for the test above: it must be able to see an arm at all. If the
/// parse found nothing, an empty `stray` would mean "read nothing", not "clean".
#[test]
fn the_source_scan_can_actually_see_dispatch_arms() {
    const SOURCE: &str = include_str!("../src/main.rs");
    let start = SOURCE
        .find("match args[1].as_str() {")
        .expect("match exists");
    let block = &SOURCE[start..start + 800];
    assert!(
        block.contains("\"--version\"") && block.contains("\"--help\""),
        "the scan window does not contain the known short-circuit arms, so \
         no_command_dispatches_outside_the_table would pass by reading nothing"
    );
}

/// `version` and `help` are deliberately NOT in `COMMANDS` — they short-circuit
/// before it and are not operations over a model. They must still work, and
/// they must still be visible in the help, which is why `print_usage` prints
/// them on their own lines.
#[test]
fn the_short_circuit_spellings_work_and_are_visible() {
    for form in ["--help", "-h", "help"] {
        let out = spar().arg(form).output().expect("runs");
        assert_eq!(out.status.code(), Some(0), "`spar {form}` must exit 0");
    }
    for form in ["--version", "-V", "version"] {
        let out = spar().arg(form).output().expect("runs");
        assert_eq!(out.status.code(), Some(0), "`spar {form}` must exit 0");
    }
    let text = help_text();
    assert!(
        text.contains("--version") && text.contains("--help"),
        "both flag spellings must appear in the help, or they are as \
         undiscoverable as the subcommands this test exists for"
    );
}
