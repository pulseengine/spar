//! Evaluation of arithmetic in property values (REQ-AADL-PROPERTY-ARITHMETIC-001, #546).
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
//! declaration (`ns => ps * 1000`), which is what gives each unit its factor.
//!
//! spar has accepted `+ - *` in property values since #127 as an EXTENSION,
//! but lowered only the first operand, so `Period => 5 * 8 ms` became a bare
//! `5` that the timing accessors read as 5 ps (#546). This module gives the
//! extension a defined meaning instead:
//!
//! - `a + b`, `a - b`: both unitless, or both carrying units of the same
//!   units type. Different units are converted to the FINER of the two
//!   (`1 sec - 500 ms` is `500 ms`), so integer arithmetic stays exact.
//! - `n * x`, `x * n`, `x / n`: one side is a unitless number.
//! - unary minus, written `-x` or `-(…)`, is `-1 * x`.
//! - integer op integer is an integer; any real operand makes the result
//!   real. An integer `/` that is not exact is re-expressed in a finer unit
//!   of the same units type when that makes it exact (`1 ms / 4` is
//!   `250 us`), and is an error otherwise: it is NEVER truncated.
//!
//! Errors, each of which yields NO value: unit times or over unit
//! (`ms * ms`), a unitless number over a unit, a sum of different
//! dimensions (`ms + KByte`) or of a unit and a bare number, division by
//! zero, overflow, a non-numeric operand, and a property-constant operand —
//! spar does not resolve property constant VALUES (the resolver records
//! constant names only), so a constant inside arithmetic is reported as not
//! evaluated rather than guessed.
//!
//! Units are converted with the same Time_Units and Size_Units factor tables
//! the analyses use ([`crate::property_value`]), so an evaluated value and a
//! literal of the same quantity are read identically downstream. Two
//! different units outside those tables cannot be converted and are an
//! error; the same unit on both sides needs no table.

use std::fmt;

use crate::item_tree::{BinaryOpKind, PropertyExpr};
use crate::name::Name;
use crate::property_value::{SIZE_UNITS, TIME_UNITS};

/// Why an arithmetic property value has no value.
#[derive(Debug, Clone, PartialEq)]
pub enum ArithError {
    /// `5 ms * 8 ms`, `5 ms / 8 ms`: no AADL units type has `ms * ms`.
    UnitOnUnit {
        op: BinaryOpKind,
        lhs: String,
        rhs: String,
    },
    /// `2 / 5 ms`: an inverse unit.
    UnitlessOverUnit { rhs: String },
    /// `5 ms + 3 KByte`, `5 ms + 3`: operands of different dimensions.
    DimensionMismatch {
        op: BinaryOpKind,
        lhs: Option<String>,
        rhs: Option<String>,
    },
    /// Two different units that are not in a known units table.
    UnknownConversion { lhs: String, rhs: String },
    /// `x / 0`.
    DivisionByZero,
    /// An integer division with a remainder, in no finer unit exact.
    InexactIntegerDivision { lhs: i64, rhs: i64 },
    /// The result does not fit an `aadlinteger` (i64), or is not finite.
    Overflow,
    /// A property constant (or other name) used as an operand.
    ConstantOperand(String),
    /// An operand that is not a number at all (a string, a list, ...).
    NotNumeric(&'static str),
}

fn op_symbol(op: BinaryOpKind) -> &'static str {
    match op {
        BinaryOpKind::Add => "+",
        BinaryOpKind::Sub => "-",
        BinaryOpKind::Mul => "*",
        BinaryOpKind::Div => "/",
    }
}

