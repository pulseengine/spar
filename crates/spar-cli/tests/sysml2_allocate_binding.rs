//! REQ-SYSML2-ALLOCATE-BINDING-001 — end to end: a SysML v2 `allocate`
//! becomes an AADL `Actual_Processor_Binding` that `spar analyze` checks.
//!
//! Three arms over one model, so the green arm cannot pass vacuously:
//!
//! * (a) no `allocate`: binding_rules MUST report the thread as missing its
//!   required processor binding. This proves the thread is instantiated as a
//!   thread and the processor counts as one — without it, "no
//!   missing-binding error" in arm (b) would also hold for a model in which
//!   the thread never exists (which is what unmodified lowering produced).
//! * (b) `allocate app.t to cpu;`: the lowered AADL carries the binding, and
//!   binding_rules reports nothing about `t`.
//! * (c) `allocate app.t to board;` where `board` is a system: binding_rules
//!   reports a wrong-category binding target.

use std::path::{Path, PathBuf};
use std::process::Command;

const MODEL: &str = r#"
package Deploy {
    private import AADL::*;
    part def Worker :> Thread;
    part def Cpu :> Processor;
    part def Board :> System;
    part def App :> Process {
        part t : Worker;
    }
    part def Platform :> System {
        part app : App;
        part cpu : Cpu;
        part board : Board;
        ALLOC
    }
}
"#;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-sysml2-alloc-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Lower the model with `alloc` spliced in, then analyze the lowered AADL.
/// Returns (lowered AADL text, analyze stdout+stderr).
fn lower_then_analyze(tag: &str, alloc: &str) -> (String, String) {
    let dir = workdir(tag);
    let sysml = dir.join("deploy.sysml");
    let aadl = dir.join("deploy.aadl");
    std::fs::write(&sysml, MODEL.replace("ALLOC", alloc)).unwrap();

    let lower = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["sysml2", "lower", "-o"])
        .arg(&aadl)
        .arg(&sysml)
        .output()
        .expect("run spar sysml2 lower");
    assert!(
        lower.status.success(),
        "lower failed: {}",
        String::from_utf8_lossy(&lower.stderr)
    );
    let text = std::fs::read_to_string(&aadl).unwrap();

    let out = analyze(&aadl);
    let _ = std::fs::remove_dir_all(&dir);
    (text, out)
}

fn analyze(aadl: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Deploy::Platform.impl"])
        .arg(aadl)
        .output()
        .expect("run spar analyze");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !all.contains("parse error") && !all.contains("Cannot"),
        "lowered AADL did not load:\n{all}"
    );
    all
}

const MISSING: &str = "thread 't' is missing required Actual_Processor_Binding";

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn without_allocate_the_thread_is_reported_unbound() {
    let (_, out) = lower_then_analyze("none", "");
    assert!(
        out.contains(MISSING),
        "control arm must see the thread:\n{out}"
    );
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_processor_is_a_binding_spar_analyze_accepts() {
    let (text, out) = lower_then_analyze("cpu", "allocate app.t to cpu;");
    assert!(
        text.contains(
            "Deployment_Properties::Actual_Processor_Binding => (reference (cpu)) applies to app.t;"
        ),
        "{text}"
    );
    assert!(!out.contains(MISSING), "{out}");
    assert!(!out.contains("Actual_Processor_Binding target"), "{out}");
    assert!(!out.contains("does not exist"), "{out}");
    // The instance really contains the bound thread.
    assert!(out.contains("Instantiated Deploy::Platform.impl"), "{out}");
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_a_system_is_reported_as_a_wrong_category_target() {
    let (text, out) = lower_then_analyze("board", "allocate app.t to board;");
    assert!(
        text.contains("Actual_Processor_Binding => (reference (board)) applies to app.t;"),
        "{text}"
    );
    assert!(
        out.contains(
            "Actual_Processor_Binding target 'board' is a system component, expected one of: processor, virtual processor"
        ),
        "{out}"
    );
}
