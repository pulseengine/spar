//! #515, REQ-HIR-CONNECTION-INLINE-PROPS-001 — a connection's inline
//! `{ Actual_Connection_Binding => … }` is seen, exactly as the
//! `applies to <conn>` form is.
//!
//! Two processes on two processors in a root system, connected by `nav`
//! bound to on-chip `ram`. Before the fix the inline form was never lowered,
//! and wrpc-binding warned "has no Actual_Connection_Binding"; the
//! `applies to nav` form was found. Both forms must now agree.

use std::process::Command;

const INLINE: &str = r#"package P515
public
  processor Cpu
  end Cpu;
  memory Ram
  end Ram;
  thread T
    features
      o : out data port;
      i : in data port;
  end T;
  process Pr
    features
      o : out data port;
      i : in data port;
  end Pr;
  process implementation Pr.impl
    subcomponents
      t : thread T;
    connections
      co : port t.o -> o;
      ci : port i -> t.i;
  end Pr.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      m7 : processor Cpu;
      m4 : processor Cpu;
      ram : memory Ram;
      est : process Pr.impl;
      cas : process Pr.impl;
    connections
      nav : port est.o -> cas.i { Actual_Connection_Binding => (reference (ram)); };
    properties
      Actual_Processor_Binding => (reference (m4)) applies to est;
      Actual_Processor_Binding => (reference (m7)) applies to cas;
  end S.impl;
end P515;
"#;

fn wrpc_lines(name: &str, src: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spar-515-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "P515::S.impl", path.to_str().unwrap()])
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

fn applies_to_form() -> String {
    let s = INLINE
        .replace(
            "nav : port est.o -> cas.i { Actual_Connection_Binding => (reference (ram)); };",
            "nav : port est.o -> cas.i;",
        )
        .replace(
            "Actual_Processor_Binding => (reference (m7)) applies to cas;",
            "Actual_Processor_Binding => (reference (m7)) applies to cas;\n      Actual_Connection_Binding => (reference (ram)) applies to nav;",
        );
    assert_ne!(s, INLINE);
    s
}

// rivet: verifies REQ-HIR-CONNECTION-INLINE-PROPS-001
#[test]
fn the_inline_form_is_read_like_the_applies_to_form() {
    let inline = wrpc_lines("inline.aadl", INLINE);
    assert!(
        !inline
            .iter()
            .any(|l| l.contains("has no Actual_Connection_Binding")),
        "the inline binding must be seen: {inline:#?}"
    );
    assert_eq!(
        inline,
        wrpc_lines("applies.aadl", &applies_to_form()),
        "both forms agree"
    );
}

// rivet: verifies REQ-HIR-CONNECTION-INLINE-PROPS-001
#[test]
fn an_inline_binding_to_a_bus_satisfies_wrpc_binding() {
    // Positive control: bind `nav` inline to a real bus. No wrpc warning.
    let src = INLINE
        .replace(
            "  memory Ram\n  end Ram;",
            "  memory Ram\n  end Ram;\n  bus Can\n  end Can;",
        )
        .replace(
            "      ram : memory Ram;",
            "      ram : memory Ram;\n      can : bus Can;",
        )
        .replace("(reference (ram))", "(reference (can))");
    assert_ne!(src, INLINE);
    assert!(wrpc_lines("bus.aadl", &src).is_empty());
}
