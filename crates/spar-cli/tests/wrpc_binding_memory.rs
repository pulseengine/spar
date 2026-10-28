//! #515 point 2, REQ-WRPC-BINDING-MEMORY-001 — a memory is a legal
//! `Actual_Connection_Binding` target. Deployment_Properties.aadl:96-97
//! (osate2 @ 87468992) declares the property as `list of reference
//! (processor, virtual processor, bus, virtual bus, device, memory, system,
//! abstract)`. Shared on-chip RAM is the transport between the cores of a
//! dual-core MCU.
//!
//! Two processes on two processors, connected by `nav`, which is bound to
//! the memory `shared_mem`. Before the fix, wrpc-binding warned that `nav`
//! "is bound via Actual_Connection_Binding to 'shared_mem', which is a memory
//! — not a bus or virtual bus". Now it reports the memory as the transport,
//! at Info severity.

use std::process::Command;

const MODEL: &str = r#"package WM515
public
  processor Cpu
  end Cpu;
  memory Ram
  end Ram;
  process Pr
    features
      o : out data port;
      i : in data port;
  end Pr;
  system S
  end S;
  system implementation S.impl
    subcomponents
      m7 : processor Cpu;
      m4 : processor Cpu;
      shared_mem : memory Ram;
      logger : process Pr;
      est : process Pr;
      cas : process Pr;
    connections
      nav : port est.o -> cas.i { Actual_Connection_Binding => (reference (shared_mem)); };
    properties
      Actual_Processor_Binding => (reference (m4)) applies to est;
      Actual_Processor_Binding => (reference (m7)) applies to cas;
  end S.impl;
end WM515;
"#;

const BINDING: &str = "{ Actual_Connection_Binding => (reference (shared_mem)); }";

/// The `wrpc-binding:` lines `spar analyze` prints for `src`.
fn wrpc_lines(name: &str, src: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spar-515-mem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "WM515::S.impl", path.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("wrpc-binding:"))
        .map(str::to_string)
        .collect()
}

fn with(from: &str, to: &str) -> String {
    let s = MODEL.replace(from, to);
    assert_ne!(s, MODEL, "the edit `{from}` must apply");
    s
}

// rivet: verifies REQ-WRPC-BINDING-MEMORY-001
#[test]
fn a_connection_bound_to_a_memory_reports_the_memory_as_its_transport() {
    let lines = wrpc_lines("memory.aadl", MODEL);
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("has no Actual_Connection_Binding")),
        "the memory binding must be seen: {lines:#?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("not a bus")),
        "a memory is a legal connection binding target: {lines:#?}"
    );
    assert_eq!(lines.len(), 1, "exactly one wrpc-binding line: {lines:#?}");
    assert!(
        lines[0].contains("[INFO]")
            && lines[0].contains("connection 'nav'")
            && lines[0].contains("memory 'shared_mem'"),
        "the memory is reported as the binding, at Info: {lines:#?}"
    );
}

// rivet: verifies REQ-WRPC-BINDING-MEMORY-001
#[test]
fn the_applies_to_form_reports_the_memory_too() {
    let src = with(BINDING, "").replace(
        "applies to cas;",
        "applies to cas;\n      Actual_Connection_Binding => (reference (shared_mem)) applies to nav;",
    );
    assert_eq!(
        wrpc_lines("applies.aadl", &src),
        wrpc_lines("memory.aadl", MODEL),
        "the inline and `applies to` forms agree"
    );
}

// rivet: verifies REQ-WRPC-BINDING-MEMORY-001
#[test]
fn an_unbound_crossing_still_warns() {
    // Negative control: drop the binding. The crossing has no transport.
    let lines = wrpc_lines("unbound.aadl", &with(BINDING, ""));
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("[WARNING]")
            && lines[0].contains("connection 'nav'")
            && lines[0].contains("has no Actual_Connection_Binding"),
        "{lines:#?}"
    );
}

// rivet: verifies REQ-WRPC-BINDING-MEMORY-001
#[test]
fn a_binding_to_a_process_still_warns() {
    // Negative control: a process is not among the categories
    // Actual_Connection_Binding may reference, so the binding is still wrong.
    let lines = wrpc_lines(
        "process.aadl",
        &with("(reference (shared_mem))", "(reference (logger))"),
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("[WARNING]")
            && lines[0].contains("connection 'nav'")
            && lines[0].contains("'logger', which is a process"),
        "{lines:#?}"
    );
}

// rivet: verifies REQ-WRPC-BINDING-MEMORY-001
#[test]
fn a_memory_on_a_connection_that_stays_on_one_processor_is_silent() {
    // Both ends on m4: no crossing, so wrpc-binding says nothing about it.
    let src = with(
        "(reference (m7)) applies to cas",
        "(reference (m4)) applies to cas",
    );
    assert!(wrpc_lines("same.aadl", &src).is_empty());
}
