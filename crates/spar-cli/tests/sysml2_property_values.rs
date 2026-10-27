//! `spar sysml2 lower` end to end, for REQ-SYSML2-PROPERTY-VALUES-001.
//!
//! Each test writes an inline SysML v2 model to its own temp directory and
//! runs the real binary, so what is asserted is what an engineer sees: the
//! AADL on stdout, the warnings on stderr and the exit status.

use std::path::PathBuf;
use std::process::{Command, Output};

fn lower(test: &str, model: &str) -> Output {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "spar_sysml2_property_values_{test}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.sysml");
    std::fs::write(&path, model).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["sysml2", "lower", path.to_str().unwrap()])
        .output()
        .expect("failed to run spar");
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// G1: a property-bearing construct the lowering cannot express is reported
/// on stderr, with its source position, instead of vanishing.
// rivet: verifies REQ-SYSML2-PROPERTY-VALUES-001
#[test]
fn lower_reports_dropped_property_bearing_constructs() {
    let out = lower(
        "g1",
        "package P {\n\
         \x20   part def Cpu { }\n\
         \x20   part def Base { attribute period = 10 ms; }\n\
         \x20   part def Sys {\n\
         \x20       part b : Base { attribute period = 7 ms; }\n\
         \x20       part cpu : Cpu;\n\
         \x20       allocate b to cpu;\n\
         \x20   }\n\
         }\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Warnings, not errors: the AADL that could be lowered is still produced.
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("period => 10 ms;"), "{stdout}");
    assert!(
        stderr.contains(
            "warning at 5:25: unsupported: `attribute period = 7 ms` in the body of part usage `b` (not lowered"
        ),
        "{stderr}"
    );
    // An allocate in a part def body is LOWERED, to a deployment binding
    // (REQ-SYSML2-ALLOCATE-BINDING-001), so it is not reported as dropped.
    assert!(
        stdout.contains("Actual_Processor_Binding => (reference (cpu)) applies to b;"),
        "{stdout}"
    );
    assert!(!stderr.contains("unsupported: `allocate"), "{stderr}");
}

/// G2: `10 [ms]` lowers to the same AADL as `10 ms`; a wrong-dimension unit on
/// a Time property is reported.
// rivet: verifies REQ-SYSML2-PROPERTY-VALUES-001
#[test]
fn lower_accepts_bracketed_units_and_checks_their_dimension() {
    let out = lower(
        "g2",
        "package P {\n\
         \x20   part def A {\n\
         \x20       attribute period = 10 [ms];\n\
         \x20       attribute deadline = 4 [SI::kg];\n\
         \x20   }\n\
         }\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("period => 10 ms;"), "{stdout}");
    assert!(
        stderr.contains(
            "warning at 4:9: unit `kg` is not a Time_Units unit: `deadline` matches the \
             predeclared AADL property Timing_Properties::Deadline"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("`period`"), "{stderr}");
}

/// G3: an attribute declared without a value does not lower its TYPE NAME
/// as the property value.
// rivet: verifies REQ-SYSML2-PROPERTY-VALUES-001
#[test]
fn lower_does_not_emit_a_type_name_as_a_value() {
    let out = lower(
        "g3",
        "package P {\n\
         \x20   part def Imu {\n\
         \x20       attribute biasStability : Real;\n\
         \x20       attribute rate = 100;\n\
         \x20   }\n\
         }\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stderr}");
    assert!(!stdout.contains("biasStability"), "{stdout}");
    assert!(!stdout.contains("=> Real"), "{stdout}");
    assert!(stdout.contains("rate => 100;"), "{stdout}");
}
