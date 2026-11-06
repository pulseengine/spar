//! REQ-DBC-CYCLE-TIME-001, #476: a CAN frame's cycle time reaches the model,
//! and a frame that gets no rate is reported.
//!
//! Up to v0.48.2 `spar dbc` dropped `GenMsgCycleTime` on ingest: the crate
//! never read a DBC attribute, so every model ingested from a `.dbc` was
//! timing-blind and said nothing about it.
//!
//! The oracle is spar's own consumer. Each case runs `spar dbc`, then
//! `spar instance --format json` on what it wrote, and reads the rate off the
//! transmitting port, so the value is checked after spar's parser and
//! instantiation have read it, not as text in the emitter's output.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const HEADER: &str = r#"VERSION "1.0"

NS_ :
    BA_DEF_
    BA_
    BA_DEF_DEF_

BS_:

BU_: Ecu1 Ecu2

"#;

/// Five frames, Ecu1 to Ecu2, one per resolution path:
/// `Explicit` has its own 30 ms; `Defaulted` has no `BA_` and inherits the
/// 100 ms default; `Sporadic` is 0 (non-cyclic); `Ext` is an extended ID with
/// 20 ms; `Orphan` has 50 ms but no transmitter, so no flow and no port.
const FRAMES: &str = r#"BO_ 100 Explicit: 8 Ecu1
 SG_ A : 0|8@1+ (1,0) [0|255] "" Ecu2

BO_ 200 Defaulted: 8 Ecu1
 SG_ B : 0|8@1+ (1,0) [0|255] "" Ecu2

BO_ 300 Sporadic: 8 Ecu1
 SG_ C : 0|8@1+ (1,0) [0|255] "" Ecu2

BO_ 2147483748 Ext: 8 Ecu1
 SG_ D : 0|8@1+ (1,0) [0|255] "" Ecu2

BO_ 400 Orphan: 8 Vector__XXX
 SG_ E : 0|8@1+ (1,0) [0|255] "" Ecu2

BA_DEF_ BO_ "GenMsgCycleTime" INT 0 65535;
BA_DEF_DEF_ "GenMsgCycleTime" 100;
BA_ "GenMsgCycleTime" BO_ 100 30;
BA_ "GenMsgCycleTime" BO_ 300 0;
BA_ "GenMsgCycleTime" BO_ 2147483748 20;
BA_ "GenMsgCycleTime" BO_ 400 50;
"#;

struct Ingested {
    stderr: String,
    instance: serde_json::Value,
}

fn ingest(dbc: &str) -> Ingested {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!("spar-dbc-cycle-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let n = N.fetch_add(1, Ordering::SeqCst);
    let src = dir.join(format!("net-{n}.dbc"));
    let aadl = dir.join(format!("net-{n}.aadl"));
    fs::write(&src, dbc).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "dbc",
            src.to_str().unwrap(),
            "-o",
            aadl.to_str().unwrap(),
            "--package",
            "Can",
        ])
        .output()
        .expect("spar dbc runs");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "spar dbc failed: {stderr}");
    let inst = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "instance",
            "--root",
            "Can::CAN_Network.impl",
            "--format",
            "json",
            aadl.to_str().unwrap(),
        ])
        .output()
        .expect("spar instance runs");
    let instance: serde_json::Value = serde_json::from_slice(&inst.stdout).unwrap_or_else(|e| {
        panic!(
            "instance --format json must parse ({e}): {}{}",
            String::from_utf8_lossy(&inst.stdout),
            String::from_utf8_lossy(&inst.stderr)
        )
    });
    Ingested { stderr, instance }
}

/// The `Output_Rate` the instance carries on Ecu1's port for `frame`, if any.
fn rate_on_tx_port(i: &Ingested, frame: &str) -> Option<String> {
    let ecu1 = i.instance["children"]
        .as_array()
        .expect("children")
        .iter()
        .find(|c| c["name"] == "ecu1")
        .expect("child ecu1");
    let port = ecu1["features"]
        .as_array()
        .expect("features")
        .iter()
        .find(|f| f["name"] == format!("{frame}_tx"))
        .unwrap_or_else(|| panic!("feature {frame}_tx on ecu1: {ecu1:#}"));
    port["properties"]["Communication_Properties::Output_Rate"]
        .as_str()
        .map(str::to_string)
}

/// The rate's `Value_Range` lower bound, in frames per second.
fn per_second(rate: &str) -> f64 {
    let after = rate
        .split("Value_Range")
        .nth(1)
        .expect("a Value_Range field");
    let num: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    assert!(
        rate.contains("PerSecond"),
        "rate unit must be PerSecond: {rate}"
    );
    num.trim_end_matches('.').parse().expect("a number")
}

