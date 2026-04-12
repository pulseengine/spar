//! Weight and power aggregation analysis.
//!
//! Walks the component hierarchy bottom-up, aggregating weight and power
//! budget values from children and comparing against parent limits.
//!
//! # Properties checked
//!
//! - **Weight**: `SEI::GrossWeight` or `Weight` on each component (in kg).
//! - **Weight limit**: `SEI::WeightLimit` or `Weight_Limit` on parent components.
//! - **Power budget**: `SEI::PowerBudget` or `Power_Budget` on each component (in mW).
//! - **Power capacity**: `SEI::PowerCapacity` or `Power_Capacity` on parent components.
//!
//! # Algorithm
//!
//! 1. Walk the component hierarchy bottom-up (deepest children first).
//! 2. For each component, read its own weight and power values.
//! 3. Sum children's values at each parent level.
//! 4. Compare aggregated children total against parent limit.
//! 5. Error if children sum exceeds parent limit.
//! 6. Info with aggregated totals per system/component.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

use crate::{Analysis, AnalysisDiagnostic, Severity, component_depth, component_path};

/// Weight and power aggregation analysis.
pub struct WeightPowerAnalysis;

impl Analysis for WeightPowerAnalysis {
    fn name(&self) -> &str {
        "weight_power"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        check_weight_budgets(instance, &mut diags);
        check_power_budgets(instance, &mut diags);

        diags
    }
}

// ── Weight budget ───────────────────────────────────────────────────

/// Check weight budgets: for each component with a weight limit, sum
/// children's weights and compare against the limit.
fn check_weight_budgets(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    // Process components bottom-up so leaf values are resolved first.
    let ordered = bottom_up_order(instance);

    // Cache aggregated weight per component: own weight + children's aggregated weights.
    let mut aggregated: rustc_hash::FxHashMap<ComponentInstanceIdx, f64> =
        rustc_hash::FxHashMap::default();

    for &idx in &ordered {
        let comp = instance.component(idx);
        let props = instance.properties_for(idx);

        // Own weight: check SEI::GrossWeight, SEI::Weight, or unqualified Weight.
        let own_weight = get_weight_property(props);

        // Sum of children's aggregated weights.
        let children_weight: f64 = comp
            .children
            .iter()
            .filter_map(|&child| aggregated.get(&child))
            .sum();

        // This component's aggregated weight = own + children.
        let total = own_weight.unwrap_or(0.0) + children_weight;
        if total > 0.0 {
            aggregated.insert(idx, total);
        }

        // Check weight limit if specified on this component.
        let weight_limit = get_weight_limit_property(props);
        if let Some(limit) = weight_limit {
            let path = component_path(instance, idx);
            if children_weight > 0.0 && children_weight > limit {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "weight limit exceeded on '{}': children weigh {:.2} kg, limit is {:.2} kg ({:.1}% of capacity)",
                        comp.name,
                        children_weight,
                        limit,
                        (children_weight / limit) * 100.0,
                    ),
                    path: path.clone(),
                    analysis: "weight_power".to_string(),
                });
            } else if children_weight > 0.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "weight budget for '{}': {:.2} kg of {:.2} kg limit ({:.1}% utilization)",
                        comp.name,
                        children_weight,
                        limit,
                        (children_weight / limit) * 100.0,
                    ),
                    path,
                    analysis: "weight_power".to_string(),
                });
            }
        }
    }
}

// ── Power budget ────────────────────────────────────────────────────

