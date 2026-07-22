//! Property grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `properties ...` section.
pub(crate) fn property_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PROPERTIES_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name() {
            property_association(p);
        }
    }
    m.complete(p, SyntaxKind::PROPERTY_SECTION);
}

/// Parse a property association: `Property => Value ;`
pub(crate) fn property_association(p: &mut Parser) {
    let m = p.start();

    // Property reference (possibly qualified: `PropSet::PropName`)
    property_ref(p);

    // `=>` or `+=>`
    if !p.eat(SyntaxKind::FAT_ARROW) && !p.eat(SyntaxKind::PLUS_ARROW) {
        p.error("expected `=>` or `+=>`");
    }

    // Optional `constant`
    p.eat(SyntaxKind::CONSTANT_KW);

    // Value(s) — may be modal
    property_value(p);
    while p.at(SyntaxKind::COMMA) {
        p.bump(SyntaxKind::COMMA);
        property_value(p);
    }

    // Optional `applies to`
    if p.at(SyntaxKind::APPLIES_KW) {
        applies_to(p);
    }

    // Optional `in binding`
    if p.at(SyntaxKind::IN_KW) && p.nth(1) == SyntaxKind::BINDING_KW {
        in_binding(p);
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::PROPERTY_ASSOCIATION);
}

/// Parse a property reference: `Name` or `PropSet::Name`.
fn property_ref(p: &mut Parser) {
    let m = p.start();
    if p.at_name() {
        p.bump_any();
        if p.at(SyntaxKind::COLON_COLON) {
            p.bump(SyntaxKind::COLON_COLON);
            if p.at_name() {
                p.bump_any();
            } else {
                p.error("expected property name after `::`");
            }
        }
    } else {
        p.error("expected property name");
    }
    m.complete(p, SyntaxKind::PROPERTY_REF);
}

/// Parse a property value expression.
fn property_value(p: &mut Parser) {
    // Could be modal: `value in modes (m1, m2)`
    property_expression(p);

    // Optional modal qualifier
    if p.at(SyntaxKind::IN_KW) && p.nth(1) == SyntaxKind::MODES_KW {
        let m = p.start();
        super::modes::in_modes(p);
        m.complete(p, SyntaxKind::MODAL_PROPERTY_VALUE);
    }
}

/// Parse a property expression, including binary operator chains.
///
/// AS5506B §11.2.5 admits `numeric_term` with binary operators (e.g.
/// `5 * 1000 ps`). Precedence is flat/left-associative — that is enough
/// to parse the common unit-scaling idiom without a full expression
/// grammar.
fn property_expression(p: &mut Parser) {
    property_expression_primary(p);
    while matches!(
        p.current(),
        SyntaxKind::STAR | SyntaxKind::PLUS | SyntaxKind::MINUS
    ) {
        p.bump_any();
        property_expression_primary(p);
    }
}

