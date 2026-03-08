//! Connection grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `connections ...` section.
pub(crate) fn connection_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CONNECTIONS_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name() {
            connection(p);
        }
    }
    m.complete(p, SyntaxKind::CONNECTION_SECTION);
}

/// Parse a single connection.
fn connection(p: &mut Parser) {
    let m = p.start();
    // name : [refined to] connection_kind src -> dst { properties } [in modes] ;
    p.bump_any(); // name (IDENT or keyword-as-name)
    p.expect(SyntaxKind::COLON);

    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }

    // Connection kind keyword
    let kind = match p.current() {
        SyntaxKind::PORT_KW => {
            p.bump(SyntaxKind::PORT_KW);
            SyntaxKind::PORT_CONNECTION
        }
        SyntaxKind::ACCESS_KW => {
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        SyntaxKind::FEATURE_KW => {
            p.bump(SyntaxKind::FEATURE_KW);
            if p.at(SyntaxKind::GROUP_KW) {
                p.bump(SyntaxKind::GROUP_KW);
                SyntaxKind::FEATURE_GROUP_CONNECTION
            } else {
                SyntaxKind::FEATURE_CONNECTION
            }
        }
        SyntaxKind::PARAMETER_KW => {
            p.bump(SyntaxKind::PARAMETER_KW);
            SyntaxKind::PARAMETER_CONNECTION
        }
        SyntaxKind::DATA_KW if p.nth(1) == SyntaxKind::ACCESS_KW => {
            p.bump(SyntaxKind::DATA_KW);
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        SyntaxKind::BUS_KW if p.nth(1) == SyntaxKind::ACCESS_KW => {
            p.bump(SyntaxKind::BUS_KW);
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        SyntaxKind::SUBPROGRAM_KW if p.nth(1) == SyntaxKind::ACCESS_KW => {
            p.bump(SyntaxKind::SUBPROGRAM_KW);
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        SyntaxKind::SUBPROGRAM_KW
            if p.nth(1) == SyntaxKind::GROUP_KW && p.nth(2) == SyntaxKind::ACCESS_KW =>
        {
            p.bump(SyntaxKind::SUBPROGRAM_KW);
            p.bump(SyntaxKind::GROUP_KW);
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        SyntaxKind::VIRTUAL_KW
            if p.nth(1) == SyntaxKind::BUS_KW && p.nth(2) == SyntaxKind::ACCESS_KW =>
        {
            p.bump(SyntaxKind::VIRTUAL_KW);
            p.bump(SyntaxKind::BUS_KW);
            p.bump(SyntaxKind::ACCESS_KW);
            SyntaxKind::ACCESS_CONNECTION
        }
        _ => {
            p.error("expected connection kind (`port`, `access`, `feature`, `parameter`)");
            // Try to recover by parsing as generic connection
            SyntaxKind::PORT_CONNECTION
        }
    };

    // Source and destination elements (optional for refined connections)
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() || p.at(SyntaxKind::SELF_KW) {
        connected_element(p);

        // `->` or `<->`
        if !p.eat(SyntaxKind::ARROW) && !p.eat(SyntaxKind::BIDI_ARROW) {
            p.error("expected `->` or `<->` in connection");
        }

        // Destination element
        connected_element(p);
    }

    // Optional property block
    if p.at(SyntaxKind::L_CURLY) {
        super::properties::property_block(p);
    }

    // Optional in modes
    if p.at(SyntaxKind::IN_KW) {
        super::modes::in_modes(p);
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, kind);
}

/// Parse a connected element reference: `subcomponent.port` or just `port`.
fn connected_element(p: &mut Parser) {
    let m = p.start();
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            } else {
                p.error("expected feature name after `.`");
            }
        }
    } else if p.at(SyntaxKind::SELF_KW) {
        p.bump(SyntaxKind::SELF_KW);
        if p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    } else {
        p.error("expected connection endpoint");
    }
    m.complete(p, SyntaxKind::CONNECTED_ELEMENT);
}
