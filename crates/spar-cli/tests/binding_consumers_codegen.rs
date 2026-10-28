//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — codegen's per-processor proof
//! takes the threads bound to THAT processor, by identity.
//!
//! `TwoCpu` has two different processors both named `cpu`, one 6 ms / 10 ms
//! thread each. `threads_for_processor` compared each binding's last segment
//! with the processor's NAME, so on the published v0.46.0 binary each `cpu`
//! collected both threads, and `spar codegen --verify proof` emitted a theorem
//! that charges `t` 6 ms of interference from a thread that runs on the OTHER
//! processor — 6 + 6 = 12 ms against a 10 ms deadline: a FALSE theorem.
//!
//! (Both processors also wrote `proofs/cpu_scheduling.lean`, so one proof
//! overwrote the other — a separate defect, #510. Proof files are now named
//! by instance path, `proofs/left__cpu_scheduling.lean` and
//! `proofs/right__cpu_scheduling.lean`; see the #510 test at the end.)

use std::process::Command;

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
const ONE_CPU_TWO_SPELLINGS: &str = r#"package OneCpu
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
      t1 : thread W;
      t2 : thread W;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
    properties
      Actual_Processor_Binding => (reference (left.cpu)) applies to left.app.t2;
      Actual_Processor_Binding => (reference (right.cpu)) applies to right.app.t2;
  end Top.impl;
end OneCpu;
"#;

/// Run `spar codegen --verify proof` and return its output directory.
fn codegen_proofs(root: &str, name: &str, src: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-bind-codegen-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("m.aadl");
    std::fs::write(&model, src).unwrap();
    let out_dir = dir.join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "codegen",
            "--root",
            root,
            "--verify",
            "proof",
            "--output",
            out_dir.to_str().unwrap(),
            model.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    assert!(
        out.status.success(),
        "codegen failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out_dir
}

/// The generated Lean scheduling proof for processor `left.cpu`.
fn lean_proof(root: &str, name: &str, src: &str) -> String {
    let out_dir = codegen_proofs(root, name, src);
    std::fs::read_to_string(out_dir.join("proofs/left__cpu_scheduling.lean")).expect("proof file")
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_processor_proof_holds_only_its_own_threads() {
    let lean = lean_proof("TwoCpu::Top.impl", "two", TWO_CPUS_SAME_NAME);
    assert_eq!(lean.matches("theorem ").count(), 1, "{lean}");
    assert!(
        !lean.contains("(10000000000, 6000000000)"),
        "no interference from the other processor's thread: {lean}"
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_processor_named_two_ways_holds_both_its_threads() {
    // Negative control: each `cpu` really has two threads (bound as `cpu`
    // and as `left.cpu` / `right.cpu`). Both belong in its proof.
    let lean = lean_proof("OneCpu::Top.impl", "one", ONE_CPU_TWO_SPELLINGS);
    assert_eq!(lean.matches("theorem ").count(), 2, "{lean}");
}

// rivet: verifies REQ-PROOF-GEN-NAMES-INJECTIVE-001
#[test]
fn same_named_processors_each_keep_their_proof_on_disk() {
    // #510: on main both processors wrote `proofs/cpu_scheduling.lean`, the
    // second silently overwriting the first. On disk now: one proof file
    // per processor, each naming its own processor and its own thread.
    let out_dir = codegen_proofs("TwoCpu::Top.impl", "510", TWO_CPUS_SAME_NAME);
    let mut lean: Vec<String> = std::fs::read_dir(out_dir.join("proofs"))
        .expect("proofs dir")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".lean"))
        .collect();
    lean.sort();
    assert_eq!(
        lean,
        ["left__cpu_scheduling.lean", "right__cpu_scheduling.lean"]
    );
    for side in ["left", "right"] {
        let proof =
            std::fs::read_to_string(out_dir.join(format!("proofs/{side}__cpu_scheduling.lean")))
                .unwrap();
        assert!(
            proof.contains(&format!("-- Instance: Top.impl/{side}/cpu\n")),
            "{proof}"
        );
        assert!(
            proof.contains(&format!("theorem {side}__app__t_meets_deadline :")),
            "{proof}"
        );
        let harness = out_dir.join(format!("proofs/kani/{side}__app/t_harness.rs"));
        assert!(harness.is_file(), "missing {}", harness.display());
    }
}
