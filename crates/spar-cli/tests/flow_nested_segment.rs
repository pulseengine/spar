//! Integration test for spar#455 item 3: REQ-ANALYSIS-FLOW-SEGMENT-LEGALITY-001
//! (verified), plus the oracle for REQ-ANALYSIS-FLOW-THREAD-DESCENT-001.
//!
//! A system-level end-to-end flow written as
//! `app.caller.lock_flow -> app.c_hand -> app.consumer.consumer_flow`, where
//! `caller` / `consumer` are threads inside process `app`, used to resolve
//! every segment to its FIRST dotted part — the process — so the threads'
//! timing was never read and the run warned `components without timing
//! properties: app, app`.
//!
//! That spelling is not AADL: a flow element is `[subcomponent '.'] flow`,
//! one level, and a connection element is a bare identifier (osate2@87468992,
//! `Aadl2.xtext` rules `ETESubcomponentFlow` / `ETEConnectionFlow`). So the
//! fix REJECTS it rather than resolving it, and the oracle is a PAIR:
//!
//!   * NESTED — must fail (exit != 0) with a flow_check Error naming the
//!     segment, and latency must no longer report on components the flow never
//!     named (`app, app` gone, no `not computed`).
//!   * LEGAL — the same threads, the flow written inside `App.impl` where AADL
//!     puts it, must derive a latency FIGURE from the threads' execution times
//!     (worst case 10 + 12 = 22 ms) and exit 0. This is the spelling the
//!     rejection points users at; a fix whose recommended form did not work
//!     would fail here.
//!
//! A build rejecting every flow fails LEGAL; a build that stopped warning
//! without rejecting fails NESTED's Error assertion.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const NESTED_MODEL: &str = r#"
package Descent
public
  data Msg end Msg;

  thread Caller
    features
      req : out event data port Msg;
    flows
      lock_flow : flow source req;
    properties
      Timing_Properties::Compute_Execution_Time => 5 ms .. 10 ms;
  end Caller;

  thread Consumer
    features
      inp : in event data port Msg;
    flows
      consumer_flow : flow sink inp;
    properties
      Timing_Properties::Compute_Execution_Time => 7 ms .. 12 ms;
  end Consumer;

  process App
  end App;

  process implementation App.impl
    subcomponents
      caller : thread Caller;
      consumer : thread Consumer;
    connections
      c_hand : port caller.req -> consumer.inp;
  end App.impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      app : process App.impl;
    flows
      req_to_done : end to end flow app.caller.lock_flow -> app.c_hand -> app.consumer.consumer_flow;
  end Top.impl;
end Descent;
"#;

const LEGAL_MODEL: &str = r#"
package Descent
public
  data Msg end Msg;

  thread Caller
    features
      req : out event data port Msg;
    flows
      lock_flow : flow source req;
    properties
      Timing_Properties::Compute_Execution_Time => 5 ms .. 10 ms;
  end Caller;

  thread Consumer
    features
      inp : in event data port Msg;
    flows
      consumer_flow : flow sink inp;
    properties
      Timing_Properties::Compute_Execution_Time => 7 ms .. 12 ms;
  end Consumer;

  process App
  end App;

  process implementation App.impl
    subcomponents
      caller : thread Caller;
      consumer : thread Consumer;
    connections
      c_hand : port caller.req -> consumer.inp;
    flows
      req_to_done : end to end flow caller.lock_flow -> c_hand -> consumer.consumer_flow;
  end App.impl;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      app : process App.impl;
  end Top.impl;
end Descent;
"#;

/// The LEGAL route to thread timing from system level: a flow path on the
/// process, implemented in `App.impl` through both threads, used as
/// `s.src -> c1 -> app.proc_flow -> c2 -> k.snk`. Every segment is AADL.
/// Latency descends into `proc_flow`'s implementation
/// (REQ-ANALYSIS-FLOW-THREAD-DESCENT-001).
const PATH_DESCENT_MODEL: &str = r#"
package Descent
public
  data Msg end Msg;

  thread Caller
    features
      i : in event data port Msg;
      req : out event data port Msg;
    flows
      lock_flow : flow path i -> req;
    properties
      Timing_Properties::Compute_Execution_Time => 5 ms .. 10 ms;
  end Caller;

  thread Consumer
    features
      inp : in event data port Msg;
      o : out event data port Msg;
    flows
      consumer_flow : flow path inp -> o;
    properties
      Timing_Properties::Compute_Execution_Time => 7 ms .. 12 ms;
  end Consumer;

  process App
    features
      pin : in event data port Msg;
      pout : out event data port Msg;
    flows
      proc_flow : flow path pin -> pout;
  end App;

  process implementation App.impl
    subcomponents
      caller : thread Caller;
      consumer : thread Consumer;
    connections
      c_in : port pin -> caller.i;
      c_hand : port caller.req -> consumer.inp;
      c_out : port consumer.o -> pout;
    flows
      proc_flow : flow path pin -> c_in -> caller.lock_flow -> c_hand -> consumer.consumer_flow -> c_out -> pout;
  end App.impl;

  device Src
    features
      o : out event data port Msg;
    flows
      src : flow source o;
  end Src;

  device Snk
    features
      i : in event data port Msg;
    flows
      snk : flow sink i;
  end Snk;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      s : device Src;
      app : process App.impl;
      k : device Snk;
    connections
      c1 : port s.o -> app.pin;
      c2 : port app.pout -> k.i;
    flows
      req_to_done : end to end flow s.src -> c1 -> app.proc_flow -> c2 -> k.snk;
  end Top.impl;
