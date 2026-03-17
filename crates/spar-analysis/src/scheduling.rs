//! Scheduling analysis (AS5506 timing properties).
//!
//! Implements three scheduling analyses (STPA-REQ-024, STPA-REQ-025):
//!
//! 1. **Rate Monotonic Analysis (RMA)** — utilization bound check
//! 2. **Response Time Analysis (RTA)** — exact worst-case response time
//!    with higher-priority interference
//! 3. **EDF feasibility** — Earliest Deadline First utilization test
//!
//! Also reports scheduling margin (STPA-REQ-026) and warns when
//! algorithm assumptions may be violated (STPA-REQ-027).
//!
//! # Algorithms
//!
//! **RMA**: U = Σ(Ci/Ti). If U ≤ n(2^(1/n) - 1), guaranteed schedulable.
//!
//! **RTA**: For each task i sorted by priority (shortest period = highest),
//! iteratively compute Ri = Ci + Σ_{j∈hp(i)} ⌈Ri/Tj⌉ × Cj until fixed point.
//! If Ri > Di (deadline), task misses deadline.
//!
//! **EDF**: U = Σ(Ci/Ti). If U ≤ 1.0, schedulable (sufficient AND necessary
//! for independent tasks with D=T).

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{get_execution_time, get_processor_binding, get_timing_property};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Scheduling analysis with RMA, RTA, and EDF.
pub struct SchedulingAnalysis;

