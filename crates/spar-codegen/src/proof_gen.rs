//! Scheduling proof generation: Lean4 proofs + Kani harnesses.
//!
//! For each processor with bound threads, generates:
//! - A Lean4 file with response-time analysis (RTA) theorems
//! - Per-thread Kani verification harnesses
//!
//! The Lean4 proofs use a standard RTA formulation where the response
//! time of a task is the smallest fixpoint of:
//!   R_i = C_i + sum_{j in hp(i)} ceil(R_i / T_j) * C_j
//!
//! The Kani harnesses verify that for any execution time bounded by
//! WCET, the thread meets its deadline.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

use crate::naming::{encode_instance_path, path_below_root};
use crate::{GeneratedFile, extract_timing, format_time_ps, threads_for_processor};

/// Generate a Lean4 scheduling proof file for a processor instance.
///
/// The proof contains one theorem per bound thread, each asserting that
/// the worst-case response time (computed via RTA) does not exceed the
/// thread's deadline.
pub fn generate_lean4_proof(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
) -> GeneratedFile {
    let proc_comp = inst.component(proc_idx);
    // Named by instance path, not by name: two processors that share a
    // name in different subsystems must not share a file (#510).
    let stem = format!(
        "{}_scheduling",
        encode_instance_path(&path_below_root(inst, proc_idx))
    );

    let threads = threads_for_processor(inst, proc_idx);

    let mut lean = String::new();

    // Header
    lean.push_str(&format!(
        "-- Generated from AADL processor: {}::{}\n",
        proc_comp.package, proc_comp.name
    ));
    lean.push_str(&format!(
        "-- Instance: {}\n",
        inst.component_instance_path(proc_idx)
    ));
    lean.push_str("-- DO NOT EDIT -- regenerate with `spar codegen`.\n\n");
    // The theorems are stated over the RTA definitions in
    // `proofs/Proofs/Scheduling/RTACore.lean` (`Task`, `rtaStep`) and the
    // lemmas in `RTA.lean` (`isFixedPoint`, `iterN`, `iterN_le_fixed_point`,
    // `rtaStep_mono`), the single source the scheduling proofs and the
    // generated Rust analysis share (REQ-PROOF-SCHED-SPEC-SOURCE-001).
    // REQ-PROOF-GEN-COMPILES-001 (#542).
    lean.push_str("import Proofs.Scheduling.RTA\n\n");
    lean.push_str("open Spar.Scheduling.RTA\n\n");

    // Collect thread timing data for higher-priority set
    let mut thread_data: Vec<(String, u64, u64, u64)> = Vec::new();
    for &t_idx in &threads {
        // By instance path: two threads named `t` in different processes,
        // both bound here, must not declare the same theorem (#510).
        let t_name = encode_instance_path(&path_below_root(inst, t_idx));
        let (period, deadline, wcet) = extract_timing(inst, t_idx);

        let period = period.unwrap_or(10_000_000_000); // default 10ms
        let deadline = deadline.unwrap_or(period);
        let wcet = wcet.unwrap_or(1_000_000_000); // default 1ms

        thread_data.push((t_name, period, deadline, wcet));
    }

    // Sort by period ascending (rate-monotonic: shorter period = higher priority).
    // The Lean4 proof treats threads with index < i as higher-priority, so the
    // ordering must match rate-monotonic assignment.
    thread_data.sort_by_key(|(_, period, _, _)| *period);

    // Generate namespace
    lean.push_str(&format!("namespace {}\n\n", capitalize(&stem)));

    // Generate one theorem per thread
    for (i, (t_name, period, deadline, wcet)) in thread_data.iter().enumerate() {
        lean.push_str(&format!(
            "-- Thread: {t_name}, Period={}, WCET={}, Deadline={}\n",
            format_time_ps(*period),
            format_time_ps(*wcet),
            format_time_ps(*deadline),
        ));

        // Higher-priority task set: all tasks with index < i (rate-monotonic).
        let task = RtaTask {
            exec: *wcet,
            period: *period,
            deadline: *deadline,
        };
        let hp: Vec<RtaTask> = thread_data[..i]
            .iter()
            .map(|(_, p, d, c)| RtaTask {
                exec: *c,
                period: *p,
                deadline: *d,
            })
            .collect();
        emit_rta_theorem(&mut lean, t_name, &task, &hp);
    }

    lean.push_str(&format!("end {}\n", capitalize(&stem)));

    GeneratedFile {
        path: format!("proofs/{stem}.lean"),
        content: lean,
    }
}

/// One task as the Lean `Spar.Scheduling.RTA.Task` structure sees it:
/// worst-case execution time, period and relative deadline in picoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RtaTask {
    exec: u64,
    period: u64,
    deadline: u64,
}

/// What the RTA recurrence does for one task, computed at generation time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RtaOutcome {
    /// The iteration from `exec` reached the fixed point `r`, and `r ≤ deadline`.
    Meets(u128),
    /// The iterates `exec = r_0 < r_1 < … < r_k`, where only `r_k > deadline`.
    Misses(Vec<u128>),
    /// No verdict within [`MAX_RTA_STEPS`] steps, or a value overflowed.
    Undecided,
}

/// Bound on the RTA iterations the generator runs, and so on the length of
/// the step chain a `_misses_deadline` proof spells out.
const MAX_RTA_STEPS: usize = 256;

/// `RTACore.rtaStep`: `C_i + Σ_j ⌈r / T_j⌉ · C_j`, with `ceilDiv a b = (a + b - 1) / b`.
fn rta_step(task: &RtaTask, hp: &[RtaTask], r: u128) -> Option<u128> {
    let mut acc = u128::from(task.exec);
    for h in hp {
        let t = u128::from(h.period);
        let ceil = r.checked_add(t)?.checked_sub(1)? / t;
        acc = acc.checked_add(ceil.checked_mul(u128::from(h.exec))?)?;
    }
    Some(acc)
}