fn property_expression_primary(p: &mut Parser) {
    match p.current() {
        SyntaxKind::INTEGER_LIT => {
            let m = p.start();
            p.bump(SyntaxKind::INTEGER_LIT);
            // Optional unit identifier
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT); // unit name
            }
            // Check for range: `..`
            if p.at(SyntaxKind::DOT_DOT) {
                p.bump(SyntaxKind::DOT_DOT);
                property_expression(p);
                if p.at(SyntaxKind::DELTA_KW) {
                    let d = p.start();
                    p.bump(SyntaxKind::DELTA_KW);
                    property_expression(p);
                    d.complete(p, SyntaxKind::DELTA_VALUE);
                }
                m.complete(p, SyntaxKind::RANGE_VALUE);
            } else {
                m.complete(p, SyntaxKind::INTEGER_VALUE);
            }
        }
        SyntaxKind::REAL_LIT => {
            let m = p.start();
            p.bump(SyntaxKind::REAL_LIT);
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT); // unit name
            }
            if p.at(SyntaxKind::DOT_DOT) {
                p.bump(SyntaxKind::DOT_DOT);
                property_expression(p);
                if p.at(SyntaxKind::DELTA_KW) {
                    let d = p.start();
                    p.bump(SyntaxKind::DELTA_KW);
                    property_expression(p);
                    d.complete(p, SyntaxKind::DELTA_VALUE);
                }
                m.complete(p, SyntaxKind::RANGE_VALUE);
            } else {
                m.complete(p, SyntaxKind::REAL_VALUE);
            }
        }
        SyntaxKind::STRING_LIT => {
            let m = p.start();
            p.bump(SyntaxKind::STRING_LIT);
            m.complete(p, SyntaxKind::STRING_VALUE);
        }
        SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW => {
            let m = p.start();
            p.bump_any();
            m.complete(p, SyntaxKind::BOOLEAN_VALUE);
        }
        SyntaxKind::NOT_KW => {
            // Boolean negation
            let m = p.start();
            p.bump(SyntaxKind::NOT_KW);
            property_expression(p);
            m.complete(p, SyntaxKind::BOOLEAN_VALUE);
        }
        SyntaxKind::CLASSIFIER_KW => {
            let m = p.start();
            p.bump(SyntaxKind::CLASSIFIER_KW);
            p.expect(SyntaxKind::L_PAREN);
            super::classifier_ref(p);
            p.expect(SyntaxKind::R_PAREN);
            m.complete(p, SyntaxKind::CLASSIFIER_VALUE);
        }
        SyntaxKind::REFERENCE_KW => {
            let m = p.start();
            p.bump(SyntaxKind::REFERENCE_KW);
            p.expect(SyntaxKind::L_PAREN);
            containment_path(p);
            p.expect(SyntaxKind::R_PAREN);
            m.complete(p, SyntaxKind::REFERENCE_VALUE);
        }
        SyntaxKind::COMPUTE_KW => {
            let m = p.start();
            p.bump(SyntaxKind::COMPUTE_KW);
            p.expect(SyntaxKind::L_PAREN);
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT);
            }
            p.expect(SyntaxKind::R_PAREN);
            m.complete(p, SyntaxKind::COMPUTED_VALUE);
        }
        SyntaxKind::L_PAREN => {
            // List value: (val1, val2, ...)
            let m = p.start();
            p.bump(SyntaxKind::L_PAREN);
            if !p.at(SyntaxKind::R_PAREN) {
                property_expression(p);
                while p.eat(SyntaxKind::COMMA) {
                    property_expression(p);
                }
            }
            p.expect(SyntaxKind::R_PAREN);
            m.complete(p, SyntaxKind::LIST_VALUE);
        }
        SyntaxKind::L_BRACKET => {
            // Record value: [ field => value; ... ]
            let m = p.start();
            p.bump(SyntaxKind::L_BRACKET);
            while p.at(SyntaxKind::IDENT) {
                let f = p.start();
                p.bump(SyntaxKind::IDENT);
                p.expect(SyntaxKind::FAT_ARROW);
                property_expression(p);
                p.expect(SyntaxKind::SEMICOLON);
                f.complete(p, SyntaxKind::RECORD_FIELD);
            }
            p.expect(SyntaxKind::R_BRACKET);
            m.complete(p, SyntaxKind::RECORD_VALUE);
        }
        SyntaxKind::IDENT => {
            // Named value, enumeration literal, or property constant reference
            let m = p.start();
            p.bump(SyntaxKind::IDENT);
            // Possible qualified reference
            while p.at(SyntaxKind::COLON_COLON) {
                p.bump(SyntaxKind::COLON_COLON);
                if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                    p.bump_any();
                }
            }
            // Check for range: `ident .. expr`
            if p.at(SyntaxKind::DOT_DOT) {
                p.bump(SyntaxKind::DOT_DOT);
                property_expression(p);
                if p.at(SyntaxKind::DELTA_KW) {
                    let d = p.start();
                    p.bump(SyntaxKind::DELTA_KW);
                    property_expression(p);
                    d.complete(p, SyntaxKind::DELTA_VALUE);
                }
                m.complete(p, SyntaxKind::RANGE_VALUE);
            } else {
                m.complete(p, SyntaxKind::PROPERTY_EXPRESSION);
            }
        }
        SyntaxKind::PLUS | SyntaxKind::MINUS => {
            // Signed numeric value. Recurse into *primary* (not the outer
            // wrapper), otherwise the binary-op loop would greedily consume
            // the following `+`/`-`/`*` operators into the signed operand —
            // causing `-1 + 2` to parse as `-(1+2)` instead of `(-1)+2`.
            // AS-5506B §11.2.5: `numeric_term ::= [sign] numeric_literal`
            // — the sign is part of the signed literal, not a prefix over
            // the additive expression.
            let m = p.start();
            p.bump_any();
            property_expression_primary(p);
            m.complete(p, SyntaxKind::PROPERTY_EXPRESSION);
        }
        SyntaxKind::ERROR if is_c_style_radix_literal(p.current_text()) => {
            // A C-style radix literal (`0x40003000`, `0b1010`, `0o17`) — the
            // lexer emits these as a single ERROR token because they are not
            // valid AADL. Emit a targeted, actionable diagnostic and consume
            // the token so the property association recovers to the `;` (#337).
            let m = p.start();
            p.error(
                "C-style radix literal is not valid AADL; use based notation, \
                 e.g. `16#…#` for hexadecimal",
            );
            p.bump_any();
            m.complete(p, SyntaxKind::ERROR);
        }
        _ => {
            p.error("expected property expression");
        }
    }
}

