//! Does OSATE accept the AADL pulseengine writes? — Tier-B, reverse direction.
//!
//! `osate_corpus.rs` asks whether spar can read OSATE's files. This asks the
//! question that actually bites a user: **they open our model in OSATE and it
//! errors.** Measured over the 117 first-party `.aadl` files (#420).
//!
//! ## Why the OSATE column is committed rather than computed
//!
//! CI has no OSATE — it is a 370 MB Eclipse product. So the OSATE verdicts are
//! a committed reference in `osate-first-party.tsv`, regenerated deliberately
//! (see `tools/osate-conformance/README.md`), exactly as the `.aaxl2`
//! references are. The **spar** column is re-derived live on every run.
//!
//! That asymmetry is the design, not a shortcut. The gate's job is to catch
//! *spar* moving, and spar is what CI can execute. An OSATE upgrade changing a
//! verdict is a deliberate re-blessing, not something to discover mid-PR.
//!
//! ## What ratchets, and what may never move
//!
//! ```text
//!                    OSATE accepts   OSATE rejects
//!   spar accepts          63              54        <- ratchet, may only fall
//!   spar rejects           0              20        <- HARD ZERO
//! ```
//!
//! **too-strict must stay 0.** spar rejecting AADL that OSATE accepts means we
//! refuse a model the reference implementation considers valid — the user
//! cannot use spar at all on that file. There is no acceptable non-zero value,
//! so this is an invariant rather than a ratchet.
//!
//! **too-permissive ratchets down from 54.** Each is a model we accept and
//! OSATE refuses: the user's error arrives later, in another tool. Fixing one
//! lowers the floor; the test says so when it drops.
//!
//! ## What this deliberately does NOT do
//!
//! Assert that the live spar column equals the committed spar column. A
//! changed spar verdict is usually *progress* — we tighten the parser, a file
//! moves from the too-permissive cell into agreement. Pinning the column would
//! make every fix look like a regression.
//!
//! The committed spar column is **advisory, not decorative**: it feeds the
//! `moved` list, which the below-floor message uses to tell "spar got
//! stricter" apart from "someone edited the OSATE column". Nothing verifies it
//! is still accurate, so a hand-edited wrong value would report a phantom move
//! on every run. Re-generate both columns together.
//!
//! Note also that the OSATE column came from OSATE's **full validation** while
//! spar's comes from `parse`. Some of the 54 may be caught by later spar
//! stages (`spar instance`); #420 tracks re-running those at matched stage.
//! The floor is honest about what was measured, not about what it means.

use std::path::{Path, PathBuf};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/interop/baseline/osate-first-party.tsv"
);

/// Models spar accepts that OSATE refuses. MEASURED 54 on OSATE 2.18.0.
/// Lower this as #420's categories are fixed; it may never rise.
const MAX_TOO_PERMISSIVE: usize = 54;

struct Row {
    path: String,
    spar_committed: String,
    osate: String,
}

/// Which cell of the 2x2 a (spar, OSATE) verdict pair falls in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cell {
    Agree,
    /// spar refuses what OSATE accepts — the invariant that must stay empty.
    TooStrict,
    /// spar accepts what OSATE refuses — the ratchet.
    TooPermissive,
}

/// Split out of the sweep so it can be tested on SYNTHETIC pairs.
///
/// This is not decomposition for tidiness. Inline, the `TooStrict` arm was
/// unreachable by any corpus-level mutation: real data has zero too-strict
/// files, so deleting the arm let those pairs fall to the catch-all and the
/// suite stayed green *even with a too-strict row injected into the baseline*.
/// Verified — the arm was decorative. A guard for an empty cell cannot be
/// exercised by data in which the cell is empty, so it must be tested
/// directly. See `the_classifier_distinguishes_all_four_cells`.
fn classify(spar: &str, osate: &str) -> Cell {
    match (spar, osate) {
        ("REJECT", "ACCEPT") => Cell::TooStrict,
        ("ACCEPT", "REJECT") => Cell::TooPermissive,
        _ => Cell::Agree,
    }
}

#[test]
fn the_classifier_distinguishes_all_four_cells() {
    // All four are reachable here by construction, including the one the live
    // corpus can never produce while spar stays correct.
    assert_eq!(classify("ACCEPT", "ACCEPT"), Cell::Agree);
    assert_eq!(classify("REJECT", "REJECT"), Cell::Agree);
    assert_eq!(classify("REJECT", "ACCEPT"), Cell::TooStrict);
    assert_eq!(classify("ACCEPT", "REJECT"), Cell::TooPermissive);

    // The two failure cells must be distinct. A classifier collapsing them
    // would satisfy "not Agree" on both and still misreport which direction
    // spar diverged in — and the two have opposite remedies.
    assert_ne!(classify("REJECT", "ACCEPT"), classify("ACCEPT", "REJECT"));
}

fn load_baseline() -> Vec<Row> {
    let text = std::fs::read_to_string(BASELINE).unwrap_or_else(|e| {
        panic!(
            "cannot read {BASELINE}: {e} — the baseline IS the OSATE oracle; \
                without it this test has nothing to compare against"
        )
    });
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut f = l.split('\t');
            let path = f.next().expect("path column").to_string();
            let spar_committed = f.next().expect("spar column").to_string();
            let osate = f.next().expect("osate column").to_string();
            Row {
                path,
                spar_committed,
                osate,
            }
        })
        .collect()
}

