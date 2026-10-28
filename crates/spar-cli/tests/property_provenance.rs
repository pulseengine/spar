//! REQ-AADL-PROPERTY-PROVENANCE-001: `spar instance --explain <path>` says,
//! for each property value of one component instance, WHAT value spar uses and
//! WHERE it came from: the classifier, the implementation, the declaration
//! itself, a contained `applies to` association (and its declarer), a
//! refined-away original (#496), or an `inherit` property taken from a
//! container (#460). For a `reference (...)` value it names the resolved
//! target and the context component it was resolved from
//! (REQ-AADL-REF-CONTEXT-001).
//!
//! NON-VACUITY: every assertion goes through [`explain`], which first requires
//! the command to exit 0 and print the instance header AND at least one
//! `source:` line. A build without `--explain` (origin/main exits 2 on the
//! unknown option) or one that prints nothing cannot pass a single test here.
//!
//! The assertions are on EXACT output lines, so a source mislabelled — say an
//! inherited value reported as `declared-local`, or a reference resolved from
//! the inheritor instead of the declarer — fails.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// One model for most mechanisms.
///
/// - `T` (type) writes Thread_Properties::Dispatch_Protocol: `classifier`.
/// - `T.impl` writes Compute_Execution_Time: `implementation`.
/// - `Top.impl` writes Actual_Processor_Binding `applies to sub.app`:
///   `contained (applies to) from Top.impl` on `sub.app`, and `inherited` on
///   both threads below it.
/// - `app` declares Period 20 ms locally; `t` inherits it, `t2` overrides it
///   with its own 5 ms.
/// - Two components are named `cpu`: the processor `Top.impl/cpu`, where the
///   declarer names it, and a MEMORY `Top.impl/sub/cpu`, deeper. The reference
///   must resolve from the declarer, `Top.impl`, to the processor.
const MODEL: &str = r#"
package Prov
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
    properties
      Thread_Properties::Dispatch_Protocol => Periodic;
  end T;
  thread implementation T.impl
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end T.impl;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T.impl;
      t2 : thread T.impl { Period => 5 ms; };
  end P.impl;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      app : process P.impl { Period => 20 ms; };
      cpu : memory Mem;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      sub : system Sub.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to sub.app;
  end Top.impl;
end Prov;
"#;

/// The mirror of `binding_reference_context.rs`'s DECLARED_IN_SUB:
/// `reference (cpu)` declared in Sub.impl, where `cpu` is a processor;
/// `Sub.impl/inner/cpu`, one level deeper, is a memory.
const DECLARED_IN_SUB: &str = r#"
package CtxSub
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system Inner
  end Inner;
  system implementation Inner.impl
    subcomponents
      cpu : memory Mem;
  end Inner.impl;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      app : process P.impl;
      cpu : processor Cpu;
      inner : system Inner.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      sub : system Sub.impl;
  end Top.impl;
end CtxSub;
"#;

/// #496: `hw` refined in D.bound; the original in D.base wrote Code_Size and
/// Data_Size, the refinement overrides Code_Size only.
const REFINE: &str = r#"
package Rf
public
  processor Cpu
  end Cpu;
  system Plat
  end Plat;
  system implementation Plat.single
    subcomponents
      cpu : processor Cpu;
  end Plat.single;
  system D
  end D;
  system implementation D.base
    subcomponents
      hw : system Plat { Code_Size => 4 KByte; Data_Size => 1 KByte; };
  end D.base;
  system implementation D.bound extends D.base
    subcomponents
      hw : refined to system Plat.single { Code_Size => 8 KByte; };
  end D.bound;
end Rf;
"#;

