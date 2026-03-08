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
use spar_hir_def::property_value::parse_time_value;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Rate Monotonic scheduling analysis.
pub struct SchedulingAnalysis;

impl Analysis for SchedulingAnalysis {
    fn name(&self) -> &str {
        "scheduling"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
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
                processor_threads.entry(proc_key).or_default().push(ThreadInfo {
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

/// Extract a timing property (Period, Deadline, etc.) in picoseconds.
fn get_timing_property(
    props: &spar_hir_def::properties::PropertyMap,
    name: &str,
) -> Option<u64> {
    // Try with Timing_Properties property set first, then unqualified
    let raw = props
        .get("Timing_Properties", name)
        .or_else(|| props.get("", name))?;
    parse_time_value(raw)
}

/// Extract Compute_Execution_Time in picoseconds.
///
/// This property is typically a range (e.g., "1 ms .. 5 ms"). We take the
/// worst case (max). If it's a single value, we use that.
fn get_execution_time(
    props: &spar_hir_def::properties::PropertyMap,
) -> Option<u64> {
    let raw = props
        .get("Timing_Properties", "Compute_Execution_Time")
        .or_else(|| props.get("", "Compute_Execution_Time"))?;

    // Try range format: "min .. max"
    if let Some((_, max_str)) = raw.split_once("..") {
        return parse_time_value(max_str.trim());
    }

    // Single value
    parse_time_value(raw)
}

/// Extract processor binding target name from property.
fn get_processor_binding(
    props: &spar_hir_def::properties::PropertyMap,
) -> Option<String> {
    let raw = props
        .get("Deployment_Properties", "Actual_Processor_Binding")
        .or_else(|| props.get("", "Actual_Processor_Binding"))?;

    // Parse "reference (name)" or "(reference (name))"
    extract_reference_target(raw).map(|s| s.to_string())
}

/// Extract the target name from a `reference(name)` string.
fn extract_reference_target(val: &str) -> Option<&str> {
    let trimmed = val.trim();
    if let Some(start) = trimmed.find("reference") {
        let after_ref = &trimmed[start + "reference".len()..];
        if let Some(paren_start) = after_ref.find('(') {
            let inner = &after_ref[paren_start + 1..];
            if let Some(paren_end) = inner.find(')') {
                let target = inner[..paren_end].trim();
                if !target.is_empty() {
                    return Some(target);
                }
            }
        }
    }
    None
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
    use spar_hir_def::item_tree::*;
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
            })
        }

        fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
            self.components[parent].children = children;
        }

        fn set_property(
            &mut self,
            comp: ComponentInstanceIdx,
            set: &str,
            name: &str,
            value: &str,
        ) {
            let map = self.property_maps.entry(comp).or_insert_with(PropertyMap::new);
            map.add(PropertyValue {
                name: PropertyRef {
                    property_set: if set.is_empty() { None } else { Some(Name::new(set)) },
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
        b.set_property(t1, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");

        // t2: period=20ms, exec=2ms -> U=0.1
        b.set_property(t2, "Timing_Properties", "Period", "20 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(t2, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // Total U = 0.2, well under RMA bound of 0.828 for 2 tasks
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "schedulable system should have no errors: {:?}", errors);

        let infos: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Info && d.message.contains("utilization")).collect();
        assert!(!infos.is_empty(), "should report utilization info: {:?}", diags);
        assert!(infos[0].message.contains("20.0%"), "utilization should be 20%: {}", infos[0].message);
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
        b.set_property(t1, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");

        // t2: period=10ms, exec=5ms -> U=0.5
        // Total: 1.3 > 1.0
        b.set_property(t2, "Timing_Properties", "Period", "10 ms");
        b.set_property(t2, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(t2, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "should have 1 overload error: {:?}", diags);
        assert!(errors[0].message.contains("overloaded"), "error should mention overload: {}", errors[0].message);
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

        let period_warns: Vec<_> = diags.iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("Period"))
            .collect();
        assert_eq!(period_warns.len(), 1, "should warn about missing Period: {:?}", diags);
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

        let binding_warns: Vec<_> = diags.iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("Actual_Processor_Binding"))
            .collect();
        assert_eq!(binding_warns.len(), 1, "should warn about missing binding: {:?}", diags);
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
        b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "1 ms .. 5 ms");
        b.set_property(t1, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // U = 5/10 = 0.5 = 50%
        let infos: Vec<_> = diags.iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert!(!infos.is_empty(), "should report utilization: {:?}", diags);
        assert!(infos[0].message.contains("50.0%"), "should be 50%: {}", infos[0].message);
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
            b.set_property(t, "Deployment_Properties", "Actual_Processor_Binding", "reference (cpu1)");
        }

        let inst = b.build(root);
        let diags = SchedulingAnalysis.analyze(&inst);

        // U = 0.9, RMA bound for 3 tasks ≈ 0.780
        let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "should not be error (U < 1.0): {:?}", errors);

        let warnings: Vec<_> = diags.iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("RMA bound"))
            .collect();
        assert_eq!(warnings.len(), 1, "should warn about exceeding RMA bound: {:?}", diags);
    }
}
