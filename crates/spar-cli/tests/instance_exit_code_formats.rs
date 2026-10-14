//! Integration test for GitHub issue #472 — the #469 shape in the sibling
//! `spar instance` command:
//!
//!   `spar instance --analyze` exited **0** on a model carrying analysis
//!   **errors** when `--format json` was used. Only `--format text` exited
//!   non-zero. Worse than #469's `analyze` defect: the JSON arm `return`ed
//!   BEFORE the analyze block, so with `--format json` the analysis was never
//!   run at all and `--analyze` was silently dropped — the structured document
//!   carried neither the diagnostics nor a non-zero verdict. A caller checking
//!   the exit code read an erroring model as analysed-clean.
//!
//!   The fix (mirroring #471 for `analyze`) runs the analysis ONCE when
//!   `--analyze` is set regardless of format, computes the verdict before any
//!   format dispatch, and exits from one shared gate, so text and json return
//!   the same exit code for the same model and the json document carries the
//!   diagnostics.
//!
//! NON-VACUITY (the oracle the issue demands — a MATRIX, not a single case):
//!
//!   * {text, json} × {clean model, error model} = four exit-code assertions.
//!     A test that only checked the error model would pass a build that exited
//!     1 UNCONDITIONALLY; a test that only checked one format would miss the
//!     defect entirely. The clean assertions (exit 0) and the error assertions
//!     (exit != 0) disagree on every format, so a build constant in either
//!     direction fails half the matrix. Against the pre-fix binary the json
//!     error row fails (it exited 0) — that is the bug's own repro.
//!
//!   * The error case asserts the emitted json document STILL CARRIES the
//!     diagnostic (`not computed`). Before the fix json carried nothing
//!     (`--analyze` was dropped); a "fix" that merely exited 1 while dropping
//!     the payload would satisfy a code-only test and break the structured
//!     consumer. This rejects both.
//!
//!   * An unrecognised `--format` must FAIL (exit != 0) and print nothing to
//!     stdout, for BOTH a clean and an error model — not fall through to the
//!     text renderer, where a typo'd format on a clean model exited 0.
//!
//! The CLEAN_MODEL / ERROR_MODEL bodies are the same pair
//! `analyze_exit_code_formats` uses — only the package names differ (renamed to
//! disambiguate the two test files), so the analysis-relevant content
//! (components, properties, flows, timing) is identical and the two sibling
//! commands are pinned against the same inputs.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A minimal, self-contained CLEAN model: one periodic thread with complete
/// timing, bound to a processor. 0 error diagnostics on every analysis today.
const CLEAN_MODEL: &str = r#"package CleanInstanceExit
public
  processor CPU
  end CPU;

  thread Worker
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 2 ms;
  end Worker;

  process App
  end App;

  process implementation App.Impl
    subcomponents
      w : thread Worker;
  end App.Impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      cpu : processor CPU;
      app : process App.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.w;
  end Top.impl;
end CleanInstanceExit;
"#;

/// An ERROR model: a well-formed end-to-end flow on which no element carries
/// any timing property, so the latency pass reports `not computed` as an Error
/// (spar#455 / #468 provenance) rather than a fabricated 0.000 range. Exactly
/// one Error diagnostic.
const ERROR_MODEL: &str = r#"package ErrorInstanceExit
public
  device Src
    features
      dout : out data port;
    flows
      fsrc : flow source dout;
  end Src;

  device Dst
    features
      din : in data port;
    flows
      fsink : flow sink din;
  end Dst;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      src : device Src;
      dst : device Dst;
    connections
      c1 : port src.dout -> dst.din;
    flows
      req_to_done : end to end flow src.fsrc -> c1 -> dst.fsink;
  end Top.impl;
end ErrorInstanceExit;
"#;

const CLEAN_ROOT: &str = "CleanInstanceExit::Top.impl";
const ERROR_ROOT: &str = "ErrorInstanceExit::Top.impl";

/// Write a model to a per-test temp file. cargo runs tests in parallel within
/// one process, so `process::id()` alone collides; the `tag` disambiguates.
fn write_model(tag: &str, src: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_instance_exit_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, src).expect("write temp AADL");
    path
}

