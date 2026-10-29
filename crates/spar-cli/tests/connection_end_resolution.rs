//! REQ-INSTANCE-CONNECTION-END-RESOLVES-001 — a connection end that names a
//! feature which does not exist is an Error, not a silent instance (Refs #555).
//!
//! AS5506 Ch.9 (Connections): a connection end names a feature of
//! a subcomponent of the containing implementation (`sub.feature`), or a
//! feature of the containing component's own type (`feature`). Before the fix,
//! `c2 : bus access b -> d.zzz;` with no feature `zzz` on `d` parsed,
//! instantiated and analysed with exit 0 and no diagnostic, and
//! `instance --format json` kept a connection to `d.zzz`.
//!
//! The diagnostic goes through the same channel as an unresolved SUBCOMPONENT
//! end (`connection 'c': unresolved destination subcomponent 'q'`): an
//! instance diagnostic, which `analyze` reports as a `completeness` ERROR
//! (exit 1) and `instance` prints as `warning:`.
//!
//! NON-VACUITY: the two Error tests fail on origin/main (no diagnostic, exit
//! 0). The silent tests are the guards against false positives: a feature
//! group end (own `fg.member`, subcomponent `sub.fg`, and the collapsed
//! `sub.fg.member`), an inherited feature from an `extends` ancestor, an array
//! feature, a bare data-subcomponent end on a non-access connection, and the
//! positive control, which is the Error fixture with `zzz` spelled correctly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: &str = "P::S.impl";

/// The #555 model: `c2` names `d.zzz`, and `d : device D` has only `ba`.
/// Every connection kind is covered by one dangling connection each.
const SUB_END_MISSING: &str = r#"
package P
public
  bus B
  end B;
  data Dt
  end Dt;
  feature group FG
    features
      x : out data port;
  end FG;
  device D
    features
      ba : requires bus access B;
      inp : in data port;
      o : out data port;
      fg : feature group FG;
      da : requires data access Dt;
  end D;
  system S
  end S;
  system implementation S.impl
    subcomponents
      b : bus B;
      d : device D;
      e : device D;
      m : data Dt;
    connections
      c1 : bus access b -> d.ba;
      c2 : bus access b -> d.zzz;
      cp : port e.qqq -> d.inp;
      cg : feature group e.nofg <-> d.fg;
      cf : feature m -> d.nofeat;
      ca : data access m -> d.noaccess;
  end S.impl;
end P;
"#;

/// The containing component's own end (no dot) names a missing feature.
/// `cpar` is a parameter connection whose own end `nopar` does not exist;
/// `ccall` names a parameter `zz` that the called subprogram `Sp` lacks.
const OWN_END_MISSING: &str = r#"
package P
public
  subprogram Sp
    features
      a : in parameter;
  end Sp;
  subprogram Outer
    features
      arg : in parameter;
  end Outer;
  subprogram implementation Outer.impl
    calls
      seq : { call1 : subprogram Sp; };
    connections
      cpar : parameter nopar -> call1.a;
      ccall : parameter arg -> call1.zz;
  end Outer.impl;
  device D
    features
      inp : in data port;
  end D;
  system S
    features
      inp : in data port;
  end S;
  system implementation S.impl
    subcomponents
      d : device D;
      sp : subprogram Outer.impl;
    connections
      c3 : port nope -> d.inp;
      c4 : port inp -> d.inp;
  end S.impl;
end P;
"#;

/// Feature-group ends that resolve, an inherited (`extends`) feature, an array
/// feature, and a bare data subcomponent on a port connection. Must be silent.
const FEATURE_GROUP_RESOLVES: &str = r#"
package P
public
  data Dt
  end Dt;
  feature group FG
    features
      x : out data port;
      y : in data port;
  end FG;
  device Base
    features
      inherited_in : in data port;
  end Base;
  device Derived extends Base
    features
      fgd : feature group FG;
      arr : in data port [2];
      dp : in data port Dt;
  end Derived;
  device Src
    features
      fgs : feature group inverse of FG;
      o : out data port;
  end Src;
  system S
    features
      own : feature group FG;
  end S;
  system implementation S.impl
    subcomponents
      s : device Src;
      d : device Derived;
      m : data Dt;
    connections
      g1 : feature group s.fgs <-> d.fgd;
      g2 : port own.x -> d.inherited_in;
      g3 : port s.o -> d.fgd.y;
      g4 : port s.o -> d.arr;
      g5 : port m -> d.dp;
  end S.impl;
end P;
"#;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar binary runs");
    (
        out.status.code().unwrap_or(-1),
        strip_ansi(&String::from_utf8_lossy(&out.stdout)),
        strip_ansi(&String::from_utf8_lossy(&out.stderr)),
    )
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn write_model(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-conn-end-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, src).unwrap();
    path
}

