//! Requirement relationship grammar rules: satisfy, verify, refine, allocate, derive.
//!
//! SysML v2 relationship syntax:
//! ```sysml
//! satisfy sensorLatency by ecu.controller;
//! verify sensorLatency by latencyTest;
//! refine sensorLatency by detailedLatencyReq;
//! allocate sensorProcessing to ecu;
//! derive detailedReq from highLevelReq;
//! ```

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse a `satisfy req by impl;` relationship.
pub(crate) fn satisfy_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::SATISFY_KW);

    // Requirement reference
    name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::SATISFY_REQ);
}

/// Parse a `verify req by test;` relationship.
pub(crate) fn verify_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::VERIFY_KW);

    // Requirement reference
    name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::VERIFY_REQ);
}

/// Parse a `refine req1 by req2;` relationship.
pub(crate) fn refine_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::REFINE_KW);

    // Source requirement reference
    name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::REFINE_REQ);
}

/// Parse an `allocate source to target;` relationship.
pub(crate) fn allocate_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ALLOCATE_KW);

    // Source reference
    name_ref(p);

    // `to` target
    if p.eat(SyntaxKind::TO_KW) {
        name_ref(p);
    } else {
        p.error("expected `to`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ALLOCATE_REQ);
}

/// Parse a `derive source from target;` relationship.
pub(crate) fn derive_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::DERIVE_KW);

    // Source reference
    name_ref(p);

    // `from` target
    if p.eat(SyntaxKind::FROM_KW) {
        name_ref(p);
    } else {
        p.error("expected `from`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::DERIVE_REQ);
}

/// Parse a dotted name reference: `name` or `name.name.name`.
fn name_ref(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) && !p.current().is_keyword() {
        p.error("expected name");
        return false;
    }
    let m = p.start();
    super::bump_as_ident(p);
    while p.at(SyntaxKind::DOT) {
        p.bump(SyntaxKind::DOT);
        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
            super::bump_as_ident(p);
        } else {
            p.error("expected identifier after `.`");
            break;
        }
    }
    // Also handle `::` qualified paths
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
            super::bump_as_ident(p);
        } else {
            p.error("expected identifier after `::`");
            break;
        }
    }
    m.complete(p, SyntaxKind::NAME_REF);
    true
}