/// Run the RTA recurrence from `exec` until it reaches a fixed point or
/// exceeds the deadline. Every task must have positive fields.
fn rta_outcome(task: &RtaTask, hp: &[RtaTask]) -> RtaOutcome {
    let deadline = u128::from(task.deadline);
    let mut iterates = vec![u128::from(task.exec)];
    for _ in 0..MAX_RTA_STEPS {
        let r = *iterates.last().expect("non-empty");
        if r > deadline {
            return RtaOutcome::Misses(iterates);
        }
        let Some(next) = rta_step(task, hp, r) else {
            return RtaOutcome::Undecided;
        };
        if next == r {
            return RtaOutcome::Meets(r);
        }
        iterates.push(next);
    }
    RtaOutcome::Undecided
}

/// The Lean term for one task, as a `Spar.Scheduling.RTA.Task` literal.
///
/// On one line: Lean's structure-instance parser wants every field at or
/// right of the first one's column, so a wrapped field fails to parse.
fn lean_task(t: &RtaTask) -> String {
    format!(
        "{{ exec := {}, period := {}, deadline := {}, \
         exec_pos := by decide, period_pos := by decide, deadline_pos := by decide }}",
        t.exec, t.period, t.deadline
    )
}

/// Emit the theorem for one thread, stated over the existing RTA definitions.
///
/// * Schedulable: `<t>_meets_deadline`. Some fixed point `r` of `rtaStep`
///   with `exec ≤ r ≤ deadline` exists, and every RTA iterate from `exec` is
///   `≤ deadline`. The proof checks the generator's fixed point `R` by
///   `decide` and closes the rest with `iterN_le_fixed_point`. A wrong `R`
///   fails the build; it cannot yield a false theorem.
/// * Unschedulable: `<t>_misses_deadline`. Every fixed point `r ≥ exec`
///   exceeds the deadline, so the worst-case response time does. Proved by
///   walking the iterates with `rtaStep_mono`.
/// * No verdict, or a non-positive field: a comment and no theorem. Never
///   `sorry`.
fn emit_rta_theorem(lean: &mut String, t_name: &str, task: &RtaTask, hp: &[RtaTask]) {
    let positive = |t: &RtaTask| t.exec > 0 && t.period > 0 && t.deadline > 0;
    if !positive(task) || !hp.iter().all(positive) {
        lean.push_str(
            "-- No theorem: RTA's `Task` needs a positive WCET, period and deadline \
             for this thread and every higher-priority one.\n\n",
        );
        return;
    }
    let outcome = rta_outcome(task, hp);
    let suffix = match outcome {
        RtaOutcome::Meets(_) => "meets_deadline",
        RtaOutcome::Misses(_) => "misses_deadline",
        RtaOutcome::Undecided => {
            lean.push_str(&format!(
                "-- No theorem: the RTA iteration gave no verdict within {MAX_RTA_STEPS} steps.\n\n"
            ));
            return;
        }
    };

    lean.push_str(&format!("theorem {t_name}_{suffix} :\n"));
    lean.push_str(&format!("    let task : Task := {}\n", lean_task(task)));
    if hp.is_empty() {
        lean.push_str("    let hp : List Task := []\n");
    } else {
        let items: Vec<String> = hp
            .iter()
            .map(|h| format!("      {}", lean_task(h)))
            .collect();
        lean.push_str(&format!(
            "    let hp : List Task := [\n{}]\n",
            items.join(",\n")
        ));
    }

    match outcome {
        RtaOutcome::Meets(r) => {
            lean.push_str(
                "    (∃ r, isFixedPoint task hp r ∧ task.exec ≤ r ∧ r ≤ task.deadline) ∧\n",
            );
            lean.push_str("    ∀ n, iterN (rtaStep task hp) n task.exec ≤ task.deadline := by\n");
            lean.push_str("  intro task hp\n");
            lean.push_str(&format!(
                "  have hfp : isFixedPoint task hp {r} := by unfold isFixedPoint; decide\n"
            ));
            lean.push_str(&format!("  have hge : task.exec ≤ {r} := by decide\n"));
            lean.push_str(&format!(
                "  have hle : ({r} : Nat) ≤ task.deadline := by decide\n"
            ));
            lean.push_str(
                "  exact ⟨⟨_, hfp, hge, hle⟩,\n    \
                 fun n => Nat.le_trans (iterN_le_fixed_point task hp _ hfp hge n) hle⟩\n\n",
            );
        }
        RtaOutcome::Misses(iterates) => {
            lean.push_str(
                "    ∀ r, isFixedPoint task hp r → task.exec ≤ r → task.deadline < r := by\n",
            );
            lean.push_str("  intro task hp r hfp hge\n");
            lean.push_str("  unfold isFixedPoint at hfp\n");
            lean.push_str(
                "  have step : ∀ a b, a ≤ r → rtaStep task hp a = b → b ≤ r := by\n    \
                 intro a b ha hab\n    \
                 rw [← hab]\n    \
                 exact Nat.le_trans (rtaStep_mono ha) (Nat.le_of_eq hfp)\n",
            );
            lean.push_str(&format!("  have h0 : ({} : Nat) ≤ r := hge\n", iterates[0]));
            for (k, value) in iterates.iter().enumerate().skip(1) {
                lean.push_str(&format!(
                    "  have h{k} : ({value} : Nat) ≤ r := step _ _ h{} (by decide)\n",
                    k - 1
                ));
            }
            let last = iterates.len() - 1;
            lean.push_str(&format!(
                "  exact Nat.lt_of_lt_of_le (by decide) h{last}\n\n"
            ));
        }
        RtaOutcome::Undecided => unreachable!("returned above"),
    }
}

