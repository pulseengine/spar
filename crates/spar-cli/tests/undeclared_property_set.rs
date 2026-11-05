//! REQ-AADL-UNDECLARED-PROPERTY-SET-001 (#445): a property association whose
//! property set no loaded file declares, and that is not predeclared, is
//! reported.
//!
//! `Nonsense_Properties::Not_A_Real_Thing => 16#DEADBEEF#;` on a memory, with
//! no such property set anywhere: up to v0.48.1 `spar analyze` said nothing,
//! so an invented or misspelt property looked like a satisfied one.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `@@PROP@@` is replaced by the memory's property association.
const MODEL: &str = r#"package Ps
public
  @@WITH@@
  memory Ram
    properties
      @@PROP@@
  end Ram;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      r : memory Ram;
  end Top.impl;
end Ps;
"#;

const MY_PROPS: &str = "property set My_Props is
  Thing : aadlinteger applies to (memory);
end My_Props;
";

fn tmp(src: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("spar-ps-undecl-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("ps-{}.aadl", N.fetch_add(1, Ordering::SeqCst)));
    fs::write(&path, src).unwrap();
    path
}

/// The property_sets lines of `spar analyze` over the model (and `extra`
/// files, loaded alongside it).
fn property_set_lines(with: &str, prop: &str, extra: &[&str]) -> Vec<String> {
    let model = tmp(&MODEL.replace("@@WITH@@", with).replace("@@PROP@@", prop));
    let mut args = vec![
        "analyze".to_string(),
        "--root".to_string(),
        "Ps::Top.impl".to_string(),
        model.to_str().unwrap().to_string(),
    ];
    for src in extra {
        args.push(tmp(src).to_str().unwrap().to_string());
    }
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
        .filter(|l| l.contains("property_sets:"))
        .map(str::to_string)
        .collect()
}

// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn an_undeclared_property_set_is_a_warning_naming_set_and_association() {
    let lines = property_set_lines(
        "",
        "Nonsense_Properties::Not_A_Real_Thing => 16#DEADBEEF#;",
        &[],
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("[WARNING]"), "{lines:#?}");
    assert!(
        lines[0].contains(
            "property set 'Nonsense_Properties' is not declared in any loaded file and is not \
             a predeclared set, so Nonsense_Properties::Not_A_Real_Thing cannot be checked"
        ),
        "{lines:#?}"
    );
}

/// The set is declared, but its file is not loaded: still reported.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn a_set_whose_file_is_not_loaded_is_a_warning() {
    let lines = property_set_lines("with My_Props;", "My_Props::Thing => 3;", &[]);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("[WARNING]") && lines[0].contains("'My_Props'"),
        "{lines:#?}"
    );
}

/// Negative control: the same association with the set's file loaded.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn a_declared_and_loaded_set_is_silent() {
    let lines = property_set_lines("with My_Props;", "My_Props::Thing => 3;", &[MY_PROPS]);
    assert!(lines.is_empty(), "{lines:#?}");
}

/// Negative control: a predeclared set, in any letter case.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn a_predeclared_set_is_silent() {
    let lines = property_set_lines("", "memory_properties::Memory_Size => 4 KByte;", &[]);
    assert!(lines.is_empty(), "{lines:#?}");
}

/// A set OSATE contributes through a plugin is an Info, not a Warning.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn an_osate_plugin_set_is_info() {
    let lines = property_set_lines("", "sei::Platform => \"x\";", &[]);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("[INFO]"), "{lines:#?}");
    assert!(
        lines[0].contains("property set 'sei' is contributed by an OSATE plugin"),
        "{lines:#?}"
    );
}

/// One report per set, listing each association once.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn associations_of_one_set_are_reported_together() {
    let lines = property_set_lines(
        "",
        "Nope::A => 1;\n      Nope::B => 2;\n      Nope::C => 3;\n      Nope::D => 4;",
        &[],
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("so Nope::A, Nope::B, Nope::C, and 1 more cannot be checked"),
        "{lines:#?}"
    );
}

/// Exactly three associations are all named, with nothing elided.
// rivet: verifies REQ-AADL-UNDECLARED-PROPERTY-SET-001
#[test]
fn three_associations_are_named_without_an_elision() {
    let lines = property_set_lines(
        "",
        "Nope::A => 1;\n      Nope::B => 2;\n      Nope::C => 3;",
        &[],
    );
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains("so Nope::A, Nope::B, Nope::C cannot be checked"),
        "{lines:#?}"
    );
}
