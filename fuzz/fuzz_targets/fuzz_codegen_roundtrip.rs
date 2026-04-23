#![no_main]
//! Fuzz target: deterministic AADL → instance model → codegen roundtrip.
//!
//! Per issue #138, this target does *not* derive the `SystemInstance` from
//! arbitrary bytes (the HIR construction surface is too deep and most
//! inputs would be rejected upstream). Instead it feeds a known-good AADL
//! fixture through `SystemInstance::instantiate` and varies the
//! `CodegenConfig` flags from the fuzzer input. The contract: `generate()`
//! must not panic on any reachable configuration combination.
//!
//! Traceability: REQ-CODEGEN-001, REQ-CODEGEN-WIT, REQ-CODEGEN-RUST.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use spar_codegen::{CodegenConfig, OutputFormat, VerifyMode, generate};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

/// Seed AADL model — same file the golden codegen tests use. Compiled in
/// so the fuzz binary is hermetic.
const SEED_AADL: &str = include_str!("../../test-data/codegen/building_control.aadl");

#[derive(Arbitrary, Debug)]
struct Knobs {
    format_pick: u8,
    verify_pick: u8,
    rivet: bool,
    dry_run: bool,
}

fn build_seed_instance() -> SystemInstance {
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(
        &db,
        "fuzz.aadl".to_string(),
        SEED_AADL.to_string(),
    );
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    SystemInstance::instantiate(
        &scope,
        &Name::new("BuildingControl"),
        &Name::new("BuildingSystem"),
        &Name::new("impl"),
    )
}

fuzz_target!(|knobs: Knobs| {
    let inst = build_seed_instance();

    let format = match knobs.format_pick % 3 {
        0 => OutputFormat::Rust,
        1 => OutputFormat::Wit,
        _ => OutputFormat::Both,
    };

    let verify = match knobs.verify_pick % 5 {
        0 => None,
        1 => Some(VerifyMode::All),
        2 => Some(VerifyMode::Build),
        3 => Some(VerifyMode::Test),
        _ => Some(VerifyMode::Proof),
    };

    let config = CodegenConfig {
        root_name: "fuzz_root".to_string(),
        output_dir: "out".to_string(),
        format,
        verify,
        rivet: knobs.rivet,
        dry_run: knobs.dry_run,
    };

    // Contract: no panic on any config combination, even though inputs are
    // identical for the instance model.
    let out = generate(&inst, &config);
    // Touch every file path + content length so a latent panic in formatting
    // code would fire here rather than being dead-code-eliminated.
    for f in &out.files {
        std::hint::black_box((f.path.len(), f.content.len()));
    }
});
