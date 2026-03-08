//! Flow grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `flows ...` section in a component type (flow specifications).
pub(crate) fn flow_spec_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::FLOWS_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name() {
            flow_spec(p);
        }
    }
    m.complete(p, SyntaxKind::FLOW_SPEC_SECTION);
}

/// Parse `flows ...` section in a component implementation (flow implementations + e2e flows).
pub(crate) fn flow_impl_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::FLOWS_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name() {
            // Peek to see if this is a flow impl or end-to-end flow
            // Both start with `name :` so look further
            flow_impl_or_e2e(p);
        }
    }
    m.complete(p, SyntaxKind::FLOW_IMPL_SECTION);
}

/// Parse a flow specification: `name : flow source/sink/path ...`
fn flow_spec(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // name (IDENT or keyword-as-name)
    p.expect(SyntaxKind::COLON);

    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }

    p.expect(SyntaxKind::FLOW_KW);

    // flow kind
    let k = p.start();
    match p.current() {
        SyntaxKind::SOURCE_KW => p.bump(SyntaxKind::SOURCE_KW),
        SyntaxKind::SINK_KW => p.bump(SyntaxKind::SINK_KW),
        SyntaxKind::PATH_KW => p.bump(SyntaxKind::PATH_KW),
        _ => p.error("expected `source`, `sink`, or `path`"),
    }
    k.complete(p, SyntaxKind::FLOW_KIND);

    // Flow endpoints (optional for refined flows)
    flow_end(p);
    if p.eat(SyntaxKind::ARROW) {
        flow_end(p);
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
    m.complete(p, SyntaxKind::FLOW_SPEC);
}

/// Parse a flow implementation or end-to-end flow.
fn flow_impl_or_e2e(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // name (IDENT or keyword-as-name)
    p.expect(SyntaxKind::COLON);

    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }

    if p.at(SyntaxKind::END_KW) && p.nth(1) == SyntaxKind::TO_KW {
        // end to end flow
        p.bump(SyntaxKind::END_KW);
        p.expect(SyntaxKind::TO_KW);
        p.expect(SyntaxKind::END_KW);
        p.expect(SyntaxKind::FLOW_KW);

        // segments: element -> element -> ... (optional for refined flows)
        if at_flow_segment(p) {
            flow_segment(p);
            while p.eat(SyntaxKind::ARROW) {
                flow_segment(p);
            }
        }

        if p.at(SyntaxKind::L_CURLY) {
            super::properties::property_block(p);
        }
        if p.at(SyntaxKind::IN_KW) {
            super::modes::in_modes(p);
        }
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::END_TO_END_FLOW);
    } else {
        // flow implementation
        p.expect(SyntaxKind::FLOW_KW);

        let k = p.start();
        match p.current() {
            SyntaxKind::SOURCE_KW => p.bump(SyntaxKind::SOURCE_KW),
            SyntaxKind::SINK_KW => p.bump(SyntaxKind::SINK_KW),
            SyntaxKind::PATH_KW => p.bump(SyntaxKind::PATH_KW),
            _ => p.error("expected flow kind"),
        }
        k.complete(p, SyntaxKind::FLOW_KIND);

        // segments (optional for refined flows)
        if at_flow_segment(p) {
            flow_segment(p);
            while p.eat(SyntaxKind::ARROW) {
                flow_segment(p);
            }
        }

        if p.at(SyntaxKind::L_CURLY) {
            super::properties::property_block(p);
        }
        if p.at(SyntaxKind::IN_KW) {
            super::modes::in_modes(p);
        }
        p.expect(SyntaxKind::SEMICOLON);
        m.complete(p, SyntaxKind::FLOW_IMPL);
    }
}

/// Parse a flow endpoint: `feature_name` or `subcomponent.feature`.
fn flow_end(p: &mut Parser) {
    let m = p.start();
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    m.complete(p, SyntaxKind::FLOW_END);
}

/// Check if we're at a flow segment (not at `in modes`, `{`, or `;`).
fn at_flow_segment(p: &Parser) -> bool {
    if p.at(SyntaxKind::IDENT) {
        return true;
    }
    // Keywords can be segment names, but `in` followed by `modes` is not a segment
    if p.current().is_keyword() {
        if p.at(SyntaxKind::IN_KW) && p.nth(1) == SyntaxKind::MODES_KW {
            return false;
        }
        // Other section/terminator keywords are not segments
        if matches!(
            p.current(),
            SyntaxKind::END_KW | SyntaxKind::SEMICOLON | SyntaxKind::L_CURLY
        ) {
            return false;
        }
        return true;
    }
    false
}

/// Parse a flow segment: `subcomponent` or `connection_name` or `subcomponent.flow_spec`.
fn flow_segment(p: &mut Parser) {
    let m = p.start();
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        while p.at(SyntaxKind::DOT) {
            p.bump(SyntaxKind::DOT);
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    m.complete(p, SyntaxKind::FLOW_SEGMENT);
}
