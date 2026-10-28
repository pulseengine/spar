//! REQ-AADL-PROPERTY-INHERIT-001 (#460) — an `inherit` property takes the
//! CONTAINING component's value where a component declares none.
//!
//! `Actual_Processor_Binding` is declared `inherit` in the predeclared
//! Deployment_Properties (osate2 @ 87468992ee0d4c4cc856a72bd542899953f088f3,
//! Deployment_Properties.aadl:41). A thread inside a process bound with
//! `... applies to app` IS bound. spar did not implement inheritance, so
//! `spar analyze` reported `thread 't' is missing required
//! Actual_Processor_Binding` on that legal model — measured on v0.46.0 and on
//! origin/main 833486f, and three times on this repo's own
//! test-data/parser/complex_system.aadl.
//!
//! NON-VACUITY:
//! - The negative control: a thread in a process that NOTHING binds must
//!   still be reported, so a build that silenced the rule cannot pass.
//! - The declarer's context: the bound process `app` has a child named `cpu`
//!   that is a THREAD GROUP, and the thread has no children at all. The
//!   inherited `reference (cpu)` must resolve from Top.impl, its declarer, to
//!   the processor. Resolving from the thread would report a `cpu` that does
//!   not exist; resolving from the process would report a thread group. Both
//!   are binding diagnostics on the thread's path, checked for below.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const MODEL: &str = r#"
package Inh
public
  processor Cpu
  end Cpu;
  thread group G
  end G;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
      cpu : thread group G;
  end P.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
      lone : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app;
  end Top.impl;
end Inh;
"#;

/// Run `spar analyze`; `witness` is a line the run must print, so an empty
/// result cannot pass by the analysis not having run.
fn analyze(root: &str, file: &std::path::Path, witness: &str) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let lines: Vec<String> = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::to_string)
        .collect();
    assert!(
        lines.iter().any(|l| l.contains(witness)),
        "expected `{witness}` — otherwise an empty result proves nothing:\n{}",
        lines.join("\n")
    );
    lines
}

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-inherit-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

const MISSING: &str = "is missing required Actual_Processor_Binding";

// rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
#[test]
fn a_thread_in_a_bound_process_is_bound_and_an_unbound_one_is_not() {
    let lines = analyze(
        "Inh::Top.impl",
        &tmp("inherit.aadl", MODEL),
        "binding_rules",
    );
    let missing: Vec<&String> = lines.iter().filter(|l| l.contains(MISSING)).collect();
    // Negative control: `lone`'s thread is bound by nothing.
    assert_eq!(
        missing.len(),
        1,
        "exactly the unbound thread is reported: {missing:#?}"
    );
    assert!(
        missing[0].contains("Top.impl/lone/t"),
        "the report names the unbound thread: {missing:#?}"
    );
    // The bound thread: no binding error of any kind. A context taken from
    // the thread reports `cpu` missing; one taken from `app` reports a thread
    // group.
    let on_bound_thread: Vec<&String> = lines
        .iter()
        .filter(|l| {
            l.contains("Top.impl/app/t)")
                && (l.contains("binding_rules") || l.contains("binding_check"))
        })
        .collect();
    assert!(
        on_bound_thread.is_empty(),
        "the inherited binding names Top.impl/cpu, a processor: {on_bound_thread:#?}"
    );
}

// rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
#[test]
fn complex_system_has_no_false_missing_binding() {
    // The three false positives #460's triage measured on this repo's model.
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/parser/complex_system.aadl");
    // Witness: with the binding inherited, scheduling now SEES both
    // nav_threads on `cpu` (25 ms / 50 ms each = 100%); before, it reported
    // them as unbound and computed no utilization at all.
    let lines = analyze(
        "Avionics_System::Avionics_Platform.deployed",
        &file,
        "processor 'cpu' utilization 100.0%",
    );
    let missing: Vec<&String> = lines.iter().filter(|l| l.contains(MISSING)).collect();
    assert!(missing.is_empty(), "{missing:#?}");
}
