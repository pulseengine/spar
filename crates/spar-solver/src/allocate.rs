//! Bin-packing allocation of threads to processors.
//!
//! Provides First-Fit Decreasing (FFD) and Best-Fit Decreasing (BFD)
//! heuristics for assigning threads to processors based on utilization
//! (WCET / Period). Both respect existing bindings and produce
//! deterministic output per SOLVER-REQ-023.

use serde::Serialize;

use crate::constraints::ModelConstraints;

/// A single thread-to-processor binding produced by the allocator.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Binding {
    /// Thread instance name.
    pub thread: String,
    /// Processor instance name.
    pub processor: String,
    /// This thread's utilization contribution (WCET / Period).
    pub utilization: f64,
}

/// Result of an allocation attempt.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AllocationResult {
    /// Successful thread-to-processor bindings.
    pub bindings: Vec<Binding>,
    /// Thread names that could not be allocated (infeasible).
    pub unallocated: Vec<String>,
    /// Total utilization per processor after allocation.
    pub per_processor_utilization: Vec<(String, f64)>,
    /// Warnings generated during allocation.
    pub warnings: Vec<String>,
}

/// Impact analysis of a proposed allocation.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactAnalysis {
    /// Per-processor RMA utilization (name, utilization, bound)
    pub processor_utilization: Vec<ProcessorImpact>,
    /// Threads that would miss their deadline under the proposed allocation.
    pub deadline_violations: Vec<String>,
    /// Whether the proposed allocation is schedulable.
    pub schedulable: bool,
}

/// Impact summary for one processor.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessorImpact {
    pub name: String,
    pub utilization: f64,
    pub rma_bound: f64,
    pub thread_count: usize,
    pub feasible: bool,
}

impl AllocationResult {
    /// Whether all threads were successfully allocated.
    pub fn is_feasible(&self) -> bool {
        self.unallocated.is_empty()
    }

    /// Compute impact analysis for the proposed allocation.
    ///
    /// Checks RMA utilization bounds per processor and identifies
    /// potential deadline violations.
    pub fn impact(&self, constraints: &ModelConstraints) -> ImpactAnalysis {
        let mut proc_impacts = Vec::new();
        let mut deadline_violations = Vec::new();
        let mut all_feasible = true;

        for (proc_name, total_util) in &self.per_processor_utilization {
            let threads_on_proc: Vec<_> = self
                .bindings
                .iter()
                .filter(|b| &b.processor == proc_name)
                .collect();
            let n = threads_on_proc.len();

            // RMA utilization bound: n * (2^(1/n) - 1)
            let rma_bound = if n > 0 {
                (n as f64) * (2.0_f64.powf(1.0 / n as f64) - 1.0)
            } else {
                1.0
            };

            let feasible = *total_util <= 1.0;
            if !feasible {
                all_feasible = false;
            }

            // Check individual thread deadlines (simple utilization-based check)
            for binding in &threads_on_proc {
                if let Some(tc) = constraints
                    .threads
                    .iter()
                    .find(|t| t.name == binding.thread)
                    && tc.deadline_ps > 0
                    && tc.deadline_ps < tc.period_ps
                    && *total_util > rma_bound
                {
                    deadline_violations.push(format!(
                        "{} on {}: utilization {:.1}% exceeds RMA bound {:.1}% with constrained deadline",
                        binding.thread, proc_name,
                        total_util * 100.0, rma_bound * 100.0
                    ));
                }
            }

            proc_impacts.push(ProcessorImpact {
                name: proc_name.clone(),
                utilization: *total_util,
                rma_bound,
                thread_count: n,
                feasible,
            });
        }

        if !self.unallocated.is_empty() {
            all_feasible = false;
        }

        // Sort for determinism (SOLVER-REQ-023)
        proc_impacts.sort_by(|a, b| a.name.cmp(&b.name));
        deadline_violations.sort();

        let schedulable = all_feasible && deadline_violations.is_empty();
        ImpactAnalysis {
            processor_utilization: proc_impacts,
            deadline_violations,
            schedulable,
        }
    }
}

/// Thread with precomputed utilization, used for sorting.
struct SortableThread {
    name: String,
    utilization: f64,
    current_binding: Option<String>,
}

/// Allocator implementing bin-packing heuristics.
pub struct Allocator;

