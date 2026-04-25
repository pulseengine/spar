//! Criterion benchmarks for the scheduling/deployment solver.
//!
//! Covers the three public solver entrypoints exercised by the schedule
//! analysis flow:
//!
//! * [`spar_solver::milp::solve_milp`] — exact MILP allocator (HiGHS backend).
//! * [`spar_solver::nsga2::optimize`] — NSGA-II multi-objective Pareto front.
//! * [`spar_solver::allocate::Allocator::ffd`] / `bfd` — bin-packing heuristics.
//!
//! Workloads:
//! * `solver_small`      — 8 periodic tasks, total utilisation ≤ 0.69
//!   (safely below the RM n*(2^(1/n)-1) bound for n=8 ≈ 0.724).
//! * `solver_medium`     — 64 periodic tasks, same shape.
//! * `solver_large`      — 256 periodic tasks, `sample_size(10)` to keep
//!   wall-clock manageable under MILP.
//! * `solver_worst_case` — utilisation ≈ 0.95, many harmonic/near-harmonic
//!   periods so that the MILP capacity constraints are tight and every
//!   rate-monotonic priority-inversion boundary is exercised.
//!
//! Tracks: REQ-SOLVER-003, REQ-SOLVER-004, REQ-SOLVER-005, REQ-SOLVER-006.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use la_arena::Arena;
use spar_hir_def::instance::{ComponentInstance, ComponentInstanceIdx};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::Name;

use spar_solver::allocate::Allocator;
use spar_solver::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};
use spar_solver::milp::solve_milp;
use spar_solver::nsga2::{Nsga2Config, optimize as nsga2_optimize};

// ---------------------------------------------------------------------------
// Workload synthesis helpers
// ---------------------------------------------------------------------------

/// Allocate a throwaway `ComponentInstanceIdx`. The solver only uses the
/// idx for downstream rewriting; for benchmarks the id just needs to be
/// syntactically valid.
fn dummy_idx() -> ComponentInstanceIdx {
    let mut arena: Arena<ComponentInstance> = Arena::new();
    arena.alloc(ComponentInstance {
        name: Name::new("bench_dummy"),
        category: ComponentCategory::Thread,
        type_name: Name::new("BenchDummy"),
        impl_name: None,
        package: Name::new("Bench"),
        parent: None,
        children: Vec::new(),
        features: Vec::new(),
        connections: Vec::new(),
        flows: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        array_index: None,
        in_modes: Vec::new(),
    })
}

fn make_thread(name: String, period_ps: u64, wcet_ps: u64) -> ThreadConstraint {
    ThreadConstraint {
        idx: dummy_idx(),
        name,
        period_ps,
        wcet_ps,
        deadline_ps: period_ps,
        current_binding: None,
        priority: None,
    }
}

fn make_processor(name: String) -> ProcessorConstraint {
    ProcessorConstraint {
        idx: dummy_idx(),
        name,
        memory_bytes: None,
    }
}

/// Synthesise a "realistic" periodic task set of `n_tasks` tasks distributed
/// across `n_procs` processors, with aggregate utilisation capped at
/// `target_util` per processor (so the resulting MILP is feasible).
///
/// Periods are drawn from a small catalogue of typical avionics/automotive
/// rates (1 ms … 200 ms). WCETs are derived so that the sum of per-thread
/// utilisation stays under `target_util * n_procs`.
fn synth_periodic_taskset(n_tasks: usize, n_procs: usize, target_util: f64) -> ModelConstraints {
    // Mix of common periods in picoseconds. These intentionally span a wide
    // range so the RMA priority-inversion computation has non-trivial work.
    const PERIODS_PS: &[u64] = &[
        1_000_000_000,   // 1 ms  — high-rate control
        2_000_000_000,   // 2 ms
        5_000_000_000,   // 5 ms
        10_000_000_000,  // 10 ms — typical control loop
        20_000_000_000,  // 20 ms
        25_000_000_000,  // 25 ms
        50_000_000_000,  // 50 ms
        100_000_000_000, // 100 ms — sensor fusion
        200_000_000_000, // 200 ms — logging/telemetry
    ];

    // Target total utilisation across the fleet; divide equally so no
    // single task is degenerate (≥ 1.0).
    let total_util_budget = target_util * n_procs as f64;
    let per_task_util = total_util_budget / n_tasks as f64;

    let threads: Vec<ThreadConstraint> = (0..n_tasks)
        .map(|i| {
            let period = PERIODS_PS[i % PERIODS_PS.len()];
            // WCET = utilisation * period, floored to an integer picosecond.
            let wcet = (per_task_util * period as f64).max(1.0) as u64;
            make_thread(format!("task_{i:04}"), period, wcet)
        })
        .collect();

    let processors: Vec<ProcessorConstraint> = (0..n_procs)
        .map(|j| make_processor(format!("cpu_{j:02}")))
        .collect();

    ModelConstraints {
        threads,
        processors,
        warnings: Vec::new(),
    }
}

