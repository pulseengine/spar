//! Bus bandwidth analysis (AS5506 communication resource budgets).
//!
//! Computes bandwidth demand on each bus by examining all connections
//! bound to it via `Actual_Connection_Binding`, then comparing the
//! aggregate demand against the bus's declared bandwidth capacity.
//!
//! # Algorithm
//!
//! For each bus component in the instance model:
//! 1. Find connections whose owning component has `Actual_Connection_Binding`
//!    pointing to this bus.
//! 2. For each bound connection, compute bandwidth demand:
//!    - `Data_Size` of the data type being transferred (from the source port's
//!      classifier or the source component's `Data_Size` property).
//!    - `Period` of the source thread (message rate = 1 / Period).
//!    - demand = Data_Size / Period (bits per picosecond, converted to bps).
//! 3. Sum all demands on the bus.
//! 4. Compare against the bus's `Bandwidth` property (`SEI::Bandwidth` or
//!    `Communication_Properties::Bandwidth`) or `Data_Rate`.
//! 5. Error if demand > capacity.
//! 6. Warning if utilization > 80%.
//! 7. Info with utilization summary.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{extract_reference_target, get_size_property, get_timing_property};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Bus bandwidth analysis.
pub struct BusBandwidthAnalysis;

impl Analysis for BusBandwidthAnalysis {
    fn name(&self) -> &str {
        "bus_bandwidth"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Collect all buses and their capacities.
        let buses: Vec<(ComponentInstanceIdx, &str)> = instance
            .all_components()
            .filter(|(_, c)| {
                matches!(
                    c.category,
                    ComponentCategory::Bus | ComponentCategory::VirtualBus
                )
            })
            .map(|(idx, c)| (idx, c.name.as_str()))
            .collect();

        if buses.is_empty() {
            return diags;
        }

        // Build a map: bus_name (lowercase) -> bus_idx for quick lookup.
        let bus_map: FxHashMap<String, ComponentInstanceIdx> = buses
            .iter()
            .map(|&(idx, name)| (name.to_lowercase(), idx))
            .collect();

        // For each bus, accumulate bandwidth demands from bound connections.
        for &(bus_idx, _bus_name) in &buses {
            let bus_comp = instance.component(bus_idx);
            let bus_props = instance.properties_for(bus_idx);
            let bus_path = component_path(instance, bus_idx);

            // Get bus bandwidth capacity.
            // Try SEI::Bandwidth, Communication_Properties::Bandwidth, then Data_Rate.
            let capacity_bps = get_bandwidth_capacity(bus_props);

            if capacity_bps.is_none() {
                // No bandwidth property -- nothing to check against.
                continue;
            }
            let capacity_bps = capacity_bps.unwrap();

            // Find all connections bound to this bus and compute demands.
            let mut total_demand_bps: f64 = 0.0;
            let mut bound_connections: Vec<(String, f64)> = Vec::new();

            // Walk all components looking for Actual_Connection_Binding referencing this bus.
            for (comp_idx, _comp) in instance.all_components() {
                let comp_props = instance.properties_for(comp_idx);

                let binding = comp_props
                    .get("Deployment_Properties", "Actual_Connection_Binding")
                    .or_else(|| comp_props.get("", "Actual_Connection_Binding"));

                let bound_to_this_bus = match binding {
                    Some(val) => {
                        if let Some(target) = extract_reference_target(val) {
                            target.eq_ignore_ascii_case(bus_comp.name.as_str())
                        } else {
                            // Fallback: substring match for non-reference format
                            val.to_lowercase()
                                .contains(&bus_comp.name.as_str().to_lowercase())
                        }
                    }
                    None => false,
                };

                if !bound_to_this_bus {
                    continue;
                }

                // This component has connections bound to this bus.
                // Walk its connection instances and compute demand for each.
                let comp = instance.component(comp_idx);
                for &conn_idx in &comp.connections {
                    let conn = &instance.connections[conn_idx];

                    // Try to resolve the source subcomponent to get Data_Size and Period.
                    let src_sub_idx = conn
                        .src
                        .as_ref()
                        .and_then(|end| end.subcomponent.as_ref())
                        .and_then(|sub_name| {
                            find_child_by_name(instance, comp_idx, sub_name.as_str())
                        });

                    let demand = compute_connection_demand(instance, src_sub_idx, &bus_map);
                    if demand > 0.0 {
                        bound_connections.push((conn.name.as_str().to_string(), demand));
                        total_demand_bps += demand;
                    }
                }
            }

            if bound_connections.is_empty() {
                continue;
            }

            let utilization = total_demand_bps / capacity_bps * 100.0;

            if total_demand_bps > capacity_bps {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "bus '{}' bandwidth exceeded: {:.1} bps demand vs {:.1} bps capacity \
                         ({:.1}% utilization, {} bound connections)",
                        bus_comp.name,
                        total_demand_bps,
                        capacity_bps,
                        utilization,
                        bound_connections.len(),
                    ),
                    path: bus_path.clone(),
                    analysis: self.name().to_string(),
                });
            } else if utilization > 80.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "bus '{}' bandwidth utilization is high: {:.1} bps demand vs {:.1} bps capacity \
                         ({:.1}% utilization, {} bound connections)",
                        bus_comp.name,
                        total_demand_bps,
                        capacity_bps,
                        utilization,
                        bound_connections.len(),
                    ),
                    path: bus_path.clone(),
                    analysis: self.name().to_string(),
                });
            } else {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "bus '{}' bandwidth utilization: {:.1} bps of {:.1} bps ({:.1}%, {} bound connections)",
                        bus_comp.name,
                        total_demand_bps,
                        capacity_bps,
                        utilization,
                        bound_connections.len(),
                    ),
                    path: bus_path,
                    analysis: self.name().to_string(),
                });
            }
        }

        diags
    }
}