fn assert_cycle_ms(i: &Ingested, frame: &str, ms: f64) {
    let rate = rate_on_tx_port(i, frame)
        .unwrap_or_else(|| panic!("{frame} must carry an Output_Rate on its tx port"));
    let back = 1000.0 / per_second(&rate);
    assert!(
        (back - ms).abs() < 1e-9,
        "{frame}: the rate {rate} must recover the {ms} ms cycle (got {back})"
    );
}

fn warned<'a>(i: &'a Ingested, frame: &str) -> Option<&'a str> {
    i.stderr
        .lines()
        .find(|l| l.contains("warning: dbc:") && l.contains(&format!("message '{frame}'")))
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn an_explicit_cycle_time_reaches_the_transmitting_port() {
    let i = ingest(&format!("{HEADER}{FRAMES}"));
    // 30 ms is 33.33... frames/s, which no short decimal represents exactly.
    assert_cycle_ms(&i, "Explicit", 30.0);
    assert!(warned(&i, "Explicit").is_none(), "{}", i.stderr);
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn a_frame_without_its_own_value_inherits_the_default() {
    let i = ingest(&format!("{HEADER}{FRAMES}"));
    assert_cycle_ms(&i, "Defaulted", 100.0);
    assert!(warned(&i, "Defaulted").is_none(), "{}", i.stderr);
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn an_extended_id_frame_is_matched_to_its_attribute() {
    let i = ingest(&format!("{HEADER}{FRAMES}"));
    // Its own 20 ms, not the 100 ms default a mismatched ID would fall back to.
    assert_cycle_ms(&i, "Ext", 20.0);
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn a_zero_cycle_is_non_cyclic_reported_and_never_a_zero_rate() {
    let i = ingest(&format!("{HEADER}{FRAMES}"));
    assert_eq!(rate_on_tx_port(&i, "Sporadic"), None);
    let w = warned(&i, "Sporadic").unwrap_or_else(|| panic!("no warning: {}", i.stderr));
    assert!(w.contains("non-cyclic"), "{w}");
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn a_cycle_time_with_no_port_to_carry_it_is_reported() {
    let i = ingest(&format!("{HEADER}{FRAMES}"));
    let w = warned(&i, "Orphan").unwrap_or_else(|| panic!("no warning: {}", i.stderr));
    assert!(w.contains("50") && w.contains("not carried"), "{w}");
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn no_cycle_time_and_no_default_is_reported() {
    let frames = FRAMES
        .lines()
        .filter(|l| !l.starts_with("BA_DEF_DEF_"))
        .collect::<Vec<_>>()
        .join("\n");
    let i = ingest(&format!("{HEADER}{frames}\n"));
    assert_eq!(rate_on_tx_port(&i, "Defaulted"), None);
    let w = warned(&i, "Defaulted").unwrap_or_else(|| panic!("no warning: {}", i.stderr));
    assert!(w.contains("no GenMsgCycleTime"), "{w}");
    // The frames with their own values are unaffected by the missing default.
    assert_cycle_ms(&i, "Explicit", 30.0);
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn a_fractional_cycle_time_reaches_the_port() {
    // can-dbc reads `12.5` as a Double; it is as valid a cycle as an integer.
    let frames = FRAMES.replace(
        "BA_ \"GenMsgCycleTime\" BO_ 100 30;",
        "BA_ \"GenMsgCycleTime\" BO_ 100 12.5;",
    );
    let i = ingest(&format!("{HEADER}{frames}"));
    assert_cycle_ms(&i, "Explicit", 12.5);
    assert!(warned(&i, "Explicit").is_none(), "{}", i.stderr);
}

// rivet: verifies REQ-DBC-CYCLE-TIME-001
#[test]
fn a_negative_cycle_time_is_reported_and_never_a_rate() {
    let frames = FRAMES.replace(
        "BA_ \"GenMsgCycleTime\" BO_ 100 30;",
        "BA_ \"GenMsgCycleTime\" BO_ 100 -10;",
    );
    let i = ingest(&format!("{HEADER}{frames}"));
    assert_eq!(rate_on_tx_port(&i, "Explicit"), None);
    let w = warned(&i, "Explicit").unwrap_or_else(|| panic!("no warning: {}", i.stderr));
    assert!(
        w.contains("-10") && w.contains("not a non-negative number"),
        "{w}"
    );
}
