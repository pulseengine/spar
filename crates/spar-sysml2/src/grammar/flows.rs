//! Grammar rule for SysML v2 flow usages (REQ-SYSML2-FLOWS-MODES-001).
//!
//! The shapes are the official corpus's (`training/13. Flows`, Annex A
//! `SimpleVehicleModel`, `validation/03-Function-based Behavior`, the
//! `Interaction Sequencing Examples`):
//!
//! ```text
//! flow of Fuel from tankAssy.fuelTankPort.fuelSupply to eng.engineFuelPort.fuelSupply;
//! flow publish_request from producer.publish to server.publish;
//! flow from focus.image to shoot.image;
//! flow 'amplify torque'.transmissionTorque to 'transfer torque'.transmissionTorque;
//! flow :>> publish_message from a.p to b.q { ... }
//! succession flow lightFlow from a.p to b.q;
//! ```
//!
//! The two ends are `CONNECT_ENDPOINT`s, the same node `connect a to b;`
//! produces, so lowering reads both with one function.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// `[succession] flow [declaration] [of T] [[from] S to D] (; | { ... })`.
pub(crate) fn flow_usage(p: &mut Parser) {
    let m = p.start();
    p.eat(SyntaxKind::SUCCESSION_KW);
    // `expect`, not `bump`: `is_member_start` admits `succession` only before
    // `flow`, and a caller that did not ask it must get an error, not a panic.
    p.expect(SyntaxKind::FLOW_KW);
    if at_flow_name(p) {
        super::name(p);
    }
    super::parts::opt_typing_or_specialization(p);
    if p.at(SyntaxKind::OF_KW) {
        let o = p.start();
        p.bump(SyntaxKind::OF_KW);
        super::qualified_name(p);
        super::parts::opt_typing_or_specialization(p);
        o.complete(p, SyntaxKind::FLOW_PAYLOAD);
    }
    let has_from = p.eat(SyntaxKind::FROM_KW);
    if has_from || !(p.at(SyntaxKind::SEMICOLON) || p.at(SyntaxKind::L_CURLY)) {
        super::parts::connect_endpoint(p);
        p.expect(SyntaxKind::TO_KW);
        super::parts::connect_endpoint(p);
    }
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }
    m.complete(p, SyntaxKind::FLOW_USAGE);
}

/// A flow's own name is ONE identifier followed by what may follow a
/// declaration (`from`, a typing, `of`, or the end of the member). An
/// identifier followed by `.` or `to` is the start of the source end:
/// `flow a.p to b.q;` and `flow p to q;` are anonymous. A keyword is never a
/// flow's name (`flow from ...`, `flow of ...`); a quoted name lexes as IDENT.
fn at_flow_name(p: &Parser) -> bool {
    p.at(SyntaxKind::IDENT)
        && matches!(
            p.nth(1),
            SyntaxKind::FROM_KW
                | SyntaxKind::OF_KW
                | SyntaxKind::COLON
                | SyntaxKind::COLON_GT
                | SyntaxKind::COLON_GT_GT
                | SyntaxKind::SEMICOLON
                | SyntaxKind::L_CURLY
        )
}
