//! Grammar rules for SysML v2 definitions and usages.
//!
//! Handles:
//! - `part def Name { ... }` / `part name : Type { ... }`
//! - `port def Name { ... }` / `port name : Type;`
//! - `connection def Name { ... }` / `connect a.p to b.p;`
//! - `attribute name : Type;`
//! - `item def Name { ... }` / `item name : Type;`
//! - Feature declarations, specialization, multiplicity

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// Recovery set for member-level parsing -- stop at tokens that start new members.
const MEMBER_RECOVERY: TokenSet = TokenSet::new(&[
    SyntaxKind::PART_KW,
    SyntaxKind::PORT_KW,
    SyntaxKind::CONNECTION_KW,
    SyntaxKind::CONNECT_KW,
    SyntaxKind::ACTION_KW,
    SyntaxKind::STATE_KW,
    SyntaxKind::ATTRIBUTE_KW,
    SyntaxKind::ITEM_KW,
    SyntaxKind::INTERFACE_KW,
    SyntaxKind::ENUM_KW,
    SyntaxKind::REQUIREMENT_KW,
    SyntaxKind::CONSTRAINT_KW,
    SyntaxKind::IMPORT_KW,
    SyntaxKind::PACKAGE_KW,
    SyntaxKind::R_CURLY,
    SyntaxKind::IN_KW,
    SyntaxKind::OUT_KW,
    SyntaxKind::INOUT_KW,
    SyntaxKind::REF_KW,
    SyntaxKind::ABSTRACT_KW,
    SyntaxKind::COMMENT_KW,
    SyntaxKind::DOC_KW,
    SyntaxKind::FEATURE_KW,
]);

