//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — ARINC 653 partition assignment
//! resolves a process's Actual_Processor_Binding from the component that
//! declared it, by identity.
//!
//! `(reference (cpu.p1)) applies to a` in Top.impl binds `a` to partition
//! `p1` of processor `cpu`. The published v0.46.0 binary compared the whole
//! reference `cpu.p1` with each virtual processor's NAME, found none, and
//! reported "process 'a' is not bound to any virtual processor (partition)"
//! for both processes.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ARINC: &str = r#"package Arinc
public
  virtual processor Part
  end Part;
  processor Cpu
  end Cpu;
  processor implementation Cpu.impl
    subcomponents
      p1 : virtual processor Part;
      p2 : virtual processor Part;
  end Cpu.impl;
  process P
  end P;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu.impl;
      a : process P;
      b : process P;
    properties
      Actual_Processor_Binding => (reference (cpu.p1)) applies to a;
      Actual_Processor_Binding => (reference (cpu.p2)) applies to b;
  end Top.impl;
end Arinc;
"#;

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-bind-arinc-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// Processes reported as not bound to any partition.
fn unassigned(name: &str, src: &str) -> Vec<String> {
    let file = tmp(name, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "Arinc::Top.impl",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    // arinc653 prints nothing when every process is assigned, so the
    // precondition is instantiation; the negative control proves the pass
    // runs and still reports.
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("ARINC-PARTITION-ASSIGNMENT"))
        .map(str::to_string)
        .collect()
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_dotted_partition_binding_assigns_the_process() {
    let lines = unassigned("arinc.aadl", ARINC);
    assert!(
        lines.is_empty(),
        "both processes are bound to a partition: {lines:#?}"
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_binding_to_the_processor_is_not_a_partition() {
    // Negative control: bind `a` to the PROCESSOR. It is not a partition, so
    // the warning must still fire, for `a` only.
    let src = ARINC.replace("(reference (cpu.p1))", "(reference (cpu))");
    assert_ne!(src, ARINC);
    let lines = unassigned("arinc_cpu.aadl", &src);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("process 'a'"), "{lines:#?}");
}
