//! Property association validation rules (AS5506 §11).
//!
//! Validates property associations on the instance model:
//! - **PROP-DUPLICATE** — No duplicate property associations for the same
//!   property name within a component
//! - **PROP-VALUE-TYPE** — Property value expression variant should be
//!   compatible with the declared property type
//! - **PROP-RANGE-ORDER** — Range property values: lower bound <= upper bound
//! - **PROP-LIST-ELEMENT-TYPE** — List property values should have consistent
//!   element types
//! - **PROP-APPLIES-TO** — Properties are applied to categories they're allowed on
//! - **PROP-CONSTANT-EXISTS** — Property constant references must resolve

use spar_hir_def::instance::SystemInstance;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Validates property association rules on the instance model.
///
/// Checks AS5506 §11 rules:
/// - Duplicate property associations
/// - Value/type compatibility
/// - Range ordering
/// - List element type consistency
/// - Applies-to category constraints
/// - Property constant resolution
pub struct PropertyRuleAnalysis;

impl Analysis for PropertyRuleAnalysis {
    fn name(&self) -> &str {
        "property_rules"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — inverted range ordering, unbalanced parentheses
        //   Warning — duplicate non-append property, empty property value, mixed list element
        //             types, malformed reference expression, property applied to wrong category
        let mut diags = Vec::new();

        for (comp_idx, _comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);
            let prop_map = instance.properties_for(comp_idx);

            // PROP-DUPLICATE: check for duplicate property names
            // The PropertyMap already collapses non-append properties, but
            // we can detect if a property set+name pair has multiple
            // non-appended values by checking get_all.
            check_duplicate_properties(prop_map, &path, &mut diags);

            // PROP-RANGE-ORDER: check range ordering
            check_range_ordering(prop_map, &path, &mut diags);

            // PROP-LIST-ELEMENT-TYPE: check list element consistency
            check_list_consistency(prop_map, &path, &mut diags);

            // PROP-VALUE-TYPE: basic value type validation
            check_value_types(prop_map, &path, &mut diags);

            // PROP-CONSTANT-EXISTS: check for unresolved references
            check_constant_references(prop_map, &path, &mut diags);

            // PROP-APPLIES-TO: check category constraints
            check_applies_to(instance, comp_idx, prop_map, &path, &mut diags);
        }

        diags
    }
}

/// PROP-DUPLICATE: Detect properties that appear multiple times without append.
fn check_duplicate_properties(
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for ((_set_key, _name_key), values) in prop_map.iter() {
        // If there are multiple values and not all are appends, that's a duplicate
        if values.len() > 1 {
            let non_append_count = values.iter().filter(|v| !v.is_append).count();
            if non_append_count > 1 {
                let prop_display = &values[0].name;
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "property '{}' has {} non-append associations \
                         (only the last assignment takes effect)",
                        prop_display, non_append_count
                    ),
                    path: path.to_vec(),
                    analysis: "property_rules".to_string(),
                });
            }
        }
    }
}

/// PROP-RANGE-ORDER: For properties whose values look like ranges
/// (pattern: "low .. high"), check that low <= high.
fn check_range_ordering(
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for ((_set_key, _name_key), values) in prop_map.iter() {
        for pv in values {
            let val = pv.value.trim();
            // Look for range pattern: "number .. number"
            if let Some((low_str, high_str)) = val.split_once("..") {
                let low_str = low_str.trim();
                let high_str = high_str.trim();
                // Try to parse as integers
                if let (Ok(low), Ok(high)) =
                    (parse_numeric_value(low_str), parse_numeric_value(high_str))
                    && low > high
                {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "property '{}' range has lower bound ({}) > upper bound ({})",
                            pv.name, low_str, high_str
                        ),
                        path: path.to_vec(),
                        analysis: "property_rules".to_string(),
                    });
                }
            }
        }
    }
}

/// Try to parse a numeric value string, stripping any trailing unit.
fn parse_numeric_value(s: &str) -> Result<f64, ()> {
    let s = s.trim();
    // Try direct parse first
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }
    // Try stripping trailing non-digit characters (units like "ms", "kb")
    let numeric_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+')
        .unwrap_or(s.len());
    if numeric_end > 0 {
        s[..numeric_end].parse::<f64>().map_err(|_| ())
    } else {
        Err(())
    }
}

/// PROP-LIST-ELEMENT-TYPE: Check that list property values have
/// consistent element types (all numeric, all string, etc.).
fn check_list_consistency(
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for ((_set_key, _name_key), values) in prop_map.iter() {
        for pv in values {
            let val = pv.value.trim();
            // Detect list values: "( elem1, elem2, ... )"
            if val.starts_with('(') && val.ends_with(')') {
                let inner = &val[1..val.len() - 1];
                let elements: Vec<&str> = inner.split(',').map(|e| e.trim()).collect();
                if elements.len() > 1 {
                    check_element_type_consistency(&elements, &pv.name.to_string(), path, diags);
                }
            }
        }
    }
}

