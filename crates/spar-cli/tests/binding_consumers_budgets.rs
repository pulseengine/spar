//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — the memory and bus budgets count
//! a component against the memory or bus its binding NAMES, by identity, not
//! by matching the binding's text against every component's name.
//!
//! The model has two DIFFERENT memories, both named `ram` (one per node), each
//! with one 60 KB process bound to it, and two buses, `net` and `can_net`, with
//! one link bound to `can_net`. On the published v0.46.0 binary:
//!
//! - memory_budget merges the two `ram`s: "memory 'ram' budget exceeded:
//!   120.0 KB demand vs 100.0 KB capacity" on BOTH, which is false;
//! - resource_budget matched the bus binding as a SUBSTRING, so the link
//!   bound to `can_net` also counted against `net`: "bus 'net' bandwidth may
//!   be exceeded", which is false.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const BUDGETS: &str = r#"package Budgets
public
  bus Can
  end Can;
  memory Ram
    properties
      Memory_Size => 100 KByte;
  end Ram;
  process P
    properties
      Code_Size => 30 KByte;
      Data_Size => 30 KByte;
  end P;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      ram : memory Ram;
      app : process P;
    properties
      Actual_Memory_Binding => (reference (ram)) applies to app;
  end Node.impl;
  system Link
  end Link;
  system implementation Link.impl
    properties
      Actual_Connection_Binding => (reference (can_net));
      Data_Rate => 5 MBytesps;
  end Link.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      net : bus Can { Data_Rate => 1 MBytesps; };
      can_net : bus Can { Data_Rate => 10 MBytesps; };
      left : system Node.impl;
      right : system Node.impl;
      link : system Link.impl;
  end Top.impl;
end Budgets;
"#;

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-bind-budgets-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

fn strip_ansi(l: &str) -> String {
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
}

/// `memory_budget` / `resource_budget` lines of `spar analyze`.
fn budget_lines(name: &str, src: &str) -> Vec<String> {
    let file = tmp(name, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "Budgets::Top.impl",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let lines: Vec<String> = text
        .lines()
        .map(strip_ansi)
        .filter(|l| l.contains("memory_budget:") || l.contains("resource_budget:"))
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("memory_budget:")),
        "memory_budget must have run on {name}: {text}"
    );
    lines
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn two_memories_with_one_name_are_budgeted_separately() {
    let lines = budget_lines("mem.aadl", BUDGETS);
    assert!(
        !lines.iter().any(|l| l.contains("budget exceeded")),
        "false overrun from merging two `ram`s: {lines:#?}"
    );
    for side in ["Top.impl/left/ram", "Top.impl/right/ram"] {
        assert!(
            lines.iter().any(
                |l| l.contains("memory_budget: memory 'ram' utilization: 60.0 KB")
                    && l.contains("1 bound component")
                    && l.contains(side)
            ),
            "{side} must carry exactly its own process: {lines:#?}"
        );
        // resource_budget counts the same binding; it must agree.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("resource_budget: memory 'ram' utilization:")
                    && l.contains("1 bound components")
                    && l.contains(side)),
            "resource_budget: {side} must carry exactly its own process: {lines:#?}"
        );
    }
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_bus_binding_is_not_a_substring_match() {
    let lines = budget_lines("bus.aadl", BUDGETS);
    assert!(
        !lines.iter().any(|l| l.contains("bus 'net'")),
        "a link bound to `can_net` must not count against `net`: {lines:#?}"
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_bus_binding_still_counts_against_the_bus_it_names() {
    // Positive control: push the link above `can_net`'s 10 MBytesps. The
    // warning must appear, on `can_net` and only there — so a build that
    // simply stopped counting bus bindings cannot pass the test above.
    let over = BUDGETS.replace("Data_Rate => 5 MBytesps;", "Data_Rate => 20 MBytesps;");
    assert_ne!(over, BUDGETS);
    let lines = budget_lines("bus_over.aadl", &over);
    let warned: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("bandwidth may be exceeded"))
        .collect();
    assert_eq!(warned.len(), 1, "exactly one bus is over: {lines:#?}");
    assert!(warned[0].contains("bus 'can_net'"), "{warned:#?}");
}
