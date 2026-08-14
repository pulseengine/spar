//! spar against TWO independent AADL implementations (#246, #420, #421).
//!
//! `osate_agreement.rs` compares spar to OSATE alone, which makes OSATE the
//! tie-breaker on every question. This adds Ocarina — OpenAADL / Telecom
//! ParisTech, written in Ada, sharing no code with OSATE's Xtext grammar — so
//! a disagreement can be adjudicated instead of assumed.
//!
//! # The three verdicts, and why "contested" is one of them
//!
//! ```text
//!   OSATE + Ocarina AGREE legal    -> spar must accept. Rejecting is a bug
//!                                     that stops the user opening the file.
//!   OSATE + Ocarina AGREE illegal  -> spar should reject. Accepting is a
//!                                     ratchet, not an error: the user's
//!                                     problem surfaces later, elsewhere.
//!   OSATE + Ocarina DIFFER         -> CONTESTED. Neither is the authority
//!                                     (AS5506 is), so spar cannot be wrong
//!                                     on the available evidence.
//! ```
//!
//! Contested rows are **exempt but counted**. Exempt because grading spar
//! against a disagreement would be picking a winner by fiat; counted because a
//! silently growing contested set is how "we checked" decays into "we stopped
//! being able to tell". The floor makes growth visible.
//!
//! This is what the maintainer's "OSATE is a test, not the authority" asks for
//! in mechanism rather than in prose. Before it, every OSATE quirk was
//! something spar was obliged to reproduce.
//!
//! # Why both columns are committed
//!
//! CI has neither tool — OSATE is a 370 MB Eclipse product and Ocarina needs
//! an Ada toolchain (nixpkgs marks `gprbuild` broken on aarch64-darwin, and it
//! does hang; Alire's prebuilt toolchain is what worked). Both columns are
//! committed references, like the `.aaxl2` files. Only spar's column is
//! re-derived live, because spar is what CI can execute and spar is what this
//! gate exists to catch moving.
//!
//! Regenerate per `tools/osate-conformance/README.md`. `ocarina -aadlv2 -f` —
//! the `-f` is load-bearing, and without it six files are falsely rejected for
//! want of the predefined property sets.

use std::path::PathBuf;

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/baseline/three-way.tsv"
);

/// Models spar accepts that BOTH implementations reject. MEASURED 48.
/// Lower as #420 is worked; it may never rise.
const MAX_TOO_PERMISSIVE: usize = 48;

/// Files where OSATE and Ocarina disagree. MEASURED 14.
///
/// A ceiling, not a target. These are exempt from grading, so an unnoticed
/// increase would quietly shrink the set of files this gate actually judges —
/// the same vacuity as a corpus that silently stops being scanned.
const MAX_CONTESTED: usize = 14;

/// Files spar rejects that BOTH accept, deliberately.
///
/// Empty, and allowed to be non-empty: two implementations can still both be
/// lenient where AS5506 is not. Each entry needs its clause. What may not
/// accumulate is the un-looked-at middle.
const ADJUDICATED_STRICTER: &[(&str, &str)] = &[
    // ("path.aadl", "AS5506 §X.Y forbids this; both tools accept it — filed as <link>"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Consensus {
    Legal,
    Illegal,
    Contested,
}

/// What the two independent implementations jointly say.
///
/// Split out and unit-tested because the corpus cannot exercise every branch:
/// `Contested` occurs 14 times today but could fall to zero, and a guard that
/// only real data reaches is a guard that stops being tested the moment the
/// data changes. Same lesson as the too-strict arm in `osate_agreement.rs`,
/// which was deletable with no test failing.
fn consensus(osate: &str, ocarina: &str) -> Consensus {
    match (osate, ocarina) {
        ("ACCEPT", "ACCEPT") => Consensus::Legal,
        ("REJECT", "REJECT") => Consensus::Illegal,
        _ => Consensus::Contested,
    }
}

#[test]
fn the_consensus_function_distinguishes_all_three_verdicts() {
    assert_eq!(consensus("ACCEPT", "ACCEPT"), Consensus::Legal);
    assert_eq!(consensus("REJECT", "REJECT"), Consensus::Illegal);
    // Contested must be reachable from BOTH orderings. A version testing only
    // one would pass while treating the other as agreement.
    assert_eq!(consensus("ACCEPT", "REJECT"), Consensus::Contested);
    assert_eq!(consensus("REJECT", "ACCEPT"), Consensus::Contested);
    assert_ne!(consensus("ACCEPT", "ACCEPT"), consensus("REJECT", "REJECT"));
}

struct Row {
    path: String,
    osate: String,
    ocarina: String,
}

fn load() -> Vec<Row> {
    let text = std::fs::read_to_string(BASELINE)
        .unwrap_or_else(|e| panic!("cannot read {BASELINE}: {e} — this file IS the oracle"));
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Row {
                path: f[0].to_string(),
                osate: f[2].to_string(),
                ocarina: f[3].to_string(),
            }
        })
        .collect()
}

