//! Test harness generation from AADL thread instances.
//!
//! Generates Rust test files that exercise the dispatch logic of each
//! thread component, including timing property assertions and port
//! connectivity checks.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

use crate::{GeneratedFile, extract_timing, sanitize_ident, to_pascal_case};

/// Generate a test harness for a thread instance.
pub fn generate_test_harness(
    inst: &SystemInstance,
    thread_idx: ComponentInstanceIdx,
) -> GeneratedFile {
    let comp = inst.component(thread_idx);
    let name = sanitize_ident(comp.name.as_str());
    let struct_name = to_pascal_case(comp.name.as_str());

    let (period, deadline, wcet) = extract_timing(inst, thread_idx);

    let mut code = String::new();

    code.push_str(&format!(
        "//! Generated tests for AADL thread: {}::{}\n",
        comp.package, comp.name
    ));
    code.push_str("//! DO NOT EDIT — regenerate with `spar codegen`.\n\n");

    code.push_str(&format!("use super::{name}::*;\n\n"));

    // The harness may only call lifecycle methods the thread's `Dispatch_Protocol`
    // actually exposes on the trait — a Periodic thread has `compute` only, so
    // emitting `initialize`/`finalize` calls would reference methods that
    // rust_gen no longer generates (REQ-CODEGEN-WIT-RECORDS-001 #319 item 7).
    let dispatch = crate::dispatch_protocol(inst, thread_idx);
    let methods = crate::lifecycle_for(&dispatch).methods();
    let has_initialize = methods.contains(&"initialize");
    let has_finalize = methods.contains(&"finalize");

    // Test: component initializes without panic (only if it has an initialize).
    if has_initialize {
        code.push_str("#[test]\n");
        code.push_str(&format!("fn {name}_initializes() {{\n"));
        code.push_str(&format!("    let mut comp = {struct_name}Default;\n"));
        code.push_str(&format!(
            "    let mut ports = {struct_name}Ports::default();\n"
        ));
        code.push_str("    comp.initialize(&mut ports);\n");
        code.push_str("}\n\n");
    }

    // Test: compute dispatch executes (all threads have `compute`).
    code.push_str("#[test]\n");
    code.push_str(&format!("fn {name}_compute_dispatches() {{\n"));
    code.push_str(&format!("    let mut comp = {struct_name}Default;\n"));
    code.push_str(&format!(
        "    let mut ports = {struct_name}Ports::default();\n"
    ));
    if has_initialize {
        code.push_str("    comp.initialize(&mut ports);\n");
    }
    code.push_str("    comp.compute(&mut ports);\n");
    code.push_str("}\n\n");

    // Test: finalize executes (only if it has a finalize).
    if has_finalize {
        code.push_str("#[test]\n");
        code.push_str(&format!("fn {name}_finalizes() {{\n"));
        code.push_str(&format!("    let mut comp = {struct_name}Default;\n"));
        code.push_str(&format!(
            "    let mut ports = {struct_name}Ports::default();\n"
        ));
        if has_initialize {
            code.push_str("    comp.initialize(&mut ports);\n");
        }
        code.push_str("    comp.finalize(&mut ports);\n");
        code.push_str("}\n\n");
    }

    // Test: timing constants are consistent
    if period.is_some() || deadline.is_some() || wcet.is_some() {
        code.push_str("#[test]\n");
        code.push_str(&format!("fn {name}_timing_consistent() {{\n"));

        if let (Some(_p), Some(_d)) = (period, deadline) {
            code.push_str("    // Deadline must not exceed period\n");
            code.push_str("    assert!(DEADLINE_PS <= PERIOD_PS, \"deadline exceeds period\");\n");
        }

        if let (Some(_w), Some(_d)) = (wcet, deadline) {
            code.push_str("    // WCET must not exceed deadline\n");
            code.push_str("    assert!(WCET_PS <= DEADLINE_PS, \"WCET exceeds deadline\");\n");
        }

        if let (Some(_w), Some(_p)) = (wcet, period) {
            code.push_str("    // WCET must not exceed period\n");
            code.push_str("    assert!(WCET_PS <= PERIOD_PS, \"WCET exceeds period\");\n");
        }

        code.push_str("}\n\n");
    }

    // Test: port count matches model
    let port_count = comp.features.len();
    code.push_str("#[test]\n");
    code.push_str(&format!("fn {name}_has_expected_ports() {{\n"));
    code.push_str(&format!(
        "    // AADL model declares {port_count} features\n"
    ));
    code.push_str(&format!("    let ports = {struct_name}Ports::default();\n"));
    code.push_str("    let _ = std::mem::size_of_val(&ports);\n");
    code.push_str("}\n");

    // Determine parent process for path
    let parent_name = comp
        .parent
        .map(|p| sanitize_ident(inst.component(p).name.as_str()))
        .unwrap_or_else(|| "unknown".to_string());

    GeneratedFile {
        path: format!("tests/{parent_name}/{name}_test.rs"),
        content: code,
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
    thread CtrlThread
        properties
            Thread_Properties::Dispatch_Protocol => Sporadic;
    end CtrlThread;

    thread implementation CtrlThread.Impl
    end CtrlThread.Impl;

    process Controller
    end Controller;

    process implementation Controller.Impl
        subcomponents
            ctrl_thread: thread CtrlThread.Impl;
    end Controller.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
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
    fn test_harness_generation() {
        let inst = build_test_instance();
        let thread_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx);

        if let Some(idx) = thread_idx {
            // CtrlThread is Sporadic → full initialize/compute/finalize lifecycle.
            let file = generate_test_harness(&inst, idx);
            assert!(file.path.contains("_test.rs"));
            assert!(file.content.contains("#[test]"));
            assert!(file.content.contains("_initializes"));
            assert!(file.content.contains("_compute_dispatches"));
            assert!(file.content.contains("_finalizes"));
        }
    }

    /// A Periodic thread exposes only `compute`, so its harness must NOT call
    /// (or define tests for) initialize/finalize — those methods no longer exist
    /// on the trait (REQ-CODEGEN-WIT-RECORDS-001 #319 item 7). Guards against the
    /// test_gen/rust_gen desync the lifecycle trimming could introduce.
    #[test]
    fn periodic_harness_omits_initialize_and_finalize() {
        let aadl = r#"
package PerPkg
public
    thread PWorker
        properties
            Thread_Properties::Dispatch_Protocol => Periodic;
    end PWorker;

    thread implementation PWorker.Impl
    end PWorker.Impl;

    process Proc
    end Proc;

    process implementation Proc.Impl
        subcomponents
            w: thread PWorker.Impl;
    end Proc.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            p: process Proc.Impl;
    end Top.Impl;
end PerPkg;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "per.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("PerPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Thread)
            .map(|(idx, _)| idx)
            .expect("model has a thread");
        let file = generate_test_harness(&inst, idx);

        assert!(
            file.content.contains("_compute_dispatches"),
            "periodic harness must still test compute:\n{}",
            file.content
        );
        assert!(
            !file.content.contains("comp.initialize"),
            "periodic harness must not call initialize:\n{}",
            file.content
        );
        assert!(
            !file.content.contains("comp.finalize"),
            "periodic harness must not call finalize:\n{}",
            file.content
        );
    }
}