end Descent;
"#;

fn write_model(tag: &str, src: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_flow_nested_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, src).expect("write temp AADL");
    path
}

/// `(exit_code, stdout + stderr)` from `spar analyze --root Descent::Top.impl`.
fn analyze(path: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Descent::Top.impl"])
        .arg(path)
        .output()
        .expect("spar binary runs");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn nested_flow_segment_is_rejected_not_resolved_to_the_process() {
    let path = write_model("nested", NESTED_MODEL);
    let (code, out) = analyze(&path);
    let _ = fs::remove_file(&path);

    assert_ne!(code, 0, "a rejected flow must fail the run:\n{out}");
    assert!(
        out.contains("flow segment 'app.caller.lock_flow' at position 0 reaches inside"),
        "flow_check must reject the nested segment by name:\n{out}"
    );
    assert!(
        !out.contains("app, app"),
        "latency must not resolve the threads to the process:\n{out}"
    );
    assert!(
        !out.contains("not computed"),
        "latency must not report on a rejected flow:\n{out}"
    );
}

#[test]
fn the_legal_spelling_derives_latency_from_the_threads() {
    let path = write_model("legal", LEGAL_MODEL);
    let (code, out) = analyze(&path);
    let _ = fs::remove_file(&path);

    assert_eq!(code, 0, "the legal spelling must analyse clean:\n{out}");
    let line = out
        .lines()
        .find(|l| l.contains("end-to-end flow 'req_to_done' latency:"))
        .unwrap_or_else(|| panic!("no latency figure for req_to_done:\n{out}"));
    assert!(
        line.contains("22.000 ms]"),
        "worst case must be the threads' 10 + 12 ms:\n{line}"
    );
    assert!(!out.contains("flow_check"), "no flow_check finding:\n{out}");
}

/// REQ-ANALYSIS-FLOW-THREAD-DESCENT-001: the legal flow-specification route
/// descends into the flow IMPLEMENTATION on `app` (`proc_flow`, implemented in
/// `App.impl` through both threads), so the threads' execution times reach the
/// latency: BCET 5 + 7 = 12 ms, WCET 10 + 12 = 22 ms, the figure the
/// in-implementation spelling gives. The devices `s` / `k` carry no timing and
/// may be reported; `app` must not be, because its threads supplied it.
#[test]
fn the_flow_specification_route_descends_into_the_process() {
    let path = write_model("path_descent", PATH_DESCENT_MODEL);
    let (code, out) = analyze(&path);
    let _ = fs::remove_file(&path);

    assert!(
        !out.contains("flow_check"),
        "every segment is legal AADL, so nothing may reject it:\n{out}"
    );
    assert!(
        !out.contains("not computed"),
        "descent derives a figure, so nothing is 'not computed':\n{out}"
    );
    let line = out
        .lines()
        .find(|l| l.contains("end-to-end flow 'req_to_done' latency:"))
        .unwrap_or_else(|| panic!("no latency figure for req_to_done:\n{out}"));
    assert!(
        line.contains("latency: [12.000 ms .. 22.000 ms]"),
        "BCET 5 + 7, WCET 10 + 12 from the threads inside the process:\n{line}"
    );
    if let Some(missing) = out
        .lines()
        .find(|l| l.contains("components without timing properties"))
    {
        let list = missing.split("properties:").nth(1).unwrap_or("");
        assert!(
            !list.contains("app"),
            "app's threads supplied its timing, it is not missing:\n{missing}"
        );
    }
    assert_eq!(code, 0, "a derived latency analyses clean:\n{out}");
}
