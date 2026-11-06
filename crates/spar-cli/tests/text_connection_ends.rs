//! The text output of `spar instance` and `spar items` names both ends of
//! every connection, as the JSON output and the AADL source do.
//!
//! Before the fix both printers wrote `connection c1 : Port ->` and stopped:
//! the arrow was printed, the ends it connects were not, so a reader of the
//! text output could not tell what any connection connects.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const MODEL: &str = r#"
package P
public
  bus Bu
  end Bu;
  system A
    features
      o : out data port;
      ba : requires bus access Bu;
  end A;
  system B
    features
      i : in data port;
  end B;
  system Top
    features
      ext : out data port;
  end Top;
  system implementation Top.impl
    subcomponents
      a : system A;
      b : system B;
      bu : bus Bu;
    connections
      c1 : port a.o -> b.i;
      c2 : port a.o -> ext;
      c3 : bus access bu <-> a.ba;
  end Top.impl;
end P;
"#;

/// One file per test: the tests run in parallel and must not read a file
/// another test is rewriting.
fn model(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-text-conn-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{name}.aadl"));
    fs::write(&p, MODEL).unwrap();
    p
}

fn stdout(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn connection_lines(text: &str) -> Vec<String> {
    // `items` prefixes a member with its visibility (`public  connection ...`).
    text.lines()
        .filter_map(|l| l.find("connection ").map(|i| l[i..].trim_end().to_string()))
        .collect()
}

#[test]
fn instance_text_names_both_ends_of_every_connection() {
    let m = model("instance");
    let text = stdout(&["instance", "--root", "P::Top.impl", m.to_str().unwrap()]);
    assert_eq!(
        connection_lines(&text),
        [
            "connection c1 : Port a.o -> b.i",
            "connection c2 : Port a.o -> ext",
            "connection c3 : Access bu <-> a.ba",
        ],
        "{text}"
    );
}

#[test]
fn items_text_names_both_ends_of_every_connection() {
    let m = model("items");
    let text = stdout(&["items", m.to_str().unwrap()]);
    assert_eq!(
        connection_lines(&text),
        [
            "connection c1 : Port a.o -> b.i",
            "connection c2 : Port a.o -> ext",
            "connection c3 : Access bu <-> a.ba",
        ],
        "{text}"
    );
}
