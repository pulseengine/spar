//! Typed property accessors for analysis passes (STPA-REQ-015).
//!
//! All analysis passes must access property values through these functions
//! rather than parsing raw strings directly. This module consolidates the
//! duplicate helpers that were previously spread across scheduling, latency,
//! resource_budget, and arinc653 modules.
//!
//! Each accessor tries [`PropertyMap::get_typed`] first (returning a
//! [`PropertyExpr`]), falling back to [`PropertyMap::get`] with string
//! parsing when no typed expression is available.

use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::PropertyMap;
use spar_hir_def::property_value::{parse_size_value, parse_time_value};

// ── Typed-value extraction helpers ────────────────────────────────────

/// Standard AADL time units and their conversion factors to picoseconds.
const TIME_UNIT_FACTORS: &[(&str, u64)] = &[
    ("ps", 1),
    ("ns", 1_000),
    ("us", 1_000_000),
    ("ms", 1_000_000_000),
    ("sec", 1_000_000_000_000),
    ("min", 60_000_000_000_000),
    ("hr", 3_600_000_000_000_000),
];

/// Standard AADL size units and their conversion factors to bits.
const SIZE_UNIT_FACTORS: &[(&str, u64)] = &[
    ("bits", 1),
    ("Bytes", 8),
    ("KByte", 8 * 1024),
    ("MByte", 8 * 1024 * 1024),
    ("GByte", 8 * 1024 * 1024 * 1024),
    ("TByte", 8 * 1024 * 1024 * 1024 * 1024),
];