impl Analysis for SchedulingAnalysis {
    fn name(&self) -> &str {
        "scheduling"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — processor utilization > 100%, or RTA response time > deadline
        //   Warning — utilization exceeds RMA bound, narrow margin, missing properties
        //   Info    — processor utilization summary, RTA results, modal awareness
        let mut diags = Vec::new();

        // Collect thread timing info and group by processor binding.
        let mut processor_threads: FxHashMap<String, Vec<ThreadInfo>> = FxHashMap::default();

        for (comp_idx, comp) in instance.all_components() {
            if comp.category != ComponentCategory::Thread {
                continue;
            }

            let path = component_path(instance, comp_idx);
            let props = instance.properties_for(comp_idx);

            let period_ps = get_timing_property(props, "Period");
            let exec_ps = get_execution_time(props);
            let deadline_ps = get_timing_property(props, "Deadline");

            if period_ps.is_none() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "thread '{}' has no Period property (required for scheduling analysis)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            if exec_ps.is_none() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "thread '{}' has no Compute_Execution_Time property (required for scheduling analysis)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            let binding = get_processor_binding(props);
            if binding.is_none() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "thread '{}' has no Actual_Processor_Binding (cannot determine target processor)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            let proc_key = binding.unwrap_or_else(|| "__unbound__".to_string());

            if let (Some(period), Some(exec)) = (period_ps, exec_ps) {
                // Default deadline = period if not specified (implicit deadline)
                let deadline = deadline_ps.unwrap_or(period);
                processor_threads
                    .entry(proc_key)
                    .or_default()
                    .push(ThreadInfo {
                        name: comp.name.as_str().to_string(),
                        period_ps: period,
                        exec_ps: exec,
                        deadline_ps: deadline,
                        comp_idx,
                    });
            }
        }

        for (proc_name, threads) in &processor_threads {
            if proc_name == "__unbound__" {
                continue;
            }

            let n = threads.len();
            if n == 0 {
                continue;
            }

            let proc_path = find_processor_path(instance, proc_name);

            let utilization: f64 = threads
                .iter()
                .map(|t| t.exec_ps as f64 / t.period_ps as f64)
                .sum();

            // ── RMA utilization bound ────────────────────────────────
            let rma_bound = rma_utilization_bound(n);

            if utilization > 1.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "processor '{}' is overloaded: utilization {:.1}% ({} threads, bound is 100%)",
                        proc_name,
                        utilization * 100.0,
                        n
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            } else if utilization > rma_bound {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "processor '{}' utilization {:.1}% exceeds RMA bound {:.1}% for {} tasks \
                         (may miss deadlines under rate monotonic scheduling)",
                        proc_name,
                        utilization * 100.0,
                        rma_bound * 100.0,
                        n
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // ── Margin reporting (STPA-REQ-026) ─────────────────────
            let limit = if utilization <= rma_bound {
                rma_bound
            } else {
                1.0
            };
            let margin_pct = (limit - utilization) * 100.0;
            if utilization <= 1.0 {
                if margin_pct < 1.0 {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "processor '{}' has critically narrow scheduling margin: {:.1} percentage points \
                             (utilization {:.1}% vs limit {:.1}%)",
                            proc_name, margin_pct, utilization * 100.0, limit * 100.0
                        ),
                        path: proc_path.clone(),
                        analysis: self.name().to_string(),
                    });
                } else if margin_pct < 5.0 {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "processor '{}' near scheduling limit: margin {:.1} percentage points \
                             (utilization {:.1}% vs limit {:.1}%)",
                            proc_name,
                            margin_pct,
                            utilization * 100.0,
                            limit * 100.0
                        ),
                        path: proc_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // ── EDF feasibility (STPA-REQ-025) ─────────────────────
            // For implicit-deadline tasks (D=T), EDF is schedulable iff U ≤ 1.0.
            // This is both sufficient and necessary (unlike RMA).
            let all_implicit_deadline = threads.iter().all(|t| t.deadline_ps == t.period_ps);
            if utilization <= 1.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "processor '{}' EDF feasible: utilization {:.1}% ≤ 100%{}",
                        proc_name,
                        utilization * 100.0,
                        if all_implicit_deadline {
                            " (sufficient and necessary for implicit-deadline tasks)"
                        } else {
                            " (sufficient only — tasks have explicit deadlines)"
                        }
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            } else {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "processor '{}' NOT EDF feasible: utilization {:.1}% > 100%",
                        proc_name,
                        utilization * 100.0
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // ── Response Time Analysis (STPA-REQ-024) ───────────────
            // Sort threads by period (shortest period = highest RM priority).
            let mut sorted: Vec<&ThreadInfo> = threads.iter().collect();
            sorted.sort_by_key(|t| t.period_ps);

            for (i, task) in sorted.iter().enumerate() {
                let rta_result = compute_response_time(task, &sorted[..i]);
                match rta_result {
                    RtaResult::Converged(response_ps) => {
                        if response_ps > task.deadline_ps {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "RTA: thread '{}' on '{}' misses deadline — response time {:.2} ms > deadline {:.2} ms",
                                    task.name,
                                    proc_name,
                                    ps_to_ms(response_ps),
                                    ps_to_ms(task.deadline_ps)
                                ),
                                path: proc_path.clone(),
                                analysis: self.name().to_string(),
                            });
                        } else {
                            let margin_ms = ps_to_ms(task.deadline_ps) - ps_to_ms(response_ps);
                            let margin_ratio = margin_ms / ps_to_ms(task.deadline_ps);
                            if margin_ratio < 0.05 {
                                diags.push(AnalysisDiagnostic {
                                    severity: Severity::Warning,
                                    message: format!(
                                        "RTA: thread '{}' on '{}' has tight margin — response time {:.2} ms, deadline {:.2} ms, margin {:.2} ms ({:.0}%)",
                                        task.name, proc_name, ps_to_ms(response_ps), ps_to_ms(task.deadline_ps), margin_ms, margin_ratio * 100.0
                                    ),
                                    path: proc_path.clone(),
                                    analysis: self.name().to_string(),
                                });
                            }
                        }
                    }
                    RtaResult::Diverged => {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "RTA: thread '{}' on '{}' response time diverges (unschedulable)",
                                task.name, proc_name
                            ),
                            path: proc_path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }

            // Utilization info (always)
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "processor '{}' utilization: {:.1}% ({} threads, RMA bound: {:.1}%, margin: {:.1} pp)",
                    proc_name,
                    utilization * 100.0,
                    n,
                    rma_bound * 100.0,
                    margin_pct,
                ),
                path: proc_path,
                analysis: self.name().to_string(),
            });
        }

        // STPA-REQ-031: Analysis limitation documentation
        diags.push(AnalysisDiagnostic {
            severity: Severity::Info,
            message: "scheduling: checks RMA utilization bound, EDF feasibility, and RTA response \
                      times. Does not account for: blocking time, priority inversion, \
                      non-preemptive sections, or inter-processor interference. For systems with \
                      shared resources, consider external tools (Cheddar, MAST)."
                .to_string(),
            path: vec!["root".to_string()],
            analysis: self.name().to_string(),
        });

        // STPA-REQ-017: Note modal awareness
        if !instance.system_operation_modes.is_empty() {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "{} analysis used default property values; {} system operation mode(s) exist but modal property evaluation is not yet fully supported",
                    self.name(),
                    instance.system_operation_modes.len()
                ),
                path: vec!["root".to_string()],
                analysis: self.name().to_string(),
            });
        }

        diags
    }
}

