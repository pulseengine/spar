//! REQ-AADL-BINDING-CONSUMERS-002 item (2): `spar moves enumerate` ranks a
//! candidate processor by the RTA results of the threads that run ON THAT
//! PROCESSOR, identified by identity rather than by the processor name in
//! RTA's message text (#498).
//!
//! Up to v0.48.2 the ranker matched `on processor '<leaf name>'` in the
//! message. Two failures followed, one per model below:
//!
//! * COLLIDE: two subsystems each hold a processor named `cpu`. A deadline
//!   miss on `s2.cpu` printed `on processor 'cpu'` and was charged to `s1.cpu`
//!   too, so both candidates ranked as deadline misses.
//! * DOTTED: a binding written `reference (hw.cpu)` prints `on processor
//!   'hw.cpu'`, which the leaf name `cpu` never matched, so the candidate had
//!   no response time at all.

use std::path::PathBuf;
use std::process::Command;

const COLLIDE: &str = r#"package Rk
public
  processor CPU
    properties
      Scheduling_Protocol => (RMS);
  end CPU;
  thread Fine
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
      Deadline => 10 ms;
  end Fine;
  thread Late
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 50 ms .. 50 ms;
      Deadline => 10 ms;
  end Late;
  thread Mover
  end Mover;
  process PFine
  end PFine;
  process implementation PFine.impl
    subcomponents
      w: thread Fine;
  end PFine.impl;
  process PLate
  end PLate;
  process implementation PLate.impl
    subcomponents
      w: thread Late;
  end PLate.impl;
  process PMove
  end PMove;
  process implementation PMove.impl
    subcomponents
      t1: thread Mover;
  end PMove.impl;
  system Sub
  end Sub;
  system implementation Sub.fine
    subcomponents
      cpu: processor CPU;
      app: process PFine.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.w;
  end Sub.fine;
  system implementation Sub.late
    subcomponents
      cpu: processor CPU;
      app: process PLate.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.w;
  end Sub.late;
  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      s1: system Sub.fine;
      s2: system Sub.late;
      mv: process PMove.impl;
  end Sys.impl;
end Rk;
"#;

const DOTTED: &str = r#"package Rk
public
  processor CPU
    properties
      Scheduling_Protocol => (RMS);
  end CPU;
  thread Fine
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
      Deadline => 10 ms;
  end Fine;
  thread Late
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 50 ms .. 50 ms;
      Deadline => 10 ms;
  end Late;
  thread Mover
  end Mover;
  process PFine
  end PFine;
  process implementation PFine.impl
    subcomponents
      w: thread Fine;
  end PFine.impl;
  process PLate
  end PLate;
  process implementation PLate.impl
    subcomponents
      w: thread Late;
  end PLate.impl;
  process PMove
  end PMove;
  process implementation PMove.impl
    subcomponents
      t1: thread Mover;
  end PMove.impl;
  system Sub
  end Sub;
  system implementation Sub.fine
    subcomponents
      cpu: processor CPU;
      app: process PFine.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.w;
  end Sub.fine;
  system implementation Sub.late
    subcomponents
      cpu: processor CPU;
      app: process PLate.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.w;
  end Sub.late;
  system HW
  end HW;
  system implementation HW.impl
    subcomponents
      cpu: processor CPU;
  end HW.impl;
  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      hw: system HW.impl;
      other: processor CPU;
      app: process PFine.impl;
      mv: process PMove.impl;
    properties
      Actual_Processor_Binding => (reference (hw.cpu)) applies to app.w;
  end Sys.impl;
end Rk;
"#;

fn write(tag: &str, src: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "spar-rank-identity-{}-{tag}.aadl",
        std::process::id()
    ));
    std::fs::write(&path, src).unwrap();
    path
}

/// `(target, max_response_ns)` for every candidate of moving `t1`.
fn ranks(tag: &str, src: &str) -> Vec<(String, Option<i64>)> {
    let path = write(tag, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "moves",
            "enumerate",
            "--root",
            "Rk::Sys.impl",
            "--component",
            "t1",
            "--objective",
            "max-response",
            "--format",
            "json",
        ])
        .arg(&path)
        .output()
        .expect("spar runs");
    let _ = std::fs::remove_file(&path);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "enumerate JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let mut r: Vec<(String, Option<i64>)> = v["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| {
            (
                c["target"].as_str().unwrap().to_string(),
                c["rank"]["max_response_ns"].as_i64(),
            )
        })
        .collect();
    r.sort();
    r
}

// rivet: verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn a_deadline_miss_is_charged_only_to_the_processor_it_runs_on() {
    assert_eq!(
        ranks("collide", COLLIDE),
        vec![
            // s1.cpu runs only the fine thread: 2 ms.
            ("Sys.impl/s1/cpu".to_string(), Some(2_000_000_000)),
            // s2.cpu runs the late thread: the miss sentinel.
            ("Sys.impl/s2/cpu".to_string(), Some(-1)),
        ]
    );
}

// rivet: verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn a_dotted_binding_reaches_the_processor_it_names() {
    assert_eq!(
        ranks("dotted", DOTTED),
        vec![
            ("Sys.impl/hw/cpu".to_string(), Some(2_000_000_000)),
            // Negative control: nothing runs on `other`, so it has no
            // response time, exactly as before.
            ("Sys.impl/other".to_string(), None),
        ]
    );
}
