//! The SysML v2 parser, graded by somebody else's models.
//!
//! ## Why this file exists
//!
//! Until it did, every SysML v2 number this project quoted was measured against
//! 43 fixtures we wrote ourselves, and `conformance_tests.rs` — the suite whose
//! name promised otherwise — could not fail. Its assertion was
//! `assert!(!result.syntax_node().text().is_empty())`, which checks that the
//! *input source text* is non-empty. That is true by construction. Zero of its
//! eight tests called `errors()`, so `parse_annex_a_simple_vehicle` reported
//! `ok` while the specification's own Annex A model failed to parse.
//!
//! That is the defect class `REQ-GUARD-GATE-EVIDENCE-002` names — an operation
//! that can produce "nothing happened" rendering identically to "it worked".
//! Four releases of that discipline went into the CI guardrails and none of it
//! had ever been pointed at the tool's own interop claim.
//!
//! ## What this gate does and does not claim
//!
//! It claims exactly one thing: **the official model files this parser accepts
//! are exactly the ones listed in `official_corpus_parsing.txt`, and that list
//! may only grow.** It does not claim the parser is correct — a file that
//! parses may still be lowered wrongly, and `Parse::ok()` is a syntax
//! predicate, not a semantic one.
//!
//! Nor is a parse *rate* a property of the parser alone. It is equally a
//! property of the corpus, which is why `test-data/sysml2/PROVENANCE.md`
//! classifies the failures rather than only counting them.
//!
//! ## Why a list and not a count
//!
//! Until `REQ-SYSML2-CORPUS-RATCHET-001` this gate compared the NUMBER of
//! parsing files to a constant. A count cannot see a swap: a change that made
//! two new files parse and broke one that used to would move the count up by
//! one and read as a pure win. The list names every file, so a file that stops
//! parsing fails the gate by name whatever else moved with it — and a file that
//! newly parses fails it too, until it is added, so the list stays the truth
//! rather than a lower bound that slack can leak back out of.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const CORPUS: &str = "test-data/sysml2/official";

/// The official model files that parse today, one corpus-relative path per
/// line, `/`-separated, sorted bytewise, no duplicates.
///
/// History of its size: 1 file on 2026-08-26 (the corpus pinned at
/// `29a3d2ac`); 16 on 2026-10-14 after `REQ-SYSML2-VISIBILITY-001`; 24 on
/// 2026-10-28 after `REQ-SYSML2-CORPUS-RATCHET-001` (multiplicity after a
/// type, `perform`, KerML classifiers).
const PARSING_LIST: &str = include_str!("official_corpus_parsing.txt");

/// Guards against the corpus vanishing. An empty walk parses nothing and
/// nothing fails; every listed file would read as a regression rather than as
/// "the directory is gone", and a `git clean` or a bad merge would look like a
/// parser bug. Checked separately so the two are never confused.
///
/// EXACT rather than a floor (`>= 300`): a partial re-vendor that lost a
/// handful of files that never parsed would pass a floor unnoticed and shrink
/// the denominator every published rate is quoted against.
const OFFICIAL_TOTAL: usize = 310;

/// Walks with `std::fs` rather than a shell glob on purpose: paths in this
/// corpus contain spaces (`kerml/examples/Address Book Example/…`), which
/// word-split under `for f in $(find …)` and silently turn one file into
/// several non-existent ones. That inflated the failure count twice while this
/// baseline was being measured. See PROVENANCE.md.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("sysml") | Some("kerml")
        ) {
            out.push(path);
        }
    }
}

fn corpus_root() -> PathBuf {
    PathBuf::from(REPO).join(CORPUS)
}

fn corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&corpus_root(), &mut files);
    files.sort();
    files
}

/// The corpus-relative, `/`-separated spelling the list uses, so it is the
/// same string on every host.
fn relative(path: &Path) -> String {
    let rel = path
        .strip_prefix(corpus_root())
        .unwrap_or_else(|_| panic!("{} is outside the corpus", path.display()));
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn listed() -> Vec<&'static str> {
    PARSING_LIST.lines().collect()
}

/// The same predicate the CLI exits non-zero on, so the gate and
/// `spar sysml2 parse` can never disagree about whether a file parsed.
fn parses(path: &Path) -> bool {
    // NOT `unwrap_or_default()`. An unreadable file would become an empty
    // string, an empty string parses clean, and the file would score as
    // PASSING — inflating the ratchet with files nobody read.
    let Ok(src) = std::fs::read_to_string(path) else {
        panic!("cannot read {}", path.display());
    };
    spar_sysml2::parse(&src).ok()
}

#[test]
fn the_corpus_is_present() {
    let files = corpus();
    assert_eq!(
        files.len(),
        OFFICIAL_TOTAL,
        "expected {OFFICIAL_TOTAL} vendored official model files under {CORPUS}, \
         found {}. A sweep that reads nothing agrees with every expectation, so \
         this is checked before the parse list rather than being allowed to \
         masquerade as a parser regression. If the pin moved, re-vendor with \
         tools/vendor-sysml2-corpus.sh and update OFFICIAL_TOTAL and \
         official_corpus_parsing.txt together.",
        files.len()
    );
}

