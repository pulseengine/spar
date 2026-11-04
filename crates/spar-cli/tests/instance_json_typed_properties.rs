//! REQ-INSTANCE-JSON-TYPED-PROPERTIES-001 (#563): `spar instance --format
//! json` carries every property value TYPED, in a `typed_properties` map
//! beside the raw-text `properties` map, which stays byte-for-byte what it
//! was before.
//!
//! Each typed value is a JSON object tagged by `kind`, so an enumeration
//! literal and a string are told apart. A value with any part that has no
//! faithful typed form is emitted whole as `{"kind":"opaque","text":<raw>}`.
//!
//! NON-VACUITY: every test goes through [`instance_json`], which requires
//! exit 0 and parses stdout as JSON, and every assertion compares a WHOLE map
//! with `assert_eq!`, so a missing key, an extra key, a swapped `kind`, a
//! dropped unit or a value typed when it should be opaque all fail. On
//! origin/main before this change `typed_properties` does not exist, so every
//! test here fails there.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

/// One model with every kind of value.
///
/// - `Worker.impl` writes one property of every kind, plus the values that
///   must come out opaque: an integer too large for i64 (`TP::Big`, which
///   lowering turns into an `Enum`), arithmetic (`TP::Sum`, and inside a
///   list, a range bound and a range delta), a real that overflows f64
///   (`TP::Huge`, and as a range delta), and a duplicated record field.
/// - `Top.impl` attaches a `reference` to `p.w` with `applies to`.
/// - `Proc.impl` attaches a string to the feature `w.din`, an enumeration to
///   the across connection `c1` (a semantic connection instance) and one to
///   the down connection `c0` (emitted only because it carries a property),
///   and `w2` appends to `Source_Text`, so its map holds two values.
const MODEL: &str = r#"
property set TP is
  Flag : aadlboolean applies to (all);
  Span : range of Time applies to (all);
  Weight_Units : type units (g, kg => g * 1000);
  Weight : aadlreal units TP::Weight_Units applies to (all);
  Count : aadlinteger applies to (all);
  Label : aadlstring applies to (all);
  Spec : record (Lo : aadlinteger; Name : aadlstring;) applies to (all);
  Target : reference (processor) applies to (all);
  Impl : classifier (thread) applies to (all);
  Big : aadlinteger applies to (all);
  Fine : aadlreal applies to (all);
  Stepped : range of aadlinteger applies to (all);
  Nested : list of list of aadlinteger applies to (all);
  Calc : aadlinteger applies to (all);
  Sum : aadlinteger applies to (all);
  Huge : aadlreal applies to (all);
  Mixed : list of aadlinteger applies to (all);
  Dup : record (Lo : aadlinteger;) applies to (all);
  Wide : range of aadlinteger applies to (all);
  Gap : range of aadlinteger applies to (all);
end TP;

package Typed
public
  with TP;

  thread Worker
  features
    din : in data port;
    dout : out data port;
  end Worker;

  thread implementation Worker.impl
  properties
    Period => 4 ms;
    Dispatch_Protocol => Periodic;
    Source_Text => ("a.c", "b.c");
    TP::Flag => true;
    TP::Span => 1 ms .. 2 ms;
    TP::Weight => 2.5 kg;
    TP::Count => 7;
    TP::Label => "hello";
    TP::Spec => [Lo => 3; Name => "x";];
    TP::Impl => classifier (Typed::Worker.impl);
    TP::Big => 99999999999999999999;
    TP::Fine => 1.5E3;
    TP::Stepped => 1 .. 10 delta 2;
    TP::Nested => ((1, 2), (3));
    TP::Calc => compute (my_fn);
    TP::Sum => 1 + 2;
    TP::Huge => 1.0E999;
    TP::Mixed => (1 + 2, 3);
    TP::Dup => [Lo => 1; Lo => 2;];
    TP::Wide => 1 .. 2 * 3;
    TP::Gap => 1 .. 10 delta 1.0E999;
  end Worker.impl;

  processor Cpu
  end Cpu;

  process Proc
  features
    pin : in data port;
  end Proc;

  process implementation Proc.impl
  subcomponents
    w : thread Worker.impl;
    w2 : thread Worker.impl { Source_Text +=> ("c.c"); };
  connections
    c0 : port pin -> w.din;
    c1 : port w.dout -> w2.din;
  properties
    Timing => Delayed applies to c1;
    Timing => Immediate applies to c0;
    TP::Label => "port-label" applies to w.din;
  end Proc.impl;

  system Top
  end Top;

  system implementation Top.impl
  subcomponents
    p : process Proc.impl;
    cpu : processor Cpu;
  properties
    TP::Target => reference (cpu) applies to p.w;
  end Top.impl;
