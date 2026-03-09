//! ARINC 653 partition scheduling analysis (DO-297).
//!
//! Checks ARINC 653 constraints for time/space-partitioned real-time
//! operating systems used in avionics:
//!
//! - **ARINC-PROCESSOR-HAS-PARTITIONS**: Processors should contain virtual
//!   processor subcomponents (partitions).
//! - **ARINC-PARTITION-ASSIGNMENT**: Every process must be bound to a virtual
//!   processor (partition).
//! - **ARINC-PARTITION-ISOLATION**: Processes under different virtual processors
//!   should not share direct connections without an inter-partition mechanism.
//! - **ARINC-WINDOW-UTILIZATION**: The sum of virtual processor execution times
//!   on a processor must not exceed the processor's major frame period.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::property_value::parse_time_value;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// ARINC 653 partition scheduling analysis.
pub struct Arinc653Analysis;

impl Analysis for Arinc653Analysis {
    fn name(&self) -> &str {
        "arinc653"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        check_processor_has_partitions(instance, &mut diags);
        check_partition_assignment(instance, &mut diags);
        check_partition_isolation(instance, &mut diags);
        check_window_utilization(instance, &mut diags);

        diags
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Find all components matching the given category.
fn find_components_by_category(
    instance: &SystemInstance,
    category: ComponentCategory,
) -> Vec<ComponentInstanceIdx> {
    instance
        .all_components()
        .filter(|(_, c)| c.category == category)
        .map(|(idx, _)| idx)
        .collect()
}

/// Find a component by name anywhere in the instance hierarchy.
fn find_component_by_name(
    instance: &SystemInstance,
    name: &str,
) -> Option<ComponentInstanceIdx> {
    instance
        .all_components()
        .find(|(_, c)| c.name.as_str().eq_ignore_ascii_case(name))
        .map(|(idx, _)| idx)
}

/// Find the nearest ancestor (or the component itself) of the given
/// category. Returns `None` if no such ancestor exists.
fn find_ancestor_of_category(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
    category: ComponentCategory,
) -> Option<ComponentInstanceIdx> {
    let mut current = Some(idx);
    while let Some(ci) = current {
        if instance.component(ci).category == category {
            return Some(ci);
        }
        current = instance.component(ci).parent;
    }
    None
}

/// For a given component, determine which virtual processor (partition)
/// it is bound to by walking up the hierarchy. Returns the first
/// VirtualProcessor ancestor, if any.
fn owning_partition(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
) -> Option<ComponentInstanceIdx> {
    let mut current = instance.component(idx).parent;
    while let Some(ci) = current {
        if instance.component(ci).category == ComponentCategory::VirtualProcessor {
            return Some(ci);
        }
        current = instance.component(ci).parent;
    }
    // Also check explicit binding property
    let props = instance.properties_for(idx);
    if let Some(raw) = props
        .get("Deployment_Properties", "Actual_Processor_Binding")
        .or_else(|| props.get("", "Actual_Processor_Binding"))
    {
        if let Some(target) = extract_reference_target(raw) {
            // Look up the target by name and check if it's a virtual processor
            for (comp_idx, comp) in instance.all_components() {
                if comp.name.as_str().eq_ignore_ascii_case(target)
                    && comp.category == ComponentCategory::VirtualProcessor
                {
                    return Some(comp_idx);
                }
            }
        }
    }
    None
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

// ── Check functions ─────────────────────────────────────────────────

/// **ARINC-PROCESSOR-HAS-PARTITIONS**: Every processor should contain
/// at least one virtual processor (partition) subcomponent.
fn check_processor_has_partitions(
    instance: &SystemInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let processors = find_components_by_category(instance, ComponentCategory::Processor);

    for proc_idx in processors {
        let proc = instance.component(proc_idx);
        let has_vp = proc
            .children
            .iter()
            .any(|&child| instance.component(child).category == ComponentCategory::VirtualProcessor);

        if !has_vp {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "processor '{}' has no virtual processor (partition) subcomponents \
                     (ARINC-PROCESSOR-HAS-PARTITIONS)",
                    proc.name
                ),
                path: component_path(instance, proc_idx),
                analysis: "arinc653".to_string(),
            });
        }
    }
}

