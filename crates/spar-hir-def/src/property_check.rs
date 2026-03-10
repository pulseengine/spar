//! Property expression type validation (STPA-REQ-006).
//!
//! Validates that a property association's expression is compatible with
//! the declared property type.  Type mismatches (e.g. a string value
//! assigned to an integer property, or an invalid enumeration literal)
//! produce error diagnostics.

use crate::item_tree::{PropertyExpr, PropertyTypeDef};

/// A diagnostic produced when a property expression does not match the
/// declared property type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyTypeDiagnostic {
    pub message: String,
    pub property_name: String,
}

/// Validate that `expr` is compatible with `type_def` for the property
/// named `property_name`.  Returns a (possibly empty) list of
/// diagnostics for every type mismatch found.
pub fn validate_property_type(
    property_name: &str,
    expr: &PropertyExpr,
    type_def: &PropertyTypeDef,
) -> Vec<PropertyTypeDiagnostic> {
    let mut diagnostics = Vec::new();
    check_expr(property_name, expr, type_def, &mut diagnostics);
    diagnostics
}

fn check_expr(
    property_name: &str,
    expr: &PropertyExpr,
    type_def: &PropertyTypeDef,
    diags: &mut Vec<PropertyTypeDiagnostic>,
) {
    match (expr, type_def) {
        // Skip validation for opaque and computed values — we cannot
        // type-check them statically.
        (PropertyExpr::Opaque(_), _) | (PropertyExpr::ComputedValue(_), _) => {}

        // UnitValue: check inner expression against the type.
        (PropertyExpr::UnitValue(inner, _unit), _) => {
            check_expr(property_name, inner, type_def, diags);
        }

        // Integer ↔ AadlInteger
        (PropertyExpr::Integer(..), PropertyTypeDef::AadlInteger { .. }) => {}

        // Real ↔ AadlReal
        (PropertyExpr::Real(..), PropertyTypeDef::AadlReal { .. }) => {}

        // StringLit ↔ AadlString
        (PropertyExpr::StringLit(_), PropertyTypeDef::AadlString) => {}

        // Boolean ↔ AadlBoolean
        (PropertyExpr::Boolean(_), PropertyTypeDef::AadlBoolean) => {}

        // Enum(val) ↔ Enumeration(variants)
        (PropertyExpr::Enum(val), PropertyTypeDef::Enumeration(variants)) => {
            let found = variants.iter().any(|v| v.eq_ci(val));
            if !found {
                let variant_list: Vec<&str> = variants.iter().map(|v| v.as_str()).collect();
                diags.push(PropertyTypeDiagnostic {
                    message: format!(
                        "invalid enumeration literal '{}' for property '{}'; expected one of: {}",
                        val.as_str(),
                        property_name,
                        variant_list.join(", "),
                    ),
                    property_name: property_name.to_string(),
                });
            }
        }

        // List(items) ↔ ListOf(element_type)
        (PropertyExpr::List(items), PropertyTypeDef::ListOf(element_type)) => {
            for item in items {
                check_expr(property_name, item, element_type, diags);
            }
        }

        // ClassifierValue ↔ Classifier
        (PropertyExpr::ClassifierValue(_), PropertyTypeDef::Classifier(_)) => {}

        // ReferenceValue ↔ Reference
        (PropertyExpr::ReferenceValue(_), PropertyTypeDef::Reference(_)) => {}

        // Range{min,max} ↔ Range(inner)
        (
            PropertyExpr::Range {
                min, max, delta, ..
            },
            PropertyTypeDef::Range(inner),
        ) => {
            check_expr(property_name, min, inner, diags);
            check_expr(property_name, max, inner, diags);
            if let Some(d) = delta {
                check_expr(property_name, d, inner, diags);
            }
        }

        // Record(fields) ↔ RecordType(type_fields)
        (PropertyExpr::Record(fields), PropertyTypeDef::RecordType(type_fields)) => {
            for (field_name, field_expr) in fields {
                if let Some((_tf_name, tf_type)) =
                    type_fields.iter().find(|(n, _)| n.eq_ci(field_name))
                {
                    check_expr(property_name, field_expr, tf_type, diags);
                }
                // Unknown fields are not flagged here — that is a
                // separate validation concern.
            }
        }

        // TypeRef: we cannot resolve the referenced type here, so skip.
        (_, PropertyTypeDef::TypeRef(_)) => {}

        // UnitsType: integers and reals are valid for unit types
        (PropertyExpr::Integer(..), PropertyTypeDef::UnitsType(_))
        | (PropertyExpr::Real(..), PropertyTypeDef::UnitsType(_)) => {}

        // Everything else is a type mismatch.
        _ => {
            diags.push(PropertyTypeDiagnostic {
                message: format!(
                    "type mismatch for property '{}': expression {} is not compatible with type {}",
                    property_name,
                    expr_kind_name(expr),
                    type_kind_name(type_def),
                ),
                property_name: property_name.to_string(),
            });
        }
    }
}