end Typed;
"#;

/// Run `spar instance --format json` on [`MODEL`] and parse its stdout.
fn instance_json() -> Value {
    let dir = std::env::temp_dir().join(format!("spar-typed-props-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("typed.aadl");
    // Every test writes the same bytes, so a concurrent rewrite is harmless.
    fs::write(&path, MODEL).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["instance", "--root", "Typed::Top.impl", "--format", "json"])
        .arg(&path)
        .output()
        .expect("spar runs");
    assert!(
        out.status.success(),
        "spar instance failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is JSON")
}

/// `Top.impl/p`.
fn proc_node(root: &Value) -> &Value {
    let p = &root["children"][0];
    assert_eq!(p["name"], "p");
    p
}

/// `Top.impl/p/w`.
fn worker(root: &Value) -> &Value {
    let w = &proc_node(root)["children"][0];
    assert_eq!(w["name"], "w");
    w
}

fn opaque(text: &str) -> Value {
    json!({"kind": "opaque", "text": text})
}

fn int(v: i64) -> Value {
    json!({"kind": "integer", "value": v})
}

// rivet: verifies REQ-INSTANCE-JSON-TYPED-PROPERTIES-001
#[test]
fn every_kind_of_value_is_typed() {
    let root = instance_json();
    let expected = json!({
        "Dispatch_Protocol": {"kind": "enum", "value": "Periodic"},
        "Period": {"kind": "integer", "value": 4, "unit": "ms"},
        "Source_Text": {"kind": "list", "items": [
            {"kind": "string", "value": "a.c"},
            {"kind": "string", "value": "b.c"}
        ]},
        "TP::Big": opaque("99999999999999999999"),
        "TP::Calc": {"kind": "computed", "function": "my_fn"},
        "TP::Count": int(7),
        "TP::Dup": opaque("[Lo => 1; Lo => 2;]"),
        "TP::Fine": {"kind": "real", "value": 1500.0},
        "TP::Flag": {"kind": "boolean", "value": true},
        "TP::Gap": opaque("1 .. 10 delta 1.0E999"),
        "TP::Huge": opaque("1.0E999"),
        "TP::Impl": {"kind": "classifier", "package": "Typed", "type_name": "Worker", "impl_name": "impl"},
        "TP::Label": {"kind": "string", "value": "hello"},
        "TP::Mixed": opaque("(1 + 2, 3)"),
        "TP::Nested": {"kind": "list", "items": [
            {"kind": "list", "items": [int(1), int(2)]},
            {"kind": "list", "items": [int(3)]}
        ]},
        "TP::Span": {"kind": "range",
            "min": {"kind": "integer", "value": 1, "unit": "ms"},
            "max": {"kind": "integer", "value": 2, "unit": "ms"}},
        "TP::Spec": {"kind": "record", "fields": {
            "Lo": int(3),
            "Name": {"kind": "string", "value": "x"}
        }},
        "TP::Stepped": {"kind": "range", "min": int(1), "max": int(10), "delta": int(2)},
        "TP::Sum": opaque("1 + 2"),
        "TP::Target": {"kind": "reference", "path": "cpu"},
        "TP::Weight": {"kind": "real", "value": 2.5, "unit": "kg"},
        "TP::Wide": opaque("1 .. 2 * 3"),
    });
    assert_eq!(worker(&root)["typed_properties"], expected);
    // Numbers are JSON numbers of the right kind: an integer is not a float.
    assert!(worker(&root)["typed_properties"]["Period"]["value"].is_i64());
    assert!(worker(&root)["typed_properties"]["TP::Fine"]["value"].is_f64());
}

// rivet: verifies REQ-INSTANCE-JSON-TYPED-PROPERTIES-001
#[test]
fn raw_text_map_is_unchanged() {
    // Exactly what origin/main printed for this model before the typed map
    // was added (captured from the 0.48.1 binary).
    let root = instance_json();
    let expected = json!({
        "Dispatch_Protocol": "Periodic",
        "Period": "4 ms",
        "Source_Text": "(\"a.c\", \"b.c\")",
        "TP::Big": "99999999999999999999",
        "TP::Calc": "compute (my_fn)",
        "TP::Count": "7",
        "TP::Dup": "[Lo => 1; Lo => 2;]",
        "TP::Fine": "1.5E3",
        "TP::Flag": "true",
        "TP::Gap": "1 .. 10 delta 1.0E999",
        "TP::Huge": "1.0E999",
        "TP::Impl": "classifier (Typed::Worker.impl)",
        "TP::Label": "\"hello\"",
        "TP::Mixed": "(1 + 2, 3)",
        "TP::Nested": "((1, 2), (3))",
        "TP::Span": "1 ms .. 2 ms",
        "TP::Spec": "[Lo => 3; Name => \"x\";]",
        "TP::Stepped": "1 .. 10 delta 2",
        "TP::Sum": "1 + 2",
        "TP::Target": "reference (cpu)",
        "TP::Weight": "2.5 kg",
        "TP::Wide": "1 .. 2 * 3",
    });
    assert_eq!(worker(&root)["properties"], expected);
    // The typed map has exactly the raw map's keys.
    let raw: Vec<&String> = expected.as_object().unwrap().keys().collect();
    let typed: Vec<&String> = worker(&root)["typed_properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(typed, raw);
}

// rivet: verifies REQ-INSTANCE-JSON-TYPED-PROPERTIES-001
#[test]
fn features_and_connections_carry_typed_values() {
    let root = instance_json();
    let din = &worker(&root)["features"][0];
    assert_eq!(din["name"], "din");
    assert_eq!(din["properties"], json!({"TP::Label": "\"port-label\""}));
    assert_eq!(
        din["typed_properties"],
        json!({"TP::Label": {"kind": "string", "value": "port-label"}})
    );
    // A feature without properties has neither map.
    let dout = &worker(&root)["features"][1];
    assert_eq!(dout["name"], "dout");
    assert!(dout.get("properties").is_none());
    assert!(dout.get("typed_properties").is_none());

    let conns = proc_node(&root)["connections"].as_array().unwrap();
    let by_name = |n: &str| {
        conns
            .iter()
            .find(|c| c["name"] == n)
            .unwrap_or_else(|| panic!("connection {n} missing: {conns:?}"))
    };
    // c1: an across connection, emitted as a semantic connection instance.
    let c1 = by_name("c1");
    assert_eq!(c1["properties"], json!({"Timing": "Delayed"}));
    assert_eq!(
        c1["typed_properties"],
        json!({"Timing": {"kind": "enum", "value": "Delayed"}})
    );
    // c0: a down connection, emitted only for its property.
    let c0 = by_name("c0");
    assert_eq!(c0["properties"], json!({"Timing": "Immediate"}));
    assert_eq!(
        c0["typed_properties"],
        json!({"Timing": {"kind": "enum", "value": "Immediate"}})
    );
}

// rivet: verifies REQ-INSTANCE-JSON-TYPED-PROPERTIES-001
#[test]
fn an_appended_value_types_the_same_value_the_raw_map_shows() {
    // `w2` appends `("c.c")` to the inherited `("a.c", "b.c")`, so the
    // property map holds two values. The raw map shows the first; the typed
    // map must type that same value, not another one.
    let root = instance_json();
    let w2 = &proc_node(&root)["children"][1];
    assert_eq!(w2["name"], "w2");
    assert_eq!(w2["properties"]["Source_Text"], "(\"a.c\", \"b.c\")");
    assert_eq!(
        w2["typed_properties"]["Source_Text"],
        json!({"kind": "list", "items": [
            {"kind": "string", "value": "a.c"},
            {"kind": "string", "value": "b.c"}
        ]})
    );
}

// rivet: verifies REQ-INSTANCE-JSON-TYPED-PROPERTIES-001
#[test]
fn a_component_without_properties_has_neither_map() {
    let root = instance_json();
    let cpu = &root["children"][1];
    assert_eq!(cpu["name"], "cpu");
    assert!(cpu.get("properties").is_none());
    assert!(cpu.get("typed_properties").is_none());
}
