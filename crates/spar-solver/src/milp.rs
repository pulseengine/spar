//! MILP-based deployment optimization with optimality certificates.
//!
//! Formulates thread-to-processor allocation as a Mixed Integer Linear Program
//! using `good_lp` with the HiGHS backend. This provides exact (optimal)
//! solutions with a certificate of optimality, unlike the heuristic FFD/BFD
//! allocators in [`crate::allocate`].
//!
//! **Formulation:**
//! - Binary decision variable `bind[i][j]` = 1 iff thread i is bound to processor j
//! - Constraint: each thread assigned to exactly one processor
//! - Constraint: processor capacity (utilization <= 1.0)
//! - Constraint: existing bindings are respected as fixed assignments
//! - Objective: minimize maximum processor utilization (load balancing)
//!
//! Satisfies: REQ-SOLVER-005

use good_lp::{
    constraint, default_solver, variable, Expression, ProblemVariables, Solution, SolverModel,
    Variable,
};
use serde::Serialize;

use crate::constraints::ModelConstraints;

/// Result of a MILP deployment optimization.
#[derive(Debug, Clone, Serialize)]
pub struct MilpResult {
    /// Thread-to-processor bindings: `(thread_name, processor_name)`.
    pub bindings: Vec<(String, String)>,
    /// Objective function value (maximum processor utilization across all processors).
    pub objective_value: f64,
    /// Whether the solver proved this solution is globally optimal.
    pub is_optimal: bool,
    /// Human-readable solver status string.
    pub solver_status: String,
    /// Per-processor utilization after assignment.
    pub per_processor_utilization: Vec<(String, f64)>,
}