/// Get bandwidth capacity of a bus in bits per second.
///
/// Tries the following properties in order:
/// 1. `SEI::Bandwidth`
/// 2. `Communication_Properties::Bandwidth`
/// 3. `Bandwidth` (unqualified)
/// 4. `Communication_Properties::Data_Rate`
/// 5. `Data_Rate` (unqualified)
fn get_bandwidth_capacity(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    // Try Bandwidth property first (SEI or Communication_Properties).
    let raw = props
        .get("SEI", "Bandwidth")
        .or_else(|| props.get("Communication_Properties", "Bandwidth"))
        .or_else(|| props.get("", "Bandwidth"));

    if let Some(bps) = raw.and_then(parse_bandwidth) {
        return Some(bps);
    }

    // Fall back to Data_Rate.
    let raw = props
        .get("Communication_Properties", "Data_Rate")
        .or_else(|| props.get("", "Data_Rate"));

    if let Some(val) = raw {
        return parse_data_rate(val);
    }

    None
}

/// Compute the bandwidth demand of a single connection in bits per second.
///
/// demand = Data_Size (bits) / Period (seconds)
///
/// Where Data_Size comes from the source component's data type properties,
/// and Period comes from the source thread's timing properties.
fn compute_connection_demand(
    instance: &SystemInstance,
    src_sub_idx: Option<ComponentInstanceIdx>,
    _bus_map: &FxHashMap<String, ComponentInstanceIdx>,
) -> f64 {
    let src_idx = match src_sub_idx {
        Some(idx) => idx,
        None => return 0.0,
    };

    // Get data size from the source component (bits).
    // We look for Data_Size on the source or find a thread child.
    let data_size_bits = get_data_size_for_component(instance, src_idx);
    if data_size_bits == 0 {
        return 0.0;
    }

    // Get Period from the source component (picoseconds).
    // Walk down to find a thread if the source is a process.
    let period_ps = get_period_for_component(instance, src_idx);
    if period_ps == 0 {
        return 0.0;
    }

    // demand = Data_Size (bits) / Period (seconds)
    // Period is in picoseconds, so Period_sec = period_ps / 1e12
    // demand_bps = data_size_bits / (period_ps / 1e12) = data_size_bits * 1e12 / period_ps
    (data_size_bits as f64) * 1e12 / (period_ps as f64)
}

/// Get Data_Size for a component in bits.
///
/// First checks the component itself, then checks any child threads.
fn get_data_size_for_component(instance: &SystemInstance, idx: ComponentInstanceIdx) -> u64 {
    let props = instance.properties_for(idx);

    // Direct Data_Size on the component.
    if let Some(size) = get_size_property(props, "Data_Size") {
        return size;
    }

    // If this is a process, look at child threads.
    let comp = instance.component(idx);
    if comp.category == ComponentCategory::Process || comp.category == ComponentCategory::Thread {
        for &child_idx in &comp.children {
            let child = instance.component(child_idx);
            if child.category == ComponentCategory::Thread {
                let child_props = instance.properties_for(child_idx);
                if let Some(size) = get_size_property(child_props, "Data_Size") {
                    return size;
                }
            }
        }
    }

    0
}

