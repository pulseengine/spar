//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — latency decides whether an
//! end-to-end flow crosses a processor boundary by the processor each thread's
//! binding RESOLVES to, not by the binding's text.
//!
//! `LatSame`: `t1` is bound by `Node.impl` to `cpu` and `t2` by `Top.impl` to
//! `left.cpu`, which is the SAME processor. The published v0.46.0 binary
//! compared the text, saw `cpu` != `left.cpu`, and reported "end-to-end flow
//! 'e' crosses processor boundaries but no ... communication delay". The model
//! carries an unused `spare` processor because the note is only considered
//! when a model has two processors.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ONE_PROCESSOR: &str = r#"package LatSame
public
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      spare : processor Cpu;
    properties
      Actual_Processor_Binding => (reference (left.cpu)) applies to left.app.t2;
  end Top.impl;
end LatSame;
"#;

const TWO_PROCESSORS: &str = r#"package LatTwo
public
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
    properties
      Actual_Processor_Binding => (reference (right.cpu)) applies to left.app.t2;
  end Top.impl;
end LatTwo;
"#;

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-bind-latency-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// (latency ran, number of "crosses processor boundaries" notes)
fn crossings(root: &str, name: &str, src: &str) -> usize {
    let file = tmp(name, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("end-to-end flow 'e' latency:"),
        "latency must have computed the flow on {name}: {text}"
    );
    text.lines()
        .filter(|l| l.contains("crosses processor boundaries"))
        .count()
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn two_spellings_of_one_processor_are_not_a_crossing() {
    assert_eq!(
        crossings("LatSame::Top.impl", "same.aadl", ONE_PROCESSOR),
        0
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn two_processors_are_still_a_crossing() {
    // Negative control: t2 on right.cpu, a different processor.
    assert_eq!(crossings("LatTwo::Top.impl", "two.aadl", TWO_PROCESSORS), 1);
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn one_processor_pays_no_inter_processor_overhead() {
    // The latency WALK makes the same decision: with 5 ms of
    // Inter_Processor_Overhead on the flow's owner, one processor named two
    // ways must not pay it. v0.46.0 reported [2 .. 9] ms (4 ms of execution
    // plus a crossing that does not exist).
    let src = ONE_PROCESSOR.replace(
        "      e : end to end flow t1.f -> c1 -> t2.f;\n  end P.impl;",
        "      e : end to end flow t1.f -> c1 -> t2.f;\n    properties\n      Inter_Processor_Overhead => 5 ms;\n  end P.impl;",
    );
    assert_ne!(src, ONE_PROCESSOR);
    let file = tmp("same_overhead.aadl", &src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "LatSame::Top.impl",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("end-to-end flow 'e' latency: [2.000 ms .. 4.000 ms]"),
        "no crossing, no overhead: {text}"
    );
}