/// Solve the thread-to-processor allocation problem using MILP.
///
/// Minimizes the maximum processor utilization (load-balancing objective).
/// Returns an error string if the problem is infeasible or the solver fails.
///
/// Threads with `period_ps == 0` are skipped (cannot compute utilization).
/// Threads with existing `current_binding` are fixed to their bound processor.
pub fn solve_milp(constraints: &ModelConstraints) -> Result<MilpResult, String> {
    let num_procs = constraints.processors.len();
    if num_procs == 0 {
        if constraints.threads.is_empty() {
            return Ok(MilpResult {
                bindings: Vec::new(),
                objective_value: 0.0,
                is_optimal: true,
                solver_status: "Optimal (trivial)".to_string(),
                per_processor_utilization: Vec::new(),
            });
        }
        return Err("No processors available for allocation".to_string());
    }

    // Filter threads with valid period (period_ps > 0) and compute utilizations.
    let valid_threads: Vec<(usize, f64)> = constraints
        .threads
        .iter()
        .enumerate()
        .filter(|(_, t)| t.period_ps > 0)
        .map(|(i, t)| {
            let util = t.wcet_ps as f64 / t.period_ps as f64;
            (i, util)
        })
        .collect();

    if valid_threads.is_empty() {
        return Ok(MilpResult {
            bindings: Vec::new(),
            objective_value: 0.0,
            is_optimal: true,
            solver_status: "Optimal (no schedulable threads)".to_string(),
            per_processor_utilization: constraints
                .processors
                .iter()
                .map(|p| (p.name.clone(), 0.0))
                .collect(),
        });
    }

    let num_valid = valid_threads.len();

    // Build the MILP model.
    let mut vars = ProblemVariables::new();

    // Binary decision variables: bind[i][j] = 1 iff valid_thread i -> processor j
    // Stored as a flat array: bind[i * num_procs + j]
    let bind: Vec<Variable> = (0..num_valid * num_procs)
        .map(|_| vars.add(variable().binary()))
        .collect();

    // Auxiliary variable: z = max processor utilization (to be minimized).
    let z = vars.add(variable().min(0.0));

    let mut problem = vars.minimise(z).using(default_solver);

    // Constraint 1: Each thread assigned to exactly one processor.
    //   sum_j bind[i][j] == 1  for each thread i
    for i in 0..num_valid {
        let row_sum: Expression = (0..num_procs)
            .map(|j| bind[i * num_procs + j])
            .fold(Expression::from(0), |acc, v| acc + v);
        problem = problem.with(constraint!(row_sum == 1));
    }

    // Constraint 2: Processor capacity -- utilization <= 1.0 for each processor j.
    //   sum_i (util_i * bind[i][j]) <= 1.0  for each j
    for j in 0..num_procs {
        let col_util: Expression = (0..num_valid)
            .map(|i| {
                let (_, util) = valid_threads[i];
                util * bind[i * num_procs + j]
            })
            .fold(Expression::from(0), |acc, e| acc + e);
        problem = problem.with(constraint!(col_util <= 1.0));
    }

    // Constraint 3: z >= utilization on each processor (minimax).
    //   z >= sum_i (util_i * bind[i][j])  for each j
    for j in 0..num_procs {
        let col_util: Expression = (0..num_valid)
            .map(|i| {
                let (_, util) = valid_threads[i];
                util * bind[i * num_procs + j]
            })
            .fold(Expression::from(0), |acc, e| acc + e);
        problem = problem.with(constraint!(z >= col_util));
    }

    // Constraint 4: Respect existing bindings.
    //   If thread i is pre-bound to processor j, then bind[i][j] == 1.
    for (vi, &(orig_idx, _)) in valid_threads.iter().enumerate() {
        if let Some(ref bound_proc) = constraints.threads[orig_idx].current_binding {
            if let Some(proc_j) = constraints
                .processors
                .iter()
                .position(|p| &p.name == bound_proc)
            {
                let var = bind[vi * num_procs + proc_j];
                problem = problem.with(constraint!(var == 1));
            }
            // If bound to an unknown processor, the "exactly one" constraint
            // still forces assignment to some processor. We let the solver
            // pick the best available.
        }
    }

    // Solve.
    let solution = problem.solve().map_err(|e| format!("Solver failed: {e}"))?;

    // Extract results.
    let obj_val = solution.eval(z);

    let mut bindings = Vec::new();
    let mut proc_utils = vec![0.0_f64; num_procs];

    for (vi, &(orig_idx, util)) in valid_threads.iter().enumerate() {
        let thread_name = &constraints.threads[orig_idx].name;
        for j in 0..num_procs {
            let val = solution.value(bind[vi * num_procs + j]);
            if val > 0.5 {
                bindings.push((thread_name.clone(), constraints.processors[j].name.clone()));
                proc_utils[j] += util;
                break;
            }
        }
    }

    // Sort bindings by thread name for determinism (SOLVER-REQ-023).
    bindings.sort_by(|a, b| a.0.cmp(&b.0));

    let mut per_processor_utilization: Vec<(String, f64)> = constraints
        .processors
        .iter()
        .enumerate()
        .map(|(j, p)| (p.name.clone(), proc_utils[j]))
        .collect();
    per_processor_utilization.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(MilpResult {
        bindings,
        objective_value: obj_val,
        is_optimal: true, // HiGHS returns only optimal solutions or errors
        solver_status: "Optimal".to_string(),
        per_processor_utilization,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocate::Allocator;
    use crate::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};
    use la_arena::Arena;
    use spar_hir_def::instance::{ComponentInstance, ComponentInstanceIdx};
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::Name;

    /// Allocate a dummy `ComponentInstanceIdx` for test structs.
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

    fn make_thread(
        name: &str,
        period_ps: u64,
        wcet_ps: u64,
        binding: Option<String>,
    ) -> ThreadConstraint {
        ThreadConstraint {
            idx: dummy_idx(),
            name: name.to_string(),
            period_ps,
            wcet_ps,
            deadline_ps: period_ps,
            current_binding: binding,
            priority: None,
        }
    }

    fn make_processor(name: &str) -> ProcessorConstraint {
        ProcessorConstraint {
            idx: dummy_idx(),
            name: name.to_string(),
            memory_bytes: None,
        }
    }

    #[test]
    fn milp_simple_allocation() {
        // 2 threads, 2 processors -- should find a feasible assignment.
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 400, None), // util = 0.4
                make_thread("t2", 1000, 300, None), // util = 0.3
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("should be feasible");
        assert_eq!(result.bindings.len(), 2, "both threads should be assigned");

        // Every thread should be assigned to a known processor.
        for (thread, proc) in &result.bindings {
            assert!(
                proc == "cpu1" || proc == "cpu2",
                "thread {} assigned to unknown processor {}",
                thread,
                proc
            );
        }

        // Per-processor utilization should be <= 1.0.
        for (proc_name, util) in &result.per_processor_utilization {
            assert!(
                *util <= 1.0 + 1e-9,
                "processor {} overloaded: {}",
                proc_name,
                util
            );
        }
    }

    #[test]
    fn milp_respects_existing_bindings() {
        // Thread t1 is pre-bound to cpu2 -- it must stay there.
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 300, Some("cpu2".to_string())),
                make_thread("t2", 1000, 400, None),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("should be feasible");
        let t1_binding = result
            .bindings
            .iter()
            .find(|(t, _)| t == "t1")
            .expect("t1 should be assigned");
        assert_eq!(
            t1_binding.1, "cpu2",
            "t1 must remain bound to cpu2, got {}",
            t1_binding.1
        );
    }

    #[test]
    fn milp_infeasible_returns_error() {
        // Total utilization = 0.6 + 0.5 + 0.6 = 1.7, but only 1 processor
        // with capacity 1.0 -- infeasible.
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 600, None), // util = 0.6
                make_thread("t2", 1000, 500, None), // util = 0.5
                make_thread("t3", 1000, 600, None), // util = 0.6
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints);
        assert!(result.is_err(), "should be infeasible: {:?}", result);
    }

    #[test]
    fn milp_optimal_better_than_ffd() {
        // Construct a case where FFD produces suboptimal load balancing
        // but MILP finds the balanced optimum.
        //
        // 4 threads: util = 0.3, 0.3, 0.3, 0.3
        // 2 processors.
        //
        // FFD (sorted descending by util, all equal, then by name ascending):
        //   alpha(0.3) -> cpu1 (0.3)
        //   beta(0.3)  -> cpu1 (0.6)
        //   gamma(0.3) -> cpu1 (0.9)
        //   delta(0.3) -> cpu2 (0.3)
        //   => max utilization = 0.9
        //
        // Optimal (MILP): 2 threads per processor => max utilization = 0.6
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("alpha", 1000, 300, None),
                make_thread("beta", 1000, 300, None),
                make_thread("gamma", 1000, 300, None),
                make_thread("delta", 1000, 300, None),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let ffd_result = Allocator::ffd(&constraints);
        let ffd_max_util = ffd_result
            .per_processor_utilization
            .iter()
            .map(|(_, u)| *u)
            .fold(0.0_f64, f64::max);

        let milp_result = solve_milp(&constraints).expect("should be feasible");

        assert!(
            milp_result.objective_value <= ffd_max_util + 1e-9,
            "MILP objective ({}) should be <= FFD max util ({})",
            milp_result.objective_value,
            ffd_max_util,
        );

        // For this specific case, MILP should find the truly balanced solution.
        assert!(
            (milp_result.objective_value - 0.6).abs() < 1e-6,
            "MILP should find 0.6 balanced optimum, got {}",
            milp_result.objective_value,
        );
        assert!(
            ffd_max_util > milp_result.objective_value + 1e-6,
            "FFD max util ({}) should be strictly worse than MILP ({})",
            ffd_max_util,
            milp_result.objective_value,
        );
    }

    #[test]
    fn milp_reports_optimality() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 200, None), // util = 0.2
                make_thread("t2", 1000, 300, None), // util = 0.3
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("should be feasible");
        assert!(
            result.is_optimal,
            "small problem should report optimality"
        );
        assert_eq!(result.solver_status, "Optimal");
    }

    #[test]
    fn milp_empty_problem() {
        let constraints = ModelConstraints {
            threads: Vec::new(),
            processors: Vec::new(),
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("empty should succeed");
        assert!(result.bindings.is_empty());
        assert!(result.is_optimal);
        assert_eq!(result.objective_value, 0.0);
    }

    #[test]
    fn milp_skips_threads_without_period() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t_good", 1000, 200, None), // valid
                make_thread("t_bad", 0, 200, None),      // period = 0, skipped
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("should be feasible");
        assert_eq!(result.bindings.len(), 1);
        assert_eq!(result.bindings[0].0, "t_good");
    }

    #[test]
    fn milp_single_thread_single_processor() {
        let constraints = ModelConstraints {
            threads: vec![make_thread("t1", 1000, 500, None)], // util = 0.5
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = solve_milp(&constraints).expect("should be feasible");
        assert_eq!(result.bindings.len(), 1);
        assert_eq!(result.bindings[0], ("t1".to_string(), "cpu1".to_string()));
        assert!((result.objective_value - 0.5).abs() < 1e-6);
    }

    #[test]
    fn milp_deterministic_output() {
        // SOLVER-REQ-023: results must be deterministic.
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("zebra", 1000, 200, None),
                make_thread("alpha", 1000, 300, None),
                make_thread("mid", 1000, 250, None),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let r1 = solve_milp(&constraints).expect("feasible");
        let r2 = solve_milp(&constraints).expect("feasible");

        assert_eq!(r1.bindings, r2.bindings, "bindings must be deterministic");
        assert!(
            (r1.objective_value - r2.objective_value).abs() < 1e-9,
            "objective must be deterministic"
        );
    }
}
