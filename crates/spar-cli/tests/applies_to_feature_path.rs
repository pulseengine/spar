//! AADL v2.3 (AS5506D §11.3): `applies to` paths may end with a feature
//! name. This test covers a property association with a feature-path
//! target — e.g. `Latency => 5 ms .. 10 ms applies to fw.input_port;`.
//!
//! Pre-fix behavior: spar rejected the path because the final segment
//! `input_port` was not a subcomponent, emitting a spurious "could not
//! be resolved" diagnostic and dropping the property.
//!
//! Post-fix behavior: the path resolves to a feature; the property is
//! recorded against the owning component instance and no diagnostic is
//! emitted.

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

const MODEL_FEATURE_PATH: &str = "\
package Test_Applies_To_Feature
public
  processor Cpu
  end Cpu;

  thread Worker
    features
      input_port: in data port;
  end Worker;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      w: thread Worker;
  end Proc.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu: processor Cpu;
      fw: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to fw.w;
      Required_Connection_Quality_Of_Service => (Latency) applies to fw.w.input_port;
  end Sys.Impl;
end Test_Applies_To_Feature;
";

fn write_model(tag: &str) -> std::path::PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_applies_to_feature_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, MODEL_FEATURE_PATH).expect("write temp AADL");
    path
}

#[test]
fn applies_to_feature_path_does_not_emit_unresolved_diagnostic() {
    let path = write_model("nodiag");
    let output = spar()
        .arg("instance")
        .arg("--root")
        .arg("Test_Applies_To_Feature::Sys.Impl")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    assert!(
        output.status.success(),
        "spar instance must not crash on feature-path applies_to; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("fw.w.input_port") || !stderr.contains("could not be resolved"),
        "spar incorrectly rejected feature-path applies_to as unresolvable.\nstderr:\n{stderr}"
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn applies_to_unknown_segment_still_emits_diagnostic() {
    let src = "\
package Test_Bad_Feature_Path
public
  processor Cpu
  end Cpu;

  thread Worker
    features
      input_port: in data port;
  end Worker;

  process Proc
  end Proc;

  process implementation Proc.Impl
    subcomponents
      w: thread Worker;
  end Proc.Impl;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu: processor Cpu;
      fw: process Proc.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to fw.w.no_such_port;
  end Sys.Impl;
end Test_Bad_Feature_Path;
";
    let path = env::temp_dir().join(format!(
        "spar_applies_to_bad_feature_{}.aadl",
        std::process::id()
    ));
    fs::write(&path, src).expect("write temp AADL");

    let output = spar()
        .arg("instance")
        .arg("--root")
        .arg("Test_Bad_Feature_Path::Sys.Impl")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no_such_port") && stderr.contains("could not be resolved"),
        "expected unresolved-feature diagnostic in stderr, got:\n{stderr}"
    );

    let _ = fs::remove_file(&path);
}
