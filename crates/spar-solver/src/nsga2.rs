//! NSGA-II multi-objective Pareto front computation for deployment optimization.
//!
//! Pure Rust, WASM-compatible implementation with no external dependencies
//! beyond the crate's existing set. Uses a simple LCG PRNG for deterministic
//! results from a given seed (SOLVER-REQ-023).
//!
//! Three default objectives for deployment optimization:
//! 1. **Minimize max processor utilization** (load balancing)
//! 2. **Minimize total communication cost** (co-locate communicating threads)
//! 3. **Maximize safety margin** (utilization headroom below 1.0)
//!
//! Satisfies: REQ-SOLVER-006

use crate::constraints::ModelConstraints;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A deployment solution (individual in the population).
#[derive(Debug, Clone)]
pub struct Solution {
    /// Thread index -> processor index assignment.
    pub assignment: Vec<usize>,
    /// Objective values (lower is better for all).
    pub objectives: Vec<f64>,
    /// Pareto front rank (0 = first front = non-dominated).
    pub rank: usize,
    /// Crowding distance (higher = more diverse).
    pub crowding_distance: f64,
}

/// Multi-objective optimization objectives.
#[derive(Debug, Clone)]
pub struct Objectives {
    pub names: Vec<String>,
}

/// NSGA-II configuration.
#[derive(Debug, Clone)]
pub struct Nsga2Config {
    /// Population size (default 100).
    pub population_size: usize,
    /// Number of generations (default 200).
    pub generations: usize,
    /// Crossover probability (default 0.9).
    pub crossover_rate: f64,
    /// Mutation probability per gene (default 0.1).
    pub mutation_rate: f64,
    /// PRNG seed for deterministic runs (SOLVER-REQ-023).
    pub seed: u64,
}

impl Default for Nsga2Config {
    fn default() -> Self {
        Self {
            population_size: 100,
            generations: 200,
            crossover_rate: 0.9,
            mutation_rate: 0.1,
            seed: 42,
        }
    }
}

/// Result of NSGA-II optimization.
#[derive(Debug, Clone)]
pub struct ParetoResult {
    /// Solutions on the first Pareto front (non-dominated).
    pub pareto_front: Vec<Solution>,
    /// Number of generations actually run.
    pub generations_run: usize,
}

// ---------------------------------------------------------------------------
// Simple LCG PRNG (deterministic, no external deps, WASM-safe)
// ---------------------------------------------------------------------------

