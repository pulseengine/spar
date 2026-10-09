//! Integration test for GitHub issue #459:
//!
//!   An `applies to` clause carrying a comma-separated LIST of containment
//!   paths — `applies to app.s, app.c;` — was treated as a single path. The
//!   grammar accepted the comma (parse succeeded) but resolution never split
//!   the list, so the whole string `app.s, app.c` was reported as one
//!   unresolvable element reference and both threads were then counted as
//!   unbound: three errors from one legal construct.
//!
//!   AS5506D §11.3 defines the clause as
//!     `applies to` contained_model_element_path { `,` contained_model_element_path }*
//!   so the association applies to EACH element independently. The fix splits
//!   the list and resolves each path on its own.

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

fn write_model(tag: &str, src: &str) -> std::path::PathBuf {
    // Per-test tag: cargo runs tests in parallel within one process, so
    // process::id() alone collides. The trailing tag disambiguates so one
    // test's remove_file does not race another's spar invocation.
    let path = env::temp_dir().join(format!(
        "spar_applies_to_list_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, src).expect("write temp AADL");
    path
}

fn analyze(path: &std::path::Path) -> String {
    let output = spar()
        .arg("analyze")
        .arg("--root")
        .arg("Test_Applies_List::Sys.Impl")
        .arg(path)
        .output()
        .expect("failed to run spar");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}\n{stderr}")
}

/// A process `App.Impl` with two threads `s` and `c`, bound to `cpu` via a
/// single list-form `applies to`. `{BINDING}` is substituted per test.
fn model(binding: &str) -> String {
    format!(
        "\
package Test_Applies_List
public
  processor CPU
  end CPU;

  thread Worker
  end Worker;

  process App
  end App;

  process implementation App.Impl
    subcomponents
      s: thread Worker;
      c: thread Worker;
  end App.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu: processor CPU;
      app: process App.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) {binding}
  end Sys.Impl;
end Test_Applies_List;
"
    )
}

/// #459 kill-criterion: the list form binds EVERY element. Neither thread is
/// reported unbound and the joined string is never reported unresolvable.
#[test]
fn list_applies_to_binds_every_element() {
    let path = write_model("list", &model("applies to app.s, app.c;"));
    let combined = analyze(&path);

    assert!(
        !combined.contains("missing required Actual_Processor_Binding"),
        "a list-form `applies to app.s, app.c` must bind both threads — #459 \
         regression.\ncombined output:\n{combined}"
    );
    assert!(
        !combined.contains("could not be resolved"),
        "the comma-joined path must not be reported as one unresolvable path — \
         #459 regression.\ncombined output:\n{combined}"
    );

    let _ = fs::remove_file(&path);
}

/// Non-vacuity + partial-list contract: with `applies to app.s, no_such` the
/// resolvable element `app.s` is bound while only the FAILED element is
/// reported — and the diagnostic names exactly `no_such`, the path the author
/// wrote, never the joined string `app.s, no_such`.
///
/// This is what proves the split is real: a single-path resolver (the bug)
/// would have named the whole string and left thread `s` unbound. Distinct
/// inputs — a resolvable vs. an unresolvable element — produce distinct
/// outputs, so the oracle cannot pass vacuously.
#[test]
fn partial_list_names_only_the_failed_element() {
    let path = write_model("partial", &model("applies to app.s, no_such;"));
    let combined = analyze(&path);

    // The one bad element is reported, naming exactly that path...
    assert!(
        combined.contains("applies_to path 'no_such' could not be resolved"),
        "the failed element must be reported by its own name — #459 regression.\n\
         combined output:\n{combined}"
    );
    // ...and never the comma-joined clause the author never wrote as a path.
    assert!(
        !combined.contains("'app.s, no_such'"),
        "the diagnostic must name the failed element, not the joined list — \
         #459 regression.\ncombined output:\n{combined}"
    );
    // The resolvable element still bound its thread: `s` is NOT reported
    // unbound. (Thread `c` is legitimately unbound here — this list never
    // targets it — so we assert specifically on `s`.)
    assert!(
        !combined.contains("thread 's' is missing required Actual_Processor_Binding"),
        "the resolvable element `app.s` must still receive the binding — #459 \
         regression (a whole-string resolver would leave `s` unbound).\n\
         combined output:\n{combined}"
    );

    let _ = fs::remove_file(&path);
}