/// Parse a member of a namespace body.
///
/// Dispatches based on the leading keyword to the appropriate definition or
/// usage parser.
pub(crate) fn member(p: &mut Parser) {
    // Handle direction prefix: `in`, `out`, `inout`
    let has_direction = matches!(
        p.current(),
        SyntaxKind::IN_KW | SyntaxKind::OUT_KW | SyntaxKind::INOUT_KW
    );

    // Check for `comment` and `doc` nodes
    if p.at(SyntaxKind::COMMENT_KW) {
        comment_node(p);
        return;
    }
    if p.at(SyntaxKind::DOC_KW) {
        doc_node(p);
        return;
    }

    // Direction prefix must precede a definition keyword, `attribute`, or `item`
    if has_direction {
        // Peek past direction to see what follows
        let next = p.nth(1);
        match next {
            // `in port name`, `out attribute name`, etc.
            k if k.is_definition_kw() || k == SyntaxKind::IDENT || k == SyntaxKind::FEATURE_KW => {
                // Will be handled below with direction consumed inside the
                // specific handler
            }
            _ => {
                // Standalone `in name : Type;` -- feature decl with direction
                feature_decl_with_direction(p);
                return;
            }
        }
    }

    match p.current() {
        SyntaxKind::PART_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::PART_DEF);
            } else {
                part_usage(p);
            }
        }
        SyntaxKind::PORT_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::PORT_DEF);
            } else {
                port_usage(p);
            }
        }
        SyntaxKind::CONNECTION_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::CONNECTION_DEF);
            } else {
                // `connection` usage -- named connection
                connection_usage_named(p);
            }
        }
        SyntaxKind::CONNECT_KW => {
            connection_usage(p);
        }
        SyntaxKind::ACTION_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::ACTION_DEF);
            } else {
                generic_usage(p, SyntaxKind::ACTION_USAGE);
            }
        }
        SyntaxKind::STATE_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::STATE_DEF);
            } else {
                generic_usage(p, SyntaxKind::STATE_USAGE);
            }
        }
        SyntaxKind::ATTRIBUTE_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::ATTRIBUTE_DEF);
            } else {
                attribute_usage(p);
            }
        }
        SyntaxKind::ITEM_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::ITEM_DEF);
            } else {
                item_usage(p);
            }
        }
        SyntaxKind::INTERFACE_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::INTERFACE_DEF);
            } else {
                generic_usage(p, SyntaxKind::INTERFACE_USAGE);
            }
        }
        SyntaxKind::ENUM_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::ENUM_DEF);
            } else {
                generic_usage(p, SyntaxKind::ENUM_USAGE);
            }
        }
        SyntaxKind::REQUIREMENT_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::REQUIREMENT_DEF);
            } else {
                generic_usage(p, SyntaxKind::PART_USAGE);
            }
        }
        SyntaxKind::CONSTRAINT_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::CONSTRAINT_DEF);
            } else {
                generic_usage(p, SyntaxKind::PART_USAGE);
            }
        }
        SyntaxKind::CALC_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::CALC_DEF);
            } else {
                generic_usage(p, SyntaxKind::PART_USAGE);
            }
        }
        SyntaxKind::ALLOCATION_KW => {
            if p.nth(1) == SyntaxKind::DEF_KW {
                definition(p, SyntaxKind::ALLOCATION_DEF);
            } else {
                generic_usage(p, SyntaxKind::PART_USAGE);
            }
        }
        SyntaxKind::IN_KW | SyntaxKind::OUT_KW | SyntaxKind::INOUT_KW => {
            // Direction prefix: consume it, then dispatch
            let next = p.nth(1);
            if next == SyntaxKind::PORT_KW {
                port_usage(p);
            } else if next == SyntaxKind::ATTRIBUTE_KW {
                attribute_usage(p);
            } else if next == SyntaxKind::ITEM_KW {
                item_usage(p);
            } else {
                feature_decl_with_direction(p);
            }
        }
        SyntaxKind::REF_KW => {
            ref_usage(p);
        }
        SyntaxKind::ABSTRACT_KW => {
            // `abstract part def ...` or `abstract def ...`
            let m = p.start();
            p.bump(SyntaxKind::ABSTRACT_KW);
            if p.current().is_definition_kw() && p.nth(1) == SyntaxKind::DEF_KW {
                let def_kind = match p.current() {
                    SyntaxKind::PART_KW => SyntaxKind::PART_DEF,
                    SyntaxKind::PORT_KW => SyntaxKind::PORT_DEF,
                    SyntaxKind::CONNECTION_KW => SyntaxKind::CONNECTION_DEF,
                    SyntaxKind::ACTION_KW => SyntaxKind::ACTION_DEF,
                    SyntaxKind::STATE_KW => SyntaxKind::STATE_DEF,
                    SyntaxKind::ATTRIBUTE_KW => SyntaxKind::ATTRIBUTE_DEF,
                    SyntaxKind::ITEM_KW => SyntaxKind::ITEM_DEF,
                    SyntaxKind::INTERFACE_KW => SyntaxKind::INTERFACE_DEF,
                    _ => SyntaxKind::PART_DEF,
                };
                p.bump_any(); // category keyword
                p.bump(SyntaxKind::DEF_KW);
                if super::at_ident_or_kw(p) {
                    super::name(p);
                }
                opt_specialization(p);
                if p.at(SyntaxKind::L_CURLY) {
                    super::packages::namespace_body(p);
                } else {
                    p.expect(SyntaxKind::SEMICOLON);
                }
                m.complete(p, def_kind);
            } else {
                p.err_and_bump("expected definition after `abstract`");
                m.abandon(p);
            }
        }
        SyntaxKind::FEATURE_KW => {
            feature_decl_with_direction(p);
        }
        SyntaxKind::IDENT => {
            // Named feature or usage: `name : Type;`
            feature_decl(p);
        }
        _ => {
            p.err_recover("expected member declaration", MEMBER_RECOVERY);
        }
    }
}

/// Parse a definition: `<category> def Name specialization? body`
///
/// ```text
/// part def Vehicle { ... }
/// port def SensorPort { ... }
/// connection def SensorConnection { ... }
/// ```
fn definition(p: &mut Parser, node_kind: SyntaxKind) {
    let m = p.start();

    // Consume category keyword (part, port, connection, etc.)
    p.bump_any();
    // Consume `def`
    p.bump(SyntaxKind::DEF_KW);

    // Name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Optional specialization
    opt_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, node_kind);
}

/// Parse a part usage.
///
/// ```text
/// part name : Type { ... }
/// part name : Type;
/// part name :> SuperPart;
/// part name [0..*] : Type;
/// ```
fn part_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PART_KW);

    // Optional name
    if super::at_ident_or_kw(p) && !p.at(SyntaxKind::DEF_KW) {
        super::name(p);
    }

    // Optional multiplicity
    opt_multiplicity(p);

    // Typing, specialization, or redefines
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::PART_USAGE);
}