/// Minimal linear congruential generator.
///
/// Constants from Numerical Recipes (Knuth MMIX): a = 6364136223846793005,
/// c = 1442695040888963407, m = 2^64 (implicit wrap).
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Return the next pseudo-random u64.
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    /// Return a uniform f64 in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Return a uniform usize in [0, bound).
    fn next_usize(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Objective evaluation
// ---------------------------------------------------------------------------

/// Compute per-processor utilization given an assignment.
///
/// Returns a vector of length `num_processors` with the total utilization
/// on each processor. Thread utilization = wcet_ps / period_ps.
fn processor_utilizations(
    constraints: &ModelConstraints,
    assignment: &[usize],
    num_processors: usize,
) -> Vec<f64> {
    let mut utils = vec![0.0_f64; num_processors];
    for (ti, &pi) in assignment.iter().enumerate() {
        let thread = &constraints.threads[ti];
        if thread.period_ps > 0 {
            utils[pi] += thread.wcet_ps as f64 / thread.period_ps as f64;
        }
    }
    utils
}

/// Evaluate the three default objectives for a deployment solution.
///
/// 1. Max processor utilization (lower is better — balanced load).
/// 2. Communication cost: number of thread pairs on different processors
///    that share a name prefix (heuristic for "communicate"). Lower is better.
/// 3. Negative safety margin: `max_util - 1.0` is negative when feasible;
///    lower (more negative) means more headroom. We use `max_util` directly
///    so that solutions closer to 0.0 are better.
fn evaluate(
    constraints: &ModelConstraints,
    assignment: &[usize],
    num_processors: usize,
) -> Vec<f64> {
    let utils = processor_utilizations(constraints, assignment, num_processors);

    // Objective 1: max processor utilization (lower = better balance).
    let max_util = utils.iter().cloned().fold(0.0_f64, f64::max);

    // Objective 2: communication cost heuristic.
    // Count pairs of threads assigned to different processors that share
    // a common parent prefix (same process). This is a simple proxy for
    // inter-processor communication.
    let mut comm_cost = 0.0_f64;
    let n_threads = constraints.threads.len();
    for i in 0..n_threads {
        for j in (i + 1)..n_threads {
            if assignment[i] != assignment[j] {
                let prefix_i = parent_prefix(&constraints.threads[i].name);
                let prefix_j = parent_prefix(&constraints.threads[j].name);
                if prefix_i == prefix_j && !prefix_i.is_empty() {
                    comm_cost += 1.0;
                }
            }
        }
    }

    // Objective 3: negative safety margin = max_util (lower means more
    // headroom below 1.0).
    let safety_cost = max_util;

    vec![max_util, comm_cost, safety_cost]
}

/// Extract the parent prefix of a dot-separated component path.
/// E.g. "root.proc.worker" -> "root.proc".
fn parent_prefix(name: &str) -> &str {
    match name.rfind('.') {
        Some(pos) => &name[..pos],
        None => "",
    }
}

// ---------------------------------------------------------------------------
// Feasibility check
// ---------------------------------------------------------------------------

/// A solution is feasible iff no processor utilization exceeds 1.0.
fn is_feasible(
    constraints: &ModelConstraints,
    assignment: &[usize],
    num_processors: usize,
) -> bool {
    let utils = processor_utilizations(constraints, assignment, num_processors);
    utils.iter().all(|&u| u <= 1.0)
}

// ---------------------------------------------------------------------------
// Non-dominated sorting
// ---------------------------------------------------------------------------

/// Returns true if solution `a` dominates solution `b`:
/// all objectives of `a` <= those of `b`, and at least one is strictly less.
fn dominates(a: &[f64], b: &[f64]) -> bool {
    let mut any_better = false;
    for (ai, bi) in a.iter().zip(b.iter()) {
        if ai > bi {
            return false;
        }
        if ai < bi {
            any_better = true;
        }
    }
    any_better
}

/// Non-dominated sorting: assign each solution to a Pareto front (rank).
///
/// O(M * N^2) where M = number of objectives, N = population size.
/// Fine for population sizes <= 200.
fn non_dominated_sort(population: &mut [Solution]) {
    let n = population.len();
    if n == 0 {
        return;
    }

    // domination_count[i] = how many solutions dominate solution i
    let mut domination_count = vec![0usize; n];
    // dominated_set[i] = indices of solutions that solution i dominates
    let mut dominated_set: Vec<Vec<usize>> = vec![Vec::new(); n];

    for i in 0..n {
        for j in (i + 1)..n {
            if dominates(&population[i].objectives, &population[j].objectives) {
                dominated_set[i].push(j);
                domination_count[j] += 1;
            } else if dominates(&population[j].objectives, &population[i].objectives) {
                dominated_set[j].push(i);
                domination_count[i] += 1;
            }
        }
    }

    // Build fronts iteratively.
    let mut current_front: Vec<usize> = (0..n).filter(|&i| domination_count[i] == 0).collect();

    let mut rank = 0;
    while !current_front.is_empty() {
        let mut next_front = Vec::new();
        for &i in &current_front {
            population[i].rank = rank;
            for &j in &dominated_set[i] {
                domination_count[j] -= 1;
                if domination_count[j] == 0 {
                    next_front.push(j);
                }
            }
        }
        rank += 1;
        current_front = next_front;
    }
}

// ---------------------------------------------------------------------------
// Crowding distance
// ---------------------------------------------------------------------------

/// Compute crowding distance for all solutions within each front.
///
/// Solutions at the boundary of each objective get infinite distance.
fn compute_crowding_distance(population: &mut [Solution]) {
    let n = population.len();
    if n == 0 {
        return;
    }

    // Reset crowding distances.
    for sol in population.iter_mut() {
        sol.crowding_distance = 0.0;
    }

    let num_objectives = population[0].objectives.len();
    let indices: Vec<usize> = (0..n).collect();

    for m in 0..num_objectives {
        // Sort indices by objective m.
        let mut sorted = indices.clone();
        sorted.sort_by(|&a, &b| {
            population[a].objectives[m]
                .partial_cmp(&population[b].objectives[m])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let obj_min = population[sorted[0]].objectives[m];
        let obj_max = population[sorted[n - 1]].objectives[m];
        let range = obj_max - obj_min;

        // Boundary solutions get infinite distance.
        population[sorted[0]].crowding_distance = f64::INFINITY;
        population[sorted[n - 1]].crowding_distance = f64::INFINITY;

        if range > 0.0 {
            for k in 1..(n - 1) {
                let prev_obj = population[sorted[k - 1]].objectives[m];
                let next_obj = population[sorted[k + 1]].objectives[m];
                population[sorted[k]].crowding_distance += (next_obj - prev_obj) / range;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Genetic operators
// ---------------------------------------------------------------------------

/// Binary tournament selection: pick two random individuals, return the
/// better one (lower rank, then higher crowding distance).
fn tournament_select(population: &[Solution], rng: &mut Lcg) -> usize {
    let a = rng.next_usize(population.len());
    let b = rng.next_usize(population.len());
    if crowded_comparison_better(&population[a], &population[b]) {
        a
    } else {
        b
    }
}

/// Crowded comparison operator: returns true if `a` is preferred over `b`.
fn crowded_comparison_better(a: &Solution, b: &Solution) -> bool {
    if a.rank < b.rank {
        return true;
    }
    if a.rank > b.rank {
        return false;
    }
    // Same rank — prefer higher crowding distance (more diverse).
    a.crowding_distance > b.crowding_distance
}

/// Uniform crossover: for each gene, swap with probability 0.5.
fn crossover(parent_a: &[usize], parent_b: &[usize], rng: &mut Lcg) -> Vec<usize> {
    parent_a
        .iter()
        .zip(parent_b.iter())
        .map(|(&a, &b)| if rng.next_f64() < 0.5 { a } else { b })
        .collect()
}

/// Mutation: randomly reassign one thread to a different processor.
fn mutate(assignment: &mut [usize], num_processors: usize, rng: &mut Lcg) {
    if assignment.is_empty() || num_processors <= 1 {
        return;
    }
    let gene = rng.next_usize(assignment.len());
    let old = assignment[gene];
    // Pick a different processor.
    let new_proc = loop {
        let p = rng.next_usize(num_processors);
        if p != old {
            break p;
        }
    };
    assignment[gene] = new_proc;
}

// ---------------------------------------------------------------------------
// Main NSGA-II loop
// ---------------------------------------------------------------------------

/// Run NSGA-II multi-objective optimization on the deployment problem.
///
/// The constraints must have at least one thread and one processor.
/// Returns the first Pareto front — the set of non-dominated deployment
/// solutions found after running the configured number of generations.
pub fn optimize(constraints: &ModelConstraints, config: &Nsga2Config) -> ParetoResult {
    let num_threads = constraints.threads.len();
    let num_processors = constraints.processors.len();

    // Degenerate cases.
    if num_threads == 0 || num_processors == 0 {
        return ParetoResult {
            pareto_front: Vec::new(),
            generations_run: 0,
        };
    }

    let mut rng = Lcg::new(config.seed);

    // --- Initialize population ---
    let pop_size = config.population_size;
    let mut population = Vec::with_capacity(pop_size);

    for _ in 0..pop_size {
        let assignment: Vec<usize> = (0..num_threads)
            .map(|_| rng.next_usize(num_processors))
            .collect();
        let objectives = evaluate(constraints, &assignment, num_processors);
        population.push(Solution {
            assignment,
            objectives,
            rank: 0,
            crowding_distance: 0.0,
        });
    }

    // --- Generational loop ---
    let mut generations_run = 0;

    for _ in 0..config.generations {
        // Sort + crowding on current population.
        non_dominated_sort(&mut population);
        compute_crowding_distance(&mut population);

        // Generate offspring.
        let mut offspring = Vec::with_capacity(pop_size);
        while offspring.len() < pop_size {
            let p1 = tournament_select(&population, &mut rng);
            let p2 = tournament_select(&population, &mut rng);

            let mut child_assignment = if rng.next_f64() < config.crossover_rate {
                crossover(
                    &population[p1].assignment,
                    &population[p2].assignment,
                    &mut rng,
                )
            } else {
                population[p1].assignment.clone()
            };

            if rng.next_f64() < config.mutation_rate {
                mutate(&mut child_assignment, num_processors, &mut rng);
            }

            let objectives = evaluate(constraints, &child_assignment, num_processors);
            offspring.push(Solution {
                assignment: child_assignment,
                objectives,
                rank: 0,
                crowding_distance: 0.0,
            });
        }

        // Combine parent + offspring.
        let mut combined = population;
        combined.extend(offspring);

        // Non-dominated sort and crowding on combined population.
        non_dominated_sort(&mut combined);
        compute_crowding_distance(&mut combined);

        // Select the best pop_size individuals by rank, then crowding distance.
        combined.sort_by(|a, b| {
            a.rank.cmp(&b.rank).then_with(|| {
                // Higher crowding distance is better — reverse order.
                b.crowding_distance
                    .partial_cmp(&a.crowding_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        combined.truncate(pop_size);

        population = combined;
        generations_run += 1;
    }

    // Final sort to extract the first Pareto front.
    non_dominated_sort(&mut population);

    let pareto_front: Vec<Solution> = population
        .into_iter()
        .filter(|s| s.rank == 0)
        .filter(|s| is_feasible(constraints, &s.assignment, num_processors))
        .collect();

    ParetoResult {
        pareto_front,
        generations_run,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{ModelConstraints, ProcessorConstraint, ThreadConstraint};
    use la_arena::Arena;
    use spar_hir_def::instance::ComponentInstance;
    use spar_hir_def::instance::ComponentInstanceIdx;
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

    fn make_thread(name: &str, period_ps: u64, wcet_ps: u64) -> ThreadConstraint {
        ThreadConstraint {
            idx: dummy_idx(),
            name: name.to_string(),
            period_ps,
            wcet_ps,
            deadline_ps: period_ps,
            current_binding: None,
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

    // ----- Test 1: Basic Pareto front from a simple problem -----

    #[test]
    fn nsga2_simple_pareto() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("proc.t1", 1000, 200), // 0.2
                make_thread("proc.t2", 1000, 200), // 0.2
                make_thread("proc.t3", 1000, 100), // 0.1
                make_thread("proc.t4", 1000, 100), // 0.1
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let config = Nsga2Config {
            population_size: 50,
            generations: 50,
            seed: 12345,
            ..Default::default()
        };

        let result = optimize(&constraints, &config);
        assert!(
            !result.pareto_front.is_empty(),
            "Pareto front must have at least 1 solution"
        );
        assert_eq!(result.generations_run, 50);

        // All solutions in the front should have 3 objectives.
        for sol in &result.pareto_front {
            assert_eq!(sol.objectives.len(), 3);
        }
    }

    // ----- Test 2: Single objective matches FFD-like behavior -----

    #[test]
    fn nsga2_single_objective() {
        // With a single very heavy thread and a light one, the only feasible
        // assignment is to spread them across processors (both fit on separate
        // CPUs).
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("proc.t1", 1000, 800), // 0.8
                make_thread("proc.t2", 1000, 800), // 0.8
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let config = Nsga2Config {
            population_size: 50,
            generations: 100,
            seed: 99,
            ..Default::default()
        };

        let result = optimize(&constraints, &config);
        assert!(
            !result.pareto_front.is_empty(),
            "should find at least one feasible Pareto solution"
        );

        // In all feasible Pareto solutions, each thread must be on a
        // different processor (0.8 + 0.8 = 1.6 > 1.0 on one CPU).
        for sol in &result.pareto_front {
            assert_ne!(
                sol.assignment[0], sol.assignment[1],
                "threads must be on different processors to be feasible"
            );
        }
    }

    // ----- Test 3: Deterministic with same seed (SOLVER-REQ-023) -----

    #[test]
    fn nsga2_deterministic_with_seed() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("proc.t1", 1000, 200),
                make_thread("proc.t2", 1000, 300),
                make_thread("proc.t3", 1000, 150),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let config = Nsga2Config {
            population_size: 40,
            generations: 30,
            seed: 42,
            ..Default::default()
        };

        let result1 = optimize(&constraints, &config);
        let result2 = optimize(&constraints, &config);

        assert_eq!(
            result1.pareto_front.len(),
            result2.pareto_front.len(),
            "same seed must produce same number of Pareto solutions"
        );
        assert_eq!(result1.generations_run, result2.generations_run);

        for (s1, s2) in result1.pareto_front.iter().zip(result2.pareto_front.iter()) {
            assert_eq!(
                s1.assignment, s2.assignment,
                "same seed must produce identical assignments"
            );
            for (o1, o2) in s1.objectives.iter().zip(s2.objectives.iter()) {
                assert!(
                    (o1 - o2).abs() < f64::EPSILON,
                    "same seed must produce identical objectives"
                );
            }
        }
    }

    // ----- Test 4: All Pareto solutions are feasible -----

    #[test]
    fn nsga2_respects_capacity() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("proc.t1", 1000, 400), // 0.4
                make_thread("proc.t2", 1000, 400), // 0.4
                make_thread("proc.t3", 1000, 400), // 0.4
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let config = Nsga2Config {
            population_size: 60,
            generations: 80,
            seed: 7,
            ..Default::default()
        };

        let result = optimize(&constraints, &config);

        for (i, sol) in result.pareto_front.iter().enumerate() {
            let utils =
                processor_utilizations(&constraints, &sol.assignment, constraints.processors.len());
            for (pi, &u) in utils.iter().enumerate() {
                assert!(
                    u <= 1.0,
                    "Pareto solution {} has processor {} utilization {:.4} > 1.0",
                    i,
                    pi,
                    u
                );
            }
        }
    }

    // ----- Test 5: Empty problem -----

    #[test]
    fn nsga2_empty_problem() {
        // No threads.
        let constraints_no_threads = ModelConstraints {
            threads: Vec::new(),
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };
        let result = optimize(&constraints_no_threads, &Nsga2Config::default());
        assert!(result.pareto_front.is_empty());
        assert_eq!(result.generations_run, 0);

        // No processors.
        let constraints_no_procs = ModelConstraints {
            threads: vec![make_thread("proc.t1", 1000, 200)],
            processors: Vec::new(),
            warnings: Vec::new(),
        };
        let result = optimize(&constraints_no_procs, &Nsga2Config::default());
        assert!(result.pareto_front.is_empty());
        assert_eq!(result.generations_run, 0);

        // Both empty.
        let constraints_empty = ModelConstraints {
            threads: Vec::new(),
            processors: Vec::new(),
            warnings: Vec::new(),
        };
        let result = optimize(&constraints_empty, &Nsga2Config::default());
        assert!(result.pareto_front.is_empty());
        assert_eq!(result.generations_run, 0);
    }

    // ----- Test 6: Pareto front is truly non-dominated -----

    #[test]
    fn nsga2_pareto_front_is_non_dominated() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("proc.t1", 1000, 300),
                make_thread("proc.t2", 1000, 200),
                make_thread("proc.t3", 1000, 250),
                make_thread("proc.t4", 1000, 150),
            ],
            processors: vec![
                make_processor("cpu1"),
                make_processor("cpu2"),
                make_processor("cpu3"),
            ],
            warnings: Vec::new(),
        };

        let config = Nsga2Config {
            population_size: 80,
            generations: 60,
            seed: 321,
            ..Default::default()
        };

        let result = optimize(&constraints, &config);

        // Verify that no solution in the Pareto front dominates another.
        let front = &result.pareto_front;
        for i in 0..front.len() {
            for j in 0..front.len() {
                if i == j {
                    continue;
                }
                assert!(
                    !dominates(&front[i].objectives, &front[j].objectives),
                    "Solution {} dominates solution {} in the Pareto front.\n  {}: {:?}\n  {}: {:?}",
                    i,
                    j,
                    i,
                    front[i].objectives,
                    j,
                    front[j].objectives,
                );
            }
        }
    }

    // ----- Unit tests for internal helpers -----

    #[test]
    fn dominates_basic() {
        assert!(dominates(&[1.0, 2.0], &[1.0, 3.0]));
        assert!(dominates(&[1.0, 2.0], &[2.0, 3.0]));
        assert!(!dominates(&[1.0, 2.0], &[1.0, 2.0])); // equal = not dominated
        assert!(!dominates(&[1.0, 3.0], &[2.0, 2.0])); // neither dominates
    }

    #[test]
    fn lcg_deterministic() {
        let mut rng1 = Lcg::new(42);
        let mut rng2 = Lcg::new(42);
        for _ in 0..100 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
    }

    #[test]
    fn parent_prefix_extraction() {
        assert_eq!(parent_prefix("root.proc.worker"), "root.proc");
        assert_eq!(parent_prefix("worker"), "");
        assert_eq!(parent_prefix("a.b"), "a");
    }
}
