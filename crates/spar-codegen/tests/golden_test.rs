//! Golden model integration test for spar-codegen.
//!
//! Builds a `SystemInstance` from a realistic AADL model (BuildingControl)
//! and runs the full `generate()` pipeline, verifying that all 7 modules
//! produce the expected output.

use spar_codegen::{CodegenConfig, CodegenOutput, OutputFormat, VerifyMode, generate};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

/// Build a SystemInstance from inline AADL text.
fn build_instance(aadl: &str, pkg: &str, typ: &str, imp: &str) -> SystemInstance {
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), aadl.to_string());
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    SystemInstance::instantiate(&scope, &Name::new(pkg), &Name::new(typ), &Name::new(imp))
}

/// Load the golden AADL model from test-data.
fn golden_instance() -> SystemInstance {
    let aadl = include_str!("../../../test-data/codegen/building_control.aadl");
    build_instance(aadl, "BuildingControl", "BuildingSystem", "impl")
}

// ── Sanity check: the instance is well-formed ──────────────────────

#[test]
fn golden_instance_has_expected_hierarchy() {
    let inst = golden_instance();

    // Root is a system
    let root = inst.component(inst.root);
    assert_eq!(root.category, ComponentCategory::System);

    // Has a processor subcomponent
    let processors: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Processor)
        .collect();
    assert!(!processors.is_empty(), "Should have at least one processor");

    // Has a process subcomponent
    let processes: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Process)
        .collect();
    assert!(!processes.is_empty(), "Should have at least one process");

    // Has a thread subcomponent (inside the process)
    let threads: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Thread)
        .collect();
    assert!(!threads.is_empty(), "Should have at least one thread");
}

// ── Full generation: all modules produce output ────────────────────

fn generate_all() -> CodegenOutput {
    let inst = golden_instance();
    let config = CodegenConfig {
        root_name: "building_system".into(),
        output_dir: "output".into(),
        format: OutputFormat::Both,
        verify: Some(VerifyMode::All),
        rivet: true,
        dry_run: true,
    };
    generate(&inst, &config)
}

#[test]
fn golden_model_generates_nonempty_output() {
    let output = generate_all();
    assert!(
        !output.files.is_empty(),
        "generate() should produce at least one file"
    );
}

// ── WIT generation ─────────────────────────────────────────────────

#[test]
fn golden_model_generates_wit() {
    let output = generate_all();
    let wit_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.ends_with(".wit"))
        .collect();
    assert!(
        !wit_files.is_empty(),
        "Should generate at least one .wit file"
    );

    let wit = &wit_files[0];
    assert!(
        wit.content.contains("package"),
        "WIT should contain a package declaration"
    );
    assert!(
        wit.content.contains("world") || wit.content.contains("interface"),
        "WIT should have world or interface"
    );
}

// ── Rust generation ────────────────────────────────────────────────

#[test]
fn golden_model_generates_rust_component() {
    let output = generate_all();
    let rust_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("src/") && f.path.ends_with(".rs"))
        .collect();
    assert!(
        !rust_files.is_empty(),
        "Should generate at least one Rust component file under src/"
    );

    // Should reference the thread name or struct
    let any_has_thread_ref = rust_files.iter().any(|f| {
        f.content.contains("ControlLoop")
            || f.content.contains("controlloop")
            || f.content.contains("ctrl")
            || f.content.contains("Ctrl")
    });
    assert!(
        any_has_thread_ref,
        "Rust component should reference the thread (ControlLoop or ctrl)"
    );
}

// ── Config generation ──────────────────────────────────────────────

#[test]
fn golden_model_generates_config() {
    let output = generate_all();
    let config_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("config/") && f.path.ends_with(".toml"))
        .collect();
    assert!(
        !config_files.is_empty(),
        "Should generate at least one TOML config file"
    );

    let config = &config_files[0];
    assert!(
        config.content.contains("[process]"),
        "Config should have [process] section"
    );
    assert!(
        config.content.contains("[[threads]]"),
        "Config should have [[threads]] section"
    );
}

// ── Test harness generation ────────────────────────────────────────

#[test]
fn golden_model_generates_tests() {
    let output = generate_all();
    let test_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("tests/") && f.path.ends_with("_test.rs"))
        .collect();
    assert!(
        !test_files.is_empty(),
        "Should generate at least one test harness"
    );

    let test = &test_files[0];
    assert!(
        test.content.contains("#[test]"),
        "Test harness should contain #[test] attributes"
    );
    assert!(
        test.content.contains("_initializes"),
        "Test harness should have initialization test"
    );
    assert!(
        test.content.contains("_compute_dispatches"),
        "Test harness should have dispatch test"
    );
}

