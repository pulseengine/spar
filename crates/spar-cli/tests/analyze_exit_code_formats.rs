//! Integration test for GitHub issue #469:
//!
//!   `spar analyze` exited **0** on a model carrying analysis **errors** when
//!   `--format json` or `--format sarif` was used. Only `--format text` exited
//!   non-zero. The structured arms `return`ed before the error gate the text arm
//!   ran, so the Error survived in the emitted document but was lost from the
//!   process result — and SARIF/json are precisely the documented CI integration
//!   paths (README advertises SARIF for CI; rivet shells out to `--format json`).
//!   A caller checking the exit code read an erroring model as analysed-clean.
//!
//!   The fix computes the verdict ONCE, from the analysis result, before any
//!   format dispatch, so all three formats return the same exit code for the
//!   same model.
//!
//! NON-VACUITY (this is the oracle the issue demands, verbatim):
//!
//!   * The oracle is a MATRIX, not a single case: {text, json, sarif} ×
//!     {clean model, error model} = six exit-code assertions. A test that only
//!     checked the error model would pass a build that exited 1
//!     UNCONDITIONALLY; a test that only checked one format would miss the
//!     defect entirely. The clean assertions (exit 0) and the error assertions
//!     (exit != 0) disagree on every format, so a build that is constant in
//!     either direction fails half the matrix. Against the pre-fix binary the
//!     json and sarif error rows fail (they exited 0); that is the bug's own
//!     repro.
//!
//!   * The error case asserts the emitted document STILL CARRIES the diagnostic
//!     (`not computed`). A "fix" that exited 1 by dropping the payload would
//!     satisfy a code-only test and break both structured consumers; this
//!     rejects it.
//!
//!   * An unrecognised `--format` must FAIL (exit != 0) and print nothing to
//!     stdout, for BOTH a clean and an error model — not fall through to the
//!     text renderer, where a typo'd format on a clean model would exit 0.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A minimal, self-contained CLEAN model: one periodic thread with complete
/// timing, bound to a processor. 0 error diagnostics on every analysis today;
/// kept deliberately small so later analyses have little surface to turn it
/// non-clean by accident.
const CLEAN_MODEL: &str = r#"package CleanExitCode
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
end CleanExitCode;
"#;

/// An ERROR model: a well-formed end-to-end flow on which no element carries
/// any timing property, so the latency pass reports `not computed` as an Error
/// (spar#455 / #468 provenance) rather than a fabricated 0.000 range. Exactly
/// one Error diagnostic.
const ERROR_MODEL: &str = r#"package ErrorExitCode
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
end ErrorExitCode;
"#;

const CLEAN_ROOT: &str = "CleanExitCode::Top.impl";
const ERROR_ROOT: &str = "ErrorExitCode::Top.impl";

/// Write a model to a per-test temp file. cargo runs tests in parallel within
/// one process, so `process::id()` alone collides; the `tag` disambiguates.
fn write_model(tag: &str, src: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_analyze_exit_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, src).expect("write temp AADL");
    path
}

/// `(exit_code, stdout, stderr)` from `spar analyze --root R --format F <file>`.
fn run(root: &str, format: &str, path: &Path) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .arg("analyze")
        .arg("--root")
        .arg(root)
        .arg("--format")
        .arg(format)
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
    for format in ["text", "json", "sarif"] {
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
    for format in ["text", "json", "sarif"] {
        let (code, _stdout, stderr) = run(ERROR_ROOT, format, &path);
        assert_ne!(
            code, 0,
            "a model with Error diagnostics must exit non-zero in --format \
             {format} (this is the #469 defect for json/sarif); stderr:\n{stderr}"
        );
    }
    let _ = fs::remove_file(&path);
}

#[test]
fn text_and_structured_formats_agree_on_the_same_model() {
    // The discriminating pair, stated as one invariant: for a fixed model, the
    // exit code is identical across formats. A build that broke this — json
    // disagreeing with text — is exactly the pre-fix binary.
    let clean = write_model("agree_clean", CLEAN_MODEL);
    let error = write_model("agree_error", ERROR_MODEL);

    let clean_text = run(CLEAN_ROOT, "text", &clean).0;
    let error_text = run(ERROR_ROOT, "text", &error).0;
    for format in ["json", "sarif"] {
        assert_eq!(
            run(CLEAN_ROOT, format, &clean).0,
            clean_text,
            "--format {format} must agree with text on the clean model"
        );
        assert_eq!(
            run(ERROR_ROOT, format, &error).0,
            error_text,
            "--format {format} must agree with text on the error model"
        );
    }
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
fn structured_error_document_still_carries_the_diagnostic() {
    // A fix that exited non-zero by DROPPING the error from the payload would
    // pass the exit-code assertions and silently break both structured
    // consumers. The document must still carry the Error.
    let path = write_model("carry_diag", ERROR_MODEL);

    for format in ["json", "sarif"] {
        let (code, stdout, _stderr) = run(ERROR_ROOT, format, &path);
        assert_ne!(code, 0, "--format {format} error model must exit non-zero");
        assert!(
            stdout.contains("not computed"),
            "--format {format} document must still carry the Error diagnostic \
             text; got:\n{stdout}"
        );
    }

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