/// Generate a Kani verification harness for a thread instance.
///
/// The harness verifies that for any non-deterministic execution time
/// bounded by the declared WCET, the computed response time does not
/// exceed the thread's deadline.
pub fn generate_kani_harness(
    inst: &SystemInstance,
    thread_idx: ComponentInstanceIdx,
) -> GeneratedFile {
    let comp = inst.component(thread_idx);
    // The directory is the parent's instance path, the file the thread's own
    // name, both encoded injectively: a process `app` in two subsystems no
    // longer gives two threads one harness file (#510).
    let mut path = path_below_root(inst, thread_idx);
    let own = path.pop().map(|seg| vec![seg]).unwrap_or_default();
    let name = encode_instance_path(&own);
    let parent_dir = encode_instance_path(&path);

    let (period, deadline, wcet) = extract_timing(inst, thread_idx);

    let period_ps = period.unwrap_or(10_000_000_000);
    let deadline_ps = deadline.unwrap_or(period_ps);
    let wcet_ps = wcet.unwrap_or(1_000_000_000);

    let mut code = String::new();

    // Header
    code.push_str(&format!(
        "//! Kani verification harness for AADL thread: {}::{}\n",
        comp.package, comp.name
    ));
    code.push_str(&format!(
        "//! Instance: {}\n",
        inst.component_instance_path(thread_idx)
    ));
    code.push_str("//! DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    // Emit warnings when defaults are used — the harness may trivially pass
    if period.is_none() {
        code.push_str("// WARNING: Period defaulted to 10ms — verify against actual system\n");
    }
    if deadline.is_none() {
        code.push_str("// WARNING: Deadline defaulted to period — verify against actual system\n");
    }
    if wcet.is_none() {
        code.push_str("// WARNING: WCET defaulted to 1ms — verify against actual system\n");
    }
    if period.is_none() || deadline.is_none() || wcet.is_none() {
        code.push('\n');
    }

    // Constants
    code.push_str(&format!("/// Period: {}\n", format_time_ps(period_ps)));
    code.push_str(&format!("const PERIOD_PS: u64 = {period_ps};\n"));
    code.push_str(&format!("/// Deadline: {}\n", format_time_ps(deadline_ps)));
    code.push_str(&format!("const DEADLINE_PS: u64 = {deadline_ps};\n"));
    code.push_str(&format!(
        "/// Worst-case execution time: {}\n",
        format_time_ps(wcet_ps)
    ));
    code.push_str(&format!("const WCET_PS: u64 = {wcet_ps};\n\n"));

    // Kani harness
    code.push_str("#[cfg(kani)]\n");
    code.push_str("#[kani::proof]\n");
    code.push_str("#[kani::unwind(20)]\n");
    code.push_str(&format!("fn verify_{name}_no_deadline_miss() {{\n"));
    code.push_str("    let wcet: u64 = kani::any();\n");
    code.push_str("    kani::assume(wcet <= WCET_PS);\n");
    code.push_str("    kani::assume(wcet > 0);\n\n");
    code.push_str("    // Simple response time check: in isolation, response time = wcet\n");
    code.push_str("    // With higher-priority interference, R = C + sum(ceil(R/T_j)*C_j)\n");
    code.push_str("    // This harness checks the base case (no interference)\n");
    code.push_str("    let response_time = wcet;\n");
    code.push_str("    assert!(\n");
    code.push_str("        response_time <= DEADLINE_PS,\n");
    code.push_str(&format!(
        "        \"Thread {name} misses deadline: response_time={{}} > deadline={{}}\",\n"
    ));
    code.push_str("        response_time,\n");
    code.push_str("        DEADLINE_PS,\n");
    code.push_str("    );\n\n");
    code.push_str("    // Verify period constraint: execution must complete within one period\n");
    code.push_str("    assert!(\n");
    code.push_str("        response_time <= PERIOD_PS,\n");
    code.push_str(&format!(
        "        \"Thread {name} overruns period: response_time={{}} > period={{}}\",\n"
    ));
    code.push_str("        response_time,\n");
    code.push_str("        PERIOD_PS,\n");
    code.push_str("    );\n");
    code.push_str("}\n\n");

    // Additional harness: utilization bound check
    code.push_str("#[cfg(kani)]\n");
    code.push_str("#[kani::proof]\n");
    code.push_str(&format!("fn verify_{name}_utilization_bound() {{\n"));
    code.push_str("    let wcet: u64 = kani::any();\n");
    code.push_str("    kani::assume(wcet <= WCET_PS);\n");
    code.push_str("    kani::assume(wcet > 0);\n");
    code.push_str("    kani::assume(PERIOD_PS > 0);\n\n");
    code.push_str("    // Utilization must be <= 1.0 (wcet/period <= 1)\n");
    code.push_str("    assert!(\n");
    code.push_str("        wcet <= PERIOD_PS,\n");
    code.push_str(&format!(
        "        \"Thread {name} utilization > 1: wcet={{}} > period={{}}\",\n"
    ));
    code.push_str("        wcet,\n");
    code.push_str("        PERIOD_PS,\n");
    code.push_str("    );\n");
    code.push_str("}\n");

    GeneratedFile {
        path: format!("proofs/kani/{parent_dir}/{name}_harness.rs"),
        content: code,
    }
}

/// Capitalize the first letter of a string.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::instance::SystemInstance;
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::Name;
    use spar_hir_def::resolver::GlobalScope;

    fn build_test_instance() -> SystemInstance {
        let aadl = r#"
package TestPkg
public
    processor MainCPU
    end MainCPU;

    processor implementation MainCPU.Impl
    end MainCPU.Impl;

    thread CtrlThread
        properties
            Timing_Properties::Period => 10 ms;
            Timing_Properties::Deadline => 8 ms;
            Timing_Properties::Compute_Execution_Time => 1 ms .. 2 ms;
    end CtrlThread;

    thread implementation CtrlThread.Impl
    end CtrlThread.Impl;

    thread SensorThread
        properties
            Timing_Properties::Period => 20 ms;
            Timing_Properties::Deadline => 15 ms;
            Timing_Properties::Compute_Execution_Time => 1 ms .. 3 ms;
    end SensorThread;

    thread implementation SensorThread.Impl
    end SensorThread.Impl;

    process Controller
    end Controller;

    process implementation Controller.Impl
        subcomponents
            ctrl: thread CtrlThread.Impl;
            sensor: thread SensorThread.Impl;
    end Controller.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            cpu: processor MainCPU.Impl;
            ctrl: process Controller.Impl;
    end Top.Impl;
end TestPkg;
"#;

        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        SystemInstance::instantiate(
            &scope,
            &Name::new("TestPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        )
    }

    #[test]
    fn lean4_proof_contains_theorem() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Processor)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_lean4_proof(&inst, idx);
            assert!(file.path.ends_with(".lean"));
            assert!(
                file.content.contains("theorem"),
                "Lean4 proof must contain 'theorem'"
            );
            assert!(
                file.content.contains("rtaStep") && file.content.contains("isFixedPoint"),
                "Lean4 proof must be stated over the RTA library's rtaStep / isFixedPoint"
            );
            assert!(
                file.content.contains("meets_deadline"),
                "Lean4 proof must have deadline theorem"
            );
            assert!(
                file.content.contains("import Proofs.Scheduling.RTA"),
                "Lean4 proof must import RTA library"
            );
        }
    }

    #[test]
    fn kani_harness_contains_proof_attribute() {
        let inst = build_test_instance();
        let thread_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx);

        if let Some(idx) = thread_idx {
            let file = generate_kani_harness(&inst, idx);
            assert!(file.path.ends_with("_harness.rs"));
            assert!(
                file.content.contains("#[kani::proof]"),
                "Kani harness must contain #[kani::proof]"
            );
            assert!(
                file.content.contains("#[kani::unwind(20)]"),
                "Kani harness must set unwind bound"
            );
            assert!(
                file.content.contains("kani::any()"),
                "Kani harness must use non-deterministic input"
            );
            assert!(
                file.content.contains("kani::assume"),
                "Kani harness must constrain inputs"
            );
            assert!(
                file.content.contains("DEADLINE_PS"),
                "Kani harness must reference deadline"
            );
            assert!(
                file.content.contains("WCET_PS"),
                "Kani harness must reference WCET"
            );
        }
    }

    #[test]
    fn lean4_proof_includes_higher_priority_tasks() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Processor)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_lean4_proof(&inst, idx);
            // Should have theorems for multiple threads
            let theorem_count = file.content.matches("theorem").count();
            assert!(
                theorem_count >= 1,
                "Expected at least 1 theorem, got {theorem_count}"
            );
        }
    }

    #[test]
    fn kani_harness_has_utilization_check() {
        let inst = build_test_instance();
        let thread_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx);

        if let Some(idx) = thread_idx {
            let file = generate_kani_harness(&inst, idx);
            assert!(
                file.content.contains("verify_") && file.content.contains("_utilization_bound"),
                "Kani harness must include utilization bound check"
            );
        }
    }

    #[test]
    fn lean4_proof_sorted_by_period_rate_monotonic() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Processor)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_lean4_proof(&inst, idx);
            // ctrl has period 10ms, sensor has period 20ms.
            // Rate-monotonic ordering: ctrl (10ms) should appear before
            // sensor (20ms). The first theorem must be for the
            // shorter-period thread.
            let ctrl_pos = file.content.find("ctrl_meets_deadline");
            let sensor_pos = file.content.find("sensor_meets_deadline");
            assert!(
                ctrl_pos.is_some() && sensor_pos.is_some(),
                "Both theorems must exist in generated proof"
            );
            assert!(
                ctrl_pos.unwrap() < sensor_pos.unwrap(),
                "ctrl (10ms) must appear before sensor (20ms) in rate-monotonic order"
            );

            // sensor's higher-priority set should include ctrl's
            // period and WCET values.
            let sensor_section_start = sensor_pos.unwrap();
            let sensor_section = &file.content[sensor_section_start..];
            assert!(
                sensor_section.contains("10000000000"),
                "sensor's hp set must include ctrl's period (10ms in ps)"
            );
        }
    }

    #[test]
    fn kani_harness_warns_on_default_values() {
        // Build an instance with a thread that has NO timing properties,
        // so all defaults are used.
        let aadl = r#"
package DefaultPkg
public
    thread BareThread
    end BareThread;

    thread implementation BareThread.Impl
    end BareThread.Impl;

    process Proc
    end Proc;

    process implementation Proc.Impl
        subcomponents
            bare: thread BareThread.Impl;
    end Proc.Impl;

    system Sys
    end Sys;

    system implementation Sys.Impl
        subcomponents
            p: process Proc.Impl;
    end Sys.Impl;
end DefaultPkg;
"#;

        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "default.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("DefaultPkg"),
            &Name::new("Sys"),
            &Name::new("Impl"),
        );

        let thread_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx);

        if let Some(idx) = thread_idx {
            let file = generate_kani_harness(&inst, idx);
            assert!(
                file.content.contains("WARNING: Period defaulted"),
                "Kani harness must warn when period is defaulted"
            );
            assert!(
                file.content.contains("WARNING: Deadline defaulted"),
                "Kani harness must warn when deadline is defaulted"
            );
            assert!(
                file.content.contains("WARNING: WCET defaulted"),
                "Kani harness must warn when WCET is defaulted"
            );
        }
    }

    #[test]
    fn kani_harness_no_warnings_with_explicit_timing() {
        // The standard test instance has explicit timing properties
        let inst = build_test_instance();
        let thread_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx);

        if let Some(idx) = thread_idx {
            let file = generate_kani_harness(&inst, idx);
            assert!(
                !file.content.contains("WARNING:"),
                "Kani harness must not warn when timing properties are explicit"
            );
        }
    }

    // ── #510: proof file and module names are injective ──────────────

    /// Test-only inverse of [`encode_instance_path`]. That it recovers every
    /// path is the proof that the encoding is injective.
    fn decode_instance_path(encoded: &str) -> Vec<String> {
        if encoded == "zr" {
            return Vec::new();
        }
        let body = encoded.strip_prefix("zd").unwrap_or(encoded);
        body.split("__")
            .map(|seg| {
                if seg == "ze" {
                    return String::new();
                }
                let chars: Vec<char> = seg.chars().collect();
                let mut out = String::new();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i] != 'z' {
                        out.push(chars[i]);
                        i += 1;
                        continue;
                    }
                    match chars[i + 1] {
                        'z' => {
                            out.push('z');
                            i += 2;
                        }
                        'u' => {
                            out.push('_');
                            i += 2;
                        }
                        'c' => {
                            out.push(chars[i + 2].to_ascii_uppercase());
                            i += 3;
                        }
                        'x' => {
                            let hex: String = chars[i + 2..i + 8].iter().collect();
                            let code = u32::from_str_radix(&hex, 16).unwrap();
                            out.push(char::from_u32(code).unwrap());
                            i += 8;
                        }
                        other => panic!("bad escape z{other} in {encoded}"),
                    }
                }
                out
            })
            .collect()
    }

    /// Every encoded name is lowercase `[a-z0-9_]`, so a case-insensitive
    /// filesystem cannot fold two of them together, and it starts with a
    /// letter, so `<name>_scheduling` is a Lean module and namespace stem and
    /// `<name>_meets_deadline` a Lean theorem name.
    fn assert_valid_stem(encoded: &str, from: &[String]) {
        // `from` is read only by the assert messages, which Kani's build
        // drops; without this the Kani job fails on `-D unused-variables`.
        let _ = from;
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{from:?} -> {encoded:?} is not lowercase [a-z0-9_]"
        );
        assert!(
            encoded.starts_with(|c: char| c.is_ascii_lowercase()),
            "{from:?} -> {encoded:?} does not start with a letter"
        );
    }

    // rivet: verifies REQ-PROOF-GEN-NAMES-INJECTIVE-001
    #[test]
    #[cfg_attr(miri, ignore = "exhaustive enumeration is too slow under Miri")]
    fn instance_path_encoding_is_injective_over_tricky_names() {
        // Every string of length <= 3 over an alphabet chosen to collide
        // under naive mangling: case (`a`/`A`), the escape letter (`z`/`Z`),
        // the separator (`_`), the AADL path separator (`.`), a dash, a
        // digit, and a non-ASCII letter.
        let alphabet = ['a', 'A', 'z', 'Z', '_', '.', '-', '1', 'é'];
        let mut segments = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..3 {
            frontier = frontier
                .iter()
                .flat_map(|s| alphabet.iter().map(move |c| format!("{s}{c}")))
                .collect();
            segments.extend(frontier.iter().cloned());
        }
        let short: Vec<&String> = segments.iter().filter(|s| s.chars().count() <= 1).collect();

        let mut paths: Vec<Vec<String>> = vec![Vec::new()];
        paths.extend(segments.iter().map(|s| vec![s.clone()]));
        for a in &segments {
            for b in segments.iter().filter(|s| s.chars().count() <= 2) {
                paths.push(vec![a.clone(), b.clone()]);
            }
        }
        for a in &short {
            for b in &short {
                for c in &short {
                    paths.push(vec![(*a).clone(), (*b).clone(), (*c).clone()]);
                }
            }
        }
        // The named collisions, explicitly: `a_b` vs `a`/`b`, `A` vs `a`, and
        // Lean reserved words as instance names.
        for p in [
            vec!["a_b"],
            vec!["a", "b"],
            vec!["A"],
            vec!["a"],
            vec!["def"],
            vec!["end"],
            vec!["theorem"],
            vec!["namespace"],
            vec!["Type"],
            vec!["import"],
        ] {
            paths.push(p.into_iter().map(String::from).collect());
        }

        let mut seen: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for path in &paths {
            let encoded = encode_instance_path(path);
            assert_valid_stem(&encoded, path);
            assert_eq!(&decode_instance_path(&encoded), path, "via {encoded:?}");
            if let Some(prev) = seen.insert(encoded.clone(), path.clone()) {
                assert_eq!(
                    &prev, path,
                    "{prev:?} and {path:?} both encode to {encoded:?}"
                );
            }
        }
        // As many distinct encodings as distinct paths (76 449 here).
        let distinct: std::collections::HashSet<&Vec<String>> = paths.iter().collect();
        assert_eq!(seen.len(), distinct.len());
        assert!(seen.len() > 70_000, "only {} distinct paths", seen.len());

        assert_eq!(encode_instance_path(&["cpu"]), "cpu");
        assert_eq!(encode_instance_path(&["left", "cpu"]), "left__cpu");
        assert_eq!(encode_instance_path(&["sensor_proc"]), "sensor_proc");
        assert_eq!(encode_instance_path(&["a_b"]), "a_b");
        assert_eq!(encode_instance_path(&["a", "b"]), "a__b");
        assert_eq!(encode_instance_path(&["A"]), "zca");
        assert_eq!(encode_instance_path(&["_a__"]), "zuazuzu");
        // Every `_` of a run is escaped, not only the first.
        assert_eq!(encode_instance_path(&["a__b"]), "azuzub");
        assert_eq!(encode_instance_path(&["é.z"]), "zx0000e9zx00002ezz");
        assert_eq!(encode_instance_path(&[""]), "ze");
    }

    // rivet: verifies REQ-PROOF-GEN-NAMES-INJECTIVE-001
    #[test]
    fn lean_reserved_words_give_non_reserved_identifiers() {
        const LEAN_KEYWORDS: &[&str] = &[
            "def",
            "end",
            "theorem",
            "namespace",
            "import",
            "open",
            "section",
            "where",
            "by",
            "fun",
            "let",
            "match",
            "with",
            "do",
            "if",
            "then",
            "else",
            "Type",
            "Prop",
            "Sort",
        ];
        for kw in LEAN_KEYWORDS {
            let encoded = encode_instance_path(&[*kw]);
            for ident in [
                format!("{encoded}_scheduling"),
                capitalize(&format!("{encoded}_scheduling")),
                format!("{encoded}_meets_deadline"),
            ] {
                assert!(!LEAN_KEYWORDS.contains(&ident.as_str()), "{kw} -> {ident}");
            }
        }
        assert_eq!(encode_instance_path(&["1cpu"]), "zd1cpu");
        assert_eq!(encode_instance_path::<&str>(&[]), "zr");
    }

    fn instantiate(src: &str, pkg: &str) -> (SystemInstance, GlobalScope) {
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "m.aadl".to_string(), src.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new(pkg),
            &Name::new("Top"),
            &Name::new("impl"),
        );
        (inst, scope)
    }

    /// The proof artifacts `spar codegen --verify proof` emits for `inst`.
    fn proof_files((inst, scope): &(SystemInstance, GlobalScope)) -> Vec<GeneratedFile> {
        let config = crate::CodegenConfig {
            root_name: "top".to_string(),
            output_dir: "out".to_string(),
            format: crate::OutputFormat::Wit,
            verify: Some(crate::VerifyMode::Proof),
            rivet: false,
            dry_run: true,
        };
        crate::generate(inst, scope, &config)
            .expect("codegen")
            .files
            .into_iter()
            .filter(|f| f.path.starts_with("proofs/"))
            .collect()
    }

    /// Two processors both named `cpu`, in subsystems `left` and `right`,
    /// each with a process `app` holding a thread `t` bound to its own `cpu`
    /// — the #510 repro.
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

    // rivet: verifies REQ-PROOF-GEN-NAMES-INJECTIVE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn same_named_processors_write_two_distinct_proof_files() {
        let files = proof_files(&instantiate(TWO_CPUS_SAME_NAME, "TwoCpu"));
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();

        // No two proof files share a path, even case-insensitively.
        let folded: std::collections::BTreeSet<String> =
            paths.iter().map(|p| p.to_lowercase()).collect();
        assert_eq!(
            folded.len(),
            paths.len(),
            "colliding proof paths: {paths:?}"
        );

        let mut lean: Vec<&GeneratedFile> =
            files.iter().filter(|f| f.path.ends_with(".lean")).collect();
        lean.sort_by(|a, b| a.path.cmp(&b.path));
        let lean_paths: Vec<&str> = lean.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            lean_paths,
            [
                "proofs/left__cpu_scheduling.lean",
                "proofs/right__cpu_scheduling.lean"
            ]
        );
        // Neither proof is lost: each file is its own processor's, proving
        // its own thread.
        for (file, side) in lean.iter().zip(["left", "right"]) {
            let content = &file.content;
            assert!(
                content.contains(&format!("-- Instance: Top.impl/{side}/cpu\n")),
                "{content}"
            );
            assert!(
                content.contains(&format!("theorem {side}__app__t_meets_deadline :")),
                "{content}"
            );
            assert_eq!(content.matches("theorem ").count(), 1, "{content}");
            let ns = capitalize(&format!("{side}__cpu_scheduling"));
            assert!(content.contains(&format!("namespace {ns}\n")), "{content}");
            assert!(content.contains(&format!("end {ns}\n")), "{content}");
        }

        let mut kani: Vec<&str> = paths
            .iter()
            .copied()
            .filter(|p| p.ends_with("_harness.rs"))
            .collect();
        kani.sort();
        assert_eq!(
            kani,
            [
                "proofs/kani/left__app/t_harness.rs",
                "proofs/kani/right__app/t_harness.rs"
            ]
        );
        for side in ["left", "right"] {
            let harness = files
                .iter()
                .find(|f| f.path == format!("proofs/kani/{side}__app/t_harness.rs"))
                .unwrap();
            assert!(
                harness
                    .content
                    .contains(&format!("//! Instance: Top.impl/{side}/app/t\n")),
                "{}",
                harness.content
            );
        }
    }

    // rivet: verifies REQ-PROOF-GEN-NAMES-INJECTIVE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn same_named_threads_on_one_processor_get_distinct_theorems() {
        // Two processes, each with a thread `t`, both bound to one `cpu`: by
        // name, one proof file declared `t_meets_deadline` twice, which Lean
        // rejects as an already-declared constant.
        let src = r#"package OneCpu