/// Parse a port usage.
///
/// ```text
/// port name : Type;
/// in port name : Type;
/// out port name : Type;
/// ```
fn port_usage(p: &mut Parser) {
    let m = p.start();

    // Optional direction: in, out, inout
    opt_direction(p);

    p.bump(SyntaxKind::PORT_KW);

    // Optional name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Optional multiplicity
    opt_multiplicity(p);

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::PORT_USAGE);
}

/// Parse a connection usage: `connect a.p to b.p;`
///
/// ```text
/// connect a.sensor to b.input;
/// ```
fn connection_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CONNECT_KW);

    // First endpoint
    connect_endpoint(p);

    // `to` keyword
    p.expect(SyntaxKind::TO_KW);

    // Second endpoint
    connect_endpoint(p);

    // Semicolon
    p.expect(SyntaxKind::SEMICOLON);

    m.complete(p, SyntaxKind::CONNECTION_USAGE);
}

/// Parse a named connection usage: `connection name : ConnectionDef { ... }`
fn connection_usage_named(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CONNECTION_KW);

    // Optional name
    if super::at_ident_or_kw(p) && !p.at(SyntaxKind::DEF_KW) {
        super::name(p);
    }

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::CONNECTION_USAGE);
}

/// Parse a connect endpoint: `a.b.c`
fn connect_endpoint(p: &mut Parser) {
    let m = p.start();
    super::feature_chain(p);
    m.complete(p, SyntaxKind::CONNECT_ENDPOINT);
}

/// Parse an attribute usage.
///
/// ```text
/// attribute name : Type;
/// attribute name : Type = value;
/// ```
fn attribute_usage(p: &mut Parser) {
    let m = p.start();

    // Optional direction
    opt_direction(p);

    p.bump(SyntaxKind::ATTRIBUTE_KW);

    // Optional name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Optional multiplicity
    opt_multiplicity(p);

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Optional default value: `= expr`
    if p.at(SyntaxKind::EQ) {
        p.bump(SyntaxKind::EQ);
        // Simple expression: literal or identifier
        if p.at(SyntaxKind::INTEGER_LIT)
            || p.at(SyntaxKind::REAL_LIT)
            || p.at(SyntaxKind::STRING_LIT)
            || p.at(SyntaxKind::TRUE_KW)
            || p.at(SyntaxKind::FALSE_KW)
        {
            p.bump_any();
        } else if super::at_ident_or_kw(p) {
            super::bump_as_ident(p);
        }
    }

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::ATTRIBUTE_USAGE);
}

/// Parse an item usage.
///
/// ```text
/// item name : Type;
/// out item data : SensorData;
/// ```
fn item_usage(p: &mut Parser) {
    let m = p.start();

    // Optional direction
    opt_direction(p);

    p.bump(SyntaxKind::ITEM_KW);

    // Optional name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Optional multiplicity
    opt_multiplicity(p);

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::ITEM_USAGE);
}

/// Parse a `ref` usage: `ref part name : Type;`
fn ref_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::REF_KW);

    // The kind after `ref`
    let node_kind = match p.current() {
        SyntaxKind::PART_KW => {
            p.bump(SyntaxKind::PART_KW);
            SyntaxKind::REF_USAGE
        }
        SyntaxKind::PORT_KW => {
            p.bump(SyntaxKind::PORT_KW);
            SyntaxKind::REF_USAGE
        }
        _ => {
            // Just `ref name : Type;`
            SyntaxKind::REF_USAGE
        }
    };

    // Optional name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, node_kind);
}

/// Parse a generic usage for keywords we handle uniformly.
fn generic_usage(p: &mut Parser, node_kind: SyntaxKind) {
    let m = p.start();
    p.bump_any(); // keyword

    // Optional name
    if super::at_ident_or_kw(p) && !p.at(SyntaxKind::DEF_KW) {
        super::name(p);
    }

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, node_kind);
}

/// Parse a feature declaration: `name : Type;`
fn feature_decl(p: &mut Parser) {
    let m = p.start();
    super::name(p);

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Optional multiplicity
    opt_multiplicity(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::FEATURE_DECL);
}