impl fmt::Display for ArithError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArithError::UnitOnUnit { op, lhs, rhs } => write!(
                f,
                "`{lhs} {} {rhs}` multiplies or divides a unit by a unit, which has no AADL units type",
                op_symbol(*op)
            ),
            ArithError::UnitlessOverUnit { rhs } => write!(
                f,
                "a unitless number divided by `{rhs}` has no AADL units type"
            ),
            ArithError::DimensionMismatch { op, lhs, rhs } => write!(
                f,
                "`{} {} {}` combines operands of different dimensions",
                lhs.as_deref().unwrap_or("unitless"),
                op_symbol(*op),
                rhs.as_deref().unwrap_or("unitless")
            ),
            ArithError::UnknownConversion { lhs, rhs } => write!(
                f,
                "cannot convert between `{lhs}` and `{rhs}`: not in the Time_Units or Size_Units tables"
            ),
            ArithError::DivisionByZero => write!(f, "division by zero"),
            ArithError::InexactIntegerDivision { lhs, rhs } => write!(
                f,
                "integer division {lhs} / {rhs} is not exact in any unit (it is never truncated)"
            ),
            ArithError::Overflow => write!(f, "the result overflows"),
            ArithError::ConstantOperand(name) => write!(
                f,
                "operand `{name}` is a name; property constant values are not evaluated in arithmetic"
            ),
            ArithError::NotNumeric(kind) => write!(f, "a {kind} operand is not a number"),
        }
    }
}

/// A number: an `aadlinteger` or an `aadlreal`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    Int(i64),
    Real(f64),
}

impl Num {
    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Real(r) => r,
        }
    }

    fn is_zero(self) -> bool {
        match self {
            Num::Int(i) => i == 0,
            Num::Real(r) => r == 0.0,
        }
    }

    /// Multiply by an integer conversion factor.
    fn scale(self, factor: u64) -> Result<Num, ArithError> {
        match self {
            Num::Int(i) => {
                let f = i64::try_from(factor).map_err(|_| ArithError::Overflow)?;
                i.checked_mul(f).map(Num::Int).ok_or(ArithError::Overflow)
            }
            Num::Real(r) => real(r * factor as f64),
        }
    }
}

fn real(r: f64) -> Result<Num, ArithError> {
    if r.is_finite() {
        Ok(Num::Real(r))
    } else {
        Err(ArithError::Overflow)
    }
}

/// A number with an optional unit.
#[derive(Debug, Clone, PartialEq)]
struct Quantity {
    num: Num,
    unit: Option<Name>,
}

/// A units table: each unit's name and its factor to the base unit.
type UnitTable = &'static [(&'static str, u64)];

/// The units tables, each with a name for its dimension.
const UNIT_TABLES: &[(&str, UnitTable)] = &[("time", TIME_UNITS), ("size", SIZE_UNITS)];

/// A unit's dimension, its table, and its factor in that table.
fn unit_entry(unit: &str) -> Option<(&'static str, UnitTable, u64)> {
    UNIT_TABLES.iter().find_map(|(dim, table)| {
        table
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(unit))
            .map(|(_, f)| (*dim, *table, *f))
    })
}

fn int_op(op: BinaryOpKind, l: i64, r: i64) -> Option<i64> {
    match op {
        BinaryOpKind::Add => l.checked_add(r),
        BinaryOpKind::Sub => l.checked_sub(r),
        BinaryOpKind::Mul => l.checked_mul(r),
        BinaryOpKind::Div => l.checked_div(r),
    }
}

fn real_op(op: BinaryOpKind, l: f64, r: f64) -> f64 {
    match op {
        BinaryOpKind::Add => l + r,
        BinaryOpKind::Sub => l - r,
        BinaryOpKind::Mul => l * r,
        BinaryOpKind::Div => l / r,
    }
}

/// `l op r` on bare numbers (units already checked). Integer division here
/// must be exact; the caller handles a remainder.
fn num_op(op: BinaryOpKind, l: Num, r: Num) -> Result<Num, ArithError> {
    match (l, r) {
        (Num::Int(a), Num::Int(b)) => int_op(op, a, b).map(Num::Int).ok_or(ArithError::Overflow),
        _ => real(real_op(op, l.as_f64(), r.as_f64())),
    }
}

