//! Criterion benchmarks for the codegen emit path.
//!
//! Builds a synthetic AADL model with 64 periodic threads (the scheduling
//! "medium" workload) and benches the `generate()` emit-to-string pipeline.
//! This is the slow outer loop any build/CI run hits after the solver
//! has decided on bindings — we want to catch regressions in string
//! formatting, property lookup, and workspace generation.
//!
//! Tracks: REQ-CODEGEN-* (see artifacts/verification.yaml).

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use spar_base_db::SourceFile;
use spar_codegen::{CodegenConfig, OutputFormat, VerifyMode, generate};
use spar_hir_def::HirDefDatabase;
use spar_hir_def::file_item_tree;
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

/// Render an AADL package string with `n_threads` periodic threads inside
/// a single process on a single processor. Threads have distinct names and
/// varied Period / Compute_Execution_Time so property lookup isn't cached
/// into a trivial constant.
fn synth_aadl(n_threads: usize) -> String {
    // A small catalogue of periods to cycle through — covers the typical
    // avionics/automotive range used in the solver benches.
    const PERIODS_MS: &[u32] = &[1, 2, 5, 10, 20, 50, 100, 200];

    let mut out = String::with_capacity(4096);
    out.push_str(
        "package BenchSystem\n\
         public\n\
           with Deployment_Properties;\n\
           with Timing_Properties;\n\n\
           data Msg\n\
           end Msg;\n\n\
           processor CPU\n\
           end CPU;\n\n",
    );

    // Emit N distinct thread types (one type per instance keeps the test
    // representative of heterogeneous deployments).
    for i in 0..n_threads {
        let period = PERIODS_MS[i % PERIODS_MS.len()];
        let wcet_low = period.max(1) / 2;
        let wcet_high = period.max(2).saturating_sub(1).max(wcet_low + 1);
        out.push_str(&format!(
            "  thread Worker_{i:04}\n    \
                properties\n      \
                  Timing_Properties::Period => {period} ms;\n      \
                  Timing_Properties::Deadline => {period} ms;\n      \
                  Timing_Properties::Compute_Execution_Time => {wcet_low} ms .. {wcet_high} ms;\n      \
                  Deployment_Properties::Dispatch_Protocol => Periodic;\n  \
             end Worker_{i:04};\n\n  \
             thread implementation Worker_{i:04}.impl\n  \
             end Worker_{i:04}.impl;\n\n",
        ));
    }

    // One process that contains all threads.
    out.push_str("  process Orchestrator\n  end Orchestrator;\n\n");
    out.push_str("  process implementation Orchestrator.impl\n    subcomponents\n");
    for i in 0..n_threads {
        out.push_str(&format!("      w_{i:04}: thread Worker_{i:04}.impl;\n"));
    }
    out.push_str("  end Orchestrator.impl;\n\n");

    // Top-level system.
    out.push_str(
        "  system BenchRoot\n  end BenchRoot;\n\n\
         system implementation BenchRoot.impl\n    \
           subcomponents\n      \
             cpu: processor CPU;\n      \
             proc: process Orchestrator.impl;\n  \
           end BenchRoot.impl;\n\n\
         end BenchSystem;\n",
    );

    out
}

/// Build a `SystemInstance` from an AADL source string.
fn build_instance(aadl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let sf = SourceFile::new(&db, "bench.aadl".to_string(), aadl.to_string());
    let tree = file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    SystemInstance::instantiate(
        &scope,
        &Name::new("BenchSystem"),
        &Name::new("BenchRoot"),
        &Name::new("impl"),
    )
}

fn bench_codegen_emit(c: &mut Criterion) {
    let mut group = c.benchmark_group("codegen_emit");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(5));

    let n_threads = 64;
    let aadl = synth_aadl(n_threads);
    let instance = build_instance(&aadl);
    let config = CodegenConfig {
        root_name: "bench_root".into(),
        output_dir: "output".into(),
        format: OutputFormat::Both,
        verify: Some(VerifyMode::All),
        rivet: true,
        dry_run: true,
    };

    group.throughput(Throughput::Elements(n_threads as u64));

    // End-to-end emit: Rust + WIT + config + tests + proofs + docs +
    // workspace, concatenated into in-memory strings.
    group.bench_function("generate_full_64", |b| {
        b.iter(|| {
            let output = generate(black_box(&instance), black_box(&config));
            black_box(output);
        });
    });

    // Rust-only emit: isolates the schedule-emitting hot path (per-thread
    // timing properties → Rust attributes) from WIT / Lean / Kani / docs.
    let rust_only_config = CodegenConfig {
        verify: None,
        rivet: false,
        format: OutputFormat::Rust,
        ..config.clone()
    };
    group.bench_function("generate_rust_only_64", |b| {
        b.iter(|| {
            let output = generate(black_box(&instance), black_box(&rust_only_config));
            black_box(output);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_codegen_emit);
criterion_main!(benches);
