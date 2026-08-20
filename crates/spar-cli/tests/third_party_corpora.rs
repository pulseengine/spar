//! Tier-A over corpora written by neither us nor the OSATE team (#421).
//!
//! `osate_corpus.rs` ratchets the 548 vendored OSATE examples. This ratchets
//! four more, and the reason there are four is the whole point.
//!
//! # Why more than one corpus
//!
//! `applies to (system, system implementation)` was **rejected by spar and
//! accepted by OSATE with 0 diagnostics** — a bug in the direction that makes
//! spar unusable on a valid file. The 548-model OSATE corpus uses that
//! construct in ZERO files. So do our own 120. No amount of parse-rate on
//! either could have surfaced it; it took the Galois corpus, and once that
//! opened the area, three more owner forms turned up in AADLib and one in
//! VERDICT.
//!
//! **Two corpora agreeing on what they do not contain is a shared blind spot,
//! not a signal.**
//!
//! # These are tests, not specifications
//!
//! Two AADLib files are rejected by spar AND by OSATE, both with grammar
//! errors — Ocarina-specific syntax. A corpus is another opinion, not another
//! authority. Its failures are recorded in the baseline without any claim that
//! spar is wrong; see `osate_agreement.rs` for how divergence gets adjudicated.
//!
//! # On the numbers
//!
//! A parse-rate measures the corpus — its duplication, its style, its era — at
//! least as much as the parser. VERDICT's original 14 failures were 14 copies
//! of one file: one distinct gap. This test therefore ratchets a **set of
//! paths**, not a percentage, so a corpus that duplicates a file cannot make
//! the number move without a real change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/baseline/third-party-gaps.txt"
);

/// (directory, minimum models expected) — the floor guards against a corpus
/// silently disappearing, which would make "no regressions" vacuously true.
const CORPORA: &[(&str, usize)] = &[
    ("test-data/interop/case-aadl", 52),
    ("test-data/interop/aadlib", 239),
    ("test-data/interop/fmw", 924),
    ("test-data/interop/verdict", 132),
];

/// Every third-party-gaps row must carry one of these class tokens as its
/// second tab-separated field. #427: a raw count of the ratchet was being read
/// as raw debt — nine tenths of it is not. The class token names WHY a row is
/// in the file, so a "gap" file that is not AADL at all is not accounted for
/// the same as a "gap" file spar is being too strict on.
///
/// A row whose class is not in this set fails `every_baseline_entry_is_classified`.
/// So the moment someone adds a NEW row — which is the moment someone actually
/// looks — the class has to be settled explicitly, in the file that the
/// count is read from, not in a follow-up comment somewhere else.
const CLASSES: &[&str] = &["SPAR-DEFECT", "UPSTREAM-INVALID", "AADLV1", "MALFORMED-V2"];

/// SPAR-DEFECT is the only class that names debt spar owes — a legal AADL
/// construct we reject. It is asserted EXACTLY, like `MAX_TOO_PERMISSIVE` in
/// `three_way_conformance.rs`: above the floor is a regression, below the
/// floor means the ground was held and the constant must fall to lock it in.
///
/// This number is not the total row count of `third-party-gaps.txt`; that
/// total also includes files no parser accepts and files written to an
/// older AADL revision. Bundling them into one number is what let #427's
/// "14 vendored models that spar does not parse" read as 14 units of debt
/// when it was 3.
///
/// Lower this number as (a) and (b) of #427 land. Do not raise it — a new
/// SPAR-DEFECT is a regression the corpus is meant to catch, not to absorb.
const MAX_SPAR_DEFECT: usize = 3;

/// One parsed row from `third-party-gaps.txt` (path + class + first-diagnostic).
struct Gap {
    path: String,
    class: String,
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

fn read_baseline() -> String {
    std::fs::read_to_string(BASELINE).unwrap_or_else(|e| panic!("cannot read {BASELINE}: {e}"))
}

/// Every non-comment, non-empty row parsed into (path, class). A row without a
/// second tab-separated field is kept with an empty class so the classifier
/// test can call out the offending row rather than silently dropping it.
fn parse_baseline(src: &str) -> Vec<Gap> {
    src.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut parts = l.splitn(3, '\t');
            let path = parts.next().unwrap_or("").to_string();
            let class = parts.next().unwrap_or("").to_string();
            Gap { path, class }
        })
        .collect()
}

fn known_gaps() -> BTreeSet<String> {
    parse_baseline(&read_baseline())
        .into_iter()
        .map(|g| g.path)
        .collect()
}

