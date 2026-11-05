//! A component implementation declared outside its type's package is a parse
//! error (GitHub issue #563, item 2).
//!
//! ## The defect
//!
//! Inside `package Example_Fleet`, spar 0.48.0 accepted
//! `system implementation Warte_Fleet_Types::Fleet.example` with `spar parse`
//! printing OK and exiting 0. `spar instance --root
//! Warte_Fleet_Types::Fleet.example` then failed with "no component
//! implementation with that qualified name" and listed the implementation as
//! `Example_Fleet::Warte_Fleet_Types::Fleet.example`. The declaration is not
//! AADL: an implementation is declared in the package of its type, and the
//! grammar names that type by a plain identifier (OSATE's Aadl2.xtext:
//! `Realization: implemented=[ComponentType|ID]`). The same was true of a
//! type qualified with its OWN package (`P::T.i` inside `package P`), which
//! lowered to `P::P::T.i` and could not be instantiated either.
//!
//! ## The oracle
//!
//! * `spar parse` exits 1 and prints the diagnostic at the line and column of
//!   the qualified type name, naming the type's package and the
//!   extend-locally hint with the implementation's own category.
//! * A model loader (`instance`) refuses the same files with
//!   `Cannot instance: parse errors in <file>` (#554).
//! * Positive control: the fix the hint proposes (extend the type locally,
//!   implement it by its plain name) parses OK and `instance` exits 0. Without
//!   it, a build that refused every implementation would pass.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TYPES: &str = "package Warte_Fleet_Types
public
  system Fleet
  end Fleet;
  thread group Workers
  end Workers;
end Warte_Fleet_Types;
";

/// The issue's repro: the implementation names a type of another package.
const FOREIGN: &str = "package Example_Fleet
public
  with Warte_Fleet_Types;
  system implementation Warte_Fleet_Types::Fleet.example
  end Warte_Fleet_Types::Fleet.example;
end Example_Fleet;
";

/// A two-token category: the hint must repeat it in full.
const FOREIGN_THREAD_GROUP: &str = "package Example_Fleet
public
  with Warte_Fleet_Types;
  thread group implementation Warte_Fleet_Types::Workers.pool
  end Warte_Fleet_Types::Workers.pool;
end Example_Fleet;
";

/// Qualified with its own package: not AADL either, and not instantiable.
const OWN_QUALIFIED: &str = "package P
public
  system T
  end T;
  system implementation P::T.i
  end P::T.i;
end P;
";

/// What the hint proposes: extend the type locally, implement it plainly.
const EXTENDED_LOCALLY: &str = "package Example_Fleet
public
  with Warte_Fleet_Types;
  system Fleet extends Warte_Fleet_Types::Fleet
  end Fleet;
  system implementation Fleet.example
  end Fleet.example;
end Example_Fleet;
";

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("spar_impl_pkg_563_{}_{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create temp dir");
    d
}

fn write(dir: &Path, name: &str, src: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, src).expect("write AADL");
    p
}

