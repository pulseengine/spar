//! REQ-RTA-PRIORITY-ORDER-001 (#526) — under AADL, a LARGER `Priority` value
//! is the HIGHER priority.
//!
//! `Thread_Properties::Priority` in the predeclared property set (osate2 @
//! 87468992ee0d4c4cc856a72bd542899953f088f3,
//! `core/org.osate.aadl2.contrib/resources/properties/Predeclared_Property_Sets/Thread_Properties.aadl`,
//! lines 41-44): "A larger value represents a higher priority."
//!
//! spar's `rta` pass sorted a lower number first, so a thread the author made
//! the most important was analysed as the least important.
//!
//! Every response time below is hand-computed with the standard recurrence
//! R = C + Σ_{j ∈ hp} ceil(R / T_j) · C_j, iterated from R = C to a fixed
//! point (no jitter, no blocking, no ISRs). spar stops iterating once R
//! passes the deadline, so a miss prints "response time exceeds deadline D"
//! rather than the fixed point.
//!
//! NON-VACUITY: each test first asserts that the `rta` pass ran and printed a
//! response time for every thread, so "no error lines" can never pass on an
//! empty run. `priority_overrides_rate_monotonic` is the case that separates
//! "AADL order" from "Priority ignored": there the two disagree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Build a two-thread model on one processor. Each thread is
/// `(name, priority, period, wcet, deadline)`; `priority = None` omits the
/// property.
fn model(pkg: &str, threads: &[(&str, Option<u32>, &str, &str, &str)]) -> String {
    model_spelled(pkg, "Priority", threads)
}

/// [`model`], writing the priority property as `prio_prop` (for instance the
/// qualified `Thread_Properties::Priority`) instead of the bare `Priority`.
fn model_spelled(
    pkg: &str,
    prio_prop: &str,
    threads: &[(&str, Option<u32>, &str, &str, &str)],
) -> String {
    let mut types = String::new();
    let mut subs = String::new();
    let mut binds = String::new();
    for (name, prio, period, wcet, deadline) in threads {
        let prio_line = prio
            .map(|p| format!("      {prio_prop} => {p};\n"))
            .unwrap_or_default();
        types.push_str(&format!(
            "  thread T_{name}\n    properties\n      Dispatch_Protocol => Periodic;\n      \
             Period => {period};\n      Compute_Execution_Time => {wcet} .. {wcet};\n      \
             Deadline => {deadline};\n{prio_line}  end T_{name};\n"
        ));
        subs.push_str(&format!("      {name} : thread T_{name};\n"));
        binds.push_str(&format!(
            "      Actual_Processor_Binding => (reference (cpu)) applies to app.{name};\n"
        ));
    }
    format!(
        "package {pkg}\npublic\n  processor Cpu\n  end Cpu;\n{types}  process P\n  end P;\n  \
         process implementation P.impl\n    subcomponents\n{subs}  end P.impl;\n  \
         system Top\n  end Top;\n  system implementation Top.impl\n    subcomponents\n      \
         cpu : processor Cpu;\n      app : process P.impl;\n    properties\n{binds}  \
         end Top.impl;\nend {pkg};\n"
    )
}

/// Run `spar analyze` and return the `rta` lines that name a thread.
fn rta_lines(pkg: &str, src: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spar-rta-prio-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file: PathBuf = dir.join(format!("{pkg}.aadl"));
    fs::write(&file, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            &format!("{pkg}::Top.impl"),
            file.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let lines: Vec<String> = stdout
        .lines()
        .chain(stderr.lines())
        .filter(|l| l.contains("rta") && l.contains("thread '") && l.contains("response time"))
        .map(str::to_string)
        .collect();
    // Non-vacuity: the rta pass ran and computed a response time per thread.
    assert_eq!(
        lines.len(),
        2,
        "rta must print one response-time line per thread on {pkg}:\n{stdout}{stderr}"
    );
    lines
}

fn line_for<'a>(lines: &'a [String], thread: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(&format!("thread '{thread}'")))
        .unwrap_or_else(|| panic!("no rta line for '{thread}': {lines:#?}"))
}

