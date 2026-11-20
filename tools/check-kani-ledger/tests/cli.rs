//! The command line: the default reads `.github/kani-narrowing-ledger.tsv`
//! under `--root` (default `.`); a mismatch is 1, an unjudgeable tree or an
//! unknown flag is 2, never a pass.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Run with a 20 s deadline and kill the child past it, so a mutated loop
/// that never ends fails the test instead of holding the mutation job until
/// its 300 s timeout or outliving the test process.
fn run(cmd: &mut Command) -> Output {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("check-kani-ledger still running after 20 s; killed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_check-kani-ledger"))
}

fn tree(tag: &str, harness: &str, ledger: Option<&str>) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("kani-ledger-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join(".github")).unwrap();
    std::fs::write(d.join("src/h.rs"), harness).unwrap();
    if let Some(l) = ledger {
        std::fs::write(d.join(".github/kani-narrowing-ledger.tsv"), l).unwrap();
    }
    d
}

const H: &str = "#[kani::proof]\nfn h() { let x: u8 = kani::any(); kani::assume(x < 2); }\n";

#[test]
fn cli_exit_codes() {
    let ok = tree("ok", H, Some("source\tsrc/h.rs\tkani-assume\th\n"));
    let grown = tree("grown", H, Some(""));
    let stale = tree(
        "stale",
        H,
        Some("source\tsrc/h.rs\tkani-assume\th\nsource\tsrc/h.rs\tkani-assume\th\n"),
    );
    let noharness = tree("none", "fn f() {}\n", Some(""));
    let noledger = tree("noledger", H, None);

    let code = |d: &std::path::Path, extra: &[&str]| {
        run(bin().arg("--root").arg(d).args(extra)).status.code()
    };
    assert_eq!(code(&ok, &[]), Some(0));
    assert_eq!(code(&ok, &["--negative-control"]), Some(0));
    assert_eq!(code(&grown, &[]), Some(1));
    assert_eq!(code(&stale, &[]), Some(1));
    assert_eq!(code(&noharness, &[]), Some(2));
    assert_eq!(code(&noledger, &[]), Some(2));
    assert_eq!(code(&ok, &["--bogus"]), Some(2));

    // no --root reads the current directory
    let o = run(bin().current_dir(&grown));
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("UNLISTED source src/h.rs kani-assume h"));

    // --write then check is green
    assert_eq!(
        run(bin().arg("--root").arg(&grown).arg("--write"))
            .status
            .code(),
        Some(0)
    );
    assert_eq!(code(&grown, &[]), Some(0));
}

#[test]
fn version_names_the_binary() {
    let o = run(bin().arg("--version"));
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("check-kani-ledger "));
}

#[test]
fn ledger_flag_help_and_when_the_control_runs() {
    let ok = tree("flags", H, Some("source\tsrc/h.rs\tkani-assume\th\n"));
    let elsewhere = ok.join("elsewhere.tsv");
    std::fs::write(&elsewhere, "source\tsrc/h.rs\tkani-assume\th\n").unwrap();
    std::fs::write(ok.join(".github/kani-narrowing-ledger.tsv"), "").unwrap();
    // --ledger is read instead of the default path, which now disagrees
    let o = run(bin().arg("--root").arg(&ok).arg("--ledger").arg(&elsewhere));
    assert_eq!(o.status.code(), Some(0));
    let o = run(bin().arg("--root").arg(&ok));
    assert_eq!(o.status.code(), Some(1));

    let o = run(bin().arg("--help"));
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("usage: check-kani-ledger"));

    // the planted controls run only with --negative-control, and not after a 2
    let ctl = |extra: &[&str], ledger: &std::path::Path| {
        let o = run(bin()
            .arg("--root")
            .arg(&ok)
            .arg("--ledger")
            .arg(ledger)
            .args(extra));
        String::from_utf8_lossy(&o.stdout).contains("controls:")
    };
    assert!(ctl(&["--negative-control"], &elsewhere));
    assert!(!ctl(&[], &elsewhere));
    assert!(!ctl(&["--negative-control"], &ok.join("missing.tsv")));
}
