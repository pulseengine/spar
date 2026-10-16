//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — scheduling and RTA group threads by
//! the processor their binding RESOLVES to, from the component that declared
//! it (REQ-AADL-REF-CONTEXT-001), not by the binding's text.
//!
//! Both models give each processor exactly the threads bound to it, at 6 ms of
//! every 10 ms (60 %). Grouping by text is wrong in both directions:
//!
//! - `TwoCpu`: two DIFFERENT processors, both named `cpu`, one thread each.
//!   The published v0.46.0 binary merges them and reports a false
//!   "processor 'cpu' is overloaded: utilization 120.0%", plus a false RTA
//!   deadline miss.
//! - `OneCpu`: ONE processor per node, named `cpu` by `Node.impl` and
//!   `left.cpu` / `right.cpu` by `Top.impl`, two threads each — a real 120 %
//!   overload on BOTH processors. v0.46.0 splits each into a 60 % group and
//!   reports right/cpu's overload nowhere (its one "overloaded" line pairs the
//!   two different `t1`s).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const TWO_CPUS_SAME_NAME: &str = r#"package TwoCpu
public
  processor Cpu
  end Cpu;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 6 ms .. 6 ms;
  end W;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread W;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
  end Top.impl;
end TwoCpu;
"#;

const ONE_CPU_TWO_SPELLINGS: &str = r#"package OneCpu
public
  processor Cpu
  end Cpu;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 6 ms .. 6 ms;
  end W;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread W;
      t2 : thread W;
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
      Actual_Processor_Binding => (reference (left.cpu)) applies to left.app.t2;
      Actual_Processor_Binding => (reference (right.cpu)) applies to right.app.t2;
  end Top.impl;
end OneCpu;
"#;

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-bind-consumers-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// `scheduling` / `rta` diagnostic lines of `spar analyze`, ANSI-stripped.
fn sched_lines(root: &str, name: &str, src: &str) -> Vec<String> {
    let file = tmp(name, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let lines: Vec<String> = text
        .lines()
        .map(|l| {
            let mut s = String::new();
            let mut esc = false;
            for c in l.chars() {
                match (esc, c) {
                    (false, '\x1b') => esc = true,
                    (true, 'm') => esc = false,
                    (true, _) => {}
                    (false, c) => s.push(c),
                }
            }
            s
        })
        .filter(|l| l.contains("scheduling:") || l.contains("rta:"))
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("utilization:")),
        "scheduling must have run and reported utilization on {name}: {text}"
    );
    lines
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn two_processors_with_one_name_are_scheduled_separately() {
    let lines = sched_lines("TwoCpu::Top.impl", "two.aadl", TWO_CPUS_SAME_NAME);
    let overloaded: Vec<_> = lines.iter().filter(|l| l.contains("overloaded")).collect();
    assert!(
        overloaded.is_empty(),
        "false overload from merging two cpus: {overloaded:#?}"
    );
    let misses: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("misses deadline"))
        .collect();
    assert!(
        misses.is_empty(),
        "false RTA miss from merging two cpus: {misses:#?}"
    );
    for side in ["Top.impl/left/cpu", "Top.impl/right/cpu"] {
        assert!(
            lines
                .iter()
                .any(|l| l.contains("utilization: 60.0% (1 threads") && l.contains(side)),
            "{side} must carry exactly its own thread: {lines:#?}"
        );
    }
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn one_processor_named_two_ways_is_one_processor() {
    let lines = sched_lines("OneCpu::Top.impl", "one.aadl", ONE_CPU_TWO_SPELLINGS);
    for side in ["Top.impl/left/cpu", "Top.impl/right/cpu"] {
        assert!(
            lines.iter().any(
                |l| l.contains("is overloaded: utilization 120.0% (2 threads") && l.contains(side)
            ),
            "the real overload on {side} must be reported: {lines:#?}"
        );
    }
    // No group may be keyed by a spelling instead of a processor.
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("'left.cpu'") || l.contains("'right.cpu'")),
        "a spelling must not form its own processor group: {lines:#?}"
    );
}