public
  processor Cpu
  end Cpu;
  thread W
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 1 ms .. 1 ms;
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
      cpu : processor Cpu;
      p1 : process P.impl;
      p2 : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to p1.t;
      Actual_Processor_Binding => (reference (cpu)) applies to p2.t;
  end Top.impl;
end OneCpu;
"#;
        let files = proof_files(&instantiate(src, "OneCpu"));
        let lean: Vec<&GeneratedFile> =
            files.iter().filter(|f| f.path.ends_with(".lean")).collect();
        assert_eq!(lean.len(), 1);
        // A processor directly under the root keeps its plain name.
        assert_eq!(lean[0].path, "proofs/cpu_scheduling.lean");
        let theorems: Vec<&str> = lean[0]
            .content
            .lines()
            .filter_map(|l| l.strip_prefix("theorem "))
            .collect();
        assert_eq!(
            theorems,
            ["p1__t_meets_deadline :", "p2__t_meets_deadline :"],
            "{}",
            lean[0].content
        );
    }

    // ── REQ-PROOF-GEN-COMPILES-001 (#542) ──────────────────────────────

    const MS: u64 = 1_000_000_000;

    fn t(exec_ms: u64, period_ms: u64, deadline_ms: u64) -> RtaTask {
        RtaTask {
            exec: exec_ms * MS,
            period: period_ms * MS,
            deadline: deadline_ms * MS,
        }
    }

    #[test]
    fn rta_outcome_matches_hand_computed_response_times() {
        let fast = t(1, 5, 5);
        let mid = t(2, 10, 10);
        let slow = t(5, 20, 20);
        assert_eq!(rta_outcome(&fast, &[]), RtaOutcome::Meets(u128::from(MS)));
        // 2 + ⌈2/5⌉·1 = 3; 2 + ⌈3/5⌉·1 = 3.
        assert_eq!(
            rta_outcome(&mid, &[fast]),
            RtaOutcome::Meets(u128::from(3 * MS))
        );
        // 5 → 5+1+2 = 8 → 5+2+2 = 9 → 9.
        assert_eq!(
            rta_outcome(&slow, &[fast, mid]),
            RtaOutcome::Meets(u128::from(9 * MS))
        );
        // A response time exactly at the deadline still meets it.
        assert_eq!(
            rta_outcome(&t(5, 20, 9), &[fast, mid]),
            RtaOutcome::Meets(u128::from(9 * MS))
        );
        // One below: the iterates 5, 8, 9 — 9 > 8 — miss.
        assert_eq!(
            rta_outcome(&t(5, 20, 8), &[fast, mid]),
            RtaOutcome::Misses(
                vec![5, 8, 9]
                    .into_iter()
                    .map(|v| v * u128::from(MS))
                    .collect()
            )
        );
        // WCET above the deadline misses at step 0.
        assert_eq!(
            rta_outcome(&t(3, 10, 2), &[]),
            RtaOutcome::Misses(vec![u128::from(3 * MS)])
        );
    }

    #[test]
    fn rta_outcome_is_undecided_past_the_step_bound() {
        // ⌈r/1⌉·1 = r, so rtaStep r = 1 + r: one picosecond a step, never a
        // fixed point, deadline far away.
        let hp = RtaTask {
            exec: 1,
            period: 1,
            deadline: 1,
        };
        let task = RtaTask {
            exec: 1,
            period: u64::MAX,
            deadline: u64::MAX,
        };
        assert_eq!(rta_outcome(&task, &[hp]), RtaOutcome::Undecided);
        let mut lean = String::new();
        emit_rta_theorem(&mut lean, "x", &task, &[hp]);
        assert!(!lean.contains("theorem x_"), "{lean}");
        assert!(!lean.contains("sorry"), "{lean}");
    }

    #[test]
    fn non_positive_task_gets_no_theorem() {
        let mut lean = String::new();
        emit_rta_theorem(&mut lean, "x", &t(1, 10, 10), &[t(0, 5, 5)]);
        assert!(lean.starts_with("-- No theorem"), "{lean}");
        assert!(!lean.contains("theorem x_"), "{lean}");
    }

    #[test]
    fn zero_period_task_gets_no_theorem() {
        // Period exactly 0: `period_pos := by decide` would be false, so no
        // theorem — not a `_meets_deadline` over a task Lean cannot build.
        let mut lean = String::new();
        emit_rta_theorem(&mut lean, "x", &t(1, 0, 10), &[]);
        assert!(lean.starts_with("-- No theorem"), "{lean}");
        assert!(!lean.contains("theorem x_"), "{lean}");
        assert!(!lean.contains("period := 0"), "{lean}");
    }

    #[test]
    fn zero_deadline_task_gets_no_theorem() {
        // Deadline exactly 0: `deadline_pos := by decide` would be false, so
        // no theorem — not a `_misses_deadline` over a task Lean cannot build.
        let mut lean = String::new();
        emit_rta_theorem(&mut lean, "x", &t(1, 10, 0), &[]);
        assert!(lean.starts_with("-- No theorem"), "{lean}");
        assert!(!lean.contains("theorem x_"), "{lean}");
        assert!(!lean.contains("deadline := 0"), "{lean}");
    }

    #[test]
    fn missed_deadline_emits_a_misses_theorem_not_a_false_one() {
        let mut lean = String::new();
        emit_rta_theorem(&mut lean, "late", &t(5, 20, 8), &[t(1, 5, 5), t(2, 10, 10)]);
        assert!(lean.contains("theorem late_misses_deadline"), "{lean}");
        assert!(!lean.contains("meets_deadline"), "{lean}");
        assert!(
            lean.contains("task.deadline < r"),
            "the miss theorem must state the response time exceeds the deadline: {lean}"
        );
        // The chain walks every iterate the Rust recurrence produced.
        for v in ["5000000000", "8000000000", "9000000000"] {
            assert!(
                lean.contains(&format!("({v} : Nat) ≤ r")),
                "{v} missing: {lean}"
            );
        }
    }

    /// The repository root, from the RUNTIME `CARGO_MANIFEST_DIR` that cargo
    /// sets for test processes. `env!` would bake in the path the test binary
    /// was compiled in, which is stale when a target dir is reused across
    /// copies of the tree (cargo-mutants does exactly that).
    fn repo_root() -> std::path::PathBuf {
        let dir = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        dir.join("../..")
    }

    /// The Lean library the generated proofs import: `proofs/Proofs`.
    fn proofs_dir() -> std::path::PathBuf {
        repo_root().join("proofs")
    }

    fn lean_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read proofs dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                lean_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "lean") {
                out.push(path);
            }
        }
    }

    /// Every name the library declares: `def`/`theorem`/`structure`/… names,
    /// the fields of its structures, and its namespaces.
    fn library_names() -> std::collections::BTreeSet<String> {
        let mut files = Vec::new();
        lean_files(&proofs_dir().join("Proofs"), &mut files);
        assert!(!files.is_empty(), "no .lean file under proofs/Proofs");
        let decl_kw = [
            "def",
            "theorem",
            "lemma",
            "structure",
            "abbrev",
            "inductive",
            "namespace",
        ];
        let mut names = std::collections::BTreeSet::new();
        for f in files {
            let text = std::fs::read_to_string(&f).expect("read .lean");
            let mut in_structure = false;
            for line in text.lines() {
                let words: Vec<&str> = line.split_whitespace().collect();
                let first_decl = words.iter().position(|w| decl_kw.contains(w)).filter(|&i| {
                    words[..i]
                        .iter()
                        .all(|w| ["private", "protected", "noncomputable", "@[simp]"].contains(w))
                });
                if let Some(i) = first_decl {
                    if let Some(name) = words.get(i + 1) {
                        names.insert(name.trim_end_matches(':').to_string());
                    }
                    in_structure = words[i] == "structure";
                    continue;
                }
                if in_structure {
                    if line.starts_with(' ') && words.get(1) == Some(&":") {
                        names.insert(words[0].to_string());
                    } else if !line.trim().is_empty() && !line.starts_with(' ') {
                        in_structure = false;
                    }
                }
            }
        }
        names
    }

    /// The identifiers a generated proof uses that are neither bound in the
    /// file itself nor Lean syntax / core Lean, and that the library does not
    /// declare. Empty means every library name it leans on exists.
    fn undeclared_library_names(content: &str) -> Vec<String> {
        // Lean syntax and the few core (non-library) names the generator uses.
        // This is NOT the library vocabulary: a name the generator expects
        // `proofs/` to provide must be found in `proofs/`, never listed here.
        const LEAN: &[&str] = &[
            "import",
            "open",
            "namespace",
            "end",
            "theorem",
            "let",
            "by",
            "intro",
            "have",
            "unfold",
            "decide",
            "exact",
            "fun",
            "at",
            "rw",
            "List",
            "Nat",
            "Nat.le_trans",
            "Nat.lt_of_lt_of_le",
            "Nat.le_of_eq",
            "_",
        ];
        let lib = library_names();
        let code: String = content
            .lines()
            .map(|l| l.split("--").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let is_ident_char = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '\'';
        let tokens: Vec<&str> = code
            .split(|c: char| !is_ident_char(c))
            .filter(|s| !s.is_empty() && !s.chars().next().unwrap().is_ascii_digit())
            .collect();

        // Names the file binds itself.
        let mut local = std::collections::BTreeSet::new();
        let binders = [
            "theorem",
            "namespace",
            "let",
            "have",
            "intro",
            "fun",
            "∃",
            "∀",
        ];
        for line in code.lines() {
            let mut words = line.split_whitespace().peekable();
            while let Some(w) = words.next() {
                if !binders.contains(&w.trim_start_matches('(')) {
                    continue;
                }
                // `intro a b c`, `fun a b =>`, `∀ a b,` bind several names.
                while let Some(&next) = words.peek() {
                    let name = next.trim_end_matches([',', ':']);
                    if name.is_empty()
                        || !name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        || LEAN.contains(&name)
                    {
                        break;
                    }
                    local.insert(name.to_string());
                    words.next();
                    if next.ends_with([',', ':'])
                        || ["theorem", "namespace", "let", "have"].contains(&w)
                    {
                        break;
                    }
                }
            }
        }

        let mut missing = std::collections::BTreeSet::new();
        for (i, tok) in tokens.iter().enumerate() {
            let tok = tok.trim_matches('.');
            if tok.is_empty() || LEAN.contains(&tok) || local.contains(tok) || lib.contains(tok) {
                continue;
            }
            // `import Proofs.Scheduling.RTA`: the module must be a file.
            if i > 0 && tokens[i - 1] == "import" {
                let rel = tok.replace('.', "/") + ".lean";
                if proofs_dir().join(&rel).is_file() {
                    continue;
                }
            }
            // `task.exec`: a local followed by declared field names.
            let mut parts = tok.split('.');
            let head = parts.next().unwrap_or("");
            if local.contains(head) && parts.all(|p| lib.contains(p)) {
                continue;
            }
            missing.insert(tok.to_string());
        }
        missing.into_iter().collect()
    }

    /// The model `proofs.yml` lake-builds: `FIXTURE` in
    /// tools/check_generated_proofs.py. Kept out of test-data/ because every
    /// first-party .aadl needs committed OSATE and Ocarina verdicts.
    const RTA_FIXTURE: &str = "\
package Rta
public
  processor Cpu
  end Cpu;
  thread Fast
    properties
      Dispatch_Protocol => Periodic;
      Period => 5 ms;
      Deadline => 5 ms;
      Compute_Execution_Time => 1 ms .. 1 ms;
  end Fast;
  thread Mid
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Deadline => 10 ms;
      Compute_Execution_Time => 2 ms .. 2 ms;
  end Mid;
  thread Slow
    properties
      Dispatch_Protocol => Periodic;
      Period => 20 ms;
      Deadline => 20 ms;
      Compute_Execution_Time => 5 ms .. 5 ms;
  end Slow;
  thread Late
    properties
      Dispatch_Protocol => Periodic;
      Period => 40 ms;
      Deadline => 12 ms;
      Compute_Execution_Time => 6 ms .. 6 ms;
  end Late;
  process App
  end App;
  process implementation App.impl
    subcomponents
      fast : thread Fast;
      mid : thread Mid;
      slow : thread Slow;
      late : thread Late;
  end App.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      app : process App.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.fast;
      Actual_Processor_Binding => (reference (cpu)) applies to app.mid;
      Actual_Processor_Binding => (reference (cpu)) applies to app.slow;
      Actual_Processor_Binding => (reference (cpu)) applies to app.late;
  end Top.impl;
end Rta;
";

    #[test]
    fn rta_fixture_matches_the_ci_gate_copy() {
        let tool = repo_root().join("tools/check_generated_proofs.py");
        let text = std::fs::read_to_string(&tool).expect("read check_generated_proofs.py");
        let start = text.find("FIXTURE = \"\"\"\\\n").expect("FIXTURE start") + 15;
        let end = start + text[start..].find("\"\"\"").expect("FIXTURE end");
        assert_eq!(
            &text[start..end],
            RTA_FIXTURE,
            "the CI gate's model and this test's model drifted apart"
        );
    }

    fn fixture_proof() -> String {
        let src = RTA_FIXTURE.to_string();
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "rta_proof.aadl".to_string(), src);
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("Rta"),
            &Name::new("Top"),
            &Name::new("impl"),
        );
        let (idx, _) = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Processor)
            .expect("fixture has a processor");
        generate_lean4_proof(&inst, idx).content
    }

    #[test]
    fn generated_proof_names_only_what_proofs_library_declares() {
        // #542: the generator called `compute_response_time`, which no file
        // under proofs/Proofs defines, so no generated proof could build.
        // The lake build in proofs.yml is the real oracle; this is the cheap
        // check that runs with `cargo test`.
        for content in [fixture_proof(), {
            let inst = build_test_instance();
            let (idx, _) = inst
                .all_components()
                .find(|(_, c)| c.category == ComponentCategory::Processor)
                .expect("processor");
            generate_lean4_proof(&inst, idx).content
        }] {
            assert!(
                content.matches("\ntheorem ").count() >= 2,
                "expected theorems to check, got:\n{content}"
            );
            let missing = undeclared_library_names(&content);
            assert!(
                missing.is_empty(),
                "generated proof uses names proofs/Proofs does not declare: {missing:?}\n{content}"
            );
            assert!(!content.contains("sorry"), "{content}");
        }
    }

    #[test]
    fn name_check_rejects_an_undeclared_library_name() {
        // The check must be able to fail: the #542 shape is caught.
        let bad = "import Proofs.Scheduling.RTA\n\ntheorem x :\n    \
                   match compute_response_time 1 2 [] with\n    | .converged r => r ≤ 2 := by\n  \
                   decide\n";
        let missing = undeclared_library_names(bad);
        assert!(
            missing.iter().any(|m| m == "compute_response_time"),
            "{missing:?}"
        );
        // And a missing module.
        let missing = undeclared_library_names("import Proofs.Scheduling.Nope\n");
        assert!(
            missing.iter().any(|m| m == "Proofs.Scheduling.Nope"),
            "{missing:?}"
        );
    }

    #[test]
    fn fixture_proof_covers_both_theorem_shapes() {
        let content = fixture_proof();
        for name in [
            "app__fast_meets_deadline",
            "app__mid_meets_deadline",
            "app__slow_meets_deadline",
            "app__late_misses_deadline",
        ] {
            assert!(
                content.contains(&format!("theorem {name} ")),
                "{name}:\n{content}"
            );
        }
        assert!(
            content.contains("have hfp : isFixedPoint task hp 9000000000"),
            "{content}"
        );
    }
}
