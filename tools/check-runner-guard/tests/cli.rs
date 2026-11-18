//! The command line: no argument checks the current directory, `--root DIR`
//! checks DIR, anything else is a usage error (2), not a pass.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_check-runner-guard"))
}

fn tree(body: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "runner-guard-cli-{}-{}",
        std::process::id(),
        body.len()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".github/workflows")).unwrap();
    std::fs::write(d.join(".github/workflows/w.yml"), body).unwrap();
    d
}

#[test]
fn cli_reads_the_tree_it_is_pointed_at() {
    let bad = tree("on: [pull_request]\njobs:\n  a:\n    runs-on: [self-hosted]\n");
    let empty = std::env::temp_dir().join(format!("runner-guard-cli-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();

    // `--root DIR` reads DIR: the unguarded tree fails, the empty one cannot be read.
    let o = bin()
        .arg("--root")
        .arg(&bad)
        .current_dir(&empty)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o = bin()
        .arg("--root")
        .arg(&empty)
        .current_dir(&bad)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));

    // No argument reads the current directory.
    assert_eq!(
        bin().current_dir(&bad).output().unwrap().status.code(),
        Some(1)
    );
    assert_eq!(
        bin().current_dir(&empty).output().unwrap().status.code(),
        Some(2)
    );

    // Anything else is a usage error. The unknown flag names the UNGUARDED tree,
    // so a parser that took any flag for `--root` would exit 1, not 2.
    let bad_s = bad.to_str().unwrap();
    for args in [&["--bogus", bad_s][..], &["--root"][..], &["x"][..]] {
        let o = bin().args(args).current_dir(&empty).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}
