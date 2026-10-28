//! Every subcommand that loads an AADL model refuses one that does not parse
//! (GitHub issue #552).
//!
//! ## The defect
//!
//! spar 0.47.1 started rejecting a feature named `out` (an AS-5506B Annex A
//! reserved word): `spar parse` and `spar analyze` exit 1. `spar instance`
//! loaded the same file, exited 0, and printed an instance model with the
//! feature gone, in every format, with `--analyze` and with `--explain`. So did
//! `modes`, `emit` and `render`; `diff` printed the error as a raw byte offset
//! and compared the broken trees anyway. Each command had its own copy of the
//! load loop and only some checked `parsed.ok()`.
//!
//! ## The oracle
//!
//! * `spar parse` is the reference. Each test runs it on the SAME file and
//!   requires every line it prints to appear, as a whole line, in the loader's
//!   stderr, with the same exit code (1), followed by
//!   `Cannot <verb>: parse errors in <file>`. Nothing may reach stdout, and an
//!   `-o`/`--output` path must not be created.
//! * The commands under test are read from `spar --help`, not from a list kept
//!   here. Every command must be classified below as a model loader, as
//!   exempt, or as taking non-AADL input. A command added to the dispatch table
//!   without a classification fails `every_command_is_classified`.
//! * Positive control: the same model with `out` renamed to `vsys_out` parses.
//!   No loader prints the refusal for it, `instance` exits 0 and lists BOTH
//!   features, and the `-o` paths that stayed absent above are created. Without
//!   this, a build that refused every model would pass.
//!
//! Against origin/main (v0.47.1 + #541) the refusal test fails on instance
//! (5 variants), diff, modes, emit (mermaid, mermaid-class, constants,
//! linker; the last two did exit 1, but for "no Base_Address", not for the
//! parse error) and render, and on the moves rows (their line read
//! `error: Parse error in …`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The issue's 16-line repro: feature `out` is a reserved word.
const RESERVED: &str = "package R
public
  bus B
  end B;
  device D
  features
    a   : provides bus access B;
    out : provides bus access B;
  end D;
  system S
  end S;
  system implementation S.i
  subcomponents
    d : device D;
  end S.i;
end R;
";

/// The issue's open question: a connection to the reserved-word feature.
/// Before #552, `instance --format json` kept `c2` with destination `d.out`,
/// a feature the instance did not list, and exited 0 without a warning.
const RESERVED_WITH_CONNECTION: &str = "package R
public
  bus B
  end B;
  device D
  features
    a   : requires bus access B;
    out : requires bus access B;
  end D;
  system S
  end S;
  system implementation S.i
  subcomponents
    d : device D;
    b : bus B;
  connections
    c1 : bus access b -> d.a;
    c2 : bus access b -> d.out;
  end S.i;
end R;
";

/// A requirements file `verify` accepts.
const REQUIREMENTS: &str = "[[requirement]]
id = \"R1\"
description = \"x\"
analysis = \"scheduling\"
";

/// `out` → `vsys_out`, the rename jess applied. Parses cleanly.
fn control(src: &str) -> String {
    src.replace("    out :", "    vsys_out :")
        .replace("d.out;", "d.vsys_out;")
}

// ── Classification of every `spar --help` command ──────────────────────

/// Commands that load an AADL model and must refuse one that does not parse.
/// Each has at least one invocation in `invocations`.
const LOADERS: &[&str] = &[
    "instance", "analyze", "allocate", "diff", "modes", "emit", "render", "verify", "codegen",
    "moves", "insight",
];

/// Allow-listed: `parse` IS the reference diagnostic; `items` dumps the
/// declarations of the error-recovered tree as a debugging aid; `lsp` and `mcp`
/// are servers that take no model on the command line.
const EXEMPT: &[&str] = &["parse", "items", "lsp", "mcp"];

