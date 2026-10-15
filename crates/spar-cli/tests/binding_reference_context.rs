//! REQ-AADL-REF-CONTEXT-001 (#460) — a binding's `reference (...)` resolves
//! from the component that DECLARED the association, as OSATE does.
//!
//! Each model has two components named `cpu`: a processor where the declaring
//! implementation names it, and a MEMORY deeper in the tree. The old resolver
//! took the deepest component of that name anywhere in the model, so on these
//! LEGAL models `spar analyze` reported
//! `Actual_Processor_Binding ... 'cpu' which is a memory component` — from both
//! `binding_check` and `binding_rules` — having bound a processor the author
//! never named.
//!
//! NON-VACUITY: the observable is that category error, which the published
//! v0.46.0 binary prints on both models. A binding that really does name a
//! memory, from the declaring context, must still produce it, so a build that
//! merely silenced the category check cannot pass.
//!
//! Reference semantics: osate2 @ 87468992ee0d4c4cc856a72bd542899953f088f3,
//! `ReferenceValueImpl.instantiate` (an exact containment-path walk from the
//! context) and `CacheContainedPropertyAssociationsSwitch` ("replace reference
//! values in the context of the contained PA's owner").

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// `reference (cpu)` declared in Top.impl, applied to `sub.app`. Top.impl/cpu
/// is a processor; Top.impl/sub/cpu is a memory.
const DECLARED_AT_TOP: &str = r#"
package CtxTop
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      app : process P.impl;
      cpu : memory Mem;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      sub : system Sub.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to sub.app;
  end Top.impl;
end CtxTop;
"#;

/// The mirror: `reference (cpu)` declared in Sub.impl. Sub.impl/cpu is a
/// processor; Sub.impl/inner/cpu, one level deeper, is a memory.
const DECLARED_IN_SUB: &str = r#"
package CtxSub
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system Inner
  end Inner;
  system implementation Inner.impl
    subcomponents
      cpu : memory Mem;
  end Inner.impl;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      app : process P.impl;
      cpu : processor Cpu;
      inner : system Inner.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      sub : system Sub.impl;
  end Top.impl;
end CtxSub;
"#;

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-ref-ctx-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// The `binding_check` / `binding_rules` lines saying the binding names a memory.
fn memory_category_errors(root: &str, name: &str, src: &str) -> Vec<String> {
    let file = tmp(name, src);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stdout.contains("binding_rules") || stderr.contains("binding_rules"),
        "the binding passes must have run on {name} — otherwise an empty result proves nothing:\n{stdout}{stderr}"
    );
    stdout
        .lines()
        .chain(stderr.lines())
        .filter(|l| l.contains("Actual_Processor_Binding") && l.contains("memory component"))
        .map(str::to_string)
        .collect()
}

// rivet: verifies REQ-AADL-REF-CONTEXT-001
#[test]
fn a_binding_declared_at_the_top_names_the_tops_cpu() {
    let errs = memory_category_errors("CtxTop::Top.impl", "top.aadl", DECLARED_AT_TOP);
    assert!(errs.is_empty(), "bound the deeper memory: {errs:#?}");
}

// rivet: verifies REQ-AADL-REF-CONTEXT-001
#[test]
fn a_binding_declared_in_a_subsystem_names_the_subsystems_cpu() {
    let errs = memory_category_errors("CtxSub::Top.impl", "sub.aadl", DECLARED_IN_SUB);
    assert!(errs.is_empty(), "bound the deeper memory: {errs:#?}");
}

// rivet: verifies REQ-AADL-REF-CONTEXT-001
#[test]
fn a_binding_that_really_names_a_memory_is_still_an_error() {
    // Negative control: from Top.impl, `sub.cpu` IS the memory. The category
    // check must still fire, from both passes.
    let src = DECLARED_AT_TOP.replace("(reference (cpu))", "(reference (sub.cpu))");
    let errs = memory_category_errors("CtxTop::Top.impl", "real_memory.aadl", &src);
    assert_eq!(
        errs.len(),
        2,
        "binding_check and binding_rules must both reject a memory: {errs:#?}"
    );
}
