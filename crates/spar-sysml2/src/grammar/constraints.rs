//! Constraint definition and usage grammar rules.
//!
//! SysML v2 constraint syntax:
//! ```sysml
//! constraint def TimingBudget {
//!     attribute totalLatency : Real;
//!     attribute bound : Real;
//!     totalLatency <= bound;
//! }
//!
//! constraint timingCheck : TimingBudget {
//!     totalLatency = sensorDelay + processingTime;
//!     bound = 20.0;
//! }
//! ```

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// Recovery set for constraint body members.
const CONSTRAINT_BODY_RECOVERY: TokenSet = TokenSet::new(&[
    SyntaxKind::ATTRIBUTE_KW,
    SyntaxKind::DOC_KW,
    SyntaxKind::R_CURLY,
    SyntaxKind::IDENT,
]);

/// Parse a constraint definition or usage.
///
/// ```sysml
/// ConstraintDef = 'constraint' 'def' Name '{' Body '}'
/// ConstraintUsage = 'constraint' Name ':' Type '{' Body '}'
/// ```
pub(crate) fn constraint(p: &mut Parser) {
    if p.nth(1) == SyntaxKind::DEF_KW {
        constraint_def(p);
    } else {
        constraint_usage(p);
    }
}

/// Parse a constraint definition: `constraint def Name { body }`
fn constraint_def(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CONSTRAINT_KW);
    p.bump(SyntaxKind::DEF_KW);

    // Name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected constraint definition name");
    }

    // Body block
    if p.at(SyntaxKind::L_CURLY) {
        constraint_body(p);
    }

    m.complete(p, SyntaxKind::CONSTRAINT_DEF);
}

/// Parse a constraint usage: `constraint name : Type { body }` or
/// `constraint name : Type ;`
fn constraint_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CONSTRAINT_KW);

    // Name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected constraint name");
    }

    // Optional : Type
    if p.eat(SyntaxKind::COLON) {
        super::type_ref(p);
    }

    // Body block or semicolon
    if p.at(SyntaxKind::L_CURLY) {
        constraint_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::CONSTRAINT_USAGE);
}

/// Parse a constraint body: `{ members... }`
///
/// Members can be:
/// - `attribute name : Type;`
/// - `attribute name : Type = value;`
/// - `doc /* text */`
/// - constraint expressions: `a <= b;`, `a = expr;`
pub(crate) fn constraint_body(p: &mut Parser) {
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
            SyntaxKind::IDENT
            | SyntaxKind::INTEGER_LIT
            | SyntaxKind::REAL_LIT
            | SyntaxKind::STRING_LIT => {
                // Constraint expression: `a <= b;` or `a = expr;`
                constraint_expr_stmt(p);
            }
            _ => {
                p.err_recover(
                    "expected `attribute`, expression, or `}`",
                    CONSTRAINT_BODY_RECOVERY,
                );
            }
        }
    }

    p.expect(SyntaxKind::R_CURLY);
    m.complete(p, SyntaxKind::BODY_BLOCK);
}

/// Parse a constraint expression statement: `expr;`
///
/// Examples:
/// - `totalLatency <= bound;`
/// - `totalLatency = sensorDelay + processingTime;`
/// - `bound = 20.0;`
fn constraint_expr_stmt(p: &mut Parser) {
    super::expression(p);
    p.expect(SyntaxKind::SEMICOLON);
}
