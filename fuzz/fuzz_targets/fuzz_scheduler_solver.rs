#![no_main]
//! Fuzz target: adversarial task sets for the MILP scheduler/allocator.
//!
//! We derive a small `TaskSet` via `arbitrary::Arbitrary` (≤8 tasks, ≤4
//! processors, small integers for period/wcet/priority), build a
//! `ModelConstraints` the same way the unit tests do (dummy arena idx), and
//! call `solve_milp`. The contract is: the call must return `Ok` or `Err` —
//! it must never panic, and the outer libfuzzer `-timeout` backstop catches
//! hangs.
//!
//! Traceability: REQ-SOLVER-001, REQ-SOLVER-003, REQ-SOLVER-005.

use arbitrary::Arbitrary;
use la_arena::Arena;
use libfuzzer_sys::fuzz_target;

use spar_hir_def::instance::{ComponentInstance, ComponentInstanceIdx};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::Name;
use spar_solver::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};
use spar_solver::milp::solve_milp;

/// Bounded task description. Small integer ranges keep the MILP search space
/// tractable inside a 10-second libfuzzer slice.
#[derive(Arbitrary, Debug)]
struct Task {
    /// Period in picoseconds — clamped to [0, 65535] via `u16` at the wire.
    period: u16,
    /// WCET in picoseconds — clamped to [0, 65535] via `u16` at the wire.
    wcet: u16,
    /// Optional deadline; if None, defaults to period.
    deadline: Option<u16>,
    /// Optional priority (not read by solve_milp but exercises the struct path).
    priority: Option<u8>,
    /// Optional existing binding: if Some(n), bind to processor index n % len.
    bind_to: Option<u8>,
}

/// Bounded processor description.
#[derive(Arbitrary, Debug)]
struct Processor {
    memory_bytes: Option<u32>,
}

#[derive(Arbitrary, Debug)]
struct TaskSet {
    tasks: Vec<Task>,
    processors: Vec<Processor>,
}

/// Mint a throwaway `ComponentInstanceIdx` via a local arena. The solver
/// reads `name`, `period_ps`, `wcet_ps`, `deadline_ps`, `current_binding`,
/// `priority` — but never dereferences `idx` — so a dummy index is safe.
fn dummy_idx() -> ComponentInstanceIdx {
    let mut arena: Arena<ComponentInstance> = Arena::new();
    arena.alloc(ComponentInstance {
        name: Name::new("dummy"),
        category: ComponentCategory::Thread,
        type_name: Name::new("Dummy"),
        impl_name: None,
        package: Name::new("Test"),
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

fuzz_target!(|input: TaskSet| {
    // Cap sizes. The Arbitrary Vecs are already short in practice but we
    // enforce explicit bounds so the time budget per iteration stays small.
    let n_tasks = input.tasks.len().min(8);
    let n_procs = input.processors.len().min(4);

    let processors: Vec<ProcessorConstraint> = (0..n_procs)
        .map(|j| ProcessorConstraint {
            idx: dummy_idx(),
            name: format!("cpu{j}"),
            memory_bytes: input.processors[j].memory_bytes.map(u64::from),
        })
        .collect();

    let threads: Vec<ThreadConstraint> = (0..n_tasks)
        .map(|i| {
            let t = &input.tasks[i];
            let period_ps = u64::from(t.period);
            let wcet_ps = u64::from(t.wcet);
            let deadline_ps = t.deadline.map(u64::from).unwrap_or(period_ps);
            let current_binding = t.bind_to.and_then(|b| {
                if processors.is_empty() {
                    None
                } else {
                    let j = (b as usize) % processors.len();
                    Some(processors[j].name.clone())
                }
            });
            ThreadConstraint {
                idx: dummy_idx(),
                name: format!("t{i}"),
                period_ps,
                wcet_ps,
                deadline_ps,
                current_binding,
                priority: t.priority.map(u64::from),
            }
        })
        .collect();

    let constraints = ModelConstraints {
        threads,
        processors,
        warnings: Vec::new(),
    };

    // The only invariant we check here is: no panic. Both Ok and Err are
    // acceptable outcomes — infeasible task sets are legitimate inputs.
    let _ = solve_milp(&constraints);
});
