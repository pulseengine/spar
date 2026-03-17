//! Property expression evaluation and `value()` resolution.
//!
//! This module provides functions to resolve AADL `value()` references
//! (represented as [`PropertyExpr::ComputedValue`]) by looking up the
//! referenced property in a component's associations or falling back to
//! the property definition's default value. It also provides simple
//! numeric evaluation helpers.
//!
//! # AADL semantics
//!
//! In AADL, `value(PropertyName)` within a property expression refers
//! to the value of another property. Resolution checks explicit
//! associations first, then falls back to the property definition's
//! default value. Resolution is recursive: a `value()` reference may
//! itself reference another `value()`.

use crate::item_tree::{BinaryOpKind, PropertyAssociationItem, PropertyDefItem, PropertyExpr};
use crate::name::Name;

/// Resolve a [`PropertyExpr`], substituting `ComputedValue` references
/// with the actual values from the component's property associations
/// or from property definition defaults.
///
/// Non-`ComputedValue` variants are traversed recursively so that
/// nested `value()` references inside lists, records, ranges, etc.
/// are also resolved.
pub fn resolve_property_expr(
    expr: &PropertyExpr,
    associations: &[PropertyAssociationItem],
    definitions: &[PropertyDefItem],
) -> PropertyExpr {
    match expr {
        PropertyExpr::ComputedValue(ref_name) | PropertyExpr::ValueRef(ref_name) => {
            // Look up the referenced property in associations
            if let Some(assoc) = associations
                .iter()
                .find(|a| a.name.property_name.eq_ci(ref_name))
                && let Some(ref typed) = assoc.typed_value
            {
                return resolve_property_expr(typed, associations, definitions);
            }
            // Fall back to default from definition
            if let Some(def) = definitions.iter().find(|d| d.name.eq_ci(ref_name))
                && let Some(ref default) = def.default_value
            {
                return resolve_property_expr(default, associations, definitions);
            }
            expr.clone()
        }
        PropertyExpr::List(items) => PropertyExpr::List(
            items
                .iter()
                .map(|i| resolve_property_expr(i, associations, definitions))
                .collect(),
        ),
        PropertyExpr::Record(fields) => PropertyExpr::Record(
            fields
                .iter()
                .map(|(n, v)| {
                    (
                        n.clone(),
                        resolve_property_expr(v, associations, definitions),
                    )
                })
                .collect(),
        ),
        PropertyExpr::Range { min, max, delta } => PropertyExpr::Range {
            min: Box::new(resolve_property_expr(min, associations, definitions)),
            max: Box::new(resolve_property_expr(max, associations, definitions)),
            delta: delta
                .as_ref()
                .map(|d| Box::new(resolve_property_expr(d, associations, definitions))),
        },
        PropertyExpr::UnitValue(inner, unit) => PropertyExpr::UnitValue(
            Box::new(resolve_property_expr(inner, associations, definitions)),
            unit.clone(),
        ),
        PropertyExpr::BinaryOp { op, lhs, rhs } => PropertyExpr::BinaryOp {
            op: *op,
            lhs: Box::new(resolve_property_expr(lhs, associations, definitions)),
            rhs: Box::new(resolve_property_expr(rhs, associations, definitions)),
        },
        _ => expr.clone(),
    }
}

/// Look up a property value for a component, checking explicit
/// associations first, then falling back to the property definition's
/// default value.
///
/// Returns `None` if neither an explicit association nor a default
/// value exists for the given property name.
pub fn lookup_property(
    prop_name: &Name,
    associations: &[PropertyAssociationItem],
    definitions: &[PropertyDefItem],
) -> Option<PropertyExpr> {
    // Check explicit associations
    if let Some(assoc) = associations
        .iter()
        .find(|a| a.name.property_name.eq_ci(prop_name))
        && let Some(ref typed) = assoc.typed_value
    {
        return Some(resolve_property_expr(typed, associations, definitions));
    }
    // Fall back to default
    if let Some(def) = definitions.iter().find(|d| d.name.eq_ci(prop_name))
        && let Some(ref default) = def.default_value
    {
        return Some(resolve_property_expr(default, associations, definitions));
    }
    None
}

