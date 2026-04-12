//! Response Time Analysis (RTA) for fixed-priority preemptive scheduling.
//!
//! Uses exact response-time computation via fixed-point iteration to determine
//! whether each thread meets its deadline. This is more precise than the
//! utilization-based RMA check in [`scheduling`]: a task set may exceed the
//! RMA utilization bound yet still be schedulable per RTA.
//!
//! # Algorithm
//!
//! For each processor, threads are sorted by priority (explicit
//! `Deployment_Properties::Priority`, or shorter period = higher priority).
//! For each thread *i*, the worst-case response time is computed via:
//!
//! ```text
//! R(0) = C_i
//! R(n+1) = C_i + Σ_j ⌈R(n)/T_j⌉ × C_j   (for all higher-priority threads j)
//! ```
//!
//! The iteration uses the Lean4-verified `compute_response_time()` from
//! [`scheduling_verified`]. If the converged response time exceeds the
//! thread's deadline (or period, when no explicit deadline is set), the
//! thread misses its deadline and an error is reported.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance, SystemOperationMode};
use spar_hir_def::item_tree::ComponentCategory;

use crate::modal::is_component_active_in_som;
use crate::property_accessors::{get_execution_time, get_processor_binding, get_timing_property};
use crate::scheduling_verified::{self, RtaResult};
use crate::{Analysis, AnalysisDiagnostic, ModalAnalysis, Severity, component_path};

/// Response Time Analysis pass.
pub struct RtaAnalysis;