/// `(exit_code, stdout, stderr)` from
/// `spar instance --root R --format F --analyze <file>`.
///
/// Every row passes `--analyze`: the exit-code gate only exists when an
/// analysis runs, and the defect was that `--format json` dropped the flag.
fn run(root: &str, format: &str, path: &Path) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .arg("instance")
        .arg("--root")
        .arg(root)
        .arg("--format")
        .arg(format)
        .arg("--analyze")
        .arg(path)
        .output()
        .expect("spar binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn clean_model_exits_zero_in_every_format() {
    let path = write_model("clean_ok", CLEAN_MODEL);
    for format in ["text", "json"] {
        let (code, _stdout, stderr) = run(CLEAN_ROOT, format, &path);
        assert_eq!(
            code, 0,
            "a clean model must exit 0 in --format {format}; stderr:\n{stderr}"
        );
    }
    let _ = fs::remove_file(&path);
}

#[test]
fn error_model_exits_nonzero_in_every_format() {
    let path = write_model("error_nonzero", ERROR_MODEL);
    for format in ["text", "json"] {
        let (code, _stdout, stderr) = run(ERROR_ROOT, format, &path);
        assert_ne!(
            code, 0,
            "a model with Error diagnostics must exit non-zero in --format \
             {format} (this is the #472 defect for json); stderr:\n{stderr}"
        );
    }
    let _ = fs::remove_file(&path);
}

#[test]
fn text_and_json_agree_on_the_same_model() {
    // The discriminating pair, stated as one invariant: for a fixed model, the
    // exit code is identical across formats. A build that broke this — json
    // disagreeing with text — is exactly the pre-fix binary.
    let clean = write_model("agree_clean", CLEAN_MODEL);
    let error = write_model("agree_error", ERROR_MODEL);

    let clean_text = run(CLEAN_ROOT, "text", &clean).0;
    let error_text = run(ERROR_ROOT, "text", &error).0;
    assert_eq!(
        run(CLEAN_ROOT, "json", &clean).0,
        clean_text,
        "--format json must agree with text on the clean model"
    );
    assert_eq!(
        run(ERROR_ROOT, "json", &error).0,
        error_text,
        "--format json must agree with text on the error model"
    );
    // And the two models must themselves disagree, or the agreement above is
    // vacuous (both formats could be a constant).
    assert_ne!(
        clean_text, error_text,
        "clean and error models must produce different exit codes, else the \
         cross-format agreement proves nothing"
    );

    let _ = fs::remove_file(&clean);
    let _ = fs::remove_file(&error);
}

#[test]
fn json_error_document_still_carries_the_diagnostic() {
    // A fix that exited non-zero by DROPPING the error from the payload (or
    // leaving --analyze silently dropped, as before) would pass the exit-code
    // assertions and leave the structured consumer blind. The json document
    // must carry the Error, and the text arm must still report it on stderr.
    let path = write_model("carry_diag", ERROR_MODEL);

    let (code, stdout, _stderr) = run(ERROR_ROOT, "json", &path);
    assert_ne!(code, 0, "--format json error model must exit non-zero");
    assert!(
        stdout.contains("not computed"),
        "--format json document must carry the Error diagnostic text; got:\n{stdout}"
    );

    // Text writes diagnostics to stderr; assert there too for symmetry.
    let (_code, _stdout, stderr) = run(ERROR_ROOT, "text", &path);
    assert!(
        stderr.contains("not computed"),
        "text error model must still report the Error diagnostic; got:\n{stderr}"
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn unrecognised_format_fails_and_prints_nothing() {
    // An unknown --format must FAIL on EVERY model — not fall through to the
    // text renderer, where a clean model would exit 0 and hide the typo.
    for (root, model, tag) in [
        (CLEAN_ROOT, CLEAN_MODEL, "badfmt_clean"),
        (ERROR_ROOT, ERROR_MODEL, "badfmt_error"),
    ] {
        let path = write_model(tag, model);
        let (code, stdout, stderr) = run(root, "jsonn", &path);
        assert_ne!(
            code, 0,
            "an unrecognised --format must fail even on {root}; stderr:\n{stderr}"
        );
        assert!(
            stdout.trim().is_empty(),
            "an unrecognised --format must print nothing to stdout; got:\n{stdout}"
        );
        assert!(
            stderr.contains("Unknown --format"),
            "the error must name the unknown format; got:\n{stderr}"
        );
        let _ = fs::remove_file(&path);
    }
}
