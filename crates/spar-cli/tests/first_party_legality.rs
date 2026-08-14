//! Every `.aadl` file **pulseengine itself writes** must have the verdict it
//! claims — the reverse direction of the OSATE plug-fest (issue #246).
//!
//! ## Why this exists
//!
//! `osate_corpus.rs` asks *can spar read what OSATE produced*, over 548
//! vendored files. That is the direction everyone tests, and it is the less
//! interesting one: those files are OSATE's, so OSATE accepts them by
//! construction. It says nothing about whether the AADL **we** write is
//! legal, and a model that only parses in spar is a wall the user hits the
//! moment they open it in OSATE.
//!
//! This test covers the 117 first-party files — the ones under our control.
//!
//! ## The gap that motivated it
//!
//! `test_data_parse_roundtrip.rs` sweeps only **top-level** `test-data/*.aadl`
//! (subdirectories excluded by design), and nothing walked `docs/` at all. So
//! `docs/research/forge-replication/descriptor.aadl` — added in #215, and whose
//! stated purpose is to be the AADL-form cross-check of `descriptor.sexp` —
//! sat unparseable for its entire life. It used `annex` as a subcomponent
//! identifier, plus ports named `in` and `out`; all three are AS5506B Annex A
//! reserved words. No test looked, so nothing said so.
//!
//! That the file was *wrong* is minor. That **no gate covered the directory**
//! is the defect this test closes.
//!
//! ## Verdicts are declared, not discovered
//!
//! A sweep that merely records what spar currently does would have called the
//! broken descriptor "expected: reject" and stayed green forever. So each file
//! carries an EXPECTED verdict derived from where it lives:
//!
//!   * `test-data/negative/**` — must be REJECTED. These exist to be invalid.
//!   * everything else         — must be ACCEPTED.
//!
//! A first-party file that lands in neither class is a bug in this test's
//! policy, not a licence to pass.
//!
//! Unlike `osate_corpus.rs` there is deliberately **no ratchet**. A baseline is
//! the right tool for someone else's corpus, where a failure may be an
//! unimplemented annex. These files are ours: if one stops parsing, we either
//! broke the parser or committed bad AADL, and both should be loud.
//!
//! ## What this does NOT claim
//!
//! That an accepted file is *AS5506-legal*. spar's parser is more permissive
//! than the standard in at least one known way (it accepts a port named `in`
//! or `out`, which the 548-file OSATE corpus never does in 5414 declarations),
//! so "spar accepts it" and "OSATE accepts it" are different questions. The
//! second needs OSATE and is Tier-B's job. This test pins the first.

use std::path::{Path, PathBuf};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Trees vendored from upstream OSATE. Not our AADL, not our verdict to
/// declare — `osate_corpus.rs` ratchets those separately.
const VENDORED: &[&str] = &[
    "test-data/interop",
    "test-data/osate2",
    "test-data/osate2-inline",
];

/// Files whose whole purpose is to be rejected.
const MUST_REJECT: &str = "test-data/negative";

/// Same opt-out marker `test_data_parse_roundtrip.rs` honours, so a fixture
/// needs one convention rather than two.
const SKIP_MARKER: &str = "-- spar-test: skip-parse";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    Accept,
    Reject,
}

fn collect_aadl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip build output and VCS internals; they never hold sources.
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(name, "target" | ".git" | "node_modules") {
                continue;
            }
            collect_aadl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("aadl") {
            out.push(path);
        }
    }
}