impl Allocator {
    /// First-Fit Decreasing: assign each thread to the first processor
    /// where it fits (total utilization <= 1.0).
    ///
    /// Threads are sorted by utilization descending, then by name ascending
    /// as a tiebreaker to ensure deterministic output (SOLVER-REQ-023).
    /// Pre-bound threads (those with `current_binding`) are placed first.
    pub fn ffd(constraints: &ModelConstraints) -> AllocationResult {
        Self::allocate(constraints, Strategy::FirstFit)
    }

    /// Best-Fit Decreasing: assign each thread to the processor with the
    /// least remaining capacity that still fits the thread.
    ///
    /// Same sorting and pre-binding rules as FFD.
    pub fn bfd(constraints: &ModelConstraints) -> AllocationResult {
        Self::allocate(constraints, Strategy::BestFit)
    }

    fn allocate(constraints: &ModelConstraints, strategy: Strategy) -> AllocationResult {
        let mut warnings = Vec::new();

        // Build processor capacity tracking: (name, used_utilization).
        let mut processors: Vec<(String, f64)> = constraints
            .processors
            .iter()
            .map(|p| (p.name.clone(), 0.0))
            .collect();

        // If no processors, everything is unallocated.
        if processors.is_empty() && !constraints.threads.is_empty() {
            warnings.push("No processors available for allocation".to_string());
            return AllocationResult {
                bindings: Vec::new(),
                unallocated: constraints.threads.iter().map(|t| t.name.clone()).collect(),
                per_processor_utilization: Vec::new(),
                warnings,
            };
        }

        // Compute sortable threads, skipping those with period=0.
        let mut pre_bound = Vec::new();
        let mut unbound = Vec::new();

        for thread in &constraints.threads {
            if thread.period_ps == 0 {
                warnings.push(format!(
                    "Thread '{}' has period=0; skipping allocation",
                    thread.name
                ));
                continue;
            }

            let utilization = thread.wcet_ps as f64 / thread.period_ps as f64;

            let st = SortableThread {
                name: thread.name.clone(),
                utilization,
                current_binding: thread.current_binding.clone(),
            };

            if st.current_binding.is_some() {
                pre_bound.push(st);
            } else {
                unbound.push(st);
            }
        }

        // Sort both groups: utilization descending, name ascending as tiebreaker.
        let sort_fn = |a: &SortableThread, b: &SortableThread| {
            b.utilization
                .partial_cmp(&a.utilization)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        };
        pre_bound.sort_by(sort_fn);
        unbound.sort_by(sort_fn);

        let mut bindings = Vec::new();
        let mut unallocated = Vec::new();

        // Place pre-bound threads first.
        for thread in &pre_bound {
            let target = thread.current_binding.as_ref().unwrap();
            if let Some(proc) = processors.iter_mut().find(|(name, _)| name == target) {
                let new_util = proc.1 + thread.utilization;
                if new_util <= 1.0 {
                    proc.1 = new_util;
                    bindings.push(Binding {
                        thread: thread.name.clone(),
                        processor: target.clone(),
                        utilization: thread.utilization,
                    });
                } else {
                    warnings.push(format!(
                        "Thread '{}' pre-bound to '{}' exceeds utilization (would be {:.4})",
                        thread.name, target, new_util
                    ));
                    unallocated.push(thread.name.clone());
                }
            } else {
                warnings.push(format!(
                    "Thread '{}' pre-bound to unknown processor '{}'",
                    thread.name, target
                ));
                unallocated.push(thread.name.clone());
            }
        }

        // Place unbound threads.
        for thread in &unbound {
            match strategy {
                Strategy::FirstFit => {
                    if let Some(proc) = processors
                        .iter_mut()
                        .find(|(_, used)| *used + thread.utilization <= 1.0)
                    {
                        proc.1 += thread.utilization;
                        bindings.push(Binding {
                            thread: thread.name.clone(),
                            processor: proc.0.clone(),
                            utilization: thread.utilization,
                        });
                    } else {
                        unallocated.push(thread.name.clone());
                    }
                }
                Strategy::BestFit => {
                    // Find the processor with the least remaining capacity that still fits.
                    let best = processors
                        .iter_mut()
                        .filter(|(_, used)| *used + thread.utilization <= 1.0)
                        .max_by(|(_, a_used), (_, b_used)| {
                            a_used
                                .partial_cmp(b_used)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });

                    if let Some(proc) = best {
                        proc.1 += thread.utilization;
                        bindings.push(Binding {
                            thread: thread.name.clone(),
                            processor: proc.0.clone(),
                            utilization: thread.utilization,
                        });
                    } else {
                        unallocated.push(thread.name.clone());
                    }
                }
            }
        }

        // Build per-processor utilization (sorted by name for determinism).
        let mut per_processor_utilization: Vec<(String, f64)> = processors.into_iter().collect();
        per_processor_utilization.sort_by(|(a, _), (b, _)| a.cmp(b));

        AllocationResult {
            bindings,
            unallocated,
            per_processor_utilization,
            warnings,
        }
    }
}

