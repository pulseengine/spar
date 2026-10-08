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
//! It claims exactly one thing: **the number of official model files this
//! parser accepts is what the constant says, and it may only rise.** It does
//! not claim the parser is correct — a file that parses may still be lowered
//! wrongly, and `Parse::ok()` is a syntax predicate, not a semantic one.
//!
//! Nor is a parse *rate* a property of the parser alone. It is equally a
//! property of the corpus, which is why `test-data/sysml2/PROVENANCE.md`
//! classifies the failures rather than only counting them: 309 failures across
//! five distinct first-error kinds is a handful of grammar gaps, and reporting
//! it as "0.3%" without that classification would be alarmism.

use std::path::{Path, PathBuf};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const CORPUS: &str = "test-data/sysml2/official";

/// How many official model files the parser accepts today.
///
/// EXACT, not a floor — below fails as a regression, above fails until the
/// constant is raised. Same two-sided discipline as `MAX_TOO_PERMISSIVE` in the
/// OSATE agreement gate: a win that is not locked in is slack that quietly
/// leaks back out.
///
/// Measured 2026-08-26 against the corpus pinned at `29a3d2ac`. The path to
/// moving it is `REQ-SYSML2-VISIBILITY-001` — quoted names and visibility
/// modifiers alone account for 258 of the 309 failures.
const OFFICIAL_PARSING: usize = 1;

/// Guards against the corpus vanishing. An empty walk parses nothing and
/// nothing fails; the count would read as a catastrophic regression rather than
/// as "the directory is gone", and a `git clean` or a bad merge would look like
/// a parser bug. Checked separately so the two are never confused.
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

fn corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&PathBuf::from(REPO).join(CORPUS), &mut files);
    files.sort();
    files
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
         this is checked before the parse count rather than being allowed to \
         masquerade as a parser regression. If the pin moved, re-vendor with \
         tools/vendor-sysml2-corpus.sh and update both constants together.",
        files.len()
    );
}

#[test]
fn the_official_parse_count_is_exactly_what_we_claim() {
    let files = corpus();
    assert_eq!(
        files.len(),
        OFFICIAL_TOTAL,
        "corpus size changed; see the_corpus_is_present"
    );

    let passing: Vec<&PathBuf> = files.iter().filter(|f| parses(f)).collect();
    let n = passing.len();

    if n > OFFICIAL_PARSING {
        panic!(
            "{n} official files now parse, above the declared {OFFICIAL_PARSING}. \
             This is a WIN — raise OFFICIAL_PARSING to {n} so it is locked in \
             rather than left as slack that can leak back out."
        );
    }
    assert_eq!(
        n, OFFICIAL_PARSING,
        "{n} official files parse, below the declared {OFFICIAL_PARSING}. A file \
         that parsed has stopped parsing — that is a regression in the grammar, \
         not a corpus change (the corpus size is asserted separately)."
    );
}

/// Non-vacuity. The gate above compares a count to a constant; if `parses()`
/// were stuck on one answer the comparison would still pass whenever the
/// constant happened to match. Both verdicts must be reachable on real input
/// for the count to carry information.
#[test]
fn the_parse_predicate_discriminates() {
    assert!(
        spar_sysml2::parse("package P { part def A; }").ok(),
        "a known-good model must parse — otherwise a count of zero would mean \
         nothing about the corpus"
    );
    assert!(
        !spar_sysml2::parse("package P { part def").ok(),
        "a truncated model must NOT parse — otherwise every file would score as \
         passing and the ratchet would be decorative"
    );
}

/// The corpus is the point of this file, but a gate over 310 files says nothing
/// about *which* constructs are missing. These pin the four grammar gaps that
/// account for the failures, so closing one shows up as a named test flipping
/// rather than only as a number moving. Each is currently expected to FAIL —
/// when `REQ-SYSML2-VISIBILITY-001` lands, these invert and say so loudly.
#[test]
fn the_known_grammar_gaps_are_the_ones_we_think_they_are() {
    let gaps: &[(&str, &str)] = &[
        ("quoted name", "package 'Application Layer' { }"),
        (
            "visibility on import",
            "package P { private import Objects::*; }",
        ),
        ("KerML class", "package P { class ShoppingCart; }"),
        (
            "abstract type",
            "package P { abstract type A specializes Base::Anything; }",
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
         remove them from this list and raise OFFICIAL_PARSING, which will have \
         moved. The list exists so a fix is visible as a named construct rather \
         than only as a count."
    );
}