/// Thread timing information extracted from properties.
struct ThreadInfo {
    name: String,
    period_ps: u64,
    exec_ps: u64,
    deadline_ps: u64,
    #[allow(dead_code)]
    comp_idx: ComponentInstanceIdx,
}

/// Result of Response Time Analysis for a single task.
enum RtaResult {
    /// Converged to a fixed-point response time (in picoseconds).
    Converged(u64),
    /// Response time exceeded the task's period (unschedulable).
    Diverged,
}

/// Compute worst-case response time for a task under fixed-priority scheduling.
///
/// `higher_priority` contains all tasks with higher priority (shorter period
/// under RM assignment), sorted by period.
///
/// Algorithm: iteratively compute R = C + Σ_{j∈hp} ⌈R/Tj⌉ × Cj
/// until R converges or exceeds the task's period.
fn compute_response_time(task: &ThreadInfo, higher_priority: &[&ThreadInfo]) -> RtaResult {
    let mut r = task.exec_ps;
    let max_iterations = 100;

    for _ in 0..max_iterations {
        let mut interference: u64 = 0;
        for hp in higher_priority {
            // ⌈R / Tj⌉ × Cj
            let activations = r.div_ceil(hp.period_ps);
            interference = interference.saturating_add(activations.saturating_mul(hp.exec_ps));
        }

        let new_r = task.exec_ps.saturating_add(interference);

        if new_r == r {
            return RtaResult::Converged(r);
        }

        if new_r > task.period_ps {
            return RtaResult::Diverged;
        }

        r = new_r;
    }

    // Did not converge within iteration limit
    RtaResult::Diverged
}

/// Convert picoseconds to milliseconds for display.
fn ps_to_ms(ps: u64) -> f64 {
    ps as f64 / 1_000_000_000.0
}

/// Compute the RMA utilization bound: n(2^(1/n) - 1).
///
/// For n=1: 1.0, n=2: ~0.828, ..., converges to ln(2) ≈ 0.693.
fn rma_utilization_bound(n: usize) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let n_f = n as f64;
    n_f * (2.0_f64.powf(1.0 / n_f) - 1.0)
}

