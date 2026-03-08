//! Property type system and validation (AS5506 Appendix A).
//!
//! Validates that [`PropertyExpr`] values conform to their declared
//! [`PropertyTypeDef`] types, and provides unit conversion utilities
//! for AADL's typed property system.
//!
//! # AADL Property Type System
//!
//! AADL properties have declared types (aadlinteger, aadlreal, aadlstring,
//! aadlboolean, enumeration, range, classifier, reference, record, list)
//! and values must conform to those types. This module performs that
//! validation.

use crate::item_tree::{PropertyExpr, PropertyTypeDef};

/// Result of validating a property expression against a type definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeCheckResult {
    /// The value conforms to the type.
    Ok,
    /// The value does not conform; includes an explanation.
    Error(String),
    /// The value is opaque and cannot be type-checked.
    Skipped,
}

/// Validate that a `PropertyExpr` conforms to a `PropertyTypeDef`.
///
/// Returns `TypeCheckResult::Ok` if valid, `Error` with explanation if not,
/// or `Skipped` if the expression is opaque and can't be checked.
pub fn check_type(expr: &PropertyExpr, type_def: &PropertyTypeDef) -> TypeCheckResult {
    match expr {
        PropertyExpr::Opaque(_) => TypeCheckResult::Skipped,
        _ => check_typed(expr, type_def),
    }
}