// ── Proof generation ───────────────────────────────────────────────

#[test]
fn golden_model_generates_lean4_proof() {
    let output = generate_all();
    let lean_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.ends_with(".lean"))
        .collect();
    assert!(
        !lean_files.is_empty(),
        "Should generate at least one Lean4 proof"
    );

    let lean = &lean_files[0];
    assert!(
        lean.content.contains("theorem"),
        "Lean4 proof should contain 'theorem'"
    );
    assert!(
        lean.content.contains("compute_response_time"),
        "Lean4 proof should reference RTA"
    );
}

#[test]
fn golden_model_generates_kani_harness() {
    let output = generate_all();
    let kani_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.contains("kani") && f.path.ends_with("_harness.rs"))
        .collect();
    assert!(
        !kani_files.is_empty(),
        "Should generate at least one Kani harness"
    );

    let kani = &kani_files[0];
    assert!(
        kani.content.contains("#[kani::proof]"),
        "Kani harness should contain proof attribute"
    );
    assert!(
        kani.content.contains("DEADLINE_PS"),
        "Kani harness should reference deadline"
    );
}

// ── Doc generation ─────────────────────────────────────────────────

#[test]
fn golden_model_generates_design_doc() {
    let output = generate_all();
    let doc_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("docs/design/") && f.path.ends_with(".md"))
        .collect();
    assert!(
        !doc_files.is_empty(),
        "Should generate at least one design document"
    );

    let doc = &doc_files[0];
    assert!(
        doc.content.starts_with("---\n"),
        "Design doc should start with YAML frontmatter"
    );
    assert!(
        doc.content.contains("type: design-decision"),
        "Design doc should have artifact type"
    );
    assert!(
        doc.content.contains("## Threads"),
        "Design doc should have threads section"
    );
}

#[test]
fn golden_model_generates_verification_yaml() {
    let output = generate_all();
    let verify_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("verification/") && f.path.ends_with(".yaml"))
        .collect();
    assert!(
        !verify_files.is_empty(),
        "Should generate at least one verification record"
    );

    let verify = &verify_files[0];
    assert!(
        verify.content.contains("type: verification-verdict"),
        "Should have verdict records"
    );
    assert!(
        verify.content.contains("status: pass"),
        "Verdicts should have pass status"
    );
}

// ── Workspace generation ───────────────────────────────────────────

#[test]
fn golden_model_generates_workspace_cargo_toml() {
    let output = generate_all();
    let cargo_toml = output.files.iter().find(|f| f.path == "Cargo.toml");
    assert!(cargo_toml.is_some(), "Should generate root Cargo.toml");

    let cargo = cargo_toml.unwrap();
    assert!(
        cargo.content.contains("[workspace]"),
        "Cargo.toml should have [workspace] section"
    );
    assert!(
        cargo.content.contains("members = ["),
        "Cargo.toml should have members list"
    );
}

#[test]
fn golden_model_generates_crate_lib_rs() {
    let output = generate_all();
    let lib_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("crates/") && f.path.ends_with("lib.rs"))
        .collect();
    assert!(
        !lib_files.is_empty(),
        "Should generate at least one crate lib.rs"
    );
}

#[test]
fn golden_model_generates_build_bazel() {
    let output = generate_all();
    let bazel_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.ends_with("BUILD.bazel"))
        .collect();
    assert!(
        !bazel_files.is_empty(),
        "Should generate at least one BUILD.bazel"
    );
}

// ── Cross-module consistency ───────────────────────────────────────