/// Get Period for a component in picoseconds.
///
/// First checks the component itself, then checks any child threads.
fn get_period_for_component(instance: &SystemInstance, idx: ComponentInstanceIdx) -> u64 {
    let props = instance.properties_for(idx);

    // Direct Period on the component.
    if let Some(period) = get_timing_property(props, "Period") {
        return period;
    }

    // If this is a process, look at child threads.
    let comp = instance.component(idx);
    for &child_idx in &comp.children {
        let child = instance.component(child_idx);
        if child.category == ComponentCategory::Thread {
            let child_props = instance.properties_for(child_idx);
            if let Some(period) = get_timing_property(child_props, "Period") {
                return period;
            }
        }
    }

    0
}

/// Find a child component instance by name.
fn find_child_by_name(
    instance: &SystemInstance,
    parent: ComponentInstanceIdx,
    name: &str,
) -> Option<ComponentInstanceIdx> {
    let parent_comp = instance.component(parent);
    for &child_idx in &parent_comp.children {
        let child = instance.component(child_idx);
        if child.name.as_str().eq_ignore_ascii_case(name) {
            return Some(child_idx);
        }
    }
    None
}

/// Parse a bandwidth value string into bits per second.
///
/// Supports formats like:
/// - "100.0 Kbitsps" or "1 Mbitsps"
/// - "12.5 KBytesps" or "1 MBytesps"
/// - "100000.0 bitsps"
/// - Plain numeric (assumed bps)
fn parse_bandwidth(s: &str) -> Option<f64> {
    parse_data_rate(s)
}

/// Parse a data rate value string like "100 KBytesps" into bits per second.
fn parse_data_rate(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(suffix, factor) in DATA_RATE_UNITS {
        if let Some(val) = s
            .strip_suffix(suffix)
            .map(|s| s.trim())
            .and_then(|n| n.parse::<f64>().ok())
        {
            return Some(val * factor);
        }
    }
    // Try plain number (assume bps).
    s.parse::<f64>().ok()
}

/// Data rate units and their conversion factors to bits per second.
const DATA_RATE_UNITS: &[(&str, f64)] = &[
    ("Gbitsps", 1_000_000_000.0),
    ("Mbitsps", 1_000_000.0),
    ("Kbitsps", 1_000.0),
    ("bitsps", 1.0),
    ("GBytesps", 8_000_000_000.0),
    ("MBytesps", 8_000_000.0),
    ("KBytesps", 8_000.0),
    ("Bytesps", 8.0),
];