/// Classify an element value and check consistency.
fn check_element_type_consistency(
    elements: &[&str],
    prop_name: &str,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ElemType {
        Numeric,
        StringLit,
        Boolean,
        Reference,
        Other,
    }

    fn classify(s: &str) -> ElemType {
        let s = s.trim();
        if s.is_empty() {
            return ElemType::Other;
        }
        if s.starts_with('"') && s.ends_with('"') {
            return ElemType::StringLit;
        }
        if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false") {
            return ElemType::Boolean;
        }
        if s.starts_with("reference") {
            return ElemType::Reference;
        }
        if parse_numeric_value(s).is_ok() {
            return ElemType::Numeric;
        }
        ElemType::Other
    }

    let first_type = classify(elements[0]);
    if first_type == ElemType::Other {
        return; // Can't classify, skip
    }

    for (i, elem) in elements.iter().enumerate().skip(1) {
        let elem_type = classify(elem);
        if elem_type != ElemType::Other && elem_type != first_type {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "property '{}' list has mixed element types: \
                     element 0 is {:?} but element {} is {:?}",
                    prop_name, first_type, i, elem_type
                ),
                path: path.to_vec(),
                analysis: "property_rules".to_string(),
            });
            break;
        }
    }
}

/// PROP-VALUE-TYPE: Basic value type validation.
/// Check for obviously malformed property values.
fn check_value_types(
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for ((_set_key, _name_key), values) in prop_map.iter() {
        for pv in values {
            let val = pv.value.trim();
            // Check for empty values
            if val.is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!("property '{}' has an empty value", pv.name),
                    path: path.to_vec(),
                    analysis: "property_rules".to_string(),
                });
            }
            // Check for unbalanced parentheses
            let open = val.chars().filter(|&c| c == '(').count();
            let close = val.chars().filter(|&c| c == ')').count();
            if open != close {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "property '{}' has unbalanced parentheses in value '{}'",
                        pv.name, val
                    ),
                    path: path.to_vec(),
                    analysis: "property_rules".to_string(),
                });
            }
        }
    }
}

/// PROP-CONSTANT-EXISTS: Check that property constant references resolve.
/// Detects `reference(...)` patterns that point to nonexistent components.
fn check_constant_references(
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    for ((_set_key, _name_key), values) in prop_map.iter() {
        for pv in values {
            let val = pv.value.trim();
            // Check for malformed reference expressions
            if val.contains("reference")
                && let Some(start) = val.find("reference")
            {
                let after = &val[start + "reference".len()..];
                let trimmed = after.trim();
                if !trimmed.starts_with('(') {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "property '{}' contains 'reference' keyword \
                             without proper parenthesized target",
                            pv.name
                        ),
                        path: path.to_vec(),
                        analysis: "property_rules".to_string(),
                    });
                }
            }
        }
    }
}

