//! REQ-AADL-BINDING-CONSUMERS-002 item (3), #515: bus_bandwidth reads the
//! `Actual_Connection_Binding` written ON a connection.
//!
//! The binding can sit inline on the connection (`{ ... }`) or in the
//! system's properties as `applies to <conn>`. Both are stored on the
//! connection's own property map. Up to v0.48.1, bus_bandwidth read only
//! component property maps, so a bus carrying a connection bound either way
//! counted as idle. In this model `src` sends 100 bytes every 10 ms
//! (80 000 bps) over `slow` (1 000 bps), and the published binary printed no
//! bus_bandwidth diagnostic at all.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `@@CONN@@` is replaced by the connection and, where a case needs them, the
/// system's properties.
const MODEL: &str = r#"package Bbw
public
  bus Link
  end Link;
  thread Producer
    features
      o : out data port;
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Data_Size => 100 Bytes;
  end Producer;
  thread Consumer
    features
      i : in data port;
  end Consumer;
  process Src
    features
      o : out data port;
  end Src;
  process implementation Src.impl
    subcomponents
      t : thread Producer;
    connections
      tc : port t.o -> o;
  end Src.impl;
  process Dst
    features
      i : in data port;
  end Dst;
  process implementation Dst.impl
    subcomponents
      t : thread Consumer;
    connections
      tc : port i -> t.i;
  end Dst.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      slow : bus Link { Data_Rate => 1000 bitsps; };
      fast : bus Link { Data_Rate => 1000000 bitsps; };
      src : process Src.impl;
      dst : process Dst.impl;
    connections
      @@CONN@@
  end Top.impl;
end Bbw;
"#;

fn tmp(src: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-bbw-conn-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("bbw-{}.aadl", N.fetch_add(1, Ordering::SeqCst)));
    fs::write(&path, src).unwrap();
    path
}

/// The bus_bandwidth lines `spar analyze` prints for the model with `conn`.
fn bus_lines(conn: &str) -> Vec<String> {
    let file = tmp(&MODEL.replace("@@CONN@@", conn));
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Bbw::Top.impl", file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("bus_bandwidth:"))
        .map(str::to_string)
        .collect()
}

const OVERLOAD: &str = "bus 'slow' bandwidth exceeded: 80000.0 bps demand vs 1000.0 bps capacity \
                        (8000.0% utilization, 1 bound connections)";

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_inline_connection_binding_loads_the_bus() {
    let lines =
        bus_lines("c1 : port src.o -> dst.i { Actual_Connection_Binding => (reference (slow)); };");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(OVERLOAD), "{lines:#?}");
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_applies_to_connection_binding_loads_the_bus() {
    let lines = bus_lines(
        "c1 : port src.o -> dst.i;\n    properties\n      \
         Actual_Connection_Binding => (reference (slow)) applies to c1;",
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(OVERLOAD), "{lines:#?}");
}

/// A qualified property name is the same property.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn a_qualified_connection_binding_loads_the_bus() {
    let lines = bus_lines(
        "c1 : port src.o -> dst.i \
         { Deployment_Properties::Actual_Connection_Binding => (reference (slow)); };",
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(OVERLOAD), "{lines:#?}");
}

/// The connection is bound to `fast`, so it loads `fast` only, and `slow`
/// gets no diagnostic.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn a_connection_binding_loads_only_its_own_bus() {
    let lines =
        bus_lines("c1 : port src.o -> dst.i { Actual_Connection_Binding => (reference (fast)); };");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains(
            "bus 'fast' bandwidth utilization: 80000.0 bps of 1000000.0 bps (8.0%, 1 bound connections)"
        ),
        "{lines:#?}"
    );
}

/// When a connection and its owner disagree, the connection's own binding
/// governs. The owner's binding is inherited by `src` and `dst` and still
/// applies to their internal `tc` connections, so `slow` keeps that load.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn the_connections_own_binding_overrides_its_owners() {
    let lines = bus_lines(
        "c1 : port src.o -> dst.i { Actual_Connection_Binding => (reference (fast)); };\n    \
         properties\n      Actual_Connection_Binding => (reference (slow));",
    );
    let fast: Vec<_> = lines.iter().filter(|l| l.contains("bus 'fast'")).collect();
    assert_eq!(fast.len(), 1, "{lines:#?}");
    assert!(
        fast[0].contains("80000.0 bps of 1000000.0 bps (8.0%, 1 bound connections)"),
        "{lines:#?}"
    );
    // Only src's internal `tc` (80 000 bps) is on `slow`, not `c1` as well.
    assert!(
        lines.iter().any(|l| l
            .contains("bus 'slow' bandwidth exceeded: 80000.0 bps demand vs 1000.0 bps capacity")),
        "{lines:#?}"
    );
}

/// Negative control: with no binding anywhere, no bus carries load and the
/// pass stays silent.
// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-002
#[test]
fn an_unbound_connection_loads_no_bus() {
    let lines = bus_lines("c1 : port src.o -> dst.i;");
    assert!(lines.is_empty(), "{lines:#?}");
}