/// `analyze` lines that are completeness ERRORs.
fn completeness_errors(all: &str) -> Vec<&str> {
    all.lines()
        .filter(|l| l.contains("[ERROR]") && l.contains("completeness:"))
        .collect()
}

fn analyze(path: &Path) -> (i32, String) {
    let (code, out, err) = run(&["analyze", "--root", ROOT, path.to_str().unwrap()]);
    (code, format!("{out}{err}"))
}

fn assert_error(all: &str, conn: &str, end: &str, holder: &str, feature: &str) {
    let want = format!("connection '{conn}': ");
    let hit = completeness_errors(all).into_iter().any(|l| {
        l.contains(&want)
            && l.contains(&format!("'{end}'"))
            && l.contains(holder)
            && l.contains(&format!("has no feature '{feature}'"))
    });
    assert!(
        hit,
        "expected a completeness ERROR naming connection '{conn}', end '{end}', \
         {holder} and missing feature '{feature}':\n{all}"
    );
}

// rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
#[test]
fn a_subcomponent_end_naming_a_missing_feature_is_an_error_for_every_kind() {
    let path = write_model("sub_missing.aadl", SUB_END_MISSING);
    let (code, all) = analyze(&path);
    assert_eq!(
        code, 1,
        "a dangling connection end must fail analyze:\n{all}"
    );
    assert_error(&all, "c2", "d.zzz", "subcomponent 'd'", "zzz");
    assert_error(&all, "cp", "e.qqq", "subcomponent 'e'", "qqq");
    assert_error(&all, "cg", "e.nofg", "subcomponent 'e'", "nofg");
    assert_error(&all, "cf", "d.nofeat", "subcomponent 'd'", "nofeat");
    assert_error(&all, "ca", "d.noaccess", "subcomponent 'd'", "noaccess");
    // The resolving connection on the same model stays silent.
    assert!(
        !all.contains("connection 'c1'"),
        "c1 names an existing feature and must be silent:\n{all}"
    );
    assert_eq!(
        completeness_errors(&all).len(),
        5,
        "exactly the five dangling ends are errors:\n{all}"
    );

    // `instance` reports it on the same channel as an unresolved subcomponent.
    let (_code, _out, err) = run(&["instance", "--root", ROOT, path.to_str().unwrap()]);
    assert!(
        err.contains("warning: connection 'c2': destination 'd.zzz': subcomponent 'd'"),
        "instance must print the dangling end:\n{err}"
    );
}

// rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
#[test]
fn an_own_end_naming_a_missing_feature_is_an_error() {
    let path = write_model("own_missing.aadl", OWN_END_MISSING);
    let (code, all) = analyze(&path);
    assert_eq!(code, 1, "{all}");
    assert_error(&all, "c3", "nope", "component 'S.impl'", "nope");
    assert_error(&all, "cpar", "nopar", "component 'sp'", "nopar");
    assert_error(&all, "ccall", "call1.zz", "called subprogram 'Sp'", "zz");
    assert_eq!(completeness_errors(&all).len(), 3, "{all}");

    // `instance --analyze --format json` carries the same Error and exits 1,
    // as it does for an unresolved subcomponent end.
    let (code, out, err) = run(&[
        "instance",
        "--analyze",
        "--format",
        "json",
        "--root",
        ROOT,
        path.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(
        out.contains("connection 'c3': source 'nope': component 'S.impl'"),
        "the JSON diagnostics must carry the dangling end:\n{out}"
    );
    assert!(
        !all.contains("connection 'c4'"),
        "c4 names the own feature `inp` and must be silent:\n{all}"
    );
}

// rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
#[test]
fn feature_group_inherited_and_array_ends_that_resolve_are_silent() {
    let path = write_model("fg_resolves.aadl", FEATURE_GROUP_RESOLVES);
    let (_code, all) = analyze(&path);
    assert!(
        !all.contains("has no feature '"),
        "a resolving end was reported as dangling:\n{all}"
    );
    assert!(
        completeness_errors(&all).is_empty(),
        "no completeness error expected:\n{all}"
    );
    assert!(all.contains("Instantiated P::S.impl"), "{all}");
}

// rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
#[test]
fn positive_control_the_same_connection_to_an_existing_feature_is_silent() {
    // The Error fixture with every dangling name spelled as a real feature.
    let fixed = SUB_END_MISSING
        .replace("d.zzz", "d.ba")
        .replace("e.qqq", "e.o")
        .replace("e.nofg", "e.fg")
        .replace("d.nofeat", "d.da")
        .replace("d.noaccess", "d.da");
    assert_ne!(fixed, SUB_END_MISSING);
    let path = write_model("control.aadl", &fixed);
    let (code, all) = analyze(&path);
    assert!(
        !all.contains("has no feature '"),
        "the control must not report a dangling end:\n{all}"
    );
    assert!(completeness_errors(&all).is_empty(), "{all}");
    assert_eq!(code, 0, "{all}");
}