/// Construct a deliberately adversarial task set near the schedulability
/// boundary. Properties:
///
/// * Aggregate utilisation ≈ 0.95 × n_procs (just below the 1.0 capacity
///   bound, so bin-packing has very little slack and MILP must explore
///   many branches).
/// * Harmonic and near-harmonic period mix (1, 2, 5, 7, 11, 13 ms …) so
///   the rate-monotonic priority-inversion checks inside the schedulability
///   analysis fire on many pairs.
/// * Several "heavy" tasks (> 0.3 util each) forcing non-trivial packing.
///
/// This is the style of input that fuzzing tends to produce in the
/// scheduling corpus — use it as a perf canary.
fn synth_worst_case(n_tasks: usize, n_procs: usize) -> ModelConstraints {
    // Primes/near-primes in ms → picoseconds: maximise hyperperiod and
    // guarantee non-harmonic interactions between tasks.
    const ADVERSARIAL_PERIODS_PS: &[u64] = &[
        1_000_000_000,
        2_000_000_000,
        5_000_000_000,
        7_000_000_000,
        11_000_000_000,
        13_000_000_000,
        17_000_000_000,
        23_000_000_000,
        29_000_000_000,
    ];

    let target_util = 0.95_f64;
    let total_budget = target_util * n_procs as f64;

    // Split budget unevenly: 40% of tasks are "heavy" (each ~ 3× the
    // average), the rest share what's left. This forces the solver off
    // the trivial "spread evenly" path.
    let n_heavy = (n_tasks as f64 * 0.4).round() as usize;
    let n_light = n_tasks - n_heavy;

    let heavy_util_each = if n_heavy > 0 {
        (total_budget * 0.7) / n_heavy as f64
    } else {
        0.0
    };
    let light_util_each = if n_light > 0 {
        (total_budget * 0.3) / n_light as f64
    } else {
        0.0
    };

    let mut threads = Vec::with_capacity(n_tasks);

    for i in 0..n_heavy {
        let period = ADVERSARIAL_PERIODS_PS[i % ADVERSARIAL_PERIODS_PS.len()];
        // Clamp per-task util ≤ 0.95 so individual tasks remain feasible
        // on a single processor (else the instance is trivially infeasible).
        let util = heavy_util_each.min(0.95);
        let wcet = (util * period as f64).max(1.0) as u64;
        threads.push(make_thread(format!("heavy_{i:04}"), period, wcet));
    }

    for i in 0..n_light {
        let period = ADVERSARIAL_PERIODS_PS[(i + 3) % ADVERSARIAL_PERIODS_PS.len()];
        let util = light_util_each.clamp(0.01, 0.95);
        let wcet = (util * period as f64).max(1.0) as u64;
        threads.push(make_thread(format!("light_{i:04}"), period, wcet));
    }

    let processors: Vec<ProcessorConstraint> = (0..n_procs)
        .map(|j| make_processor(format!("cpu_{j:02}")))
        .collect();

    ModelConstraints {
        threads,
        processors,
        warnings: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Benchmark groups
// ---------------------------------------------------------------------------

fn bench_solver_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("solver_small");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(5));

    // 8 tasks, 2 processors, u ≈ 0.69 per CPU (below RM bound 0.724).
    let workload = synth_periodic_taskset(8, 2, 0.69);
    group.throughput(Throughput::Elements(workload.threads.len() as u64));

    group.bench_function(BenchmarkId::new("milp", 8), |b| {
        b.iter(|| {
            let result = solve_milp(black_box(&workload)).expect("feasible");
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("ffd", 8), |b| {
        b.iter(|| {
            let result = Allocator::ffd(black_box(&workload));
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("bfd", 8), |b| {
        b.iter(|| {
            let result = Allocator::bfd(black_box(&workload));
            black_box(result);
        });
    });

    // Small NSGA-II: tiny population to keep the small-bench fast.
    let nsga_cfg = Nsga2Config {
        population_size: 20,
        generations: 20,
        seed: 42,
        ..Default::default()
    };
    group.bench_function(BenchmarkId::new("nsga2", 8), |b| {
        b.iter(|| {
            let result = nsga2_optimize(black_box(&workload), black_box(&nsga_cfg));
            black_box(result);
        });
    });

    group.finish();
}

fn bench_solver_medium(c: &mut Criterion) {
    let mut group = c.benchmark_group("solver_medium");
    group
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(5));

    // 64 tasks, 8 processors: representative avionics/automotive workload.
    let workload = synth_periodic_taskset(64, 8, 0.69);
    group.throughput(Throughput::Elements(workload.threads.len() as u64));

    group.bench_function(BenchmarkId::new("milp", 64), |b| {
        b.iter(|| {
            let result = solve_milp(black_box(&workload)).expect("feasible");
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("ffd", 64), |b| {
        b.iter(|| {
            let result = Allocator::ffd(black_box(&workload));
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("bfd", 64), |b| {
        b.iter(|| {
            let result = Allocator::bfd(black_box(&workload));
            black_box(result);
        });
    });

    let nsga_cfg = Nsga2Config {
        population_size: 40,
        generations: 30,
        seed: 42,
        ..Default::default()
    };
    group.bench_function(BenchmarkId::new("nsga2", 64), |b| {
        b.iter(|| {
            let result = nsga2_optimize(black_box(&workload), black_box(&nsga_cfg));
            black_box(result);
        });
    });

    group.finish();
}

fn bench_solver_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("solver_large");
    // Large MILP runs are expensive; keep sample_size small to bound wall-time.
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(5));

    // 256 tasks, 32 processors: stress / regression detection.
    let workload = synth_periodic_taskset(256, 32, 0.69);
    group.throughput(Throughput::Elements(workload.threads.len() as u64));

    group.bench_function(BenchmarkId::new("milp", 256), |b| {
        b.iter(|| {
            let result = solve_milp(black_box(&workload)).expect("feasible");
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("ffd", 256), |b| {
        b.iter(|| {
            let result = Allocator::ffd(black_box(&workload));
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("bfd", 256), |b| {
        b.iter(|| {
            let result = Allocator::bfd(black_box(&workload));
            black_box(result);
        });
    });

    let nsga_cfg = Nsga2Config {
        population_size: 50,
        generations: 20,
        seed: 42,
        ..Default::default()
    };
    group.bench_function(BenchmarkId::new("nsga2", 256), |b| {
        b.iter(|| {
            let result = nsga2_optimize(black_box(&workload), black_box(&nsga_cfg));
            black_box(result);
        });
    });

    group.finish();
}

fn bench_solver_worst_case(c: &mut Criterion) {
    let mut group = c.benchmark_group("solver_worst_case");
    // Worst-case instances are slow under MILP; keep sample_size small.
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(5));

    // 64 tasks, 8 processors, u ≈ 0.95, prime periods → many RMA
    // priority-inversion checks.
    let workload = synth_worst_case(64, 8);
    group.throughput(Throughput::Elements(workload.threads.len() as u64));

    group.bench_function(BenchmarkId::new("milp", "worst_64"), |b| {
        b.iter(|| {
            // Worst-case inputs may be infeasible — treat the Result as data.
            let result = solve_milp(black_box(&workload));
            let _ = black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("ffd", "worst_64"), |b| {
        b.iter(|| {
            let result = Allocator::ffd(black_box(&workload));
            black_box(result);
        });
    });

    group.bench_function(BenchmarkId::new("bfd", "worst_64"), |b| {
        b.iter(|| {
            let result = Allocator::bfd(black_box(&workload));
            black_box(result);
        });
    });

    let nsga_cfg = Nsga2Config {
        population_size: 40,
        generations: 30,
        seed: 42,
        ..Default::default()
    };
    group.bench_function(BenchmarkId::new("nsga2", "worst_64"), |b| {
        b.iter(|| {
            let result = nsga2_optimize(black_box(&workload), black_box(&nsga_cfg));
            black_box(result);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_solver_small,
    bench_solver_medium,
    bench_solver_large,
    bench_solver_worst_case,
);
criterion_main!(benches);
