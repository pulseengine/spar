//! REQ-AADL-MODAL-PROPERTY-VALUES-001 — a property association may give a
//! different value per mode (AS5506 §11.3 modal property values):
//!
//! ```text
//! Period => 10 ms in modes (fast), 40 ms in modes (slow);
//! ```
//!
//! Measured on origin/main 15645a8 before this change: the parser kept every
//! value with its `in modes` clause, but the item-tree lowering stopped at the
//! FIRST clause, so the first value won in every mode, the others were
//! dropped, and nothing said so. `spar analyze --per-som` printed 20.0% for
//! both SOMs of the first model below.
//!
//! The rule (osate2 @ 87468992ee0d4c4cc856a72bd542899953f088f3):
//! - a modal value applies in the modes it lists
//!   (PropertyAssociationImpl.java:510-524, `valueInMode`);
//! - a value with no `in modes` in the same association applies in every
//!   other mode (the `def` of that same method; CachePropertyAssociationsSwitch
//!   .java:328-337 keeps a non-modal value for every SOM);
//! - the modes named are those of the component that DECLARED the
//!   association, selected through the SOM.
//!
//! NON-VACUITY: every test counts the `[mode: …]` SOMs it reads, so a run in
//! which the per-SOM pass printed nothing cannot pass, and every expected
//! number differs between the modes, so a value that ignored the mode fails.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Run `spar analyze --per-som` and return the ANSI-stripped output lines.
fn analyze_per_som(root: &str, name: &str, src: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spar-modal-pv-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file: PathBuf = dir.join(name);
    fs::write(&file, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--per-som",
            "--root",
            root,
            file.to_str().unwrap(),
        ])
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
        if c == '\x1b' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The distinct SOM names that tagged a line of `analysis`.
fn soms_of(lines: &[String], analysis: &str) -> BTreeSet<String> {
    let tag = format!("{analysis}: [mode: ");
    lines
        .iter()
        .filter_map(|l| {
            let rest = &l[l.find(&tag)? + tag.len()..];
            Some(rest[..rest.find(']')?].to_string())
        })
        .collect()
}

/// The one line of `analysis` in SOM `som` that contains `needle`.
fn line_in<'a>(lines: &'a [String], analysis: &str, som: &str, needle: &str) -> &'a str {
    let tag = format!("{analysis}: [mode: {som}] ");
    let hits: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains(&tag) && l.contains(needle))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one `{tag}…{needle}` line:\n{}",
        lines.join("\n")
    );
    hits[0]
}

const UTIL: &str = "processor 'cpu' utilization: ";

// ── 1. A value per mode, on the thread's own modes ──────────────────────

const PER_MODE: &str = r#"
package MpvA
public
  processor Cpu
  end Cpu;
  thread T
    modes
      fast: initial mode;
      slow: mode;
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms in modes (fast), 40 ms in modes (slow);
      Compute_Execution_Time => 2 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t: thread T;
  end P.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      p: process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
  end S.impl;
end MpvA;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn each_mode_uses_its_own_period() {
    let lines = analyze_per_som("MpvA::S.impl", "per_mode.aadl", PER_MODE);
    // Non-vacuity: the thread's two modes make exactly two SOMs.
    assert_eq!(
        soms_of(&lines, "scheduling"),
        BTreeSet::from(["fast".to_string(), "slow".to_string()]),
        "{}",
        lines.join("\n")
    );
    // fast: U = C/T = 2 ms / 10 ms = 0.200 → 20.0%.
    assert!(line_in(&lines, "scheduling", "fast", UTIL).contains("utilization: 20.0% (1 threads"));
    // slow: U = 2 ms / 40 ms = 0.050 → 5.0%. origin/main printed 20.0% here.
    assert!(line_in(&lines, "scheduling", "slow", UTIL).contains("utilization: 5.0% (1 threads"));
}

// ── 2. A value with no `in modes` applies in every other mode ───────────

const DEFAULT_VALUE: &str = r#"
package MpvB
public
  processor Cpu
  end Cpu;
  thread T
    modes
      fast: initial mode;
      slow: mode;
    properties
      Dispatch_Protocol => Periodic;
      Period => 40 ms in modes (slow), 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end T;
  thread U
    modes
      fast: initial mode;
      slow: mode;
    properties
      Dispatch_Protocol => Periodic;
      Period => 25 ms;
      Compute_Execution_Time => 5 ms .. 5 ms;
  end U;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t: thread T;
  end P.impl;
  process Q
  end Q;
  process implementation Q.impl
    subcomponents
      u: thread U;
  end Q.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      cpu2: processor Cpu;
      p: process P.impl;
      q: process Q.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
      Actual_Processor_Binding => (reference (cpu2)) applies to q.u;
  end S.impl;
