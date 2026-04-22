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
            // Direction. AS-5506B §8.1 grammar:
            //   feature_direction ::= in | out | in out
            // Only the `in out` combination is legal — `out in`, `in in`,
            // and `out out` are not. The HIR lowering normalizes unknown
            // direction text to `None`, so these used to parse silently
            // and leave downstream direction_rules to skip the feature;
            // reject at the syntax layer for better locality of diagnosis
            // and spec-conformance.
            let d = p.start();
            let first_was_in = p.at(SyntaxKind::IN_KW);
            p.bump_any(); // in or out
            // Only `in` may be followed by `out` to form `in out`.
            if first_was_in && p.at(SyntaxKind::OUT_KW) {
                p.bump(SyntaxKind::OUT_KW);
            } else if p.at(SyntaxKind::IN_KW) || p.at(SyntaxKind::OUT_KW) {
                p.error("feature direction must be `in`, `out`, or `in out` (AS-5506B §8.1)");
                p.bump_any(); // consume the offending keyword so the parser recovers
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
                        // Abstract feature: optionally with classifier(...) or plain ref
                        abstract_feature_classifier(p);
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
                // Abstract feature: optionally with classifier(...) or plain ref
                abstract_feature_classifier(p);
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

/// Parse the optional classifier part of an abstract feature.
///
/// AADL v2.3 allows `classifier(Pkg::Type)` syntax after the `feature` keyword,
/// in addition to the v2.2 plain classifier reference.
fn abstract_feature_classifier(p: &mut Parser) {
    if p.at(SyntaxKind::CLASSIFIER_KW) && p.nth(1) == SyntaxKind::L_PAREN {
        // v2.3 form: `classifier(Pkg::Type)`
        let m = p.start();
        p.bump(SyntaxKind::CLASSIFIER_KW);
        p.bump(SyntaxKind::L_PAREN);
        super::classifier_ref(p);
        p.expect(SyntaxKind::R_PAREN);
        m.complete(p, SyntaxKind::CLASSIFIER_VALUE);
    } else if p.at(SyntaxKind::IDENT) || (p.current().is_keyword() && !is_section_kw(p)) {
        super::classifier_ref(p);
    }
}

/// Returns true if the current token is a section keyword that should not be
/// consumed as a classifier name.
fn is_section_kw(p: &Parser) -> bool {
    matches!(
        p.current(),
        SyntaxKind::END_KW
            | SyntaxKind::FEATURES_KW
            | SyntaxKind::FLOWS_KW
            | SyntaxKind::CONNECTIONS_KW
            | SyntaxKind::MODES_KW
            | SyntaxKind::PROPERTIES_KW
            | SyntaxKind::SUBCOMPONENTS_KW
            | SyntaxKind::ANNEX_KW
            | SyntaxKind::CALLS_KW
    )
}

fn eat_to_semicolon(p: &mut Parser) {
    while !p.at(SyntaxKind::SEMICOLON) && !p.at_end() {
        p.bump_any();
    }
    if p.at(SyntaxKind::SEMICOLON) {
        p.bump(SyntaxKind::SEMICOLON);
    }
}