/// The list is data the gate trusts, so it is checked as data first. A stale
/// entry (a path no longer in the corpus) would otherwise be skipped by any
/// set-difference that starts from the corpus, and a rename would silently
/// shrink what the gate protects.
// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn the_parsing_list_is_well_formed() {
    let list = listed();
    assert!(
        !list.is_empty(),
        "official_corpus_parsing.txt is empty — at least the files that parsed \
         when this gate was written must be listed"
    );

    let mut problems = Vec::new();
    for (i, entry) in list.iter().enumerate() {
        if entry.trim() != *entry || entry.is_empty() {
            problems.push(format!("line {}: blank or padded entry {entry:?}", i + 1));
        }
        if i > 0 && list[i - 1] >= *entry {
            problems.push(format!(
                "line {}: {entry:?} is not strictly after {:?} (unsorted or duplicate)",
                i + 1,
                list[i - 1]
            ));
        }
    }

    let present: BTreeSet<String> = corpus().iter().map(|f| relative(f)).collect();
    for entry in &list {
        if !present.contains(*entry) {
            problems.push(format!("{entry:?} is listed but is not in the corpus"));
        }
    }

    assert!(
        problems.is_empty(),
        "official_corpus_parsing.txt is malformed:\n  {}",
        problems.join("\n  ")
    );
}

/// The ratchet. Two-sided, by file name:
///
/// * a LISTED file that no longer parses is a grammar regression;
/// * an UNLISTED file that now parses is a win that must be added, so the list
///   cannot fall behind what the parser does and leave room for a later
///   regression to hide in.
// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn exactly_the_listed_official_files_parse() {
    let files = corpus();
    assert_eq!(
        files.len(),
        OFFICIAL_TOTAL,
        "corpus size changed; see the_corpus_is_present"
    );

    let list: BTreeSet<&str> = listed().into_iter().collect();
    let mut regressed = Vec::new();
    let mut unlisted = Vec::new();
    for file in &files {
        let rel = relative(file);
        match (list.contains(rel.as_str()), parses(file)) {
            (true, false) => regressed.push(rel),
            (false, true) => unlisted.push(rel),
            _ => {}
        }
    }

    let mut report = Vec::new();
    if !regressed.is_empty() {
        report.push(format!(
            "{} listed file(s) NO LONGER PARSE — a grammar regression, not a \
             corpus change (the corpus size is asserted separately):\n    {}",
            regressed.len(),
            regressed.join("\n    ")
        ));
    }
    if !unlisted.is_empty() {
        report.push(format!(
            "{} file(s) now parse but are NOT LISTED. This is a WIN — add them \
             to crates/spar-sysml2/tests/official_corpus_parsing.txt (sorted) so \
             it is locked in:\n    {}",
            unlisted.len(),
            unlisted.join("\n    ")
        ));
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// Non-vacuity. The gate above compares a predicate to a list; if `parses()`
/// were stuck on one answer the comparison would still pass whenever the list
/// happened to match. Both verdicts must be reachable on real input for the
/// list to carry information.
#[test]
fn the_parse_predicate_discriminates() {
    assert!(
        spar_sysml2::parse("package P { part def A; }").ok(),
        "a known-good model must parse — otherwise an empty list would mean \
         nothing about the corpus"
    );
    assert!(
        !spar_sysml2::parse("package P { part def").ok(),
        "a truncated model must NOT parse — otherwise every file would score as \
         passing and the ratchet would be decorative"
    );
}

/// The corpus is the point of this file, but a gate over 310 files says nothing
/// about *which* constructs are missing. These pin named grammar gaps that are
/// still open, so closing one shows up as a named test flipping rather than
/// only as files joining the list. Each is currently expected to FAIL.
/// `perform action` and KerML `class` were on this list until
/// `REQ-SYSML2-CORPUS-RATCHET-001` closed them; they are now pinned as PARSING
/// by `closed_grammar_gaps_stay_closed` below.
#[test]
fn the_known_grammar_gaps_are_the_ones_we_think_they_are() {
    let gaps: &[(&str, &str)] = &[
        (
            "abstract type",
            "package P { abstract type A specializes Base::Anything; }",
        ),
        // The next blocker in the specification's Annex A vehicle model
        // (`SysML v2 Spec Annex A SimpleVehicleModel.sysml:41`), once
        // `perform action providePower;` stopped it at line 35.
        (
            "exhibit state",
            "part def P { exhibit state s parallel { } }",
        ),
    ];
    let unexpectedly_ok: Vec<&str> = gaps
        .iter()
        .filter(|(_, src)| spar_sysml2::parse(src).ok())
        .map(|(name, _)| *name)
        .collect();

    assert!(
        unexpectedly_ok.is_empty(),
        "these grammar gaps now PARSE: {unexpectedly_ok:?}. That is progress — \
         move them to closed_grammar_gaps_stay_closed; the corpus gate will name \
         any official file that moved with them. The list exists so a fix is \
         visible as a named construct rather than only as a file count."
    );
}

/// Gaps that were on the list above and have been closed. Each must keep
/// parsing; these snippets were verified to FAIL on the commit before the fix.
// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn closed_grammar_gaps_stay_closed() {
    let closed: &[(&str, &str)] = &[
        (
            "perform action",
            "part def P { perform action providePower; }",
        ),
        ("KerML class", "package P { class ShoppingCart; }"),
    ];
    let reopened: Vec<&str> = closed
        .iter()
        .filter(|(_, src)| !spar_sysml2::parse(src).ok())
        .map(|(name, _)| *name)
        .collect();
    assert!(
        reopened.is_empty(),
        "closed grammar gaps no longer parse: {reopened:?}"
    );
}