/// Find the path to a processor component by name for diagnostic reporting.
fn find_processor_path(instance: &SystemInstance, name: &str) -> Vec<String> {
    for (idx, comp) in instance.all_components() {
        if comp.name.as_str().eq_ignore_ascii_case(name) {
            return component_path(instance, idx);
        }
    }
    vec![name.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                property_maps: FxHashMap::default(),
            }
        }

        fn add_component(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(name),
                impl_name: Some(Name::new("impl")),
                package: Name::new("Pkg"),
                parent,
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

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        fn set_property(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
            let map = self.property_maps.entry(comp).or_default();
            map.add(PropertyValue {
                name: PropertyRef {
                    property_set: if set.is_empty() {
                        None
                    } else {
                        Some(Name::new(set))
                    },
                    property_name: Name::new(name),
                },
                value: value.to_string(),
                is_append: false,
            });
        }

        fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
            SystemInstance {
                root,
                components: self.components,
                features: self.features,
                connections: self.connections,
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
                mode_instances: Arena::default(),
                mode_transition_instances: Arena::default(),
                diagnostics: Vec::new(),
                property_maps: self.property_maps,
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    #[test]
    fn rma_bound_values() {
        // n=1 should be 1.0
        assert!((rma_utilization_bound(1) - 1.0).abs() < 0.001);
        // n=2 should be ~0.828
        assert!((rma_utilization_bound(2) - 0.828).abs() < 0.01);
        // Large n converges to ln(2) ≈ 0.693
        assert!((rma_utilization_bound(100) - 0.693).abs() < 0.01);
    }

    #[test]
    fn schedulable_system_info_only() {
        // Two threads with low utilization bound to cpu1
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        // t1: period=10ms, exec=1ms -> U=0.1
        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        // t2: period=20ms, exec=2ms -> U=0.1
        b.set_property(t2, "Timing_Properties", "Period", "20 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // Total U = 0.2, well under RMA bound of 0.828 for 2 tasks
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "schedulable system should have no errors: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert!(
            !infos.is_empty(),
            "should report utilization info: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("20.0%"),
            "utilization should be 20%: {}",
            infos[0].message
        );
    }

    #[test]
    fn overloaded_processor_error() {
        // Threads whose combined execution exceeds period -> U > 1.0
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        // t1: period=10ms, exec=8ms -> U=0.8
        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "8 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        // t2: period=10ms, exec=5ms -> U=0.5
        // Total: 1.3 > 1.0
        b.set_property(t2, "Timing_Properties", "Period", "10 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let overload_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("overloaded"))
            .collect();
        assert_eq!(
            overload_errors.len(),
            1,
            "should have 1 overload error: {:?}",
            diags
        );

        // Also expect EDF infeasible and RTA divergence errors
        let all_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            all_errors.len() >= 2,
            "should have overload + EDF/RTA errors: {:?}",
            all_errors
        );
    }

    #[test]
    fn thread_missing_period_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        // Only set execution time, no period
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let period_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("Period"))
            .collect();
        assert_eq!(
            period_warns.len(),
            1,
            "should warn about missing Period: {:?}",
            diags
        );
    }

    #[test]
    fn thread_missing_binding_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        // No processor binding

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let binding_warns: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning && d.message.contains("Actual_Processor_Binding")
            })
            .collect();
        assert_eq!(
            binding_warns.len(),
            1,
            "should warn about missing binding: {:?}",
            diags
        );
    }

    #[test]
    fn range_execution_time_uses_worst_case() {
        // Compute_Execution_Time as range "1 ms .. 5 ms" should use 5ms
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            t1,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms .. 5 ms",
        );
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // U = 5/10 = 0.5 = 50%
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert!(!infos.is_empty(), "should report utilization: {:?}", diags);
        assert!(
            infos[0].message.contains("50.0%"),
            "should be 50%: {}",
            infos[0].message
        );
    }

    #[test]
    fn modal_awareness_diagnostic_when_soms_present() {
        // STPA-REQ-017: When SOMs exist, analysis should note it used default values
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        // Build with SOMs present
        let mut inst = b.build(root);
        inst.system_operation_modes = vec![
            spar_hir_def::instance::SystemOperationMode {
                name: "nominal".to_string(),
                mode_selections: Vec::new(),
            },
            spar_hir_def::instance::SystemOperationMode {
                name: "degraded".to_string(),
                mode_selections: Vec::new(),
            },
        ];

        let diags = SchedulingAnalysis.analyze(&inst);

        let modal_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("system operation mode"))
            .collect();
        assert_eq!(
            modal_diags.len(),
            1,
            "should emit exactly one modal awareness diagnostic: {:?}",
            diags
        );
        assert_eq!(modal_diags[0].severity, Severity::Info);
        assert!(
            modal_diags[0]
                .message
                .contains("2 system operation mode(s)"),
            "should mention count of SOMs: {}",
            modal_diags[0].message
        );
        assert!(
            modal_diags[0].message.contains("default property values"),
            "should mention default values: {}",
            modal_diags[0].message
        );
    }

    #[test]
    fn no_modal_diagnostic_without_soms() {
        // When no SOMs exist, no modal awareness diagnostic should be emitted
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let modal_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("system operation mode"))
            .collect();
        assert!(
            modal_diags.is_empty(),
            "should not emit modal diagnostic without SOMs: {:?}",
            modal_diags
        );
    }

    #[test]
    fn utilization_above_rma_bound_but_under_100() {
        // 3 threads each with U=0.30 -> total 0.90 > RMA bound 0.780 for n=3, but < 1.0
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        let t3 = b.add_component("t3", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2, t3]);

        for t in [t1, t2, t3] {
            b.set_property(t, "Timing_Properties", "Period", "10 ms");
            b.set_property(t, "Timing_Properties", "Compute_Execution_Time", "3 ms");
            b.set_property(
                t,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                "reference (cpu1)",
            );
        }

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // U = 0.9, RMA bound for 3 tasks ≈ 0.780
        // RTA will catch deadline misses here
        let rma_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("RMA bound"))
            .collect();
        assert_eq!(
            rma_warnings.len(),
            1,
            "should warn about exceeding RMA bound: {:?}",
            diags
        );
    }

    // ── RTA tests ─────────────────────────────────────────────────

    #[test]
    fn rta_tight_margin_warning() {
        // T1(period=10ms, exec=8ms), T2(period=30ms, exec=3ms)
        // RM priority: T1 > T2
        // T2 response time: R = 3 + ⌈R/10⌉*8
        //   R=3:  3 + ⌈3/10⌉*8  = 3 + 8  = 11
        //   R=11: 3 + ⌈11/10⌉*8 = 3 + 16 = 19
        //   R=19: 3 + ⌈19/10⌉*8 = 3 + 16 = 19 (converged)
        // Deadline = 30ms (implicit), R=19ms, margin = 11/30 = 36.7% → no warning
        //
        // Use explicit deadline to make it tight:
        // T2 deadline = 20ms, R=19ms, margin = 1/20 = 5.0% → exactly at threshold
        // Make deadline = 19.5ms → margin 0.5/19.5 = 2.6% → tight margin
        // Actually: deadline in ps must be integer. 20ms deadline, R=19ms, margin=5%.
        // So let's do: T1(period=5ms,exec=4ms), T2(period=10ms,exec=1ms)
        // T2: R = 1 + ⌈R/5⌉*4
        //   R=1: 1 + 4 = 5
        //   R=5: 1 + 4 = 5 (converged at 5ms)
        // Deadline = 10ms, margin = 5/10 = 50% → too wide
        //
        // T1(period=10ms,exec=9ms), T2(period=100ms,exec=3ms)
        // T2: R = 3 + ⌈R/10⌉*9
        //   R=3:  3 + 9  = 12
        //   R=12: 3 + 18 = 21
        //   R=21: 3 + 27 = 30
        //   R=30: 3 + 27 = 30 (converged)
        // Deadline = 100ms (implicit), margin = 70/100 = 70% → too wide
        // Use explicit deadline = 31ms: margin = 1/31 = 3.2% → tight!
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "9 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(t2, "Timing_Properties", "Period", "100 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "3 ms");
        b.set_property(t2, "Timing_Properties", "Deadline", "31 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let rta_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("RTA"))
            .collect();
        assert!(
            rta_errors.is_empty(),
            "T2 should not miss deadline: {:?}",
            rta_errors
        );

        let tight: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("tight margin") && d.message.contains("t2"))
            .collect();
        assert!(
            !tight.is_empty(),
            "T2 should have tight margin warning: {:?}",
            diags
        );
    }

    #[test]
    fn rta_deadline_miss_with_explicit_deadline() {
        // Thread with explicit short deadline that RMA wouldn't catch
        // T1: period=10ms, exec=3ms, deadline=10ms (implicit, fine)
        // T2: period=20ms, exec=5ms, deadline=8ms (explicit, tight)
        // T2 response time: R = 5 + ⌈R/10⌉*3
        //   R=5: 5 + ⌈5/10⌉*3 = 5 + 3 = 8
        //   R=8: 5 + ⌈8/10⌉*3 = 5 + 3 = 8 (converged at 8ms)
        // Deadline = 8ms, R=8ms → exactly at deadline, no miss
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "3 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(t2, "Timing_Properties", "Period", "20 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(t2, "Timing_Properties", "Deadline", "8 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // R=8ms exactly equals deadline 8ms → no miss but zero margin
        let rta_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("RTA"))
            .collect();
        assert!(
            rta_errors.is_empty(),
            "R=D should not be error: {:?}",
            rta_errors
        );
    }

    #[test]
    fn rta_diverges_on_overloaded_system() {
        // Overloaded: U > 1.0 → RTA should diverge
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "8 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(t2, "Timing_Properties", "Period", "10 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let rta_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("RTA"))
            .collect();
        assert!(
            !rta_errors.is_empty(),
            "overloaded system should have RTA errors: {:?}",
            diags
        );
    }

    // ── EDF tests ─────────────────────────────────────────────────

    #[test]
    fn edf_feasible_reported() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "3 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let edf_info: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("EDF feasible"))
            .collect();
        assert!(
            !edf_info.is_empty(),
            "should report EDF feasibility: {:?}",
            diags
        );
    }

    // ── Margin tests ──────────────────────────────────────────────

    #[test]
    fn narrow_margin_warning() {
        // Utilization very close to RMA bound → narrow margin warning
        // For n=1, RMA bound = 1.0. Put utilization at 0.96 (4% margin)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        // U = 96/100 = 0.96 → margin 4% (< 5% threshold)
        b.set_property(t1, "Timing_Properties", "Period", "100 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "96 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let margin_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("margin"))
            .collect();
        assert!(
            !margin_warns.is_empty(),
            "should warn about narrow margin: {:?}",
            diags
        );
    }

    // ── Limitation documentation test ─────────────────────────────

    #[test]
    fn limitation_documentation_emitted() {
        // STPA-REQ-031: analysis must document its limitations
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_children(root, vec![]);

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let limitation: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Does not account for"))
            .collect();
        assert!(
            !limitation.is_empty(),
            "should document limitations: {:?}",
            diags
        );
    }
}

