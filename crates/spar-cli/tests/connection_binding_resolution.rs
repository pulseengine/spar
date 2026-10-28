//! #539, REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001 — an
//! `Actual_Connection_Binding` whose `reference (...)` resolves nowhere is
//! reported, as `Actual_Processor_Binding` is.
//!
//! Two processes on two processors, connected by `nav`. Before the fix,
//! `nav { Actual_Connection_Binding => (reference (no_such_thing)); }` drew
//! no diagnostic from any analysis: binding_rules checked only processor and
//! memory bindings, and wrpc-binding accepts an unresolvable name.
//!
//! The reference resolves from the component that declared the association
//! (REQ-AADL-REF-CONTEXT-001): the connection's owner for its own `{ … }`
//! block, the declaring implementation for `applies to nav`. A resolved
//! target's category is checked against Deployment_Properties.aadl:96-97
//! (osate2 @ 87468992): processor, virtual processor, bus, virtual bus,
//! device, memory, system, abstract.
//!
//! NON-VACUITY: the error assertions fail on origin/main b2225ea, which
//! prints no `Actual_Connection_Binding` line from binding_rules at all.
//! Every legal category is a silent case, so a build that reported every
//! connection binding, or dropped a category from the allowed list, fails.

use std::process::Command;

const MODEL: &str = r#"package CB539
public
  processor Cpu
  end Cpu;
  virtual processor VCpu
  end VCpu;
  bus Can
  end Can;
  virtual bus VCan
  end VCan;
  memory Ram
  end Ram;
  device Dev
  end Dev;
  system Box
  end Box;
  abstract Thing
  end Thing;
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
      vcpu : virtual processor VCpu;
      can : bus Can;
      vcan : virtual bus VCan;
      shared_mem : memory Ram;
      gw : device Dev;
      box : system Box;
      thing : abstract Thing;
      logger : process Pr;
      est : process Pr;
      cas : process Pr;
    connections
      nav : port est.o -> cas.i { Actual_Connection_Binding => (reference (no_such_thing)); };
    properties
      Actual_Processor_Binding => (reference (m4)) applies to est;
      Actual_Processor_Binding => (reference (m7)) applies to cas;
  end S.impl;
end CB539;
"#;

const INLINE: &str = "{ Actual_Connection_Binding => (reference (no_such_thing)); }";

/// The binding_rules lines about `Actual_Connection_Binding` that
/// `spar analyze` prints for `src`. Other binding_rules lines (a process
/// with no Actual_Memory_Binding) are not this check's.
fn acb_lines(name: &str, src: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spar-539-acb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "CB539::S.impl", path.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text.lines()
        .filter(|l| l.contains("binding_rules:") && l.contains("Actual_Connection_Binding"))
        .map(str::to_string)
        .collect()
}

fn with(from: &str, to: &str) -> String {
    let s = MODEL.replace(from, to);
    assert_ne!(s, MODEL, "the edit `{from}` must apply");
    s
}

/// The same binding written as `applies to nav` from the root instead.
fn applies_to(target: &str) -> String {
    with(INLINE, "").replace(
        "applies to cas;",
        &format!(
            "applies to cas;\n      Actual_Connection_Binding => (reference ({target})) applies to nav;"
        ),
    )
}

fn assert_unresolved(lines: &[String]) {
    assert_eq!(lines.len(), 1, "exactly one line: {lines:#?}");
    assert!(
        lines[0].contains("[ERROR]")
            && lines[0].contains("connection 'nav'")
            && lines[0].contains("'no_such_thing' which does not exist"),
        "{lines:#?}"
    );
}

fn assert_wrong_category(lines: &[String]) {
    assert_eq!(lines.len(), 1, "exactly one line: {lines:#?}");
    assert!(
        lines[0].contains("[ERROR]")
            && lines[0].contains("connection 'nav'")
            && lines[0].contains("'logger' is a process component"),
        "{lines:#?}"
    );
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn a_binding_to_a_name_that_resolves_nowhere_is_an_error() {
    assert_unresolved(&acb_lines("unresolved.aadl", MODEL));
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn the_applies_to_form_is_an_error_too() {
    assert_unresolved(&acb_lines(
        "unresolved_at.aadl",
        &applies_to("no_such_thing"),
    ));
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn a_binding_to_a_process_is_the_wrong_category() {
    let src = with("(reference (no_such_thing))", "(reference (logger))");
    assert_wrong_category(&acb_lines("process.aadl", &src));
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn the_applies_to_form_of_a_process_binding_is_the_wrong_category() {
    assert_wrong_category(&acb_lines("process_at.aadl", &applies_to("logger")));
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn every_legal_target_category_is_silent() {
    // Deployment_Properties.aadl:96-97: one existing component of each of the
    // eight categories, inline and through `applies to`.
    for target in [
        "m7",
        "vcpu",
        "can",
        "vcan",
        "shared_mem",
        "gw",
        "box",
        "thing",
    ] {
        let inline = with(
            "(reference (no_such_thing))",
            &format!("(reference ({target}))"),
        );
        assert!(
            acb_lines(&format!("{target}.aadl"), &inline).is_empty(),
            "inline binding to {target} must be silent"
        );
        assert!(
            acb_lines(&format!("{target}_at.aadl"), &applies_to(target)).is_empty(),
            "applies-to binding to {target} must be silent"
        );
    }
}

// rivet: verifies REQ-ANALYSIS-CONNECTION-BINDING-RESOLVE-001
#[test]
fn an_unbound_connection_is_silent_from_this_check() {
    // Negative control: no binding at all is wrpc-binding's concern, not a
    // reference that fails to resolve.
    assert!(acb_lines("unbound.aadl", &with(INLINE, "")).is_empty());
}