/// Check power budgets: for each component with a power capacity, sum
/// children's power budgets and compare against the capacity.
fn check_power_budgets(instance: &SystemInstance, diags: &mut Vec<AnalysisDiagnostic>) {
    let ordered = bottom_up_order(instance);

    let mut aggregated: rustc_hash::FxHashMap<ComponentInstanceIdx, f64> =
        rustc_hash::FxHashMap::default();

    for &idx in &ordered {
        let comp = instance.component(idx);
        let props = instance.properties_for(idx);

        let own_power = get_power_budget_property(props);

        let children_power: f64 = comp
            .children
            .iter()
            .filter_map(|&child| aggregated.get(&child))
            .sum();

        let total = own_power.unwrap_or(0.0) + children_power;
        if total > 0.0 {
            aggregated.insert(idx, total);
        }

        let power_capacity = get_power_capacity_property(props);
        if let Some(capacity) = power_capacity {
            let path = component_path(instance, idx);
            if children_power > 0.0 && children_power > capacity {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "power capacity exceeded on '{}': children require {:.2} mW, capacity is {:.2} mW ({:.1}% of capacity)",
                        comp.name,
                        children_power,
                        capacity,
                        (children_power / capacity) * 100.0,
                    ),
                    path: path.clone(),
                    analysis: "weight_power".to_string(),
                });
            } else if children_power > 0.0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "power budget for '{}': {:.2} mW of {:.2} mW capacity ({:.1}% utilization)",
                        comp.name,
                        children_power,
                        capacity,
                        (children_power / capacity) * 100.0,
                    ),
                    path,
                    analysis: "weight_power".to_string(),
                });
            }
        }
    }
}

// ── Ordering ────────────────────────────────────────────────────────

/// Return all component indices in bottom-up order (deepest first).
fn bottom_up_order(instance: &SystemInstance) -> Vec<ComponentInstanceIdx> {
    let mut all: Vec<(ComponentInstanceIdx, usize)> = instance
        .all_components()
        .map(|(idx, _)| (idx, component_depth(instance, idx)))
        .collect();
    // Sort by depth descending so leaves are processed first.
    all.sort_by_key(|b| std::cmp::Reverse(b.1));
    all.into_iter().map(|(idx, _)| idx).collect()
}

// ── Property helpers ────────────────────────────────────────────────

/// Parse a weight property value from a component.
///
/// Checks `SEI::GrossWeight`, `SEI::Weight`, `Physical_Properties::Weight`,
/// and unqualified `GrossWeight` / `Weight`. Accepts values like
/// `"10.5 kg"`, `"10.5"`, or `"3200 g"`.
fn get_weight_property(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    let raw = props
        .get("SEI", "GrossWeight")
        .or_else(|| props.get("SEI", "Weight"))
        .or_else(|| props.get("Physical_Properties", "GrossWeight"))
        .or_else(|| props.get("Physical_Properties", "Weight"))
        .or_else(|| props.get("", "GrossWeight"))
        .or_else(|| props.get("", "Weight"))?;
    parse_weight_value(raw)
}

/// Parse a weight limit property value from a component.
fn get_weight_limit_property(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    let raw = props
        .get("SEI", "WeightLimit")
        .or_else(|| props.get("Physical_Properties", "WeightLimit"))
        .or_else(|| props.get("Physical_Properties", "Weight_Limit"))
        .or_else(|| props.get("", "WeightLimit"))
        .or_else(|| props.get("", "Weight_Limit"))?;
    parse_weight_value(raw)
}

/// Parse a power budget property value from a component.
fn get_power_budget_property(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    let raw = props
        .get("SEI", "PowerBudget")
        .or_else(|| props.get("Physical_Properties", "PowerBudget"))
        .or_else(|| props.get("Physical_Properties", "Power_Budget"))
        .or_else(|| props.get("", "PowerBudget"))
        .or_else(|| props.get("", "Power_Budget"))?;
    parse_power_value(raw)
}

/// Parse a power capacity property value from a component.
fn get_power_capacity_property(props: &spar_hir_def::properties::PropertyMap) -> Option<f64> {
    let raw = props
        .get("SEI", "PowerCapacity")
        .or_else(|| props.get("Physical_Properties", "PowerCapacity"))
        .or_else(|| props.get("Physical_Properties", "Power_Capacity"))
        .or_else(|| props.get("", "PowerCapacity"))
        .or_else(|| props.get("", "Power_Capacity"))?;
    parse_power_value(raw)
}

/// Parse a weight value string into kilograms.
///
/// Supports: `"10.5 kg"`, `"3200 g"`, `"23100 mg"`, or a bare number
/// (assumed kg).
fn parse_weight_value(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(suffix, factor) in WEIGHT_UNITS {
        if let Some(val) = s
            .strip_suffix(suffix)
            .map(|s| s.trim())
            .and_then(|n| n.parse::<f64>().ok())
        {
            return Some(val * factor);
        }
    }
    // Bare number: assume kg.
    s.parse::<f64>().ok()
}