/// Look up a unit name (case-insensitive) in a table and return its factor.
fn unit_factor(table: &[(&str, u64)], unit: &str) -> Option<u64> {
    table
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

/// Extract a time value in picoseconds from a typed [`PropertyExpr`].
///
/// Handles `Integer(n, Some(unit))`, `Real(s, Some(unit))`, and
/// `UnitValue(inner, unit)` forms.
pub fn extract_time_ps(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(val, Some(unit)) => {
            let factor = unit_factor(TIME_UNIT_FACTORS, unit.as_str())?;
            Some((*val as u64) * factor)
        }
        PropertyExpr::Integer(val, None) => {
            // Bare integer -- assume picoseconds (AADL base unit for time).
            Some(*val as u64)
        }
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            let factor = unit_factor(TIME_UNIT_FACTORS, unit.as_str())?;
            Some((v * factor as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = unit_factor(TIME_UNIT_FACTORS, unit.as_str())?;
            match inner.as_ref() {
                PropertyExpr::Integer(val, _) => Some((*val as u64) * factor),
                PropertyExpr::Real(s, _) => {
                    let v: f64 = s.parse().ok()?;
                    Some((v * factor as f64) as u64)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Extract a size value in bits from a typed [`PropertyExpr`].
pub fn extract_size_bits(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(val, Some(unit)) => {
            let factor = unit_factor(SIZE_UNIT_FACTORS, unit.as_str())?;
            Some((*val as u64) * factor)
        }
        PropertyExpr::Integer(val, None) => {
            // Bare integer -- assume bits (AADL base unit for size).
            Some(*val as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = unit_factor(SIZE_UNIT_FACTORS, unit.as_str())?;
            match inner.as_ref() {
                PropertyExpr::Integer(val, _) => Some((*val as u64) * factor),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Extract a time range (min, max) in picoseconds from a typed [`PropertyExpr`].
///
/// Handles `Range { min, max, .. }` and single time values (returned as
/// identical min and max).
pub fn extract_time_range_ps(expr: &PropertyExpr) -> Option<(u64, u64)> {
    match expr {
        PropertyExpr::Range { min, max, .. } => {
            let min_ps = extract_time_ps(min)?;
            let max_ps = extract_time_ps(max)?;
            Some((min_ps, max_ps))
        }
        _ => {
            let val = extract_time_ps(expr)?;
            Some((val, val))
        }
    }
}

/// Extract a reference target string from a typed [`PropertyExpr::ReferenceValue`].
pub fn extract_typed_reference(expr: &PropertyExpr) -> Option<&str> {
    match expr {
        PropertyExpr::ReferenceValue(s) => {
            let s = s.trim();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

/// Extract a boolean from a typed [`PropertyExpr`].
pub fn extract_bool(expr: &PropertyExpr) -> Option<bool> {
    match expr {
        PropertyExpr::Boolean(b) => Some(*b),
        _ => None,
    }
}

/// Extract a u64 integer from a typed [`PropertyExpr`].
pub fn extract_integer(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, _) => {
            if *v >= 0 {
                Some(*v as u64)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract an f64 from a typed [`PropertyExpr`].
pub fn extract_real(expr: &PropertyExpr) -> Option<f64> {
    match expr {
        PropertyExpr::Real(s, _) => s.parse::<f64>().ok(),
        PropertyExpr::Integer(v, _) => Some(*v as f64),
        _ => None,
    }
}

/// Extract a string representation from a typed [`PropertyExpr`].
///
/// Works for `StringLit`, `Enum`, and falls through to `None` for
/// non-string-like expressions.
pub fn extract_string(expr: &PropertyExpr) -> Option<String> {
    match expr {
        PropertyExpr::StringLit(s) => Some(s.clone()),
        PropertyExpr::Enum(name) => Some(name.as_str().to_string()),
        _ => None,
    }
}

// ── Helper: try typed then fall back to string for a given set+name ───

/// Try `get_typed` for a property across qualified and unqualified names,
/// returning the first match.
fn get_typed_qualified<'a>(
    props: &'a PropertyMap,
    set: &str,
    name: &str,
) -> Option<&'a PropertyExpr> {
    props
        .get_typed(set, name)
        .or_else(|| props.get_typed("", name))
}

// ── Public accessor functions ─────────────────────────────────────────

/// Get a timing property value in picoseconds.
///
/// Looks up the property in the `Timing_Properties` set first, then
/// falls back to an unqualified lookup.  Tries typed access first.
pub fn get_timing_property(props: &PropertyMap, name: &str) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, "Timing_Properties", name) {
        if let Some(ps) = extract_time_ps(expr) {
            return Some(ps);
        }
    }

    // String fallback
    let raw = props
        .get("Timing_Properties", name)
        .or_else(|| props.get("", name))?;
    parse_time_value(raw)
}

/// Get `Compute_Execution_Time` (worst-case from range) in picoseconds.
///
/// This property is typically a range (e.g., "1 ms .. 5 ms"). We take the
/// worst case (max). If it is a single value, we use that.
pub fn get_execution_time(props: &PropertyMap) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, "Timing_Properties", "Compute_Execution_Time") {
        if let Some((_, max_ps)) = extract_time_range_ps(expr) {
            return Some(max_ps);
        }
    }

    // String fallback
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

/// Get `Compute_Execution_Time` as a (min, max) pair in picoseconds.
///
/// For a range "1 ms .. 5 ms", returns (1_000_000_000, 5_000_000_000).
/// For a single value "3 ms", returns (3_000_000_000, 3_000_000_000).
pub fn get_execution_time_range(props: &PropertyMap) -> Option<(u64, u64)> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, "Timing_Properties", "Compute_Execution_Time") {
        if let Some(range) = extract_time_range_ps(expr) {
            return Some(range);
        }
    }

    // String fallback
    let raw = props
        .get("Timing_Properties", "Compute_Execution_Time")
        .or_else(|| props.get("", "Compute_Execution_Time"))?;

    if let Some((min_str, max_str)) = raw.split_once("..") {
        let min_ps = parse_time_value(min_str.trim())?;
        let max_ps = parse_time_value(max_str.trim())?;
        Some((min_ps, max_ps))
    } else {
        let val = parse_time_value(raw)?;
        Some((val, val))
    }
}

/// Get execution time in picoseconds, also checking the `Execution_Time`
/// property name (used by ARINC 653 virtual processors).
///
/// Checks `Execution_Time` first, then falls back to `Compute_Execution_Time`.
/// Handles range format "min .. max" by taking the worst case.
pub fn get_execution_time_or_exec(props: &PropertyMap) -> Option<u64> {
    // Typed path: try Execution_Time then Compute_Execution_Time
    let typed = get_typed_qualified(props, "Timing_Properties", "Execution_Time")
        .or_else(|| get_typed_qualified(props, "Timing_Properties", "Compute_Execution_Time"));
    if let Some(expr) = typed {
        if let Some((_, max_ps)) = extract_time_range_ps(expr) {
            return Some(max_ps);
        }
    }

    // String fallback
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

/// Get processor binding reference name from `Actual_Processor_Binding`.
pub fn get_processor_binding(props: &PropertyMap) -> Option<String> {
    // Typed path
    if let Some(expr) =
        get_typed_qualified(props, "Deployment_Properties", "Actual_Processor_Binding")
    {
        if let Some(target) = extract_typed_reference(expr) {
            return Some(target.to_string());
        }
    }

    // String fallback
    let raw = props
        .get("Deployment_Properties", "Actual_Processor_Binding")
        .or_else(|| props.get("", "Actual_Processor_Binding"))?;

    extract_reference_target(raw).map(|s| s.to_string())
}

/// Get a memory/size property in bits.
///
/// Looks up the property in the `Memory_Properties` set first, then
/// falls back to an unqualified lookup.
pub fn get_size_property(props: &PropertyMap, name: &str) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, "Memory_Properties", name) {
        if let Some(bits) = extract_size_bits(expr) {
            return Some(bits);
        }
    }

    // String fallback
    let raw = props
        .get("Memory_Properties", name)
        .or_else(|| props.get("", name))?;
    parse_size_value(raw)
}

/// Get memory binding reference name from `Actual_Memory_Binding`.
pub fn get_memory_binding(props: &PropertyMap) -> Option<String> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, "Deployment_Properties", "Actual_Memory_Binding")
    {
        if let Some(target) = extract_typed_reference(expr) {
            return Some(target.to_string());
        }
    }

    // String fallback
    let raw = props
        .get("Deployment_Properties", "Actual_Memory_Binding")
        .or_else(|| props.get("", "Actual_Memory_Binding"))?;
    extract_reference_target(raw).map(|s| s.to_string())
}

/// Extract the target name from a `reference(name)` or `(reference(name))` string.
///
/// Returns `None` if the string does not match the expected format or the
/// target name is empty.
pub fn extract_reference_target(val: &str) -> Option<&str> {
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

// ── AI_ML property accessors ───────────────────────────────────────

const AI_ML: &str = "AI_ML";

/// Get `AI_ML::Inference_Latency` as a (min, max) range in picoseconds.
///
/// Handles range format "20 ms .. 60 ms" and single values.
pub fn get_inference_latency_range(props: &PropertyMap) -> Option<(u64, u64)> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, "Inference_Latency") {
        if let Some(range) = extract_time_range_ps(expr) {
            return Some(range);
        }
    }

    // String fallback
    let raw = props
        .get(AI_ML, "Inference_Latency")
        .or_else(|| props.get("", "Inference_Latency"))?;
    if let Some((min_str, max_str)) = raw.split_once("..") {
        let min_ps = parse_time_value(min_str.trim())?;
        let max_ps = parse_time_value(max_str.trim())?;
        Some((min_ps, max_ps))
    } else {
        let val = parse_time_value(raw)?;
        Some((val, val))
    }
}

/// Get `AI_ML::Fallback_Latency` in picoseconds.
pub fn get_fallback_latency(props: &PropertyMap) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, "Fallback_Latency") {
        if let Some(ps) = extract_time_ps(expr) {
            return Some(ps);
        }
    }

    // String fallback
    let raw = props
        .get(AI_ML, "Fallback_Latency")
        .or_else(|| props.get("", "Fallback_Latency"))?;
    parse_time_value(raw)
}

/// Get `AI_ML::Confidence_Threshold` as f64 (0.0-1.0).
pub fn get_confidence_threshold(props: &PropertyMap) -> Option<f64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, "Confidence_Threshold") {
        if let Some(v) = extract_real(expr) {
            return Some(v);
        }
    }

    // String fallback
    let raw = props
        .get(AI_ML, "Confidence_Threshold")
        .or_else(|| props.get("", "Confidence_Threshold"))?;
    raw.trim().parse::<f64>().ok()
}