// rivet: verifies REQ-RTA-PRIORITY-ORDER-001
#[test]
fn larger_priority_value_preempts() {
    // hi: Priority 10, T = 10 ms, C = 2 ms, D = 10 ms.
    // lo: Priority 1,  T = 100 ms, C = 20 ms, D = 30 ms.
    //
    // AADL order (hi pre-empts lo):
    //   R_hi = 2 ms (no hp).
    //   R_lo: 20 → 20 + ceil(20/10)·2 = 24 → 20 + ceil(24/10)·2 = 26
    //         → 20 + ceil(26/10)·2 = 26. R_lo = 26 ms <= 30 ms.
    // Inverted order (lo pre-empts hi):
    //   R_lo = 20 ms.
    //   R_hi: 2 → 2 + ceil(2/100)·20 = 22 → 22. R_hi = 22 ms > 10 ms: MISS.
    let src = model(
        "PrioOrder",
        &[
            ("hi", Some(10), "10 ms", "2 ms", "10 ms"),
            ("lo", Some(1), "100 ms", "20 ms", "30 ms"),
        ],
    );
    let lines = rta_lines("PrioOrder", &src);
    let hi = line_for(&lines, "hi");
    let lo = line_for(&lines, "lo");
    assert!(
        !hi.contains("misses deadline")
            && hi.contains("response time 2.00 ms <= deadline 10.00 ms"),
        "hi (Priority 10) must pre-empt lo and meet its deadline: {hi}"
    );
    assert!(
        !lo.contains("misses deadline")
            && lo.contains("response time 26.00 ms <= deadline 30.00 ms"),
        "lo (Priority 1) must be pre-empted by hi: {lo}"
    );
}

// rivet: verifies REQ-RTA-PRIORITY-ORDER-001
#[test]
fn priority_overrides_rate_monotonic() {
    // urgent: Priority 10, T = 100 ms, C = 20 ms, D = 100 ms.
    // fast:   Priority 1,  T = 10 ms,  C = 2 ms,  D = 10 ms.
    //
    // AADL order (urgent pre-empts fast, against RM):
    //   R_urgent = 20 ms <= 100 ms.
    //   R_fast: 2 → 2 + ceil(2/100)·20 = 22 → 22. R_fast = 22 ms > 10 ms: MISS.
    // Inverted order, and Priority ignored (RM), both put fast first:
    //   R_fast = 2 ms; R_urgent: 20 → 24 → 26 → 26 = 26 ms. No miss.
    let src = model(
        "PrioOverRm",
        &[
            ("urgent", Some(10), "100 ms", "20 ms", "100 ms"),
            ("fast", Some(1), "10 ms", "2 ms", "10 ms"),
        ],
    );
    let lines = rta_lines("PrioOverRm", &src);
    let urgent = line_for(&lines, "urgent");
    let fast = line_for(&lines, "fast");
    assert!(
        urgent.contains("response time 20.00 ms <= deadline 100.00 ms"),
        "urgent (Priority 10) must be analysed with no interference: {urgent}"
    );
    assert!(
        fast.contains("misses deadline: response time exceeds deadline 10.00 ms"),
        "fast (Priority 1) must be pre-empted by urgent and miss: {fast}"
    );
}

// rivet: verifies REQ-RTA-PRIORITY-ORDER-001
#[test]
fn rate_monotonic_control_without_priority_is_unchanged() {
    // The hi/lo model with NO Priority: rate-monotonic puts the 10 ms
    // thread first, so the numbers are those of the AADL-order case above:
    // R_hi = 2 ms, R_lo = 26 ms. This verdict holds before and after #526.
    let src = model(
        "RmControl",
        &[
            ("hi", None, "10 ms", "2 ms", "10 ms"),
            ("lo", None, "100 ms", "20 ms", "30 ms"),
        ],
    );
    let lines = rta_lines("RmControl", &src);
    assert!(
        line_for(&lines, "hi").contains("response time 2.00 ms <= deadline 10.00 ms"),
        "{lines:#?}"
    );
    assert!(
        line_for(&lines, "lo").contains("response time 26.00 ms <= deadline 30.00 ms"),
        "{lines:#?}"
    );
}

// rivet: verifies REQ-RTA-PRIORITY-ORDER-001
#[test]
fn a_declared_priority_outranks_an_undeclared_one() {
    // Mixed: only the long-period thread declares Priority. A declared
    // priority ranks above an undeclared one (unchanged by #526), so it is
    // analysed first, against RM:
    //   R_slow = 20 ms <= 100 ms.
    //   R_fast: 2 → 22 → 22 ms > 10 ms: MISS.
    let src = model(
        "PrioMixed",
        &[
            ("slow", Some(1), "100 ms", "20 ms", "100 ms"),
            ("fast", None, "10 ms", "2 ms", "10 ms"),
        ],
    );
    let lines = rta_lines("PrioMixed", &src);
    assert!(
        line_for(&lines, "slow").contains("response time 20.00 ms <= deadline 100.00 ms"),
        "{lines:#?}"
    );
    assert!(
        line_for(&lines, "fast")
            .contains("misses deadline: response time exceeds deadline 10.00 ms"),
        "{lines:#?}"
    );
}