/// Whether `text` is a C-style radix-prefixed literal (`0x…`, `0b…`, `0o…`,
/// any case). The lexer produces these as ERROR tokens; the parser uses this
/// to attach a specific diagnostic rather than the generic "expected
/// expression" message.
fn is_c_style_radix_literal(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() > 2
        && bytes[0] == b'0'
        && matches!(bytes[1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
}

/// Parse `applies to path1, path2 ...`
fn applies_to(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::APPLIES_KW);
    p.expect(SyntaxKind::TO_KW);
    containment_path(p);
    while p.eat(SyntaxKind::COMMA) {
        containment_path(p);
    }
    m.complete(p, SyntaxKind::APPLIES_TO);
}

/// Parse a containment path: `sub1.sub2.feature`
fn containment_path(p: &mut Parser) {
    let m = p.start();
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
        array_subscripts(p);
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT);
                array_subscripts(p);
            }
        }
    }
    m.complete(p, SyntaxKind::CONTAINMENT_PATH);
}

/// Consume zero or more array subscripts on a path segment, e.g. the
/// `[1]` in `reference(pc[1])` or `cpu.cores[0]` (AS5506B §10.6.2
/// contained-element array selection). The index is an integer literal,
/// an integer range (`[1 .. 3]`), or a named constant — accept all three
/// rather than just literals so property-constant indices parse too.
fn array_subscripts(p: &mut Parser) {
    while p.at(SyntaxKind::L_BRACKET) {
        p.bump(SyntaxKind::L_BRACKET);
        // index ::= INTEGER_LIT [ '..' INTEGER_LIT ] | IDENT (named constant)
        if p.at(SyntaxKind::INTEGER_LIT) {
            p.bump(SyntaxKind::INTEGER_LIT);
            if p.eat(SyntaxKind::DOT_DOT) && p.at(SyntaxKind::INTEGER_LIT) {
                p.bump(SyntaxKind::INTEGER_LIT);
            }
        } else if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        }
        p.expect(SyntaxKind::R_BRACKET);
    }
}

/// Parse `in binding (Classifier)`
fn in_binding(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IN_KW);
    p.bump(SyntaxKind::BINDING_KW);
    p.expect(SyntaxKind::L_PAREN);
    super::classifier_ref(p);
    p.expect(SyntaxKind::R_PAREN);
    m.complete(p, SyntaxKind::IN_BINDING);
}

/// Parse a property block: `{ prop => val; ... }`
pub(crate) fn property_block(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_CURLY);
    while p.at_name() {
        property_association(p);
    }
    p.expect(SyntaxKind::R_CURLY);
    m.complete(p, SyntaxKind::PROPERTY_SECTION);
}

/// Parse a property set declaration: `property set Name is ... end Name;`
pub(crate) fn property_set(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PROPERTY_KW);
    p.expect(SyntaxKind::SET_KW);
    super::name(p);
    p.expect(SyntaxKind::IS_KW);

    // with clauses
    while p.at(SyntaxKind::WITH_KW) {
        let w = p.start();
        p.bump(SyntaxKind::WITH_KW);
        super::name(p);
        while p.eat(SyntaxKind::COMMA) {
            super::name(p);
        }
        p.expect(SyntaxKind::SEMICOLON);
        w.complete(p, SyntaxKind::WITH_CLAUSE);
    }

    // property definitions and constants
    while !p.at(SyntaxKind::END_KW) && !p.at_end() {
        if p.at_name() {
            property_definition_or_constant(p);
        } else {
            break;
        }
    }

    p.expect(SyntaxKind::END_KW);
    super::name(p);
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::PROPERTY_SET);
}