/// Evaluate a numeric property expression to an `f64` value.
///
/// Handles `Integer`, `Real`, `UnitValue` (which wraps a numeric inner
/// expression), and `BinaryOp` (arithmetic on sub-expressions).
/// Returns `None` for non-numeric expressions.
pub fn eval_numeric(expr: &PropertyExpr) -> Option<f64> {
    match expr {
        PropertyExpr::Integer(v, _unit) => Some(*v as f64),
        PropertyExpr::Real(v, _unit) => v.parse::<f64>().ok(),
        PropertyExpr::UnitValue(inner, _unit) => eval_numeric(inner),
        PropertyExpr::BinaryOp { op, lhs, rhs } => {
            let l = eval_numeric(lhs)?;
            let r = eval_numeric(rhs)?;
            match op {
                BinaryOpKind::Add => Some(l + r),
                BinaryOpKind::Sub => Some(l - r),
                BinaryOpKind::Mul => Some(l * r),
                BinaryOpKind::Div => {
                    if r == 0.0 {
                        None
                    } else {
                        Some(l / r)
                    }
                }
            }
        }
        _ => None,
    }
}

/// Compute a numeric property expression, returning both the value
/// and its unit (if any).
///
/// For `Integer(v, Some(u))` returns `Some((v, Some(u)))`.
/// For `UnitValue(inner, u)` returns `Some((numeric(inner), Some(u)))`.
/// For bare `Integer(v, None)` returns `Some((v, None))`.
pub fn numeric_with_unit(expr: &PropertyExpr) -> Option<(f64, Option<&Name>)> {
    match expr {
        PropertyExpr::Integer(v, unit) => Some((*v as f64, unit.as_ref())),
        PropertyExpr::Real(v, unit) => {
            let val = v.parse::<f64>().ok()?;
            Some((val, unit.as_ref()))
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let val = eval_numeric(inner)?;
            Some((val, Some(unit)))
        }
        PropertyExpr::BinaryOp { .. } => {
            let val = eval_numeric(expr)?;
            Some((val, None))
        }
        _ => None,
    }
}

/// Compute a range property expression to `(min, max)` as `f64` values.
///
/// Returns `None` if the expression is not a `Range` or if either
/// bound cannot be computed to a number.
pub fn eval_range(expr: &PropertyExpr) -> Option<(f64, f64)> {
    match expr {
        PropertyExpr::Range { min, max, .. } => {
            let min_val = eval_numeric(min)?;
            let max_val = eval_numeric(max)?;
            Some((min_val, max_val))
        }
        _ => None,
    }
}

/// Extract a field value from a record property expression.
///
/// Performs case-insensitive lookup of the field name.
/// Returns `None` if `expr` is not a record or the field is not found.
pub fn get_record_field<'a>(expr: &'a PropertyExpr, field_name: &str) -> Option<&'a PropertyExpr> {
    match expr {
        PropertyExpr::Record(fields) => fields
            .iter()
            .find(|(name, _)| name.as_str().eq_ignore_ascii_case(field_name))
            .map(|(_, val)| val),
        _ => None,
    }
}