fn check_typed(expr: &PropertyExpr, type_def: &PropertyTypeDef) -> TypeCheckResult {
    match (expr, type_def) {
        // ── Integer ─────────────────────────────────────────────
        (PropertyExpr::Integer(val, _units), PropertyTypeDef::AadlInteger { range, .. }) => {
            if let Some((lo, hi)) = range {
                if val < lo || val > hi {
                    return TypeCheckResult::Error(format!(
                        "integer value {} is outside range {} .. {}",
                        val, lo, hi
                    ));
                }
            }
            TypeCheckResult::Ok
        }

        // Integer where UnitValue wraps an integer
        (PropertyExpr::UnitValue(inner, _unit), PropertyTypeDef::AadlInteger { .. }) => {
            check_typed(inner, type_def)
        }

        // ── Real ────────────────────────────────────────────────
        (PropertyExpr::Real(_, _), PropertyTypeDef::AadlReal { .. }) => TypeCheckResult::Ok,

        (PropertyExpr::UnitValue(inner, _unit), PropertyTypeDef::AadlReal { .. }) => {
            check_typed(inner, type_def)
        }

        // Integer is also acceptable where real is expected
        (PropertyExpr::Integer(_, _), PropertyTypeDef::AadlReal { .. }) => TypeCheckResult::Ok,

        // ── String ──────────────────────────────────────────────
        (PropertyExpr::StringLit(_), PropertyTypeDef::AadlString) => TypeCheckResult::Ok,

        // ── Boolean ─────────────────────────────────────────────
        (PropertyExpr::Boolean(_), PropertyTypeDef::AadlBoolean) => TypeCheckResult::Ok,

        // ── Enumeration ─────────────────────────────────────────
        (PropertyExpr::Enum(val), PropertyTypeDef::Enumeration(variants)) => {
            if variants.iter().any(|v| v.eq_ci(val)) {
                TypeCheckResult::Ok
            } else {
                TypeCheckResult::Error(format!(
                    "enumeration value '{}' is not one of: {}",
                    val,
                    variants
                        .iter()
                        .map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }

        // ── List ────────────────────────────────────────────────
        (PropertyExpr::List(items), PropertyTypeDef::ListOf(elem_type)) => {
            for (i, item) in items.iter().enumerate() {
                match check_typed(item, elem_type) {
                    TypeCheckResult::Ok | TypeCheckResult::Skipped => {}
                    TypeCheckResult::Error(e) => {
                        return TypeCheckResult::Error(format!(
                            "list element [{}]: {}",
                            i, e
                        ));
                    }
                }
            }
            TypeCheckResult::Ok
        }

        // ── Record ──────────────────────────────────────────────
        (PropertyExpr::Record(fields), PropertyTypeDef::RecordType(field_types)) => {
            for (field_name, field_val) in fields {
                if let Some((_name, field_type)) = field_types.iter().find(|(n, _)| n.eq_ci(field_name)) {
                    match check_typed(field_val, field_type) {
                        TypeCheckResult::Ok | TypeCheckResult::Skipped => {}
                        TypeCheckResult::Error(e) => {
                            return TypeCheckResult::Error(format!(
                                "record field '{}': {}",
                                field_name, e
                            ));
                        }
                    }
                } else {
                    return TypeCheckResult::Error(format!(
                        "record field '{}' is not declared in the type",
                        field_name
                    ));
                }
            }
            TypeCheckResult::Ok
        }

        // ── Range ───────────────────────────────────────────────
        (
            PropertyExpr::Range { min, max, .. },
            PropertyTypeDef::Range(inner_type),
        ) => {
            let min_result = check_typed(min, inner_type);
            if let TypeCheckResult::Error(e) = min_result {
                return TypeCheckResult::Error(format!("range min: {}", e));
            }
            let max_result = check_typed(max, inner_type);
            if let TypeCheckResult::Error(e) = max_result {
                return TypeCheckResult::Error(format!("range max: {}", e));
            }
            TypeCheckResult::Ok
        }

        // ── Classifier ──────────────────────────────────────────
        (PropertyExpr::ClassifierValue(_), PropertyTypeDef::Classifier(_)) => {
            // Category checking could be done here with scope resolution,
            // but for now accept any classifier reference
            TypeCheckResult::Ok
        }

        // ── Reference ───────────────────────────────────────────
        (PropertyExpr::ReferenceValue(_), PropertyTypeDef::Reference(_)) => {
            TypeCheckResult::Ok
        }

        // ── Type mismatch ───────────────────────────────────────
        _ => TypeCheckResult::Error(format!(
            "type mismatch: {} value is not compatible with {}",
            expr_kind_name(expr),
            type_kind_name(type_def)
        )),
    }
}

/// Human-readable name for a property expression kind.
fn expr_kind_name(expr: &PropertyExpr) -> &'static str {
    match expr {
        PropertyExpr::Integer(_, _) => "integer",
        PropertyExpr::Real(_, _) => "real",
        PropertyExpr::StringLit(_) => "string",
        PropertyExpr::Boolean(_) => "boolean",
        PropertyExpr::Enum(_) => "enumeration",
        PropertyExpr::List(_) => "list",
        PropertyExpr::Record(_) => "record",
        PropertyExpr::Range { .. } => "range",
        PropertyExpr::ClassifierValue(_) => "classifier",
        PropertyExpr::ReferenceValue(_) => "reference",
        PropertyExpr::ComputedValue(_) => "compute",
        PropertyExpr::UnitValue(_, _) => "unit value",
        PropertyExpr::Opaque(_) => "opaque",
    }
}

/// Human-readable name for a property type definition kind.
fn type_kind_name(type_def: &PropertyTypeDef) -> &'static str {
    match type_def {
        PropertyTypeDef::AadlInteger { .. } => "aadlinteger",
        PropertyTypeDef::AadlReal { .. } => "aadlreal",
        PropertyTypeDef::AadlString => "aadlstring",
        PropertyTypeDef::AadlBoolean => "aadlboolean",
        PropertyTypeDef::Enumeration(_) => "enumeration",
        PropertyTypeDef::Range(_) => "range",
        PropertyTypeDef::Classifier(_) => "classifier",
        PropertyTypeDef::Reference(_) => "reference",
        PropertyTypeDef::RecordType(_) => "record",
        PropertyTypeDef::TypeRef(_) => "type reference",
        PropertyTypeDef::ListOf(_) => "list",
        PropertyTypeDef::UnitsType(_) => "units",
    }
}

// ── Unit conversion ────────────────────────────────────────────────

/// Standard AADL time units and their conversion factors to picoseconds.
const TIME_UNITS: &[(&str, u64)] = &[
    ("ps", 1),
    ("ns", 1_000),
    ("us", 1_000_000),
    ("ms", 1_000_000_000),
    ("sec", 1_000_000_000_000),
    ("min", 60_000_000_000_000),
    ("hr", 3_600_000_000_000_000),
];

/// Standard AADL size units and their conversion factors to bits.
const SIZE_UNITS: &[(&str, u64)] = &[
    ("bits", 1),
    ("Bytes", 8),
    ("KByte", 8 * 1024),
    ("MByte", 8 * 1024 * 1024),
    ("GByte", 8 * 1024 * 1024 * 1024),
    ("TByte", 8 * 1024 * 1024 * 1024 * 1024),
];

/// Convert a value from one unit to another within the same unit family.
///
/// Returns `None` if either unit is unknown or they belong to different families.
pub fn convert_units(value: f64, from_unit: &str, to_unit: &str) -> Option<f64> {
    // Try time units
    if let (Some(from_factor), Some(to_factor)) = (
        find_unit_factor(TIME_UNITS, from_unit),
        find_unit_factor(TIME_UNITS, to_unit),
    ) {
        return Some(value * (from_factor as f64) / (to_factor as f64));
    }

    // Try size units
    if let (Some(from_factor), Some(to_factor)) = (
        find_unit_factor(SIZE_UNITS, from_unit),
        find_unit_factor(SIZE_UNITS, to_unit),
    ) {
        return Some(value * (from_factor as f64) / (to_factor as f64));
    }

    None
}

fn find_unit_factor(table: &[(&str, u64)], unit: &str) -> Option<u64> {
    table
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, factor)| *factor)
}

