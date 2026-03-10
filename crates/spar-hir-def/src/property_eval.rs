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

use crate::item_tree::{PropertyAssociationItem, PropertyDefItem, PropertyExpr};
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
        PropertyExpr::ComputedValue(ref_name) => {
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
/// Handles `Integer`, `Real`, and `UnitValue` (which wraps a numeric
/// inner expression). Returns `None` for non-numeric expressions.
pub fn eval_numeric(expr: &PropertyExpr) -> Option<f64> {
    match expr {
        PropertyExpr::Integer(v, _unit) => Some(*v as f64),
        PropertyExpr::Real(v, _unit) => v.parse::<f64>().ok(),
        PropertyExpr::UnitValue(inner, _unit) => eval_numeric(inner),
        _ => None,
    }
}

/// Evaluate a range property expression to `(min, max)` as `f64` values.
///
/// Returns `None` if the expression is not a `Range` or if either
/// bound cannot be evaluated to a number.
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
}