// ── Tests ──────────────────────────────────────────────────────────

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

        fn add_connection(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: &str,
            src_feat: &str,
            dst_sub: &str,
            dst_feat: &str,
        ) -> ConnectionInstanceIdx {
            let conn_idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(src_sub)),
                    feature: Name::new(src_feat),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(dst_sub)),
                    feature: Name::new(dst_feat),
                }),
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(conn_idx);
            conn_idx
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

    // ── Helper: build a standard test model ─────────────────────────
    //
    // root (System)
    //   |- bus1 (Bus) -- with Bandwidth property
    //   |- sender (Process)
    //   |    |- sender_thread (Thread) -- with Period and Data_Size
    //   |- receiver (Process)
    //   |    |- receiver_thread (Thread)
    //   |- connection c1: sender.out -> receiver.in
    //   root has Actual_Connection_Binding => reference(bus1)

    fn build_basic_model(
        bus_bandwidth: &str,
        data_size: &str,
        period: &str,
    ) -> (TestBuilder, ComponentInstanceIdx) {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let sender_thread =
            b.add_component("sender_thread", ComponentCategory::Thread, Some(sender));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        let _receiver_thread =
            b.add_component("receiver_thread", ComponentCategory::Thread, Some(receiver));

        b.set_children(root, vec![bus1, sender, receiver]);
        b.set_children(sender, vec![sender_thread]);
        b.set_children(receiver, vec![_receiver_thread]);

        // Bus bandwidth capacity.
        b.set_property(bus1, "SEI", "Bandwidth", bus_bandwidth);

        // Sender thread properties.
        b.set_property(sender_thread, "Memory_Properties", "Data_Size", data_size);
        b.set_property(sender_thread, "Timing_Properties", "Period", period);

        // Bind connections to bus1.
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        (b, root)
    }

    // ── Overloaded bus ──────────────────────────────────────────────

    #[test]
    fn overloaded_bus_produces_error() {
        // Bus capacity: 1000 bps
        // Data_Size: 1 KByte = 8192 bits, Period: 1 sec
        // Demand = 8192 bps > 1000 bps
        let (b, root) = build_basic_model("1000 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "should error on overloaded bus: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("exceeded"),
            "should mention exceeded: {}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("bus1"),
            "should mention bus name: {}",
            errors[0].message
        );
    }

    // ── Normal usage (under capacity) ───────────────────────────────

    #[test]
    fn normal_usage_produces_info() {
        // Bus capacity: 1 Mbitsps = 1,000,000 bps
        // Data_Size: 1 KByte = 8192 bits, Period: 1 sec
        // Demand = 8192 bps << 1,000,000 bps (~0.8% utilization)
        let (b, root) = build_basic_model("1 Mbitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should not error: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report utilization info: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("bus1"),
            "should mention bus name: {}",
            infos[0].message
        );
    }

    // ── High utilization produces warning ───────────────────────────

    #[test]
    fn high_utilization_produces_warning() {
        // Bus capacity: 10000 bitsps
        // Data_Size: 1 KByte = 8192 bits, Period: 1 sec
        // Demand = 8192 bps / 10000 bps = 81.9% > 80%
        let (b, root) = build_basic_model("10000 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "should warn on high utilization: {:?}",
            diags
        );
        assert!(
            warnings[0].message.contains("high"),
            "should mention high utilization: {}",
            warnings[0].message
        );
    }

    // ── No connection binding => no diagnostics ─────────────────────

    #[test]
    fn no_binding_produces_no_diagnostics() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender, receiver]);

        // Bus has bandwidth but no connections are bound to it.
        b.set_property(bus1, "SEI", "Bandwidth", "1 Mbitsps");
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        // NOTE: no Actual_Connection_Binding set

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let bus_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.analysis == "bus_bandwidth")
            .collect();
        assert!(
            bus_diags.is_empty(),
            "no binding = no bus bandwidth diagnostics: {:?}",
            bus_diags
        );
    }

    // ── Missing properties (Data_Size or Period) => no demand ───────

    #[test]
    fn missing_data_size_no_demand() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let sender_thread =
            b.add_component("sender_thread", ComponentCategory::Thread, Some(sender));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender, receiver]);
        b.set_children(sender, vec![sender_thread]);

        b.set_property(bus1, "SEI", "Bandwidth", "1 Mbitsps");
        // Only set Period, no Data_Size.
        b.set_property(sender_thread, "Timing_Properties", "Period", "10 ms");
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        // No demand computed, so no diagnostics for this bus.
        let bus_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.analysis == "bus_bandwidth")
            .collect();
        assert!(
            bus_diags.is_empty(),
            "missing Data_Size = no demand: {:?}",
            bus_diags
        );
    }

    #[test]
    fn missing_period_no_demand() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let sender_thread =
            b.add_component("sender_thread", ComponentCategory::Thread, Some(sender));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender, receiver]);
        b.set_children(sender, vec![sender_thread]);

        b.set_property(bus1, "SEI", "Bandwidth", "1 Mbitsps");
        // Only set Data_Size, no Period.
        b.set_property(sender_thread, "Memory_Properties", "Data_Size", "1 KByte");
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let bus_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.analysis == "bus_bandwidth")
            .collect();
        assert!(
            bus_diags.is_empty(),
            "missing Period = no demand: {:?}",
            bus_diags
        );
    }

    // ── Bus with no bandwidth property => skip ──────────────────────

    #[test]
    fn bus_without_bandwidth_property_skipped() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender]);

        // No bandwidth property on bus.
        b.add_connection("c1", root, "sender", "out_port", "sender", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        assert!(
            diags.is_empty(),
            "no bandwidth property = skip: {:?}",
            diags
        );
    }

    // ── Communication_Properties::Bandwidth also works ──────────────

    #[test]
    fn communication_properties_bandwidth_works() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let sender_thread =
            b.add_component("sender_thread", ComponentCategory::Thread, Some(sender));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender, receiver]);
        b.set_children(sender, vec![sender_thread]);

        // Use Communication_Properties::Bandwidth instead of SEI::Bandwidth.
        b.set_property(bus1, "Communication_Properties", "Bandwidth", "1 Mbitsps");
        b.set_property(sender_thread, "Memory_Properties", "Data_Size", "1 KByte");
        b.set_property(sender_thread, "Timing_Properties", "Period", "1 sec");
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(infos.len(), 1, "should report utilization: {:?}", diags);
    }

    // ── Data_Rate fallback also works ───────────────────────────────

    #[test]
    fn data_rate_fallback_works() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        let sender_thread =
            b.add_component("sender_thread", ComponentCategory::Thread, Some(sender));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender, receiver]);
        b.set_children(sender, vec![sender_thread]);

        // Use Data_Rate instead of Bandwidth.
        b.set_property(bus1, "Communication_Properties", "Data_Rate", "1 Mbitsps");
        b.set_property(sender_thread, "Memory_Properties", "Data_Size", "1 KByte");
        b.set_property(sender_thread, "Timing_Properties", "Period", "1 sec");
        b.add_connection("c1", root, "sender", "out_port", "receiver", "in_port");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report utilization with Data_Rate: {:?}",
            diags
        );
    }

    // ── Multiple connections on one bus ──────────────────────────────

    #[test]
    fn multiple_connections_summed() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let bus1 = b.add_component("bus1", ComponentCategory::Bus, Some(root));
        let sender1 = b.add_component("sender1", ComponentCategory::Process, Some(root));
        let sender1_thread =
            b.add_component("sender1_thread", ComponentCategory::Thread, Some(sender1));
        let sender2 = b.add_component("sender2", ComponentCategory::Process, Some(root));
        let sender2_thread =
            b.add_component("sender2_thread", ComponentCategory::Thread, Some(sender2));
        let receiver = b.add_component("receiver", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![bus1, sender1, sender2, receiver]);
        b.set_children(sender1, vec![sender1_thread]);
        b.set_children(sender2, vec![sender2_thread]);

        // Bus capacity: 10000 bitsps
        b.set_property(bus1, "SEI", "Bandwidth", "10000 bitsps");

        // sender1: 1 KByte / 1 sec = 8192 bps
        b.set_property(sender1_thread, "Memory_Properties", "Data_Size", "1 KByte");
        b.set_property(sender1_thread, "Timing_Properties", "Period", "1 sec");

        // sender2: 1 KByte / 1 sec = 8192 bps
        b.set_property(sender2_thread, "Memory_Properties", "Data_Size", "1 KByte");
        b.set_property(sender2_thread, "Timing_Properties", "Period", "1 sec");

        // Two connections, both bound to bus1.
        b.add_connection("c1", root, "sender1", "out", "receiver", "in1");
        b.add_connection("c2", root, "sender2", "out", "receiver", "in2");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "reference (bus1)",
        );

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        // Total demand = 8192 + 8192 = 16384 bps > 10000 bps capacity.
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "two connections should overload 10kbps bus: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("2 bound connections"),
            "should report 2 connections: {}",
            errors[0].message
        );
    }

    // ── No bus in model => no diagnostics ───────────────────────────

    #[test]
    fn no_bus_in_model() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sender = b.add_component("sender", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![sender]);

        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        assert!(diags.is_empty(), "no bus = no diagnostics: {:?}", diags);
    }

    // ── Boundary tests (kill > vs >= mutants) ─────────────────────

    #[test]
    fn bandwidth_exactly_at_capacity() {
        // demand == capacity must NOT error (boundary: > not >=).
        // Bus: 8192 bitsps, Data_Size: 1 KByte = 8192 bits, Period: 1 sec
        // demand = 8192 / 1 = 8192 bps == capacity
        let (b, root) = build_basic_model("8192 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "demand == capacity should NOT error: {:?}",
            errors
        );
    }

    #[test]
    fn bandwidth_one_bps_over_capacity() {
        // demand > capacity must error.
        // Bus: 8191 bitsps, Data_Size: 1 KByte = 8192 bits, Period: 1 sec
        // demand = 8192 bps > 8191 bps
        let (b, root) = build_basic_model("8191 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "demand > capacity should error: {:?}",
            diags
        );
        assert!(
            errors[0].message.contains("exceeded"),
            "should mention exceeded: {}",
            errors[0].message
        );
    }

    #[test]
    fn bandwidth_exactly_80_percent_no_warning() {
        // 80% utilization must NOT warn (boundary: > 80.0, not >= 80.0).
        // Bus: 10240 bitsps, demand: 8192 bps => 80.0% exactly
        let (b, root) = build_basic_model("10240 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(
            errors.is_empty(),
            "80%% utilization should not error: {:?}",
            errors
        );
        assert!(
            warnings.is_empty(),
            "exactly 80%% should NOT warn (> 80, not >=): {:?}",
            warnings
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("utilization"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should emit info for 80%% utilization: {:?}",
            diags
        );
    }

    #[test]
    fn bandwidth_just_above_80_percent_warns() {
        // Just above 80% should warn.
        // Bus: 10239 bitsps, demand: 8192 bps => 80.008...% > 80%
        let (b, root) = build_basic_model("10239 bitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "just above 80%% should warn: {:?}",
            diags
        );
        assert!(
            warnings[0].message.contains("high"),
            "should mention high utilization: {}",
            warnings[0].message
        );
    }

    #[test]
    fn compute_connection_demand_uses_multiply_not_add() {
        // demand = data_size * (1e12 / period), verify it's multiplication.
        // Data_Size: 100 bits, Period: 1 sec = 1e12 ps
        // demand = 100 * 1e12 / 1e12 = 100 bps (multiply)
        // If mutated to +: 100 + 1e12/1e12 = 101 bps
        // Use capacity of 99 bps: if multiply, 100 > 99 => error.
        // If add, 101 > 99 => also error, so this test won't distinguish.
        //
        // Better: capacity 150, Data_Size: 10 bits, Period: 100 ms = 1e11 ps
        // demand = 10 * 1e12 / 1e11 = 100 bps (multiply) => under 150
        // If add: 10 + 1e12/1e11 = 10 + 10 = 20 bps => also under 150
        //
        // Actually use: capacity 50, Data_Size: 10 bits, Period: 100 ms
        // multiply: 10 * 1e12/1e11 = 100 bps > 50 => error
        // add: 10 + 10 = 20 bps < 50 => no error
        let (b, root) = build_basic_model("50 bitsps", "10 bits", "100 ms");
        let inst = b.build(root);
        let diags = BusBandwidthAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "demand = 10 bits / 0.1 sec = 100 bps > 50 bps should error: {:?}",
            diags
        );
    }

    #[test]
    fn bus_bandwidth_analysis_field_matches_name() {
        // Verify every diagnostic has .analysis == self.name().
        let (b, root) = build_basic_model("1 Mbitsps", "1 KByte", "1 sec");
        let inst = b.build(root);
        let pass = BusBandwidthAnalysis;
        let diags = pass.analyze(&inst);

        assert!(!diags.is_empty(), "should produce diagnostics");
        for diag in &diags {
            assert_eq!(
                diag.analysis,
                pass.name(),
                "diagnostic .analysis must match .name(): {:?}",
                diag,
            );
        }
    }

    // ── parse_data_rate tests ───────────────────────────────────────

    #[test]
    fn parse_data_rate_kbitsps() {
        assert_eq!(parse_data_rate("100 Kbitsps"), Some(100_000.0));
    }

    #[test]
    fn parse_data_rate_mbitsps() {
        assert_eq!(parse_data_rate("1 Mbitsps"), Some(1_000_000.0));
    }

    #[test]
    fn parse_data_rate_kbytesps() {
        assert_eq!(parse_data_rate("10 KBytesps"), Some(80_000.0));
    }

    #[test]
    fn parse_data_rate_plain_number() {
        assert_eq!(parse_data_rate("1000"), Some(1000.0));
    }

    #[test]
    fn parse_data_rate_invalid() {
        assert_eq!(parse_data_rate("invalid"), None);
    }

    #[test]
    fn parse_data_rate_gbitsps() {
        assert_eq!(parse_data_rate("1 Gbitsps"), Some(1_000_000_000.0));
    }

    #[test]
    fn parse_data_rate_bytesps() {
        assert_eq!(parse_data_rate("100 Bytesps"), Some(800.0));
    }
}
