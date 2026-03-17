//! Rate Monotonic Scheduling analysis (AS5506 timing properties).
//!
//! Performs Rate Monotonic Analysis (RMA) on threads grouped by processor
//! binding. Checks utilization bounds to determine schedulability.
//!
//! # Algorithm
//!
//! For each processor with bound threads:
//! 1. Compute utilization U = Σ(Ci/Ti) where Ci is worst-case execution time
//!    and Ti is the period.
//! 2. If U > 1.0, the processor is overloaded (error).
//! 3. If U ≤ RMA bound n(2^(1/n) - 1), guaranteed schedulable.
//! 4. If RMA bound < U ≤ 1.0, schedulability is uncertain (warning).

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{
    get_execution_time, get_execution_time_range, get_processor_binding, get_timing_property,
};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Rate Monotonic scheduling analysis.
pub struct SchedulingAnalysis;

impl Analysis for SchedulingAnalysis {
    fn name(&self) -> &str {
        "scheduling"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — processor utilization exceeds 100%
        //   Warning — missing Period/Execution_Time/binding, utilization exceeds RMA bound
        //   Info    — processor utilization summary, modal awareness note
        let mut diags = Vec::new();

        // Collect thread timing info and group by processor binding.
        // Key: processor name (or "__unbound__" for threads without binding).
        let mut processor_threads: FxHashMap<String, Vec<ThreadInfo>> = FxHashMap::default();

        for (comp_idx, comp) in instance.all_components() {
            if comp.category != ComponentCategory::Thread {
                continue;
            }

            let path = component_path(instance, comp_idx);
            let props = instance.properties_for(comp_idx);

            // Extract Period
            let period_ps = get_timing_property(props, "Period");
            // Extract Compute_Execution_Time (worst case from range, or single value)
            let exec_ps = get_execution_time(props);

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

            // STPA-REQ-013: Execution time range validation (min <= max, max <= period)
            if let Some((min_ps, max_ps)) = get_execution_time_range(props) {
                if min_ps > max_ps {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "thread '{}' Compute_Execution_Time range has min ({:.3} ms) > max ({:.3} ms)",
                            comp.name,
                            min_ps as f64 / 1_000_000_000.0,
                            max_ps as f64 / 1_000_000_000.0,
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
                if let Some(period) = period_ps
                    && max_ps > period
                {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "thread '{}' worst-case execution time ({:.3} ms) exceeds period ({:.3} ms)",
                            comp.name,
                            max_ps as f64 / 1_000_000_000.0,
                            period as f64 / 1_000_000_000.0,
                        ),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // STPA-REQ-008: Validate that explicit Deadline property is set
            let deadline_ps = get_timing_property(props, "Deadline");
            if period_ps.is_some() && deadline_ps.is_none() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "thread '{}' has Period but no explicit Deadline property \
                         (implicit deadline equals period; set Deadline for constrained-deadline tasks)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Get processor binding
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
                processor_threads
                    .entry(proc_key)
                    .or_default()
                    .push(ThreadInfo {
                        name: comp.name.as_str().to_string(),
                        period_ps: period,
                        exec_ps: exec,
                        comp_idx,
                    });
            }
        }

        // For each processor, compute utilization and check RMA bound
        for (proc_name, threads) in &processor_threads {
            if proc_name == "__unbound__" {
                // Skip unbound threads for RMA check (already warned above)
                continue;
            }

            let n = threads.len();
            if n == 0 {
                continue;
            }

            let utilization: f64 = threads
                .iter()
                .map(|t| t.exec_ps as f64 / t.period_ps as f64)
                .sum();

            let rma_bound = rma_utilization_bound(n);

            // Find processor component for the path
            let proc_path = find_processor_path(instance, proc_name);

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

            // STPA-REQ-001: Cross-check between RMA and EDF schedulability.
            // EDF can schedule any task set with U <= 1.0 (necessary and sufficient
            // for independent, preemptive tasks on a uniprocessor). If RMA says
            // "not schedulable" but EDF says "schedulable", report the discrepancy
            // so the engineer knows switching to EDF would help.
            let edf_schedulable = utilization <= 1.0;
            let rma_schedulable = utilization <= rma_bound;

            if edf_schedulable && !rma_schedulable {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "processor '{}' cross-check: not guaranteed schedulable under RMA \
                         (U={:.1}% > RMA bound {:.1}%) but schedulable under EDF (U <= 100%); \
                         consider EDF scheduling or response-time analysis",
                        proc_name,
                        utilization * 100.0,
                        rma_bound * 100.0,
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Always emit info with utilization
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "processor '{}' utilization: {:.1}% ({} threads, RMA bound: {:.1}%)",
                    proc_name,
                    utilization * 100.0,
                    n,
                    rma_bound * 100.0
                ),
                path: proc_path,
                analysis: self.name().to_string(),
            });
        }

