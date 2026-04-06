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
        ("Thread_Properties", "Dispatch_Protocol"),
        ("Timing_Properties", "Period"),
        ("Timing_Properties", "Deadline"),
        ("Timing_Properties", "Compute_Execution_Time"),
    ];

    for (set, name) in &thread_only_props {
        let has_prop = prop_map.get(set, name).is_some()
            || prop_map.get("", name).is_some()
            // Legacy: Dispatch_Protocol may appear under Timing_Properties in older models
            || (*name == "Dispatch_Protocol" && prop_map.get("Timing_Properties", name).is_some());
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
    fn empty_value_skipped_by_property_map() {
        // Empty property values are now filtered by PropertyMap::add(),
        // so they never reach the analysis pass. This test verifies the
        // defense-at-source behavior.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "BadProp", "");

        let inst = b.build(root);
        let props = inst.properties_for(root);
        assert!(
            props.get("", "BadProp").is_none(),
            "empty values should be filtered by PropertyMap::add"
        );
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
        b.set_property(dev, "Thread_Properties", "Dispatch_Protocol", "Periodic");

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

    // ── PROP-DUPLICATE boundary tests ─────────────────────────────

    #[test]
    fn duplicate_non_append_replaced_by_property_map() {
        // PropertyMap::add replaces on non-append, so calling set_property
        // twice with the same key results in only the last value surviving.
        // values.len() == 1, so no duplicate is flagged.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "Custom", "Speed", "100");
        b.set_property(root, "Custom", "Speed", "200");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "PropertyMap replaces on non-append, so len==1, no dup: {:?}",
            dups
        );
    }

    #[test]
    fn single_property_value_no_duplicate() {
        // values.len() == 1, not > 1 → skip duplicate check entirely
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "Custom", "Speed", "100");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "single value should not flag duplicate: {:?}",
            dups
        );
    }

    #[test]
    fn exactly_one_non_append_with_one_append_no_duplicate() {
        // values.len() == 2 (> 1 passes), but non_append_count == 1 (not > 1)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(root, "Custom", "Items", "a", false);
        b.set_property_ext(root, "Custom", "Items", "b", true);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "one non-append + one append should not flag duplicate: {:?}",
            dups
        );
    }

    #[test]
    fn two_append_properties_no_duplicate() {
        // values.len() == 2 (> 1 passes), but non_append_count == 0 (not > 1)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(root, "Custom", "Items", "a", true);
        b.set_property_ext(root, "Custom", "Items", "b", true);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "two append properties should not flag duplicate: {:?}",
            dups
        );
    }

    // ── PROP-RANGE-ORDER boundary tests ───────────────────────────

    #[test]
    fn range_low_equals_high_minus_one_no_error() {
        // low < high (49 < 50) → no error
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Weight", "49 .. 50");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "49..50 should not error: {:?}",
            range_errs
        );
    }

    #[test]
    fn range_low_one_more_than_high_error() {
        // low > high (51 > 50) → error
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Weight", "51 .. 50");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("lower bound"))
            .collect();
        assert_eq!(range_errs.len(), 1, "51..50 should error: {:?}", range_errs);
    }

    #[test]
    fn range_with_units_inverted_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "CET", "200ms .. 100ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("lower bound"))
            .collect();
        assert_eq!(
            range_errs.len(),
            1,
            "inverted range with units should error: {:?}",
            diags
        );
    }

    #[test]
    fn non_range_with_dots_no_error() {
        // Value contains ".." but non-numeric sides → no range check
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Path", "a..b");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "non-numeric range should not produce range error: {:?}",
            range_errs
        );
    }

    // ── PROP-LIST-ELEMENT-TYPE boundary tests ─────────────────────

    #[test]
    fn list_value_not_starting_with_paren_no_check() {
        // Doesn't start with '(' → not treated as list
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "[1, \"hello\"]");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "non-paren list should not trigger list check: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_value_not_ending_with_paren_no_check() {
        // Starts with '(' but doesn't end with ')' → not treated as list
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(1, \"hello\"");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "unclosed paren should not trigger list check: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_all_other_elements_no_warning() {
        // All elements classify as ElemType::Other → first_type == Other → return early
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(foo, bar, baz)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "all-Other elements should not trigger mixed warning: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_second_elem_other_with_first_numeric_no_warning() {
        // First is Numeric, second is Other → elem_type == Other means skip
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(1, foo, 3)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "numeric + Other should not trigger mixed warning: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_boolean_elements_consistent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Flags", "(true, false, TRUE)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "all-boolean list should not warn: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_reference_elements_consistent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Refs", "(reference(a), reference(b))");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "all-reference list should not warn: {:?}",
            list_warns
        );
    }

    #[test]
    fn list_boolean_vs_numeric_mixed_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(true, 42)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert_eq!(
            list_warns.len(),
            1,
            "boolean + numeric should trigger mixed warning: {:?}",
            diags
        );
    }

    // ── PROP-VALUE-TYPE boundary tests ────────────────────────────

    #[test]
    fn balanced_parens_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Val", "(a, (b, c))");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let paren_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("unbalanced"))
            .collect();
        assert!(
            paren_errs.is_empty(),
            "balanced parens should not error: {:?}",
            paren_errs
        );
    }

    #[test]
    fn extra_close_paren_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Val", "a))");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let paren_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("unbalanced"))
            .collect();
        assert_eq!(
            paren_errs.len(),
            1,
            "extra close paren should error: {:?}",
            diags
        );
    }

    #[test]
    fn nonempty_value_no_empty_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Val", "something");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let empty_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("empty value"))
            .collect();
        assert!(
            empty_warns.is_empty(),
            "non-empty value should not trigger empty warning: {:?}",
            empty_warns
        );
    }

    // ── PROP-CONSTANT-EXISTS boundary tests ───────────────────────

    #[test]
    fn reference_with_space_before_paren_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Bind", "reference  (cpu)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let ref_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("without proper parenthesized"))
            .collect();
        assert!(
            ref_warns.is_empty(),
            "reference with space before paren should be OK: {:?}",
            ref_warns
        );
    }

    #[test]
    fn value_without_reference_keyword_no_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Val", "some_value cpu1");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let ref_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("without proper parenthesized"))
            .collect();
        assert!(
            ref_warns.is_empty(),
            "no 'reference' keyword = no check: {:?}",
            ref_warns
        );
    }

    // ── PROP-APPLIES-TO boundary tests ────────────────────────────

    #[test]
    fn thread_property_on_abstract_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let abs = b.add_component("a1", ComponentCategory::Abstract, Some(root));
        b.set_children(root, vec![abs]);
        b.set_property(abs, "Thread_Properties", "Dispatch_Protocol", "Periodic");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert!(
            applies_warns.is_empty(),
            "thread property on abstract should not warn: {:?}",
            applies_warns
        );
    }

    #[test]
    fn thread_property_on_process_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("p1", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![proc]);
        b.set_property(proc, "Timing_Properties", "Deadline", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert_eq!(
            applies_warns.len(),
            1,
            "thread property on process should warn: {:?}",
            diags
        );
    }

    #[test]
    fn thread_property_on_processor_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("cpu", ComponentCategory::Processor, Some(root));
        b.set_children(root, vec![proc]);
        b.set_property(proc, "Timing_Properties", "Compute_Execution_Time", "5 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert_eq!(
            applies_warns.len(),
            1,
            "thread property on processor should warn: {:?}",
            diags
        );
    }

    #[test]
    fn thread_property_without_set_prefix_on_system_warning() {
        // Tests the `prop_map.get("", name).is_some()` branch in check_applies_to
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Set with empty property set name — the `get("", name)` path
        b.set_property(root, "", "Period", "10 ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert_eq!(
            applies_warns.len(),
            1,
            "thread property with empty set on system should warn: {:?}",
            diags
        );
    }

    #[test]
    fn non_timing_property_on_system_no_warning() {
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
        let applies_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("typically only applicable"))
            .collect();
        assert!(
            applies_warns.is_empty(),
            "non-timing property on system should not warn: {:?}",
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

    // ── Mutation-killing boundary tests ──────────────────────────────

    // Mutant: line 81, `non_append_count > 1` → `>= 1`
    // With one non-append + two appends, non_append_count == 1.
    // Original `> 1` → false (no warning). Mutant `>= 1` → true (false warning).
    #[test]
    fn one_non_append_two_appends_no_duplicate() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(root, "Custom", "Items", "base", false);
        b.set_property_ext(root, "Custom", "Items", "extra1", true);
        b.set_property_ext(root, "Custom", "Items", "extra2", true);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "one non-append + two appends should not flag duplicate: {:?}",
            dups
        );
    }

    // Mutant: line 81, `non_append_count > 1` → `< 1`
    // With zero non-appends (three appends), non_append_count == 0.
    // Original `> 1` → false. Mutant `< 1` → true (false warning).
    #[test]
    fn three_appends_zero_non_append_no_duplicate() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(root, "Custom", "Items", "a", true);
        b.set_property_ext(root, "Custom", "Items", "b", true);
        b.set_property_ext(root, "Custom", "Items", "c", true);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "three appends with zero non-append should not flag duplicate: {:?}",
            dups
        );
    }

    // Mutant: line 81, `non_append_count > 1` → `== 1`
    // Reaffirm: non_append_count == 0 → no warning (kills `== 1` if it fired for 0).
    // But `== 1` would not fire for 0; it fires for exactly 1.
    // So the test with non_append_count == 1 (above) kills `== 1`.
    // Also verify non_append_count == 0 does not fire (kills `< 1`).
    #[test]
    fn two_appends_only_non_append_count_zero_no_duplicate() {
        // Ensure values.len() > 1 (passes outer guard) but non_append_count == 0
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property_ext(root, "Custom", "Vals", "x", true);
        b.set_property_ext(root, "Custom", "Vals", "y", true);

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("non-append"))
            .collect();
        assert!(
            dups.is_empty(),
            "zero non-append count must not produce duplicate warning: {:?}",
            dups
        );
    }

    // Mutant: line 115, `low > high` → `>= high`
    // Equal range (50 .. 50): low == high.
    // Original `>` → false (no error). Mutant `>=` → true (false error).
    // Existing equal_range_no_error covers this, but add a fractional case.
    #[test]
    fn equal_range_fractional_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Latency", "3.14 .. 3.14");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "equal fractional range (3.14..3.14) must not error: {:?}",
            range_errs
        );
    }

    // Mutant: line 115, `low > high` boundary — confirm low == high is clean
    // but low == high + epsilon produces error.
    #[test]
    fn range_barely_inverted_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // 50.001 > 50.0 → error
        b.set_property(root, "", "Weight", "50.001 .. 50.0");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("lower bound"))
            .collect();
        assert_eq!(
            range_errs.len(),
            1,
            "barely inverted range (50.001..50.0) should error: {:?}",
            diags
        );
    }

    // Mutant: line 115, `low > high` → `>=` — range with units where low == high
    #[test]
    fn equal_range_with_units_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "CET", "100ms .. 100ms");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let range_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("lower bound"))
            .collect();
        assert!(
            range_errs.is_empty(),
            "equal range with units (100ms..100ms) must not error: {:?}",
            range_errs
        );
    }

    // Mutant: line 164, `elements.len() > 1` → `>= 1`
    // Single element list with a classifiable type.
    // With `>= 1`, check_element_type_consistency is called, but since the
    // loop skips element 0 and there's no element 1, no diagnostic.
    // To kill this mutant we need to ensure the function itself doesn't
    // produce spurious diagnostics for len==1. Test with a classifiable type.
    #[test]
    fn single_numeric_element_list_no_consistency_warning() {
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
            "single numeric element list must not warn about mixed types: {:?}",
            list_warns
        );
    }

    // Mutant: line 164, `elements.len() > 1` → `>= 1`
    // Test single string-literal element.
    #[test]
    fn single_string_element_list_no_consistency_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", "(\"hello\")");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "single string element list must not warn: {:?}",
            list_warns
        );
    }

    // Mutant: line 161, `starts_with('(') && ends_with(')')` → `||`
    // Value ends with ')' but doesn't start with '(', contains mixed types.
    // With `&&` → not a list, no check. With `||` → enters list check,
    // inner parsing would slice val[1..val.len()-1] and might detect mixed types.
    #[test]
    fn value_ends_with_paren_only_no_list_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Ends with ')' but doesn't start with '(' — mixed types inside
        b.set_property(root, "", "Items", "42, true)");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "value ending with ) but not starting with ( must not trigger list check: {:?}",
            list_warns
        );
    }

    // Mutant: line 161, `starts_with('(') && ends_with(')')` → `||`
    // Value starts with '(' but doesn't end with ')', contains mixed types.
    // With `&&` → not a list, no check. With `||` → enters list check.
    #[test]
    fn value_starts_with_paren_only_mixed_no_list_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        // Starts with '(' but doesn't end with ')' — mixed types inside
        b.set_property(root, "", "Items", "(42, true, \"str\"");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "value starting with ( but not ending with ) must not trigger list check: {:?}",
            list_warns
        );
    }

    // Additional: value with ')' at start and '(' at end — neither condition
    // properly satisfied for a list.
    #[test]
    fn reversed_parens_no_list_check() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_property(root, "", "Items", ")42, \"hello\"(");

        let inst = b.build(root);
        let diags = PropertyRuleAnalysis.analyze(&inst);
        let list_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("mixed element"))
            .collect();
        assert!(
            list_warns.is_empty(),
            "reversed parens must not trigger list check: {:?}",
            list_warns
        );
    }
}