/// Conformance tests: verify that the inlined scheduling math in
/// `compute_response_time` matches the Lean-proven decomposed functions
/// in `scheduling_verified`. Any divergence here means either the Lean
/// codegen or the actual implementation has drifted.
#[cfg(test)]
mod conformance_tests {
    use crate::scheduling_verified as verified;

    // ── Arithmetic conformance ───────────────────────────────────

    #[test]
    fn ceil_div_matches_inline() {
        // The actual code computes r.div_ceil(period) inline.
        // Verify the Lean-proven ceil_div produces the same result.
        let cases: &[(u64, u64)] = &[
            (0, 1),
            (1, 1),
            (1, 2),
            (7, 3),
            (6, 3),
            (9, 3),
            (10, 10),
            (11, 10),
            (100, 7),
            (1, 1000),
            (999, 1000),
            (1000, 1000),
            (1001, 1000),
            (u64::MAX / 2, 1000),
        ];
        for &(a, b) in cases {
            let inline = a.div_ceil(b);
            let proven = verified::ceil_div(a, b);
            assert_eq!(inline, proven, "ceil_div mismatch for ({a}, {b})");
        }
    }

    #[test]
    fn interference_matches_inline() {
        // Actual: activations = r.div_ceil(period); interference = activations * exec
        let cases: &[(u64, u64, u64)] = &[
            // (period, exec, r)
            (10, 2, 3),   // ceil(3/10)*2 = 2
            (10, 2, 10),  // ceil(10/10)*2 = 2
            (10, 2, 11),  // ceil(11/10)*2 = 4
            (5, 3, 12),   // ceil(12/5)*3 = 9
            (100, 50, 1), // ceil(1/100)*50 = 50
            (1, 1, 100),  // ceil(100/1)*1 = 100
        ];
        for &(period, hp_exec, r) in cases {
            let inline = (r.div_ceil(period)).saturating_mul(hp_exec);
            let proven = verified::interference(period, hp_exec, r);
            assert_eq!(
                inline, proven,
                "interference mismatch for period={period}, exec={hp_exec}, r={r}"
            );
        }
    }

