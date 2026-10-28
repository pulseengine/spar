//! ARINC 653 partition scheduling analysis (DO-297).
//!
//! Checks ARINC 653 constraints for time/space-partitioned real-time
//! operating systems used in avionics:
//!
//! - **ARINC-PROCESSOR-HAS-PARTITIONS**: Processors should contain virtual
//!   processor subcomponents (partitions).
//! - **ARINC-PARTITION-ASSIGNMENT**: Every process must be bound to a virtual
//!   processor (partition).
//! - **ARINC-PARTITION-ISOLATION**: Partitions communicate only through ports.
//!   A port connection between processes bound to different partitions IS
//!   the ARINC 653 inter-partition channel (a data port is a sampling port,
//!   an event data port a queueing port) and passes. Data shared by access
//!   connections from more than one partition bypasses those ports and is an
//!   Error; a pure event port across partitions, which has no ARINC 653
//!   inter-partition service, is a Warning (#504,
//!   REQ-ARINC-ISOLATION-FP-001, DEC-ARINC-ANNEX-VS-OSATE-001).
//! - **ARINC-WINDOW-UTILIZATION**: The sum of virtual processor execution times
//!   on a processor must not exceed the processor's major frame period.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance, SystemOperationMode};
use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind, FeatureKind};
use spar_hir_def::name::Name;

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
        //   Error   — data shared by access connections across partitions
        //             (bypasses the ARINC 653 inter-partition ports)
        //   Warning — processor has no partitions, process not bound to partition,
        //             pure event port connection across partitions
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
        // Resolved from the component that declared the binding, by identity
        // — the resolver `ProcessorGroups` uses (#498,
        // REQ-AADL-BINDING-CONSUMERS-002): `(reference (cpu.p1)) applies to a`
        // names partition `p1` of `cpu`. A binding that resolves to a
        // processor is not a partition, whatever virtual processor shares
        // its name.
        if let Some(bound) =
            spar_hir_def::overlay::actual_processor_binding_with_overlay(instance, idx, None)
        {
            return (instance.component(bound).category == ComponentCategory::VirtualProcessor)
                .then_some(bound);
        }
        // Unresolved from the declarer: look the target up by name, as before.
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

/// The partition a connection endpoint lives in: the partition of the
/// nearest enclosing process (the endpoint itself when it is one), with that
/// process. `None` when the endpoint is in no process, or the process is
/// bound to no partition.
fn endpoint_partition(
    instance: &SystemInstance,
    comp: ComponentInstanceIdx,
) -> Option<(ComponentInstanceIdx, ComponentInstanceIdx)> {
    let process = find_ancestor_of_category(instance, comp, ComponentCategory::Process)?;
    owning_partition(instance, process).map(|vp| (process, vp))
}

/// The kind of `comp`'s feature `name`; `None` when the endpoint names no
/// feature of `comp` (an access connection ends on the data component
/// itself, with an empty feature name).
fn endpoint_feature_kind(
    instance: &SystemInstance,
    comp: ComponentInstanceIdx,
    name: &Name,
) -> Option<FeatureKind> {
    instance
        .component(comp)
        .features
        .iter()
        .map(|&f| &instance.features[f])
        .find(|f| f.name.as_str().eq_ignore_ascii_case(name.as_str()))
        .map(|f| f.kind)
}

/// Whether `kind` is an access feature: a path to a component's memory or
/// code rather than an ARINC 653 port.
fn is_access(kind: Option<FeatureKind>) -> bool {
    matches!(
        kind,
        Some(
            FeatureKind::DataAccess
                | FeatureKind::BusAccess
                | FeatureKind::SubprogramAccess
                | FeatureKind::SubprogramGroupAccess
        )
    )
}