end MpvB;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn a_value_without_in_modes_applies_in_every_other_mode() {
    let lines = analyze_per_som("MpvB::S.impl", "default_value.aadl", DEFAULT_VALUE);
    // Non-vacuity: t's two modes × u's two modes = four SOMs, named
    // `<t mode>_<u mode>` in instantiation order.
    let soms = soms_of(&lines, "scheduling");
    assert_eq!(soms.len(), 4, "{soms:?}\n{}", lines.join("\n"));
    for som in &soms {
        let t_mode = som.split('_').next().unwrap();
        // The modal association's non-modal value: in `fast`, which no
        // `in modes` names, t's Period is 10 ms → U = 2/10 = 20.0%. In `slow`
        // it is 40 ms → U = 2/40 = 5.0%. origin/main took the FIRST value,
        // 40 ms, in both, so it printed 5.0% for fast.
        let want = if t_mode == "fast" { "20.0%" } else { "5.0%" };
        let got = line_in(&lines, "scheduling", som, "processor 'cpu' utilization: ");
        assert!(got.contains(want), "SOM {som}: want {want}: {got}");
        // A non-modal association applies in every SOM: u on cpu2 is
        // 5 ms / 25 ms = 20.0% whatever the mode.
        let got = line_in(&lines, "scheduling", som, "processor 'cpu2' utilization: ");
        assert!(got.contains("20.0%"), "SOM {som}: {got}");
    }
}

// ── 3. The modes are the DECLARING component's, not the thread's ───────

/// The process declares the modes and the association; the thread has no
/// modes at all. The root system has modes with the SAME names, so a lookup
/// that matched a mode name in any selected mode (rather than the mode the
/// SOM selects for the declarer) would read the system's mode instead.
const DECLARER_MODES: &str = r#"
package MpvC
public
  processor Cpu
  end Cpu;
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t: thread T;
      v: thread T {
        Period => 10 ms in modes (fast), 40 ms in modes (slow);
      };
    modes
      fast: initial mode;
      slow: mode;
    properties
      Period => 10 ms in modes (fast), 40 ms in modes (slow) applies to t;
  end P.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      cpu2: processor Cpu;
      p: process P.impl;
    modes
      fast: initial mode;
      slow: mode;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
      Actual_Processor_Binding => (reference (cpu2)) applies to p.v;
  end S.impl;