#[test]
fn golden_model_all_modules_produce_output() {
    let output = generate_all();

    // Count files per category
    let wit_count = output
        .files
        .iter()
        .filter(|f| f.path.ends_with(".wit"))
        .count();
    let rust_count = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("src/") && f.path.ends_with(".rs"))
        .count();
    let config_count = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("config/") && f.path.ends_with(".toml"))
        .count();
    let test_count = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("tests/") && f.path.ends_with("_test.rs"))
        .count();
    let lean_count = output
        .files
        .iter()
        .filter(|f| f.path.ends_with(".lean"))
        .count();
    let kani_count = output
        .files
        .iter()
        .filter(|f| f.path.contains("kani") && f.path.ends_with("_harness.rs"))
        .count();
    let doc_count = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("docs/design/") && f.path.ends_with(".md"))
        .count();
    let verify_count = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("verification/") && f.path.ends_with(".yaml"))
        .count();
    let workspace_count = output
        .files
        .iter()
        .filter(|f| f.path == "Cargo.toml" || f.path == "BUILD.bazel")
        .count();

    assert!(
        wit_count >= 1,
        "wit_gen: expected >= 1 file, got {wit_count}"
    );
    assert!(
        rust_count >= 1,
        "rust_gen: expected >= 1 file, got {rust_count}"
    );
    assert!(
        config_count >= 1,
        "config_gen: expected >= 1 file, got {config_count}"
    );
    assert!(
        test_count >= 1,
        "test_gen: expected >= 1 file, got {test_count}"
    );
    assert!(
        lean_count >= 1,
        "proof_gen (lean): expected >= 1 file, got {lean_count}"
    );
    assert!(
        kani_count >= 1,
        "proof_gen (kani): expected >= 1 file, got {kani_count}"
    );
    assert!(
        doc_count >= 1,
        "doc_gen: expected >= 1 file, got {doc_count}"
    );
    assert!(
        verify_count >= 1,
        "doc_gen (verify): expected >= 1 file, got {verify_count}"
    );
    assert!(
        workspace_count >= 2,
        "workspace_gen: expected >= 2 files, got {workspace_count}"
    );

    // Print summary for debugging
    eprintln!("Golden model generation summary:");
    eprintln!("  WIT files:          {wit_count}");
    eprintln!("  Rust files:         {rust_count}");
    eprintln!("  Config files:       {config_count}");
    eprintln!("  Test files:         {test_count}");
    eprintln!("  Lean4 proofs:       {lean_count}");
    eprintln!("  Kani harnesses:     {kani_count}");
    eprintln!("  Design docs:        {doc_count}");
    eprintln!("  Verification YAML:  {verify_count}");
    eprintln!("  Workspace files:    {workspace_count}");
    eprintln!("  Total files:        {}", output.files.len());
}

// ── Timing property extraction ─────────────────────────────────────

#[test]
fn golden_model_timing_properties_in_rust() {
    let output = generate_all();
    let rust_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("src/") && f.path.ends_with(".rs"))
        .collect();

    let has_period = rust_files.iter().any(|f| f.content.contains("PERIOD_PS"));
    let has_deadline = rust_files.iter().any(|f| f.content.contains("DEADLINE_PS"));
    let has_wcet = rust_files.iter().any(|f| f.content.contains("WCET_PS"));

    assert!(has_period, "Rust component should have PERIOD_PS constant");
    assert!(
        has_deadline,
        "Rust component should have DEADLINE_PS constant"
    );
    assert!(has_wcet, "Rust component should have WCET_PS constant");
}

#[test]
fn golden_model_timing_properties_in_config() {
    let output = generate_all();
    let config_files: Vec<_> = output
        .files
        .iter()
        .filter(|f| f.path.starts_with("config/") && f.path.ends_with(".toml"))
        .collect();

    // At least one config should mention period or dispatch
    let has_dispatch = config_files.iter().any(|f| f.content.contains("dispatch"));
    assert!(
        has_dispatch,
        "Config should have dispatch protocol information"
    );
}

// ── File path uniqueness ───────────────────────────────────────────

#[test]
fn golden_model_no_duplicate_paths() {
    let output = generate_all();
    let mut paths: Vec<&str> = output.files.iter().map(|f| f.path.as_str()).collect();
    let original_count = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(
        paths.len(),
        original_count,
        "No two generated files should have the same path"
    );
}

// ── Write to temp directory (verify files are valid) ───────────────

#[test]
fn golden_model_write_to_temp_dir() {
    let output = generate_all();

    let tmp = std::env::temp_dir().join("spar-codegen-golden-test");
    // Clean up from any prior run
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    for file in &output.files {
        let path = tmp.join(&file.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &file.content).unwrap();
    }

    // Verify key files exist on disk
    assert!(tmp.join("Cargo.toml").exists(), "Cargo.toml should exist");
    assert!(tmp.join("BUILD.bazel").exists(), "BUILD.bazel should exist");

    // Verify the wit directory has files
    let wit_dir = tmp.join("wit");
    if wit_dir.exists() {
        let wit_entries: Vec<_> = std::fs::read_dir(&wit_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!wit_entries.is_empty(), "wit/ should have files");
    }

    // Verify the config directory has files
    let config_dir = tmp.join("config");
    if config_dir.exists() {
        let config_entries: Vec<_> = std::fs::read_dir(&config_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!config_entries.is_empty(), "config/ should have files");
    }

    // Clean up
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Non-empty content ──────────────────────────────────────────────

#[test]
fn golden_model_all_files_have_content() {
    let output = generate_all();
    for file in &output.files {
        assert!(
            !file.content.is_empty(),
            "File '{}' should not be empty",
            file.path
        );
    }
}