fn add_sub(op: BinaryOpKind, l: Quantity, r: Quantity) -> Result<Quantity, ArithError> {
    match (&l.unit, &r.unit) {
        (None, None) => Ok(Quantity {
            num: num_op(op, l.num, r.num)?,
            unit: None,
        }),
        (Some(lu), Some(ru)) if lu.eq_ci(ru) => Ok(Quantity {
            num: num_op(op, l.num, r.num)?,
            unit: l.unit,
        }),
        (Some(lu), Some(ru)) => match (unit_entry(lu.as_str()), unit_entry(ru.as_str())) {
            (Some((ld, _, lf)), Some((rd, _, rf))) if ld == rd => {
                // Express both in the finer unit: an exact integer rescale.
                let (fine, coarse_scale_l, coarse_scale_r) = if lf <= rf {
                    (l.unit.clone(), 1, rf / lf)
                } else {
                    (r.unit.clone(), lf / rf, 1)
                };
                let ln = l.num.scale(coarse_scale_l)?;
                let rn = r.num.scale(coarse_scale_r)?;
                Ok(Quantity {
                    num: num_op(op, ln, rn)?,
                    unit: fine,
                })
            }
            (Some(_), Some(_)) => Err(ArithError::DimensionMismatch {
                op,
                lhs: Some(lu.to_string()),
                rhs: Some(ru.to_string()),
            }),
            _ => Err(ArithError::UnknownConversion {
                lhs: lu.to_string(),
                rhs: ru.to_string(),
            }),
        },
        _ => Err(ArithError::DimensionMismatch {
            op,
            lhs: l.unit.as_ref().map(|u| u.to_string()),
            rhs: r.unit.as_ref().map(|u| u.to_string()),
        }),
    }
}

fn mul(l: Quantity, r: Quantity) -> Result<Quantity, ArithError> {
    let unit = match (l.unit, r.unit) {
        (Some(lu), Some(ru)) => {
            return Err(ArithError::UnitOnUnit {
                op: BinaryOpKind::Mul,
                lhs: lu.to_string(),
                rhs: ru.to_string(),
            });
        }
        (u, None) | (None, u) => u,
    };
    Ok(Quantity {
        num: num_op(BinaryOpKind::Mul, l.num, r.num)?,
        unit,
    })
}

fn div(l: Quantity, r: Quantity) -> Result<Quantity, ArithError> {
    if let Some(ru) = &r.unit {
        return Err(match &l.unit {
            Some(lu) => ArithError::UnitOnUnit {
                op: BinaryOpKind::Div,
                lhs: lu.to_string(),
                rhs: ru.to_string(),
            },
            None => ArithError::UnitlessOverUnit {
                rhs: ru.to_string(),
            },
        });
    }
    if r.num.is_zero() {
        return Err(ArithError::DivisionByZero);
    }
    let (Num::Int(a), Num::Int(b)) = (l.num, r.num) else {
        return Ok(Quantity {
            num: num_op(BinaryOpKind::Div, l.num, r.num)?,
            unit: l.unit,
        });
    };
    match a.checked_rem(b) {
        // i64::MIN / -1
        None => return Err(ArithError::Overflow),
        Some(0) => {
            return Ok(Quantity {
                num: num_op(BinaryOpKind::Div, l.num, r.num)?,
                unit: l.unit,
            });
        }
        Some(_) => {}
    }
    // Not exact in this unit: try each finer unit of the same table, the
    // coarsest first, so `1 ms / 4` is `250 us` rather than `250000000 ps`.
    if let Some(unit) = &l.unit
        && let Some((_, table, factor)) = unit_entry(unit.as_str())
    {
        let mut finer: Vec<(&str, u64)> = table
            .iter()
            .copied()
            .filter(|(_, f)| *f < factor && factor % *f == 0)
            .collect();
        finer.sort_by_key(|(_, f)| std::cmp::Reverse(*f));
        for (name, f) in finer {
            let scaled = a
                .checked_mul(i64::try_from(factor / f).map_err(|_| ArithError::Overflow)?)
                .ok_or(ArithError::Overflow)?;
            if scaled.checked_rem(b) == Some(0) {
                return Ok(Quantity {
                    num: Num::Int(scaled / b),
                    unit: Some(Name::new(name)),
                });
            }
        }
    }
    Err(ArithError::InexactIntegerDivision { lhs: a, rhs: b })
}