#[test]
fn spar_agrees_with_the_consensus_of_two_independent_implementations() {
    let root = PathBuf::from(REPO);
    let rows = load();

    assert!(
        rows.len() > 100,
        "baseline has {} rows, expected >100 — a short baseline reports a clean \
         result because it measured almost nothing",
        rows.len()
    );

    // Both columns must vary. If either were constant the consensus would be
    // determined by one tool, which is the situation this test exists to end.
    let osate_acc = rows.iter().filter(|r| r.osate == "ACCEPT").count();
    let ocarina_acc = rows.iter().filter(|r| r.ocarina == "ACCEPT").count();
    for (name, acc) in [("osate", osate_acc), ("ocarina", ocarina_acc)] {
        assert!(
            acc > 0 && acc < rows.len(),
            "the {name} column is constant ({acc} of {}) — it contributes no \
             information and the 'consensus' would be one tool's opinion",
            rows.len()
        );
    }

    let mut too_strict: Vec<String> = Vec::new();
    let mut too_permissive: Vec<String> = Vec::new();
    let mut contested: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();

    for row in &rows {
        let Ok(src) = std::fs::read_to_string(root.join(&row.path)) else {
            absent.push(row.path.clone());
            continue;
        };
        let spar_accepts = spar_syntax::parse(&src).ok();
        match consensus(&row.osate, &row.ocarina) {
            Consensus::Legal if !spar_accepts => too_strict.push(row.path.clone()),
            Consensus::Illegal if spar_accepts => too_permissive.push(row.path.clone()),
            Consensus::Contested => contested.push(row.path.clone()),
            _ => {}
        }
    }

    assert!(
        absent.is_empty(),
        "{} baseline rows name files that no longer exist. A missing file drops \
         out of every cell, so deletions would improve the score:\n  {}",
        absent.len(),
        absent.join("\n  ")
    );

    // The strongest signal available: spar refuses a file TWO independent
    // implementations accept.
    let unadjudicated: Vec<&String> = too_strict
        .iter()
        .filter(|p| {
            !ADJUDICATED_STRICTER
                .iter()
                .any(|(known, _)| *known == p.as_str())
        })
        .collect();
    assert!(
        unadjudicated.is_empty(),
        "spar REJECTS {} file(s) that BOTH OSATE and Ocarina accept. Two \
         implementations sharing no code is the strongest evidence available \
         that spar is wrong — the user cannot open these at all. Fix the \
         parser, or add to ADJUDICATED_STRICTER with the AS5506 clause if spar \
         is right and both tools are lenient:\n  {}",
        unadjudicated.len(),
        unadjudicated
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    assert!(
        contested.len() <= MAX_CONTESTED,
        "{} contested files, above the declared {MAX_CONTESTED}. Contested rows \
         are EXEMPT from grading, so letting them grow silently shrinks what \
         this gate judges. Each new one needs a look:\n  {}",
        contested.len(),
        contested.join("\n  ")
    );

    assert!(
        too_permissive.len() <= MAX_TOO_PERMISSIVE,
        "{} models accepted by spar and rejected by BOTH implementations, above \
         the floor of {MAX_TOO_PERMISSIVE}. See #420:\n  {}",
        too_permissive.len(),
        too_permissive.join("\n  ")
    );

    if too_permissive.len() < MAX_TOO_PERMISSIVE {
        panic!(
            "{} too-permissive is BELOW the floor of {MAX_TOO_PERMISSIVE} — lower \
             MAX_TOO_PERMISSIVE to {} so the ground is held.",
            too_permissive.len(),
            too_permissive.len()
        );
    }
    if contested.len() < MAX_CONTESTED {
        panic!(
            "{} contested is BELOW the declared {MAX_CONTESTED} — lower \
             MAX_CONTESTED to {}. A ceiling left above the real number lets \
             contested cases creep back without notice.",
            contested.len(),
            contested.len()
        );
    }

    println!(
        "three-way: {} files, {} too-permissive (floor {}), {} contested \
         (ceiling {}), 0 unadjudicated too-strict",
        rows.len(),
        too_permissive.len(),
        MAX_TOO_PERMISSIVE,
        contested.len(),
        MAX_CONTESTED
    );
}

/// The three-way baseline must cover the same files as the two-way one.
///
/// Otherwise adding a model to one and not the other leaves it graded by OSATE
/// alone while the repo appears to have a three-way gate.
#[test]
fn the_three_way_baseline_covers_the_same_corpus_as_the_two_way() {
    let root = PathBuf::from(REPO);
    let two: std::collections::BTreeSet<String> =
        std::fs::read_to_string(root.join("test-data/interop/baseline/osate-first-party.tsv"))
            .expect("two-way baseline readable")
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| l.split('\t').next().unwrap_or(l).to_string())
            .collect();
    let three: std::collections::BTreeSet<String> = load().into_iter().map(|r| r.path).collect();

    let missing: Vec<&String> = two.difference(&three).collect();
    assert!(
        missing.is_empty(),
        "{} file(s) are in the OSATE baseline but not the three-way one, so \
         they are graded by one implementation while the repo looks like it \
         has two:\n  {}",
        missing.len(),
        missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
