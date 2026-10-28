//! REQ-AADL-PROPERTY-ARITHMETIC-001 (#546) — an arithmetic property value is
//! EVALUATED, never truncated to its first operand.
//!
//! On v0.47.1 `Period => 5 * 8 ms` lowered to `Integer(5)` with no unit, which
//! the timing accessors read as 5 ps: `spar analyze` reported a 40000000000%
//! overload and a missed 5 ps deadline on a thread that runs 2 ms every 40 ms.
//!
//! The oracle is the literal spelling of the same value. Every arithmetic
//! spelling of 40 ms must give output IDENTICAL to `Period => 40 ms` in the
//! scheduling and RTA lines, and the 40 ms run itself is hand-checked:
//! 2 ms of Compute_Execution_Time over a 40 ms period is 2/40 = 5.0%.
//!
//! NON-VACUITY:
//! - `witness` lines: each run must print the analysis lines it is compared
//!   on, so two empty outputs cannot compare equal.
//! - Negative controls: `5 ms * 8 ms` (unit times unit), `5 ms + 3 KByte`
//!   (time plus size) and `40 ms / 0` must each give an ERROR diagnostic AND
//!   no value at all — scheduling must say the thread has no Period and must
//!   print no utilization. On v0.47.1 the first two silently became `5 ms`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One thread `t` bound to processor `cpu`, with `period` and `cet` spliced in.
fn model(period: &str, cet: &str) -> String {
    format!(
        r#"
package Arith
public
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Period => {period};
      Compute_Execution_Time => {cet};
  end T;
  thread implementation T.i
  end T.i;
  processor Cpu
  end Cpu;
  process P
  end P;
  process implementation P.i
    subcomponents
      t : thread T.i;
  end P.i;
  system S
  end S;
  system implementation S.i
    subcomponents
      cpu : processor Cpu;
      p : process P.i;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
  end S.i;
end Arith;
"#
    )
}

/// A fresh file per call: the tests run in parallel, and a shared name would
/// let one test analyse another's model.
fn tmp(name: &str, src: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-arith-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}-{name}", NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::write(&path, src).unwrap();
    path
}

/// `spar analyze` on the model, ANSI-stripped, stdout then stderr.
fn analyze(name: &str, period: &str, cet: &str) -> Vec<String> {
    let file = tmp(name, &model(period, cet));
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Arith::S.i", file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    strip_ansi(&text).lines().map(str::to_string).collect()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The scheduling and RTA lines — the verdicts the bug falsified.
fn timing_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.contains("scheduling:") || l.contains("rta:"))
        .cloned()
        .collect()
}

/// The reference run: the literal `40 ms`, hand-checked.
fn reference(cet: &str) -> Vec<String> {
    let lines = timing_lines(&analyze("literal.aadl", "40 ms", cet));
    assert!(
        lines.iter().any(|l| l.contains("utilization")),
        "reference run printed no utilization line — nothing to compare:\n{}",
        lines.join("\n")
    );
    lines
}

