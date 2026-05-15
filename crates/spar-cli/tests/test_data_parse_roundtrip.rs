//! Parse-roundtrip sweep across every shipped top-level `test-data/*.aadl`
//! fixture.
//!
//! The goal is simple: a junior dev cloning the repo and running
//! `spar parse test-data/<name>.aadl` against any of the fixtures
//! shipped at the top of `test-data/` should never see a parse error.
//! This test re-checks that invariant from CI — a fixture that
//! regresses (like `render_test.aadl` did pre-v0.9.1) will fail this
//! sweep instead of silently shipping broken.
//!
//! Scope:
//!   * Only top-level `test-data/*.aadl` files (subdirectories like
//!     `negative/`, `parser/`, `osate2/`, etc. are out of scope —
//!     they have their own per-feature tests, and `negative/` files
//!     are *expected* to fail to parse).
//!   * A fixture may opt out of the sweep by including the comment
//!     `-- spar-test: skip-parse` anywhere in its first 20 lines.
//!     This is intended for future fixtures that demonstrate parser
//!     behaviour we don't yet support; today, no fixture opts out.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// Resolve the workspace `test-data/` directory relative to this
/// crate's `CARGO_MANIFEST_DIR` (`crates/spar-cli`). Going two levels
/// up lands at the workspace root.
fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
}

/// Returns true if the file's first 20 lines contain the opt-out
/// marker `-- spar-test: skip-parse`.
fn has_skip_marker(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    contents
        .lines()
        .take(20)
        .any(|line| line.contains("-- spar-test: skip-parse"))
}

#[test]
fn every_top_level_test_data_aadl_parses_cleanly() {
    let dir = test_data_dir();
    assert!(
        dir.is_dir(),
        "test-data directory not found at {}; CARGO_MANIFEST_DIR layout changed?",
        dir.display(),
    );

    // Collect top-level *.aadl files (no recursion into subdirs).
    let mut fixtures: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read test-data/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("aadl"))
        .collect();
    fixtures.sort();

    assert!(
        !fixtures.is_empty(),
        "no top-level *.aadl fixtures found in {}",
        dir.display(),
    );

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut skipped: Vec<PathBuf> = Vec::new();

    for path in &fixtures {
        if has_skip_marker(path) {
            skipped.push(path.clone());
            continue;
        }
        checked += 1;

        let out = spar()
            .arg("parse")
            .arg(path)
            .output()
            .expect("failed to run spar parse");

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            failures.push(format!(
                "  {}\n    exit={:?}\n    stderr: {}\n    stdout: {}",
                path.display(),
                out.status.code(),
                stderr.trim(),
                stdout.trim(),
            ));
        }
    }

    assert!(
        checked > 0,
        "every fixture was skipped via `-- spar-test: skip-parse`; sweep is a no-op",
    );

    assert!(
        failures.is_empty(),
        "{} of {} top-level test-data/*.aadl fixtures failed `spar parse`:\n{}\n\
         (skipped via `-- spar-test: skip-parse`: {:?})",
        failures.len(),
        checked,
        failures.join("\n"),
        skipped,
    );
}
