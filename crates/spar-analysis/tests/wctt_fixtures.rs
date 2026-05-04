//! Fixture-based WCTT integration tests.
//!
//! Mirrors the structure of `rta_fixtures.rs`. Each fixture is an
//! `.aadl` file under `tests/fixtures/wctt/`. We parse it, instantiate
//! the named root system, run [`WcttAnalysis`], sort the resulting
//! diagnostic messages, and compare them against the committed
//! `.expected.json` snapshot. Set `SPAR_FIXTURE_UPDATE=1` to refresh
//! the snapshot if the diagnostic wording intentionally changes.
//!
//! Traceability (Rivet): REQ-NETWORK-{004,005,006} — the WCTT pass
//! consumes the Track D commit 2 graph extraction and commit 3 NC
//! primitives.

use std::path::PathBuf;

use spar_analysis::{Analysis, AnalysisDiagnostic, wctt::WcttAnalysis};
use spar_hir_def::{Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope};

fn run_wctt_sorted(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> Vec<String> {
    let db = spar_hir_def::HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(&db, "fixture.aadl".to_string(), aadl_src.to_string());
    let tree = file_item_tree(&db, file);
    let scope = GlobalScope::from_trees(vec![tree]);

    let inst = SystemInstance::instantiate(
        &scope,
        &Name::new(pkg),
        &Name::new(sys),
        &Name::new(sys_impl),
    );
    assert!(
        inst.component_count() > 0,
        "instantiation failed, diagnostics: {:?}",
        inst.diagnostics
    );

    let diags: Vec<AnalysisDiagnostic> = WcttAnalysis::default().analyze(&inst);
    let mut msgs: Vec<String> = diags.into_iter().map(|d| d.message).collect();
    msgs.sort();
    msgs
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wctt")
}

fn check_fixture(aadl_name: &str, pkg: &str, sys: &str, sys_impl: &str) {
    let dir = fixtures_dir();
    let aadl_path = dir.join(aadl_name);
    let expected_path = aadl_path.with_extension("expected.json");

    let src =
        std::fs::read_to_string(&aadl_path).unwrap_or_else(|e| panic!("read {aadl_path:?}: {e}"));
    let actual = run_wctt_sorted(&src, pkg, sys, sys_impl);

    if std::env::var("SPAR_FIXTURE_UPDATE").is_ok() {
        let pretty = serde_json::to_string_pretty(&actual).unwrap();
        std::fs::write(&expected_path, pretty).unwrap();
        return;
    }

    let expected_bytes = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        let pretty = serde_json::to_string_pretty(&actual).unwrap();
        panic!(
            "missing expected file: {expected_path:?}: {e}\n\
             Actual diagnostics for this fixture (commit this as \
             {expected_path:?} if correct):\n{pretty}"
        )
    });
    let expected: Vec<String> = serde_json::from_str(&expected_bytes)
        .unwrap_or_else(|e| panic!("parse {expected_path:?} as JSON array of strings: {e}"));

    if actual != expected {
        let actual_pretty = serde_json::to_string_pretty(&actual).unwrap();
        panic!(
            "fixture {} diagnostics mismatch.\nExpected ({}):\n{}\nActual ({}):\n{}",
            aadl_name,
            expected_path.display(),
            serde_json::to_string_pretty(&expected).unwrap(),
            aadl_path.display(),
            actual_pretty,
        );
    }
}

#[test]
fn fixture_classical_ethernet() {
    check_fixture("classical_ethernet.aadl", "WcttClassical", "Sys", "impl");
}
