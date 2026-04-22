//! Integration test for GitHub issues #128 and #129:
//!
//!   #128 — `binding_rules` analyzer missed `Actual_Processor_Binding`
//!         declared with `applies to fw.firmware` (dotted path).
//!   #129 — `spar instance --format json` omitted all `properties`.
//!
//! Both have the same root cause: property associations with a non-empty
//! `applies to <path>` clause were dropped when converting
//! PropertyAssociationItem → PropertyValue during instantiation. The fix
//! attaches them eagerly to the resolved target instance.

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// AS5506B §8.3 reproducer from issue #128: `applies to fw.firmware`
/// where `fw` is a process subcomponent containing thread `firmware`.
const MODEL: &str = "\
package Test_Applies_To
public
  processor NRF52840
  end NRF52840;

  thread DoorFirmware
  end DoorFirmware;

  process DoorFirmwareProcess
  end DoorFirmwareProcess;

  process implementation DoorFirmwareProcess.Impl
    subcomponents
      firmware: thread DoorFirmware;
  end DoorFirmwareProcess.Impl;

  system DoorNode
  end DoorNode;

  system implementation DoorNode.Battery
    subcomponents
      mcu: processor NRF52840;
      fw: process DoorFirmwareProcess.Impl;
    properties
      Actual_Processor_Binding => (reference (mcu)) applies to fw.firmware;
  end DoorNode.Battery;
end Test_Applies_To;
";

fn write_model(tag: &str) -> std::path::PathBuf {
    // Per-test tag: cargo runs tests in parallel within the same process,
    // so process::id() alone collides. The trailing tag disambiguates so
    // one test's fs::remove_file does not race another test's spar
    // invocation reading the same path.
    let path = env::temp_dir().join(format!(
        "spar_applies_to_nested_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, MODEL).expect("write temp AADL");
    path
}

/// #128: binding_rules must see the binding on the thread instance.
#[test]
fn issue_128_binding_rules_accepts_nested_applies_to() {
    let path = write_model("128");
    let output = spar()
        .arg("analyze")
        .arg("--root")
        .arg("Test_Applies_To::DoorNode.Battery")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    assert!(
        !combined.contains("missing required Actual_Processor_Binding"),
        "binding_rules still reports the thread as unbound — #128 regression.\n\
         combined output:\n{combined}"
    );

    let _ = fs::remove_file(&path);
}

/// Unresolvable-path fallback: a property association whose `applies to`
/// path does not resolve to a real subcomponent must not crash the
/// pipeline. The property stays on the declaring component and a
/// diagnostic is emitted; `spar instance` still completes.
#[test]
fn applies_to_unresolvable_path_emits_diagnostic() {
    let src = "\
package Test_Unresolvable
public
  processor Proc
  end Proc;

  system Sys
  end Sys;

  system implementation Sys.Impl
    subcomponents
      cpu: processor Proc;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to no_such_sub.no_such_thread;
  end Sys.Impl;
end Test_Unresolvable;
";
    let path = env::temp_dir().join(format!("spar_applies_to_bad_{}.aadl", std::process::id()));
    fs::write(&path, src).expect("write temp AADL");

    let output = spar()
        .arg("instance")
        .arg("--root")
        .arg("Test_Unresolvable::Sys.Impl")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    assert!(
        output.status.success(),
        "spar instance must not crash on unresolvable applies_to; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("applies_to path")
            && stderr.contains("no_such_sub.no_such_thread")
            && stderr.contains("could not be resolved"),
        "expected unresolved-path diagnostic in stderr, got:\n{stderr}"
    );

    let _ = fs::remove_file(&path);
}

/// #129: `spar instance --format json` must emit the property on the
/// target instance, not the declaring system.
#[test]
fn issue_129_instance_json_includes_applies_to_properties() {
    let path = write_model("129");
    let output = spar()
        .arg("instance")
        .arg("--root")
        .arg("Test_Applies_To::DoorNode.Battery")
        .arg("--format")
        .arg("json")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    assert!(
        output.status.success(),
        "spar instance failed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The property is attached to the thread instance `firmware` that
    // lives under `fw`. A serialized snippet would look like:
    //   "name": "firmware", ..., "properties": { "Actual_Processor_Binding": ... }
    // We do a shape-insensitive assertion against the raw text.
    assert!(
        stdout.contains("Actual_Processor_Binding"),
        "instance JSON does not contain the expected property name — #129 regression.\n\
         stdout:\n{stdout}"
    );
    // Bonus guard: the reference text should be present too.
    assert!(
        stdout.contains("reference") && stdout.contains("mcu"),
        "instance JSON does not contain the bound target — #129 regression.\n\
         stdout:\n{stdout}"
    );

    let _ = fs::remove_file(&path);
}
