//! OSATE interop corpus — Tier-A of the AADL plug-fest (issue #246).
//!
//! Walks the vendored `osate/examples` corpus (verbatim under
//! test-data/interop/osate-examples, see PROVENANCE.md) and parses every
//! `.aadl` file with spar's own parser (`spar_syntax::parse`). The point is
//! **not** to be all-green — much of the OSATE corpus is annex-heavy (EMV2,
//! Behavior, Resolute, AGREE) that spar does not yet accept. The point is
//! to produce a *number* — parsed / total, bucketed by annex usage — and
//! to ratchet it: spar must never regress on a file it currently accepts.
//!
//! ## How the ratchet works
//!
//! Two committed baselines under `test-data/interop/baseline/` hold the
//! files spar currently fails to parse: `spar-gaps.txt` (genuine parser
//! gaps to fix) and `unadjudicated.txt` (bugtrack-* fixtures that may be
//! intentionally malformed — not spar bugs until the Tier-B OSATE oracle
//! confirms OSATE accepts them). The test fails only if a file in **neither**
//! baseline fails to parse (a genuine regression). Files leaving a baseline
//! are reported as progress — re-bless to lock the win in:
//!
//! ```text
//! SPAR_CORPUS_BLESS=1 cargo test -p spar --test osate_corpus
//! ```
//!
//! If the corpus is absent (it is vendored, so this should not happen on a
//! normal checkout), the test skips with a hint rather than failing.
//!
//! See issue #246 for the full plug-fest design (Tiers A/B/C, OSATE-in-
//! Docker round-trips, instance-equivalence). This file is Tier-A only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CORPUS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/osate-examples"
);
/// Genuine spar parser gaps — files spar *should* eventually parse.
/// Ratchet these down by fixing the parser (issue #246).
const BASELINE_GAPS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/baseline/spar-gaps.txt"
);
/// `bugtrack-*` regression fixtures that may be intentionally malformed —
/// NOT counted as spar bugs until the Tier-B OSATE oracle confirms OSATE
/// itself accepts them. Quarantined so a parser gap and a bad fixture are
/// never conflated.
const BASELINE_UNADJUDICATED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/baseline/unadjudicated.txt"
);

/// Coarse classification by which AADL annex (if any) the file leans on.
/// A file is bucketed by the first annex surface it mentions; the buckets
/// exist so the parsed/total number is legible (core drift is far more
/// alarming than an unimplemented annex).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Core,
    Emv2,
    Behavior,
    AnnexOther,
}

impl Bucket {
    fn label(self) -> &'static str {
        match self {
            Bucket::Core => "core",
            Bucket::Emv2 => "emv2",
            Bucket::Behavior => "behavior",
            Bucket::AnnexOther => "annex-other",
        }
    }

    const ALL: [Bucket; 4] = [
        Bucket::Core,
        Bucket::Emv2,
        Bucket::Behavior,
        Bucket::AnnexOther,
    ];
}

fn classify(src: &str) -> Bucket {
    let low = src.to_ascii_lowercase();
    if low.contains("annex emv2") || low.contains("annex error") {
        Bucket::Emv2
    } else if low.contains("annex behavior_specification") {
        Bucket::Behavior
    } else if low.contains("annex resolute")
        || low.contains("annex agree")
        || low.contains("annex verify")
    {
        Bucket::AnnexOther
    } else {
        Bucket::Core
    }
}

fn collect_aadl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_aadl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("aadl") {
            out.push(path);
        }
    }
}