// ── #529: the property's own qualified name ─────────────────────────────
//
// `Priority` is declared in `Thread_Properties` (osate2 @
// 87468992ee0d4c4cc856a72bd542899953f088f3, `Thread_Properties.aadl` line
// 44). A model that writes the qualified name must get the same analysis as
// one that writes the bare name. Before #529 the qualified spelling was not
// read at all, so these threads silently fell back to rate-monotonic order.

/// The `urgent`/`fast` model of `priority_overrides_rate_monotonic`, where
/// the declared order and rate-monotonic order DISAGREE, so a dropped
/// priority is visible in the response times.
fn urgent_fast(pkg: &str, prio_prop: &str) -> String {
    model_spelled(
        pkg,
        prio_prop,
        &[
            ("urgent", Some(10), "100 ms", "20 ms", "100 ms"),
            ("fast", Some(1), "10 ms", "2 ms", "10 ms"),
        ],
    )
}

/// Strip the package name so lines from two models can be compared.
fn without_pkg(lines: &[String], pkg: &str) -> Vec<String> {
    lines.iter().map(|l| l.replace(pkg, "PKG")).collect()
}

// rivet: verifies REQ-RTA-PRIORITY-QUALIFIED-001
#[test]
fn qualified_thread_properties_priority_overrides_rate_monotonic() {
    // Same numbers as `priority_overrides_rate_monotonic`:
    //   declared order (urgent first): R_urgent = 20 ms;
    //     R_fast: 2 → 2 + ceil(2/100)·20 = 22 → 22 ms > 10 ms: MISS.
    //   rate-monotonic (Priority dropped): R_fast = 2 ms;
    //     R_urgent: 20 → 20 + ceil(20/10)·2 = 24 → 26 → 26 ms.
    let src = urgent_fast("PrioQualified", "Thread_Properties::Priority");
    let lines = rta_lines("PrioQualified", &src);
    let urgent = line_for(&lines, "urgent");
    let fast = line_for(&lines, "fast");
    // The rate-monotonic numbers must NOT appear: that is the #529 symptom.
    assert!(
        !urgent.contains("26.00 ms") && !fast.contains("response time 2.00 ms"),
        "Thread_Properties::Priority was ignored (rate-monotonic order): {lines:#?}"
    );
    assert!(
        urgent.contains("response time 20.00 ms <= deadline 100.00 ms"),
        "urgent (Thread_Properties::Priority 10) must see no interference: {urgent}"
    );
    assert!(
        fast.contains("misses deadline: response time exceeds deadline 10.00 ms"),
        "fast (Thread_Properties::Priority 1) must be pre-empted by urgent: {fast}"
    );
}

// rivet: verifies REQ-RTA-PRIORITY-QUALIFIED-001
#[test]
fn qualified_and_bare_priority_give_identical_rta() {
    let bare = rta_lines("PrioBare", &urgent_fast("PrioBare", "Priority"));
    let qual = rta_lines(
        "PrioQual2",
        &urgent_fast("PrioQual2", "Thread_Properties::Priority"),
    );
    let mut bare = without_pkg(&bare, "PrioBare");
    let mut qual = without_pkg(&qual, "PrioQual2");
    bare.sort();
    qual.sort();
    assert_eq!(
        bare, qual,
        "Thread_Properties::Priority must analyse exactly like bare Priority"
    );
}

// rivet: verifies REQ-RTA-PRIORITY-QUALIFIED-001
#[test]
fn legacy_deployment_properties_priority_still_read() {
    // `Deployment_Properties::Priority` is not a predeclared AADL 2 property,
    // but spar has always accepted it; #529 keeps it as a legacy alias.
    let src = urgent_fast("PrioLegacy", "Deployment_Properties::Priority");
    let lines = rta_lines("PrioLegacy", &src);
    assert!(
        line_for(&lines, "urgent").contains("response time 20.00 ms <= deadline 100.00 ms"),
        "{lines:#?}"
    );
    assert!(
        line_for(&lines, "fast")
            .contains("misses deadline: response time exceeds deadline 10.00 ms"),
        "{lines:#?}"
    );
}