        // STPA-REQ-003: Sensitivity analysis — perturb execution times by +10% and
        // check if schedulability conclusion changes. This reveals fragile task sets
        // with thin margins.
        for (proc_name, threads) in &processor_threads {
            if proc_name == "__unbound__" || threads.is_empty() {
                continue;
            }

            let n = threads.len();
            let rma_bound = rma_utilization_bound(n);
            let nominal_util: f64 = threads
                .iter()
                .map(|t| t.exec_ps as f64 / t.period_ps as f64)
                .sum();
            let perturbed_util = nominal_util * 1.1; // +10% perturbation

            let proc_path = find_processor_path(instance, proc_name);

            // If nominal passes RMA but +10% fails, the margin is thin
            if nominal_util <= rma_bound && perturbed_util > rma_bound {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "processor '{}' sensitivity: a 10% increase in execution times \
                         would exceed the RMA bound ({:.1}% -> {:.1}%, bound {:.1}%); \
                         consider increasing timing margins",
                        proc_name,
                        nominal_util * 100.0,
                        perturbed_util * 100.0,
                        rma_bound * 100.0,
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // If nominal passes EDF (U<=1.0) but +10% fails, the margin is very thin
            if nominal_util <= 1.0 && perturbed_util > 1.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "processor '{}' sensitivity: a 10% increase in execution times \
                         would exceed 100% utilization ({:.1}% -> {:.1}%); \
                         the system has critically thin timing margins",
                        proc_name,
                        nominal_util * 100.0,
                        perturbed_util * 100.0,
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // STPA-REQ-021: Clock drift margin advisory.
            // In multi-rate systems, clock drift can cause period jitter. Warn
            // if utilization is above 80% since drift makes tight systems fragile.
            if nominal_util > 0.8 && nominal_util <= 1.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "processor '{}' clock drift advisory: utilization {:.1}% is above 80%; \
                         clock drift between processors may cause period jitter that \
                         increases effective utilization — consider a timing margin \
                         (SPAR_Properties::Clock_Drift_Margin)",
                        proc_name,
                        nominal_util * 100.0,
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // STPA-REQ-022: Interrupt and context-switch overhead advisory.
            // Warn when overhead is not accounted for in WCET.
            let has_any_overhead_prop = threads.iter().any(|t| {
                let props = instance.properties_for(t.comp_idx);
                // Check for standard or custom overhead properties
                props
                    .get("Timing_Properties", "Context_Switch_Time")
                    .is_some()
                    || props.get("", "Context_Switch_Time").is_some()
                    || props.get("SPAR_Properties", "Interrupt_Overhead").is_some()
                    || props.get("", "Interrupt_Overhead").is_some()
            });

            if !has_any_overhead_prop {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "processor '{}' has {} threads but no Context_Switch_Time or \
                         Interrupt_Overhead properties set; WCET values may underestimate \
                         actual execution time if overhead is not included in \
                         Compute_Execution_Time",
                        proc_name, n,
                    ),
                    path: proc_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // STPA-REQ-023: Shared resource blocking time (priority inversion).
            // Check if any threads have priority inversion protocol properties.
            // If threads share a processor but no concurrency protocol is specified,
            // warn about potential priority inversion.
            if n >= 2 {
                let has_any_protocol = threads.iter().any(|t| {
                    let props = instance.properties_for(t.comp_idx);
                    props.get("Deployment_Properties", "Priority").is_some()
                        || props.get("", "Priority").is_some()
                        || props
                            .get("Deployment_Properties", "Concurrency_Control_Protocol")
                            .is_some()
                        || props.get("", "Concurrency_Control_Protocol").is_some()
                });

                if !has_any_protocol {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "processor '{}' has {} threads but no Priority or \
                             Concurrency_Control_Protocol properties set; if threads \
                             share resources, priority inversion blocking time should \
                             be accounted for in Compute_Execution_Time",
                            proc_name, n,
                        ),
                        path: proc_path,
                        analysis: self.name().to_string(),
                    });
                }
            }
        }

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
    #[allow(dead_code)]
    name: String,
    period_ps: u64,
    exec_ps: u64,
    #[allow(dead_code)]
    comp_idx: ComponentInstanceIdx,
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

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "should have 1 overload error: {:?}", diags);
        assert!(
            errors[0].message.contains("overloaded"),
            "error should mention overload: {}",
            errors[0].message
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

    // ── STPA-REQ-013: Execution time range validation ─────────────

    #[test]
    fn exec_time_min_greater_than_max_errors() {
        // STPA-REQ-013: min > max in Compute_Execution_Time range
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        // min (5ms) > max (1ms) — inverted range
        b.set_property(
            t1,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms .. 1 ms",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let range_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("min"))
            .collect();
        assert_eq!(
            range_errors.len(),
            1,
            "should error on inverted range: {:?}",
            diags
        );
        assert!(
            range_errors[0].message.contains("5.000 ms"),
            "should show min value: {}",
            range_errors[0].message
        );
        assert!(
            range_errors[0].message.contains("1.000 ms"),
            "should show max value: {}",
            range_errors[0].message
        );
    }

    #[test]
    fn exec_time_max_exceeds_period_errors() {
        // STPA-REQ-013: max execution time > period
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        // exec max = 15ms > period = 10ms
        b.set_property(
            t1,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms .. 15 ms",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let period_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("exceeds period"))
            .collect();
        assert_eq!(
            period_errors.len(),
            1,
            "should error on exec > period: {:?}",
            diags
        );
    }

    #[test]
    fn exec_time_valid_range_no_error() {
        // STPA-REQ-013: valid range (1ms..5ms with period 10ms) should not produce range errors
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

        let range_errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && (d.message.contains("min") || d.message.contains("exceeds period"))
            })
            .collect();
        assert!(
            range_errors.is_empty(),
            "valid range should not produce errors: {:?}",
            range_errors
        );
    }

    // ── STPA-REQ-008: Deadline property validation ─────────────────

    #[test]
    fn missing_deadline_property_info() {
        // STPA-REQ-008: Thread with Period but no Deadline should emit Info
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let deadline_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("Deadline"))
            .collect();
        assert_eq!(
            deadline_diags.len(),
            1,
            "should info about missing Deadline: {:?}",
            diags
        );
        assert!(
            deadline_diags[0].message.contains("implicit deadline"),
            "should mention implicit deadline: {}",
            deadline_diags[0].message
        );
    }

    #[test]
    fn explicit_deadline_no_info() {
        // STPA-REQ-008: Thread with both Period and Deadline should not emit Deadline info
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Deadline", "8 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let deadline_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Deadline") && d.message.contains("implicit"))
            .collect();
        assert!(
            deadline_diags.is_empty(),
            "explicit Deadline should suppress info: {:?}",
            deadline_diags
        );
    }

    // ── STPA-REQ-001: RMA vs EDF cross-check ──────────────────────

    #[test]
    fn rma_edf_cross_check_diagnostic() {
        // STPA-REQ-001: When utilization exceeds RMA bound but is under 100%,
        // the cross-check should suggest EDF
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        let t3 = b.add_component("t3", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2, t3]);

        // 3 threads each at U=0.30 -> total 0.90 > RMA bound 0.780 but < 1.0
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

        let cross_check: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("cross-check") && d.message.contains("EDF"))
            .collect();
        assert_eq!(
            cross_check.len(),
            1,
            "should emit RMA/EDF cross-check: {:?}",
            diags
        );
        assert_eq!(cross_check[0].severity, Severity::Info);
    }

    #[test]
    fn rma_edf_no_cross_check_when_schedulable() {
        // When U is within RMA bound, no cross-check needed
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

        let cross_check: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("cross-check"))
            .collect();
        assert!(
            cross_check.is_empty(),
            "schedulable system needs no cross-check: {:?}",
            cross_check
        );
    }

    // ── STPA-REQ-003: Sensitivity analysis ─────────────────────────

    #[test]
    fn sensitivity_warning_thin_rma_margin() {
        // STPA-REQ-003: System with U near RMA bound should warn about sensitivity
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        // Single task: RMA bound = 1.0 (100%), U = 0.95 (95%)
        // After +10%: U = 1.045 > 1.0 -> sensitivity warning for EDF
        b.set_property(t1, "Timing_Properties", "Period", "100 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "95 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let sensitivity: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("sensitivity"))
            .collect();
        assert!(
            !sensitivity.is_empty(),
            "should warn about thin margin: {:?}",
            diags
        );
    }

    #[test]
    fn no_sensitivity_warning_for_ample_margin() {
        // STPA-REQ-003: System with ample margin should not trigger sensitivity warning
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        // Single task: U = 10%, +10% = 11% — still well within bounds
        b.set_property(t1, "Timing_Properties", "Period", "100 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "10 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let sensitivity: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("sensitivity"))
            .collect();
        assert!(
            sensitivity.is_empty(),
            "ample margin should not trigger sensitivity: {:?}",
            sensitivity
        );
    }

    // ── STPA-REQ-021: Clock drift advisory ─────────────────────────

    #[test]
    fn clock_drift_advisory_high_utilization() {
        // STPA-REQ-021: Advisory when utilization > 80%
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        // U = 85%
        b.set_property(t1, "Timing_Properties", "Period", "100 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "85 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let drift: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("clock drift"))
            .collect();
        assert_eq!(
            drift.len(),
            1,
            "should emit clock drift advisory at 85%: {:?}",
            diags
        );
        assert_eq!(drift[0].severity, Severity::Info);
    }

    #[test]
    fn no_clock_drift_advisory_low_utilization() {
        // No clock drift advisory at low utilization
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        // U = 20%
        b.set_property(t1, "Timing_Properties", "Period", "100 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "20 ms");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let drift: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("clock drift"))
            .collect();
        assert!(
            drift.is_empty(),
            "low utilization should not trigger drift advisory: {:?}",
            drift
        );
    }

    // ── STPA-REQ-022: Interrupt overhead advisory ──────────────────

    #[test]
    fn interrupt_overhead_advisory_no_properties() {
        // STPA-REQ-022: Warn when no Context_Switch_Time/Interrupt_Overhead set
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

        let overhead: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Context_Switch_Time"))
            .collect();
        assert_eq!(
            overhead.len(),
            1,
            "should advisory about missing overhead: {:?}",
            diags
        );
        assert_eq!(overhead[0].severity, Severity::Info);
    }

    #[test]
    fn no_interrupt_overhead_advisory_with_property() {
        // STPA-REQ-022: No advisory when Context_Switch_Time is set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1]);

        b.set_property(t1, "Timing_Properties", "Period", "10 ms");
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(t1, "Timing_Properties", "Context_Switch_Time", "50 us");
        b.set_property(
            t1,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let overhead: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Context_Switch_Time") && d.message.contains("overhead"))
            .collect();
        assert!(
            overhead.is_empty(),
            "Context_Switch_Time set — no advisory: {:?}",
            overhead
        );
    }

    // ── STPA-REQ-023: Resource contention / priority inversion ─────

    #[test]
    fn priority_inversion_advisory_no_protocol() {
        // STPA-REQ-023: Warn when multiple threads on same processor without Priority
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        for t in [t1, t2] {
            b.set_property(t, "Timing_Properties", "Period", "10 ms");
            b.set_property(t, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(
                t,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                "reference (cpu1)",
            );
        }

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let pi_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("priority inversion"))
            .collect();
        assert_eq!(
            pi_diags.len(),
            1,
            "should advisory about priority inversion: {:?}",
            diags
        );
        assert_eq!(pi_diags[0].severity, Severity::Info);
    }

    #[test]
    fn no_priority_inversion_advisory_with_priority() {
        // STPA-REQ-023: No advisory when Priority property is set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(proc, vec![t1, t2]);

        for (i, t) in [t1, t2].iter().enumerate() {
            b.set_property(*t, "Timing_Properties", "Period", "10 ms");
            b.set_property(*t, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(
                *t,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                "reference (cpu1)",
            );
            b.set_property(
                *t,
                "Deployment_Properties",
                "Priority",
                &format!("{}", i + 1),
            );
        }

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let pi_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("priority inversion"))
            .collect();
        assert!(
            pi_diags.is_empty(),
            "Priority set — no advisory: {:?}",
            pi_diags
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
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "should not be error (U < 1.0): {:?}",
            errors
        );

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("RMA bound"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "should warn about exceeding RMA bound: {:?}",
            diags
        );
    }
}
