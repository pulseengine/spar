//! Requirement definition, usage, satisfy, verify, and refine grammar rules.
//!
//! SysML v2 requirement syntax:
//! ```sysml
//! requirement def LatencyReq {
//!     doc /* Sensor-to-actuator latency < 20ms */
//!     attribute maxLatency : Real = 20.0;
//! }
//!
//! requirement sensorLatency : LatencyReq {
//!     subject sensor : SensorSubsystem;
//! }
//!
//! satisfy sensorLatency by ecu.controller;
//! verify sensorLatency by latencyTest;
//! refine sensorLatency by detailedLatencyReq;
//! ```

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// Recovery set for requirement body members.
const REQ_BODY_RECOVERY: TokenSet = TokenSet::new(&[
    SyntaxKind::ATTRIBUTE_KW,
    SyntaxKind::SUBJECT_KW,
    SyntaxKind::DOC_KW,
    SyntaxKind::R_CURLY,
]);

/// Parse a requirement definition or usage.
///
/// ```sysml
/// RequirementDef = 'requirement' 'def' Name '{' Body '}'
/// RequirementUsage = 'requirement' Name ':' Type '{' Body '}'
/// RequirementUsage = 'requirement' Name ':' Type ';'
/// RequirementUsage = 'requirement' Name '{' Body '}'
/// ```
pub(crate) fn requirement(p: &mut Parser) {
    if p.nth(1) == SyntaxKind::DEF_KW {
        requirement_def(p);
    } else {
        requirement_usage(p);
    }
}

/// Parse a requirement definition: `requirement def Name { body }`
fn requirement_def(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::REQUIREMENT_KW);
    p.bump(SyntaxKind::DEF_KW);

    // Name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected requirement definition name");
    }

    // Body block
    if p.at(SyntaxKind::L_CURLY) {
        requirement_body(p);
    }

    m.complete(p, SyntaxKind::REQUIREMENT_DEF);
}

/// Parse a requirement usage: `requirement name : Type { body }` or
/// `requirement name : Type ;`
fn requirement_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::REQUIREMENT_KW);

    // Name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected requirement name");
    }

    // Optional : Type
    if p.eat(SyntaxKind::COLON) {
        super::type_ref(p);
    }

    // Body block or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        requirement_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::REQUIREMENT_USAGE);
}

/// Parse a requirement body: `{ members... }`
fn requirement_body(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_CURLY);

    while !p.at(SyntaxKind::R_CURLY) && !p.at_end() {
        match p.current() {
            SyntaxKind::DOC_KW => {
                super::doc_member(p);
            }
            SyntaxKind::ATTRIBUTE_KW => {
                super::attribute_usage(p);
            }
            SyntaxKind::SUBJECT_KW => {
                super::subject_member(p);
            }
            SyntaxKind::REQUIREMENT_KW => {
                // Nested requirement
                requirement(p);
            }
            SyntaxKind::ASSUME_KW | SyntaxKind::REQUIRE_KW => {
                // assume/require constraint
                assume_or_require(p);
            }
            _ => {
                p.err_recover(
                    "expected `doc`, `attribute`, `subject`, or `}`",
                    REQ_BODY_RECOVERY,
                );
            }
        }
    }

    p.expect(SyntaxKind::R_CURLY);
    m.complete(p, SyntaxKind::BODY_BLOCK);
}

/// Parse `assume constraint ...` or `require constraint ...` members.
fn assume_or_require(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // assume or require
    if p.eat(SyntaxKind::CONSTRAINT_KW) {
        // Optional name
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        }
        // Optional : Type
        if p.eat(SyntaxKind::COLON) {
            super::type_ref(p);
        }
        // Body or semicolon
        if p.at(SyntaxKind::L_CURLY) {
            super::constraints::constraint_body(p);
        } else {
            p.expect(SyntaxKind::SEMICOLON);
        }
    } else {
        p.error("expected `constraint` after assume/require");
    }
    m.complete(p, SyntaxKind::CONSTRAINT_USAGE);
}

/// Parse a `satisfy req by impl;` relationship.
///
/// ```sysml
/// SatisfyReq = 'satisfy' NameRef 'by' NameRef ';'
/// ```
pub(crate) fn satisfy_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::SATISFY_KW);

    // Requirement reference
    super::name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        super::name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::SATISFY_REQ);
}

/// Parse a `verify req by test;` relationship.
///
/// ```sysml
/// VerifyReq = 'verify' NameRef 'by' NameRef ';'
/// ```
pub(crate) fn verify_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::VERIFY_KW);

    // Requirement reference
    super::name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        super::name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::VERIFY_REQ);
}

/// Parse a `refine req1 by req2;` relationship.
///
/// ```sysml
/// RefineReq = 'refine' NameRef 'by' NameRef ';'
/// ```
pub(crate) fn refine_req(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::REFINE_KW);

    // Source requirement reference
    super::name_ref(p);

    // `by` target
    if p.eat(SyntaxKind::BY_KW) {
        super::name_ref(p);
    } else {
        p.error("expected `by`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::REFINE_REQ);
}
