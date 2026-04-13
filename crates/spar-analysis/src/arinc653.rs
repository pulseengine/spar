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

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance, SystemOperationMode};
use spar_hir_def::item_tree::ComponentCategory;

use crate::modal::is_component_active_in_som;
use crate::property_accessors::{
    extract_reference_target, get_execution_time_or_exec, get_timing_property,
};
use crate::{Analysis, AnalysisDiagnostic, ModalAnalysis, Severity, component_path};

/// ARINC 653 partition scheduling analysis.
pub struct Arinc653Analysis;

impl Analysis for Arinc653Analysis {
    fn name(&self) -> &str {
        "arinc653"
    }

    fn as_modal(&self) -> Option<&dyn ModalAnalysis> {
        Some(self)
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — partition window overcommitted (>100% utilization)
        //   Warning — processor has no partitions, process not bound to partition,
        //             direct connection crosses partition boundary
        //   Info    — partition window utilization within budget
        let mut diags = Vec::new();

        check_processor_has_partitions(instance, &mut diags);
        check_partition_assignment(instance, None, &mut diags);
        check_partition_isolation(instance, &mut diags);
        check_window_utilization(instance, None, &mut diags);

        diags
    }
}

impl ModalAnalysis for Arinc653Analysis {
    fn analyze_in_mode(
        &self,
        instance: &SystemInstance,
        som: &SystemOperationMode,
    ) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        check_processor_has_partitions(instance, &mut diags);
        check_partition_assignment(instance, Some(som), &mut diags);
        check_partition_isolation(instance, &mut diags);
        check_window_utilization(instance, Some(som), &mut diags);

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
fn find_component_by_name(instance: &SystemInstance, name: &str) -> Option<ComponentInstanceIdx> {
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
        && let Some(target) = extract_reference_target(raw)
    {
        // Look up the target by name and check if it's a virtual processor
        for (comp_idx, comp) in instance.all_components() {
            if comp.name.as_str().eq_ignore_ascii_case(target)
                && comp.category == ComponentCategory::VirtualProcessor
            {
                return Some(comp_idx);
            }
        }
    }
    None
}

// ── Check functions ─────────────────────────────────────────────────

/// **ARINC-PROCESSOR-HAS-PARTITIONS**: Every processor should contain
/// at least one virtual processor (partition) subcomponent.
fn check_processor_has_partitions(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    let processors = find_components_by_category(instance, ComponentCategory::Processor);

    for proc_idx in processors {
        let proc = instance.component(proc_idx);
        let has_vp = proc.children.iter().any(|&child| {
            instance.component(child).category == ComponentCategory::VirtualProcessor
        });

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
    som: Option<&SystemOperationMode>,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let processes = find_components_by_category(instance, ComponentCategory::Process);

    for proc_idx in processes {
        // Filter by SOM if provided.
        if let Some(som) = som
            && !is_component_active_in_som(instance, proc_idx, som)
        {
            continue;
        }

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
fn check_partition_isolation(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    // Check semantic connections for cross-partition violations
    for sem_conn in &instance.semantic_connections {
        let (src_comp, _) = &sem_conn.ultimate_source;
        let (dst_comp, _) = &sem_conn.ultimate_destination;

        // Walk up to the owning process for each endpoint
        let src_process =
            find_ancestor_of_category(instance, *src_comp, ComponentCategory::Process);
        let dst_process =
            find_ancestor_of_category(instance, *dst_comp, ComponentCategory::Process);

        if let (Some(src_proc), Some(dst_proc)) = (src_process, dst_process) {
            if src_proc == dst_proc {
                // Same process -- no isolation concern
                continue;
            }

            let src_vp = owning_partition(instance, src_proc);
            let dst_vp = owning_partition(instance, dst_proc);

            if let (Some(svp), Some(dvp)) = (src_vp, dst_vp)
                && svp != dvp
            {
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
                        sem_conn.name, src_name, src_vp_name, dst_name, dst_vp_name,
                    ),
                    path: component_path(instance, *src_comp),
                    analysis: "arinc653".to_string(),
                });
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

                        if let (Some(svp), Some(dvp)) = (src_vp, dst_vp)
                            && svp != dvp
                        {
                            // Check if we already reported this via semantic connections
                            // to avoid duplicates
                            let already_reported = diags.iter().any(|d| {
                                d.analysis == "arinc653"
                                    && d.message.contains("ARINC-PARTITION-ISOLATION")
                                    && d.message
                                        .contains(instance.component(src_idx).name.as_str())
                                    && d.message
                                        .contains(instance.component(dst_idx).name.as_str())
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

/// **ARINC-WINDOW-UTILIZATION**: The sum of virtual processor execution
/// times on a single processor must not exceed the processor's major
/// frame period.
fn check_window_utilization(
    instance: &SystemInstance,
    som: Option<&SystemOperationMode>,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let processors = find_components_by_category(instance, ComponentCategory::Processor);

    for proc_idx in processors {
        let proc = instance.component(proc_idx);
        let proc_props = instance.properties_for(proc_idx);

        // Get processor period (major frame)
        let proc_period_ps = get_timing_property(proc_props, "Period");

        // Collect child virtual processors and their execution times,
        // filtering by SOM if provided.
        let vp_children: Vec<ComponentInstanceIdx> = proc
            .children
            .iter()
            .filter(|&&c| instance.component(c).category == ComponentCategory::VirtualProcessor)
            .filter(|&&c| {
                if let Some(som) = som {
                    is_component_active_in_som(instance, c, som)
                } else {
                    true
                }
            })
            .copied()
            .collect();

        if vp_children.is_empty() {
            continue; // Already flagged by ARINC-PROCESSOR-HAS-PARTITIONS
        }

        let mut total_exec_ps: u64 = 0;
        let mut vps_with_exec = 0;

        for &vp_idx in &vp_children {
            let vp_props = instance.properties_for(vp_idx);
            if let Some(exec_ps) = get_execution_time_or_exec(vp_props) {
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
                classifier: None,
                access_kind: None,
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
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
            idx
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
                typed_expr: None,
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
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
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
                d.severity == Severity::Info && d.message.contains("partition window utilization")
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
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
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

    // ── find_ancestor_of_category unit tests ──────────────────────

    #[test]
    fn find_ancestor_of_category_returns_self_when_matching() {
        // Component IS in the right category — should return itself
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc1", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        let result = find_ancestor_of_category(&inst, proc, ComponentCategory::Process);
        assert_eq!(result, Some(proc), "process should match itself");
    }

    #[test]
    fn find_ancestor_of_category_returns_none_when_not_matching() {
        // Component is NOT in the right category and no ancestor matches
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc1", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        // Looking for a VirtualProcessor ancestor — none exists
        let result = find_ancestor_of_category(&inst, proc, ComponentCategory::VirtualProcessor);
        assert_eq!(
            result, None,
            "no VirtualProcessor ancestor should return None"
        );
    }

    #[test]
    fn find_ancestor_of_category_walks_up_hierarchy() {
        // Thread -> Process -> VirtualProcessor: should find VP ancestor
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let proc = b.add_component("proc1", ComponentCategory::Process, Some(vp));
        let thr = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![proc]);
        b.set_children(proc, vec![thr]);

        let inst = b.build(root);
        let result = find_ancestor_of_category(&inst, thr, ComponentCategory::VirtualProcessor);
        assert_eq!(result, Some(vp), "should walk up to VP ancestor");
    }

    // ── owning_partition unit tests ───────────────────────────────

    #[test]
    fn owning_partition_thread_inside_vp() {
        // Thread directly under VirtualProcessor — owning_partition returns VP
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let thr = b.add_component("t1", ComponentCategory::Thread, Some(vp));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![thr]);

        let inst = b.build(root);
        let result = owning_partition(&inst, thr);
        assert_eq!(result, Some(vp), "thread under VP should return VP");
    }

    #[test]
    fn owning_partition_thread_outside_vp() {
        // Thread under System (not VP) with no binding — owning_partition returns None
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let thr = b.add_component("t1", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![thr]);

        let inst = b.build(root);
        let result = owning_partition(&inst, thr);
        assert_eq!(
            result, None,
            "thread not under VP and no binding should return None"
        );
    }

    #[test]
    fn owning_partition_via_binding_to_non_vp_returns_none() {
        // Thread bound to a regular Processor (not VP) — should return None
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let thr = b.add_component("t1", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu, thr]);

        // Bind to processor (not a virtual processor)
        b.set_property(
            thr,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let result = owning_partition(&inst, thr);
        assert_eq!(
            result, None,
            "binding to Processor (not VP) should return None"
        );
    }

    // ── check_partition_isolation: same partition (no warn) ────────

    #[test]
    fn partition_isolation_same_vp_via_semantic_connections_no_warn() {
        // Two processes under the SAME VP with a semantic connection — no warning
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("p1", ComponentCategory::Process, Some(vp));
        let p2 = b.add_component("p2", ComponentCategory::Process, Some(vp));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(p1));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(p2));

        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), t1);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), t2);

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![p1, p2]);
        b.set_children(p1, vec![t1]);
        b.set_children(p2, vec![t2]);

        let mut inst = b.build(root);
        inst.semantic_connections.push(SemanticConnection {
            name: Name::new("sc1"),
            kind: ConnectionKind::Port,
            ultimate_source: (t1, Name::new("out1")),
            ultimate_destination: (t2, Name::new("in1")),
            connection_path: Vec::new(),
        });

        let diags = Arinc653Analysis.analyze(&inst);
        let isolation: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect();
        assert!(
            isolation.is_empty(),
            "same-partition semantic connection should not warn: {:?}",
            isolation
        );
    }

    #[test]
    fn partition_isolation_different_vp_via_semantic_connections_warns() {
        // Two processes under DIFFERENT VPs with a semantic connection — should warn
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("part_a", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("part_b", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("app1", ComponentCategory::Process, Some(vp1));
        let p2 = b.add_component("app2", ComponentCategory::Process, Some(vp2));
        let t1 = b.add_component("sender", ComponentCategory::Thread, Some(p1));
        let t2 = b.add_component("receiver", ComponentCategory::Thread, Some(p2));

        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), t1);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), t2);

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);
        b.set_children(vp1, vec![p1]);
        b.set_children(vp2, vec![p2]);
        b.set_children(p1, vec![t1]);
        b.set_children(p2, vec![t2]);