end MpvC;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn the_declaring_components_mode_selects_the_value() {
    let lines = analyze_per_som("MpvC::S.impl", "declarer.aadl", DECLARER_MODES);
    // Non-vacuity: system {fast, slow} × process {fast, slow} = four SOMs,
    // named `<system mode>_<process mode>`.
    let soms = soms_of(&lines, "scheduling");
    assert_eq!(
        soms,
        ["fast_fast", "fast_slow", "slow_fast", "slow_slow"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "{}",
        lines.join("\n")
    );
    for som in &soms {
        let process_mode = som.split('_').nth(1).unwrap();
        // The process's mode decides: fast → 2/10 = 20.0%, slow → 2/40 = 5.0%.
        // In fast_slow and slow_fast the system's mode would give the other.
        let want = if process_mode == "fast" {
            "20.0%"
        } else {
            "5.0%"
        };
        // `applies to t` declared in P.impl: P's modes.
        let got = line_in(&lines, "scheduling", som, "processor 'cpu' utilization: ");
        assert!(
            got.contains(want),
            "applies to, SOM {som}: want {want}: {got}"
        );
        // `v: thread T { … }` declared in P.impl's subcomponents: P's modes.
        let got = line_in(&lines, "scheduling", som, "processor 'cpu2' utilization: ");
        assert!(
            got.contains(want),
            "subcomponent, SOM {som}: want {want}: {got}"
        );
    }
}

/// `Period` is an `inherit` property (Timing_Properties.aadl:190): a thread
/// with none takes its process's. A modal value is inherited with its modes,
/// which stay the DECLARING process's — the thread has none of its own.
const INHERITED: &str = r#"
package MpvF
public
  processor Cpu
  end Cpu;
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end T;
  process R
  end R;
  process implementation R.impl
    subcomponents
      x: thread T;
    modes
      fast: initial mode;
      slow: mode;
    properties
      Period => 10 ms in modes (fast), 40 ms in modes (slow);
  end R.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      r: process R.impl;
    modes
      fast: initial mode;
      slow: mode;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to r.x;
  end S.impl;
end MpvF;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn an_inherited_modal_value_keeps_its_declarers_modes() {
    let lines = analyze_per_som("MpvF::S.impl", "inherited.aadl", INHERITED);
    // Non-vacuity: system {fast, slow} × process {fast, slow} = four SOMs.
    let soms = soms_of(&lines, "scheduling");
    assert_eq!(soms.len(), 4, "{soms:?}\n{}", lines.join("\n"));
    for som in &soms {
        // The process's mode decides: fast → 2/10 = 20.0%, slow → 2/40 = 5.0%.
        let want = if som.split('_').nth(1) == Some("fast") {
            "20.0%"
        } else {
            "5.0%"
        };
        let got = line_in(&lines, "scheduling", som, UTIL);
        assert!(got.contains(want), "SOM {som}: want {want}: {got}");
    }
}

// ── 4. RTA reads the SOM's Deadline and Priority ────────────────────────

const RTA_MODAL: &str = r#"
package MpvD
public
  processor Cpu
  end Cpu;
  thread T
    modes
      fast: initial mode;
      slow: mode;
    properties
      Dispatch_Protocol => Periodic;
      Period => 50 ms;
      Compute_Execution_Time => 5 ms .. 5 ms;
      Deadline => 4 ms in modes (fast), 40 ms in modes (slow);
  end T;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 3 ms .. 3 ms;
  end W;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t: thread T;
  end P.impl;
  process Q
  end Q;
  process implementation Q.impl
    subcomponents
      w1: thread W;
      w2: thread W;
    modes
      a: initial mode;
      b: mode;
    properties
      Thread_Properties::Priority => 10 in modes (a), 1 in modes (b) applies to w1;
      Thread_Properties::Priority => 5 applies to w2;
  end Q.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      cpu2: processor Cpu;
      p: process P.impl;
      q: process Q.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
      Actual_Processor_Binding => (reference (cpu2)) applies to q.w1, q.w2;
  end S.impl;
end MpvD;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn rta_uses_the_soms_deadline_and_priority() {
    let lines = analyze_per_som("MpvD::S.impl", "rta.aadl", RTA_MODAL);
    // Non-vacuity: t {fast, slow} × q {a, b} = four SOMs, `<t>_<q>`.
    let soms = soms_of(&lines, "rta");
    assert_eq!(soms.len(), 4, "{soms:?}\n{}", lines.join("\n"));
    for som in &soms {
        let mut parts = som.split('_');
        let (t_mode, q_mode) = (parts.next().unwrap(), parts.next().unwrap());
        // t alone on cpu: R = C = 5 ms. fast: D = 4 ms → miss (the recurrence
        // stops as soon as R passes D, so the line names only the deadline).
        // slow: D = 40 ms → 5 ms <= 40 ms.
        let t = line_in(&lines, "rta", som, "thread 't' on processor 'cpu'");
        if t_mode == "fast" {
            assert!(
                t.contains("misses deadline: response time exceeds deadline 4.00 ms"),
                "SOM {som}: {t}"
            );
        } else {
            assert!(
                t.contains("response time 5.00 ms <= deadline 40.00 ms"),
                "SOM {som}: {t}"
            );
        }
        // w1, w2 on cpu2, both C = 3 ms, T = D = 10 ms. A larger Priority is
        // the higher one (#526). Mode a: w1 = 10 > w2 = 5, so w1 runs first:
        // R(w1) = 3 ms, R(w2) = 3 + 3 = 6 ms. Mode b: w1 = 1 < 5, so
        // R(w2) = 3 ms, R(w1) = 6 ms.
        let (r1, r2) = if q_mode == "a" {
            ("3.00 ms", "6.00 ms")
        } else {
            ("6.00 ms", "3.00 ms")
        };
        let w1 = line_in(&lines, "rta", som, "thread 'w1' on processor 'cpu2'");
        assert!(
            w1.contains(&format!("response time {r1} <=")),
            "SOM {som}: {w1}"
        );
        let w2 = line_in(&lines, "rta", som, "thread 'w2' on processor 'cpu2'");
        assert!(
            w2.contains(&format!("response time {r2} <=")),
            "SOM {som}: {w2}"
        );
    }
}

// ── 5. No modal value: --per-som adds no RTA pass ───────────────────────

/// Modes, but every property value is non-modal. The per-SOM RTA pass runs
/// only when a modal value can make a SOM differ, so the `--per-som` output of
/// a model like this is what it was before modal values were supported.
const MODES_NO_MODAL_VALUE: &str = r#"
package MpvE
public
  processor Cpu
  end Cpu;
  thread T
    modes
      fast: initial mode;
      slow: mode;
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t: thread T;
  end P.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu: processor Cpu;
      p: process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p.t;
  end S.impl;
end MpvE;
"#;

// rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
#[test]
fn without_a_modal_value_per_som_output_is_unchanged() {
    let lines = analyze_per_som("MpvE::S.impl", "no_modal.aadl", MODES_NO_MODAL_VALUE);
    // Non-vacuity: the per-SOM pass ran (scheduling is modal) over two SOMs,
    // and a non-modal value applies in both: U = 2/10 = 20.0%.
    let soms = soms_of(&lines, "scheduling");
    assert_eq!(soms.len(), 2, "{}", lines.join("\n"));
    for som in &soms {
        assert!(line_in(&lines, "scheduling", som, UTIL).contains("20.0%"));
    }
    // The default RTA pass ran …
    assert!(
        lines
            .iter()
            .any(|l| l.contains("rta: thread 't' on processor 'cpu'")),
        "{}",
        lines.join("\n")
    );
    // … and no per-SOM one.
    assert!(soms_of(&lines, "rta").is_empty(), "{}", lines.join("\n"));
}