/// Extract a numeric field value from a record, returning `None` if the
/// record doesn't have the field or the field is not numeric.
pub fn get_record_field_numeric(expr: &PropertyExpr, field_name: &str) -> Option<f64> {
    let field = get_record_field(expr, field_name)?;
    eval_numeric(field)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::PropertyAssociationItem;
    use crate::item_tree::PropertyDefItem;
    use crate::item_tree::PropertyExpr;
    use crate::name::{Name, PropertyRef};

    /// Helper: create a property association with a typed value.
    fn make_assoc(name: &str, value: PropertyExpr) -> PropertyAssociationItem {
        PropertyAssociationItem {
            name: PropertyRef {
                property_set: None,
                property_name: Name::new(name),
            },
            value: String::new(),
            typed_value: Some(value),
            is_append: false,
            applies_to: None,
            in_modes: vec![],
        }
    }

    /// Helper: create a property definition with a default value.
    fn make_def(name: &str, default: Option<PropertyExpr>) -> PropertyDefItem {
        PropertyDefItem {
            name: Name::new(name),
            type_def: None,
            default_value: default,
            applies_to: vec![],
        }
    }

    // ── resolve_property_expr ──────────────────────────────────

    #[test]
    fn resolve_computed_value_from_association() {
        let associations = vec![make_assoc("Period", PropertyExpr::Integer(100, None))];
        let definitions = vec![];

        let expr = PropertyExpr::ComputedValue(Name::new("Period"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::Integer(100, None));
    }

    #[test]
    fn resolve_computed_value_falls_back_to_default() {
        let associations = vec![];
        let definitions = vec![make_def("Period", Some(PropertyExpr::Integer(50, None)))];

        let expr = PropertyExpr::ComputedValue(Name::new("Period"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::Integer(50, None));
    }

    #[test]
    fn resolve_chained_computed_values() {
        // A references B, B has a value of 42
        let associations = vec![
            make_assoc("A", PropertyExpr::ComputedValue(Name::new("B"))),
            make_assoc("B", PropertyExpr::Integer(42, None)),
        ];
        let definitions = vec![];

        let expr = PropertyExpr::ComputedValue(Name::new("A"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::Integer(42, None));
    }

    #[test]
    fn resolve_unresolvable_computed_value_returned_as_is() {
        let associations = vec![];
        let definitions = vec![];

        let expr = PropertyExpr::ComputedValue(Name::new("Unknown"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::ComputedValue(Name::new("Unknown")));
    }

    #[test]
    fn resolve_nested_in_list() {
        let associations = vec![make_assoc("X", PropertyExpr::Integer(7, None))];
        let definitions = vec![];

        let expr = PropertyExpr::List(vec![
            PropertyExpr::Integer(1, None),
            PropertyExpr::ComputedValue(Name::new("X")),
        ]);
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(
            result,
            PropertyExpr::List(vec![
                PropertyExpr::Integer(1, None),
                PropertyExpr::Integer(7, None),
            ])
        );
    }

    #[test]
    fn resolve_nested_in_range() {
        let associations = vec![make_assoc("Lo", PropertyExpr::Integer(0, None))];
        let definitions = vec![make_def("Hi", Some(PropertyExpr::Integer(100, None)))];

        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::ComputedValue(Name::new("Lo"))),
            max: Box::new(PropertyExpr::ComputedValue(Name::new("Hi"))),
            delta: None,
        };
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(
            result,
            PropertyExpr::Range {
                min: Box::new(PropertyExpr::Integer(0, None)),
                max: Box::new(PropertyExpr::Integer(100, None)),
                delta: None,
            }
        );
    }

    // ── lookup_property ────────────────────────────────────────

    #[test]
    fn lookup_explicit_association() {
        let associations = vec![make_assoc("Period", PropertyExpr::Integer(20, None))];
        let definitions = vec![];

        let result = lookup_property(&Name::new("Period"), &associations, &definitions);
        assert_eq!(result, Some(PropertyExpr::Integer(20, None)));
    }

    #[test]
    fn lookup_falls_back_to_default() {
        let associations = vec![];
        let definitions = vec![make_def("Period", Some(PropertyExpr::Integer(10, None)))];

        let result = lookup_property(&Name::new("Period"), &associations, &definitions);
        assert_eq!(result, Some(PropertyExpr::Integer(10, None)));
    }

    #[test]
    fn lookup_no_match_returns_none() {
        let associations = vec![];
        let definitions = vec![];

        let result = lookup_property(&Name::new("Missing"), &associations, &definitions);
        assert_eq!(result, None);
    }

    #[test]
    fn lookup_association_takes_precedence_over_default() {
        let associations = vec![make_assoc("Period", PropertyExpr::Integer(99, None))];
        let definitions = vec![make_def("Period", Some(PropertyExpr::Integer(1, None)))];

        let result = lookup_property(&Name::new("Period"), &associations, &definitions);
        assert_eq!(result, Some(PropertyExpr::Integer(99, None)));
    }

    // ── eval_numeric ───────────────────────────────────────────

    #[test]
    fn eval_numeric_integer() {
        let expr = PropertyExpr::Integer(42, None);
        assert_eq!(eval_numeric(&expr), Some(42.0));
    }

    #[test]
    fn eval_numeric_real() {
        let expr = PropertyExpr::Real("3.14".to_string(), None);
        #[allow(clippy::approx_constant)]
        let expected = 3.14;
        assert_eq!(eval_numeric(&expr), Some(expected));
    }

    #[test]
    fn eval_numeric_unit_value() {
        let expr =
            PropertyExpr::UnitValue(Box::new(PropertyExpr::Integer(10, None)), Name::new("ms"));
        assert_eq!(eval_numeric(&expr), Some(10.0));
    }

    #[test]
    fn eval_numeric_non_numeric_returns_none() {
        let expr = PropertyExpr::StringLit("hello".to_string());
        assert_eq!(eval_numeric(&expr), None);
    }

    #[test]
    fn eval_numeric_integer_with_unit() {
        let expr = PropertyExpr::Integer(500, Some(Name::new("ms")));
        assert_eq!(eval_numeric(&expr), Some(500.0));
    }

    // ── eval_range ─────────────────────────────────────────────

    #[test]
    fn eval_range_valid() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(0, None)),
            max: Box::new(PropertyExpr::Integer(100, None)),
            delta: None,
        };
        assert_eq!(eval_range(&expr), Some((0.0, 100.0)));
    }

    #[test]
    fn eval_range_with_real_bounds() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Real("1.5".to_string(), None)),
            max: Box::new(PropertyExpr::Real("9.5".to_string(), None)),
            delta: None,
        };
        assert_eq!(eval_range(&expr), Some((1.5, 9.5)));
    }

    #[test]
    fn eval_range_non_range_returns_none() {
        let expr = PropertyExpr::Integer(42, None);
        assert_eq!(eval_range(&expr), None);
    }

    #[test]
    fn eval_range_with_non_numeric_bound_returns_none() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::StringLit("bad".to_string())),
            max: Box::new(PropertyExpr::Integer(100, None)),
            delta: None,
        };
        assert_eq!(eval_range(&expr), None);
    }

    // ── BinaryOp evaluation ─────────────────────────────────────

    #[test]
    fn eval_binary_add() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Add,
            lhs: Box::new(PropertyExpr::Integer(10, None)),
            rhs: Box::new(PropertyExpr::Integer(20, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(30.0));
    }

    #[test]
    fn eval_binary_sub() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Sub,
            lhs: Box::new(PropertyExpr::Integer(50, None)),
            rhs: Box::new(PropertyExpr::Integer(20, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(30.0));
    }

    #[test]
    fn eval_binary_mul() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Mul,
            lhs: Box::new(PropertyExpr::Integer(6, None)),
            rhs: Box::new(PropertyExpr::Integer(7, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(42.0));
    }

    #[test]
    fn eval_binary_div() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Div,
            lhs: Box::new(PropertyExpr::Integer(100, None)),
            rhs: Box::new(PropertyExpr::Integer(4, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(25.0));
    }

    #[test]
    fn eval_binary_div_by_zero_returns_none() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Div,
            lhs: Box::new(PropertyExpr::Integer(100, None)),
            rhs: Box::new(PropertyExpr::Integer(0, None)),
        };
        assert_eq!(eval_numeric(&expr), None);
    }

    #[test]
    fn eval_binary_nested() {
        // (10 + 20) * 3 = 90
        let add = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Add,
            lhs: Box::new(PropertyExpr::Integer(10, None)),
            rhs: Box::new(PropertyExpr::Integer(20, None)),
        };
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Mul,
            lhs: Box::new(add),
            rhs: Box::new(PropertyExpr::Integer(3, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(90.0));
    }

    #[test]
    fn eval_binary_with_real() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Mul,
            lhs: Box::new(PropertyExpr::Real("2.5".to_string(), None)),
            rhs: Box::new(PropertyExpr::Integer(4, None)),
        };
        assert_eq!(eval_numeric(&expr), Some(10.0));
    }

    #[test]
    fn eval_binary_non_numeric_operand_returns_none() {
        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Add,
            lhs: Box::new(PropertyExpr::StringLit("hello".to_string())),
            rhs: Box::new(PropertyExpr::Integer(1, None)),
        };
        assert_eq!(eval_numeric(&expr), None);
    }

    // ── Record field access ─────────────────────────────────────

    #[test]
    fn record_field_found() {
        let record = PropertyExpr::Record(vec![
            (
                Name::new("NetWeight"),
                PropertyExpr::Integer(5, Some(Name::new("kg"))),
            ),
            (
                Name::new("GrossWeight"),
                PropertyExpr::Integer(10, Some(Name::new("kg"))),
            ),
        ]);
        let field = get_record_field(&record, "NetWeight");
        assert_eq!(
            field,
            Some(&PropertyExpr::Integer(5, Some(Name::new("kg"))))
        );
    }

    #[test]
    fn record_field_case_insensitive() {
        let record = PropertyExpr::Record(vec![(
            Name::new("NetWeight"),
            PropertyExpr::Integer(5, None),
        )]);
        let field = get_record_field(&record, "netweight");
        assert_eq!(field, Some(&PropertyExpr::Integer(5, None)));
    }

    #[test]
    fn record_field_not_found() {
        let record = PropertyExpr::Record(vec![(
            Name::new("NetWeight"),
            PropertyExpr::Integer(5, None),
        )]);
        assert_eq!(get_record_field(&record, "Missing"), None);
    }

    #[test]
    fn record_field_on_non_record_returns_none() {
        let expr = PropertyExpr::Integer(42, None);
        assert_eq!(get_record_field(&expr, "anything"), None);
    }

    #[test]
    fn record_field_numeric_extraction() {
        let record = PropertyExpr::Record(vec![
            (
                Name::new("weight"),
                PropertyExpr::Integer(5, Some(Name::new("kg"))),
            ),
            (
                Name::new("label"),
                PropertyExpr::StringLit("test".to_string()),
            ),
        ]);
        assert_eq!(get_record_field_numeric(&record, "weight"), Some(5.0));
        assert_eq!(get_record_field_numeric(&record, "label"), None);
        assert_eq!(get_record_field_numeric(&record, "missing"), None);
    }

    // ── numeric_with_unit ───────────────────────────────────────

    #[test]
    fn numeric_with_unit_integer_with_unit() {
        let expr = PropertyExpr::Integer(10, Some(Name::new("ms")));
        let (val, unit) = numeric_with_unit(&expr).unwrap();
        assert_eq!(val, 10.0);
        assert_eq!(unit.unwrap().as_str(), "ms");
    }

    #[test]
    fn numeric_with_unit_integer_without_unit() {
        let expr = PropertyExpr::Integer(42, None);
        let (val, unit) = numeric_with_unit(&expr).unwrap();
        assert_eq!(val, 42.0);
        assert!(unit.is_none());
    }

    #[test]
    fn numeric_with_unit_unit_value() {
        let expr =
            PropertyExpr::UnitValue(Box::new(PropertyExpr::Integer(500, None)), Name::new("us"));
        let (val, unit) = numeric_with_unit(&expr).unwrap();
        assert_eq!(val, 500.0);
        assert_eq!(unit.unwrap().as_str(), "us");
    }

    #[test]
    fn numeric_with_unit_non_numeric_returns_none() {
        let expr = PropertyExpr::StringLit("hello".to_string());
        assert!(numeric_with_unit(&expr).is_none());
    }

    // ── ValueRef resolution ─────────────────────────────────────

    #[test]
    fn resolve_value_ref_from_association() {
        let associations = vec![make_assoc("Period", PropertyExpr::Integer(100, None))];
        let definitions = vec![];

        let expr = PropertyExpr::ValueRef(Name::new("Period"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::Integer(100, None));
    }

    #[test]
    fn resolve_value_ref_falls_back_to_default() {
        let associations = vec![];
        let definitions = vec![make_def("Period", Some(PropertyExpr::Integer(50, None)))];

        let expr = PropertyExpr::ValueRef(Name::new("Period"));
        let result = resolve_property_expr(&expr, &associations, &definitions);

        assert_eq!(result, PropertyExpr::Integer(50, None));
    }

    // ── BinaryOp resolution ─────────────────────────────────────

    #[test]
    fn resolve_binary_op_with_value_refs() {
        let associations = vec![
            make_assoc("X", PropertyExpr::Integer(10, None)),
            make_assoc("Y", PropertyExpr::Integer(20, None)),
        ];
        let definitions = vec![];

        let expr = PropertyExpr::BinaryOp {
            op: BinaryOpKind::Add,
            lhs: Box::new(PropertyExpr::ComputedValue(Name::new("X"))),
            rhs: Box::new(PropertyExpr::ComputedValue(Name::new("Y"))),
        };
        let resolved = resolve_property_expr(&expr, &associations, &definitions);
        assert_eq!(eval_numeric(&resolved), Some(30.0));
    }
}