/// **ARINC-PARTITION-ISOLATION** (#504, REQ-ARINC-ISOLATION-FP-001).
///
/// In the ARINC 653 annex mapping, a partition is a process (its address
/// space) bound to a virtual processor (its runtime), and partitions
/// communicate only through ports supervised by the module: an AADL data
/// port connected across processes is an ARINC 653 sampling port, an event
/// data port a queueing port (Delange et al., SIGAda 2009 §3.1, §3.4; the
/// AADLib `arinc653_annex/example_1`, "adapted from the ARINC653 annex
/// document", connects its two partitions exactly so). Such a connection is
/// the approved mechanism and is NOT reported.
///
/// What isolation forbids is a channel that bypasses those ports: shared
/// data, which the mapping confines to the threads of ONE partition (SIGAda
/// 2009 §3.3: buffers, blackboards, events and semaphores are
/// intra-partition; ERTS 2010 §2.3.2: partitions "cannot create covert
/// channels"). A DATA component reached by access connections from more
/// than one partition is an Error. Bus and subprogram access across
/// partitions is not reported: the sources speak of shared data, not of
/// those (DEC-ARINC-ANNEX-VS-OSATE-001).
///
/// A pure event port across partitions is not a covert channel, but ARINC
/// 653 has no inter-partition event service (only sampling and queueing
/// ports), so it is a Warning.
///
/// Endpoints are the ultimate (semantic) ones, and partitions are resolved
/// by identity through [`owning_partition`], so two processes that merely
/// share a name are never confused.
fn check_partition_isolation(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    // Accessed component -> the (process, partition) pairs that reach it, in
    // first-seen order so diagnostics are deterministic.
    type Users = Vec<(ComponentInstanceIdx, ComponentInstanceIdx)>;
    let mut shared: Vec<(ComponentInstanceIdx, Users)> = Vec::new();

    for sem_conn in &instance.semantic_connections {
        let (src_comp, src_feat) = &sem_conn.ultimate_source;
        let (dst_comp, dst_feat) = &sem_conn.ultimate_destination;
        let src_kind = endpoint_feature_kind(instance, *src_comp, src_feat);
        let dst_kind = endpoint_feature_kind(instance, *dst_comp, dst_feat);
        let src_part = endpoint_partition(instance, *src_comp);
        let dst_part = endpoint_partition(instance, *dst_comp);

        let access =
            sem_conn.kind == ConnectionKind::Access || is_access(src_kind) || is_access(dst_kind);
        if access {
            // The accessed component is the end that names no feature of its
            // own (`data access blackboard -> t.req`); else the source.
            let resource = if dst_kind.is_none() && src_kind.is_some() {
                *dst_comp
            } else {
                *src_comp
            };
            // Only shared DATA is an annex violation; bus and subprogram
            // access is not reported (and is no port channel either).
            let data = src_kind == Some(FeatureKind::DataAccess)
                || dst_kind == Some(FeatureKind::DataAccess)
                || instance.component(resource).category == ComponentCategory::Data;
            if !data {
                continue;
            }
            let i = match shared.iter().position(|(r, _)| *r == resource) {
                Some(i) => i,
                None => {
                    shared.push((resource, Vec::new()));
                    shared.len() - 1
                }
            };
            for part in [src_part, dst_part].into_iter().flatten() {
                if !shared[i].1.contains(&part) {
                    shared[i].1.push(part);
                }
            }
            continue;
        }

        // A port connection across partitions is the ARINC 653
        // inter-partition channel; only a pure event port has no service.
        let event_only =
            src_kind == Some(FeatureKind::EventPort) || dst_kind == Some(FeatureKind::EventPort);
        if let (Some((sp, svp)), Some((dp, dvp))) = (src_part, dst_part)
            && svp != dvp
            && event_only
        {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "event port connection '{}' crosses partition boundary: \
                     process '{}' (partition '{}') -> process '{}' (partition '{}'). \
                     ARINC 653 inter-partition communication has sampling ports \
                     (data ports) and queueing ports (event data ports), no event \
                     service (ARINC-PARTITION-ISOLATION)",
                    sem_conn.name,
                    instance.component(sp).name,
                    instance.component(svp).name,
                    instance.component(dp).name,
                    instance.component(dvp).name,
                ),
                path: component_path(instance, *src_comp),
                analysis: "arinc653".to_string(),
            });
        }
    }

    for (resource, users) in shared {
        let mut partitions: Vec<ComponentInstanceIdx> = Vec::new();
        for (_, vp) in &users {
            if !partitions.contains(vp) {
                partitions.push(*vp);
            }
        }
        if partitions.len() < 2 {
            continue;
        }
        let users = users
            .iter()
            .map(|(p, vp)| {
                format!(
                    "process '{}' (partition '{}')",
                    instance.component(*p).name,
                    instance.component(*vp).name
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let comp = instance.component(resource);
        diags.push(AnalysisDiagnostic {
            // ARINC 653 / DO-297 spatial isolation is a hard invariant: data
            // shared across partitions is a channel the module does not
            // supervise. Models that legitimately need it can demote this
            // via `spar analyze --allow arinc-partition-isolation`.
            severity: Severity::Error,
            message: format!(
                "{} '{}' is shared by access connections across partition \
                 boundaries: {}. ARINC 653 partitions communicate only through \
                 sampling and queueing ports; shared data belongs inside one \
                 partition (ARINC-PARTITION-ISOLATION)",
                comp.category, comp.name, users,
            ),
            path: component_path(instance, resource),
            analysis: "arinc653".to_string(),
        });
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
                data_size_bytes: None,
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
                feature_property_maps: Default::default(),
                connection_property_maps: Default::default(),
                emv2_models: Default::default(),
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

    /// Two partitions, each a process contained in its own VP, with one
    /// thread each, and a semantic connection `sender.out1 -> receiver.in1`
    /// over features of the given kinds.
    fn two_partitions(src_kind: FeatureKind, dst_kind: FeatureKind) -> SystemInstance {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("part_a", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("part_b", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("app1", ComponentCategory::Process, Some(vp1));
        let p2 = b.add_component("app2", ComponentCategory::Process, Some(vp2));
        let t1 = b.add_component("sender", ComponentCategory::Thread, Some(p1));
        let t2 = b.add_component("receiver", ComponentCategory::Thread, Some(p2));

        b.add_feature("out1", src_kind, Some(Direction::Out), t1);
        b.add_feature("in1", dst_kind, Some(Direction::In), t2);

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
        inst
    }

    fn isolation_diags(inst: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        Arinc653Analysis
            .analyze(inst)
            .into_iter()
            .filter(|d| d.message.contains("ARINC-PARTITION-ISOLATION"))
            .collect()
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn sampling_port_across_partitions_is_the_approved_channel() {
        // #504: a data port connected across partitions IS an ARINC 653
        // sampling port (SIGAda 2009 §3.4). main reported it as an Error.
        let inst = two_partitions(FeatureKind::DataPort, FeatureKind::DataPort);
        let isolation = isolation_diags(&inst);
        assert!(isolation.is_empty(), "{isolation:?}");
        // The partitions did resolve: neither process is unassigned.
        let diags = Arinc653Analysis.analyze(&inst);
        assert!(
            !diags
                .iter()
                .any(|d| d.message.contains("ARINC-PARTITION-ASSIGNMENT")),
            "{diags:?}"
        );
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn queueing_port_across_partitions_is_the_approved_channel() {
        // An event data port across partitions is an ARINC 653 queueing port.
        let inst = two_partitions(FeatureKind::EventDataPort, FeatureKind::EventDataPort);
        let isolation = isolation_diags(&inst);
        assert!(isolation.is_empty(), "{isolation:?}");
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn event_port_across_partitions_warns_no_inter_partition_service() {
        // ARINC 653 has sampling and queueing ports only; a pure event port
        // across partitions has no service. Warning, not Error.
        let inst = two_partitions(FeatureKind::EventPort, FeatureKind::EventPort);
        let isolation = isolation_diags(&inst);
        assert_eq!(isolation.len(), 1, "{isolation:?}");
        assert_eq!(isolation[0].severity, Severity::Warning);
        assert!(isolation[0].message.contains("event port"));
        assert!(isolation[0].message.contains("part_a"));
        assert!(isolation[0].message.contains("part_b"));
        assert!(isolation[0].message.contains("app1"));
        assert!(isolation[0].message.contains("app2"));
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn event_port_only_on_one_end_still_warns() {
        let inst = two_partitions(FeatureKind::EventPort, FeatureKind::EventDataPort);
        assert_eq!(isolation_diags(&inst).len(), 1);
        let inst = two_partitions(FeatureKind::EventDataPort, FeatureKind::EventPort);
        assert_eq!(isolation_diags(&inst).len(), 1);
    }

    /// Partition A's process holds data `blackboard`; a thread of partition
    /// B reaches it by an access connection.
    fn data_in_a_accessed_from_b(accessor_kind: FeatureKind) -> SystemInstance {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("part_a", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("part_b", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("app1", ComponentCategory::Process, Some(vp1));
        let p2 = b.add_component("app2", ComponentCategory::Process, Some(vp2));
        let d = b.add_component("blackboard", ComponentCategory::Data, Some(p1));
        let t2 = b.add_component("reader", ComponentCategory::Thread, Some(p2));
        b.add_feature("req", accessor_kind, None, t2);

        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp1, vp2]);
        b.set_children(vp1, vec![p1]);
        b.set_children(vp2, vec![p2]);
        b.set_children(p1, vec![d]);
        b.set_children(p2, vec![t2]);

        let mut inst = b.build(root);
        inst.semantic_connections.push(SemanticConnection {
            name: Name::new("cross_access"),
            kind: ConnectionKind::Access,
            ultimate_source: (d, Name::new("")),
            ultimate_destination: (t2, Name::new("req")),
            connection_path: Vec::new(),
        });
        inst
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn data_of_one_partition_accessed_from_another_is_an_error() {
        // Negative control: shared data bypasses the inter-partition ports.
        let inst = data_in_a_accessed_from_b(FeatureKind::DataAccess);
        let isolation = isolation_diags(&inst);
        assert_eq!(isolation.len(), 1, "{isolation:?}");
        assert_eq!(isolation[0].severity, Severity::Error);
        let m = &isolation[0].message;
        assert!(m.contains("'blackboard'"), "{m}");
        assert!(m.contains("part_a") && m.contains("part_b"), "{m}");
        assert!(m.contains("app1") && m.contains("app2"), "{m}");
        assert!(isolation[0].path.iter().any(|n| n.as_str() == "blackboard"));
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn access_is_recognised_by_its_feature_even_when_kind_is_not_access() {
        // A connection whose declared kind is not Access but whose ends are
        // access features (a feature connection) is still shared access.
        for kind in [
            FeatureKind::DataAccess,
            FeatureKind::BusAccess,
            FeatureKind::SubprogramAccess,
            FeatureKind::SubprogramGroupAccess,
        ] {
            let mut inst = data_in_a_accessed_from_b(kind);
            inst.semantic_connections[0].kind = ConnectionKind::Feature;
            let isolation = isolation_diags(&inst);
            assert_eq!(isolation.len(), 1, "{kind:?}: {isolation:?}");
            assert_eq!(isolation[0].severity, Severity::Error);
        }
        // Accessed component at the DESTINATION end is the resource too.
        let mut inst = data_in_a_accessed_from_b(FeatureKind::DataAccess);
        let sc = &mut inst.semantic_connections[0];
        std::mem::swap(&mut sc.ultimate_source, &mut sc.ultimate_destination);
        let isolation = isolation_diags(&inst);
        assert_eq!(isolation.len(), 1, "{isolation:?}");
        assert!(isolation[0].path.iter().any(|n| n.as_str() == "blackboard"));
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn non_access_feature_connection_is_not_shared_access() {
        // A Feature connection between abstract features is not an access
        // path: it is not reported as shared data.
        let mut inst = data_in_a_accessed_from_b(FeatureKind::AbstractFeature);
        inst.semantic_connections[0].kind = ConnectionKind::Feature;
        assert!(isolation_diags(&inst).is_empty());
    }

    /// Threads `t1` (partition part_a) and `t2` (partition part_b), each
    /// with one feature `f` of `kind`; plus a root-level component `res` of
    /// `res_cat`.
    fn two_threads_and_resource(
        kind: FeatureKind,
        res_cat: ComponentCategory,
    ) -> (
        SystemInstance,
        ComponentInstanceIdx,
        ComponentInstanceIdx,
        ComponentInstanceIdx,
    ) {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let vp1 = b.add_component("part_a", ComponentCategory::VirtualProcessor, Some(cpu));
        let vp2 = b.add_component("part_b", ComponentCategory::VirtualProcessor, Some(cpu));
        let p1 = b.add_component("app1", ComponentCategory::Process, Some(vp1));
        let p2 = b.add_component("app2", ComponentCategory::Process, Some(vp2));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(p1));
        let t2 = b.add_component("t2", ComponentCategory::Thread, Some(p2));
        let res = b.add_component("res", res_cat, Some(root));
        b.add_feature("f", kind, None, t1);
        b.add_feature("f", kind, None, t2);
        b.set_children(root, vec![cpu, res]);
        b.set_children(cpu, vec![vp1, vp2]);
        b.set_children(vp1, vec![p1]);
        b.set_children(vp2, vec![p2]);
        b.set_children(p1, vec![t1]);
        b.set_children(p2, vec![t2]);
        (b.build(root), t1, t2, res)
    }

    fn access_sc(
        src: (ComponentInstanceIdx, &str),
        dst: (ComponentInstanceIdx, &str),
    ) -> SemanticConnection {
        SemanticConnection {
            name: Name::new("acc"),
            kind: ConnectionKind::Access,
            ultimate_source: (src.0, Name::new(src.1)),
            ultimate_destination: (dst.0, Name::new(dst.1)),
            connection_path: Vec::new(),
        }
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn root_level_data_shared_by_two_partitions_is_an_error() {
        // The shared data lies in NO process (main skipped it entirely).
        let (mut inst, t1, t2, res) =
            two_threads_and_resource(FeatureKind::DataAccess, ComponentCategory::Data);
        inst.semantic_connections
            .push(access_sc((res, ""), (t1, "f")));
        inst.semantic_connections
            .push(access_sc((res, ""), (t2, "f")));
        let isolation = isolation_diags(&inst);
        assert_eq!(isolation.len(), 1, "{isolation:?}");
        assert_eq!(isolation[0].severity, Severity::Error);
        assert!(isolation[0].message.contains("'res'"));
        // One partition only: not reported.
        let (mut inst, t1, _, res) =
            two_threads_and_resource(FeatureKind::DataAccess, ComponentCategory::Data);
        inst.semantic_connections
            .push(access_sc((res, ""), (t1, "f")));
        assert!(isolation_diags(&inst).is_empty());
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn data_resource_is_recognised_without_access_features() {
        // An Access connection whose ends name no feature: the data component
        // itself makes it shared data.
        let (mut inst, t1, t2, res) =
            two_threads_and_resource(FeatureKind::AbstractFeature, ComponentCategory::Data);
        inst.semantic_connections
            .push(access_sc((res, ""), (t1, "")));
        inst.semantic_connections
            .push(access_sc((res, ""), (t2, "")));
        assert_eq!(isolation_diags(&inst).len(), 1);
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn bus_and_subprogram_access_across_partitions_is_not_reported() {
        // The sources forbid shared DATA; bus or subprogram access across
        // partitions has no annex rule in them (DEC-ARINC-ANNEX-VS-OSATE-001).
        for (kind, cat) in [
            (FeatureKind::BusAccess, ComponentCategory::Bus),
            (FeatureKind::SubprogramAccess, ComponentCategory::Subprogram),
        ] {
            let (mut inst, t1, t2, res) = two_threads_and_resource(kind, cat);
            inst.semantic_connections
                .push(access_sc((res, ""), (t1, "f")));
            inst.semantic_connections
                .push(access_sc((res, ""), (t2, "f")));
            let isolation = isolation_diags(&inst);
            assert!(isolation.is_empty(), "{kind:?}: {isolation:?}");
        }
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn feature_to_feature_data_access_across_partitions_names_the_source() {
        // Both ends name a data access feature: the source is the resource.
        let (mut inst, t1, t2, _) =
            two_threads_and_resource(FeatureKind::DataAccess, ComponentCategory::Data);
        inst.semantic_connections
            .push(access_sc((t1, "f"), (t2, "f")));
        let isolation = isolation_diags(&inst);
        assert_eq!(isolation.len(), 1, "{isolation:?}");
        assert!(isolation[0].message.contains("'t1'"), "{isolation:?}");
        assert!(isolation[0].path.iter().any(|n| n.as_str() == "t1"));
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn one_data_access_end_is_enough_for_shared_data() {
        // Only ONE end is a data access feature, and the resource (the source
        // thread) is not a data component: still shared data.
        for data_end_is_src in [true, false] {
            let (mut inst, t1, t2, _) =
                two_threads_and_resource(FeatureKind::DataAccess, ComponentCategory::Data);
            let other = if data_end_is_src { t2 } else { t1 };
            for (_, f) in inst.features.iter_mut() {
                if f.owner == other {
                    f.kind = FeatureKind::AbstractFeature;
                }
            }
            inst.semantic_connections
                .push(access_sc((t1, "f"), (t2, "f")));
            let isolation = isolation_diags(&inst);
            assert_eq!(isolation.len(), 1, "src={data_end_is_src}: {isolation:?}");
        }
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

    // ── #504: source-level fixtures (real parser + instantiation) ──────

    fn instantiate(src: &str) -> SystemInstance {
        use spar_hir_def::{HirDefDatabase, file_item_tree, resolver::GlobalScope};
        let db = HirDefDatabase::default();
        let file = spar_base_db::SourceFile::new(&db, "arinc504.aadl".to_string(), src.to_string());
        let tree = file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("Arinc504"),
            &Name::new("Top"),
            &Name::new("impl"),
        );
        assert!(
            inst.component_count() > 1,
            "instantiation failed: {:?}",
            inst.diagnostics
        );
        inst
    }

    /// Two partitions modelled as the ARINC 653 annex example does
    /// (AADLib `arinc653_annex/example_1`): each partition is a process
    /// bound to a virtual processor of `cpu` by a DOTTED reference declared
    /// on the enclosing system, and to its own memory segment. They talk over
    /// a sampling port (data port) and a queueing port (event data port).
    /// `@SHARED@` is replaced to add shared data in the negative controls.
    const ANNEX_SHAPE: &str = r#"package Arinc504
public
  data Int
  end Int;
  thread Sender
    features
      sout : out data port Int;
      qout : out event data port Int;
      @SENDER_FEATURES@
  end Sender;
  thread Receiver
    features
      sin : in data port Int;
      qin : in event data port Int;
      @RECEIVER_FEATURES@
  end Receiver;
  process P1
    features
      sout : out data port Int;
      qout : out event data port Int;
      @P1_FEATURES@
  end P1;
  process implementation P1.impl
    subcomponents
      t : thread Sender;
      @P1_SUBS@
    connections
      c1 : port t.sout -> sout;
      c2 : port t.qout -> qout;
      @P1_CONNS@
  end P1.impl;
  process P2
    features
      sin : in data port Int;
      qin : in event data port Int;
      @P2_FEATURES@
  end P2;
  process implementation P2.impl
    subcomponents
      t : thread Receiver;
    connections
      c1 : port sin -> t.sin;
      c2 : port qin -> t.qin;
      @P2_CONNS@
  end P2.impl;
  virtual processor Part
  end Part;
  processor Module
  end Module;
  processor implementation Module.impl
    subcomponents
      part1 : virtual processor Part;
      part2 : virtual processor Part;
  end Module.impl;
  memory Seg
  end Seg;
  memory Main
  end Main;
  memory implementation Main.impl
    subcomponents
      seg1 : memory Seg;
      seg2 : memory Seg;
  end Main.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Module.impl;
      mem : memory Main.impl;
      p1 : process P1.impl;
      p2 : process P2.impl;
      @TOP_SUBS@
    connections
      sampling : port p1.sout -> p2.sin;
      queueing : port p1.qout -> p2.qin;
      @TOP_CONNS@
    properties
      Actual_Processor_Binding => (reference (cpu.part1)) applies to p1;
      Actual_Processor_Binding => (reference (cpu.part2)) applies to p2;
      Actual_Memory_Binding => (reference (mem.seg1)) applies to p1;
      Actual_Memory_Binding => (reference (mem.seg2)) applies to p2;
  end Top.impl;
end Arinc504;
"#;

    fn fill(pairs: &[(&str, &str)]) -> String {
        let mut s = ANNEX_SHAPE.to_string();
        for (k, v) in pairs {
            assert!(s.contains(k), "placeholder {k}");
            s = s.replace(k, v);
        }
        for k in [
            "@SENDER_FEATURES@",
            "@RECEIVER_FEATURES@",
            "@P1_FEATURES@",
            "@P2_FEATURES@",
            "@P1_SUBS@",
            "@P1_CONNS@",
            "@P2_CONNS@",
            "@TOP_SUBS@",
            "@TOP_CONNS@",
        ] {
            s = s.replace(k, "");
        }
        s
    }

    fn arinc_lines(inst: &SystemInstance, tag: &str) -> Vec<AnalysisDiagnostic> {
        Arinc653Analysis
            .analyze(inst)
            .into_iter()
            .filter(|d| d.message.contains(tag))
            .collect()
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn annex_shaped_two_partition_model_has_no_isolation_or_assignment_finding() {
        // #504: the annex's own two-partition pattern. On main every process
        // was "not bound to any virtual processor" (the dotted binding was
        // compared with VP names); with the binding resolved, main's rule
        // then flagged both port connections as Errors.
        let inst = instantiate(&fill(&[]));
        assert!(
            inst.semantic_connections.len() >= 2,
            "the port connections must trace: {:?}",
            inst.semantic_connections.len()
        );
        let assignment = arinc_lines(&inst, "ARINC-PARTITION-ASSIGNMENT");
        assert!(assignment.is_empty(), "{assignment:#?}");
        let isolation = arinc_lines(&inst, "ARINC-PARTITION-ISOLATION");
        assert!(isolation.is_empty(), "{isolation:#?}");
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn data_shared_by_two_partitions_is_still_an_error() {
        // Negative control: a data component of the enclosing system reached
        // by access connections from both partitions bypasses the ports.
        let src = fill(&[
            ("@SENDER_FEATURES@", "w : requires data access Int;"),
            ("@RECEIVER_FEATURES@", "r : requires data access Int;"),
            ("@P1_FEATURES@", "acc : requires data access Int;"),
            ("@P2_FEATURES@", "acc : requires data access Int;"),
            ("@P1_CONNS@", "a1 : data access acc -> t.w;"),
            ("@P2_CONNS@", "a1 : data access acc -> t.r;"),
            ("@TOP_SUBS@", "shared : data Int;"),
            (
                "@TOP_CONNS@",
                "s1 : data access shared -> p1.acc; s2 : data access shared -> p2.acc;",
            ),
        ]);
        let inst = instantiate(&src);
        let isolation = arinc_lines(&inst, "ARINC-PARTITION-ISOLATION");
        assert_eq!(isolation.len(), 1, "{isolation:#?}");
        assert_eq!(isolation[0].severity, Severity::Error);
        let m = &isolation[0].message;
        assert!(m.contains("'shared'"), "{m}");
        assert!(m.contains("'part1'") && m.contains("'part2'"), "{m}");
        assert!(m.contains("'p1'") && m.contains("'p2'"), "{m}");
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn data_of_partition_one_provided_to_partition_two_is_an_error() {
        // Negative control: partition 1 PROVIDES access to its own data and
        // partition 2's thread uses it.
        let src = fill(&[
            ("@RECEIVER_FEATURES@", "r : requires data access Int;"),
            ("@P1_FEATURES@", "give : provides data access Int;"),
            ("@P2_FEATURES@", "acc : requires data access Int;"),
            ("@P1_SUBS@", "bb : data Int;"),
            ("@P1_CONNS@", "a1 : data access bb -> give;"),
            ("@P2_CONNS@", "a1 : data access acc -> t.r;"),
            ("@TOP_CONNS@", "s1 : data access p1.give -> p2.acc;"),
        ]);
        let inst = instantiate(&src);
        let isolation = arinc_lines(&inst, "ARINC-PARTITION-ISOLATION");
        assert_eq!(isolation.len(), 1, "{isolation:#?}");
        assert_eq!(isolation[0].severity, Severity::Error);
        let m = &isolation[0].message;
        assert!(m.contains("'bb'"), "{m}");
        assert!(m.contains("'part1'") && m.contains("'part2'"), "{m}");
    }

    // rivet: verifies REQ-ARINC-ISOLATION-FP-001
    #[test]
    fn data_shared_inside_one_partition_is_not_reported() {
        // The annex's intra-partition blackboard: data shared by threads of
        // ONE partition is the approved mechanism.
        let src = fill(&[
            ("@SENDER_FEATURES@", "w : requires data access Int;"),
            ("@P1_SUBS@", "bb : data Int; t2 : thread Sender;"),
            (
                "@P1_CONNS@",
                "a1 : data access bb -> t.w; a2 : data access bb -> t2.w;",
            ),
        ]);
        let inst = instantiate(&src);
        let isolation = arinc_lines(&inst, "ARINC-PARTITION-ISOLATION");
        assert!(isolation.is_empty(), "{isolation:#?}");
    }
}
