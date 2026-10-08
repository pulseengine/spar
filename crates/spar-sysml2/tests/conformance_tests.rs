//! Named official SysML v2 files, each with the verdict it actually gets.
//!
//! ## What this file used to be
//!
//! Eight tests that could not fail. Seven asserted on the CST's *text* —
//! `!result.syntax_node().text().is_empty()`, `text().contains("port")`,
//! `text().len() > 50000` — and the CST is lossless, so it echoes the input
//! back whether or not a single token parsed. Not one of the eight called
//! `errors()` or `ok()`. `parse_annex_a_simple_vehicle` reported `ok` while the
//! SysML v2 specification's own Annex A model failed on line 3.
//!
//! `REQ-GUARD-GATE-EVIDENCE-002` names that shape exactly: an operation that
//! can produce "nothing happened" must render differently from "it worked".
//! Four releases applied it to the CI guardrails; none of it had been pointed
//! here.
//!
//! ## Verdicts are DECLARED, not discovered
//!
//! The obvious repair — assert whatever the parser currently does — would have
//! been just as inert: it would have recorded "these seven files fail" as the
//! desired state and stayed green forever. So each file carries an EXPECTED
//! verdict and the reason for it, the same discipline
//! `first_party_legality.rs` uses on the AADL side. Today every expectation is
//! `Fails`, and each names the construct responsible. When
//! `REQ-SYSML2-VISIBILITY-001` closes those gaps, these tests go red and say
//! which file changed — a fix cannot land silently.
//!
//! The bulk gate lives in `official_corpus.rs`, over 310 vendored files. This
//! file is the *named* companion: the corpus gate moves a number, this one
//! moves an identifiable model.

use spar_sysml2::parse;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// Parses clean today.
    ///
    /// Currently unconstructed, and `-D warnings` would otherwise reject the
    /// crate for it — allowed rather than deleted because this is the variant
    /// every entry below is headed for. Removing it would mean the first person
    /// to fix a grammar gap has to re-invent the vocabulary before they can
    /// record the win.
    #[allow(dead_code)]
    Parses,
    /// Fails today, for the stated reason. Not an aspiration — a record.
    Fails(&'static str),
}
use Expect::*;

/// The named files, their expected verdict, and why.
///
/// Measured 2026-08-26 against `f395518`. Five of the seven fail on the very
/// first construct in the file — a quoted package name at 1:9 — which is why
/// the corpus-wide count is 1 of 310 rather than something gentler.
const CASES: &[(&str, Expect)] = &[
    (
        "Package_Example.sysml",
        Fails("1:9 quoted name — `package 'Package Example' {`"),
    ),
    ("Part_Definition_Example.sysml", Fails("1:9 quoted name")),
    ("Parts_Example.sysml", Fails("1:9 quoted name")),
    ("Connections_Example.sysml", Fails("1:9 quoted name")),
    ("Port_Example.sysml", Fails("1:9 quoted name")),
    (
        "VehicleUsages.sysml",
        Fails("7:2 visibility modifier on a member"),
    ),
    (
        "SysML_v2_Spec_Annex_A_SimpleVehicleModel.sysml",
        Fails("3:5 `public import Definitions::*;` — visibility on import"),
    ),
];

fn repo_root() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    manifest
        .replace("crates/spar-sysml2", "")
        .replace("/crates/spar-sysml2", "")
        .trim_end_matches('/')
        .to_string()
}

fn read(name: &str) -> String {
    let path = format!("{}/test-data/sysml2/{}", repo_root(), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn every_named_file_gets_the_verdict_it_claims() {
    let mut wrong = Vec::new();
    for (name, expect) in CASES {
        let parsed = parse(&read(name));
        let got_ok = parsed.ok();
        let want_ok = matches!(expect, Parses);
        if got_ok != want_ok {
            let detail = match expect {
                Parses => "expected it to PARSE, and it does not".to_string(),
                Fails(why) => format!(
                    "expected it to FAIL ({why}) — it now PARSES. That is a fix: \
                     change this entry to Parses and raise OFFICIAL_PARSING in \
                     official_corpus.rs, which will also have moved."
                ),
            };
            wrong.push(format!("  {name}: {detail}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} named file(s) changed verdict:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Discrimination guard. If `parse().ok()` were stuck on one answer the test
/// above would still pass whenever the declared set happened to agree with it.
/// Both verdicts must be reachable for the declarations to carry information.
#[test]
fn the_verdict_function_can_return_both_answers() {
    assert!(parse("package P { part def A; }").ok());
    assert!(!parse("package P { part def").ok());
}

/// A file that fails to parse must still say WHY. A parser that rejects input
/// while emitting an empty diagnostic list is unusable as an oracle — the CLI
/// would exit non-zero with nothing printed, and a user could not act on it.
#[test]
fn a_rejected_file_carries_at_least_one_diagnostic() {
    for (name, expect) in CASES {
        let Fails(_) = expect else { continue };
        let parsed = parse(&read(name));
        assert!(
            !parsed.errors().is_empty(),
            "{name} is rejected but produced no diagnostic — a refusal a user \
             cannot act on is no better than a silent acceptance"
        );
    }
}

/// The one assertion in the original suite that was real, kept verbatim in
/// intent: a lossless CST must reproduce its input byte for byte. This holds
/// whether or not the parse succeeded, which is exactly the property a lossless
/// syntax tree is for — and it is why the *other* seven tests could pass on
/// files that never parsed.
#[test]
fn the_cst_is_lossless_even_when_the_parse_fails() {
    for (name, _) in CASES {
        let source = read(name);
        let roundtrip = parse(&source).syntax_node().text().to_string();
        assert_eq!(source, roundtrip, "lossless roundtrip failed for {name}");
    }
}