/// Commands whose input is not AADL (SysML v2 / KerML, rivet YAML, CAN DBC),
/// so an AADL parse error cannot reach them.
const NOT_AADL_INPUT: &[&str] = &["sysml2", "extract", "generate", "dbc"];

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// The `Commands:` block of `spar --help`.
fn help_commands() -> Vec<String> {
    let out = spar().arg("--help").output().expect("spar --help runs");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let mut names = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_block = true;
            continue;
        }
        if in_block {
            if line.trim().is_empty() {
                break;
            }
            if let Some(first) = line.split_whitespace().next() {
                names.push(first.to_string());
            }
        }
    }
    names
}

// rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
#[test]
fn every_command_is_classified() {
    let listed = help_commands();
    // Blindness guard: 19 commands today. Reading fewer means the help format
    // changed and every check below would pass by iterating nothing.
    assert!(
        listed.len() >= 19,
        "parsed only {} command(s) from `spar --help`: {listed:?}",
        listed.len()
    );
    for name in &listed {
        let classes = [LOADERS, EXEMPT, NOT_AADL_INPUT]
            .iter()
            .filter(|c| c.contains(&name.as_str()))
            .count();
        assert_eq!(
            classes, 1,
            "`spar {name}` is in {classes} classes. A new command must be added to \
             exactly one of LOADERS (and given an invocation), EXEMPT or NOT_AADL_INPUT; \
             a loader must read its files through `load_sources_or_exit` (#552)."
        );
    }
    // A stale entry is a silently-skipped check, so every classified name must
    // still be a real command.
    for name in LOADERS.iter().chain(EXEMPT).chain(NOT_AADL_INPUT) {
        assert!(
            listed.iter().any(|l| l == name),
            "`{name}` is classified here but absent from `spar --help`: {listed:?}"
        );
    }
}

// ── Fixture workspace ──────────────────────────────────────────────────

/// A per-test directory holding the model, a diff pair, a requirements file
/// and an empty trace.
struct Ws {
    dir: PathBuf,
}

