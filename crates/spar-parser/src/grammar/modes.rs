//! Mode grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `modes ...` or `requires modes ...` section.
pub(crate) fn mode_section(p: &mut Parser) {
    let m = p.start();
    // Optional `requires` prefix
    let is_requires = p.at(SyntaxKind::REQUIRES_KW);
    if is_requires {
        p.bump(SyntaxKind::REQUIRES_KW);
    }
    p.bump(SyntaxKind::MODES_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name() || at_unnamed_transition(p) {
            mode_or_transition(p);
        }
    }
    m.complete(p, SyntaxKind::MODE_SECTION);
}

/// Check if we're at an unnamed mode transition: `source -[...]-> dest ;`
fn at_unnamed_transition(p: &Parser) -> bool {
    (p.at(SyntaxKind::IDENT) || p.current().is_keyword()) && p.nth(1) == SyntaxKind::DASH_BRACKET
}

/// Parse a mode declaration or mode transition.
fn mode_or_transition(p: &mut Parser) {
    let m = p.start();

    // Check for unnamed transition: `source -[...]-> dest ;`
    if at_unnamed_transition(p) {
        p.bump_any(); // source mode name
        parse_transition_tail(p);
        m.complete(p, SyntaxKind::MODE_TRANSITION);
        return;
    }

    p.bump_any(); // name (IDENT or keyword-as-name)
    p.expect(SyntaxKind::COLON);

    if p.at(SyntaxKind::INITIAL_KW) || p.at(SyntaxKind::MODE_KW) {
        // Mode declaration: `name : [initial] mode ;`
        if p.eat(SyntaxKind::INITIAL_KW) {
            // initial mode
        }
        p.expect(SyntaxKind::MODE_KW);
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::MODE);
    } else if p.at_name() || at_unnamed_transition(p) {
        // Named transition: `name : src -[ trigger, ... ]-> dst ;`
        p.bump_any(); // source mode

        parse_transition_tail(p);
        m.complete(p, SyntaxKind::MODE_TRANSITION);
    } else {
        p.error("expected `mode` or mode transition");
        m.complete(p, SyntaxKind::ERROR);
    }
}

/// Parse the `-[ triggers ]-> dest ;` tail of a mode transition.
fn parse_transition_tail(p: &mut Parser) {
    p.expect(SyntaxKind::DASH_BRACKET);
    // Parse triggers
    let t = p.start();
    if p.at(SyntaxKind::IDENT) || p.at(SyntaxKind::SELF_KW) || p.current().is_keyword() {
        trigger_ref(p);
        while p.eat(SyntaxKind::COMMA) {
            trigger_ref(p);
        }
    }
    t.complete(p, SyntaxKind::MODE_TRIGGER);
    p.expect(SyntaxKind::BRACKET_ARROW);

    // destination mode
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
    } else {
        p.error("expected destination mode");
    }
    p.expect(SyntaxKind::SEMICOLON);
}

/// Parse a trigger reference: `port_name` or `subcomponent.port_name`.
fn trigger_ref(p: &mut Parser) {
    if p.at(SyntaxKind::SELF_KW) {
        p.bump(SyntaxKind::SELF_KW);
        if p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    } else if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
}

/// Parse `in modes (m1, m2)` or `in modes (m1 => x1, m2 => x2)` clause.
pub(crate) fn in_modes(p: &mut Parser) {
    // Don't consume `in` if it's not followed by `modes`
    if p.at(SyntaxKind::IN_KW) && p.nth(1) == SyntaxKind::MODES_KW {
        p.bump(SyntaxKind::IN_KW);
        p.bump(SyntaxKind::MODES_KW);
        p.expect(SyntaxKind::L_PAREN);
        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
            mode_or_mapping(p);
            while p.eat(SyntaxKind::COMMA) {
                mode_or_mapping(p);
            }
        }
        p.expect(SyntaxKind::R_PAREN);
    }
}

/// Parse a mode name or mode mapping: `mode` or `parent_mode => child_mode`.
fn mode_or_mapping(p: &mut Parser) {
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        // Optional mode mapping: `=> target_mode`
        if p.eat(SyntaxKind::FAT_ARROW) && (p.at(SyntaxKind::IDENT) || p.current().is_keyword()) {
            p.bump_any();
        }
    }
}
