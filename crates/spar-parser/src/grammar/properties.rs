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
            // Signed numeric value
            let m = p.start();
            p.bump_any();
            property_expression(p);
            m.complete(p, SyntaxKind::PROPERTY_EXPRESSION);
        }
        _ => {
            p.error("expected property expression");
        }
    }
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
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT);
            }
        }
    }
    m.complete(p, SyntaxKind::CONTAINMENT_PATH);
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
            if p.at(SyntaxKind::UNITS_KW) {
                p.bump(SyntaxKind::UNITS_KW);
                numeric_units_designator(p);
            }
        }
        SyntaxKind::AADLREAL_KW => {
            p.bump(SyntaxKind::AADLREAL_KW);
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
            // Optional category constraint
            if p.at(SyntaxKind::L_PAREN) {
                p.bump(SyntaxKind::L_PAREN);
                super::classifier_ref(p);
                p.expect(SyntaxKind::R_PAREN);
            }
        }
        SyntaxKind::REFERENCE_KW => {
            p.bump(SyntaxKind::REFERENCE_KW);
            if p.at(SyntaxKind::L_PAREN) {
                p.bump(SyntaxKind::L_PAREN);
                super::classifier_ref(p);
                p.expect(SyntaxKind::R_PAREN);
            }
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
    if p.at(SyntaxKind::ALL_KW) {
        p.bump(SyntaxKind::ALL_KW);
    } else if p.current().is_component_category_kw() {
        super::component_category(p);
    } else if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected component category or `all`");
    }
}