impl Ws {
    fn new(tag: &str, model: &str) -> Ws {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("load552-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("old")).unwrap();
        fs::create_dir_all(dir.join("new")).unwrap();
        fs::write(dir.join("m.aadl"), model).unwrap();
        fs::write(dir.join("old/m.aadl"), model).unwrap();
        fs::write(dir.join("new/m.aadl"), model).unwrap();
        fs::write(dir.join("req.toml"), REQUIREMENTS).unwrap();
        fs::write(dir.join("t.ctf"), "").unwrap();
        Ws { dir }
    }
    fn p(&self, rel: &str) -> String {
        self.dir.join(rel).to_string_lossy().into_owned()
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// One way of running a loader on the model.
struct Inv {
    /// The command, as `--help` lists it.
    cmd: &'static str,
    /// The verb in `Cannot <verb>: parse errors in …`.
    verb: &'static str,
    args: Vec<String>,
    /// The file the diagnostic names (relative to the workspace).
    model: &'static str,
    /// An output path the command would create on success.
    out: Option<String>,
}

fn invocations(ws: &Ws) -> Vec<Inv> {
    let m = ws.p("m.aadl");
    let root = "R::S.i".to_string();
    let inv = |cmd, verb, args: &[&str], out: Option<String>| Inv {
        cmd,
        verb,
        args: args.iter().map(|s| s.to_string()).collect(),
        model: "m.aadl",
        out,
    };
    let svg = ws.p("out.svg");
    let mmd = ws.p("out.mmd");
    let gen_dir = ws.p("gen");
    let mut v = vec![
        inv(
            "instance",
            "instance",
            &["instance", "--root", &root, &m],
            None,
        ),
        inv(
            "instance",
            "instance",
            &["instance", "--root", &root, "--format", "json", &m],
            None,
        ),
        inv(
            "instance",
            "instance",
            &["instance", "--root", &root, "--analyze", &m],
            None,
        ),
        inv(
            "instance",
            "instance",
            &[
                "instance",
                "--root",
                &root,
                "--format",
                "json",
                "--analyze",
                &m,
            ],
            None,
        ),
        inv(
            "instance",
            "instance",
            &["instance", "--root", &root, "--explain", "d", &m],
            None,
        ),
        inv(
            "instance",
            "instance",
            &[
                "instance",
                "--root",
                &root,
                "--explain",
                "d",
                "--format",
                "json",
                &m,
            ],
            None,
        ),
        inv(
            "analyze",
            "analyze",
            &["analyze", "--root", &root, &m],
            None,
        ),
        inv(
            "analyze",
            "analyze",
            &["analyze", "--root", &root, "--format", "sarif", &m],
            None,
        ),
        inv(
            "allocate",
            "allocate",
            &["allocate", "--root", &root, &m],
            None,
        ),
        inv("modes", "modes", &["modes", "--root", &root, &m], None),
        inv(
            "emit",
            "emit",
            &["emit", "--root", &root, "-o", &mmd, &m],
            Some(mmd.clone()),
        ),
        inv(
            "emit",
            "emit",
            &["emit", "--format", "mermaid-class", "--root", &root, &m],
            None,
        ),
        inv(
            "emit",
            "emit",
            &["emit", "--format", "constants", "--root", &root, &m],
            None,
        ),
        inv(
            "emit",
            "emit",
            &["emit", "--format", "linker", "--root", &root, &m],
            None,
        ),
        inv(
            "render",
            "render",
            &["render", "--root", &root, "-o", &svg, &m],
            Some(svg.clone()),
        ),
        inv(
            "verify",
            "verify",
            &["verify", "--root", &root, &ws.p("req.toml"), &m],
            None,
        ),
        inv(
            "codegen",
            "codegen",
            &["codegen", "--root", &root, "--output", &gen_dir, &m],
            Some(gen_dir.clone()),
        ),
        inv(
            "moves",
            "moves verify",
            &[
                "moves",
                "verify",
                "--root",
                &root,
                "--component",
                "d",
                "--to",
                "d",
                &m,
            ],
            None,
        ),
        inv(
            "moves",
            "moves enumerate",
            &[
                "moves",
                "enumerate",
                "--root",
                &root,
                "--component",
                "d",
                &m,
            ],
            None,
        ),
        inv(
            "insight",
            "verify-trace",
            &[
                "insight",
                "verify-trace",
                "--root",
                &root,
                "--trace",
                &ws.p("t.ctf"),
                &m,
            ],
            None,
        ),
    ];
    // diff reads both sides from directories; the diagnostic names the file
    // under `--old`, which is checked first.
    v.push(Inv {
        cmd: "diff",
        verb: "diff",
        args: [
            "diff",
            "--root",
            &root,
            "--old",
            &ws.p("old"),
            "--new",
            &ws.p("new"),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        model: "old/m.aadl",
        out: None,
    });
    v
}

fn run(args: &[String]) -> (i32, String, String) {
    let out = spar().args(args).output().expect("spar runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `spar parse <file>`: the reference exit code and diagnostic lines.
fn parse_reference(file: &str) -> (i32, Vec<String>) {
    let (rc, _, err) = run(&["parse".to_string(), file.to_string()]);
    (rc, err.lines().map(str::to_string).collect())
}

fn assert_refuses(model: &str, tag: &str) {
    let ws = Ws::new(tag, model);
    let invs = invocations(&ws);
    // Every loader has at least one invocation, so none escapes the check.
    for name in LOADERS {
        assert!(
            invs.iter().any(|i| i.cmd == *name),
            "loader `{name}` has no invocation in this test"
        );
    }
    let mut failures = Vec::new();
    for inv in &invs {
        let file = ws.p(inv.model);
        let (parse_rc, parse_lines) = parse_reference(&file);
        assert_eq!(parse_rc, 1, "`spar parse` must reject the fixture");
        assert!(
            parse_lines.iter().any(|l| l.contains("reserved words")),
            "the fixture's parse error is the reserved word: {parse_lines:?}"
        );
        let (rc, out, err) = run(&inv.args);
        let err_lines: Vec<&str> = err.lines().collect();
        let footer = format!("Cannot {}: parse errors in {file}", inv.verb);
        let mut why = Vec::new();
        if rc != parse_rc {
            why.push(format!("exit {rc}, `spar parse` exits {parse_rc}"));
        }
        for want in &parse_lines {
            if !err_lines.contains(&want.as_str()) {
                why.push(format!("stderr lacks parse's line `{want}`"));
            }
        }
        if !err_lines.contains(&footer.as_str()) {
            why.push(format!("stderr lacks `{footer}`"));
        }
        if !out.is_empty() {
            why.push(format!("wrote {} bytes to stdout", out.len()));
        }
        if let Some(o) = &inv.out
            && Path::new(o).exists()
        {
            why.push(format!("created output {o}"));
        }
        if !why.is_empty() {
            failures.push(format!(
                "spar {}\n    {}\n    stderr: {:?}",
                inv.args.join(" "),
                why.join("\n    "),
                err.lines().take(3).collect::<Vec<_>>()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} invocation(s) accepted a model `spar parse` rejects:\n{}",
        failures.len(),
        invs.len(),
        failures.join("\n")
    );
}

// rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
#[test]
fn every_loader_refuses_the_reserved_word_model() {
    assert_refuses(RESERVED, "reserved");
}

// rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
#[test]
fn every_loader_refuses_the_reserved_word_model_with_a_connection() {
    // The connection to the dropped feature can no longer reach any output:
    // the model is refused before it is instantiated.
    assert_refuses(RESERVED_WITH_CONNECTION, "reserved-conn");
}

// ── Positive control ───────────────────────────────────────────────────

// rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
#[test]
fn the_renamed_model_is_accepted_by_every_loader() {
    let ws = Ws::new("control", &control(RESERVED));
    let (parse_rc, _) = parse_reference(&ws.p("m.aadl"));
    assert_eq!(parse_rc, 0, "the vsys_out control must parse");
    let invs = invocations(&ws);
    let mut failures = Vec::new();
    for inv in &invs {
        let (rc, out, err) = run(&inv.args);
        // Some rows legitimately fail on this tiny model for reasons of their
        // own (`emit constants` has no Base_Address, `moves --to d` is not a
        // processor). What none may do is refuse it as unparseable.
        if err.contains("parse errors in") || err.contains("reserved words") {
            failures.push(format!(
                "spar {}: refused a clean model: {err}",
                inv.args.join(" ")
            ));
        }
        if inv.cmd == "instance" && rc != 0 {
            failures.push(format!("spar {}: exit {rc}: {err}", inv.args.join(" ")));
        }
        if inv.cmd == "instance" && !inv.args.iter().any(|a| a == "--explain") {
            for f in ["a", "vsys_out"] {
                let listed = out.contains(&format!("feature {f} :"))
                    || out.contains(&format!("\"name\": \"{f}\""));
                if !listed {
                    failures.push(format!(
                        "spar {}: feature `{f}` not listed",
                        inv.args.join(" ")
                    ));
                }
            }
        }
        // The output paths the refusal test requires to stay absent ARE
        // created when the model parses, so that check is not vacuous.
        if let Some(o) = &inv.out
            && !Path::new(o).exists()
        {
            failures.push(format!(
                "spar {}: exit {rc}, did not create {o}: {err}",
                inv.args.join(" ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
#[test]
fn the_renamed_connection_resolves_to_a_listed_feature() {
    let ws = Ws::new("control-conn", &control(RESERVED_WITH_CONNECTION));
    let (rc, out, err) = run(&[
        "instance".into(),
        "--root".into(),
        "R::S.i".into(),
        "--format".into(),
        "json".into(),
        ws.p("m.aadl"),
    ]);
    assert_eq!(rc, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("instance json");
    let d = v["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "d")
        .expect("subcomponent d");
    let features: Vec<&str> = d["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(features, ["a", "vsys_out"]);
    let c2 = v["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "c2")
        .expect("connection c2");
    let dest = c2["destination"].as_str().unwrap();
    let feature = dest.strip_prefix("d.").expect("c2 ends on d");
    assert!(
        features.contains(&feature),
        "c2 ends on `{dest}`, which d does not list: {features:?}"
    );
}