/// Get a string AI_ML property (Inference_Mode, Fallback_Strategy, OOD_Detection_Method, etc.).
pub fn get_ai_ml_string(props: &PropertyMap, name: &str) -> Option<String> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, name) {
        if let Some(s) = extract_string(expr) {
            return Some(s);
        }
    }

    // String fallback
    props
        .get(AI_ML, name)
        .or_else(|| props.get("", name))
        .map(|s| s.to_string())
}

/// Get `AI_ML::OOD_Detection_Enabled` as bool.
pub fn get_ai_ml_bool(props: &PropertyMap, name: &str) -> Option<bool> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, name) {
        if let Some(b) = extract_bool(expr) {
            return Some(b);
        }
    }

    // String fallback
    let raw = props.get(AI_ML, name).or_else(|| props.get("", name))?;
    match raw.trim().to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Get `AI_ML::Max_Batch_Size` as integer.
pub fn get_ai_ml_integer(props: &PropertyMap, name: &str) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, name) {
        if let Some(v) = extract_integer(expr) {
            return Some(v);
        }
    }

    // String fallback
    let raw = props.get(AI_ML, name).or_else(|| props.get("", name))?;
    raw.trim().parse::<u64>().ok()
}

/// Get `AI_ML::Drift_Detection_Window` in picoseconds.
pub fn get_drift_detection_window(props: &PropertyMap) -> Option<u64> {
    // Typed path
    if let Some(expr) = get_typed_qualified(props, AI_ML, "Drift_Detection_Window") {
        if let Some(ps) = extract_time_ps(expr) {
            return Some(ps);
        }
    }

    // String fallback
    let raw = props
        .get(AI_ML, "Drift_Detection_Window")
        .or_else(|| props.get("", "Drift_Detection_Window"))?;
    parse_time_value(raw)
}