/// Parse a feature declaration with an optional direction prefix.
///
/// ```text
/// in feature name : Type;
/// out item data : SensorData;
/// ```
fn feature_decl_with_direction(p: &mut Parser) {
    let m = p.start();

    // Direction
    opt_direction(p);

    // Optional keyword: `feature`, `item`, `attribute`, etc.
    if p.at(SyntaxKind::FEATURE_KW) || p.at(SyntaxKind::ITEM_KW) || p.at(SyntaxKind::ATTRIBUTE_KW) {
        p.bump_any();
    }

    // Name
    if super::at_ident_or_kw(p) {
        super::name(p);
    }

    // Typing or specialization
    opt_typing_or_specialization(p);

    // Optional multiplicity
    opt_multiplicity(p);

    // Body or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::FEATURE_DECL);
}

/// Parse a comment node: `comment /* text */` or `comment "text"`
fn comment_node(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::COMMENT_KW);
    // Optional name
    if p.at(SyntaxKind::IDENT) && p.nth(1) != SyntaxKind::SEMICOLON {
        // Check if it looks like a name (followed by about/locale)
    }
    // Comment body -- could be a string or block comment token
    if p.at(SyntaxKind::STRING_LIT) {
        p.bump(SyntaxKind::STRING_LIT);
    }
    // Optional semicolon
    p.eat(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::COMMENT_NODE);
}

/// Parse a doc node: `doc /* text */`
fn doc_node(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::DOC_KW);
    if p.at(SyntaxKind::STRING_LIT) {
        p.bump(SyntaxKind::STRING_LIT);
    }
    p.eat(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::DOC_NODE);
}

// ---------------------------------------------------------------------------
// Helper parsers
// ---------------------------------------------------------------------------

/// Parse an optional direction: `in`, `out`, `inout`.
fn opt_direction(p: &mut Parser) {
    if matches!(
        p.current(),
        SyntaxKind::IN_KW | SyntaxKind::OUT_KW | SyntaxKind::INOUT_KW
    ) {
        let m = p.start();
        p.bump_any();
        m.complete(p, SyntaxKind::DIRECTION);
    }
}

/// Parse optional typing (`:`) or specialization (`:>`).
///
/// ```text
/// : Type
/// :> SuperType
/// :>> RedefType
/// specializes Type
/// ```
fn opt_typing_or_specialization(p: &mut Parser) {
    match p.current() {
        SyntaxKind::COLON if p.nth(1) != SyntaxKind::COLON => {
            let m = p.start();
            p.bump(SyntaxKind::COLON);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::TYPING);
        }
        SyntaxKind::COLON_GT => {
            let m = p.start();
            p.bump(SyntaxKind::COLON_GT);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        SyntaxKind::COLON_GT_GT => {
            let m = p.start();
            p.bump(SyntaxKind::COLON_GT_GT);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        SyntaxKind::SPECIALIZES_KW => {
            let m = p.start();
            p.bump(SyntaxKind::SPECIALIZES_KW);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        SyntaxKind::SUBSETS_KW => {
            let m = p.start();
            p.bump(SyntaxKind::SUBSETS_KW);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        SyntaxKind::REDEFINES_KW => {
            let m = p.start();
            p.bump(SyntaxKind::REDEFINES_KW);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        _ => {}
    }
}

/// Parse optional specialization for definitions.
///
/// ```text
/// :> SuperDef
/// specializes SuperDef
/// ```
fn opt_specialization(p: &mut Parser) {
    match p.current() {
        SyntaxKind::COLON_GT => {
            let m = p.start();
            p.bump(SyntaxKind::COLON_GT);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        SyntaxKind::SPECIALIZES_KW => {
            let m = p.start();
            p.bump(SyntaxKind::SPECIALIZES_KW);
            super::qualified_name(p);
            m.complete(p, SyntaxKind::SPECIALIZATION);
        }
        _ => {}
    }
}

/// Parse optional multiplicity: `[n]`, `[0..*]`, `[1..5]`.
fn opt_multiplicity(p: &mut Parser) {
    if p.at(SyntaxKind::L_BRACKET) {
        let m = p.start();
        p.bump(SyntaxKind::L_BRACKET);

        // Lower bound or single value
        if p.at(SyntaxKind::INTEGER_LIT) || p.at(SyntaxKind::STAR) {
            p.bump_any();
        }

        // Range: `..`
        if p.at(SyntaxKind::DOT_DOT) {
            p.bump(SyntaxKind::DOT_DOT);
            // Upper bound
            if p.at(SyntaxKind::INTEGER_LIT) || p.at(SyntaxKind::STAR) {
                p.bump_any();
            }
        }

        p.expect(SyntaxKind::R_BRACKET);
        m.complete(p, SyntaxKind::MULTIPLICITY);
    }
}
