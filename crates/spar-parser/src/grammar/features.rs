//! Feature grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `features ... ` section.
pub(crate) fn feature_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::FEATURES_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at_name()
            || p.at(SyntaxKind::IN_KW)
            || p.at(SyntaxKind::OUT_KW)
            || p.at(SyntaxKind::PROVIDES_KW)
            || p.at(SyntaxKind::REQUIRES_KW)
        {
            feature(p);
        }
    }
    m.complete(p, SyntaxKind::FEATURE_SECTION);
}

/// Parse a single feature declaration.
fn feature(p: &mut Parser) {
    // Features can start with:
    // name : [direction] port_type ...
    // name : provides/requires access_type ...
    // name : feature group ...
    // name : feature ;

    // First, try to read the name
    if !p.at(SyntaxKind::IDENT)
        && !p.at(SyntaxKind::IN_KW)
        && !p.at(SyntaxKind::OUT_KW)
        && !p.at(SyntaxKind::PROVIDES_KW)
        && !p.at(SyntaxKind::REQUIRES_KW)
    {
        p.err_and_bump("expected feature declaration");
        return;
    }

    let m = p.start();

    // Name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else if p.at_name() {
        p.bump_any(); // keyword-as-name
    }
    p.expect(SyntaxKind::COLON);

    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }

    // Determine feature type
    match p.current() {
        SyntaxKind::IN_KW | SyntaxKind::OUT_KW => {
            // Direction
            let d = p.start();
            p.bump_any(); // in or out
            if p.at(SyntaxKind::OUT_KW) || p.at(SyntaxKind::IN_KW) {
                p.bump_any(); // second part of `in out`
            }
            d.complete(p, SyntaxKind::DIRECTION);

            // Now determine port type
            match p.current() {
                SyntaxKind::DATA_KW => {
                    p.bump(SyntaxKind::DATA_KW);
                    p.expect(SyntaxKind::PORT_KW);
                    // Optional type reference
                    if p.at(SyntaxKind::IDENT) {
                        super::classifier_ref(p);
                    }
                    opt_property_block_and_semi(p);
                    m.complete(p, SyntaxKind::DATA_PORT);
                }
                SyntaxKind::EVENT_KW => {
                    p.bump(SyntaxKind::EVENT_KW);
                    if p.at(SyntaxKind::DATA_KW) {
                        // event data port
                        p.bump(SyntaxKind::DATA_KW);
                        p.expect(SyntaxKind::PORT_KW);
                        if p.at(SyntaxKind::IDENT) {
                            super::classifier_ref(p);
                        }
                        opt_property_block_and_semi(p);
                        m.complete(p, SyntaxKind::EVENT_DATA_PORT);
                    } else {
                        // event port
                        p.expect(SyntaxKind::PORT_KW);
                        opt_property_block_and_semi(p);
                        m.complete(p, SyntaxKind::EVENT_PORT);
                    }
                }
                SyntaxKind::PARAMETER_KW => {
                    p.bump(SyntaxKind::PARAMETER_KW);
                    if p.at(SyntaxKind::IDENT) {
                        super::classifier_ref(p);
                    }
                    opt_property_block_and_semi(p);
                    m.complete(p, SyntaxKind::PARAMETER);
                }
                SyntaxKind::FEATURE_KW => {
                    p.bump(SyntaxKind::FEATURE_KW);
                    if p.at(SyntaxKind::GROUP_KW) {
                        p.bump(SyntaxKind::GROUP_KW);
                        feature_group_ref(p);
                        opt_property_block_and_semi(p);
                        m.complete(p, SyntaxKind::FEATURE_GROUP);
                    } else {
                        // Abstract feature, optionally with classifier reference
                        if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                            super::classifier_ref(p);
                        }
                        opt_property_block_and_semi(p);
                        m.complete(p, SyntaxKind::ABSTRACT_FEATURE);
                    }
                }
                _ => {
                    p.error("expected port type after direction");
                    opt_property_block_and_semi(p);
                    m.complete(p, SyntaxKind::ERROR);
                }
            }
        }
        SyntaxKind::DATA_KW => {
            p.bump(SyntaxKind::DATA_KW);
            p.expect(SyntaxKind::PORT_KW);
            if p.at(SyntaxKind::IDENT) {
                super::classifier_ref(p);
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::DATA_PORT);
        }
        SyntaxKind::EVENT_KW => {
            p.bump(SyntaxKind::EVENT_KW);
            if p.at(SyntaxKind::DATA_KW) {
                p.bump(SyntaxKind::DATA_KW);
                p.expect(SyntaxKind::PORT_KW);
                if p.at(SyntaxKind::IDENT) {
                    super::classifier_ref(p);
                }
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::EVENT_DATA_PORT);
            } else {
                p.expect(SyntaxKind::PORT_KW);
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::EVENT_PORT);
            }
        }
        SyntaxKind::PROVIDES_KW | SyntaxKind::REQUIRES_KW => {
            p.bump_any(); // provides or requires
            access_feature(p, m);
        }
        SyntaxKind::FEATURE_KW => {
            p.bump(SyntaxKind::FEATURE_KW);
            if p.at(SyntaxKind::GROUP_KW) {
                p.bump(SyntaxKind::GROUP_KW);
                feature_group_ref(p);
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::FEATURE_GROUP);
            } else {
                // Abstract feature, optionally with classifier reference
                if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                    super::classifier_ref(p);
                }
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::ABSTRACT_FEATURE);
            }
        }
        SyntaxKind::PORT_KW => {
            // bare `port` — data port without direction
            p.bump(SyntaxKind::PORT_KW);
            if p.at(SyntaxKind::IDENT) {
                super::classifier_ref(p);
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::DATA_PORT);
        }
        SyntaxKind::IDENT if p.at_contextual_kw("prototype") => {
            // Feature bound to a prototype: `af1 : prototype fproto1;`
            p.bump(SyntaxKind::IDENT); // "prototype"
            if p.at(SyntaxKind::IDENT) {
                p.bump(SyntaxKind::IDENT); // prototype name
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::ABSTRACT_FEATURE);
        }
        _ => {
            p.error("expected feature type");
            eat_to_semicolon(p);
            m.complete(p, SyntaxKind::ERROR);
        }
    }
}