/// Check whether a component has any AI_ML property set, indicating it is an AI/ML component.
pub fn is_ai_ml_component(props: &PropertyMap) -> bool {
    get_inference_latency_range(props).is_some()
        || get_ai_ml_string(props, "Model_Version").is_some()
        || get_ai_ml_string(props, "Inference_Mode").is_some()
        || get_confidence_threshold(props).is_some()
        || get_ai_ml_string(props, "Fallback_Strategy").is_some()
        || get_ai_ml_string(props, "Model_Format").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::item_tree::PropertyExpr;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::PropertyValue;

    fn make_props(entries: &[(&str, &str, &str)]) -> PropertyMap {
        let mut props = PropertyMap::new();
        for &(set, name, value) in entries {
            props.add(PropertyValue {
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
        props
    }

    /// Build a PropertyMap with a typed expression AND a raw string value.
    fn make_typed_props(set: &str, name: &str, value: &str, expr: PropertyExpr) -> PropertyMap {
        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: if set.is_empty() {
                    None
                } else {
                    Some(Name::new(set))
                },
                property_name: Name::new(name),
            },
            value: value.to_string(),
            typed_expr: Some(expr),
            is_append: false,
        });
        props
    }

    // ── get_timing_property ─────────────────────────────────────

    #[test]
    fn timing_property_parses_ms() {
        let props = make_props(&[("Timing_Properties", "Period", "10 ms")]);
        assert_eq!(get_timing_property(&props, "Period"), Some(10_000_000_000));
    }

    #[test]
    fn timing_property_parses_sec() {
        let props = make_props(&[("Timing_Properties", "Period", "1 sec")]);
        assert_eq!(
            get_timing_property(&props, "Period"),
            Some(1_000_000_000_000)
        );
    }

    #[test]
    fn timing_property_parses_us() {
        let props = make_props(&[("Timing_Properties", "Deadline", "500 us")]);
        assert_eq!(get_timing_property(&props, "Deadline"), Some(500_000_000));
    }

    #[test]
    fn timing_property_unqualified_fallback() {
        let props = make_props(&[("", "Period", "20 ms")]);
        assert_eq!(get_timing_property(&props, "Period"), Some(20_000_000_000));
    }

    #[test]
    fn timing_property_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(get_timing_property(&props, "Period"), None);
    }

    // ── get_execution_time ──────────────────────────────────────

    #[test]
    fn execution_time_single_value() {
        let props = make_props(&[("Timing_Properties", "Compute_Execution_Time", "5 ms")]);
        assert_eq!(get_execution_time(&props), Some(5_000_000_000));
    }

    #[test]
    fn execution_time_range_uses_worst_case() {
        let props = make_props(&[(
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms .. 5 ms",
        )]);
        assert_eq!(get_execution_time(&props), Some(5_000_000_000));
    }

    #[test]
    fn execution_time_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(get_execution_time(&props), None);
    }

    // ── get_execution_time_range ────────────────────────────────

    #[test]
    fn execution_time_range_parses_range() {
        let props = make_props(&[(
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms .. 5 ms",
        )]);
        assert_eq!(
            get_execution_time_range(&props),
            Some((1_000_000_000, 5_000_000_000))
        );
    }

    #[test]
    fn execution_time_range_single_value() {
        let props = make_props(&[("Timing_Properties", "Compute_Execution_Time", "3 ms")]);
        assert_eq!(
            get_execution_time_range(&props),
            Some((3_000_000_000, 3_000_000_000))
        );
    }

    #[test]
    fn execution_time_range_missing() {
        let props = PropertyMap::new();
        assert_eq!(get_execution_time_range(&props), None);
    }

    // ── get_execution_time_or_exec ──────────────────────────────

    #[test]
    fn execution_time_or_exec_uses_execution_time() {
        let props = make_props(&[("Timing_Properties", "Execution_Time", "20 ms")]);
        assert_eq!(get_execution_time_or_exec(&props), Some(20_000_000_000));
    }

    #[test]
    fn execution_time_or_exec_falls_back_to_compute() {
        let props = make_props(&[("Timing_Properties", "Compute_Execution_Time", "3 ms")]);
        assert_eq!(get_execution_time_or_exec(&props), Some(3_000_000_000));
    }

    #[test]
    fn execution_time_or_exec_range() {
        let props = make_props(&[("Timing_Properties", "Execution_Time", "10 ms .. 30 ms")]);
        assert_eq!(get_execution_time_or_exec(&props), Some(30_000_000_000));
    }

    // ── get_processor_binding ───────────────────────────────────

    #[test]
    fn processor_binding_reference_format() {
        let props = make_props(&[(
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        )]);
        assert_eq!(get_processor_binding(&props), Some("cpu1".to_string()));
    }

    #[test]
    fn processor_binding_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(get_processor_binding(&props), None);
    }

    // ── get_size_property ───────────────────────────────────────

    #[test]
    fn size_property_parses_kbyte() {
        let props = make_props(&[("Memory_Properties", "Memory_Size", "256 KByte")]);
        assert_eq!(
            get_size_property(&props, "Memory_Size"),
            Some(256 * 8 * 1024)
        );
    }

    #[test]
    fn size_property_parses_mbyte() {
        let props = make_props(&[("Memory_Properties", "Memory_Size", "1 MByte")]);
        assert_eq!(
            get_size_property(&props, "Memory_Size"),
            Some(8 * 1024 * 1024)
        );
    }

    #[test]
    fn size_property_unqualified_fallback() {
        let props = make_props(&[("", "Source_Code_Size", "100 KByte")]);
        assert_eq!(
            get_size_property(&props, "Source_Code_Size"),
            Some(100 * 8 * 1024)
        );
    }

    #[test]
    fn size_property_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(get_size_property(&props, "Memory_Size"), None);
    }

    // ── get_memory_binding ──────────────────────────────────────

    #[test]
    fn memory_binding_reference_format() {
        let props = make_props(&[(
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
        )]);
        assert_eq!(get_memory_binding(&props), Some("ram".to_string()));
    }

    #[test]
    fn memory_binding_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(get_memory_binding(&props), None);
    }

    // ── extract_reference_target ────────────────────────────────

    #[test]
    fn reference_target_standard_format() {
        assert_eq!(extract_reference_target("reference (cpu1)"), Some("cpu1"));
    }

    #[test]
    fn reference_target_nested_parens() {
        assert_eq!(
            extract_reference_target("(reference (myproc))"),
            Some("myproc")
        );
    }

    #[test]
    fn reference_target_no_reference_keyword() {
        assert_eq!(extract_reference_target("just a string"), None);
    }

    #[test]
    fn reference_target_empty_parens() {
        assert_eq!(extract_reference_target("reference ()"), None);
    }

    #[test]
    fn reference_target_no_closing_paren() {
        assert_eq!(extract_reference_target("reference (cpu"), None);
    }

    #[test]
    fn reference_target_no_opening_paren() {
        assert_eq!(extract_reference_target("reference cpu)"), None);
    }

    #[test]
    fn reference_target_whitespace_handling() {
        assert_eq!(
            extract_reference_target("  reference (  cpu1  )  "),
            Some("cpu1")
        );
    }

    // ── Typed extraction helper tests ──────────────────────────

    #[test]
    fn extract_time_ps_integer_with_unit() {
        let expr = PropertyExpr::Integer(10, Some(Name::new("ms")));
        assert_eq!(extract_time_ps(&expr), Some(10_000_000_000));
    }

    #[test]
    fn extract_time_ps_integer_sec() {
        let expr = PropertyExpr::Integer(2, Some(Name::new("sec")));
        assert_eq!(extract_time_ps(&expr), Some(2_000_000_000_000));
    }

    #[test]
    fn extract_time_ps_real_with_unit() {
        let expr = PropertyExpr::Real("1.5".to_string(), Some(Name::new("ms")));
        assert_eq!(extract_time_ps(&expr), Some(1_500_000_000));
    }

    #[test]
    fn extract_time_ps_unit_value() {
        let inner = PropertyExpr::Integer(500, None);
        let expr = PropertyExpr::UnitValue(Box::new(inner), Name::new("us"));
        assert_eq!(extract_time_ps(&expr), Some(500_000_000));
    }

    #[test]
    fn extract_time_ps_bare_integer() {
        let expr = PropertyExpr::Integer(42, None);
        assert_eq!(extract_time_ps(&expr), Some(42));
    }

    #[test]
    fn extract_time_ps_unknown_unit_returns_none() {
        let expr = PropertyExpr::Integer(10, Some(Name::new("furlongs")));
        assert_eq!(extract_time_ps(&expr), None);
    }

    #[test]
    fn extract_size_bits_integer_kbyte() {
        let expr = PropertyExpr::Integer(256, Some(Name::new("KByte")));
        assert_eq!(extract_size_bits(&expr), Some(256 * 8 * 1024));
    }

    #[test]
    fn extract_size_bits_unit_value_bytes() {
        let inner = PropertyExpr::Integer(100, None);
        let expr = PropertyExpr::UnitValue(Box::new(inner), Name::new("Bytes"));
        assert_eq!(extract_size_bits(&expr), Some(800));
    }

    #[test]
    fn extract_time_range_ps_range_expr() {
        let min = PropertyExpr::Integer(1, Some(Name::new("ms")));
        let max = PropertyExpr::Integer(5, Some(Name::new("ms")));
        let expr = PropertyExpr::Range {
            min: Box::new(min),
            max: Box::new(max),
            delta: None,
        };
        assert_eq!(
            extract_time_range_ps(&expr),
            Some((1_000_000_000, 5_000_000_000))
        );
    }

    #[test]
    fn extract_time_range_ps_single_value() {
        let expr = PropertyExpr::Integer(3, Some(Name::new("ms")));
        assert_eq!(
            extract_time_range_ps(&expr),
            Some((3_000_000_000, 3_000_000_000))
        );
    }

    #[test]
    fn extract_typed_reference_value() {
        let expr = PropertyExpr::ReferenceValue("cpu1".to_string());
        assert_eq!(extract_typed_reference(&expr), Some("cpu1"));
    }

    #[test]
    fn extract_typed_reference_empty() {
        let expr = PropertyExpr::ReferenceValue("".to_string());
        assert_eq!(extract_typed_reference(&expr), None);
    }

    #[test]
    fn extract_bool_true() {
        assert_eq!(extract_bool(&PropertyExpr::Boolean(true)), Some(true));
    }

    #[test]
    fn extract_bool_false() {
        assert_eq!(extract_bool(&PropertyExpr::Boolean(false)), Some(false));
    }

    #[test]
    fn extract_bool_non_boolean() {
        assert_eq!(extract_bool(&PropertyExpr::Integer(1, None)), None);
    }

    #[test]
    fn extract_integer_positive() {
        assert_eq!(extract_integer(&PropertyExpr::Integer(42, None)), Some(42));
    }

    #[test]
    fn extract_real_from_real() {
        assert_eq!(
            extract_real(&PropertyExpr::Real("3.14".to_string(), None)),
            Some(3.14)
        );
    }

    #[test]
    fn extract_real_from_integer() {
        assert_eq!(extract_real(&PropertyExpr::Integer(7, None)), Some(7.0));
    }

    #[test]
    fn extract_string_from_string_lit() {
        assert_eq!(
            extract_string(&PropertyExpr::StringLit("hello".to_string())),
            Some("hello".to_string())
        );
    }

    #[test]
    fn extract_string_from_enum() {
        assert_eq!(
            extract_string(&PropertyExpr::Enum(Name::new("Periodic"))),
            Some("Periodic".to_string())
        );
    }

    // ── Typed-first accessor tests ─────────────────────────────

    #[test]
    fn timing_property_prefers_typed_expr() {
        let props = make_typed_props(
            "Timing_Properties",
            "Period",
            "10 ms",
            PropertyExpr::Integer(20, Some(Name::new("ms"))),
        );
        // Typed value is 20 ms, raw string says 10 ms -- typed wins
        assert_eq!(get_timing_property(&props, "Period"), Some(20_000_000_000));
    }

    #[test]
    fn execution_time_typed_range() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(1, Some(Name::new("ms")))),
            max: Box::new(PropertyExpr::Integer(5, Some(Name::new("ms")))),
            delta: None,
        };
        let props = make_typed_props(
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms .. 5 ms",
            expr,
        );
        assert_eq!(get_execution_time(&props), Some(5_000_000_000));
    }

    #[test]
    fn execution_time_range_typed() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(2, Some(Name::new("ms")))),
            max: Box::new(PropertyExpr::Integer(8, Some(Name::new("ms")))),
            delta: None,
        };
        let props = make_typed_props(
            "Timing_Properties",
            "Compute_Execution_Time",
            "2 ms .. 8 ms",
            expr,
        );
        assert_eq!(
            get_execution_time_range(&props),
            Some((2_000_000_000, 8_000_000_000))
        );
    }

    #[test]
    fn processor_binding_typed_reference() {
        let props = make_typed_props(
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
            PropertyExpr::ReferenceValue("cpu1".to_string()),
        );
        assert_eq!(get_processor_binding(&props), Some("cpu1".to_string()));
    }

    #[test]
    fn size_property_typed_integer_with_unit() {
        let props = make_typed_props(
            "Memory_Properties",
            "Memory_Size",
            "256 KByte",
            PropertyExpr::Integer(256, Some(Name::new("KByte"))),
        );
        assert_eq!(
            get_size_property(&props, "Memory_Size"),
            Some(256 * 8 * 1024)
        );
    }

    #[test]
    fn memory_binding_typed_reference() {
        let props = make_typed_props(
            "Deployment_Properties",
            "Actual_Memory_Binding",
            "reference (ram)",
            PropertyExpr::ReferenceValue("ram".to_string()),
        );
        assert_eq!(get_memory_binding(&props), Some("ram".to_string()));
    }

    #[test]
    fn ai_ml_bool_typed() {
        let props = make_typed_props(
            "AI_ML",
            "OOD_Detection_Enabled",
            "true",
            PropertyExpr::Boolean(true),
        );
        assert_eq!(get_ai_ml_bool(&props, "OOD_Detection_Enabled"), Some(true));
    }

    #[test]
    fn ai_ml_integer_typed() {
        let props = make_typed_props(
            "AI_ML",
            "Max_Batch_Size",
            "16",
            PropertyExpr::Integer(16, None),
        );
        assert_eq!(get_ai_ml_integer(&props, "Max_Batch_Size"), Some(16));
    }

    #[test]
    fn confidence_threshold_typed_real() {
        let props = make_typed_props(
            "AI_ML",
            "Confidence_Threshold",
            "0.85",
            PropertyExpr::Real("0.85".to_string(), None),
        );
        assert_eq!(get_confidence_threshold(&props), Some(0.85));
    }

    #[test]
    fn ai_ml_string_typed_enum() {
        let props = make_typed_props(
            "AI_ML",
            "Inference_Mode",
            "Batch",
            PropertyExpr::Enum(Name::new("Batch")),
        );
        assert_eq!(
            get_ai_ml_string(&props, "Inference_Mode"),
            Some("Batch".to_string())
        );
    }

    #[test]
    fn fallback_latency_typed() {
        let props = make_typed_props(
            "AI_ML",
            "Fallback_Latency",
            "50 ms",
            PropertyExpr::Integer(50, Some(Name::new("ms"))),
        );
        assert_eq!(get_fallback_latency(&props), Some(50_000_000_000));
    }

    #[test]
    fn drift_detection_window_typed() {
        let props = make_typed_props(
            "AI_ML",
            "Drift_Detection_Window",
            "100 ms",
            PropertyExpr::Integer(100, Some(Name::new("ms"))),
        );
        assert_eq!(get_drift_detection_window(&props), Some(100_000_000_000));
    }

    #[test]
    fn inference_latency_range_typed() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(20, Some(Name::new("ms")))),
            max: Box::new(PropertyExpr::Integer(60, Some(Name::new("ms")))),
            delta: None,
        };
        let props = make_typed_props("AI_ML", "Inference_Latency", "20 ms .. 60 ms", expr);
        assert_eq!(
            get_inference_latency_range(&props),
            Some((20_000_000_000, 60_000_000_000))
        );
    }

    // ── Typed with bad typed expr falls back to string ──────────

    #[test]
    fn typed_expr_bad_unit_falls_back_to_string() {
        let props = make_typed_props(
            "Timing_Properties",
            "Period",
            "10 ms",
            PropertyExpr::Integer(10, Some(Name::new("furlongs"))),
        );
        assert_eq!(get_timing_property(&props, "Period"), Some(10_000_000_000));
    }

    #[test]
    fn extract_time_ps_unit_value_with_real() {
        let inner = PropertyExpr::Real("2.5".to_string(), None);
        let expr = PropertyExpr::UnitValue(Box::new(inner), Name::new("ms"));
        assert_eq!(extract_time_ps(&expr), Some(2_500_000_000));
    }
}