/// Human-readable name for a property expression variant.
fn expr_kind_name(expr: &PropertyExpr) -> &'static str {
    match expr {
        PropertyExpr::Integer(..) => "integer",
        PropertyExpr::Real(..) => "real",
        PropertyExpr::StringLit(_) => "string",
        PropertyExpr::Boolean(_) => "boolean",
        PropertyExpr::Enum(_) => "enumeration literal",
        PropertyExpr::List(_) => "list",
        PropertyExpr::Record(_) => "record",
        PropertyExpr::Range { .. } => "range",
        PropertyExpr::ClassifierValue(_) => "classifier value",
        PropertyExpr::ReferenceValue(_) => "reference value",
        PropertyExpr::ComputedValue(_) => "computed value",
        PropertyExpr::UnitValue(..) => "unit value",
        PropertyExpr::Opaque(_) => "opaque",
    }
}

/// Human-readable name for a property type definition variant.
fn type_kind_name(td: &PropertyTypeDef) -> &'static str {
    match td {
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
        PropertyTypeDef::ListOf(_) => "list of",
        PropertyTypeDef::UnitsType(_) => "units",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{ClassifierRef, Name};

    #[test]
    fn integer_matches_aadl_integer() {
        let diags = validate_property_type(
            "Size",
            &PropertyExpr::Integer(42, None),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            },
        );
        assert!(diags.is_empty(), "expected no diagnostics: {diags:?}");
    }

    #[test]
    fn string_mismatches_aadl_integer() {
        let diags = validate_property_type(
            "Size",
            &PropertyExpr::StringLit("hello".into()),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            },
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("type mismatch"));
        assert!(diags[0].message.contains("string"));
        assert!(diags[0].message.contains("aadlinteger"));
    }

    #[test]
    fn invalid_enum_value() {
        let variants = vec![Name::new("High"), Name::new("Medium"), Name::new("Low")];
        let diags = validate_property_type(
            "Priority",
            &PropertyExpr::Enum(Name::new("Critical")),
            &PropertyTypeDef::Enumeration(variants),
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid enumeration literal"));
        assert!(diags[0].message.contains("Critical"));
    }

    #[test]
    fn valid_enum_value_case_insensitive() {
        let variants = vec![Name::new("High"), Name::new("Medium"), Name::new("Low")];
        let diags = validate_property_type(
            "Priority",
            &PropertyExpr::Enum(Name::new("high")),
            &PropertyTypeDef::Enumeration(variants),
        );
        assert!(
            diags.is_empty(),
            "case-insensitive match should pass: {diags:?}"
        );
    }

    #[test]
    fn list_element_type_checking() {
        // List of integers — all integers → pass
        let diags = validate_property_type(
            "Sizes",
            &PropertyExpr::List(vec![
                PropertyExpr::Integer(1, None),
                PropertyExpr::Integer(2, None),
            ]),
            &PropertyTypeDef::ListOf(Box::new(PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            })),
        );
        assert!(diags.is_empty(), "all integers should pass: {diags:?}");

        // List of integers — one string → one diagnostic
        let diags = validate_property_type(
            "Sizes",
            &PropertyExpr::List(vec![
                PropertyExpr::Integer(1, None),
                PropertyExpr::StringLit("bad".into()),
            ]),
            &PropertyTypeDef::ListOf(Box::new(PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            })),
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("type mismatch"));
    }

    #[test]
    fn opaque_skips_validation() {
        let diags = validate_property_type(
            "Anything",
            &PropertyExpr::Opaque("whatever".into()),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            },
        );
        assert!(diags.is_empty(), "opaque should skip validation: {diags:?}");
    }

    #[test]
    fn boolean_matches_aadl_boolean() {
        let diags = validate_property_type(
            "Active",
            &PropertyExpr::Boolean(true),
            &PropertyTypeDef::AadlBoolean,
        );
        assert!(diags.is_empty(), "boolean should match: {diags:?}");
    }

    #[test]
    fn range_with_correct_inner_type() {
        let diags = validate_property_type(
            "Speed",
            &PropertyExpr::Range {
                min: Box::new(PropertyExpr::Integer(0, None)),
                max: Box::new(PropertyExpr::Integer(100, None)),
                delta: None,
            },
            &PropertyTypeDef::Range(Box::new(PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            })),
        );
        assert!(
            diags.is_empty(),
            "range with integer bounds should match: {diags:?}"
        );
    }

    #[test]
    fn range_with_wrong_inner_type() {
        let diags = validate_property_type(
            "Speed",
            &PropertyExpr::Range {
                min: Box::new(PropertyExpr::StringLit("low".into())),
                max: Box::new(PropertyExpr::Integer(100, None)),
                delta: None,
            },
            &PropertyTypeDef::Range(Box::new(PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            })),
        );
        assert_eq!(diags.len(), 1, "string min in integer range should fail");
    }

    #[test]
    fn real_matches_aadl_real() {
        let diags = validate_property_type(
            "Weight",
            &PropertyExpr::Real("3.14".into(), None),
            &PropertyTypeDef::AadlReal {
                range: None,
                units: None,
            },
        );
        assert!(diags.is_empty(), "real should match aadlreal: {diags:?}");
    }

    #[test]
    fn classifier_value_matches_classifier() {
        let diags = validate_property_type(
            "MyClassifier",
            &PropertyExpr::ClassifierValue(ClassifierRef::qualified(
                Name::new("Pkg"),
                Name::new("Type"),
            )),
            &PropertyTypeDef::Classifier(None),
        );
        assert!(diags.is_empty(), "classifier should match: {diags:?}");
    }

    #[test]
    fn reference_value_matches_reference() {
        let diags = validate_property_type(
            "MyRef",
            &PropertyExpr::ReferenceValue("some.path".into()),
            &PropertyTypeDef::Reference(None),
        );
        assert!(diags.is_empty(), "reference should match: {diags:?}");
    }

    #[test]
    fn computed_value_skips_validation() {
        let diags = validate_property_type(
            "Computed",
            &PropertyExpr::ComputedValue(Name::new("my_compute")),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: None,
            },
        );
        assert!(
            diags.is_empty(),
            "computed should skip validation: {diags:?}"
        );
    }

    #[test]
    fn unit_value_checks_inner() {
        // UnitValue wrapping an integer → integer type → pass
        let diags = validate_property_type(
            "Period",
            &PropertyExpr::UnitValue(Box::new(PropertyExpr::Integer(10, None)), Name::new("ms")),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: Some(Name::new("Time_Units")),
            },
        );
        assert!(
            diags.is_empty(),
            "unit value with matching inner should pass: {diags:?}"
        );

        // UnitValue wrapping a string → integer type → fail
        let diags = validate_property_type(
            "Period",
            &PropertyExpr::UnitValue(
                Box::new(PropertyExpr::StringLit("bad".into())),
                Name::new("ms"),
            ),
            &PropertyTypeDef::AadlInteger {
                range: None,
                units: Some(Name::new("Time_Units")),
            },
        );
        assert_eq!(diags.len(), 1, "unit value with wrong inner should fail");
    }

    #[test]
    fn record_field_type_checking() {
        let type_fields = vec![
            (
                Name::new("count"),
                PropertyTypeDef::AadlInteger {
                    range: None,
                    units: None,
                },
            ),
            (Name::new("label"), PropertyTypeDef::AadlString),
        ];

        // All fields match → pass
        let diags = validate_property_type(
            "Config",
            &PropertyExpr::Record(vec![
                (Name::new("count"), PropertyExpr::Integer(5, None)),
                (Name::new("label"), PropertyExpr::StringLit("hello".into())),
            ]),
            &PropertyTypeDef::RecordType(type_fields.clone()),
        );
        assert!(
            diags.is_empty(),
            "matching record fields should pass: {diags:?}"
        );

        // Wrong type for 'count' field
        let diags = validate_property_type(
            "Config",
            &PropertyExpr::Record(vec![(
                Name::new("count"),
                PropertyExpr::StringLit("not a number".into()),
            )]),
            &PropertyTypeDef::RecordType(type_fields),
        );
        assert_eq!(
            diags.len(),
            1,
            "wrong record field type should produce error"
        );
    }

    #[test]
    fn type_ref_skips_validation() {
        // TypeRef can't be resolved at this level, so any expression is OK.
        let diags = validate_property_type(
            "Prop",
            &PropertyExpr::Integer(1, None),
            &PropertyTypeDef::TypeRef(Name::new("SomeType")),
        );
        assert!(
            diags.is_empty(),
            "TypeRef should skip validation: {diags:?}"
        );
    }

    #[test]
    fn boolean_mismatches_aadl_string() {
        let diags = validate_property_type(
            "Label",
            &PropertyExpr::Boolean(false),
            &PropertyTypeDef::AadlString,
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("type mismatch"));
        assert!(diags[0].message.contains("boolean"));
        assert!(diags[0].message.contains("aadlstring"));
    }
}
