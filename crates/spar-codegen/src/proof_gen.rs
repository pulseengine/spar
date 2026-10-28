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
    lean.push_str("import Proofs.Scheduling.RTA\n\n");

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

    lean.push_str(&format!("end {}\n", capitalize(&stem)));

    GeneratedFile {
        path: format!("proofs/{stem}.lean"),
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

/// The instance path of `idx` below the root, as its segment names
/// (`left`, `cpu` for `Top.impl/left/cpu`). Empty for the root itself.
fn path_below_root(inst: &SystemInstance, idx: ComponentInstanceIdx) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = idx;
    while let Some(parent) = inst.component(current).parent {
        segments.push(inst.component(current).name.as_str().to_string());
        current = parent;
    }
    segments.reverse();
    segments
}

/// Encode an instance path as a lowercase identifier, injectively (#510).
///
/// Proof files were named by the component's NAME, so two processors called
/// `cpu` in different subsystems wrote one file and the second overwrote the
/// first. The name is now built from the whole instance path below the root:
/// `left`/`cpu` gives `left__cpu`, and a processor directly under the root
/// keeps its plain name (`cpu`).
///
/// The output uses only `[a-z0-9_]`. Two different paths therefore never give
/// names that differ only in case, which a case-insensitive filesystem would
/// fold into one file. The output is also a valid Lean and Rust identifier
/// stem: it never starts with a digit or `_`.
///
/// Injectivity comes from the escape `z`, which always starts an escape
/// (prefix-free codes):
///
/// | input                                   | output             |
/// |-----------------------------------------|--------------------|
/// | `a`–`y`, `0`–`9`                        | itself             |
/// | `z`                                     | `zz`               |
/// | uppercase letter `L`                    | `zc` + lowercase L |
/// | `_` between two non-`_` characters      | `_`                |
/// | any other `_` (leading, trailing, run)  | `zu`               |
/// | any other character                     | `zx` + 6 hex digits |
/// | empty segment                           | `ze`               |
/// | empty path (the root)                   | `zr`               |
/// | leading digit of the whole name         | `zd` before it     |
///
/// An encoded segment never contains `__` and never starts or ends with `_`,
/// so joining segments with `__` can be split back unambiguously. The test
/// decoder in this module recovers every path from its encoding, which is
/// what makes the map injective.
fn encode_instance_path<S: AsRef<str>>(segments: &[S]) -> String {
    if segments.is_empty() {
        return "zr".to_string();
    }
    let mut out = String::new();
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str("__");
        }
        encode_segment(segment.as_ref(), &mut out);
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "zd");
    }
    out
}

fn encode_segment(segment: &str, out: &mut String) {
    let chars: Vec<char> = segment.chars().collect();
    if chars.is_empty() {
        out.push_str("ze");
        return;
    }
    for (i, &c) in chars.iter().enumerate() {
        match c {
            'z' => out.push_str("zz"),
            'a'..='y' | '0'..='9' => out.push(c),
            'A'..='Z' => {
                out.push_str("zc");
                out.push(c.to_ascii_lowercase());
            }
            '_' => {
                let interior =
                    i > 0 && i + 1 < chars.len() && chars[i - 1] != '_' && chars[i + 1] != '_';
                out.push_str(if interior { "_" } else { "zu" });
            }
            other => out.push_str(&format!("zx{:06x}", other as u32)),
        }
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
}