fn eval(expr: &PropertyExpr) -> Result<Quantity, ArithError> {
    match expr {
        PropertyExpr::Integer(v, unit) => Ok(Quantity {
            num: Num::Int(*v),
            unit: unit.clone(),
        }),
        PropertyExpr::Real(s, unit) => {
            let r: f64 = s.parse().map_err(|_| ArithError::NotNumeric("real"))?;
            Ok(Quantity {
                num: real(r)?,
                unit: unit.clone(),
            })
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let q = eval(inner)?;
            if q.unit.is_some() {
                return Err(ArithError::NotNumeric("doubly-united"));
            }
            Ok(Quantity {
                num: q.num,
                unit: Some(unit.clone()),
            })
        }
        PropertyExpr::BinaryOp { op, lhs, rhs } => {
            let l = eval(lhs)?;
            let r = eval(rhs)?;
            match op {
                BinaryOpKind::Add | BinaryOpKind::Sub => add_sub(*op, l, r),
                BinaryOpKind::Mul => mul(l, r),
                BinaryOpKind::Div => div(l, r),
            }
        }
        PropertyExpr::Enum(name) => Err(ArithError::ConstantOperand(name.to_string())),
        PropertyExpr::ComputedValue(_) | PropertyExpr::ValueRef(_) => {
            Err(ArithError::NotNumeric("compute/value reference"))
        }
        PropertyExpr::StringLit(_) => Err(ArithError::NotNumeric("string")),
        PropertyExpr::Boolean(_) => Err(ArithError::NotNumeric("boolean")),
        PropertyExpr::List(_) => Err(ArithError::NotNumeric("list")),
        PropertyExpr::Record(_) => Err(ArithError::NotNumeric("record")),
        PropertyExpr::Range { .. } => Err(ArithError::NotNumeric("range")),
        PropertyExpr::ClassifierValue(_) => Err(ArithError::NotNumeric("classifier")),
        PropertyExpr::ReferenceValue(_) => Err(ArithError::NotNumeric("reference")),
        PropertyExpr::Opaque(_) => Err(ArithError::NotNumeric("unparsed")),
    }
}

fn format_real(r: f64) -> String {
    // `{:?}` always keeps a decimal point or exponent (`40.0`, `1e-7`), so
    // the text still reads as an aadlreal and parses back to the same f64.
    format!("{r:?}")
}

/// Evaluate a numeric expression to a single `Integer` or `Real` leaf.
///
/// A leaf evaluates to itself. See the module docs for the rules.
pub fn evaluate(expr: &PropertyExpr) -> Result<PropertyExpr, ArithError> {
    let q = eval(expr)?;
    Ok(match q.num {
        Num::Int(i) => PropertyExpr::Integer(i, q.unit),
        Num::Real(r) => PropertyExpr::Real(format_real(r), q.unit),
    })
}

/// Fold an arithmetic expression where it evaluates. A `BinaryOp` that does
/// not evaluate is returned UNCHANGED — still a `BinaryOp`, which no timing,
/// size or integer accessor reads as a value, and which
/// [`arithmetic_errors`] reports.
pub fn fold(expr: PropertyExpr) -> PropertyExpr {
    match expr {
        PropertyExpr::BinaryOp { .. } => evaluate(&expr).unwrap_or(expr),
        other => other,
    }
}

/// The evaluation error of every arithmetic expression in `expr` that does
/// not evaluate, outermost first, looking inside lists, records and ranges.
pub fn arithmetic_errors(expr: &PropertyExpr) -> Vec<ArithError> {
    let mut out = Vec::new();
    collect_errors(expr, &mut out);
    out
}

