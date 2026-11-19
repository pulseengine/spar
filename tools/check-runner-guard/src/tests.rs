use super::*;

const GUARD: &str = "${{ (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && fromJSON('[\"self-hosted\",\"linux\",\"x64\",\"light\"]') || 'ubuntu-24.04' }}";

fn wf(on: &str, runs_on: &str) -> String {
    format!("name: t\n{on}\njobs:\n  a:\n    runs-on: {runs_on}\n    steps:\n      - run: true\n")
}

fn findings(name: &str, text: &str) -> Vec<String> {
    check_workflow(name, text).3
}

#[test]
fn trigger_forms() {
    assert_eq!(triggers("on: push\n").unwrap(), ["push"]);
    assert_eq!(
        triggers("on: [push, pull_request]\n").unwrap(),
        ["push", "pull_request"]
    );
    assert_eq!(
        triggers("\"on\":\n  pull_request:\n    paths: [x]\n  push:\n").unwrap(),
        ["pull_request", "push"]
    );
    assert_eq!(
        triggers("on:\n  # c\n  schedule:\n    - cron: x\n  workflow_dispatch:\njobs:\n").unwrap(),
        ["schedule", "workflow_dispatch"]
    );
    assert_eq!(
        triggers("on:\n  - push\n  - pull_request_target\n").unwrap(),
        ["push", "pull_request_target"]
    );
    assert!(
        triggers("name: x\njobs: {}\n").is_err(),
        "no on: is an error, not 'no PR trigger'"
    );
    assert!(triggers("on:\njobs:\n").is_err());
}

#[test]
fn unguarded_self_hosted_on_a_pr_trigger_is_a_finding() {
    let f = findings(
        "w.yml",
        &wf("on: [pull_request]", "[self-hosted, linux, x64, light]"),
    );
    assert_eq!(f.len(), 1);
    assert!(f[0].starts_with("UNGUARDED w.yml:5"), "{f:?}");
    let f = findings(
        "w.yml",
        &wf("on:\n  pull_request_target:", "[self-hosted, linux]"),
    );
    assert_eq!(f.len(), 1, "pull_request_target counts too: {f:?}");
}

#[test]
fn the_guard_passes_and_its_near_misses_do_not() {
    assert!(findings("w.yml", &wf("on: [pull_request]", GUARD)).is_empty());
    // The fork-flag form this repo used before: fail-open when `fork` is null.
    let old = "${{ github.event.pull_request.head.repo.fork && 'ubuntu-24.04' || fromJSON('[\"self-hosted\",\"linux\",\"x64\",\"light\"]') }}";
    assert!(!findings("w.yml", &wf("on: [pull_request]", old)).is_empty());
    // Guard inverted (`==` for `!=`).
    let inv = GUARD.replace(
        "event_name != 'pull_request'",
        "event_name == 'pull_request'",
    );
    assert!(!findings("w.yml", &wf("on: [pull_request]", &inv)).is_empty());
    // Fallback is self-hosted too.
    let bad_tail = GUARD.replace("'ubuntu-24.04'", "'self-hosted'");
    assert!(!findings("w.yml", &wf("on: [pull_request]", &bad_tail)).is_empty());
}

#[test]
fn hosted_and_matrix_runners_pass_but_matrix_self_hosted_does_not() {
    assert!(findings("w.yml", &wf("on: [pull_request]", "ubuntu-24.04")).is_empty());
    assert!(findings("w.yml", &wf("on: [pull_request]", "${{ matrix.os }}")).is_empty());
    let m = "name: t\non: [pull_request]\njobs:\n  a:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, self-hosted]\n    runs-on: ${{ matrix.os }}\n";
    let f = findings("w.yml", m);
    assert!(
        f.len() == 1 && f[0].starts_with("OUTSIDE   w.yml:7"),
        "{f:?}"
    );
    let v = findings("w.yml", &wf("on: [pull_request]", "${{ vars.RUNNER }}"));
    assert!(v.len() == 1 && v[0].starts_with("UNRESOLVED"), "{v:?}");
}

#[test]
fn block_list_runs_on_is_read_whole() {
    let t = "on: [pull_request]\njobs:\n  a:\n    runs-on:\n      - self-hosted\n      - linux\n    steps: []\n";
    let f = findings("w.yml", t);
    assert!(
        f.len() == 1 && f[0].starts_with("UNGUARDED w.yml:4"),
        "{f:?}"
    );
    let t = "on: [pull_request]\njobs:\n  a:\n    runs-on: ${{ (a)\n      && b }}\n";
    assert!(findings("w.yml", t)[0].starts_with("UNREADABLE"));
}

#[test]
fn comments_are_not_runners() {
    let t = "on: [pull_request]\njobs:\n  a:\n    # was runs-on: [self-hosted]\n    runs-on: ubuntu-24.04\n";
    assert!(findings("w.yml", t).is_empty());
}

#[test]
fn non_pr_workflows_are_not_held_to_the_guard() {
    assert!(
        findings(
            "w.yml",
            &wf("on:\n  schedule:\n    - cron: x", "[self-hosted, linux]")
        )
        .is_empty()
    );
    assert!(findings("w.yml", &wf("on: push", "[self-hosted]")).is_empty());
}

#[test]
fn unreadable_on_fails() {
    let f = findings("w.yml", "jobs:\n  a:\n    runs-on: [self-hosted]\n");
    assert!(f.len() == 1 && f[0].starts_with("UNREADABLE"), "{f:?}");
}

