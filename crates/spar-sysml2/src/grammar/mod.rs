//! SysML v2 grammar rules.
//!
<<<<<<< HEAD
//! Each function corresponds to a grammar production from the SysML v2 spec.
//! Functions take a `&mut Parser` and build nodes via markers.

mod packages;
mod parts;
=======
//! Each function corresponds to a grammar production from the SysML v2
//! specification. Functions take a `&mut Parser` and build nodes via markers.

pub mod constraints;
pub mod requirements;
>>>>>>> feat/sysml2-parser-reqs

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse a complete SysML v2 source file.
///
<<<<<<< HEAD
/// ```text
/// SourceFile = NamespaceMember*
/// NamespaceMember = Package | ImportDecl | Definition | Usage
=======
/// ```sysml
/// SourceFile = (Definition | Relationship)*
>>>>>>> feat/sysml2-parser-reqs
/// ```
pub fn source_file(p: &mut Parser) {
    let m = p.start();
    while !p.at_end() {
        match p.current() {
<<<<<<< HEAD
            SyntaxKind::PACKAGE_KW => packages::package(p),
            SyntaxKind::IMPORT_KW => packages::import_decl(p),
            k if is_member_start(k, p) => parts::member(p),
            SyntaxKind::EOF => break,
            _ => {
                p.err_and_bump("expected package, import, or definition");
=======
            SyntaxKind::REQUIREMENT_KW => {
                requirements::requirement(p);
            }
            SyntaxKind::CONSTRAINT_KW => {
                constraints::constraint(p);
            }
            SyntaxKind::SATISFY_KW => {
                requirements::satisfy_req(p);
            }
            SyntaxKind::VERIFY_KW => {
                requirements::verify_req(p);
            }
            SyntaxKind::REFINE_KW => {
                requirements::refine_req(p);
            }
            SyntaxKind::EOF => break,
            _ => {
                p.err_and_bump("expected requirement, constraint, satisfy, verify, or refine");
>>>>>>> feat/sysml2-parser-reqs
            }
        }
    }
    m.complete(p, SyntaxKind::SOURCE_FILE);
}

<<<<<<< HEAD
/// Returns true if the current token can start a namespace member
/// (definition or usage).
fn is_member_start(kind: SyntaxKind, p: &Parser) -> bool {
    match kind {
        // Definition keywords: `part def`, `port def`, etc.
        k if k.is_definition_kw() => true,
        // `connect` usage
        SyntaxKind::CONNECT_KW => true,
        // Direction prefix: `in`, `out`, `inout`
        SyntaxKind::IN_KW | SyntaxKind::OUT_KW | SyntaxKind::INOUT_KW => true,
        // `ref` prefix
        SyntaxKind::REF_KW => true,
        // `abstract` prefix
        SyntaxKind::ABSTRACT_KW => true,
        // Identifier could be a named usage
        SyntaxKind::IDENT => true,
        // `attribute` keyword
        SyntaxKind::ATTRIBUTE_KW => true,
        // `comment` or `doc` at top level
        SyntaxKind::COMMENT_KW | SyntaxKind::DOC_KW => true,
        // `feature` keyword
        SyntaxKind::FEATURE_KW => true,
        // Check for `connection` not followed by `def` -- this is still a
        // member start, handled in parts::member
        _ => {
            let _ = p;
            false
        }
    }
}

/// Parse a qualified name: `Pkg::Sub::Name` or simple `Name`.
pub(crate) fn qualified_name(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) && !p.current().is_keyword() {
=======
/// Parse a dotted name reference: `name` or `name.name` or `name.name.name`.
///
/// Used for requirement/constraint references, subject types, and `by` targets.
pub(crate) fn name_ref(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) {
>>>>>>> feat/sysml2-parser-reqs
        p.error("expected name");
        return false;
    }
    let m = p.start();
<<<<<<< HEAD
    bump_as_ident(p);
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
            bump_as_ident(p);
        } else if p.at(SyntaxKind::STAR) {
            // wildcard import: `Pkg::*`
            p.bump(SyntaxKind::STAR);
        } else {
            p.error("expected identifier after `::`");
            break;
        }
    }
    m.complete(p, SyntaxKind::QUALIFIED_NAME);
    true
}

/// Parse a simple name (single identifier).
pub(crate) fn name(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) && !p.current().is_keyword() {
        p.error("expected name");
        return false;
    }
    let m = p.start();
    bump_as_ident(p);
    m.complete(p, SyntaxKind::NAME);
    true
}

/// Parse a feature chain: `a.b.c` (dotted reference).
pub(crate) fn feature_chain(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) && !p.current().is_keyword() {
        p.error("expected feature reference");
        return false;
    }
    let m = p.start();
    bump_as_ident(p);
    while p.at(SyntaxKind::DOT) {
        p.bump(SyntaxKind::DOT);
        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
            bump_as_ident(p);
=======
    p.bump(SyntaxKind::IDENT);
    while p.at(SyntaxKind::DOT) {
        p.bump(SyntaxKind::DOT);
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
>>>>>>> feat/sysml2-parser-reqs
        } else {
            p.error("expected identifier after `.`");
            break;
        }
    }