/// **ARINC-PARTITION-ASSIGNMENT**: Every process subcomponent must be
/// bound to a virtual processor (partition), either by containment
/// hierarchy or explicit binding property.
fn check_partition_assignment(
    instance: &SystemInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let processes = find_components_by_category(instance, ComponentCategory::Process);

    for proc_idx in processes {
        if owning_partition(instance, proc_idx).is_none() {
            let comp = instance.component(proc_idx);
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "process '{}' is not bound to any virtual processor (partition) \
                     (ARINC-PARTITION-ASSIGNMENT)",
                    comp.name
                ),
                path: component_path(instance, proc_idx),
                analysis: "arinc653".to_string(),
            });
        }
    }
}

/// **ARINC-PARTITION-ISOLATION**: Processes bound to different virtual
/// processors should not share direct port connections. Connections
/// between different partitions must go through an inter-partition
/// communication mechanism.
fn check_partition_isolation(
    instance: &SystemInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // Check semantic connections for cross-partition violations
    for sem_conn in &instance.semantic_connections {
        let (src_comp, _) = &sem_conn.ultimate_source;
        let (dst_comp, _) = &sem_conn.ultimate_destination;

        // Walk up to the owning process for each endpoint
        let src_process = find_ancestor_of_category(
            instance,
            *src_comp,
            ComponentCategory::Process,
        );
        let dst_process = find_ancestor_of_category(
            instance,
            *dst_comp,
            ComponentCategory::Process,
        );

        if let (Some(src_proc), Some(dst_proc)) = (src_process, dst_process) {
            if src_proc == dst_proc {
                // Same process -- no isolation concern
                continue;
            }

            let src_vp = owning_partition(instance, src_proc);
            let dst_vp = owning_partition(instance, dst_proc);

            if let (Some(svp), Some(dvp)) = (src_vp, dst_vp) {
                if svp != dvp {
                    let src_name = instance.component(src_proc).name.as_str();
                    let dst_name = instance.component(dst_proc).name.as_str();
                    let src_vp_name = instance.component(svp).name.as_str();
                    let dst_vp_name = instance.component(dvp).name.as_str();

                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "direct connection '{}' crosses partition boundary: \
                             process '{}' (partition '{}') -> process '{}' (partition '{}'). \
                             Inter-partition communication should use approved mechanisms \
                             (ARINC-PARTITION-ISOLATION)",
                            sem_conn.name,
                            src_name,
                            src_vp_name,
                            dst_name,
                            dst_vp_name,
                        ),
                        path: component_path(instance, *src_comp),
                        analysis: "arinc653".to_string(),
                    });
                }
            }
        }
    }

    // Also check connection instances at the declarative level
    for (_idx, comp) in instance.all_components() {
        for &conn_idx in &comp.connections {
            let conn = &instance.connections[conn_idx];

            // We need both endpoints to reference subcomponents
            let src_sub = conn.src.as_ref().and_then(|e| e.subcomponent.as_ref());
            let dst_sub = conn.dst.as_ref().and_then(|e| e.subcomponent.as_ref());

            if let (Some(src_name), Some(dst_name)) = (src_sub, dst_sub) {
                // Find the referenced components by name anywhere in the hierarchy
                let src_comp_idx = find_component_by_name(instance, src_name.as_str());
                let dst_comp_idx = find_component_by_name(instance, dst_name.as_str());

                if let (Some(src_idx), Some(dst_idx)) = (src_comp_idx, dst_comp_idx) {
                    let src_cat = instance.component(src_idx).category;
                    let dst_cat = instance.component(dst_idx).category;

                    // Only check if both endpoints are processes
                    if src_cat == ComponentCategory::Process
                        && dst_cat == ComponentCategory::Process
                    {
                        let src_vp = owning_partition(instance, src_idx);
                        let dst_vp = owning_partition(instance, dst_idx);

                        if let (Some(svp), Some(dvp)) = (src_vp, dst_vp) {
                            if svp != dvp {
                                // Check if we already reported this via semantic connections
                                // to avoid duplicates
                                let already_reported = diags.iter().any(|d| {
                                    d.analysis == "arinc653"
                                        && d.message.contains("ARINC-PARTITION-ISOLATION")
                                        && d.message.contains(
                                            instance.component(src_idx).name.as_str(),
                                        )
                                        && d.message.contains(
                                            instance.component(dst_idx).name.as_str(),
                                        )
                                });

                                if !already_reported {
                                    let src_vp_name = instance.component(svp).name.as_str();
                                    let dst_vp_name = instance.component(dvp).name.as_str();

                                    diags.push(AnalysisDiagnostic {
                                        severity: Severity::Warning,
                                        message: format!(
                                            "direct connection '{}' crosses partition boundary: \
                                             process '{}' (partition '{}') -> process '{}' (partition '{}'). \
                                             Inter-partition communication should use approved mechanisms \
                                             (ARINC-PARTITION-ISOLATION)",
                                            conn.name,
                                            instance.component(src_idx).name,
                                            src_vp_name,
                                            instance.component(dst_idx).name,
                                            dst_vp_name,
                                        ),
                                        path: component_path(instance, conn.owner),
                                        analysis: "arinc653".to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// **ARINC-WINDOW-UTILIZATION**: The sum of virtual processor execution
/// times on a single processor must not exceed the processor's major
/// frame period.
fn check_window_utilization(
    instance: &SystemInstance,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let processors = find_components_by_category(instance, ComponentCategory::Processor);

    for proc_idx in processors {
        let proc = instance.component(proc_idx);
        let proc_props = instance.properties_for(proc_idx);

        // Get processor period (major frame)
        let proc_period_ps = get_timing_property(proc_props, "Period");

        // Collect child virtual processors and their execution times
        let vp_children: Vec<ComponentInstanceIdx> = proc
            .children
            .iter()
            .filter(|&&c| instance.component(c).category == ComponentCategory::VirtualProcessor)
            .copied()
            .collect();

        if vp_children.is_empty() {
            continue; // Already flagged by ARINC-PROCESSOR-HAS-PARTITIONS
        }

        let mut total_exec_ps: u64 = 0;
        let mut vps_with_exec = 0;

        for &vp_idx in &vp_children {
            let vp_props = instance.properties_for(vp_idx);
            if let Some(exec_ps) = get_execution_time(vp_props) {
                total_exec_ps += exec_ps;
                vps_with_exec += 1;
            }
        }

        if vps_with_exec == 0 {
            continue; // No VP has execution time -- nothing to check
        }

        if let Some(period_ps) = proc_period_ps {
            if period_ps == 0 {
                continue;
            }

            let utilization = total_exec_ps as f64 / period_ps as f64;

            if utilization > 1.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "processor '{}' partition window overcommitted: total VP execution \
                         time {:.1}% of major frame period ({} partitions) \
                         (ARINC-WINDOW-UTILIZATION)",
                        proc.name,
                        utilization * 100.0,
                        vps_with_exec,
                    ),
                    path: component_path(instance, proc_idx),
                    analysis: "arinc653".to_string(),
                });
            } else {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "processor '{}' partition window utilization: {:.1}% \
                         ({} partitions, major frame OK)",
                        proc.name,
                        utilization * 100.0,
                        vps_with_exec,
                    ),
                    path: component_path(instance, proc_idx),
                    analysis: "arinc653".to_string(),
                });
            }
        }
    }
}

/// Extract a timing property (Period, Execution_Time, etc.) in picoseconds.
fn get_timing_property(
    props: &spar_hir_def::properties::PropertyMap,
    name: &str,
) -> Option<u64> {
    let raw = props
        .get("Timing_Properties", name)
        .or_else(|| props.get("", name))?;
    parse_time_value(raw)
}

/// Extract Execution_Time in picoseconds.
///
/// This property is typically a range (e.g., "1 ms .. 5 ms"). We take the
/// worst case (max). If it's a single value, we use that.
fn get_execution_time(
    props: &spar_hir_def::properties::PropertyMap,
) -> Option<u64> {
    let raw = props
        .get("Timing_Properties", "Execution_Time")
        .or_else(|| props.get("", "Execution_Time"))
        .or_else(|| props.get("Timing_Properties", "Compute_Execution_Time"))
        .or_else(|| props.get("", "Compute_Execution_Time"))?;

    // Try range format: "min .. max"
    if let Some((_, max_str)) = raw.split_once("..") {
        return parse_time_value(max_str.trim());
    }

    // Single value
    parse_time_value(raw)
}

// ── Tests ───────────────────────────────────────────────────────────

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
                array_index: None,
            })
        }

        fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
            self.components[parent].children = children;
        }

        fn add_feature(
            &mut self,
            name: &str,
            kind: FeatureKind,
            direction: Option<Direction>,
            owner: ComponentInstanceIdx,
        ) -> FeatureInstanceIdx {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction,
                owner,
                array_index: None,
            });
            self.components[owner].features.push(idx);
            idx
        }

        fn add_connection(
            &mut self,
            name: &str,
            kind: ConnectionKind,
            owner: ComponentInstanceIdx,
            src: Option<ConnectionEnd>,
            dst: Option<ConnectionEnd>,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind,
                is_bidirectional: false,
                owner,
                src,
                dst,
            });
            self.components[owner].connections.push(idx);
            idx
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

    // ── ARINC-PROCESSOR-HAS-PARTITIONS ──────────────────────────

    #[test]
    fn processor_with_virtual_processor_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let partition_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PROCESSOR-HAS-PARTITIONS"))
            .collect();
        assert!(
            partition_warnings.is_empty(),
            "processor with VP should not warn: {:?}",
            partition_warnings
        );
    }

    #[test]
    fn processor_without_virtual_processor_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        b.set_children(root, vec![cpu]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let partition_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PROCESSOR-HAS-PARTITIONS"))
            .collect();
        assert_eq!(
            partition_warnings.len(),
            1,
            "processor without VP should warn: {:?}",
            diags
        );
        assert_eq!(partition_warnings[0].severity, Severity::Warning);
    }

    // ── ARINC-PARTITION-ASSIGNMENT ──────────────────────────────

    #[test]
    fn process_under_virtual_processor_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let proc = b.add_component("app", ComponentCategory::Process, Some(vp));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![proc]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let assignment_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ASSIGNMENT"))
            .collect();
        assert!(
            assignment_warnings.is_empty(),
            "process under VP should not warn: {:?}",
            assignment_warnings
        );
    }

    #[test]
    fn process_not_under_virtual_processor_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("app", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let assignment_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ASSIGNMENT"))
            .collect();
        assert_eq!(
            assignment_warnings.len(),
            1,
            "unbound process should warn: {:?}",
            diags
        );
        assert_eq!(assignment_warnings[0].severity, Severity::Warning);
    }

    #[test]
    fn process_bound_via_property_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let proc = b.add_component("app", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![cpu, proc]);
        b.set_children(cpu, vec![vp]);

        b.set_property(
            proc,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (vp1)",
        );

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let assignment_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ASSIGNMENT"))
            .collect();
        assert!(
            assignment_warnings.is_empty(),
            "process bound via property should not warn: {:?}",
            assignment_warnings
        );
    }

    // ── ARINC-PARTITION-ISOLATION ───────────────────────────────

    #[test]
    fn same_partition_no_isolation_warning() {
        // Two processes under the same VP connected directly -- no violation
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("p1", ComponentCategory::Process, Some(vp));
        let p2 = b.add_component("p2", ComponentCategory::Process, Some(vp));

        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), p1);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), p2);

        b.add_connection(
            "c1",
            ConnectionKind::Port,
            vp,
            Some(ConnectionEnd {
                subcomponent: Some(Name::new("p1")),
                feature: Name::new("out1"),
            }),
            Some(ConnectionEnd {
                subcomponent: Some(Name::new("p2")),
                feature: Name::new("in1"),
            }),
        );

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![p1, p2]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let isolation_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect();
        assert!(
            isolation_warnings.is_empty(),
            "same-partition connection should not warn: {:?}",
            isolation_warnings
        );
    }

    #[test]
    fn different_partitions_direct_connection_warns() {
        // Two processes under different VPs connected directly -- violation
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("nav_app", ComponentCategory::Process, Some(vp1));
        let p2 = b.add_component("display_app", ComponentCategory::Process, Some(vp2));

        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), p1);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), p2);

        // Connection owned by root crossing partition boundary
        b.add_connection(
            "cross_conn",
            ConnectionKind::Port,
            root,
            Some(ConnectionEnd {
                subcomponent: Some(Name::new("nav_app")),
                feature: Name::new("out1"),
            }),
            Some(ConnectionEnd {
                subcomponent: Some(Name::new("display_app")),
                feature: Name::new("in1"),
            }),
        );

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);
        b.set_children(vp1, vec![p1]);
        b.set_children(vp2, vec![p2]);

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let isolation_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect();
        assert_eq!(
            isolation_warnings.len(),
            1,
            "cross-partition connection should warn: {:?}",
            diags
        );
        assert!(isolation_warnings[0].message.contains("nav_app"));
        assert!(isolation_warnings[0].message.contains("display_app"));
        assert!(isolation_warnings[0].message.contains("vp1"));
        assert!(isolation_warnings[0].message.contains("vp2"));
    }

    // ── ARINC-WINDOW-UTILIZATION ────────────────────────────────

    #[test]
    fn window_utilization_within_budget_info() {
        // 3 VPs with Execution_Time summing to less than processor Period
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp3 = b.add_component("vp3", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2, vp3]);

        // Processor major frame = 100 ms
        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        // VP execution times: 20 + 30 + 20 = 70 ms (70% utilization)
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "20 ms");
        b.set_property(vp2, "Timing_Properties", "Execution_Time", "30 ms");
        b.set_property(vp3, "Timing_Properties", "Execution_Time", "20 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert!(
            errors.is_empty(),
            "within-budget should not error: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info
                    && d.message.contains("partition window utilization")
            })
            .collect();
        assert!(
            !infos.is_empty(),
            "should report utilization info: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("70.0%"),
            "utilization should be 70%: {}",
            infos[0].message
        );
    }

    #[test]
    fn window_utilization_exceeds_period_error() {
        // VPs exceeding processor Period
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);

        // Processor major frame = 50 ms
        b.set_property(cpu, "Timing_Properties", "Period", "50 ms");
        // VP execution times: 30 + 30 = 60 ms > 50 ms (120%)
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "30 ms");
        b.set_property(vp2, "Timing_Properties", "Execution_Time", "30 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "overcommitted windows should error: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("120.0%"),
            "utilization should be 120%: {}",
            errors[0].message
        );
    }

    #[test]
    fn window_utilization_no_exec_time_skipped() {
        // VPs without Execution_Time should not cause errors
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1]);

        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        // No Execution_Time on vp1

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let util_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("partition window"))
            .collect();
        assert!(
            util_diags.is_empty(),
            "VPs without exec time should not produce utilization diag: {:?}",
            util_diags
        );
    }

    #[test]
    fn window_utilization_no_processor_period_skipped() {
        // Processor without Period should not produce utilization diagnostics
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1]);

        // No Period on processor
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "10 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let util_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("partition window"))
            .collect();
        assert!(
            util_diags.is_empty(),
            "no-period processor should not produce utilization diag: {:?}",
            util_diags
        );
    }
}
