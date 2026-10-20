//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — WCTT resolves a connection's
//! Actual_Connection_Binding from the component that declared it, by identity.
//!
//! The fixture `classical_ethernet.aadl` binds its streams with
//! `(reference (sw))`. Written as the root-anchored path
//! `(reference (Sys.impl.sw))` — the same switch — the published v0.46.0
//! binary looked the whole text up in a bare-name map, found nothing, and
//! dropped the binding: WCTT reported NOTHING for the network. Every bound
//! stream silently lost its switch hops.

use std::process::Command;

fn fixture() -> String {
    format!(
        "{}/../spar-analysis/tests/fixtures/wctt/classical_ethernet.aadl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn strip_ansi(l: &str) -> String {
    let mut s = String::new();
    let mut esc = false;
    for c in l.chars() {
        match (esc, c) {
            (false, '\x1b') => esc = true,
            (true, 'm') => esc = false,
            (true, _) => {}
            (false, c) => s.push(c),
        }
    }
    s
}

/// Sorted `wctt` diagnostic lines for the fixture with its binding replaced.
fn wctt_lines(binding: &str) -> Vec<String> {
    let src = std::fs::read_to_string(fixture()).expect("fixture");
    let model = src.replace("(reference (sw))", binding);
    assert!(
        binding == "(reference (sw))" || model != src,
        "replacement must apply"
    );
    let dir = std::env::temp_dir().join(format!("spar-bind-wctt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("m{}.aadl", binding.len()));
    std::fs::write(&path, model).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "WcttClassical::Sys.impl",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    let mut lines: Vec<String> = text
        .lines()
        .map(strip_ansi)
        .filter(|l| l.contains("wctt"))
        .collect();
    lines.sort();
    lines
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_root_anchored_switch_binding_is_the_same_switch() {
    let bare = wctt_lines("(reference (sw))");
    assert!(!bare.is_empty(), "the fixture's WCTT must report something");
    assert_eq!(wctt_lines("(reference (Sys.impl.sw))"), bare);
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn a_binding_to_a_non_bus_contributes_no_hop() {
    // Negative control: `ecu_a` resolves, but it is a device, not a switch.
    // Resolution must not invent a hop; the network reports nothing.
    assert!(wctt_lines("(reference (ecu_a))").is_empty());
}