fn assert_same_as_literal(period: &str, cet: &str, reference: &[String]) {
    let got = timing_lines(&analyze("expr.aadl", period, cet));
    assert_eq!(
        got,
        reference,
        "`Period => {period}` must analyse exactly like `Period => 40 ms`\n--- got ---\n{}\n--- want ---\n{}",
        got.join("\n"),
        reference.join("\n")
    );
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn five_times_eight_ms_is_forty_ms() {
    let reference = reference("2 ms .. 2 ms");
    // Hand-check of the oracle itself: 2 ms / 40 ms = 5.0 %.
    assert!(
        reference
            .iter()
            .any(|l| l.contains("processor 'cpu' utilization: 5.0%")),
        "2 ms CET over a 40 ms period is 5.0% utilization:\n{}",
        reference.join("\n")
    );
    assert!(
        !reference
            .iter()
            .any(|l| l.contains("[ERROR]") || l.contains("overloaded")),
        "the 40 ms reference must be schedulable:\n{}",
        reference.join("\n")
    );
    assert_same_as_literal("5 * 8 ms", "2 ms .. 2 ms", &reference);
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn sums_groups_unit_conversion_and_division_are_forty_ms() {
    let reference = reference("2 ms .. 2 ms");
    for period in [
        "20 ms + 20 ms",
        "(2 + 3) * 8 ms",
        "8 ms * 5",
        "1 sec - 960 ms",
        "80 ms / 2",
        "-(-40 ms)",
        "40 ms + 2 * 0 ms",
        "2 * 4 ms + 4 ms * 8",
    ] {
        assert_same_as_literal(period, "2 ms .. 2 ms", &reference);
    }
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_range_bound_may_be_an_expression() {
    let reference = reference("1 ms .. 2 ms");
    for cet in [
        "1 ms .. 2 * 1 ms",
        "1 ms .. 1 ms + 1000 us",
        "2 * 500 us .. 2 ms",
        "(1 ms) .. 4 ms / 2",
    ] {
        let got = timing_lines(&analyze("range.aadl", "40 ms", cet));
        assert_eq!(
            got,
            reference,
            "`Compute_Execution_Time => {cet}` must analyse exactly like `1 ms .. 2 ms`\n--- got ---\n{}\n--- want ---\n{}",
            got.join("\n"),
            reference.join("\n")
        );
    }
}

/// A thread bound to processor `cpu` and memory `ram`, with the three size
/// properties the memory and resource budgets read spliced in.
fn memory_model(memory_size: &str, code: &str, data: &str) -> String {
    format!(
        r#"
package Mem
public
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 1 ms;
      Code_Size => {code};
      Data_Size => {data};
  end T;
  thread implementation T.i
  end T.i;
  processor Cpu
  end Cpu;
  memory Ram
    properties
      Memory_Size => {memory_size};
  end Ram;
  process P
  end P;
  process implementation P.i
    subcomponents
      t : thread T.i;
  end P.i;
  system S
  end S;
  system implementation S.i
    subcomponents
      cpu : processor Cpu;
      ram : memory Ram;
      p : process P.i;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
      Actual_Memory_Binding => (reference (ram)) applies to p.t;
  end S.i;
end Mem;
"#
    )
}

fn budget_lines(memory_size: &str, code: &str, data: &str) -> Vec<String> {
    let file = tmp("mem.aadl", &memory_model(memory_size, code, data));
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Mem::S.i", file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    strip_ansi(&text)
        .lines()
        .filter(|l| l.contains("memory_budget:") || l.contains("resource_budget:"))
        .map(str::to_string)
        .collect()
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn memory_and_resource_budgets_read_the_evaluated_size() {
    let reference = budget_lines("100 KByte", "60 KByte", "10 KByte");
    // Hand-check: 60 KByte code + 10 KByte data in a 100 KByte memory.
    assert!(
        reference
            .iter()
            .any(|l| l.contains("memory 'ram' utilization: 70.0 KB of 100.0 KB")),
        "reference memory budget is 70 of 100 KB:\n{}",
        reference.join("\n")
    );
    let got = budget_lines("1 MByte - 924 KByte", "6 * 10 KByte", "20 KByte / 2");
    assert_eq!(
        got,
        reference,
        "arithmetic sizes must budget exactly like their literals\n--- got ---\n{}\n--- want ---\n{}",
        got.join("\n"),
        reference.join("\n")
    );
}

/// An ill-typed expression: an ERROR naming the property, and NO value.
fn assert_rejected(period: &str, why: &str) {
    let lines = analyze("bad.aadl", period, "2 ms .. 2 ms");
    let diag = lines
        .iter()
        .find(|l| l.contains("[ERROR]") && l.contains("Period") && l.contains("arithmetic"));
    assert!(
        diag.is_some_and(|l| l.contains(why)),
        "`Period => {period}` must be rejected with an ERROR containing `{why}`:\n{}",
        lines.join("\n")
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("thread 't' has no Period property")),
        "`Period => {period}` must give NO value (a truncated operand is a value):\n{}",
        lines.join("\n")
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("utilization") || l.contains("misses deadline")),
        "no timing verdict may be computed from `Period => {period}`:\n{}",
        lines.join("\n")
    );
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn unit_times_unit_is_rejected_with_no_value() {
    assert_rejected("5 ms * 8 ms", "ms * ms");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn time_plus_size_is_rejected_with_no_value() {
    assert_rejected("5 ms + 3 KByte", "ms + KByte");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn division_by_zero_is_rejected_with_no_value() {
    assert_rejected("40 ms / 0", "division by zero");
}
