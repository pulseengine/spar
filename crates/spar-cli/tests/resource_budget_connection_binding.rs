//! REQ-AADL-BINDING-CONSUMERS-002 item (3), #515: resource_budget counts a
//! connection whose `Actual_Connection_Binding` is written ON the connection.
//!
//! resource_budget compares the summed `Data_Rate` of what is bound to a bus
//! with the bus's own `Data_Rate`. It read bindings only from component
//! property maps, so a connection bound inline or by `applies to <conn>`
//! added nothing: here `c1` carries 5 000 bps over a 1 000 bps bus, and up to
//! v0.48.1 resource_budget printed nothing.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `@@CONN@@` is replaced by the connection and, where a case needs them, the
/// system's properties.
const MODEL: &str = r#"package Rb
public
  bus Link
  end Link;
  process P
    features
      o : out data port;
      i : in data port;
  end P;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      slow : bus Link { Data_Rate => 1000 bitsps; };
      fast : bus Link { Data_Rate => 1000000 bitsps; };
      a : process P;
      b : process P;
    connections
      @@CONN@@
  end Top.impl;
end Rb;
"#;

fn tmp(src: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-rb-conn-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rb-{}.aadl", N.fetch_add(1, Ordering::SeqCst)));
    fs::write(&path, src).unwrap();
    path
}

/// The resource_budget lines `spar analyze` prints for the model with `conn`.
fn budget_lines(conn: &str) -> Vec<String> {
    let file = tmp(&MODEL.replace("@@CONN@@", conn));
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Rb::Top.impl", file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("resource_budget:") && l.contains("bandwidth"))
        .map(str::to_string)
        .collect()
}

const OVER: &str = "bus 'slow' bandwidth may be exceeded: 5000.0 bps demanded vs 1000.0 bps \
                    capacity (1 connections)";

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_inline_connection_binding_counts_the_connections_rate() {
    let lines = budget_lines(
        "c1 : port a.o -> b.i \
         { Data_Rate => 5000 bitsps; Actual_Connection_Binding => (reference (slow)); };",
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(OVER), "{lines:#?}");
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_applies_to_connection_binding_counts_the_connections_rate() {
    let lines = budget_lines(
        "c1 : port a.o -> b.i { Data_Rate => 5000 bitsps; };\n    properties\n      \
         Actual_Connection_Binding => (reference (slow)) applies to c1;",
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(OVER), "{lines:#?}");
}

/// Two connections on the same bus add up: 600 + 600 > 1 000.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn connection_rates_on_one_bus_add_up() {
    let lines = budget_lines(
        "c1 : port a.o -> b.i \
         { Data_Rate => 600 bitsps; Actual_Connection_Binding => (reference (slow)); };\n      \
         c2 : port b.o -> a.i \
         { Data_Rate => 600 bitsps; Actual_Connection_Binding => (reference (slow)); };",
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("1200.0 bps demanded vs 1000.0 bps capacity (2 connections)"),
        "{lines:#?}"
    );
}

/// Bound to `fast` (1 Mbps), the same connection exceeds nothing, and `slow`
/// carries nothing.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn a_connection_on_another_bus_does_not_load_this_one() {
    let lines = budget_lines(
        "c1 : port a.o -> b.i \
         { Data_Rate => 5000 bitsps; Actual_Connection_Binding => (reference (fast)); };",
    );
    assert!(lines.is_empty(), "{lines:#?}");
}

/// Negative control: a rate with no binding loads no bus.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_unbound_connection_loads_no_bus() {
    let lines = budget_lines("c1 : port a.o -> b.i { Data_Rate => 5000 bitsps; };");
    assert!(lines.is_empty(), "{lines:#?}");
}
