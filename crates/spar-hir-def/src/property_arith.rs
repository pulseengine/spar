//! Arithmetic in property values is REJECTED (REQ-AADL-PROPERTY-ARITHMETIC-001, #546).
//!
//! # Standing in the standard
//!
//! AS5506 has **no arithmetic in property values**. Its numeric property
//! term is a signed literal with an optional unit identifier, or a signed
//! property constant; a range is `term .. term [delta term]`. OSATE's grammar
//! (`org.osate.xtext.aadl2.properties/Properties.xtext`, shipped in OSATE 2:
//! `IntegerTerm`, `RealTerm`, `SignedConstant`, `NumericRangeTerm` over
//! `NumAlt`) has no operator, so OSATE rejects `5 * 8 ms` as a syntax error.
//! The only `*` the standard defines next to a number is in a units type
//! declaration (`ns => ps * 1000`).
//!
//! spar's parser has accepted `+ - * /` chains since #127, but lowering kept
//! only the first operand, so `Period => 5 * 8 ms` became a bare `5` that the
//! timing accessors read as 5 ps (#546). The maintainer's decision: such a
//! value is not AADL, so it is reported and IGNORED — never evaluated, and
//! never cut down to an operand.
//!
//! Lowering keeps the whole expression as a [`PropertyExpr::BinaryOp`] tree.
//! [`crate::properties::PropertyMap`]'s lookups (`get`, `get_typed`,
//! `get_all`) treat a value that [`is_arithmetic`] as absent, so no analysis
//! — typed or text-parsing — sees a number for it. `property_rules` reports
//! it with [`NOT_AADL`].
//!
//! A sign alone is NOT arithmetic: `-5 ms`, `-180 .. 180` and the signed
//! constant `-Some_Constant` (OSATE's `SignedConstant`) are legal AADL and
//! lower as before. A sign over a parenthesised group, `-(40 ms)`, is not
//! AADL and is rejected like any other arithmetic.

use crate::item_tree::PropertyExpr;

/// The reason given for every rejected arithmetic value.
pub const NOT_AADL: &str = "arithmetic in a property value is not AADL \
     (AS5506 numeric terms have no operators; OSATE rejects it); value ignored";

/// Does `expr` contain arithmetic anywhere — at the top, in a range bound or
/// delta, a list element, a record field, or under a unit?
pub fn is_arithmetic(expr: &PropertyExpr) -> bool {
    match expr {
        PropertyExpr::BinaryOp { .. } => true,
        PropertyExpr::List(items) => items.iter().any(is_arithmetic),
        PropertyExpr::Record(fields) => fields.iter().any(|(_, v)| is_arithmetic(v)),
        PropertyExpr::Range { min, max, delta } => {
            is_arithmetic(min) || is_arithmetic(max) || delta.as_deref().is_some_and(is_arithmetic)
        }
        PropertyExpr::UnitValue(inner, _) => is_arithmetic(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::BinaryOpKind;
    use crate::name::Name;

    fn int(v: i64, unit: Option<&str>) -> PropertyExpr {
        PropertyExpr::Integer(v, unit.map(Name::new))
    }

    fn bin() -> PropertyExpr {
        PropertyExpr::BinaryOp {
            op: BinaryOpKind::Mul,
            lhs: Box::new(int(5, None)),
            rhs: Box::new(int(8, Some("ms"))),
        }
    }

    fn range(min: PropertyExpr, max: PropertyExpr, delta: Option<PropertyExpr>) -> PropertyExpr {
        PropertyExpr::Range {
            min: Box::new(min),
            max: Box::new(max),
            delta: delta.map(Box::new),
        }
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn arithmetic_is_found_wherever_it_sits() {
        assert!(is_arithmetic(&bin()));
        assert!(is_arithmetic(&range(bin(), int(1, None), None)));
        assert!(is_arithmetic(&range(int(1, None), bin(), None)));
        assert!(is_arithmetic(&range(
            int(1, None),
            int(2, None),
            Some(bin())
        )));
        assert!(is_arithmetic(&PropertyExpr::List(vec![
            int(1, None),
            bin()
        ])));
        assert!(is_arithmetic(&PropertyExpr::Record(vec![(
            Name::new("a"),
            bin()
        )])));
        assert!(is_arithmetic(&PropertyExpr::UnitValue(
            Box::new(bin()),
            Name::new("ms")
        )));
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn literals_signed_values_and_constants_are_not_arithmetic() {
        assert!(!is_arithmetic(&int(40, Some("ms"))));
        assert!(!is_arithmetic(&int(-180, None)));
        assert!(!is_arithmetic(&range(
            int(-180, None),
            int(180, None),
            Some(int(1, None))
        )));
        assert!(!is_arithmetic(&PropertyExpr::Enum(Name::new(
            "My_Set::Base"
        ))));
        assert!(!is_arithmetic(&PropertyExpr::List(vec![int(1, None)])));
        assert!(!is_arithmetic(&PropertyExpr::Record(vec![(
            Name::new("a"),
            int(1, None)
        )])));
        assert!(!is_arithmetic(&PropertyExpr::UnitValue(
            Box::new(int(8, None)),
            Name::new("ms")
        )));
    }

    #[test]
    fn the_reason_says_it_is_not_aadl_and_ignored() {
        assert!(NOT_AADL.contains("not AADL"));
        assert!(NOT_AADL.contains("OSATE rejects it"));
        assert!(NOT_AADL.contains("value ignored"));
    }
}
