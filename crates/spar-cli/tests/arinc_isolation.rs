//! REQ-ARINC-ISOLATION-FP-001 (#504) — end to end through `spar analyze`.
//!
//! Two ARINC 653 partitions modelled as the annex example does: each a
//! process bound to a virtual processor of `cpu` by a dotted reference, the
//! two connected by a sampling port (data port) and a queueing port (event
//! data port). That is the approved inter-partition channel and must not be
//! reported. Data at the system level reached by both partitions bypasses the
//! ports and must be an Error, which `--allow arinc-partition-isolation`
//! demotes to a Warning.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const MODEL: &str = r#"package Arinc
public
  data Int
  end Int;
  thread Tx
    features
      sout : out data port Int;
      qout : out event data port Int;
      @TX@
  end Tx;
  thread Rx
    features
      sin : in data port Int;
      qin : in event data port Int;
      @RX@
  end Rx;
  process P1
    features
      sout : out data port Int;
      qout : out event data port Int;
      @P1F@
  end P1;
  process implementation P1.impl
    subcomponents
      t : thread Tx;
    connections
      c1 : port t.sout -> sout;
      c2 : port t.qout -> qout;
      @P1C@
  end P1.impl;
  process P2
    features
      sin : in data port Int;
      qin : in event data port Int;
      @P2F@
  end P2;
  process implementation P2.impl
    subcomponents
      t : thread Rx;
    connections
      c1 : port sin -> t.sin;
      c2 : port qin -> t.qin;
      @P2C@
  end P2.impl;
  virtual processor Part
  end Part;
  processor Cpu
  end Cpu;
  processor implementation Cpu.impl
    subcomponents
      part1 : virtual processor Part;
      part2 : virtual processor Part;
  end Cpu.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu.impl;
      p1 : process P1.impl;
      p2 : process P2.impl;
      @TOPS@
    connections
      sampling : port p1.sout -> p2.sin;
      queueing : port p1.qout -> p2.qin;
      @TOPC@
    properties
      Actual_Processor_Binding => (reference (cpu.part1)) applies to p1;
      Actual_Processor_Binding => (reference (cpu.part2)) applies to p2;
  end Top.impl;
end Arinc;
"#;

fn model(shared: bool) -> String {
    let fills: &[(&str, &str)] = if shared {
        &[
            ("@TX@", "w : requires data access Int;"),
            ("@RX@", "r : requires data access Int;"),
            ("@P1F@", "acc : requires data access Int;"),
            ("@P2F@", "acc : requires data access Int;"),
            ("@P1C@", "a1 : data access acc -> t.w;"),
            ("@P2C@", "a1 : data access acc -> t.r;"),
            ("@TOPS@", "shared : data Int;"),
            (
                "@TOPC@",
                "s1 : data access shared -> p1.acc; s2 : data access shared -> p2.acc;",
            ),
        ]
    } else {
        &[]
    };
    let mut s = MODEL.to_string();
    for (k, v) in fills {
        s = s.replace(k, v);
    }
    for k in [
        "@TX@", "@RX@", "@P1F@", "@P2F@", "@P1C@", "@P2C@", "@TOPS@", "@TOPC@",
    ] {
        s = s.replace(k, "");
    }
    s
}

fn tmp(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-arinc-iso-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// The arinc653 lines of `spar analyze` on `src`, with `extra` arguments.
fn arinc_lines(name: &str, src: &str, extra: &[&str]) -> Vec<String> {
    let file = tmp(name, src);
    let mut args = vec!["analyze", "--root", "Arinc::Top.impl"];
    args.extend_from_slice(extra);
    args.push(file.to_str().unwrap());
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(&args)
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("ARINC-PARTITION-"))
        .map(str::to_string)
        .collect()
}

// rivet: verifies REQ-ARINC-ISOLATION-FP-001
#[test]
fn ports_between_partitions_are_not_reported() {
    // On main: two false ARINC-PARTITION-ASSIGNMENT warnings (the dotted
    // binding never resolved). With only the binding fixed, main's isolation
    // rule reported both port connections as Errors.
    let lines = arinc_lines("ports.aadl", &model(false), &[]);
    assert!(lines.is_empty(), "{lines:#?}");
}

// rivet: verifies REQ-ARINC-ISOLATION-FP-001
#[test]
fn data_shared_by_both_partitions_is_an_error_and_allow_demotes_it() {
    let src = model(true);
    let lines = arinc_lines("shared.aadl", &src, &[]);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("[ERROR]"), "{lines:#?}");
    assert!(lines[0].contains("ARINC-PARTITION-ISOLATION"), "{lines:#?}");
    assert!(lines[0].contains("'shared'"), "{lines:#?}");

    let lines = arinc_lines(
        "shared_allow.aadl",
        &src,
        &["--allow", "arinc-partition-isolation"],
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("[WARNING]"), "{lines:#?}");
}
