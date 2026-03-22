//! Typed property accessors for analysis passes (STPA-REQ-015).
//!
//! All analysis passes must access property values through these functions
//! rather than parsing raw strings directly. This module consolidates the
//! duplicate helpers that were previously spread across scheduling, latency,
//! resource_budget, and arinc653 modules.

use spar_hir_def::properties::PropertyMap;
use spar_hir_def::property_value::{parse_size_value, parse_time_value};

/// Get a timing property value in picoseconds.
///
/// Looks up the property in the `Timing_Properties` set first, then
/// falls back to an unqualified lookup.
pub fn get_timing_property(props: &PropertyMap, name: &str) -> Option<u64> {
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
    let raw = props
        .get("Memory_Properties", name)
        .or_else(|| props.get("", name))?;
    parse_size_value(raw)
}

/// Get memory binding reference name from `Actual_Memory_Binding`.
pub fn get_memory_binding(props: &PropertyMap) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
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
                is_append: false,
            });
        }
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
}
