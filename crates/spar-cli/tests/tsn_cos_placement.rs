//! REQ-TSN-COS-PLACEMENT-001, #478 item (2): `Spar_TSN::Class_of_Service` is
//! read where its property set documents it, on the port and the connection.
//!
//! The property set says Class_of_Service and Frame_Preemption apply to port
//! and connection. Up to v0.48.2, wctt read both only from the source
//! COMPONENT, so a model that followed the documentation got a deferred TSN
//! hop and a `WcttNotComputed` Error, while the undocumented device placement
//! got a bound. A port's own `{ ... }` associations were also never lowered,
//! so the inline-port form reached no property map at all.
//!
//! The oracle is the issue's: every documented placement yields exactly the
//! wctt output of the device placement. Precedence, most specific first:
//! the connection, then the source port, then the source component.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `@@BUS@@` is the switch's TSN shaping, a gate-control list unless a case
/// sets another. `@@DEV@@` goes in the source device type's `properties` section, `@@PORT@@`
/// after its out port, `@@CONN@@` after the stream connection and `@@SYS@@` in
/// the system implementation's `properties` section.
const MODEL: &str = r#"package Net
public
  with Spar_Network, Spar_TSN, Deployment_Properties;
  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      @@BUS@@
  end eth;
  bus implementation eth.impl
  end eth.impl;
  device d
    features
      net  : requires bus access;
      in_p : in data port;
  end d;
  device implementation d.impl
  end d.impl;
  device src_d
    features
      net   : requires bus access;
      out_p : out data port @@PORT@@;
    properties
      Spar_Network::Queue_Depth => 1;
      @@DEV@@
  end src_d;
  device implementation src_d.impl
  end src_d.impl;
  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p @@CONN@@;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
      @@SYS@@
  end Sys.impl;
end Net;
"#;

#[derive(Default)]
struct At<'a> {
    bus: Option<&'a str>,
    dev: &'a str,
    port: &'a str,
    conn: &'a str,
    sys: &'a str,
}

fn tmp(src: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-cos-place-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("cos-{}.aadl", N.fetch_add(1, Ordering::SeqCst)));
    fs::write(&path, src).unwrap();
    path
}

fn model(at: &At) -> PathBuf {
    tmp(&MODEL
        .replace("@@BUS@@", at.bus.unwrap_or(GCL))
        .replace("@@DEV@@", at.dev)
        .replace("@@PORT@@", at.port)
        .replace("@@CONN@@", at.conn)
        .replace("@@SYS@@", at.sys))
}