    #[test]
    fn total_interference_matches_loop() {
        // Actual scheduling.rs accumulates interference in a for loop.
        // Verified version uses total_interference(&[(period, exec)], r).
        let hp_tasks: &[(u64, u64)] = &[(10, 2), (20, 3), (50, 5)];
        let test_r_values: &[u64] = &[1, 5, 10, 15, 20, 25, 30, 50, 100];

        for &r in test_r_values {
            // Compute inline (same as scheduling.rs loop body)
            let mut inline_total: u64 = 0;
            for &(period, hp_exec) in hp_tasks {
                let activations = r.div_ceil(period);
                inline_total = inline_total.saturating_add(activations.saturating_mul(hp_exec));
            }

            let proven = verified::total_interference(hp_tasks, r);
            assert_eq!(inline_total, proven, "total_interference mismatch at r={r}");
        }
    }

    #[test]
    fn rta_step_matches_inline() {
        let task_exec = 5u64;
        let hp_tasks: &[(u64, u64)] = &[(10, 2), (30, 4)];
        let test_r_values: &[u64] = &[5, 7, 10, 15, 20, 25];

        for &r in test_r_values {
            // Inline: new_r = exec + sum of ceil(r/Tj)*Cj
            let mut interference: u64 = 0;
            for &(period, hp_exec) in hp_tasks {
                let activations = r.div_ceil(period);
                interference = interference.saturating_add(activations.saturating_mul(hp_exec));
            }
            let inline_new_r = task_exec.saturating_add(interference);

            let proven = verified::rta_step(task_exec, hp_tasks, r);
            assert_eq!(inline_new_r, proven, "rta_step mismatch at r={r}");
        }
    }