/// Parse a time value string like "10 ms" into picoseconds.
///
/// Returns `None` if the string can't be parsed.
pub fn parse_time_value(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit_name, factor) in TIME_UNITS {
        if let Some(num_str) = s.strip_suffix(unit_name).map(|s| s.trim()) {
            if let Ok(val) = num_str.parse::<u64>() {
                return Some(val * factor);
            }
            // Try float
            if let Ok(val) = num_str.parse::<f64>() {
                return Some((val * factor as f64) as u64);
            }
        }
    }
    None
}

/// Parse a size value string like "256 KByte" into bits.
///
/// Returns `None` if the string can't be parsed.
pub fn parse_size_value(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit_name, factor) in SIZE_UNITS {
        if let Some(num_str) = s.strip_suffix(unit_name).map(|s| s.trim()) {
            if let Ok(val) = num_str.parse::<u64>() {
                return Some(val * factor);
            }
        }
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::PropertyExpr;
    use crate::name::Name;

    // ── Type checking ──────────────────────────────────────────

    #[test]
    fn integer_in_range() {
        let expr = PropertyExpr::Integer(5, None);
        let ty = PropertyTypeDef::AadlInteger {
            range: Some((0, 10)),
            units: None,
        };
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn integer_out_of_range() {
        let expr = PropertyExpr::Integer(15, None);
        let ty = PropertyTypeDef::AadlInteger {
            range: Some((0, 10)),
            units: None,
        };
        assert!(matches!(check_type(&expr, &ty), TypeCheckResult::Error(_)));
    }

    #[test]
    fn integer_no_range_constraint() {
        let expr = PropertyExpr::Integer(999999, None);
        let ty = PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        };
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn string_matches_aadlstring() {
        let expr = PropertyExpr::StringLit("hello".to_string());
        let ty = PropertyTypeDef::AadlString;
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn boolean_matches_aadlboolean() {
        let expr = PropertyExpr::Boolean(true);
        let ty = PropertyTypeDef::AadlBoolean;
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn enum_valid_variant() {
        let expr = PropertyExpr::Enum(Name::new("Periodic"));
        let ty = PropertyTypeDef::Enumeration(vec![
            Name::new("Periodic"),
            Name::new("Sporadic"),
            Name::new("Aperiodic"),
        ]);
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn enum_invalid_variant() {
        let expr = PropertyExpr::Enum(Name::new("Unknown"));
        let ty = PropertyTypeDef::Enumeration(vec![
            Name::new("Periodic"),
            Name::new("Sporadic"),
        ]);
        assert!(matches!(check_type(&expr, &ty), TypeCheckResult::Error(_)));
    }

    #[test]
    fn enum_case_insensitive() {
        let expr = PropertyExpr::Enum(Name::new("periodic"));
        let ty = PropertyTypeDef::Enumeration(vec![Name::new("Periodic")]);
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn list_of_integers() {
        let expr = PropertyExpr::List(vec![
            PropertyExpr::Integer(1, None),
            PropertyExpr::Integer(2, None),
        ]);
        let ty = PropertyTypeDef::ListOf(Box::new(PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        }));
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn list_type_mismatch() {
        let expr = PropertyExpr::List(vec![
            PropertyExpr::Integer(1, None),
            PropertyExpr::StringLit("bad".to_string()),
        ]);
        let ty = PropertyTypeDef::ListOf(Box::new(PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        }));
        assert!(matches!(check_type(&expr, &ty), TypeCheckResult::Error(_)));
    }

    #[test]
    fn record_valid_fields() {
        let expr = PropertyExpr::Record(vec![
            (Name::new("x"), PropertyExpr::Integer(10, None)),
            (Name::new("y"), PropertyExpr::StringLit("hello".to_string())),
        ]);
        let ty = PropertyTypeDef::RecordType(vec![
            (Name::new("x"), PropertyTypeDef::AadlInteger { range: None, units: None }),
            (Name::new("y"), PropertyTypeDef::AadlString),
        ]);
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn record_unknown_field() {
        let expr = PropertyExpr::Record(vec![
            (Name::new("z"), PropertyExpr::Integer(10, None)),
        ]);
        let ty = PropertyTypeDef::RecordType(vec![
            (Name::new("x"), PropertyTypeDef::AadlInteger { range: None, units: None }),
        ]);
        assert!(matches!(check_type(&expr, &ty), TypeCheckResult::Error(_)));
    }

    #[test]
    fn range_valid() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(0, None)),
            max: Box::new(PropertyExpr::Integer(100, None)),
            delta: None,
        };
        let ty = PropertyTypeDef::Range(Box::new(PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        }));
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn type_mismatch_string_for_integer() {
        let expr = PropertyExpr::StringLit("hello".to_string());
        let ty = PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        };
        assert!(matches!(check_type(&expr, &ty), TypeCheckResult::Error(_)));
    }

    #[test]
    fn opaque_skips_check() {
        let expr = PropertyExpr::Opaque("anything".to_string());
        let ty = PropertyTypeDef::AadlInteger {
            range: None,
            units: None,
        };
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Skipped);
    }

    #[test]
    fn integer_coerced_to_real() {
        let expr = PropertyExpr::Integer(42, None);
        let ty = PropertyTypeDef::AadlReal {
            range: None,
            units: None,
        };
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn unit_value_unwraps_correctly() {
        let expr = PropertyExpr::UnitValue(
            Box::new(PropertyExpr::Integer(10, None)),
            Name::new("ms"),
        );
        let ty = PropertyTypeDef::AadlInteger {
            range: Some((0, 100)),
            units: Some(Name::new("Time")),
        };
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn classifier_value_accepted() {
        let expr = PropertyExpr::ClassifierValue(crate::name::ClassifierRef::type_only(
            Name::new("MyType"),
        ));
        let ty = PropertyTypeDef::Classifier(None);
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    #[test]
    fn reference_value_accepted() {
        let expr = PropertyExpr::ReferenceValue("cpu1".to_string());
        let ty = PropertyTypeDef::Reference(None);
        assert_eq!(check_type(&expr, &ty), TypeCheckResult::Ok);
    }

    // ── Unit conversion ────────────────────────────────────────

    #[test]
    fn convert_ms_to_us() {
        let result = convert_units(10.0, "ms", "us");
        assert_eq!(result, Some(10_000.0));
    }

    #[test]
    fn convert_sec_to_ms() {
        let result = convert_units(1.0, "sec", "ms");
        assert_eq!(result, Some(1_000.0));
    }

    #[test]
    fn convert_kbyte_to_bytes() {
        let result = convert_units(1.0, "KByte", "Bytes");
        assert_eq!(result, Some(1024.0));
    }

    #[test]
    fn convert_mbyte_to_kbyte() {
        let result = convert_units(1.0, "MByte", "KByte");
        assert_eq!(result, Some(1024.0));
    }

    #[test]
    fn convert_cross_family_fails() {
        let result = convert_units(10.0, "ms", "KByte");
        assert_eq!(result, None);
    }

    #[test]
    fn convert_unknown_unit() {
        let result = convert_units(10.0, "furlongs", "ms");
        assert_eq!(result, None);
    }

    // ── Time/size parsing ──────────────────────────────────────

    #[test]
    fn parse_time_ms() {
        assert_eq!(parse_time_value("10 ms"), Some(10_000_000_000));
    }

    #[test]
    fn parse_time_sec() {
        assert_eq!(parse_time_value("1 sec"), Some(1_000_000_000_000));
    }

    #[test]
    fn parse_time_us() {
        assert_eq!(parse_time_value("500 us"), Some(500_000_000));
    }

    #[test]
    fn parse_size_kbyte() {
        assert_eq!(parse_size_value("256 KByte"), Some(256 * 8 * 1024));
    }

    #[test]
    fn parse_size_bytes() {
        assert_eq!(parse_size_value("64 Bytes"), Some(64 * 8));
    }

    #[test]
    fn parse_invalid_time() {
        assert_eq!(parse_time_value("not a time"), None);
    }
}