/// Path relative to the repo root, with forward slashes.
fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The corpus under test, and the single definition of it.
///
/// Both tests below call this rather than walking the tree themselves. That is
/// load-bearing: the first version of this file had `docs_aadl_is_covered_by
/// _the_sweep` do its own independent walk of `docs/`, which meant narrowing
/// the sweep to `test-data/` left BOTH tests green — the coverage test was
/// re-deriving its own corpus instead of inspecting the one actually swept.
/// Verified by mutation: that change now fails, and previously did not.
fn first_party_cases(root: &Path) -> Vec<(String, Expect)> {
    let mut all = Vec::new();
    collect_aadl(root, &mut all);

    let mut cases: Vec<(String, Expect)> = Vec::new();
    for path in &all {
        let r = rel(path, root);
        if VENDORED.iter().any(|v| r.starts_with(v)) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        if src.lines().take(20).any(|l| l.contains(SKIP_MARKER)) {
            continue;
        }
        let expect = if r.starts_with(MUST_REJECT) {
            Expect::Reject
        } else {
            Expect::Accept
        };
        cases.push((r, expect));
    }
    cases.sort_by(|a, b| a.0.cmp(&b.0));
    cases
}

#[test]
fn every_first_party_aadl_has_the_verdict_it_claims() {
    let root = PathBuf::from(REPO);
    let cases = first_party_cases(&root);

    // Blindness guard. An empty sweep agrees with every expectation, so zero
    // failures would otherwise be indistinguishable from zero files read.
    assert!(
        cases.len() > 100,
        "expected >100 first-party .aadl files, found {} — the walk is broken \
         or the corpus moved, and a sweep that reads nothing passes everything",
        cases.len()
    );

    // Discrimination guard. If every file carried the same expectation, a
    // verdict function stuck on one answer would pass. Both classes must be
    // exercised for the result to mean anything.
    let n_reject = cases.iter().filter(|(_, e)| *e == Expect::Reject).count();
    let n_accept = cases.len() - n_reject;
    assert!(
        n_reject > 0 && n_accept > 0,
        "both verdict classes must be present (accept={n_accept}, reject={n_reject})"
    );

    let mut wrong: Vec<String> = Vec::new();
    for (r, expect) in &cases {
        let src = std::fs::read_to_string(root.join(r)).expect("file readable");
        let got = if spar_syntax::parse(&src).ok() {
            Expect::Accept
        } else {
            Expect::Reject
        };
        if got != *expect {
            let detail = match expect {
                // Ours, and it does not parse: either we broke the parser or
                // we committed AADL that is not AADL.
                Expect::Accept => "expected ACCEPT, spar REJECTED it",
                // Lives in test-data/negative/ and parsed anyway: the fixture
                // no longer tests what it was written to test.
                Expect::Reject => "expected REJECT, spar ACCEPTED it",
            };
            wrong.push(format!("  {r}: {detail}"));
        }
    }

    assert!(
        wrong.is_empty(),
        "{} of {} first-party .aadl files have the wrong verdict \
         ({n_accept} expected-accept, {n_reject} expected-reject):\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

/// `docs/` must be INSIDE the swept corpus — the coverage gap itself, gated.
///
/// The sweep above is the mechanism; this is the claim about its reach. Note
/// what it asserts: not "the files under docs/ parse" (the sweep already says
/// that, and re-checking it here proves nothing about coverage) but "the
/// sweep's own case list contains them". Narrowing the walk to `test-data/`
/// leaves the sweep green on a smaller corpus, and only an assertion phrased
/// against `first_party_cases` can see that.
#[test]
fn the_sweep_actually_reaches_docs() {
    let root = PathBuf::from(REPO);
    let cases = first_party_cases(&root);

    let docs: Vec<&String> = cases
        .iter()
        .map(|(r, _)| r)
        .filter(|r| r.starts_with("docs/"))
        .collect();

    assert!(
        !docs.is_empty(),
        "the sweep's corpus contains no docs/**.aadl. Either the walk was \
         narrowed (the sweep then passes having read less), or the files \
         moved. Uncovered docs/ is exactly how descriptor.aadl stayed \
         unparseable from #215 until this test existed.\n\
         swept {} files, none under docs/",
        cases.len()
    );

    // Every docs model is claimed valid: a documentation artifact that is not
    // valid AADL misleads every reader who trusts it.
    for r in &docs {
        assert_eq!(
            cases.iter().find(|(p, _)| &p == r).map(|(_, e)| *e),
            Some(Expect::Accept),
            "{r} is under docs/ but not expected to parse"
        );
    }
}