/// The wctt lines `spar analyze` prints for the model.
fn wctt(at: At) -> Vec<String> {
    let file = model(&at);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Net::Sys.impl", file.to_str().unwrap()])
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
        .filter(|l| l.contains("wctt:"))
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

const GCL: &str = r#"Spar_TSN::Gate_Control_List => "0:5000:0x80;5000:5000:0x7F";"#;
const PREEMPTIVE: &str = "Spar_TSN::Frame_Preemption => true;";

const COS7: &str = "Spar_TSN::Class_of_Service => 7;";

/// The device placement, which wctt has always read: the reference output.
fn on_device(cos: u8) -> Vec<String> {
    let dev = format!("Spar_TSN::Class_of_Service => {cos};");
    let lines = wctt(At {
        dev: &dev,
        ..At::default()
    });
    assert!(
        lines
            .iter()
            .any(|l| l.contains("WcttTasGated") && l.contains(&format!("(CoS {cos})"))),
        "the device placement must gate the hop with CoS {cos}: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("WcttNotComputed")),
        "the device placement must compute a bound: {lines:#?}"
    );
    lines
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn class_of_service_inline_on_the_connection_gives_the_device_bound() {
    let conn = format!("{{ {COS7} }}");
    assert_eq!(
        wctt(At {
            conn: &conn,
            ..At::default()
        }),
        on_device(7)
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn class_of_service_applied_to_the_connection_gives_the_device_bound() {
    assert_eq!(
        wctt(At {
            sys: "Spar_TSN::Class_of_Service => 7 applies to data1;",
            ..At::default()
        }),
        on_device(7)
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn class_of_service_inline_on_the_source_port_gives_the_device_bound() {
    let port = format!("{{ {COS7} }}");
    assert_eq!(
        wctt(At {
            port: &port,
            ..At::default()
        }),
        on_device(7)
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn class_of_service_applied_to_the_source_port_gives_the_device_bound() {
    assert_eq!(
        wctt(At {
            sys: "Spar_TSN::Class_of_Service => 7 applies to a.out_p;",
            ..At::default()
        }),
        on_device(7)
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn the_connection_overrides_the_port_and_the_port_overrides_the_device() {
    // Connection 7 over port 5 over device 3: the connection's value governs.
    assert_eq!(
        wctt(At {
            dev: "Spar_TSN::Class_of_Service => 3;",
            port: "{ Spar_TSN::Class_of_Service => 5; }",
            conn: "{ Spar_TSN::Class_of_Service => 7; }",
            ..At::default()
        }),
        on_device(7)
    );
    // Port 5 over device 3: the port's value governs.
    assert_eq!(
        wctt(At {
            dev: "Spar_TSN::Class_of_Service => 3;",
            port: "{ Spar_TSN::Class_of_Service => 5; }",
            ..At::default()
        }),
        on_device(5)
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn frame_preemption_on_the_connection_overrides_the_devices_class_default() {
    // On a preemptive bus with no gate list, a CoS 7 stream is express by
    // default and takes the 802.1Qbu fragment-blocking bound. An explicit
    // Frame_Preemption => false must win over that default from the
    // connection exactly as it does from the device.
    let express = wctt(At {
        bus: Some(PREEMPTIVE),
        dev: COS7,
        ..At::default()
    });
    assert!(
        express.iter().any(|l| l.contains("WcttPreemptionApplied")),
        "a CoS 7 stream on a preemptive bus is express: {express:#?}"
    );
    let on_device_false = wctt(At {
        bus: Some(PREEMPTIVE),
        dev: "Spar_TSN::Class_of_Service => 7; Spar_TSN::Frame_Preemption => false;",
        ..At::default()
    });
    assert_ne!(
        on_device_false, express,
        "Frame_Preemption => false must change the output, or this case checks nothing"
    );
    assert_eq!(
        wctt(At {
            bus: Some(PREEMPTIVE),
            dev: COS7,
            conn: "{ Spar_TSN::Frame_Preemption => false; }",
            ..At::default()
        }),
        on_device_false
    );
    // And a CoS 7 on the connection alone makes the stream express.
    assert_eq!(
        wctt(At {
            bus: Some(PREEMPTIVE),
            conn: "{ Spar_TSN::Class_of_Service => 7; }",
            ..At::default()
        }),
        express
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn no_class_of_service_anywhere_still_defers_the_hop() {
    let lines = wctt(At::default());
    assert!(
        lines.iter().any(|l| l.contains("WcttNotComputed")),
        "with no Class_of_Service the TSN hop defers: {lines:#?}"
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn a_ports_inline_property_reaches_the_instance() {
    let file = model(&At {
        port: "{ Spar_TSN::Class_of_Service => 7; }",
        ..At::default()
    });
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "instance",
            "--root",
            "Net::Sys.impl",
            "--format",
            "json",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("spar binary runs");
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("instance --format json parses");
    let a = json["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "a")
        .expect("child a");
    let out_p = a["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "out_p")
        .expect("feature out_p");
    assert_eq!(
        out_p["properties"]["Spar_TSN::Class_of_Service"], "7",
        "the port's own association must be on the feature: {out_p:#}"
    );
}

// rivet: verifies REQ-TSN-COS-PLACEMENT-001
#[test]
fn an_applies_to_inside_a_ports_association_is_not_an_unresolved_path() {
    // OSATE bug-track issue458: the `applies to af1` names a feature of the
    // port's data classifier, which is not instantiated under the port.
    // Lowering the port's own associations must not queue it against the
    // component, where it would be an unresolved-path Error on a legal model.
    let file = tmp(r#"package Pa
public
  with ps;
  system s1
    features
      dp1 : in data port d1 { ps::def1 => true applies to af1; };
  end s1;
  system implementation s1.i1
  end s1.i1;
  data d1
    features
      af1 : feature;
  end d1;
end Pa;

property set ps is
  def1 : aadlboolean applies to (all);
end ps;
"#);
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Pa::s1.i1", file.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Instantiated "), "must instantiate: {text}");
    assert!(
        !text.contains("could not be resolved"),
        "the port-relative applies-to must not be resolved from the component: {text}"
    );
}