fn tree(files: &[(&str, &str)]) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "runner-guard-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".github/workflows")).unwrap();
    for (n, t) in files {
        std::fs::write(d.join(".github/workflows").join(n), t).unwrap();
    }
    d
}

fn exempt_tree(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = EXEMPT
        .iter()
        .map(|(n, _)| {
            (
                n.to_string(),
                wf("on:\n  schedule:\n    - cron: x", "[self-hosted, linux]"),
            )
        })
        .collect();
    v.extend(extra.iter().map(|(a, b)| (a.to_string(), b.to_string())));
    v
}

fn run_tree(files: &[(String, String)]) -> (i32, String) {
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let d = tree(&refs);
    let mut out = vec![];
    let rc = run(&d, &mut out);
    (rc, String::from_utf8(out).unwrap())
}

#[test]
fn run_passes_a_clean_tree_and_fails_each_defect() {
    let ok = exempt_tree(&[("ci.yml", &wf("on: [pull_request]", GUARD))]);
    let (rc, out) = run_tree(&ok);
    assert_eq!(rc, 0, "{out}");
    assert!(
        out.contains("PR-reachable self-hosted runs-on: 1, guarded: 1"),
        "{out}"
    );

    let (rc, out) = run_tree(&exempt_tree(&[(
        "ci.yml",
        &wf("on: [pull_request]", "[self-hosted]"),
    )]));
    assert_eq!(rc, 1, "{out}");

    // A non-PR self-hosted workflow missing from EXEMPT.
    let (rc, out) = run_tree(&exempt_tree(&[(
        "new.yml",
        &wf("on: push", "[self-hosted]"),
    )]));
    assert!(rc == 1 && out.contains("missing from EXEMPT"), "{out}");

    // An exempt workflow that gained a pull_request trigger.
    let mut t = exempt_tree(&[]);
    t[0].1 = wf("on: [pull_request, schedule]", GUARD);
    let (rc, out) = run_tree(&t);
    assert!(
        rc == 1 && out.contains("a pull request can trigger it"),
        "{out}"
    );

    // An exempt entry whose file is gone.
    let mut t = exempt_tree(&[]);
    t.remove(0);
    let (rc, out) = run_tree(&t);
    assert!(rc == 1 && out.contains("does not exist"), "{out}");

    // An exempt entry that no longer names a self-hosted runner.
    let mut t = exempt_tree(&[]);
    t[0].1 = wf("on: push", "ubuntu-24.04");
    let (rc, out) = run_tree(&t);
    assert!(
        rc == 1 && out.contains("names no self-hosted runner"),
        "{out}"
    );
}

#[test]
fn an_empty_or_missing_tree_is_an_error_not_a_pass() {
    let d = tree(&[]);
    assert_eq!(run(&d, &mut Vec::new()), 2);
    assert_eq!(run(&d.join("nope"), &mut Vec::new()), 2);
}

/// The repository's own workflows, so `cargo test` fails on an unguarded site
/// even where the CI step is not run.
#[test]
fn this_repository_is_guarded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = vec![];
    let rc = run(&root, &mut out);
    assert_eq!(rc, 0, "{}", String::from_utf8_lossy(&out));
}

/// The block-list reader: the value is the list, the sibling key after it is
/// not, a blank line inside the list does not end it, and a list that runs to
/// the end of the file is read without running past it.
#[test]
fn block_list_values_are_exact() {
    let t = "jobs:\n  a:\n    runs-on:\n      - self-hosted\n\n      - linux\n    steps: []\n";
    let (s, covered) = sites(t);
    assert_eq!(
        s,
        vec![Site {
            line: 3,
            value: "- self-hosted - linux".into()
        }]
    );
    assert_eq!(covered, vec![3, 4, 5, 6]);
    let t = "jobs:\n  a:\n    runs-on:\n      - self-hosted";
    let (s, covered) = sites(t);
    assert_eq!(s[0].value, "- self-hosted");
    assert_eq!(covered, vec![3, 4]);
    let t = "    runs-on: ubuntu-24.04\n    steps: []\n";
    assert_eq!(
        sites(t),
        (
            vec![Site {
                line: 1,
                value: "ubuntu-24.04".into()
            }],
            vec![1]
        )
    );
}

/// Only a bare `${{ matrix.<key> }}` passes as a matrix reference; anything
/// with an operator, an empty key or other text is an unresolved expression.
#[test]
fn matrix_reference_is_exact() {
    for ok in ["${{ matrix.os }}", "${{ matrix.run-on_2 }}"] {
        assert!(
            findings("w.yml", &wf("on: [pull_request]", ok)).is_empty(),
            "{ok}"
        );
    }
    for bad in [
        "${{ matrix. }}",
        "${{ matrix.os || 'x' }}",
        "${{ matrix.os.x }}",
        "${{ matrix.os}}",
        "${{matrix.os }}",
    ] {
        let f = findings("w.yml", &wf("on: [pull_request]", bad));
        assert!(
            f.len() == 1 && f[0].starts_with("UNRESOLVED"),
            "{bad}: {f:?}"
        );
    }
}

/// A comment inside a block list is covered but adds nothing to the value.
#[test]
fn comment_inside_a_block_list_is_skipped() {
    let t = "    runs-on:\n      # note\n      - self-hosted\n";
    let (s, covered) = sites(t);
    assert_eq!(s[0].value, "- self-hosted");
    assert_eq!(covered, vec![1, 2, 3]);
}