fn collect_errors(expr: &PropertyExpr, out: &mut Vec<ArithError>) {
    match expr {
        PropertyExpr::BinaryOp { .. } => {
            if let Err(e) = evaluate(expr) {
                out.push(e);
            }
        }
        PropertyExpr::List(items) => items.iter().for_each(|i| collect_errors(i, out)),
        PropertyExpr::Record(fields) => fields.iter().for_each(|(_, v)| collect_errors(v, out)),
        PropertyExpr::Range { min, max, delta } => {
            collect_errors(min, out);
            collect_errors(max, out);
            if let Some(d) = delta {
                collect_errors(d, out);
            }
        }
        PropertyExpr::UnitValue(inner, _) => collect_errors(inner, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i64, unit: Option<&str>) -> PropertyExpr {
        PropertyExpr::Integer(v, unit.map(Name::new))
    }

    fn real_lit(s: &str, unit: Option<&str>) -> PropertyExpr {
        PropertyExpr::Real(s.to_string(), unit.map(Name::new))
    }

    fn bin(op: BinaryOpKind, l: PropertyExpr, r: PropertyExpr) -> PropertyExpr {
        PropertyExpr::BinaryOp {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        }
    }

    use BinaryOpKind::{Add, Div, Mul, Sub};

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn five_times_eight_ms_is_forty_ms_either_order() {
        assert_eq!(
            evaluate(&bin(Mul, int(5, None), int(8, Some("ms")))),
            Ok(int(40, Some("ms")))
        );
        assert_eq!(
            evaluate(&bin(Mul, int(8, Some("ms")), int(5, None))),
            Ok(int(40, Some("ms")))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn one_sec_minus_500_ms_is_500_ms() {
        assert_eq!(
            evaluate(&bin(Sub, int(1, Some("sec")), int(500, Some("ms")))),
            Ok(int(500, Some("ms")))
        );
        // The finer unit wins whichever side it is on.
        assert_eq!(
            evaluate(&bin(Add, int(500, Some("ms")), int(1, Some("sec")))),
            Ok(int(1500, Some("ms")))
        );
        assert_eq!(
            evaluate(&bin(Sub, int(500, Some("ms")), int(1, Some("sec")))),
            Ok(int(-500, Some("ms")))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn same_unit_sum_keeps_the_unit_and_size_units_convert() {
        assert_eq!(
            evaluate(&bin(Add, int(20, Some("ms")), int(20, Some("ms")))),
            Ok(int(40, Some("ms")))
        );
        // Unit names compare case-insensitively.
        assert_eq!(
            evaluate(&bin(Add, int(20, Some("ms")), int(20, Some("MS")))),
            Ok(int(40, Some("ms")))
        );
        // KByte is 1024 Bytes in spar's Size_Units table.
        assert_eq!(
            evaluate(&bin(Sub, int(1, Some("KByte")), int(24, Some("Bytes")))),
            Ok(int(1000, Some("Bytes")))
        );
        // An unknown unit is fine when both sides use it.
        assert_eq!(
            evaluate(&bin(Add, int(2, Some("furlong")), int(3, Some("furlong")))),
            Ok(int(5, Some("furlong")))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn unitless_arithmetic_and_precedence_tree() {
        // (2 + 3) * 8 ms
        assert_eq!(
            evaluate(&bin(
                Mul,
                bin(Add, int(2, None), int(3, None)),
                int(8, Some("ms"))
            )),
            Ok(int(40, Some("ms")))
        );
        assert_eq!(
            evaluate(&bin(Sub, int(10, None), int(3, None))),
            Ok(int(7, None))
        );
        assert_eq!(
            evaluate(&bin(Div, int(100, None), int(4, None))),
            Ok(int(25, None))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn integer_and_real() {
        assert_eq!(
            evaluate(&bin(Mul, real_lit("2.5", None), int(4, Some("ms")))),
            Ok(real_lit("10.0", Some("ms")))
        );
        assert_eq!(
            evaluate(&bin(Add, real_lit("0.5", Some("ms")), int(1, Some("ms")))),
            Ok(real_lit("1.5", Some("ms")))
        );
        // Real converts with the same factor as an integer.
        assert_eq!(
            evaluate(&bin(Add, real_lit("1.5", Some("ms")), int(500, Some("us")))),
            Ok(real_lit("2000.0", Some("us")))
        );
        assert_eq!(
            evaluate(&bin(Div, real_lit("1.0", Some("ms")), int(4, None))),
            Ok(real_lit("0.25", Some("ms")))
        );
        // A leaf evaluates to itself.
        assert_eq!(evaluate(&int(7, Some("ms"))), Ok(int(7, Some("ms"))));
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn inexact_integer_division_moves_to_a_finer_unit_or_errors() {
        assert_eq!(
            evaluate(&bin(Div, int(80, Some("ms")), int(2, None))),
            Ok(int(40, Some("ms")))
        );
        assert_eq!(
            evaluate(&bin(Div, int(1, Some("ms")), int(4, None))),
            Ok(int(250, Some("us")))
        );
        assert_eq!(
            evaluate(&bin(Div, int(1, Some("ms")), int(3, None))),
            Err(ArithError::InexactIntegerDivision { lhs: 1, rhs: 3 })
        );
        assert_eq!(
            evaluate(&bin(Div, int(7, None), int(2, None))),
            Err(ArithError::InexactIntegerDivision { lhs: 7, rhs: 2 })
        );
        // A unit outside the tables has no finer unit to move to.
        assert_eq!(
            evaluate(&bin(Div, int(7, Some("furlong")), int(2, None))),
            Err(ArithError::InexactIntegerDivision { lhs: 7, rhs: 2 })
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn ill_typed_expressions_have_no_value() {
        assert_eq!(
            evaluate(&bin(Mul, int(5, Some("ms")), int(8, Some("ms")))),
            Err(ArithError::UnitOnUnit {
                op: Mul,
                lhs: "ms".into(),
                rhs: "ms".into()
            })
        );
        assert_eq!(
            evaluate(&bin(Div, int(5, Some("ms")), int(8, Some("ms")))),
            Err(ArithError::UnitOnUnit {
                op: Div,
                lhs: "ms".into(),
                rhs: "ms".into()
            })
        );
        assert_eq!(
            evaluate(&bin(Div, int(5, None), int(8, Some("ms")))),
            Err(ArithError::UnitlessOverUnit { rhs: "ms".into() })
        );
        assert_eq!(
            evaluate(&bin(Add, int(5, Some("ms")), int(3, Some("KByte")))),
            Err(ArithError::DimensionMismatch {
                op: Add,
                lhs: Some("ms".into()),
                rhs: Some("KByte".into())
            })
        );
        assert_eq!(
            evaluate(&bin(Sub, int(5, Some("ms")), int(3, None))),
            Err(ArithError::DimensionMismatch {
                op: Sub,
                lhs: Some("ms".into()),
                rhs: None
            })
        );
        assert_eq!(
            evaluate(&bin(Add, int(5, None), int(3, Some("ms")))),
            Err(ArithError::DimensionMismatch {
                op: Add,
                lhs: None,
                rhs: Some("ms".into())
            })
        );
        assert_eq!(
            evaluate(&bin(Add, int(5, Some("furlong")), int(3, Some("ms")))),
            Err(ArithError::UnknownConversion {
                lhs: "furlong".into(),
                rhs: "ms".into()
            })
        );
        assert_eq!(
            evaluate(&bin(Div, int(40, Some("ms")), int(0, None))),
            Err(ArithError::DivisionByZero)
        );
        assert_eq!(
            evaluate(&bin(Div, real_lit("4.0", None), real_lit("0.0", None))),
            Err(ArithError::DivisionByZero)
        );
        assert_eq!(
            evaluate(&bin(Mul, int(i64::MAX, None), int(2, None))),
            Err(ArithError::Overflow)
        );
        // 10 000 hr in ps does not fit an i64.
        assert_eq!(
            evaluate(&bin(Add, int(10_000, Some("hr")), int(1, Some("ps")))),
            Err(ArithError::Overflow)
        );
        assert_eq!(
            evaluate(&bin(Div, int(i64::MIN, None), int(-1, None))),
            Err(ArithError::Overflow)
        );
        assert_eq!(
            evaluate(&bin(
                Mul,
                int(2, None),
                PropertyExpr::Enum(Name::new("My_Set::Base"))
            )),
            Err(ArithError::ConstantOperand("My_Set::Base".into()))
        );
        assert_eq!(
            evaluate(&bin(Add, PropertyExpr::StringLit("x".into()), int(1, None))),
            Err(ArithError::NotNumeric("string"))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn an_error_anywhere_poisons_the_whole_expression() {
        // (5 ms * 8 ms) + 1 ms: the inner error is the outer one.
        let inner = bin(Mul, int(5, Some("ms")), int(8, Some("ms")));
        let e = bin(Add, inner.clone(), int(1, Some("ms")));
        assert!(matches!(evaluate(&e), Err(ArithError::UnitOnUnit { .. })));
        // fold keeps the unevaluable tree, never its first operand.
        assert_eq!(fold(e.clone()), e);
        assert_eq!(arithmetic_errors(&e).len(), 1);
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn fold_and_errors_walk_ranges_lists_and_records() {
        let good = bin(Mul, int(2, None), int(1, Some("ms")));
        let bad = bin(Mul, int(5, Some("ms")), int(8, Some("ms")));
        assert_eq!(fold(good.clone()), int(2, Some("ms")));
        assert_eq!(fold(int(3, None)), int(3, None));
        let range = PropertyExpr::Range {
            min: Box::new(int(1, Some("ms"))),
            max: Box::new(bad.clone()),
            delta: Some(Box::new(bad.clone())),
        };
        assert_eq!(arithmetic_errors(&range).len(), 2);
        let list = PropertyExpr::List(vec![good.clone(), bad.clone()]);
        assert_eq!(arithmetic_errors(&list).len(), 1);
        let rec = PropertyExpr::Record(vec![(Name::new("a"), bad.clone())]);
        assert_eq!(arithmetic_errors(&rec).len(), 1);
        let uv = PropertyExpr::UnitValue(Box::new(bad), Name::new("ms"));
        assert_eq!(arithmetic_errors(&uv).len(), 1);
        assert!(arithmetic_errors(&good).is_empty());
        assert!(arithmetic_errors(&int(1, None)).is_empty());
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn unit_value_operand() {
        let uv = PropertyExpr::UnitValue(Box::new(int(8, None)), Name::new("ms"));
        assert_eq!(
            evaluate(&bin(Mul, int(5, None), uv)),
            Ok(int(40, Some("ms")))
        );
        let doubly = PropertyExpr::UnitValue(Box::new(int(8, Some("ms"))), Name::new("ms"));
        assert_eq!(
            evaluate(&doubly),
            Err(ArithError::NotNumeric("doubly-united"))
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-ARITHMETIC-001
    #[test]
    fn messages_name_the_offending_units() {
        let m = |e: PropertyExpr| evaluate(&e).unwrap_err().to_string();
        assert!(m(bin(Mul, int(5, Some("ms")), int(8, Some("ms")))).contains("ms * ms"));
        assert!(m(bin(Div, int(5, Some("ms")), int(8, Some("us")))).contains("ms / us"));
        assert!(m(bin(Add, int(5, Some("ms")), int(3, Some("KByte")))).contains("ms + KByte"));
        assert!(m(bin(Sub, int(5, Some("ms")), int(3, None))).contains("ms - unitless"));
        assert!(m(bin(Div, int(5, None), int(0, None))).contains("division by zero"));
    }
}
