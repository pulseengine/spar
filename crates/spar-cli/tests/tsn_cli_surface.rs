//! REQ-TSN-CLI-SURFACE-001 — GCL synthesis and YANG export are reachable
//! without writing Rust.
//!
//! Before this, `synthesize_gcl*` had zero callers outside spar-network and
//! `spar --help` named neither a GCL nor a YANG command (#476): the evaluator
//! wrote Rust glue to get a schedule out.
//!
//! ## Oracle
//!
//! Every expected value is produced by the spar-network call the command
//! wraps, made IN THE TEST with hand-written picosecond inputs — never read
//! back from CLI output. The CLI takes nanoseconds, the test passes the library
//! picoseconds written out by hand (`400_000_000 ps = 400 µs`), so a broken
//! unit conversion in the CLI shows up as a different schedule, not as a
//! matching one.
//!
//! On origin/main both commands are absent (`Unknown command`, exit 2), so
//! every positive test here fails there.

use std::path::PathBuf;
use std::process::{Command, Output};

use spar_network::tsn::{ClassDemand, GateSchedule, synthesize_gcl_split_guarded};
use spar_network::{ArrivalCurve, ClassOfService};

fn spar(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(args)
        .output()
        .expect("spar binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Write an inline AADL fixture to a fresh temp file.
fn fixture(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar-tsn-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.aadl"));
    std::fs::write(&path, src).unwrap();
    path
}

// ── `spar gcl` ───────────────────────────────────────────────────────

/// Two classes on a 400 µs cycle at 1 Gbit/s that do NOT fit with one window
/// per class (the library reports Oversubscribed at K=1) and DO fit split
/// four ways with a 12.144 µs guard (1518 B · 8 / 1 Gbit/s) behind every
/// window. So the window cap, the guard and both deadlines all shape the
/// answer, and a CLI that dropped or mis-scaled any of them would print a
/// different schedule.
const GCL_ARGS: &[&str] = &[
    "gcl",
    "--cycle-ns",
    "400000",
    "--link-rate-bps",
    "1000000000",
    "--max-windows",
    "4",
    "--guard-ns",
    "12144",
    "--class",
    "7:1500:10000000:100000",
    "--class",
    "5:3000:20000000:150000",
];

/// The same demands as [`GCL_ARGS`], in the library's units.
fn demands(deadline7_ps: u64) -> Vec<ClassDemand> {
    vec![
        ClassDemand {
            cos: ClassOfService::new(7).unwrap(),
            arrival: ArrivalCurve::affine(1500, 10_000_000),
            deadline_ps: deadline7_ps,
        },
        ClassDemand {
            cos: ClassOfService::new(5).unwrap(),
            arrival: ArrivalCurve::affine(3000, 20_000_000),
            // 150 µs
            deadline_ps: 150_000_000,
        },
    ]
}

fn library_schedule(deadline7_ps: u64) -> GateSchedule {
    // 400 µs cycle, 1 Gbit/s, K ≤ 4, 12.144 µs guard.
    synthesize_gcl_split_guarded(
        &demands(deadline7_ps),
        400_000_000,
        1_000_000_000,
        4,
        12_144_000,
    )
    .expect("the library synthesizes this demand set")
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn gcl_prints_exactly_the_library_schedule() {
    // 100 µs deadline for class 7.
    let lib = library_schedule(100_000_000);

    // Non-vacuity: this input exercises splitting AND guards. One window per
    // class is oversubscribed, so the schedule cannot be the trivial one.
    assert!(
        synthesize_gcl_split_guarded(&demands(100_000_000), 400_000_000, 1_000_000_000, 1, 0)
            .is_err(),
        "fixture must need splitting"
    );
    let opens = lib
        .windows
        .iter()
        .filter(|w| w.allowed_cos_mask != 0)
        .count();
    assert!(
        opens > 2,
        "split schedule has more than one window per class"
    );
    assert!(
        lib.windows
            .iter()
            .any(|w| w.duration_ps == 12_144_000 && w.allowed_cos_mask == 0)
    );

    let out = spar(GCL_ARGS);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", lib.to_gcl_blob()));
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn gcl_output_tracks_its_input() {
    // A looser class-7 deadline (150 µs) must give a different schedule, and
    // the CLI must follow it — a command printing a fixed answer fails here.
    let tight = library_schedule(100_000_000);
    let loose = library_schedule(150_000_000);
    assert_ne!(tight.to_gcl_blob(), loose.to_gcl_blob());

    let mut args = GCL_ARGS.to_vec();
    let at = args
        .iter()
        .position(|a| *a == "7:1500:10000000:100000")
        .unwrap();
    args[at] = "7:1500:10000000:150000";
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", loose.to_gcl_blob()));
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn gcl_json_carries_the_library_windows() {
    let lib = library_schedule(100_000_000);
    let mut args = GCL_ARGS.to_vec();
    args.extend(["--format", "json"]);
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(v["cycle_ps"], lib.cycle_ps);
    assert_eq!(v["gcl"], lib.to_gcl_blob());
    let windows = v["windows"].as_array().expect("windows array");
    assert_eq!(windows.len(), lib.windows.len());
    for (j, w) in windows.iter().zip(&lib.windows) {
        assert_eq!(j["offset_ps"], w.offset_ps);
        assert_eq!(j["duration_ps"], w.duration_ps);
        assert_eq!(j["allowed_cos_mask"], w.allowed_cos_mask);
    }
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn gcl_reports_the_library_error_and_exits_1() {
    // K = 1, no guard: the library's own verdict is Oversubscribed.
    let err = synthesize_gcl_split_guarded(&demands(100_000_000), 400_000_000, 1_000_000_000, 1, 0)
        .unwrap_err();
    let mut args = GCL_ARGS.to_vec();
    let k = args.iter().position(|a| *a == "--max-windows").unwrap();
    args[k + 1] = "1";
    let g = args.iter().position(|a| *a == "--guard-ns").unwrap();
    args[g + 1] = "0";
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty(), "no schedule on failure");
    assert!(
        stderr(&out).contains(&err.to_string()),
        "stderr lacks {:?}: {}",
        err.to_string(),
        stderr(&out)
    );
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn gcl_argument_errors() {
    let with = |flag: &str, value: &str| {
        let mut args = GCL_ARGS.to_vec();
        let at = args.iter().position(|a| *a == flag).unwrap();
        args[at + 1] = value;
        args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    };
    let run = |args: &[String]| spar(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let cases: Vec<(Vec<String>, i32, &str)> = vec![
        (
            with("--class", "8:1500:10000000:100000"),
            1,
            "must be 0..=7",
        ),
        (
            with("--class", "256:1500:10000000:100000"),
            1,
            "must be 0..=7",
        ),
        (
            with("--class", "7:1500:10000000"),
            1,
            "expected COS:BURST_BYTES",
        ),
        (
            with("--class", "7:1500:10000000:100000:1"),
            1,
            "expected COS:BURST_BYTES",
        ),
        (
            with("--class", "7:x:10000000:100000"),
            1,
            "--class BURST_BYTES: not a",
        ),
        (
            with("--cycle-ns", "-5"),
            1,
            "--cycle-ns: not a non-negative integer",
        ),
        (
            with("--cycle-ns", "18446744073709551615"),
            1,
            "overflows picoseconds",
        ),
        (
            with("--guard-ns", "18446744073709551615"),
            1,
            "overflows picoseconds",
        ),
        (
            with("--class", "7:1500:10000000:18446744073709551615"),
            1,
            "overflows picoseconds",
        ),
    ];
    for (args, code, msg) in &cases {
        let out = run(args);
        assert_eq!(out.status.code(), Some(*code), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(msg), "{args:?}: {}", stderr(&out));
        assert!(stdout(&out).is_empty(), "{args:?}");
    }

    // Each required input, dropped in turn.
    for drop in [
        "--cycle-ns",
        "--link-rate-bps",
        "--max-windows",
        "--guard-ns",
    ] {
        let mut args = GCL_ARGS.to_vec();
        let at = args.iter().position(|a| *a == drop).unwrap();
        args.drain(at..at + 2);
        let out = spar(&args);
        assert_eq!(out.status.code(), Some(1), "without {drop}");
        assert!(stderr(&out).contains("are all required"), "without {drop}");
        assert!(stderr(&out).contains("Usage: spar gcl"), "without {drop}");
    }
    let out = spar(&GCL_ARGS[..9]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("at least one --class is required"));
    assert!(stderr(&out).contains("Usage: spar gcl"));

    // A flag at the end with no value.
    let mut args = GCL_ARGS.to_vec();
    args.push("--class");
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--class requires a value"));
    assert!(stderr(&out).contains("Usage: spar gcl"));

    // Unknown format, unexpected positional, unknown option.
    let mut args = GCL_ARGS.to_vec();
    args.extend(["--format", "yaml"]);
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Unknown --format: yaml"));
    let mut args = GCL_ARGS.to_vec();
    args.push("model.aadl");
    let out = spar(&args);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("Unexpected argument: model.aadl"));
    assert!(stderr(&out).contains("Usage: spar gcl"));
    let out = spar(&["gcl", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("Usage: spar gcl"));
}

// ── `spar yang` ──────────────────────────────────────────────────────

const GCL_BLOB: &str = "0:5000:0x80;5000:5000:0x7F";

fn model(gcl_property: &str) -> String {
    format!(
        r#"package Net
public
  bus eth
    properties
{gcl_property}
  end eth;
  bus implementation eth.impl
  end eth.impl;
  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
  end Sys.impl;
end Net;
"#
    )
}

fn model_with_gcl(blob: &str) -> String {
    model(&format!("      Spar_TSN::Gate_Control_List => \"{blob}\";"))
}

/// The library's document for [`GCL_BLOB`] on `port`. 5 µs + 5 µs windows,
/// masks 0x80 / 0x7F.
fn library_yang(port: &str) -> String {
    let sched = GateSchedule::parse(GCL_BLOB).unwrap();
    // Non-vacuity: the blob really parsed to two windows on a 10 µs cycle.
    assert_eq!(sched.windows.len(), 2);
    assert_eq!(sched.cycle_ps, 10_000_000);
    sched.to_qcw_yang(port)
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn yang_exports_the_bus_gate_control_list_from_the_model() {
    let f = fixture("yang_model", &model_with_gcl(GCL_BLOB));
    let out = spar(&[
        "yang",
        "--port",
        "swp0",
        "--root",
        "Net::Sys.impl",
        "--bus",
        "sw",
        f.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let doc = stdout(&out);
    assert_eq!(doc, library_yang("swp0"));
    assert!(doc.contains("<name>swp0</name>"));
    assert!(doc.contains("<gate-states-value>127</gate-states-value>"));
    // The document is not activatable (no base-time, no config-change), and
    // the command says so rather than handing it back as if it were.
    assert!(stderr(&out).contains("not activatable"), "{}", stderr(&out));
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn yang_exports_a_given_gcl_blob() {
    // The path that takes `spar gcl` output to a switch document.
    let out = spar(&["yang", "--port", "swp1", "--gcl", GCL_BLOB]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), library_yang("swp1"));
    assert!(stderr(&out).contains("not activatable"));
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn yang_refuses_a_model_spar_parse_rejects() {
    // `out` is an AS-5506B reserved word: `spar parse` rejects this model.
    let src = "package Bad\npublic\n  device D\n  features\n    out : provides bus access;\n  end D;\nend Bad;\n";
    let f = fixture("yang_parse_error", src);
    let p = f.to_str().unwrap();
    let parse = spar(&["parse", p]);
    assert_eq!(
        parse.status.code(),
        Some(1),
        "spar parse rejects the fixture"
    );

    let out = spar(&[
        "yang",
        "--port",
        "p",
        "--root",
        "Bad::S.impl",
        "--bus",
        "sw",
        p,
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(
        err.contains(&format!("{p}:5:5: ")),
        "line:col like spar parse: {err}"
    );
    assert!(
        err.contains(&format!("Cannot export: parse errors in {p}")),
        "{err}"
    );
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn yang_model_errors_exit_1_with_nothing_on_stdout() {
    let good = fixture("yang_err_good", &model_with_gcl(GCL_BLOB));
    let none = fixture("yang_err_none", &model(""));
    // A 1 µs hole between the two windows; the library's own parse error is
    // the expected message.
    let bad_blob = "0:5000:0x80;6000:5000:0x7F";
    let bad_msg = GateSchedule::parse(bad_blob).unwrap_err().to_string();
    let bad = fixture("yang_err_bad", &model_with_gcl(bad_blob));
    let two = fixture(
        "yang_err_two",
        &model_with_gcl(GCL_BLOB).replace(
            "      sw : bus eth.impl;",
            "      sw : bus eth.impl;\n      sub : system Inner.impl;",
        ).replace(
            "end Net;",
            "  system Inner\n  end Inner;\n  system implementation Inner.impl\n    subcomponents\n      sw : bus eth.impl;\n  end Inner.impl;\nend Net;",
        ),
    );
    let cases = [
        (&good, "nosuch", "no component named 'nosuch'"),
        (&none, "sw", "carries no Spar_TSN::Gate_Control_List"),
        (&bad, "sw", bad_msg.as_str()),
        (&two, "sw", "2 components are named 'sw'"),
    ];
    for (file, bus, msg) in cases {
        let out = spar(&[
            "yang",
            "--port",
            "p",
            "--root",
            "Net::Sys.impl",
            "--bus",
            bus,
            file.to_str().unwrap(),
        ]);
        assert_eq!(out.status.code(), Some(1), "{msg}: {}", stderr(&out));
        assert!(stdout(&out).is_empty(), "{msg}");
        assert!(stderr(&out).contains(msg), "{msg}: {}", stderr(&out));
    }
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn yang_argument_errors() {
    let f = fixture("yang_args", &model_with_gcl(GCL_BLOB));
    let p = f.to_str().unwrap();
    let either = "give either --gcl BLOB";
    let cases: &[(&[&str], i32, &str)] = &[
        (&["yang", "--gcl", GCL_BLOB], 1, "--port NAME is required"),
        (&["yang", "--port", "p"], 1, either),
        (&["yang", "--port", "p", "--gcl", GCL_BLOB, p], 1, either),
        (
            &[
                "yang",
                "--port",
                "p",
                "--root",
                "Net::Sys.impl",
                "--bus",
                "sw",
            ],
            1,
            either,
        ),
        (
            &["yang", "--port", "p", "--root", "Net::Sys.impl", p],
            1,
            either,
        ),
        (&["yang", "--port", "p", "--bus", "sw", p], 1, either),
        (
            &[
                "yang",
                "--port",
                "p",
                "--gcl",
                GCL_BLOB,
                "--root",
                "Net::Sys.impl",
                "--bus",
                "sw",
                p,
            ],
            1,
            either,
        ),
        (
            &["yang", "--port", "p", "--gcl", "0:0:0x80"],
            1,
            "--gcl: gate window 0 has zero duration",
        ),
        (&["yang", "--port"], 1, "--port requires a value"),
        (&["yang", "--bogus"], 2, "Unknown option: --bogus"),
    ];
    for (args, code, msg) in cases {
        let out = spar(args);
        assert_eq!(out.status.code(), Some(*code), "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(msg), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("Usage: spar yang")
                || *msg == "--gcl: gate window 0 has zero duration",
            "{args:?}"
        );
        assert!(stdout(&out).is_empty(), "{args:?}");
    }
}

// rivet: verifies REQ-TSN-CLI-SURFACE-001
#[test]
fn both_commands_are_in_the_help() {
    let out = spar(&["--help"]);
    let help = stderr(&out);
    for cmd in ["gcl", "yang"] {
        assert!(
            help.lines()
                .any(|l| l.split_whitespace().next() == Some(cmd)),
            "`{cmd}` missing from --help Commands"
        );
        let h = spar(&[cmd, "--help"]);
        assert_eq!(h.status.code(), Some(0));
    }
}