impl Analysis for RtaAnalysis {
    fn name(&self) -> &str {
        "rta"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Collect thread timing info grouped by processor binding.
        let mut processor_threads: FxHashMap<String, Vec<RtaThreadInfo>> = FxHashMap::default();

        for (idx, comp) in instance.all_components() {
            if comp.category != ComponentCategory::Thread {
                continue;
            }

            let props = instance.properties_for(idx);

            let Some(period_ps) = get_timing_property(props, "Period") else {
                // No period — skip; the scheduling pass already warns about this.
                continue;
            };

            let Some(exec_ps) = get_execution_time(props) else {
                // No execution time — skip; the scheduling pass already warns.
                continue;
            };

            let binding = get_processor_binding(props).unwrap_or("__unbound__".to_string());

            // Explicit deadline, falling back to period (implicit deadline).
            let deadline_ps = get_timing_property(props, "Deadline").unwrap_or(period_ps);

            // Explicit priority (lower number = higher priority in AADL).
            let priority = get_priority(props);

            processor_threads
                .entry(binding)
                .or_default()
                .push(RtaThreadInfo {
                    name: comp.name.as_str().to_string(),
                    period_ps,
                    exec_ps,
                    deadline_ps,
                    priority,
                    comp_idx: idx,
                });
        }

        // Sort processor names for deterministic output order.
        let mut proc_names: Vec<_> = processor_threads.keys().cloned().collect();
        proc_names.sort();

        for proc_name in proc_names {
            if proc_name == "__unbound__" {
                continue;
            }

            let threads = processor_threads.get_mut(&proc_name).unwrap();

            if threads.is_empty() {
                continue;
            }

            // Sort by priority: explicit priority first (lower number = higher
            // priority), then by shorter period (Rate Monotonic ordering).
            threads.sort_by(|a, b| match (a.priority, b.priority) {
                (Some(pa), Some(pb)) => pa.cmp(&pb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.period_ps.cmp(&b.period_ps),
            });

            // Run RTA for each thread in priority order.
            for i in 0..threads.len() {
                let thread = &threads[i];

                // Collect (period, exec) for all higher-priority threads (indices 0..i).
                let higher_priority: Vec<(u64, u64)> = threads[..i]
                    .iter()
                    .map(|t| (t.period_ps, t.exec_ps))
                    .collect();

                let thread_path = component_path(instance, thread.comp_idx);

                let result = scheduling_verified::compute_response_time(
                    thread.exec_ps,
                    thread.deadline_ps,
                    &higher_priority,
                );

                match result {
                    RtaResult::Converged(response_time) => {
                        if response_time > thread.deadline_ps {
                            // Should not happen (compute_response_time returns Diverged),
                            // but guard defensively.
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "thread '{}' on processor '{}' misses deadline: \
                                     response time {} > deadline {}",
                                    thread.name,
                                    proc_name,
                                    format_time(response_time),
                                    format_time(thread.deadline_ps),
                                ),
                                path: thread_path,
                                analysis: self.name().to_string(),
                            });
                        } else {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "thread '{}' on processor '{}': response time {} <= deadline {}",
                                    thread.name,
                                    proc_name,
                                    format_time(response_time),
                                    format_time(thread.deadline_ps),
                                ),
                                path: thread_path,
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    RtaResult::Diverged => {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "thread '{}' on processor '{}' misses deadline: \
                                 response time exceeds deadline {}",
                                thread.name,
                                proc_name,
                                format_time(thread.deadline_ps),
                            ),
                            path: thread_path,
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        diags
    }
}

/// Thread timing information for RTA.
struct RtaThreadInfo {
    name: String,
    period_ps: u64,
    exec_ps: u64,
    deadline_ps: u64,
    /// Explicit priority (lower = higher priority). `None` means unset.
    priority: Option<u64>,
    comp_idx: ComponentInstanceIdx,
}

/// Read the `Deployment_Properties::Priority` value.
fn get_priority(props: &spar_hir_def::properties::PropertyMap) -> Option<u64> {
    let raw = props
        .get("Deployment_Properties", "Priority")
        .or_else(|| props.get("", "Priority"))?;
    raw.trim().parse::<u64>().ok()
}

/// Format a time value in picoseconds as a human-readable string.
fn format_time(ps: u64) -> String {
    if ps >= 1_000_000_000_000 {
        format!("{:.2} sec", ps as f64 / 1_000_000_000_000.0)
    } else if ps >= 1_000_000_000 {
        format!("{:.2} ms", ps as f64 / 1_000_000_000.0)
    } else if ps >= 1_000_000 {
        format!("{:.2} us", ps as f64 / 1_000_000.0)
    } else {
        format!("{} ps", ps)
    }
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

    /// Helper: set up a processor and process as children of a root system.
    fn make_base() -> (TestBuilder, ComponentInstanceIdx, ComponentInstanceIdx) {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        (b, root, proc)
    }

    /// Helper: bind a thread to cpu1 with period, execution time, and optional deadline.
    fn bind_thread(
        b: &mut TestBuilder,
        idx: ComponentInstanceIdx,
        period: &str,
        exec: &str,
        deadline: Option<&str>,
    ) {
        b.set_property(idx, "Timing_Properties", "Period", period);
        b.set_property(idx, "Timing_Properties", "Compute_Execution_Time", exec);
        b.set_property(
            idx,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        if let Some(d) = deadline {
            b.set_property(idx, "Timing_Properties", "Deadline", d);
        }
    }

    // ── Test 1: Basic convergence ───────────────────────────────────
    #[test]
    fn basic_convergence_two_threads() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));

        // cpu1 is components[1], proc is components[2]
        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1, t2]);

        // t1: period=10ms, exec=1ms (higher priority — shorter period)
        bind_thread(&mut b, t1, "10 ms", "1 ms", None);
        // t2: period=20ms, exec=2ms (lower priority)
        bind_thread(&mut b, t2, "20 ms", "2 ms", None);

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        // Both threads should meet deadlines (Info only, no Errors).
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should have no errors: {:?}", errors);

        // Should have exactly 2 info diagnostics (one per thread).
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(
            infos.len(),
            2,
            "expected 2 response-time infos: {:?}",
            diags
        );

        // t1 (highest priority, no interference): R = 1ms
        assert!(
            infos[0].message.contains("t1"),
            "first info should be for t1: {}",
            infos[0].message,
        );
        assert!(
            infos[0].message.contains("1.00 ms"),
            "t1 response time should be 1ms: {}",
            infos[0].message,
        );

        // t2: R0 = 2ms, R1 = 2 + ceil(2/10)*1 = 3ms, R2 = 2 + ceil(3/10)*1 = 3ms => converged at 3ms
        assert!(
            infos[1].message.contains("t2"),
            "second info should be for t2: {}",
            infos[1].message,
        );
        assert!(
            infos[1].message.contains("3.00 ms"),
            "t2 response time should be 3ms: {}",
            infos[1].message,
        );
    }

    // ── Test 2: Deadline miss ───────────────────────────────────────
    #[test]
    fn deadline_miss_detected() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1, t2]);

        // t1: period=10ms, exec=8ms (high priority)
        bind_thread(&mut b, t1, "10 ms", "8 ms", None);
        // t2: period=20ms, exec=5ms, deadline=10ms
        // R0=5ms, R1=5+ceil(5/10)*8=5+8=13ms > deadline 10ms => miss
        bind_thread(&mut b, t2, "20 ms", "5 ms", Some("10 ms"));

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "expected 1 deadline miss error: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("t2"),
            "error should be for t2: {}",
            errors[0].message,
        );
        assert!(
            errors[0].message.contains("misses deadline"),
            "should say misses deadline: {}",
            errors[0].message,
        );
    }

    // ── Test 3: No explicit deadline (implicit = period) ────────────
    #[test]
    fn implicit_deadline_equals_period() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1]);

        // Single thread: period=10ms, exec=3ms, no Deadline => deadline = period = 10ms
        bind_thread(&mut b, t1, "10 ms", "3 ms", None);

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should have no errors: {:?}", errors);

        // The info should report deadline as 10ms (same as period).
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(infos.len(), 1, "expected 1 info diagnostic: {:?}", diags);
        assert!(
            infos[0].message.contains("10.00 ms"),
            "implicit deadline should be 10ms (period): {}",
            infos[0].message,
        );
    }

    // ── Test 4: Multiple processors ─────────────────────────────────
    #[test]
    fn multiple_processors_independent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));

        b.set_children(root, vec![cpu1, cpu2, proc]);
        b.set_children(proc, vec![t1, t2]);

        // t1 on cpu1: period=10ms, exec=1ms
        bind_thread(&mut b, t1, "10 ms", "1 ms", None);
        // Override binding to cpu1 (already set by bind_thread)

        // t2 on cpu2: period=10ms, exec=1ms
        b.set_property(t2, "Timing_Properties", "Period", "10 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(
            t2,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should have no errors: {:?}", errors);

        // Should have info for each thread on its respective processor.
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(infos.len(), 2, "expected 2 info diagnostics: {:?}", diags);

        // Verify each thread's info refers to the correct processor.
        let cpu1_infos: Vec<_> = infos
            .iter()
            .filter(|d| d.message.contains("cpu1"))
            .collect();
        let cpu2_infos: Vec<_> = infos
            .iter()
            .filter(|d| d.message.contains("cpu2"))
            .collect();
        assert_eq!(cpu1_infos.len(), 1, "expected 1 info for cpu1");
        assert_eq!(cpu2_infos.len(), 1, "expected 1 info for cpu2");
    }

    // ── Test 5: Single thread (trivial) ─────────────────────────────
    #[test]
    fn single_thread_trivial() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1]);

        // Single thread: period=50ms, exec=10ms => R = C = 10ms (no interference)
        bind_thread(&mut b, t1, "50 ms", "10 ms", None);

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "single thread should have no errors: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(infos.len(), 1, "expected 1 info: {:?}", diags);
        assert!(
            infos[0].message.contains("10.00 ms"),
            "response time should be 10ms (exec): {}",
            infos[0].message,
        );
    }

    // ── Test 6: Explicit priority ordering ──────────────────────────
    #[test]
    fn explicit_priority_overrides_period_ordering() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t_long", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t_short", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1, t2]);

        // t_long: period=100ms, exec=5ms, priority=1 (highest)
        bind_thread(&mut b, t1, "100 ms", "5 ms", None);
        b.set_property(t1, "Deployment_Properties", "Priority", "1");

        // t_short: period=10ms, exec=2ms, priority=2 (lower)
        // Without explicit priority, t_short would be higher priority (shorter period).
        // With explicit priority, t_long (priority=1) preempts t_short (priority=2).
        bind_thread(&mut b, t2, "10 ms", "2 ms", None);
        b.set_property(t2, "Deployment_Properties", "Priority", "2");

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        // t_long (highest priority): R = 5ms <= 100ms deadline => OK
        // t_short (lower priority): R0 = 2ms, R1 = 2 + ceil(2/100)*5 = 2+5 = 7ms
        //   R2 = 2 + ceil(7/100)*5 = 2+5 = 7ms => converged at 7ms <= 10ms => OK
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should have no errors: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(infos.len(), 2, "expected 2 info diagnostics: {:?}", diags);

        // First info should be t_long (higher priority = processed first).
        assert!(
            infos[0].message.contains("t_long"),
            "first thread should be t_long (priority 1): {}",
            infos[0].message,
        );
        assert!(
            infos[0].message.contains("5.00 ms"),
            "t_long response time should be 5ms: {}",
            infos[0].message,
        );

        assert!(
            infos[1].message.contains("t_short"),
            "second thread should be t_short (priority 2): {}",
            infos[1].message,
        );
        assert!(
            infos[1].message.contains("7.00 ms"),
            "t_short response time should be 7ms: {}",
            infos[1].message,
        );
    }

    // ── Test 7: Response time exactly at deadline (boundary) ────────
    #[test]
    fn response_time_exactly_at_deadline() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1]);

        // Single thread: period=10ms, exec=10ms, deadline=10ms => R=C=10ms == deadline
        bind_thread(&mut b, t1, "10 ms", "10 ms", Some("10 ms"));

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        // R = C = 10ms, deadline = 10ms → R <= deadline → Info, not Error
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "exactly at deadline should NOT error: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("response time"))
            .collect();
        assert_eq!(infos.len(), 1, "expected 1 info: {:?}", diags);
    }

    // ── Test 8: Response time 1 unit over deadline ───────────────────
    #[test]
    fn response_time_one_over_deadline() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));

        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1, t2]);

        // t1: period=10ms, exec=6ms (high priority)
        bind_thread(&mut b, t1, "10 ms", "6 ms", None);
        // t2: period=20ms, exec=4ms, deadline=9ms
        // R0=4, R1=4+ceil(4/10)*6=4+6=10 > deadline 9 → miss
        bind_thread(&mut b, t2, "20 ms", "4 ms", Some("9 ms"));

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("t2"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "1 over deadline should produce error: {:?}",
            diags
        );
    }

    // ── Test 9: Unbound threads skipped ──────────────────────────────
    #[test]
    fn unbound_threads_skipped() {
        let (mut b, root, proc) = make_base();
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(
            root,
            vec![
                ComponentInstanceIdx::from_raw(la_arena::RawIdx::from_u32(1)),
                proc,
            ],
        );
        b.set_children(proc, vec![t1]);

        // Set period and exec but NO processor binding
        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");

        let inst = b.build(root);
        let diags = RtaAnalysis.analyze(&inst);

        // Unbound threads go to "__unbound__" which is skipped
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("response time"))
            .collect();
        assert!(
            infos.is_empty(),
            "unbound threads should be skipped: {:?}",
            infos
        );
    }

    // ── Test 10: get_priority helper ─────────────────────────────────
    #[test]
    fn get_priority_parses_correctly() {
        use spar_hir_def::name::PropertyRef;
        use spar_hir_def::properties::PropertyMap;
        use spar_hir_def::properties::PropertyValue;

        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Deployment_Properties")),
                property_name: Name::new("Priority"),
            },
            value: "5".to_string(),
            is_append: false,
        });

        assert_eq!(get_priority(&props), Some(5));
    }

    #[test]
    fn get_priority_missing_returns_none() {
        let props = spar_hir_def::properties::PropertyMap::new();
        assert_eq!(get_priority(&props), None);
    }

    #[test]
    fn get_priority_invalid_value_returns_none() {
        use spar_hir_def::name::PropertyRef;
        use spar_hir_def::properties::PropertyMap;
        use spar_hir_def::properties::PropertyValue;

        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Deployment_Properties")),
                property_name: Name::new("Priority"),
            },
            value: "not_a_number".to_string(),
            is_append: false,
        });

        assert_eq!(get_priority(&props), None);
    }

    // ── Test 11: format_time helper ──────────────────────────────────
    #[test]
    fn format_time_units() {
        assert_eq!(format_time(500), "500 ps");
        assert_eq!(format_time(1_500_000), "1.50 us");
        assert_eq!(format_time(10_000_000_000), "10.00 ms");
        assert_eq!(format_time(2_500_000_000_000), "2.50 sec");
    }
}