fn property_definition_or_constant(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // name (IDENT or keyword-as-name)
    p.expect(SyntaxKind::COLON);

    if p.at(SyntaxKind::CONSTANT_KW) {
        // Property constant
        p.bump(SyntaxKind::CONSTANT_KW);
        property_type(p);
        p.expect(SyntaxKind::FAT_ARROW);
        property_expression(p);
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::PROPERTY_CONSTANT);
    } else if p.at(SyntaxKind::TYPE_KW) {
        // Property type declaration: `Name : type enumeration (...);`
        p.bump(SyntaxKind::TYPE_KW);
        property_type(p);
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::PROPERTY_TYPE_DECL);
    } else {
        // Property definition
        if p.eat(SyntaxKind::INHERIT_KW) {
            // inheritable
        }
        property_type(p);
        // Optional default value
        if p.eat(SyntaxKind::FAT_ARROW) {
            property_expression(p);
        }
        // applies to
        p.expect(SyntaxKind::APPLIES_KW);
        p.expect(SyntaxKind::TO_KW);
        p.expect(SyntaxKind::L_PAREN);
        // Applies to list
        applies_to_category(p);
        while p.eat(SyntaxKind::COMMA) {
            applies_to_category(p);
        }
        p.expect(SyntaxKind::R_PAREN);
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::PROPERTY_DEFINITION);
    }
}

fn property_type(p: &mut Parser) {
    let m = p.start();
    match p.current() {
        SyntaxKind::AADLBOOLEAN_KW => p.bump(SyntaxKind::AADLBOOLEAN_KW),
        SyntaxKind::AADLINTEGER_KW => {
            p.bump(SyntaxKind::AADLINTEGER_KW);
            numeric_range_opt(p);
            if p.at(SyntaxKind::UNITS_KW) {
                p.bump(SyntaxKind::UNITS_KW);
                numeric_units_designator(p);
            }
        }
        SyntaxKind::AADLREAL_KW => {
            p.bump(SyntaxKind::AADLREAL_KW);
            numeric_range_opt(p);
            if p.at(SyntaxKind::UNITS_KW) {
                p.bump(SyntaxKind::UNITS_KW);
                numeric_units_designator(p);
            }
        }
        SyntaxKind::AADLSTRING_KW => p.bump(SyntaxKind::AADLSTRING_KW),
        SyntaxKind::ENUMERATION_KW => {
            p.bump(SyntaxKind::ENUMERATION_KW);
            p.expect(SyntaxKind::L_PAREN);
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT);
                while p.eat(SyntaxKind::COMMA) {
                    if p.at(SyntaxKind::IDENT) {
                        p.bump(SyntaxKind::IDENT);
                    }
                }
            }
            p.expect(SyntaxKind::R_PAREN);
        }
        SyntaxKind::LIST_KW => {
            p.bump(SyntaxKind::LIST_KW);
            p.expect(SyntaxKind::OF_KW);
            property_type(p);
        }
        SyntaxKind::RANGE_KW => {
            p.bump(SyntaxKind::RANGE_KW);
            p.expect(SyntaxKind::OF_KW);
            property_type(p);
        }
        SyntaxKind::RECORD_KW => {
            p.bump(SyntaxKind::RECORD_KW);
            p.expect(SyntaxKind::L_PAREN);
            while p.at(SyntaxKind::IDENT) {
                let f = p.start();
                p.bump(SyntaxKind::IDENT);
                p.expect(SyntaxKind::COLON);
                property_type(p);
                p.expect(SyntaxKind::SEMICOLON);
                f.complete(p, SyntaxKind::RECORD_FIELD);
            }
            p.expect(SyntaxKind::R_PAREN);
        }
        SyntaxKind::UNITS_KW => {
            // units type: units (base, derived => base * factor, ...)
            p.bump(SyntaxKind::UNITS_KW);
            units_designator_body(p);
        }
        SyntaxKind::CLASSIFIER_KW => {
            p.bump(SyntaxKind::CLASSIFIER_KW);
            // Optional category constraint: a comma-separated list of
            // classifier references (AS5506B §11.3), e.g.
            // `classifier (process, thread)`.
            classifier_ref_list_in_parens(p);
        }
        SyntaxKind::REFERENCE_KW => {
            p.bump(SyntaxKind::REFERENCE_KW);
            // `reference (processor, system)` — multiple referent categories.
            classifier_ref_list_in_parens(p);
        }
        SyntaxKind::IDENT => {
            // Type reference
            super::classifier_ref(p);
        }
        _ => {
            p.error("expected property type");
        }
    }
    m.complete(p, SyntaxKind::PROPERTY_TYPE);
}

/// Parse an optional `( ref [, ref]* )` category/classifier list following
/// `reference` / `classifier` in a property type (AS5506B §11.3). No-op when
/// no parenthesis follows (the constraint is optional).
fn classifier_ref_list_in_parens(p: &mut Parser) {
    if !p.at(SyntaxKind::L_PAREN) {
        return;
    }
    p.bump(SyntaxKind::L_PAREN);
    super::classifier_ref(p);
    while p.eat(SyntaxKind::COMMA) {
        super::classifier_ref(p);
    }
    p.expect(SyntaxKind::R_PAREN);
}