fn access_feature(p: &mut Parser, m: crate::marker::Marker) {
    match p.current() {
        SyntaxKind::DATA_KW => {
            p.bump(SyntaxKind::DATA_KW);
            p.expect(SyntaxKind::ACCESS_KW);
            if p.at(SyntaxKind::IDENT) {
                super::classifier_ref(p);
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::DATA_ACCESS);
        }
        SyntaxKind::BUS_KW => {
            p.bump(SyntaxKind::BUS_KW);
            p.expect(SyntaxKind::ACCESS_KW);
            if p.at(SyntaxKind::IDENT) {
                super::classifier_ref(p);
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::BUS_ACCESS);
        }
        SyntaxKind::SUBPROGRAM_KW => {
            p.bump(SyntaxKind::SUBPROGRAM_KW);
            if p.at(SyntaxKind::GROUP_KW) {
                p.bump(SyntaxKind::GROUP_KW);
                p.expect(SyntaxKind::ACCESS_KW);
                if p.at(SyntaxKind::IDENT) {
                    super::classifier_ref(p);
                }
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::SUBPROGRAM_GROUP_ACCESS);
            } else {
                p.expect(SyntaxKind::ACCESS_KW);
                if p.at(SyntaxKind::IDENT) {
                    super::classifier_ref(p);
                }
                opt_property_block_and_semi(p);
                m.complete(p, SyntaxKind::SUBPROGRAM_ACCESS);
            }
        }
        SyntaxKind::VIRTUAL_KW if p.nth(1) == SyntaxKind::BUS_KW => {
            p.bump(SyntaxKind::VIRTUAL_KW);
            p.bump(SyntaxKind::BUS_KW);
            p.expect(SyntaxKind::ACCESS_KW);
            if p.at(SyntaxKind::IDENT) {
                super::classifier_ref(p);
            }
            opt_property_block_and_semi(p);
            m.complete(p, SyntaxKind::BUS_ACCESS);
        }
        _ => {
            p.error("expected access type after `provides`/`requires`");
            eat_to_semicolon(p);
            m.complete(p, SyntaxKind::ERROR);
        }
    }
}

/// Parse an optional feature group type reference, handling `inverse of Ref`.
fn feature_group_ref(p: &mut Parser) {
    if p.at(SyntaxKind::INVERSE_KW) {
        p.bump(SyntaxKind::INVERSE_KW);
        p.expect(SyntaxKind::OF_KW);
        super::classifier_ref(p);
    } else if p.at(SyntaxKind::IDENT) {
        super::classifier_ref(p);
    }
}

fn opt_property_block_and_semi(p: &mut Parser) {
    // Optional array dimensions
    while p.at(SyntaxKind::L_BRACKET) {
        super::components::array_dimension(p);
    }
    if p.at(SyntaxKind::L_CURLY) {
        super::properties::property_block(p);
    }
    p.expect(SyntaxKind::SEMICOLON);
}

fn eat_to_semicolon(p: &mut Parser) {
    while !p.at(SyntaxKind::SEMICOLON) && !p.at_end() {
        p.bump_any();
    }
    if p.at(SyntaxKind::SEMICOLON) {
        p.bump(SyntaxKind::SEMICOLON);
    }
}