<<<<<<< HEAD
    m.complete(p, SyntaxKind::FEATURE_CHAIN);
    true
}

/// Returns true if the current token is an identifier or a keyword that can
/// appear as an identifier in name contexts.
pub(crate) fn at_ident_or_kw(p: &Parser) -> bool {
    p.at(SyntaxKind::IDENT) || p.current().is_keyword()
}

/// Bump the current token, treating keywords as identifiers in name positions.
pub(crate) fn bump_as_ident(p: &mut Parser) {
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.bump_any();
=======
    // Also handle `::` qualified paths
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        } else {
            p.error("expected identifier after `::`");
            break;
        }
    }
    m.complete(p, SyntaxKind::NAME_REF);
    true
}

/// Parse a type reference after `:`.
pub(crate) fn type_ref(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) {
        p.error("expected type name");
        return false;
    }
    let m = p.start();
    p.bump(SyntaxKind::IDENT);
    // Handle qualified type refs: `Pkg::Type`
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        } else {
            p.error("expected identifier after `::`");
            break;
        }
    }
    m.complete(p, SyntaxKind::TYPE_REF);
    true
}

/// Parse a `doc /* text */` member.
pub(crate) fn doc_member(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::DOC_KW);
    // The block comment is trivia in the token stream, but for doc comments
    // we expect a block comment to follow. Since block comments are trivia
    // and hidden from the parser, the doc keyword alone forms the DOC_MEMBER.
    // The tree builder will attach the following block comment as trivia.
    m.complete(p, SyntaxKind::DOC_MEMBER);
}

/// Parse an `attribute name : Type = value;` member.
pub(crate) fn attribute_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ATTRIBUTE_KW);

    // name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected attribute name");
    }

    // : Type
    if p.eat(SyntaxKind::COLON) {
        type_ref(p);
    }

    // = value
    if p.eat(SyntaxKind::EQ) {
        expression(p);
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ATTRIBUTE_USAGE);
}

/// Parse a `subject name : Type;` member.
pub(crate) fn subject_member(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::SUBJECT_KW);

    // name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected subject name");
    }

    // : Type
    if p.eat(SyntaxKind::COLON) {
        type_ref(p);
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::SUBJECT_MEMBER);
}

/// Parse an expression (simple: literal, name ref, or binary).
///
/// Uses flat left-to-right parsing for binary operators. For example,
/// `a = b + c` parses as `(a = (b + c))` because `=` has lower precedence
/// than `+`. We use a simple two-level precedence scheme:
/// - Low: `=`, `<=`, `>=`, `<`, `>`
/// - High: `+`, `-`, `*`, `/`
pub(crate) fn expression(p: &mut Parser) {
    expr_low(p);
}

/// Returns true if the current token is a low-precedence binary operator.
fn at_low_op(p: &Parser) -> bool {
    matches!(
        p.current(),
        SyntaxKind::EQ | SyntaxKind::LT_EQ | SyntaxKind::GT_EQ | SyntaxKind::LT | SyntaxKind::GT
    )
}

/// Returns true if the current token is a high-precedence binary operator.
fn at_high_op(p: &Parser) -> bool {
    matches!(
        p.current(),
        SyntaxKind::PLUS | SyntaxKind::MINUS | SyntaxKind::STAR | SyntaxKind::SLASH
    )
}

/// Parse a low-precedence expression: `term (op term)*`
fn expr_low(p: &mut Parser) {
    let m = p.start();
    if !expr_high(p) {
        m.abandon(p);
        return;
    }
    if at_low_op(p) {
        p.bump_any(); // operator
        expr_high(p);
        m.complete(p, SyntaxKind::BINARY_EXPR);
    } else {
        m.complete(p, SyntaxKind::EXPRESSION);
    }
}

/// Parse a high-precedence expression: `primary (op primary)*`
fn expr_high(p: &mut Parser) -> bool {
    let m = p.start();
    if !primary_expr(p) {
        m.abandon(p);
        return false;
    }
    if at_high_op(p) {
        p.bump_any(); // operator
        if !primary_expr(p) {
            m.complete(p, SyntaxKind::BINARY_EXPR);
            return true;
        }
        // Continue for additional high-precedence ops: `a + b + c`
        let mut cm = m.complete(p, SyntaxKind::BINARY_EXPR);
        while at_high_op(p) {
            let m2 = cm.precede(p);
            p.bump_any();
            primary_expr(p);
            cm = m2.complete(p, SyntaxKind::BINARY_EXPR);
        }
        true
    } else {
        // No operator — just complete as EXPRESSION wrapper and unwrap later,
        // or abandon to avoid double-wrapping.
        m.abandon(p);
        true
    }
}

/// Parse a primary expression: literal or name reference.
fn primary_expr(p: &mut Parser) -> bool {
    match p.current() {
        SyntaxKind::INTEGER_LIT | SyntaxKind::REAL_LIT | SyntaxKind::STRING_LIT => {
            let m = p.start();
            p.bump_any();
            m.complete(p, SyntaxKind::LITERAL);
            true
        }
        SyntaxKind::IDENT => {
            name_ref(p);
            true
        }
        _ => {
            p.error("expected expression");
            false
        }
>>>>>>> feat/sysml2-parser-reqs
    }
}