/// Parse a power value string into milliwatts.
///
/// Supports: `"100 W"`, `"500 mW"`, `"1.5 kW"`, or a bare number (assumed mW).
fn parse_power_value(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(suffix, factor) in POWER_UNITS {
        if let Some(val) = s
            .strip_suffix(suffix)
            .map(|s| s.trim())
            .and_then(|n| n.parse::<f64>().ok())
        {
            return Some(val * factor);
        }
    }
    // Bare number: assume mW.
    s.parse::<f64>().ok()
}

/// Weight units and their conversion factors to kilograms.
const WEIGHT_UNITS: &[(&str, f64)] = &[
    ("kg", 1.0),
    ("g", 0.001),
    ("mg", 0.000_001),
    ("lb", 0.453_592),
];

/// Power units and their conversion factors to milliwatts.
const POWER_UNITS: &[(&str, f64)] = &[
    ("kW", 1_000_000.0),
    ("W", 1_000.0),
    ("mW", 1.0),
    ("uW", 0.001),
];

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ComponentCategory;
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

    // ── Weight: exceeded ────────────────────────────────────────

    #[test]
    fn weight_exceeded() {
        let mut b = TestBuilder::new();
        let root = b.add_component("aircraft", ComponentCategory::System, None);
        let wing = b.add_component("wing", ComponentCategory::System, Some(root));
        let engine = b.add_component("engine", ComponentCategory::System, Some(root));
        b.set_children(root, vec![wing, engine]);

        // Children weigh 60 + 50 = 110 kg, limit is 100 kg
        b.set_property(wing, "SEI", "GrossWeight", "60 kg");
        b.set_property(engine, "SEI", "GrossWeight", "50 kg");
        b.set_property(root, "SEI", "WeightLimit", "100 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "expected 1 weight error: {:?}", diags);
        assert!(
            errors[0].message.contains("weight limit exceeded"),
            "expected weight limit message: {}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("110.00 kg"),
            "expected children weight 110 kg: {}",
            errors[0].message
        );
    }

    // ── Weight: within budget ───────────────────────────────────

    #[test]
    fn weight_within_budget() {
        let mut b = TestBuilder::new();
        let root = b.add_component("aircraft", ComponentCategory::System, None);
        let wing = b.add_component("wing", ComponentCategory::System, Some(root));
        let engine = b.add_component("engine", ComponentCategory::System, Some(root));
        b.set_children(root, vec![wing, engine]);

        // Children weigh 30 + 40 = 70 kg, limit is 100 kg
        b.set_property(wing, "SEI", "GrossWeight", "30 kg");
        b.set_property(engine, "SEI", "GrossWeight", "40 kg");
        b.set_property(root, "SEI", "WeightLimit", "100 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should be within budget: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("weight budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report weight info: {:?}", diags);
        assert!(
            infos[0].message.contains("70.00 kg"),
            "expected 70 kg: {}",
            infos[0].message
        );
        assert!(
            infos[0].message.contains("70.0%"),
            "expected 70% utilization: {}",
            infos[0].message
        );
    }

    // ── No properties: skip gracefully ──────────────────────────

    #[test]
    fn no_properties_skip_gracefully() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let sub = b.add_component("sub", ComponentCategory::System, Some(root));
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        assert!(
            diags.is_empty(),
            "no weight/power properties should produce no diagnostics: {:?}",
            diags
        );
    }

    // ── Nested hierarchy ────────────────────────────────────────

    #[test]
    fn nested_hierarchy_aggregation() {
        // root (limit 200 kg)
        //   subsystem_a (limit 100 kg)
        //     device_1 (30 kg)
        //     device_2 (40 kg)
        //   subsystem_b
        //     device_3 (80 kg)
        //
        // subsystem_a children = 70 kg (within 100 kg limit)
        // root children = subsystem_a(70 aggregated) + subsystem_b(80 aggregated) = 150 kg (within 200 kg)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sub_a = b.add_component("subsystem_a", ComponentCategory::System, Some(root));
        let sub_b = b.add_component("subsystem_b", ComponentCategory::System, Some(root));
        let dev1 = b.add_component("device_1", ComponentCategory::Device, Some(sub_a));
        let dev2 = b.add_component("device_2", ComponentCategory::Device, Some(sub_a));
        let dev3 = b.add_component("device_3", ComponentCategory::Device, Some(sub_b));
        b.set_children(root, vec![sub_a, sub_b]);
        b.set_children(sub_a, vec![dev1, dev2]);
        b.set_children(sub_b, vec![dev3]);

        b.set_property(dev1, "SEI", "GrossWeight", "30 kg");
        b.set_property(dev2, "SEI", "GrossWeight", "40 kg");
        b.set_property(dev3, "SEI", "GrossWeight", "80 kg");
        b.set_property(sub_a, "SEI", "WeightLimit", "100 kg");
        b.set_property(root, "SEI", "WeightLimit", "200 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "all within budget: {:?}", errors);

        // subsystem_a info: 70 kg of 100 kg
        let sub_a_info: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info
                    && d.message.contains("subsystem_a")
                    && d.message.contains("weight budget")
            })
            .collect();
        assert_eq!(
            sub_a_info.len(),
            1,
            "should report subsystem_a weight: {:?}",
            diags
        );
        assert!(
            sub_a_info[0].message.contains("70.00 kg"),
            "subsystem_a should aggregate to 70 kg: {}",
            sub_a_info[0].message
        );

        // root info: 150 kg of 200 kg (subsystem_a=70 + subsystem_b=80)
        let root_info: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info
                    && d.message.contains("'root'")
                    && d.message.contains("weight budget")
            })
            .collect();
        assert_eq!(root_info.len(), 1, "should report root weight: {:?}", diags);
        assert!(
            root_info[0].message.contains("150.00 kg"),
            "root should aggregate to 150 kg: {}",
            root_info[0].message
        );
    }

    // ── Power: exceeded ─────────────────────────────────────────

    #[test]
    fn power_exceeded() {
        let mut b = TestBuilder::new();
        let root = b.add_component("board", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let gpu = b.add_component("gpu", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![cpu, gpu]);

        // Children power: 75W + 50W = 125W = 125000 mW, capacity = 100W = 100000 mW
        b.set_property(cpu, "", "PowerBudget", "75 W");
        b.set_property(gpu, "", "PowerBudget", "50 W");
        b.set_property(root, "", "PowerCapacity", "100 W");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "expected 1 power error: {:?}", diags);
        assert!(
            errors[0].message.contains("power capacity exceeded"),
            "expected power capacity message: {}",
            errors[0].message
        );
    }

    // ── Power: within budget ────────────────────────────────────

    #[test]
    fn power_within_budget() {
        let mut b = TestBuilder::new();
        let root = b.add_component("board", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let gpu = b.add_component("gpu", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![cpu, gpu]);

        // Children power: 40W + 30W = 70W = 70000 mW, capacity = 100W = 100000 mW
        b.set_property(cpu, "", "PowerBudget", "40 W");
        b.set_property(gpu, "", "PowerBudget", "30 W");
        b.set_property(root, "", "PowerCapacity", "100 W");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "should be within budget: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("power budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report power info: {:?}", diags);
        assert!(
            infos[0].message.contains("70.0%"),
            "expected 70% utilization: {}",
            infos[0].message
        );
    }

    // ── Weight: unit conversion ─────────────────────────────────

    #[test]
    fn weight_unit_conversion() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let part = b.add_component("part", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![part]);

        // 2500 g = 2.5 kg, limit = 3 kg
        b.set_property(part, "", "Weight", "2500 g");
        b.set_property(root, "", "WeightLimit", "3 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "2.5 kg < 3 kg: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("weight budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report weight: {:?}", diags);
        assert!(
            infos[0].message.contains("2.50 kg"),
            "expected 2.50 kg: {}",
            infos[0].message
        );
    }

    // ── Power: unit conversion ──────────────────────────────────

    #[test]
    fn power_unit_conversion() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        // 500 mW, capacity = 1 W = 1000 mW
        b.set_property(sensor, "", "PowerBudget", "500 mW");
        b.set_property(root, "", "PowerCapacity", "1 W");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "500 mW < 1000 mW: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("power budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report power: {:?}", diags);
        assert!(
            infos[0].message.contains("50.0%"),
            "expected 50% utilization: {}",
            infos[0].message
        );
    }

    // ── Parse helpers ───────────────────────────────────────────

    #[test]
    fn parse_weight_values() {
        assert_eq!(parse_weight_value("10 kg"), Some(10.0));
        assert_eq!(parse_weight_value("10.5 kg"), Some(10.5));
        assert_eq!(parse_weight_value("2500 g"), Some(2.5));
        assert_eq!(parse_weight_value("5 lb"), Some(5.0 * 0.453_592));
        assert_eq!(parse_weight_value("42"), Some(42.0));
        assert_eq!(parse_weight_value("invalid"), None);
    }

    #[test]
    fn parse_power_values() {
        assert_eq!(parse_power_value("100 W"), Some(100_000.0));
        assert_eq!(parse_power_value("500 mW"), Some(500.0));
        assert_eq!(parse_power_value("1.5 kW"), Some(1_500_000.0));
        assert_eq!(parse_power_value("200"), Some(200.0));
        assert_eq!(parse_power_value("invalid"), None);
    }

    // ── Both weight and power on same hierarchy ─────────────────

    #[test]
    fn combined_weight_and_power() {
        let mut b = TestBuilder::new();
        let root = b.add_component("vehicle", ComponentCategory::System, None);
        let motor = b.add_component("motor", ComponentCategory::Device, Some(root));
        let battery = b.add_component("battery", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![motor, battery]);

        // Weight: 25 + 15 = 40 kg, limit = 50 kg (ok)
        b.set_property(motor, "", "Weight", "25 kg");
        b.set_property(battery, "", "Weight", "15 kg");
        b.set_property(root, "", "WeightLimit", "50 kg");

        // Power: 2kW + 0.5kW = 2.5kW = 2500000 mW, capacity = 2kW = 2000000 mW (exceeded)
        b.set_property(motor, "", "PowerBudget", "2 kW");
        b.set_property(battery, "", "PowerBudget", "0.5 kW");
        b.set_property(root, "", "PowerCapacity", "2 kW");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        // Weight should be info (within budget)
        let weight_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("weight"))
            .collect();
        assert!(
            weight_errors.is_empty(),
            "weight within budget: {:?}",
            weight_errors
        );

        // Power should be error (exceeded)
        let power_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("power"))
            .collect();
        assert_eq!(
            power_errors.len(),
            1,
            "power should be exceeded: {:?}",
            diags
        );
    }

    // ── Weight: exactly at limit (boundary) ──────────────────────

    #[test]
    fn weight_exactly_at_limit() {
        let mut b = TestBuilder::new();
        let root = b.add_component("aircraft", ComponentCategory::System, None);
        let wing = b.add_component("wing", ComponentCategory::System, Some(root));
        let engine = b.add_component("engine", ComponentCategory::System, Some(root));
        b.set_children(root, vec![wing, engine]);

        // Children weigh 60 + 40 = 100 kg, limit is exactly 100 kg
        b.set_property(wing, "SEI", "GrossWeight", "60 kg");
        b.set_property(engine, "SEI", "GrossWeight", "40 kg");
        b.set_property(root, "SEI", "WeightLimit", "100 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "exactly at limit should NOT error: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("weight budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report weight info: {:?}", diags);
        assert!(
            infos[0].message.contains("100.0%"),
            "expected 100% utilization: {}",
            infos[0].message
        );
    }

    // ── Weight: 1 unit over limit ──────────────────────────────

    #[test]
    fn weight_one_over_limit() {
        let mut b = TestBuilder::new();
        let root = b.add_component("aircraft", ComponentCategory::System, None);
        let wing = b.add_component("wing", ComponentCategory::System, Some(root));
        let engine = b.add_component("engine", ComponentCategory::System, Some(root));
        b.set_children(root, vec![wing, engine]);

        // Children weigh 60 + 41 = 101 kg, limit is 100 kg
        b.set_property(wing, "SEI", "GrossWeight", "60 kg");
        b.set_property(engine, "SEI", "GrossWeight", "41 kg");
        b.set_property(root, "SEI", "WeightLimit", "100 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "1 over limit should error: {:?}", diags);
        assert!(
            errors[0].message.contains("weight limit exceeded"),
            "expected weight limit message: {}",
            errors[0].message
        );
    }

    // ── Power: exactly at capacity (boundary) ──────────────────

    #[test]
    fn power_exactly_at_capacity() {
        let mut b = TestBuilder::new();
        let root = b.add_component("board", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let gpu = b.add_component("gpu", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![cpu, gpu]);

        // Children power: 60W + 40W = 100W = 100000 mW, capacity = 100W = 100000 mW
        b.set_property(cpu, "", "PowerBudget", "60 W");
        b.set_property(gpu, "", "PowerBudget", "40 W");
        b.set_property(root, "", "PowerCapacity", "100 W");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "exactly at capacity should NOT error: {:?}",
            errors
        );

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("power budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report power info: {:?}", diags);
        assert!(
            infos[0].message.contains("100.0%"),
            "expected 100% utilization: {}",
            infos[0].message
        );
    }

    // ── Power: 1 unit over capacity ────────────────────────────

    #[test]
    fn power_one_over_capacity() {
        let mut b = TestBuilder::new();
        let root = b.add_component("board", ComponentCategory::System, None);
        let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        let gpu = b.add_component("gpu", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![cpu, gpu]);

        // Children power: 60W + 41W = 101W = 101000 mW, capacity = 100W = 100000 mW
        b.set_property(cpu, "", "PowerBudget", "60 W");
        b.set_property(gpu, "", "PowerBudget", "41 W");
        b.set_property(root, "", "PowerCapacity", "100 W");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "1 over capacity should error: {:?}", diags);
        assert!(
            errors[0].message.contains("power capacity exceeded"),
            "expected power capacity message: {}",
            errors[0].message
        );
    }

    // ── Total aggregation: zero total not inserted into map ─────

    #[test]
    fn zero_weight_children_skip_aggregation() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let sub = b.add_component("sub", ComponentCategory::System, Some(root));
        let leaf = b.add_component("leaf", ComponentCategory::System, Some(sub));
        b.set_children(root, vec![sub]);
        b.set_children(sub, vec![leaf]);

        // leaf has no weight, sub has weight limit — children_weight == 0.0
        b.set_property(sub, "SEI", "WeightLimit", "50 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        // No diagnostics for weight because children_weight is 0.0
        let weight_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("weight"))
            .collect();
        assert!(
            weight_diags.is_empty(),
            "zero children weight = no weight diagnostic: {:?}",
            weight_diags
        );
    }

    // ── Property alternatives: unqualified Weight ───────────────

    #[test]
    fn unqualified_weight_property() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let part = b.add_component("part", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![part]);

        b.set_property(part, "", "Weight", "5 kg");
        b.set_property(root, "", "Weight_Limit", "10 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "within budget: {:?}", errors);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("weight budget"))
            .collect();
        assert_eq!(infos.len(), 1, "should report weight: {:?}", diags);
    }

    // ── Parse helpers: uW power unit ────────────────────────────

    #[test]
    fn parse_power_uw() {
        assert_eq!(parse_power_value("500 uW"), Some(0.5));
    }

    // ── Parse helpers: mg weight unit ───────────────────────────

    #[test]
    fn parse_weight_mg() {
        assert_eq!(parse_weight_value("5000000 mg"), Some(5.0));
    }

    // ── Limit with no children weights: no diagnostic ───────────

    #[test]
    fn limit_no_children_weights() {
        let mut b = TestBuilder::new();
        let root = b.add_component("sys", ComponentCategory::System, None);
        let sub = b.add_component("sub", ComponentCategory::System, Some(root));
        b.set_children(root, vec![sub]);

        // Limit on root but no weights on children
        b.set_property(root, "", "WeightLimit", "100 kg");

        let inst = b.build(root);
        let diags = WeightPowerAnalysis.analyze(&inst);

        // No diagnostics since no children have weight properties
        let weight_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("weight"))
            .collect();
        assert!(
            weight_diags.is_empty(),
            "no children weights = no check: {:?}",
            weight_diags
        );
    }
}