fn read_failure_list(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Union of both baselines — the full set of files spar is allowed to
/// fail on without it being a regression.
fn load_baseline() -> BTreeSet<String> {
    read_failure_list(BASELINE_GAPS)
        .into_iter()
        .chain(read_failure_list(BASELINE_UNADJUDICATED))
        .collect()
}

fn write_baseline(path: &str, header: &str, files: &[String]) {
    let mut body = String::new();
    for line in header.lines() {
        body.push_str("# ");
        body.push_str(line);
        body.push('\n');
    }
    body.push_str("# Regenerate: SPAR_CORPUS_BLESS=1 cargo test -p spar --test osate_corpus\n");
    for f in files {
        body.push_str(f);
        body.push('\n');
    }
    std::fs::write(path, body).expect("write baseline");
}

#[test]
fn osate_corpus_parses_without_regression() {
    let corpus = Path::new(CORPUS_DIR);
    if !corpus.exists() {
        eprintln!(
            "::warning::OSATE corpus absent at {CORPUS_DIR} (it is vendored — \
             expected present on a normal checkout). Skipping."
        );
        return;
    }

    let mut files = Vec::new();
    collect_aadl(corpus, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "corpus directory present but contains no .aadl files"
    );

    // Per-bucket [parsed, total] tallies + the set of failing relative paths.
    let mut tally: std::collections::BTreeMap<&'static str, (usize, usize)> = Bucket::ALL
        .iter()
        .map(|b| (b.label(), (0usize, 0usize)))
        .collect();
    let mut failures: BTreeSet<String> = BTreeSet::new();

    for file in &files {
        let rel = file
            .strip_prefix(corpus)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(file).unwrap_or_default();
        let bucket = classify(&src);
        let parsed = spar_syntax::parse(&src).ok();

        let entry = tally.get_mut(bucket.label()).unwrap();
        entry.1 += 1;
        if parsed {
            entry.0 += 1;
        } else {
            failures.insert(rel);
        }
    }

    // Bless mode: rewrite the two baselines from the observed failures,
    // splitting `bugtrack-*` regression fixtures (unadjudicated — possibly
    // intentionally malformed) from genuine spar parser gaps.
    if std::env::var_os("SPAR_CORPUS_BLESS").is_some() {
        let (unadjudicated, gaps): (Vec<String>, Vec<String>) = failures
            .iter()
            .cloned()
            .partition(|f| f.contains("bugtrack"));
        write_baseline(
            BASELINE_GAPS,
            "spar parser gaps (issue #246, Tier-A): files spar should eventually\n\
             parse. A file leaving this list = spar improved; a file that fails\n\
             but is absent from EITHER baseline = a regression the test rejects.",
            &gaps,
        );
        write_baseline(
            BASELINE_UNADJUDICATED,
            "bugtrack-* regression fixtures — may be intentionally malformed.\n\
             NOT spar bugs until the Tier-B OSATE oracle confirms OSATE accepts them.",
            &unadjudicated,
        );
        eprintln!(
            "blessed: {} parser gaps, {} unadjudicated bugtrack fixtures",
            gaps.len(),
            unadjudicated.len()
        );
        return;
    }

    // Drift canary: always print the parsed/total breakdown.
    let total: usize = tally.values().map(|(_, t)| *t).sum();
    let parsed: usize = tally.values().map(|(p, _)| *p).sum();
    eprintln!("OSATE corpus (issue #246, Tier-A): {parsed}/{total} files parse");
    for label in Bucket::ALL.iter().map(|b| b.label()) {
        let (p, t) = tally[label];
        eprintln!("  {label:<12} {p:>4}/{t:<4}");
    }

    let expected = load_baseline();
    let regressions: Vec<&String> = failures.difference(&expected).collect();
    let newly_passing: Vec<&String> = expected.difference(&failures).collect();

    if !newly_passing.is_empty() {
        eprintln!(
            "::notice::{} corpus file(s) now parse that were in the baseline — \
             re-bless to lock the progress in (SPAR_CORPUS_BLESS=1).",
            newly_passing.len()
        );
    }

    assert!(
        regressions.is_empty(),
        "spar regressed: {} OSATE corpus file(s) failed to parse that are not in the \
         expected-failures baseline:\n{:#?}\n(If this is an intentional change, re-bless \
         with SPAR_CORPUS_BLESS=1 and justify in the PR.)",
        regressions.len(),
        regressions
    );
}
