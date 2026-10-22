//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — `spar allocate` honours an
//! existing Actual_Processor_Binding, resolved from the component that
//! declared it.
//!
//! The solver keyed processors by their dot path (`Top.impl.cpu2`) but kept a
//! thread's binding as its raw target (`cpu2`). The two never matched, so on
//! the published v0.46.0 binary `(reference (cpu2)) applies to app.t` gave
//! "warning: Thread 'Top.impl.app.t' pre-bound to unknown processor 'cpu2'"
//! and the thread was left UNALLOCATED.

use std::process::Command;

const ALLOC: &str = r#"package Alloc
public
  processor Cpu
  end Cpu;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end W;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread W;
  end P.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu1 : processor Cpu;
      cpu2 : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu2)) applies to app.t;
  end Top.impl;
end Alloc;
"#;

fn allocate(name: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("spar-bind-alloc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "allocate",
            "--root",
            "Alloc::Top.impl",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Per-processor utilization"),
        "allocate must report: {text}"
    );
    text
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_pre_bound_thread_stays_on_the_processor_its_binding_names() {
    let text = allocate("alloc.aadl", ALLOC);
    assert!(!text.contains("unknown processor"), "{text}");
    assert!(!text.contains("Unallocated threads"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.starts_with("Top.impl.app.t") && l.contains("Top.impl.cpu2")),
        "t must be placed on cpu2: {text}"
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_binding_to_a_missing_processor_is_still_unknown() {
    // Negative control: `cpu9` does not exist.
    let text = allocate(
        "alloc_missing.aadl",
        &ALLOC.replace("(reference (cpu2))", "(reference (cpu9))"),
    );
    assert!(
        text.contains("pre-bound to unknown processor 'cpu9'"),
        "{text}"
    );
}
