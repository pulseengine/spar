//! REQ-AADL-PROPERTY-ARITHMETIC-001 (#546) — arithmetic in a property value
//! is not AADL: it is an ERROR and the value is IGNORED, never evaluated and
//! never truncated to its first operand.
//!
//! On v0.47.1 `Period => 5 * 8 ms` lowered to `Integer(5)` with no unit, which
//! the timing accessors read as 5 ps: `spar analyze` reported a 40000000000%
//! overload and a missed 5 ps deadline on a thread that runs 2 ms every 40 ms.
//! AS5506 numeric terms have no operators and OSATE rejects the syntax, so
//! the maintainer's decision is reject + warn.
//!
//! NON-VACUITY:
//! - Every rejected spelling must give the ERROR AND "has no Period
//!   property" AND no utilization or deadline verdict; on v0.47.1 each of
//!   them silently produced a number (5 ps, 20 ms, a list, 5 ms, 40 ms…).
//! - Positive controls: the literal `40 ms` is hand-checked (2 ms / 40 ms =
//!   5.0%) and gives no ERROR, so a build that rejected every Period fails;
//!   `-180 .. 180` keeps its minus sign and a signed constant `-Max_Lon`
//!   lowers to its name, neither reported as arithmetic. (Both range tests
//!   are red on v0.47.1 too: there the sign wrapped the whole range, giving
//!   min 180 and min `Opaque("-Max_Lon")`.)

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One thread `t` bound to processor `cpu`, with `period` and `cet` spliced
/// in, plus a data type carrying `range` as its Integer_Range.
fn model(period: &str, cet: &str, range: &str) -> String {
    format!(
        r#"
package Arith
public
  with Data_Model;
  data Lon
    properties
      Data_Model::Integer_Range => {range};
  end Lon;
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
      lon : data Lon;
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
fn tmp(src: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-arith-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.aadl", NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::write(&path, src).unwrap();
    path
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

fn spar(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar binary runs");
    strip_ansi(&format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// `spar analyze` on the model, ANSI-stripped, stdout then stderr.
fn analyze(period: &str, cet: &str, range: &str) -> Vec<String> {
    let file = tmp(&model(period, cet, range));
    spar(&["analyze", "--root", "Arith::S.i", file.to_str().unwrap()])
        .lines()
        .map(str::to_string)
        .collect()
}

const NOT_AADL: &str = "arithmetic in a property value is not AADL";

fn arithmetic_errors<'a>(lines: &'a [String], property: &str) -> Vec<&'a String> {
    lines
        .iter()
        .filter(|l| {
            l.contains("[ERROR]")
                && l.contains(NOT_AADL)
                && l.contains("value ignored")
                && l.contains(&format!("property '{property}'"))
        })
        .collect()
}

fn no_timing_verdict(lines: &[String], what: &str) {
    assert!(
        !lines.iter().any(|l| l.contains("utilization")
            || l.contains("misses deadline")
            || l.contains("response time")),
        "no timing verdict may be computed from {what}:\n{}",
        lines.join("\n")
    );
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn the_literal_forty_ms_is_unchanged() {
    let lines = analyze("40 ms", "2 ms .. 2 ms", "-180 .. 180");
    // Hand-check: 2 ms CET over a 40 ms period is 5.0 %.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("processor 'cpu' utilization: 5.0%")),
        "2 ms CET over a 40 ms period is 5.0% utilization:\n{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|l| l.contains("response time 2.00 ms")),
        "the RTA verdict for the literal must be computed:\n{}",
        lines.join("\n")
    );
    assert!(
        !lines.iter().any(|l| l.contains(NOT_AADL)),
        "a literal and a signed range are not arithmetic:\n{}",
        lines.join("\n")
    );
}

/// `Period => {period}` must be rejected with the ERROR and give NO value.
fn assert_period_rejected(period: &str) {
    let lines = analyze(period, "2 ms .. 2 ms", "-180 .. 180");
    let errs = arithmetic_errors(&lines, "Period");
    assert_eq!(
        errs.len(),
        1,
        "`Period => {period}` must give exactly one PROP-ARITHMETIC ERROR:\n{}",
        lines.join("\n")
    );
    assert!(
        errs[0].contains(&format!("'{period}'")),
        "the ERROR must quote the whole expression `{period}`: {}",
        errs[0]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("thread 't' has no Period property")),
        "`Period => {period}` must give NO value (an operand is a value):\n{}",
        lines.join("\n")
    );
    no_timing_verdict(&lines, &format!("`Period => {period}`"));
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn five_times_eight_ms_is_rejected_not_five_ps() {
    assert_period_rejected("5 * 8 ms");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_sum_is_rejected() {
    assert_period_rejected("20 ms + 20 ms");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_parenthesised_product_is_rejected() {
    assert_period_rejected("(2 + 3) * 8 ms");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn unit_times_unit_is_rejected() {
    assert_period_rejected("5 ms * 8 ms");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn division_by_zero_is_rejected() {
    assert_period_rejected("40 ms / 0");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_signed_group_is_rejected() {
    assert_period_rejected("-(40 ms)");
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn an_expression_in_a_range_bound_is_rejected() {
    // `(1 ms) .. 2 ms`: a parenthesised bound is a grouping, not AADL (a
    // parse error on v0.47.1); it must not be read through its other bound.
    for cet in ["1 ms .. 2 * 1 ms", "2 * 500 us .. 2 ms", "(1 ms) .. 2 ms"] {
        let lines = analyze("40 ms", cet, "-180 .. 180");
        assert_eq!(
            arithmetic_errors(&lines, "Compute_Execution_Time").len(),
            1,
            "`Compute_Execution_Time => {cet}` must give one PROP-ARITHMETIC ERROR:\n{}",
            lines.join("\n")
        );
        // The whole range is ignored: no bound of it reaches scheduling.
        // (v0.47.1 read `1 ms .. 2 * 1 ms` as a 2 ps .. 1 ms envelope.)
        assert!(
            lines
                .iter()
                .any(|l| l.contains("thread 't' has no Compute_Execution_Time property")),
            "`Compute_Execution_Time => {cet}` must give NO value:\n{}",
            lines.join("\n")
        );
        no_timing_verdict(&lines, &format!("`Compute_Execution_Time => {cet}`"));
    }
}

/// The lowered value of `Data_Model::Integer_Range` on data type `Lon`.
fn lowered_range(range: &str) -> String {
    let file = tmp(&model("40 ms", "2 ms .. 2 ms", range));
    let json = spar(&["items", "--format", "json", file.to_str().unwrap()]);
    let at = json
        .find("Data_Model::Integer_Range")
        .unwrap_or_else(|| panic!("no Integer_Range in items output:\n{json}"));
    let tail = &json[at..];
    let end = tail.find("is_append").unwrap_or(tail.len());
    tail[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_signed_range_minimum_keeps_its_sign() {
    let got = lowered_range("-180 .. 180");
    assert!(
        got.contains(r#""min": { "Integer": [ -180, null ] }"#)
            && got.contains(r#""max": { "Integer": [ 180, null ] }"#),
        "`-180 .. 180` must lower to min -180, max 180: {got}"
    );
}

// rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
#[test]
fn a_signed_constant_lowers_as_before_and_is_not_arithmetic() {
    // A bare `-C` lowered to `Enum(C)` on v0.47.1 (constant values are not
    // resolved). As a range minimum it was `Opaque("-Max_Lon")` there,
    // because the sign wrapped the whole range; now it lowers like a bare
    // signed constant.
    let got = lowered_range("-Max_Lon .. Max_Lon");
    assert!(
        got.contains(r#""min": { "Enum": "Max_Lon" }"#),
        "a signed constant lowers to its name, as a bare one did: {got}"
    );
    let lines = analyze("40 ms", "2 ms .. 2 ms", "-Max_Lon .. Max_Lon");
    assert!(
        !lines.iter().any(|l| l.contains(NOT_AADL)),
        "a signed constant is legal AADL, not arithmetic:\n{}",
        lines.join("\n")
    );
}