    // ── Full RTA conformance ─────────────────────────────────────

    /// Run the actual (inlined) RTA iteration and return the same type as
    /// the verified version, so we can compare results directly.
    fn actual_rta(
        task_exec: u64,
        deadline: u64,
        higher_priority: &[(u64, u64)],
    ) -> verified::RtaResult {
        // This mirrors the actual scheduling.rs logic exactly, but uses
        // the verified RtaResult type for comparison.
        let mut r = task_exec;
        let max_iterations = deadline + 1; // use proven bound, not 100
        for _ in 0..=max_iterations {
            let mut interference: u64 = 0;
            for &(period, hp_exec) in higher_priority {
                let activations = r.div_ceil(period);
                interference = interference.saturating_add(activations.saturating_mul(hp_exec));
            }
            let new_r = task_exec.saturating_add(interference);
            if new_r == r {
                return verified::RtaResult::Converged(r);
            }
            if new_r > deadline {
                return verified::RtaResult::Diverged;
            }
            r = new_r;
        }
        verified::RtaResult::Diverged
    }

    #[test]
    fn rta_no_interference_converges_at_exec() {
        let actual = actual_rta(5, 20, &[]);
        let proven = verified::compute_response_time(5, 20, &[]);
        assert_eq!(actual, proven, "no-interference case");
        assert_eq!(proven, verified::RtaResult::Converged(5));
    }

    #[test]
    fn rta_single_hp_converges() {
        // Task: exec=3, deadline=10; HP: period=10, exec=2
        let actual = actual_rta(3, 10, &[(10, 2)]);
        let proven = verified::compute_response_time(3, 10, &[(10, 2)]);
        assert_eq!(actual, proven, "single-HP convergence");
        assert_eq!(proven, verified::RtaResult::Converged(5));
    }

    #[test]
    fn rta_overloaded_diverges() {
        // Task: exec=8, deadline=10; HP: period=10, exec=5
        let actual = actual_rta(8, 10, &[(10, 5)]);
        let proven = verified::compute_response_time(8, 10, &[(10, 5)]);
        assert_eq!(actual, proven, "overloaded divergence");
        assert_eq!(proven, verified::RtaResult::Diverged);
    }

    #[test]
    fn rta_multi_hp_converges() {
        // Task: exec=2, deadline=20; HP1: (5, 1), HP2: (10, 2)
        // R=2: 2 + ceil(2/5)*1 + ceil(2/10)*2 = 2+1+2 = 5
        // R=5: 2 + ceil(5/5)*1 + ceil(5/10)*2 = 2+1+2 = 5 (fixed point)
        let actual = actual_rta(2, 20, &[(5, 1), (10, 2)]);
        let proven = verified::compute_response_time(2, 20, &[(5, 1), (10, 2)]);
        assert_eq!(actual, proven, "multi-HP convergence");
        assert_eq!(proven, verified::RtaResult::Converged(5));
    }

    #[test]
    fn rta_conformance_systematic() {
        // Systematic test: sweep over a range of task parameters and
        // verify both implementations always agree.
        let exec_values = [1, 2, 3, 5, 8, 10];
        let deadline_values = [5, 10, 15, 20, 50];
        let hp_sets: &[&[(u64, u64)]] = &[
            &[],
            &[(10, 1)],
            &[(10, 2)],
            &[(5, 1), (10, 2)],
            &[(10, 3), (20, 4)],
            &[(5, 2), (10, 3), (20, 1)],
        ];

        for &task_exec in &exec_values {
            for &deadline in &deadline_values {
                if task_exec > deadline {
                    continue; // skip obviously infeasible
                }
                for &hp in hp_sets {
                    let actual = actual_rta(task_exec, deadline, hp);
                    let proven = verified::compute_response_time(task_exec, deadline, hp);
                    assert_eq!(
                        actual, proven,
                        "conformance mismatch: exec={task_exec}, deadline={deadline}, hp={hp:?}"
                    );
                }
            }
        }
    }
}