#[test]
fn spar_is_never_stricter_than_osate_and_permissiveness_only_falls() {
    let root = PathBuf::from(REPO);
    let rows = load_baseline();

    // Blindness guard. An empty or truncated baseline yields 0 violations in
    // both cells and would read as a perfect score.
    assert!(
        rows.len() > 100,
        "baseline has {} rows, expected >100 — a short baseline reports a \
         clean matrix because it measured almost nothing",
        rows.len()
    );

    // Discrimination guard. If OSATE's column were constant the matrix could
    // not distinguish the two failure directions at all.
    let osate_accepts = rows.iter().filter(|r| r.osate == "ACCEPT").count();
    assert!(
        osate_accepts > 0 && osate_accepts < rows.len(),
        "the OSATE column must contain both verdicts (accept={osate_accepts} \
         of {}) or the matrix has only one live cell",
        rows.len()
    );

    let mut too_strict: Vec<String> = Vec::new();
    let mut too_permissive: Vec<String> = Vec::new();
    let mut moved: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();

    for row in &rows {
        let full = root.join(&row.path);
        let Ok(src) = std::fs::read_to_string(&full) else {
            // A baseline row whose file vanished silently shrinks the corpus.
            absent.push(row.path.clone());
            continue;
        };
        let spar_live = if spar_syntax::parse(&src).ok() {
            "ACCEPT"
        } else {
            "REJECT"
        };
        if spar_live != row.spar_committed {
            moved.push(format!(
                "  {}: spar was {}, now {}",
                row.path, row.spar_committed, spar_live
            ));
        }
        match classify(spar_live, &row.osate) {
            Cell::TooStrict => too_strict.push(row.path.clone()),
            Cell::TooPermissive => too_permissive.push(row.path.clone()),
            Cell::Agree => {}
        }
    }

    assert!(
        absent.is_empty(),
        "{} baseline rows name files that no longer exist. Every missing file \
         silently drops out of BOTH cells, so deletions would improve the \
         score. Update the baseline deliberately:\n  {}",
        absent.len(),
        absent.join("\n  ")
    );

    // Report movement before asserting: on a run that fixes something, this is
    // the useful output, and it must survive a later assertion failing.
    if !moved.is_empty() {
        println!(
            "spar's verdict changed on {} file(s) since the baseline:\n{}",
            moved.len(),
            moved.join("\n")
        );
    }

    // The invariant. Not a ratchet: there is no acceptable number of files we
    // refuse that the reference implementation accepts.
    assert!(
        too_strict.is_empty(),
        "spar REJECTS {} file(s) that OSATE ACCEPTS. This is the one direction \
         that must stay empty — a user cannot open these in spar at all, \
         though they are valid AADL:\n  {}",
        too_strict.len(),
        too_strict.join("\n  ")
    );

    assert!(
        too_permissive.len() <= MAX_TOO_PERMISSIVE,
        "{} models are accepted by spar and REJECTED by OSATE, above the \
         declared floor of {MAX_TOO_PERMISSIVE}. Each one is an error the user \
         meets later, in another tool. See #420 for the taxonomy:\n  {}",
        too_permissive.len(),
        too_permissive.join("\n  ")
    );

    if too_permissive.len() < MAX_TOO_PERMISSIVE {
        // This branch fires on every future win, so it must say WHY the count
        // fell. Two very different routes reach it: spar got stricter (the
        // `moved` list is non-empty — re-bless the TSV too), or the baseline's
        // OSATE column was edited (empty `moved` — check that was deliberate).
        // Identical messages for both would leave the reader guessing.
        let why = if moved.is_empty() {
            "no spar verdict changed, so the OSATE column moved — confirm that \
             re-blessing was intended"
                .to_string()
        } else {
            format!(
                "spar's verdict changed on {} file(s), so re-generate the \
                 baseline's spar column as well:\n{}",
                moved.len(),
                moved.join("\n")
            )
        };
        panic!(
            "{} too-permissive models is BELOW the floor of \
             {MAX_TOO_PERMISSIVE}. Lower MAX_TOO_PERMISSIVE to {} so the \
             ground is held — a floor left above the real number silently \
             re-admits what was just fixed.\n{why}",
            too_permissive.len(),
            too_permissive.len()
        );
    }

    println!(
        "OSATE agreement: {} files, {} agree, {} too-permissive (floor {}), \
         0 too-strict",
        rows.len(),
        rows.len() - too_permissive.len() - too_strict.len(),
        too_permissive.len(),
        MAX_TOO_PERMISSIVE
    );
}

/// The baseline must describe the corpus the other test sweeps.
///
/// Without this, adding a first-party `.aadl` leaves it outside the matrix:
/// `first_party_legality.rs` would check it against spar, and nothing would
/// ever ask OSATE. The corpus would grow while the oracle's coverage of it
/// silently shrank.
#[test]
fn the_baseline_covers_every_first_party_file() {
    let root = PathBuf::from(REPO);
    let rows = load_baseline();
    let known: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.path.as_str()).collect();

    let mut all = Vec::new();
    collect_aadl(&root, &mut all);
    let mut unlisted: Vec<String> = Vec::new();
    for path in &all {
        let r = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if r.starts_with("test-data/interop") || r.starts_with("test-data/osate2") {
            continue; // vendored; osate_corpus.rs owns those
        }
        if !known.contains(r.as_str()) {
            unlisted.push(r);
        }
    }
    unlisted.sort();

    assert!(
        unlisted.is_empty(),
        "{} first-party .aadl file(s) are not in the OSATE baseline, so no \
         one has ever asked OSATE about them. Run the oracle (see \
         tools/osate-conformance/README.md) and add the rows:\n  {}",
        unlisted.len(),
        unlisted.join("\n  ")
    );
}

fn collect_aadl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
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