enum Strategy {
    FirstFit,
    BestFit,
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn ffd_allocates_two_threads_to_one_processor() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 200, None), // util = 0.2
                make_thread("t2", 1000, 300, None), // util = 0.3
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        assert!(result.unallocated.is_empty(), "both should fit");
        assert_eq!(result.bindings.len(), 2);
        assert_eq!(result.bindings[0].processor, "cpu1");
        assert_eq!(result.bindings[1].processor, "cpu1");

        // Total utilization = 0.5
        let cpu_util = result
            .per_processor_utilization
            .iter()
            .find(|(n, _)| n == "cpu1")
            .unwrap()
            .1;
        assert!((cpu_util - 0.5).abs() < 1e-9);
    }

    #[test]
    fn ffd_splits_across_processors() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 600, None), // util = 0.6
                make_thread("t2", 1000, 500, None), // util = 0.5
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        assert!(result.unallocated.is_empty());
        assert_eq!(result.bindings.len(), 2);
        // t1 (0.6) goes to cpu1, t2 (0.5) cannot fit on cpu1, goes to cpu2
        assert_eq!(result.bindings[0].thread, "t1");
        assert_eq!(result.bindings[0].processor, "cpu1");
        assert_eq!(result.bindings[1].thread, "t2");
        assert_eq!(result.bindings[1].processor, "cpu2");
    }

    #[test]
    fn ffd_detects_infeasible() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 600, None), // util = 0.6
                make_thread("t2", 1000, 500, None), // util = 0.5
                make_thread("t3", 1000, 600, None), // util = 0.6
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        // Only t1 fits (0.6), t2 and t3 don't fit
        assert_eq!(result.bindings.len(), 1);
        assert_eq!(result.unallocated.len(), 2);
        assert!(result.unallocated.contains(&"t2".to_string()));
        assert!(result.unallocated.contains(&"t3".to_string()));
    }

    #[test]
    fn ffd_respects_existing_bindings() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 300, Some("cpu2".to_string())), // pre-bound to cpu2
                make_thread("t2", 1000, 400, None),                     // unbound
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        assert!(result.unallocated.is_empty());
        // t1 must be on cpu2
        let t1_binding = result.bindings.iter().find(|b| b.thread == "t1").unwrap();
        assert_eq!(t1_binding.processor, "cpu2");
        // t2 goes to first fit = cpu1
        let t2_binding = result.bindings.iter().find(|b| b.thread == "t2").unwrap();
        assert_eq!(t2_binding.processor, "cpu1");
    }

    #[test]
    fn ffd_skips_threads_without_period() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t_good", 1000, 200, None),
                make_thread("t_bad", 0, 200, None), // period = 0
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        // Only t_good should be allocated
        assert_eq!(result.bindings.len(), 1);
        assert_eq!(result.bindings[0].thread, "t_good");
        // t_bad should NOT appear in unallocated — it's skipped with a warning
        assert!(result.unallocated.is_empty());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("t_bad") && w.contains("period=0"))
        );
    }

    #[test]
    fn bfd_packs_tighter() {
        // Two processors, three threads.
        // FFD: t1(0.5)->cpu1, t2(0.4)->cpu1(0.9), t3(0.3)->cpu2
        // BFD: t1(0.5)->cpu1, t2(0.4)->cpu1(0.9), t3(0.3)->cpu2
        // For a better test: use sizes that show the difference.
        //
        // FFD places in first-fit order (first processor that fits).
        // BFD places in the tightest-fit processor (most used that still fits).
        //
        // Setup: cpu1 has 0.5 used (from pre-bound), cpu2 has 0.3 used.
        // Thread t_new has util=0.4.
        // FFD: cpu1 has 0.5 + 0.4 = 0.9 <= 1.0, fits first -> cpu1
        // BFD: cpu1 has 0.5 (remaining 0.5), cpu2 has 0.3 (remaining 0.7)
        //      Tightest fit = cpu1 (0.5 used > 0.3 used), so cpu1
        //
        // Better test: cpu1=0.7 used, cpu2=0.6 used. Thread needs 0.3.
        // FFD: cpu1 can fit (0.7+0.3=1.0), done.
        // BFD: cpu1 remaining=0.3, cpu2 remaining=0.4. Tightest = cpu1 (most used).
        //
        // Even better: show BFD choosing a DIFFERENT processor than FFD.
        // cpu1=0 used, cpu2=0.6 used. Thread needs 0.35.
        // FFD: cpu1 fits first (0+0.35=0.35), picks cpu1.
        // BFD: cpu1 remaining=1.0, cpu2 remaining=0.4. Tightest = cpu2 (0.6 used).

        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t_pre", 1000, 600, Some("cpu2".to_string())), // 0.6 -> cpu2
                make_thread("t_new", 1000, 350, None),                     // 0.35
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        // FFD: t_pre -> cpu2, t_new -> cpu1 (first fit, cpu1 is at 0.0)
        let ffd_result = Allocator::ffd(&constraints);
        let ffd_new = ffd_result
            .bindings
            .iter()
            .find(|b| b.thread == "t_new")
            .unwrap();
        assert_eq!(ffd_new.processor, "cpu1");

        // BFD: t_pre -> cpu2, t_new -> cpu2 (tightest fit, cpu2 at 0.6 has only 0.4 remaining)
        let bfd_result = Allocator::bfd(&constraints);
        let bfd_new = bfd_result
            .bindings
            .iter()
            .find(|b| b.thread == "t_new")
            .unwrap();
        assert_eq!(bfd_new.processor, "cpu2");
    }

    #[test]
    fn allocation_is_deterministic() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("alpha", 1000, 300, None),
                make_thread("beta", 1000, 300, None),
                make_thread("gamma", 1000, 200, None),
                make_thread("delta", 1000, 300, None),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result1 = Allocator::ffd(&constraints);
        let result2 = Allocator::ffd(&constraints);
        assert_eq!(result1, result2, "FFD must be deterministic");

        let result3 = Allocator::bfd(&constraints);
        let result4 = Allocator::bfd(&constraints);
        assert_eq!(result3, result4, "BFD must be deterministic");
    }

    #[test]
    fn empty_constraints_produces_empty_result() {
        let constraints = ModelConstraints {
            threads: Vec::new(),
            processors: Vec::new(),
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        assert!(result.bindings.is_empty());
        assert!(result.unallocated.is_empty());
        assert!(result.per_processor_utilization.is_empty());
        assert!(result.warnings.is_empty());

        let result = Allocator::bfd(&constraints);
        assert!(result.bindings.is_empty());
        assert!(result.unallocated.is_empty());
        assert!(result.per_processor_utilization.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn pre_bound_to_unknown_processor_warns() {
        let constraints = ModelConstraints {
            threads: vec![make_thread(
                "t1",
                1000,
                200,
                Some("nonexistent".to_string()),
            )],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        assert_eq!(result.unallocated, vec!["t1"]);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("unknown processor"))
        );
    }

    #[test]
    fn pre_bound_exceeding_utilization_warns() {
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("t1", 1000, 700, Some("cpu1".to_string())),
                make_thread("t2", 1000, 400, Some("cpu1".to_string())),
            ],
            processors: vec![make_processor("cpu1")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        // t1 (0.7) fits, t2 (0.4) would make 1.1 -> rejected
        assert_eq!(result.bindings.len(), 1);
        assert_eq!(result.unallocated, vec!["t2"]);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("exceeds utilization"))
        );
    }

    #[test]
    fn tiebreaker_sorts_by_name_ascending() {
        // Two threads with identical utilization — name order determines placement.
        let constraints = ModelConstraints {
            threads: vec![
                make_thread("zebra", 1000, 500, None),
                make_thread("alpha", 1000, 500, None),
            ],
            processors: vec![make_processor("cpu1"), make_processor("cpu2")],
            warnings: Vec::new(),
        };

        let result = Allocator::ffd(&constraints);
        // alpha comes first (name ascending), gets cpu1
        assert_eq!(result.bindings[0].thread, "alpha");
        assert_eq!(result.bindings[0].processor, "cpu1");
        assert_eq!(result.bindings[1].thread, "zebra");
    }
}
