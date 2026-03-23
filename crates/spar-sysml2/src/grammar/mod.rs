//! SysML v2 grammar rules.
//!
//! Each function corresponds to a grammar production from the SysML v2 spec.
//! Functions take a `&mut Parser` and build nodes via markers.

mod packages;
mod parts;

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse a complete SysML v2 source file.
///
/// ```text
/// SourceFile = NamespaceMember*
/// NamespaceMember = Package | ImportDecl | Definition | Usage
/// ```
pub fn source_file(p: &mut Parser) {
    let m = p.start();
    while !p.at_end() {
        match p.current() {
            SyntaxKind::PACKAGE_KW => packages::package(p),
            SyntaxKind::IMPORT_KW => packages::import_decl(p),
            k if is_member_start(k, p) => parts::member(p),
            SyntaxKind::EOF => break,
            _ => {
                p.err_and_bump("expected package, import, or definition");
            }
        }
    }
    m.complete(p, SyntaxKind::SOURCE_FILE);
}

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
        p.error("expected name");
        return false;
    }
    let m = p.start();
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
        } else {
            p.error("expected identifier after `.`");
            break;
        }
    }
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
    }
}