/// Parse an optional numeric range constraint on `aadlinteger`/`aadlreal`
/// in a property-type definition (AS5506B §11.3): `lower .. upper`, where
/// each bound is a signed numeric literal — optionally carrying a unit
/// (`1.5 meter`) — or a named property constant (`Max_Aadlinteger`).
///
/// Only entered on a numeric/sign start so a bare `units`/`applies` clause
/// is left for the caller; the range keyword grammar (`range of …`) is
/// handled separately.
fn numeric_range_opt(p: &mut Parser) {
    if !(p.at(SyntaxKind::INTEGER_LIT)
        || p.at(SyntaxKind::REAL_LIT)
        || p.at(SyntaxKind::MINUS)
        || p.at(SyntaxKind::PLUS))
    {
        return;
    }
    numeric_bound(p);
    if p.eat(SyntaxKind::DOT_DOT) {
        numeric_bound(p);
    }
}

/// One bound of a numeric range: `[+|-] (INTEGER_LIT | REAL_LIT) [unit]`
/// or a named constant `IDENT`.
fn numeric_bound(p: &mut Parser) {
    let _ = p.eat(SyntaxKind::PLUS) || p.eat(SyntaxKind::MINUS);
    if p.at(SyntaxKind::INTEGER_LIT) || p.at(SyntaxKind::REAL_LIT) {
        p.bump_any();
        // Optional unit identifier on the bound, e.g. `1.5 meter`.
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        }
    } else if p.at(SyntaxKind::IDENT) {
        // Named property constant bound (possibly qualified `set::Name`).
        p.bump(SyntaxKind::IDENT);
        if p.eat(SyntaxKind::COLON_COLON) && p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        }
    }
}

/// Parse `(uA, mA => uA * 1000, ...)` — body of a `units` designator.
///
/// Called after the `units` keyword has been consumed. Shared between the
/// standalone `units (...)` property type (AS5506B §11.3) and inline use
/// on `aadlreal`/`aadlinteger` (`aadlreal units (...)`).
fn units_designator_body(p: &mut Parser) {
    p.expect(SyntaxKind::L_PAREN);
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
        while p.eat(SyntaxKind::COMMA) {
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT);
                if p.eat(SyntaxKind::FAT_ARROW) {
                    if p.at(SyntaxKind::IDENT) {
                        p.bump(SyntaxKind::IDENT);
                    }
                    if p.eat(SyntaxKind::STAR) {
                        if p.at(SyntaxKind::INTEGER_LIT) {
                            p.bump(SyntaxKind::INTEGER_LIT);
                        } else if p.at(SyntaxKind::REAL_LIT) {
                            p.bump(SyntaxKind::REAL_LIT);
                        }
                    }
                }
            }
        }
    }
    p.expect(SyntaxKind::R_PAREN);
}

/// On `aadlreal`/`aadlinteger`, accept either a named units classifier
/// (`units My_Units`) or an inline `units (...)` block (AS5506B §11.3).
fn numeric_units_designator(p: &mut Parser) {
    if p.at(SyntaxKind::L_PAREN) {
        units_designator_body(p);
    } else if p.at(SyntaxKind::IDENT) {
        super::classifier_ref(p);
    }
}

fn applies_to_category(p: &mut Parser) {
    use SyntaxKind::*;
    if p.at(ALL_KW) {
        p.bump(ALL_KW);
    } else if p.at(FEATURE_KW) {
        // `feature`, `feature group`, or `feature group type` (AS5506 §11.3
        // property-owner list).
        p.bump(FEATURE_KW);
        if p.eat(GROUP_KW) {
            p.eat(TYPE_KW);
        }
    } else if p.current().is_component_category_kw() {
        super::component_category(p);
    } else if matches!(
        p.current(),
        PORT_KW | FLOW_KW | MODE_KW | ACCESS_KW | PARAMETER_KW | CONNECTIONS_KW
    ) {
        // Non-component-category property owners (AS5506 §11.3): a property
        // may also apply to ports, flows, modes, connections, access
        // features, and parameters — not just component categories.
        p.bump_any();
    } else if p.at(IDENT) {
        p.bump(IDENT);
    } else {
        p.error(
            "expected a property owner: a component category, `all`, `port`, \
             `feature`, `flow`, `mode`, `connections`, `access`, or `parameter`",
        );
    }
}