fn run(args: &[&str], files: &[&Path]) -> Output {
    let mut c = spar();
    c.args(args);
    for f in files {
        c.arg(f);
    }
    c.output().expect("run spar")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn parse_rejects_implementation_of_a_foreign_type() {
    let d = tmpdir("foreign");
    let a = write(&d, "a.aadl", TYPES);
    let b = write(&d, "b.aadl", FOREIGN);
    let out = run(&["parse"], &[&a, &b]);
    let err = stderr(&out);

    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    assert!(
        err.lines().any(|l| l == format!("{}: OK", a.display())),
        "the types package itself is fine:\n{err}"
    );
    let expected = format!(
        "{}:4:25: component implementation `Warte_Fleet_Types::Fleet.example` names its \
         type with a package qualifier: an implementation is declared in the package of \
         its type, `Warte_Fleet_Types`, and names the type plainly there: \
         `system implementation Fleet.example`; to implement it in another package, \
         extend the type locally: `system Fleet extends Warte_Fleet_Types::Fleet`, then \
         `system implementation Fleet.example`",
        b.display()
    );
    assert!(
        err.lines().any(|l| l == expected),
        "expected the line\n  {expected}\ngot:\n{err}"
    );
    assert!(
        !err.lines().any(|l| l == format!("{}: OK", b.display())),
        "b.aadl must not be reported OK:\n{err}"
    );
    let _ = fs::remove_dir_all(&d);
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn hint_repeats_a_two_token_category() {
    let d = tmpdir("thread_group");
    let a = write(&d, "a.aadl", TYPES);
    let b = write(&d, "b.aadl", FOREIGN_THREAD_GROUP);
    let out = run(&["parse"], &[&a, &b]);
    let err = stderr(&out);

    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    let expected = format!(
        "{}:4:31: component implementation `Warte_Fleet_Types::Workers.pool` names its \
         type with a package qualifier: an implementation is declared in the package of \
         its type, `Warte_Fleet_Types`, and names the type plainly there: \
         `thread group implementation Workers.pool`; to implement it in another package, \
         extend the type locally: `thread group Workers extends Warte_Fleet_Types::Workers`, \
         then `thread group implementation Workers.pool`",
        b.display()
    );
    assert!(
        err.lines().any(|l| l == expected),
        "expected the line\n  {expected}\ngot:\n{err}"
    );
    let _ = fs::remove_dir_all(&d);
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn parse_rejects_a_type_qualified_with_its_own_package() {
    let d = tmpdir("own");
    let p = write(&d, "p.aadl", OWN_QUALIFIED);
    let out = run(&["parse"], &[&p]);
    let err = stderr(&out);

    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    // The text must also be right when the qualifier IS the enclosing
    // package: the first remedy (drop the qualifier) is the one that applies.
    let expected = format!(
        "{}:5:25: component implementation `P::T.i` names its type with a package \
         qualifier: an implementation is declared in the package of its type, `P`, and \
         names the type plainly there: `system implementation T.i`; to implement it in \
         another package, extend the type locally: `system T extends P::T`, then \
         `system implementation T.i`",
        p.display()
    );
    assert!(
        err.lines().any(|l| l == expected),
        "expected the line\n  {expected}\ngot:\n{err}"
    );
    let _ = fs::remove_dir_all(&d);
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn instance_refuses_the_same_files() {
    let d = tmpdir("instance");
    let a = write(&d, "a.aadl", TYPES);
    let b = write(&d, "b.aadl", FOREIGN);
    let out = run(
        &["instance", "--root", "Warte_Fleet_Types::Fleet.example"],
        &[&a, &b],
    );
    let err = stderr(&out);

    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    assert!(
        err.lines()
            .any(|l| l == format!("Cannot instance: parse errors in {}", b.display())),
        "expected the #554 refusal naming b.aadl:\n{err}"
    );
    assert!(
        !err.contains("no component implementation with that qualified name"),
        "must be refused at parse, not at instantiation:\n{err}"
    );
    assert!(out.stdout.is_empty(), "nothing on stdout");
    let _ = fs::remove_dir_all(&d);
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn control_extending_the_type_locally_parses_and_instantiates() {
    let d = tmpdir("control");
    let a = write(&d, "a.aadl", TYPES);
    let b = write(&d, "b.aadl", EXTENDED_LOCALLY);

    let out = run(&["parse"], &[&a, &b]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{err}");
    for f in [&a, &b] {
        assert!(
            err.lines().any(|l| l == format!("{}: OK", f.display())),
            "{} must parse OK:\n{err}",
            f.display()
        );
    }

    let out = run(
        &["instance", "--root", "Example_Fleet::Fleet.example"],
        &[&a, &b],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "instance of the control must succeed:\n{}",
        stderr(&out)
    );
    let _ = fs::remove_dir_all(&d);
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn an_incomplete_qualified_name_is_an_error_without_a_made_up_hint() {
    // Each of these qualifies the type but never names it with a plain
    // identifier after `::`. They must still fail to parse (`Q::.i` is
    // otherwise consumed with no error at all), and the message must not
    // build a hint out of punctuation (`system ; extends Q::;`) or an empty
    // package. `Q::;.i` is the case where a hint WOULD be complete if `;`
    // were taken as the type's name.
    let plain = "component implementation names its type with a package qualifier: \
                 an implementation names its type by a plain identifier";
    let cases = [
        ("semicolon", "  system implementation Q:: ;\n"),
        ("dot", "  system implementation Q::.i;\n  end Q::.i;\n"),
        ("punct_then_impl", "  system implementation Q::;.i;\n"),
        ("eof", "  system implementation Q::"),
        ("no_impl", "  system implementation Q::T ;\n"),
        ("no_impl_name", "  system implementation Q::T.;\n"),
    ];
    for (tag, decl) in cases {
        let d = tmpdir(tag);
        let src = format!("package P\npublic\n{decl}");
        let p = write(&d, "p.aadl", &src);
        let out = run(&["parse"], &[&p]);
        let err = stderr(&out);

        assert_eq!(out.status.code(), Some(1), "{tag}: stderr:\n{err}");
        let expected = format!("{}:3:25: {plain}", p.display());
        assert!(
            err.lines().any(|l| l == expected),
            "{tag}: expected the line\n  {expected}\ngot:\n{err}"
        );
        assert!(
            !err.contains("names the type plainly there"),
            "{tag}: no hint may be built from an incomplete name:\n{err}"
        );
        let _ = fs::remove_dir_all(&d);
    }
}

// rivet: verifies REQ-PARSE-IMPL-IN-TYPE-PACKAGE-001
#[test]
fn a_nested_package_qualifier_is_named_in_full() {
    // AADL package names have segments (`A::B`): the hint names the whole
    // package, and the type is the last segment.
    let d = tmpdir("nested");
    let src = "package P\npublic\n  system implementation A::B::T.i\n  end A::B::T.i;\nend P;\n";
    let p = write(&d, "p.aadl", src);
    let out = run(&["parse"], &[&p]);
    let err = stderr(&out);

    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    let expected = format!(
        "{}:3:25: component implementation `A::B::T.i` names its type with a package \
         qualifier: an implementation is declared in the package of its type, `A::B`, and \
         names the type plainly there: `system implementation T.i`; to implement it in \
         another package, extend the type locally: `system T extends A::B::T`, then \
         `system implementation T.i`",
        p.display()
    );
    assert!(
        err.lines().any(|l| l == expected),
        "expected the line\n  {expected}\ngot:\n{err}"
    );
    let _ = fs::remove_dir_all(&d);
}