/// A fresh file per call: the tests run in parallel and share fixtures, so a
/// shared path could be read while another test rewrites it.
fn tmp(name: &str, src: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("spar-prov-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

fn run(root: &str, name: &str, src: &str, extra: &[&str]) -> std::process::Output {
    let file = tmp(name, src);
    Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["instance", "--root", root])
        .args(extra)
        .arg(file)
        .output()
        .expect("spar binary runs")
}

/// `--explain <path>` as text, after checking the command really explained
/// something (the non-vacuity guard).
fn explain(root: &str, name: &str, src: &str, path: &str) -> Vec<String> {
    let out = run(root, name, src, &["--explain", path]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`--explain {path}` must exit 0 (got {:?}):\n{stdout}{stderr}",
        out.status.code()
    );
    let lines: Vec<String> = stdout.lines().map(str::to_string).collect();
    assert!(
        lines.first().is_some_and(|l| l.contains(" : ")),
        "no instance header — the command did not run:\n{stdout}{stderr}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("    source: ")),
        "no property was explained:\n{stdout}{stderr}"
    );
    lines
}

/// The `source:` and `reference` lines printed under the value line `head`.
fn under(lines: &[String], head: &str) -> Vec<String> {
    let at = lines
        .iter()
        .position(|l| l == head)
        .unwrap_or_else(|| panic!("no line {head:?} in:\n{}", lines.join("\n")));
    lines[at + 1..]
        .iter()
        .take_while(|l| l.starts_with("    "))
        .cloned()
        .collect()
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn a_thread_inherits_its_processes_binding_and_says_from_where() {
    let lines = explain("Prov::Top.impl", "prov.aadl", MODEL, "sub.app.t");
    assert_eq!(lines[0], "Top.impl/sub/app/t : thread Prov::T.impl");
    assert_eq!(
        under(&lines, "  Actual_Processor_Binding => (reference (cpu))"),
        [
            "    source: inherited from Top.impl/sub/app \
             (there: contained (applies to) from Top.impl)",
            // The declarer's cpu, the processor — not the deeper memory.
            "    reference (cpu) -> Top.impl/cpu, resolved from Top.impl",
        ]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn a_contained_association_from_the_root_names_its_declarer() {
    let lines = explain("Prov::Top.impl", "prov.aadl", MODEL, "sub.app");
    assert_eq!(lines[0], "Top.impl/sub/app : process Prov::P.impl");
    assert_eq!(
        under(&lines, "  Actual_Processor_Binding => (reference (cpu))"),
        [
            "    source: contained (applies to) from Top.impl",
            "    reference (cpu) -> Top.impl/cpu, resolved from Top.impl",
        ]
    );
    assert_eq!(
        under(&lines, "  Period => 20 ms"),
        ["    source: declared-local"]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn classifier_and_implementation_values_name_their_classifier() {
    let lines = explain("Prov::Top.impl", "prov.aadl", MODEL, "Top.impl/sub/app/t");
    assert_eq!(
        under(&lines, "  Thread_Properties::Dispatch_Protocol => Periodic"),
        ["    source: classifier Prov::T"]
    );
    assert_eq!(
        under(&lines, "  Compute_Execution_Time => 1 ms .. 2 ms"),
        ["    source: implementation Prov::T.impl"]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
// rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
#[test]
fn a_local_value_overrides_the_inherited_one() {
    let t = explain("Prov::Top.impl", "prov.aadl", MODEL, "sub.app.t");
    assert_eq!(
        under(&t, "  Period => 20 ms"),
        ["    source: inherited from Top.impl/sub/app (there: declared-local)"]
    );
    let t2 = explain("Prov::Top.impl", "prov.aadl", MODEL, "sub.app.t2");
    assert!(
        !t2.iter().any(|l| l == "  Period => 20 ms"),
        "t2's own value replaces the inherited one:\n{}",
        t2.join("\n")
    );
    assert_eq!(
        under(&t2, "  Period => 5 ms"),
        ["    source: declared-local"]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn a_refined_subcomponent_names_the_original_it_inherits_from() {
    let lines = explain("Rf::D.bound", "refine.aadl", REFINE, "hw");
    assert_eq!(lines[0], "D.bound/hw : system Rf::Plat.single");
    assert_eq!(
        under(&lines, "  Data_Size => 1 KByte"),
        ["    source: refined from D.base/hw"]
    );
    assert_eq!(
        under(&lines, "  Code_Size => 8 KByte"),
        ["    source: declared-local"]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
// rivet: verifies REQ-AADL-REF-CONTEXT-001
#[test]
fn a_reference_named_at_two_depths_resolves_from_the_declarer() {
    // `cpu` is Top.impl/sub/cpu (processor) and Top.impl/sub/inner/cpu
    // (memory). The value is declared in Sub.impl, so it names the first.
    let lines = explain("CtxSub::Top.impl", "sub.aadl", DECLARED_IN_SUB, "sub.app.t");
    assert_eq!(
        under(&lines, "  Actual_Processor_Binding => (reference (cpu))"),
        [
            "    source: inherited from Top.impl/sub/app \
             (there: contained (applies to) from Top.impl/sub)",
            "    reference (cpu) -> Top.impl/sub/cpu, resolved from Top.impl/sub",
        ]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn json_carries_source_origin_and_resolved_reference() {
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &[
            "--explain",
            "sub.app.t",
            "--property",
            "Deployment_Properties::Actual_Processor_Binding",
            "--format",
            "json",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(doc["instance"], "Top.impl/sub/app/t");
    assert_eq!(doc["category"], "thread");
    assert_eq!(doc["classifier"], "Prov::T.impl");
    let props = doc["properties"].as_array().expect("properties array");
    assert_eq!(props.len(), 1, "--property selects exactly one: {doc:#}");
    assert_eq!(props[0]["property"], "Actual_Processor_Binding");
    let v = &props[0]["values"][0];
    assert_eq!(v["value"], "(reference (cpu))");
    assert_eq!(v["append"], false);
    assert_eq!(v["source"], "inherited from Top.impl/sub/app");
    assert_eq!(
        v["origin"]["source"],
        "contained (applies to) from Top.impl"
    );
    assert_eq!(
        v["references"],
        serde_json::json!([{
            "path": "cpu",
            "context": "Top.impl",
            "target": "Top.impl/cpu",
        }])
    );

    // Classifier and implementation values carry the declaring classifier.
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub.app.t", "--format", "json"],
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let by_name = |n: &str| {
        doc["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["property"] == n)
            .unwrap_or_else(|| panic!("no {n} in {doc:#}"))["values"][0]
            .clone()
    };
    let dp = by_name("Thread_Properties::Dispatch_Protocol");
    assert_eq!(dp["source"], "classifier");
    assert_eq!(dp["declared_in"], "Prov::T");
    assert_eq!(dp["references"], serde_json::json!([]));
    let cet = by_name("Compute_Execution_Time");
    assert_eq!(cet["source"], "implementation");
    assert_eq!(cet["declared_in"], "Prov::T.impl");
    // The properties are listed in name order, whatever the map's order.
    let names: Vec<&str> = doc["properties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["property"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Actual_Processor_Binding",
            "Compute_Execution_Time",
            "Period",
            "Thread_Properties::Dispatch_Protocol",
        ]
    );
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn the_cli_contract_holds() {
    // An unknown path is an error, not an empty explanation (#417).
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub.nope"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    // `--property` that selects nothing is not success.
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub", "--property", "Period"],
    );
    assert_eq!(out.status.code(), Some(1));
    // A qualified filter matches an unqualified value of that name, and a
    // qualified value only in its own set; a bare filter matches any set.
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &[
            "--explain",
            "sub.app.t",
            "--property",
            "Other::Dispatch_Protocol",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a qualified value in another set"
    );
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &[
            "--explain",
            "sub.app.t",
            "--property",
            "thread_properties::dispatch_protocol",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "set and name match case-insensitively"
    );
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub.app.t", "--property", "Dispatch_Protocol"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "a bare filter matches a qualified value"
    );
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &[
            "--explain",
            "sub.app",
            "--property",
            "Timing_Properties::Period",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("  Period => 20 ms"));
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub.app", "--property", "Other::Period"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "an unqualified value matches any set"
    );
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub.app", "--property", "Period"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("  Period => 20 ms") && !text.contains("Actual_Processor_Binding"),
        "the filter keeps only Period:\n{text}"
    );
    // The root itself, by its name, and an instance with no values.
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "Top.impl"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Top.impl : system Prov::Top.impl\n  (no property values)\n"
    );
    // Usage errors exit 2; --help exits 0.
    for bad in [
        &["--explain"][..],
        &["--explain", "sub", "--property"][..],
        &["--property", "Period"][..],
        &["--explain", "sub", "--analyze"][..],
        &["--bogus"][..],
    ] {
        let out = run("Prov::Top.impl", "prov.aadl", MODEL, bad);
        assert_eq!(out.status.code(), Some(2), "{bad:?}");
    }
    let out = run(
        "Prov::Top.impl",
        "prov.aadl",
        MODEL,
        &["--explain", "sub", "--format", "yaml"],
    );
    assert_eq!(out.status.code(), Some(1), "unknown --format");
    // A flag with no value at all: `run` always appends the file, so pass
    // the flag LAST here, or the file would be taken as its value.
    let file = tmp("prov.aadl", MODEL);
    for flag in [&["--explain"][..], &["--explain", "sub", "--property"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_spar"))
            .args(["instance", "--root", "Prov::Top.impl"])
            .arg(&file)
            .args(flag)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag:?} with no value");
    }
    let help = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["instance", "--help"])
        .output()
        .unwrap();
    assert_eq!(help.status.code(), Some(0));
}

// rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
#[test]
fn without_explain_instance_output_is_unchanged() {
    // The tree view still prints; --explain is the only new path.
    let out = run("Prov::Top.impl", "prov.aadl", MODEL, &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("Instance model: Prov::Top.impl\n"),
        "{text}"
    );
    assert!(!text.contains("source:"), "{text}");
}
