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
//! - **PROP-ARITHMETIC** — Arithmetic in a property value (`5 * 8 ms`) is not
//!   AADL: an error, and the value is ignored (#546)

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::PropertyExpr;

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
            // Prefer the lowered range: its bounds are the real values. Read
            // from text, the bound `2 * 30` in `10 .. 2 * 30` would be taken
            // by `parse_numeric_value` as its first operand, 2, and report a
            // false inversion (#546). An arithmetic bound is not AADL and is
            // not compared at all; PROP-ARITHMETIC reports it.
            let bounds = match &pv.typed_expr {
                Some(PropertyExpr::Range { min, max, .. }) => {
                    match (bound_text(min), bound_text(max)) {
                        (Some(l), Some(h)) => Some((l, h)),
                        _ => None,
                    }
                }
                _ => val
                    .split_once("..")
                    .map(|(l, h)| (l.trim().to_string(), h.trim().to_string())),
            };
            // Look for range pattern: "number .. number"
            if let Some((low_str, high_str)) = bounds {
                let low_str = low_str.as_str();
                let high_str = high_str.as_str();
                if range_is_inverted(low_str, high_str) == Some(true) {
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

/// The text of an evaluated numeric range bound (`40 ms`, `7`), or `None`
/// for anything else — an unevaluated arithmetic bound among them.
fn bound_text(expr: &PropertyExpr) -> Option<String> {
    let (num, unit) = match expr {
        PropertyExpr::Integer(v, u) => (v.to_string(), u),
        PropertyExpr::Real(s, u) => (s.clone(), u),
        _ => return None,
    };
    Some(match unit {
        Some(u) => format!("{num} {u}"),
        None => num,
    })
}

/// Try to parse a numeric value string, stripping any trailing unit.
/// Does this bound carry a unit suffix (`20 us`) rather than being a bare
/// number (`20`)?
fn has_unit(s: &str) -> bool {
    s.trim()
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Is this range's lower bound greater than its upper bound?
///
/// `None` means the two ends could not be put on a common scale. That is NOT
/// the same as "correctly ordered" and must not be reported as ok — returning
/// a plain bool here would reintroduce the bug this exists to fix.
///
/// ## Why this is not a magnitude comparison (#454)
///
/// It used to be. `parse_numeric_value` strips the unit and returns the bare
/// number, so `200 ns .. 20 us` compared `200 > 20` and errored on a valid
/// range — and, far worse, `20 us .. 200 ns` compared `20 > 200`, found it
/// false, and passed a genuinely INVERTED range in silence. The false negative
/// is the dangerous half: `Compute_Execution_Time` feeds the latency and
/// scheduling analyses, so a WCET envelope declared backwards in mixed units
/// reached a timing argument with nothing said.
///
/// Mixed units are not exotic — `ns` for a tight compute path beside `us` or
/// `ms` for a deadline is the natural way to write these, which is how the
/// reporting model came to have one.
///
/// Units are normalised with `spar_hir_def::property_value`'s existing tables
/// (times to picoseconds, sizes to bits) rather than a second unit table here:
/// two tables would be two things to drift apart.
fn range_is_inverted(low: &str, high: &str) -> Option<bool> {
    use spar_hir_def::property_value::{parse_size_value, parse_time_value};

    // Same dimension, unit-aware: both ends normalise onto one scale, so mixed
    // units compare correctly in both directions.
    if let (Some(l), Some(h)) = (parse_time_value(low), parse_time_value(high)) {
        return Some(l > h);
    }
    if let (Some(l), Some(h)) = (parse_size_value(low), parse_size_value(high)) {
        return Some(l > h);
    }

    // Bare numbers: comparing magnitudes is correct, because there are no units
    // to lose. Guarded so a bound that DOES carry a unit never reaches here —
    // falling through to a magnitude compare is precisely the old defect.
    if !has_unit(low)
        && !has_unit(high)
        && let (Ok(l), Ok(h)) = (parse_numeric_value(low), parse_numeric_value(high))
    {
        return Some(l > h);
    }

    // Mixed dimensions, an unknown unit, or a unit on one end only: not
    // comparable. Saying nothing is honest; saying "ok" would not be.
    None
}

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
            // PROP-ARITHMETIC (#546): arithmetic is not AADL. The property
            // map already reads such a value as absent; say so here, or the
            // property would silently vanish.
            if pv.is_arithmetic() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "property '{}' value '{}': {}",
                        pv.name,
                        val,
                        spar_hir_def::property_arith::NOT_AADL
                    ),
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

        /// A value with its lowered typed expression, as instantiation
        /// records it.
        fn set_typed(
            &mut self,
            comp: ComponentInstanceIdx,
            name: &str,
            value: &str,
            e: PropertyExpr,
        ) {
            let map = self.property_maps.entry(comp).or_default();
            map.add(PropertyValue {
                name: PropertyRef {
                    property_set: None,
                    property_name: Name::new(name),
                },
                value: value.to_string(),
                typed_expr: Some(e),
                is_append: false,
            });
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
                typed_expr: None,
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
                feature_property_maps: Default::default(),
                connection_property_maps: Default::default(),
                emv2_models: Default::default(),
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

    /// The 2x2 from #454, pinned as a table.
    ///
    /// Rows b and c are the controls that made the original report a UNIT bug
    /// rather than a broken comparison: with both ends in the same unit the
    /// check was already right in both directions, so only normalisation was
    /// missing. They stay here for the same reason — if a future change breaks
    /// the comparison itself, these fail and the mixed-unit rows alone would
    /// not have told us which half broke.
    #[test]
    fn range_ordering_is_unit_aware() {
        let cases: &[(&str, &str, bool, &str)] = &[
            // low,        high,          inverted?, why it is in the table
            (
                "200 ns",
                "20 us",
                false,
                "#454 row 1 — was a FALSE POSITIVE",
            ),
            ("200 ns", "20000 ns", false, "control: same unit, ordered"),
            ("20000 ns", "200 ns", true, "control: same unit, inverted"),
            (
                "20 us",
                "200 ns",
                true,
                "#454 row 4 — was a SILENT FALSE NEGATIVE",
            ),
        ];
        for (low, high, want, why) in cases {
            assert_eq!(
                range_is_inverted(low, high),
                Some(*want),
                "{low} .. {high} ({why})"
            );
        }
    }

    /// Bare numbers still compare by magnitude — there is no unit to lose, and
    /// breaking this would be a regression dressed as a fix.
    #[test]
    fn unitless_ranges_still_compare_by_magnitude() {
        assert_eq!(range_is_inverted("1", "2"), Some(false));
        assert_eq!(range_is_inverted("2", "1"), Some(true));
        assert_eq!(range_is_inverted("1.5", "2.5"), Some(false));
    }

    /// Sizes get the same treatment as times, through the same shared tables.
    #[test]
    fn size_ranges_are_unit_aware() {
        assert_eq!(range_is_inverted("1 KByte", "1 MByte"), Some(false));
        assert_eq!(range_is_inverted("1 MByte", "1 KByte"), Some(true));
    }

    /// What CANNOT be compared must not be reported as ordered.
    ///
    /// This is the half that keeps the fix from becoming the next silent
    /// failure: a bound carrying a unit must never fall through to a magnitude
    /// comparison with the unit discarded. `None` here means "said nothing",
    /// which is honest; `Some(false)` would mean "declared it fine".
    #[test]
    fn incomparable_bounds_return_none_not_ok() {
        // a unit on one end only
        assert_eq!(range_is_inverted("20 us", "200"), None);
        assert_eq!(range_is_inverted("200", "20 us"), None);
        // different dimensions
        assert_eq!(range_is_inverted("20 us", "1 KByte"), None);
        // an unknown unit
        assert_eq!(range_is_inverted("20 furlongs", "200 furlongs"), None);
    }

    /// A range whose bounds are EQUAL is valid, not inverted.
    ///
    /// `low <= high` is the rule, so `1 KByte .. 1 KByte` is a degenerate but
    /// legal range. Found by mutation testing: `replace > with >=` in the size
    /// branch survived, because every other case here has distinct bounds and
    /// `>` and `>=` differ only when they are equal. Under `>=` a model
    /// declaring a fixed-size or fixed-duration property as a one-point range
    /// would be reported as inverted.
    ///
    /// Both dimensions are covered, not just the one that survived — the time
    /// branch is the same expression one line up and would survive the same
    /// mutation the moment whichever test currently covers it changes.
    #[test]
    fn equal_bounds_are_not_inverted() {
        assert_eq!(range_is_inverted("1 KByte", "1 KByte"), Some(false));
        assert_eq!(range_is_inverted("20 us", "20 us"), Some(false));
        assert_eq!(range_is_inverted("20 us", "20000 ns"), Some(false));
        assert_eq!(range_is_inverted("7", "7"), Some(false));
    }

    /// Non-vacuity: the function must be able to return BOTH verdicts on real
    /// input, or a table of `Some(false)` expectations would pass against a
    /// function stuck on one answer.
    #[test]
    fn the_range_verdict_discriminates() {
        assert_ne!(
            range_is_inverted("200 ns", "20 us"),
            range_is_inverted("20 us", "200 ns")
        );
    }

    // ── PROP-ARITHMETIC and range bounds read from the lowered value (#546) ──

    fn int(v: i64, unit: Option<&str>) -> PropertyExpr {
        PropertyExpr::Integer(v, unit.map(Name::new))
    }

    fn range(min: PropertyExpr, max: PropertyExpr) -> PropertyExpr {
        PropertyExpr::Range {
            min: Box::new(min),
            max: Box::new(max),
            delta: None,
        }
    }

    fn arith(l: PropertyExpr, r: PropertyExpr) -> PropertyExpr {
        PropertyExpr::BinaryOp {
            op: BinaryOpKind::Mul,
            lhs: Box::new(l),
            rhs: Box::new(r),
        }
    }

    fn run(value: &str, e: PropertyExpr) -> Vec<AnalysisDiagnostic> {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.set_typed(root, "Weight", value, e);
        PropertyRuleAnalysis.analyze(&b.build(root))
    }

    fn range_errors(d: &[AnalysisDiagnostic]) -> usize {
        d.iter()
            .filter(|d| d.message.contains("lower bound"))
            .count()
    }

    fn arith_errors(d: &[AnalysisDiagnostic]) -> Vec<&AnalysisDiagnostic> {
        d.iter()
            .filter(|d| d.message.contains(spar_hir_def::property_arith::NOT_AADL))
            .collect()
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn range_order_reads_the_lowered_bounds_not_the_first_operand() {
        // Read from text, `2 * 30` is its first operand, 2 < 10: a false
        // inversion. The lowered bound is arithmetic and is not compared.
        let d = run(
            "10 .. 2 * 30",
            range(int(10, None), arith(int(2, None), int(30, None))),
        );
        assert_eq!(range_errors(&d), 0, "{d:?}");
        // A real inversion is still caught from the lowered bounds...
        assert_eq!(
            range_errors(&run("10 .. 5", range(int(10, None), int(5, None)))),
            1
        );
        // ...and a signed minimum keeps its sign.
        assert_eq!(
            range_errors(&run("-180 .. 180", range(int(-180, None), int(180, None)))),
            0
        );
        assert_eq!(
            range_errors(&run("180 .. -180", range(int(180, None), int(-180, None)))),
            1
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn range_order_compares_real_lowered_bounds() {
        // A real bound is a literal, not arithmetic: the lowered range is
        // still compared, and the message names both bounds with units.
        let real = |v: &str| PropertyExpr::Real(v.to_string(), Some(Name::new("ms")));
        let d = run("2.5 ms .. 1.5 ms", range(real("2.5"), real("1.5")));
        assert_eq!(range_errors(&d), 1, "{d:?}");
        assert!(
            d.iter().any(|d| d
                .message
                .contains("lower bound (2.5 ms) > upper bound (1.5 ms)")),
            "{d:?}"
        );
        assert_eq!(
            range_errors(&run("1.5 ms .. 2.5 ms", range(real("1.5"), real("2.5")))),
            0
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn arithmetic_is_an_error_saying_it_is_not_aadl() {
        let d = run("5 * 8 ms", arith(int(5, None), int(8, Some("ms"))));
        let errs = arith_errors(&d);
        assert_eq!(errs.len(), 1, "{d:?}");
        assert_eq!(errs[0].severity, Severity::Error);
        assert!(
            errs[0].message.contains("'5 * 8 ms'"),
            "{}",
            errs[0].message
        );
        assert!(errs[0].message.contains("Weight"), "{}", errs[0].message);
        // Inside a range bound, too: one error for the value.
        let d = run(
            "1 ms .. 2 * 1 ms",
            range(int(1, Some("ms")), arith(int(2, None), int(1, Some("ms")))),
        );
        assert_eq!(arith_errors(&d).len(), 1, "{d:?}");
        // Literals, signed literals and signed constants are not reported.
        assert!(arith_errors(&run("40 ms", int(40, Some("ms")))).is_empty());
        assert!(
            arith_errors(&run("-180 .. 180", range(int(-180, None), int(180, None)))).is_empty()
        );
        assert!(arith_errors(&run("-C", PropertyExpr::Enum(Name::new("C")))).is_empty());
    }
}
