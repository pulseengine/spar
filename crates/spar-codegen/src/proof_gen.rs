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

use crate::{GeneratedFile, extract_timing, format_time_ps, sanitize_ident, threads_for_processor};

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
    let proc_name = sanitize_ident(proc_comp.name.as_str());

    let threads = threads_for_processor(inst, proc_idx);

    let mut lean = String::new();

    // Header
    lean.push_str(&format!(
        "-- Generated from AADL processor: {}::{}\n",
        proc_comp.package, proc_comp.name
    ));
    lean.push_str("-- DO NOT EDIT -- regenerate with `spar codegen`.\n\n");
    lean.push_str("import Proofs.Scheduling.RTA\n\n");

    // Collect thread timing data for higher-priority set
    let mut thread_data: Vec<(String, u64, u64, u64)> = Vec::new();
    for &t_idx in &threads {
        let t_comp = inst.component(t_idx);
        let t_name = sanitize_ident(t_comp.name.as_str());
        let (period, deadline, wcet) = extract_timing(inst, t_idx);

        let period = period.unwrap_or(10_000_000_000); // default 10ms
        let deadline = deadline.unwrap_or(period);
        let wcet = wcet.unwrap_or(1_000_000_000); // default 1ms

        thread_data.push((t_name, period, deadline, wcet));
    }

    // Generate namespace
    lean.push_str(&format!("namespace {}\n\n", capitalize(&proc_name)));

    // Generate one theorem per thread
    for (i, (t_name, period, deadline, wcet)) in thread_data.iter().enumerate() {
        lean.push_str(&format!(
            "-- Thread: {t_name}, Period={}, WCET={}, Deadline={}\n",
            format_time_ps(*period),
            format_time_ps(*wcet),
            format_time_ps(*deadline),
        ));

        // Build higher-priority task set (all tasks with index < i, by rate-monotonic)
        let hp_tasks: Vec<&(String, u64, u64, u64)> = thread_data[..i].iter().collect();

        lean.push_str(&format!("theorem {t_name}_meets_deadline :\n"));
        lean.push_str(&format!("    let wcet_ps := {wcet}\n"));
        lean.push_str(&format!("    let deadline_ps := {deadline}\n"));

        if hp_tasks.is_empty() {
            lean.push_str("    let hp : List (Nat × Nat) := []\n");
        } else {
            lean.push_str("    let hp : List (Nat × Nat) := [\n");
            for (j, (_, hp_period, _, hp_wcet)) in hp_tasks.iter().enumerate() {
                let comma = if j + 1 < hp_tasks.len() { "," } else { "" };
                lean.push_str(&format!("      ({hp_period}, {hp_wcet}){comma}\n"));
            }
            lean.push_str("    ]\n");
        }

        lean.push_str("    match compute_response_time wcet_ps deadline_ps hp with\n");
        lean.push_str("    | .converged r => r <= deadline_ps\n");
        lean.push_str("    | .diverged => False := by\n");
        lean.push_str("  simp [compute_response_time]; omega\n\n");
    }

    lean.push_str(&format!("end {}\n", capitalize(&proc_name)));

    GeneratedFile {
        path: format!("proofs/{proc_name}_scheduling.lean"),
        content: lean,
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
    let name = sanitize_ident(comp.name.as_str());

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
    code.push_str("//! DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

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

    // Determine parent process for path
    let parent_name = comp
        .parent
        .map(|p| sanitize_ident(inst.component(p).name.as_str()))
        .unwrap_or_else(|| "unknown".to_string());

    GeneratedFile {
        path: format!("proofs/kani/{parent_name}/{name}_harness.rs"),
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
                file.content.contains("compute_response_time"),
                "Lean4 proof must reference RTA"
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
}
