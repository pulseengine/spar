//! Fixture-based RTA integration tests.
//!
//! Each fixture is an `.aadl` file in `tests/fixtures/rta/`. For each
//! fixture we parse it with spar-hir-def, instantiate the root system,
//! run RTA, sort the resulting diagnostic messages, and compare them
//! against the committed `.expected.json` (a JSON array of strings).
//!
//! Traceability (Rivet): REQ-TIMING-IRQ-001/002/003 — hierarchical
//! IRQ-aware RTA, jitter, BCET/WCET.

use std::path::PathBuf;

use spar_analysis::{Analysis, AnalysisDiagnostic, rta::RtaAnalysis};
use spar_hir_def::{Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope};

/// Run the RTA pass on `aadl_src` with the given root system. Returns
/// the sorted list of diagnostic messages.
fn run_rta_sorted(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> Vec<String> {
    // Minimal salsa DB via HirDefDatabase (defined at the crate root
    // of spar-hir-def as `pub struct HirDefDatabase`).
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

    // Debug: dump components and properties.
    if std::env::var("SPAR_FIXTURE_DEBUG").is_ok() {
        let mut buf = String::new();
        use std::fmt::Write;
        for (_idx, comp) in inst.all_components() {
            let props = inst.properties_for(_idx);
            writeln!(
                buf,
                "  {:?} {}: {} properties",
                comp.category,
                comp.name.as_str(),
                props.len()
            )
            .unwrap();
            for ((set, name), values) in props.iter() {
                for v in values {
                    writeln!(buf, "    {:?}::{:?} = {}", set, name, v.value).unwrap();
                }
            }
        }
        // Print via panic to ensure it's visible even without --nocapture.
        panic!("FIXTURE DEBUG:\n{}", buf);
    }

    let diags: Vec<AnalysisDiagnostic> = RtaAnalysis.analyze(&inst);
    let mut msgs: Vec<String> = diags.into_iter().map(|d| d.message).collect();
    msgs.sort();
    msgs
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rta")
}

fn check_fixture(aadl_name: &str, pkg: &str, sys: &str, sys_impl: &str) {
    let dir = fixtures_dir();
    let aadl_path = dir.join(aadl_name);
    let expected_path = aadl_path.with_extension("expected.json");

    let src =
        std::fs::read_to_string(&aadl_path).unwrap_or_else(|e| panic!("read {aadl_path:?}: {e}"));
    let actual = run_rta_sorted(&src, pkg, sys, sys_impl);

    // `SPAR_FIXTURE_UPDATE=1 cargo test …` regenerates the expected
    // JSON from the current output. Useful when the diagnostic
    // wording intentionally changes; otherwise the snapshot is the
    // authority.
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
fn fixture_irq_brake_handler() {
    check_fixture("irq_brake_handler.aadl", "BrakeIrq", "BrakeSys", "impl");
}

#[test]
fn fixture_multi_isr_same_cpu() {
    check_fixture("multi_isr_same_cpu.aadl", "MultiIsr", "Sys", "impl");
}

#[test]
fn fixture_jittered_chain() {
    check_fixture("jittered_chain.aadl", "Jittered", "Sys", "impl");
}