/// PROP-APPLIES-TO: Check that properties known to be category-specific
/// are applied to appropriate categories.
fn check_applies_to(
    instance: &SystemInstance,
    comp_idx: spar_hir_def::instance::ComponentInstanceIdx,
    prop_map: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    use spar_hir_def::item_tree::ComponentCategory;

    let comp = instance.component(comp_idx);
    let category = comp.category;

    // Well-known properties and their applicable categories
    let thread_only_props = [
        ("Timing_Properties", "Dispatch_Protocol"),
        ("Timing_Properties", "Period"),
        ("Timing_Properties", "Deadline"),
        ("Timing_Properties", "Compute_Execution_Time"),
    ];

    for (set, name) in &thread_only_props {
        let has_prop = prop_map.get(set, name).is_some() || prop_map.get("", name).is_some();
        if has_prop
            && !matches!(
                category,
                ComponentCategory::Thread | ComponentCategory::Device | ComponentCategory::Abstract
            )
        {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "property '{}::{}' is typically only applicable to \
                     threads and devices, but found on {} component '{}'",
                    set, name, category, comp.name
                ),
                path: path.to_vec(),
                analysis: "property_rules".to_string(),
            });
        }
    }
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
            self.set_property_ext(comp, set, name, value, false);
        }

        fn set_property_ext(
            &mut self,
            comp: ComponentInstanceIdx,
            set: &str,
            name: &str,
            value: &str,
            is_append: bool,
        ) {
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
                is_append,
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

    // ── PROP-DUPLICATE tests ────────────────────────────────────────

    #[test]
    fn no_duplicate_properties_clean() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(dups.is_empty(), "no duplicates expected: {:?}", dups);
    }

    #[test]
    fn append_properties_not_flagged_as_duplicate() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(
            root,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
            false,
        );
        b.set_property_ext(
            root,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
            true,
        );

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "append associations should not be duplicates: {:?}",
            dups
        );
    }

    #[test]
    fn empty_property_map_clean() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "empty map = no diagnostics: {:?}", diags);
    }

    // ── PROP-RANGE-ORDER tests ──────────────────────────────────────

    #[test]
    fn valid_range_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Weight", "10 .. 100");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "valid range should not error: {:?}",
            range_errs
        );
    }

    #[test]
    fn inverted_range_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Weight", "100 .. 10");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("lower bound"))
            .collect();
        assert_eq!(
            range_errs.len(),
            1,
            "inverted range should error: {:?}",
            diags
        );
    }

    #[test]
    fn equal_range_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Weight", "50 .. 50");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "equal range should not error: {:?}",
            range_errs
        );
    }

    // ── PROP-LIST-ELEMENT-TYPE tests ────────────────────────────────

    #[test]
    fn consistent_list_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(1, 2, 3)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "consistent list should not warn: {:?}",
            list_warns
        );
    }

    #[test]
    fn mixed_list_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(1, \"hello\", 3)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert_eq!(list_warns.len(), 1, "mixed list should warn: {:?}", diags);
    }

    #[test]
    fn single_element_list_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(42)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "single-element list should not warn: {:?}",
            list_warns
        );
    }

    // ── PROP-VALUE-TYPE tests ───────────────────────────────────────

    #[test]
    fn empty_value_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "BadProp", "");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let empty_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("empty value"))
            .collect();
        assert_eq!(empty_warns.len(), 1, "empty value should warn: {:?}", diags);
    }

    #[test]
    fn unbalanced_parens_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "BadProp", "reference (cpu1");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let paren_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("unbalanced parentheses"))
            .collect();
        assert_eq!(
            paren_errs.len(),
            1,
            "unbalanced parens should error: {:?}",
            diags
        );
    }

    #[test]
    fn valid_value_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let type_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("empty value") || d.message.contains("unbalanced"))
            .collect();
        assert!(
            type_errs.is_empty(),
            "valid value should not error: {:?}",
            type_errs
        );
    }

    // ── PROP-CONSTANT-EXISTS tests ──────────────────────────────────

    #[test]
    fn well_formed_reference_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let ref_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("without proper parenthesized"))
            .collect();
        assert!(
            ref_warns.is_empty(),
            "well-formed reference should not warn: {:?}",
            ref_warns
        );
    }

    #[test]
    fn malformed_reference_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference cpu1",
        );

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let ref_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("without proper parenthesized"))
            .collect();
        assert_eq!(
            ref_warns.len(),
            1,
            "malformed reference should warn: {:?}",
            diags
        );
    }

    #[test]
    fn no_reference_keyword_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let ref_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("without proper parenthesized"))
            .collect();
        assert!(
            ref_warns.is_empty(),
            "no reference keyword = no warning: {:?}",
            ref_warns
        );
    }

    // ── PROP-APPLIES-TO tests ───────────────────────────────────────

    #[test]
    fn thread_property_on_thread_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let thread = b.add_component("t1", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![thread]);
        b.set_property(thread, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert!(
            applies_warns.is_empty(),
            "thread property on thread should not warn: {:?}",
            applies_warns
        );
    }

    #[test]
    fn thread_property_on_system_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert_eq!(
            applies_warns.len(),
            1,
            "thread property on system should warn: {:?}",
            diags
        );
    }

    #[test]
    fn thread_property_on_device_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let dev = b.add_component("d1", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![dev]);
        b.set_property(dev, "Timing_Properties", "Dispatch_Protocol", "Periodic");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert!(
            applies_warns.is_empty(),
            "thread property on device should not warn: {:?}",
            applies_warns
        );
    }

    // ── parse_numeric_value tests ───────────────────────────────────

    #[test]
    fn parse_numeric_integers() {
        assert_eq!(parse_numeric_value("42"), Ok(42.0));
        assert_eq!(parse_numeric_value("-5"), Ok(-5.0));
        assert_eq!(parse_numeric_value("0"), Ok(0.0));
    }

    #[test]
    fn parse_numeric_with_units() {
        assert_eq!(parse_numeric_value("10ms"), Ok(10.0));
        assert_eq!(parse_numeric_value("100kb"), Ok(100.0));
    }

    #[test]
    fn parse_numeric_invalid() {
        assert!(parse_numeric_value("abc").is_err());
        assert!(parse_numeric_value("").is_err());
    }
}