#[test]
fn third_party_corpora_parse_without_regression() {
    let root = PathBuf::from(REPO);
    let gaps = known_gaps();

    let mut regressions: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut newly_passing: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    for (dir, floor) in CORPORA {
        let mut files = Vec::new();
        collect_aadl(&root.join(dir), &mut files);

        // Corpus-present guard. A vendored tree that vanished would produce
        // zero failures and read as a clean run.
        assert!(
            files.len() >= *floor,
            "{dir}: found {} models, expected at least {floor}. The corpus is \
             missing or truncated, and an absent corpus cannot regress — see \
             test-data/interop/PROVENANCE-THIRD-PARTY.md",
            files.len()
        );

        files.sort();
        for path in &files {
            scanned += 1;
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            // NOT `unwrap_or_default()`. An unreadable file (permissions, a
            // broken symlink, an I/O error) would become an empty string,
            // empty input parses clean, and the file would score as PASSING —
            // the error path yielding the ideal reading, which is the exact
            // defect class this suite exists to catch. The corpus-count floor
            // above only notices files that DISAPPEAR, not ones that go
            // unreadable.
            let Ok(src) = std::fs::read_to_string(path) else {
                unreadable.push(rel);
                continue;
            };
            let parsed = spar_syntax::parse(&src).ok();
            match (parsed, gaps.contains(&rel)) {
                (false, false) => regressions.push(rel),
                (true, true) => newly_passing.push(rel),
                _ => {}
            }
        }
    }

    // Progress is reported, not failed — but it must be re-blessed, or the
    // baseline slowly becomes a list of things that no longer break.
    if !newly_passing.is_empty() {
        println!(
            "{} file(s) now parse that the baseline lists as gaps — remove them \
             from test-data/interop/baseline/third-party-gaps.txt to lock the \
             win in:\n  {}",
            newly_passing.len(),
            newly_passing.join("\n  ")
        );
    }

    // A file we could not read is not a file that passed.
    assert!(
        unreadable.is_empty(),
        "{} corpus file(s) could not be read. Not treated as passing: an \
         unreadable file that scores clean is indistinguishable from a \
         healthy one:\n  {}",
        unreadable.len(),
        unreadable.join("\n  ")
    );

    assert!(
        regressions.is_empty(),
        "{} third-party model(s) stopped parsing and are not in the baseline. \
         These corpora are written by other teams against other tools, so a new \
         failure here is usually a real gap rather than a bad model — but check \
         which before re-blessing:\n  {}",
        regressions.len(),
        regressions.join("\n  ")
    );

    println!(
        "{scanned} third-party models scanned, {} known gaps",
        gaps.len()
    );
}

/// The baseline must describe files that exist.
///
/// A stale entry is worse than a missing one: it silently excuses a path that
/// is no longer there, and if a file with that name ever returns, its failure
/// is pre-forgiven.
#[test]
fn every_baseline_entry_names_a_real_file() {
    let root = PathBuf::from(REPO);
    let missing: Vec<String> = known_gaps()
        .into_iter()
        .filter(|p| !root.join(p).is_file())
        .collect();
    assert!(
        missing.is_empty(),
        "{} baseline entries name files that do not exist — remove them:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Every baseline row shall carry a recognised class in its second field. #427.
///
/// The point of the class is that a NEW row cannot be ratcheted in without
/// someone deciding whether it is a spar defect, a genuinely invalid file, an
/// AADLv1 leftover, or a malformed v2 file. Rejecting the unclassified row
/// forces that decision at the moment someone actually looks.
#[test]
fn every_baseline_entry_is_classified() {
    let rows = parse_baseline(&read_baseline());
    let known: BTreeSet<&str> = CLASSES.iter().copied().collect();
    let bad: Vec<String> = rows
        .iter()
        .filter(|g| !known.contains(g.class.as_str()))
        .map(|g| format!("{}  [class={:?}]", g.path, g.class))
        .collect();
    assert!(
        bad.is_empty(),
        "{} row(s) in test-data/interop/baseline/third-party-gaps.txt do not \
         carry a recognised class in field 2. Every row must be classified \
         (see the file's header) — {:?}:\n  {}",
        bad.len(),
        CLASSES,
        bad.join("\n  ")
    );
}

/// The SPAR-DEFECT count in the baseline shall equal `MAX_SPAR_DEFECT` exactly.
///
/// The number is what #427 measured, held two-sided like `MAX_TOO_PERMISSIVE`.
/// Above the floor: a new too-strict rejection has landed and the ratchet is
/// meant to red rather than absorb it. Below the floor: a defect was fixed
/// but the constant was not walked back, and the ground is at risk of being
/// re-lost silently.
///
/// The class distribution (SPAR-DEFECT + others = total gaps) is also printed
/// so a maintainer reading a failure sees the full breakdown, not just the
/// number that went out of bounds.
#[test]
fn spar_defect_count_matches_ratchet() {
    let rows = parse_baseline(&read_baseline());

    let mut by_class: BTreeMap<String, usize> = BTreeMap::new();
    for g in &rows {
        *by_class.entry(g.class.clone()).or_insert(0) += 1;
    }
    let spar_defect = by_class.get("SPAR-DEFECT").copied().unwrap_or(0);

    let breakdown = || -> String {
        by_class
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    assert!(
        spar_defect <= MAX_SPAR_DEFECT,
        "SPAR-DEFECT count is {spar_defect}, above the ratchet floor of \
         {MAX_SPAR_DEFECT}. A new spar-owned parser gap has been ratcheted \
         into third-party-gaps.txt. This ratchet is exact so absorbing new \
         defects into the raw total (as #427 called out) is not an option — \
         fix the parser, or if the new row is not our defect, classify it as \
         UPSTREAM-INVALID / AADLV1 / MALFORMED-V2. Full breakdown: {}",
        breakdown()
    );
    assert!(
        spar_defect >= MAX_SPAR_DEFECT,
        "SPAR-DEFECT count is {spar_defect}, BELOW the ratchet floor of \
         {MAX_SPAR_DEFECT} — a spar defect was fixed but MAX_SPAR_DEFECT was \
         not walked back. Lower MAX_SPAR_DEFECT to {spar_defect} in \
         third_party_corpora.rs to lock the win in. Full breakdown: {}",
        breakdown()
    );
}