        let mut inst = b.build(root);
        inst.semantic_connections.push(SemanticConnection {
            name: Name::new("cross_sc"),
            kind: ConnectionKind::Port,
            ultimate_source: (t1, Name::new("out1")),
            ultimate_destination: (t2, Name::new("in1")),
            connection_path: Vec::new(),
        });

        let diags = Arinc653Analysis.analyze(&inst);
        let isolation: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect();
        assert_eq!(
            isolation.len(),
            1,
            "cross-partition semantic connection should warn: {:?}",
            diags
        );
        assert!(isolation[0].message.contains("part_a"));
        assert!(isolation[0].message.contains("part_b"));
        assert!(isolation[0].message.contains("app1"));
        assert!(isolation[0].message.contains("app2"));
    }

    #[test]
    fn partition_isolation_same_process_via_semantic_no_warn() {
        // Both endpoints in the SAME process — no isolation concern (line 188: src_proc == dst_proc)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("p1", ComponentCategory::Process, Some(vp));
        let t1 = b.add_component("sender", ComponentCategory::Thread, Some(p1));
        let t2 = b.add_component("receiver", ComponentCategory::Thread, Some(p1));

        b.add_feature("out1", FeatureKind::DataPort, Some(Direction::Out), t1);
        b.add_feature("in1", FeatureKind::DataPort, Some(Direction::In), t2);

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![p1]);
        b.set_children(p1, vec![t1, t2]);

        let mut inst = b.build(root);
        inst.semantic_connections.push(SemanticConnection {
            name: Name::new("intra_process"),
            kind: ConnectionKind::Port,
            ultimate_source: (t1, Name::new("out1")),
            ultimate_destination: (t2, Name::new("in1")),
            connection_path: Vec::new(),
        });

        let diags = Arinc653Analysis.analyze(&inst);
        let isolation: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect();
        assert!(
            isolation.is_empty(),
            "same-process connection should not warn: {:?}",
            isolation
        );
    }

    // ── check_window_utilization: boundary tests ──────────────────

    #[test]
    fn window_utilization_exactly_at_period_is_info() {
        // Total VP exec time == period exactly (100%) — should be Info, NOT Error
        // Kills `>` → `>=` mutant at line 335
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);

        // Processor period = 100 ms, total VP time = 60 + 40 = 100 ms = 100%
        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "60 ms");
        b.set_property(vp2, "Timing_Properties", "Execution_Time", "40 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert!(
            errors.is_empty(),
            "exactly 100% utilization should NOT be Error (only >100%): {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info && d.message.contains("partition window utilization")
            })
            .collect();
        assert_eq!(infos.len(), 1, "should report info for 100%: {:?}", diags);
        assert!(
            infos[0].message.contains("100.0%"),
            "utilization should be 100%: {}",
            infos[0].message
        );
    }

    #[test]
    fn window_utilization_just_over_period_is_error() {
        // Total VP exec time slightly > period — should be Error
        // Verifies the `>` threshold works at just over 100%
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);

        // Processor period = 100 ms, total VP time = 60 + 41 = 101 ms > 100 ms
        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "60 ms");
        b.set_property(vp2, "Timing_Properties", "Execution_Time", "41 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "101% utilization should be error: {:?}",
            diags
        );
    }

    #[test]
    fn window_utilization_accumulates_multiple_vps() {
        // Ensures `+=` is correct (kills `+=` → `-=` mutant at line 320)
        // 3 VPs with known exec times, verify the sum is correct
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("vp1", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("vp2", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp3 = b.add_component("vp3", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2, vp3]);

        // period = 200 ms, exec = 50+50+50 = 150 ms -> 75%
        b.set_property(cpu, "Timing_Properties", "Period", "200 ms");
        b.set_property(vp1, "Timing_Properties", "Execution_Time", "50 ms");
        b.set_property(vp2, "Timing_Properties", "Execution_Time", "50 ms");
        b.set_property(vp3, "Timing_Properties", "Execution_Time", "50 ms");

        let inst = b.build(root);
        let diags = Arinc653Analysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info && d.message.contains("partition window utilization")
            })
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
        assert!(
            infos[0].message.contains("75.0%"),
            "3 VPs at 50ms each with 200ms period = 75%: {}",
            infos[0].message
        );
        assert!(
            infos[0].message.contains("3 partitions"),
            "should count 3 partitions: {}",
            infos[0].message
        );

        // If -= were used instead of +=, the result would be negative or 0%
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "75% utilization should have no errors: {:?}",
            errors
        );
    }

    // ── ModalAnalysis tests ─────────────────────────────────────

    #[test]
    fn as_modal_returns_some() {
        let analysis = Arinc653Analysis;
        assert!(
            analysis.as_modal().is_some(),
            "Arinc653Analysis should support modal analysis"
        );
    }

    #[test]
    fn modal_partition_assignment_filters_inactive_process() {
        // Two processes, each active in a different mode.
        // In "fast" mode only proc_fast is active. proc_slow is filtered out.
        // proc_fast is unbound so it should produce a warning; proc_slow should not.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc_fast = b.add_component("proc_fast", ComponentCategory::Process, Some(root));
        let proc_slow = b.add_component("proc_slow", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc_fast, proc_slow]);

        b.components[proc_fast].in_modes = vec![Name::new("fast")];
        b.components[proc_slow].in_modes = vec![Name::new("slow")];

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: root,
        });
        let slow_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("slow"),
            is_initial: false,
            owner: root,
        });
        inst.components[root].modes = vec![fast_mode, slow_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(root, fast_mode)],
        };

        let diags = Arinc653Analysis.analyze_in_mode(&inst, &som_fast);
        let assignment_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ASSIGNMENT"))
            .collect();
        // Only proc_fast should be checked (and it's unbound)
        assert_eq!(
            assignment_warnings.len(),
            1,
            "should only warn for active process: {:?}",
            diags
        );
        assert!(
            assignment_warnings[0].message.contains("proc_fast"),
            "warning should be for proc_fast: {}",
            assignment_warnings[0].message
        );
    }

    #[test]
    fn modal_window_utilization_filters_inactive_vp() {
        // Two VPs under a processor, each active in a different mode.
        // In "fast" mode only vp_fast is active, so utilization = 80%.
        // Without filtering, total would be 80+20=100%.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp_fast = b.add_component("vp_fast", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp_slow = b.add_component("vp_slow", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp_fast, vp_slow]);

        b.components[vp_fast].in_modes = vec![Name::new("fast")];
        b.components[vp_slow].in_modes = vec![Name::new("slow")];

        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        b.set_property(vp_fast, "Timing_Properties", "Execution_Time", "80 ms");
        b.set_property(vp_slow, "Timing_Properties", "Execution_Time", "20 ms");

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: cpu,
        });
        let slow_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("slow"),
            is_initial: false,
            owner: cpu,
        });
        inst.components[cpu].modes = vec![fast_mode, slow_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(cpu, fast_mode)],
        };

        let diags = Arinc653Analysis.analyze_in_mode(&inst, &som_fast);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info && d.message.contains("partition window utilization")
            })
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report utilization for fast mode: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("80.0%"),
            "fast mode should show 80%: {}",
            infos[0].message
        );
    }

    #[test]
    fn modal_non_modal_vp_included_in_all_soms() {
        // A non-modal VP (empty in_modes) should be included in every SOM.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp_always =
            b.add_component("vp_always", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp_fast = b.add_component("vp_fast", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp_always, vp_fast]);

        b.components[vp_fast].in_modes = vec![Name::new("fast")];
        // vp_always has empty in_modes -> always active

        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        b.set_property(vp_always, "Timing_Properties", "Execution_Time", "30 ms");
        b.set_property(vp_fast, "Timing_Properties", "Execution_Time", "40 ms");

        let mut inst = b.build(root);
        let fast_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("fast"),
            is_initial: true,
            owner: cpu,
        });
        inst.components[cpu].modes = vec![fast_mode];

        let som_fast = SystemOperationMode {
            name: "fast".to_string(),
            mode_selections: vec![(cpu, fast_mode)],
        };

        let diags = Arinc653Analysis.analyze_in_mode(&inst, &som_fast);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info && d.message.contains("partition window utilization")
            })
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report utilization for fast mode: {:?}",
            diags
        );
        // 30 + 40 = 70 ms / 100 ms = 70%
        assert!(
            infos[0].message.contains("70.0%"),
            "should include both VPs: {}",
            infos[0].message
        );
    }

    #[test]
    fn modal_overcommitted_in_one_som_only() {
        // Window is overcommitted in "heavy" mode but not in "light" mode.
        use crate::ModalAnalysis;

        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp_heavy = b.add_component("vp_heavy", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp_light = b.add_component("vp_light", ComponentCategory::VirtualProcessor, Some(cpu));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp_heavy, vp_light]);

        b.components[vp_heavy].in_modes = vec![Name::new("heavy")];
        b.components[vp_light].in_modes = vec![Name::new("light")];

        b.set_property(cpu, "Timing_Properties", "Period", "100 ms");
        // vp_heavy: 120 ms -> 120% (overcommitted)
        b.set_property(vp_heavy, "Timing_Properties", "Execution_Time", "120 ms");
        // vp_light: 50 ms -> 50% (within budget)
        b.set_property(vp_light, "Timing_Properties", "Execution_Time", "50 ms");

        let mut inst = b.build(root);
        let heavy_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("heavy"),
            is_initial: true,
            owner: cpu,
        });
        let light_mode = inst.mode_instances.alloc(ModeInstance {
            name: Name::new("light"),
            is_initial: false,
            owner: cpu,
        });
        inst.components[cpu].modes = vec![heavy_mode, light_mode];

        let som_heavy = SystemOperationMode {
            name: "heavy".to_string(),
            mode_selections: vec![(cpu, heavy_mode)],
        };
        let som_light = SystemOperationMode {
            name: "light".to_string(),
            mode_selections: vec![(cpu, light_mode)],
        };

        // Heavy mode should be overcommitted
        let diags_heavy = Arinc653Analysis.analyze_in_mode(&inst, &som_heavy);
        let errors: Vec<_> = diags_heavy
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "heavy mode should be overcommitted: {:?}",
            diags_heavy
        );

        // Light mode should be within budget
        let diags_light = Arinc653Analysis.analyze_in_mode(&inst, &som_light);
        let errors_light: Vec<_> = diags_light
            .iter()
            .filter(|d| {
                d.severity == Severity::Error && d.message.contains("ARINC-WINDOW-UTILIZATION")
            })
            .collect();
        assert!(
            errors_light.is_empty(),
            "light mode should be within budget: {:?}",
            diags_light
        );
    }
}
