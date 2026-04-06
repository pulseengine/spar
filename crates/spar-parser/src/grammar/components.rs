//! Component type and implementation grammar rules.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

#[allow(dead_code)]
const COMPONENT_SECTIONS: TokenSet = TokenSet::new(&[
    SyntaxKind::PROTOTYPES_KW,
    SyntaxKind::FEATURES_KW,
    SyntaxKind::FLOWS_KW,
    SyntaxKind::CONNECTIONS_KW,
    SyntaxKind::MODES_KW,
    SyntaxKind::PROPERTIES_KW,
    SyntaxKind::SUBCOMPONENTS_KW,
    SyntaxKind::CALLS_KW,
    SyntaxKind::INTERNAL_KW,
    SyntaxKind::ANNEX_KW,
    SyntaxKind::END_KW,
]);

/// Parse a classifier declaration (component type or implementation).
pub(crate) fn classifier_decl(p: &mut Parser) {
    // Lookahead: is it `category implementation` or just `category Name`?
    // Need to skip multi-word categories: `virtual processor`, `thread group`, etc.
    if is_implementation_ahead(p) {
        component_impl(p);
    } else {
        component_type(p);
    }
}

/// Check if this is a component implementation by looking ahead past the category.
fn is_implementation_ahead(p: &mut Parser) -> bool {
    let mut lookahead = 0;
    // Skip category keywords
    match p.nth(lookahead) {
        SyntaxKind::VIRTUAL_KW => {
            lookahead += 1; // virtual
            lookahead += 1; // bus/processor
        }
        SyntaxKind::THREAD_KW | SyntaxKind::SUBPROGRAM_KW => {
            lookahead += 1;
            if p.nth(lookahead) == SyntaxKind::GROUP_KW {
                lookahead += 1;
            }
        }
        k if k.is_component_category_kw() => {
            lookahead += 1;
        }
        _ => return false,
    }
    p.nth(lookahead) == SyntaxKind::IMPLEMENTATION_KW
}

/// ```aadl
/// ComponentType = Category Name TypeExtension?
///   PrototypeSection? FeatureSection? FlowSpecSection?
///   ModeSection? PropertySection? AnnexSubclause*
///   'end' Name ';'
/// ```
fn component_type(p: &mut Parser) {
    let m = p.start();
    super::component_category(p);
    if p.at_name() {
        p.bump_any();
    } else {
        p.error("expected component type name");
    }

    // Optional extends
    if p.at(SyntaxKind::EXTENDS_KW) {
        type_extension(p);
    }

    // Sections (order-independent in practice, but the grammar defines an order)
    component_type_sections(p);

    // end Name ;
    p.expect(SyntaxKind::END_KW);
    if p.at_name() {
        p.bump_any();
    }
    p.expect(SyntaxKind::SEMICOLON);

    m.complete(p, SyntaxKind::COMPONENT_TYPE);
}

/// ```aadl
/// ComponentImpl = Category 'implementation' TypeName '.' ImplName
///   ImplExtension? PrototypeSection? SubcomponentSection?
///   ConnectionSection? FlowImplSection? ModeSection?
///   PropertySection? AnnexSubclause*
///   'end' QualifiedName ';'
/// ```
fn component_impl(p: &mut Parser) {
    let m = p.start();
    super::component_category(p);
    p.expect(SyntaxKind::IMPLEMENTATION_KW);

    // Realization: TypeName
    let r = p.start();
    if p.at_name() {
        p.bump_any();
        // Handle qualified type names like Pkg::TypeName
        while p.at(SyntaxKind::COLON_COLON) {
            p.bump(SyntaxKind::COLON_COLON);
            if p.at_name() {
                p.bump_any();
            }
        }
    }
    r.complete(p, SyntaxKind::REALIZATION);

    // `.ImplName`
    p.expect(SyntaxKind::DOT);
    if p.at_name() {
        p.bump_any();
    } else {
        p.error("expected implementation name");
    }

    // Optional prototype bindings on the impl itself: `a1.i (proto => data D)`
    if p.at(SyntaxKind::L_PAREN) {
        super::prototype_binding_list(p);
    }

    // Optional extends
    if p.at(SyntaxKind::EXTENDS_KW) {
        impl_extension(p);
    }

    // Sections
    component_impl_sections(p);

    // end QualifiedName ;
    p.expect(SyntaxKind::END_KW);
    // Eat the qualified name tokens: TypeName.ImplName (names may be keywords)
    while p.at_name() || p.at(SyntaxKind::DOT) || p.at(SyntaxKind::COLON_COLON) {
        p.bump_any();
    }
    p.expect(SyntaxKind::SEMICOLON);

    m.complete(p, SyntaxKind::COMPONENT_IMPL);
}

fn type_extension(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::EXTENDS_KW);
    super::classifier_ref(p);
    m.complete(p, SyntaxKind::TYPE_EXTENSION);
}

fn impl_extension(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::EXTENDS_KW);
    super::classifier_ref(p);
    m.complete(p, SyntaxKind::IMPL_EXTENSION);
}

fn component_type_sections(p: &mut Parser) {
    loop {
        match p.current() {
            SyntaxKind::PROTOTYPES_KW => prototype_section(p),
            SyntaxKind::FEATURES_KW => super::features::feature_section(p),
            SyntaxKind::FLOWS_KW => super::flows::flow_spec_section(p),
            SyntaxKind::MODES_KW => super::modes::mode_section(p),
            SyntaxKind::REQUIRES_KW if p.nth(1) == SyntaxKind::MODES_KW => {
                super::modes::mode_section(p);
            }
            SyntaxKind::PROPERTIES_KW => super::properties::property_section(p),
            SyntaxKind::ANNEX_KW => super::annexes::annex_subclause(p),
            _ => break,
        }
    }
}

fn component_impl_sections(p: &mut Parser) {
    loop {
        match p.current() {
            SyntaxKind::PROTOTYPES_KW => prototype_section(p),
            SyntaxKind::SUBCOMPONENTS_KW => subcomponent_section(p),
            SyntaxKind::INTERNAL_KW => internal_features_section(p),
            SyntaxKind::PROCESSOR_KW if p.nth(1) == SyntaxKind::FEATURES_KW => {
                processor_features_section(p);
            }
            SyntaxKind::CONNECTIONS_KW => super::connections::connection_section(p),
            SyntaxKind::CALLS_KW => call_section(p),
            SyntaxKind::FLOWS_KW => super::flows::flow_impl_section(p),
            SyntaxKind::MODES_KW => super::modes::mode_section(p),
            SyntaxKind::REQUIRES_KW if p.nth(1) == SyntaxKind::MODES_KW => {
                super::modes::mode_section(p);
            }
            SyntaxKind::PROPERTIES_KW => super::properties::property_section(p),
            SyntaxKind::ANNEX_KW => super::annexes::annex_subclause(p),
            _ => break,
        }
    }
}

fn prototype_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PROTOTYPES_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at(SyntaxKind::IDENT) {
            prototype(p);
        }
    }
    m.complete(p, SyntaxKind::PROTOTYPE_SECTION);
}

fn prototype(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IDENT); // name
    p.expect(SyntaxKind::COLON);

    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }

    // Optional direction for feature prototypes: `in`, `out`, `in out`
    if p.at(SyntaxKind::IN_KW) || p.at(SyntaxKind::OUT_KW) {
        let d = p.start();
        if p.at(SyntaxKind::IN_KW) {
            p.bump(SyntaxKind::IN_KW);
            if p.at(SyntaxKind::OUT_KW) {
                p.bump(SyntaxKind::OUT_KW);
            }
        } else {
            p.bump(SyntaxKind::OUT_KW);
        }
        d.complete(p, SyntaxKind::DIRECTION);
    }

    // prototype kind: component category or `feature` etc.
    if p.current().is_component_category_kw() {
        super::component_category(p);
    } else if p.at(SyntaxKind::FEATURE_KW) {
        p.bump(SyntaxKind::FEATURE_KW);
        if p.at(SyntaxKind::GROUP_KW) {
            p.bump(SyntaxKind::GROUP_KW);
        }
    } else {
        p.error("expected component category or `feature`");
    }
    // Optional classifier ref
    if p.at(SyntaxKind::IDENT) {
        super::classifier_ref(p);
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::PROTOTYPE);
}

fn subcomponent_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::SUBCOMPONENTS_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at(SyntaxKind::IDENT) || p.current().is_component_category_kw() {
            subcomponent(p);
        }
    }
    m.complete(p, SyntaxKind::SUBCOMPONENT_SECTION);
}

fn subcomponent(p: &mut Parser) {
    let m = p.start();
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT); // name
    }
    p.expect(SyntaxKind::COLON);
    // Optional `refined to`
    if p.at(SyntaxKind::REFINED_KW) {
        let r = p.start();
        p.bump(SyntaxKind::REFINED_KW);
        p.expect(SyntaxKind::TO_KW);
        r.complete(p, SyntaxKind::REFINED_TO);
    }
    // component category
    super::component_category(p);
    // Optional classifier reference
    if p.at(SyntaxKind::IDENT) {
        super::classifier_ref(p);
    }
    // Optional array dimensions
    while p.at(SyntaxKind::L_BRACKET) {
        array_dimension(p);
    }
    // Optional implementation reference list for array subcomponents:
    // `bus b[2] (b.i1, b.i2);`
    if p.at(SyntaxKind::L_PAREN) {
        impl_reference_list(p);
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
    m.complete(p, SyntaxKind::SUBCOMPONENT);
}

/// Parse an implementation reference list: `(Impl1, Impl2, ...)`
fn impl_reference_list(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_PAREN);
    if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        super::classifier_ref(p);
        while p.eat(SyntaxKind::COMMA) {
            if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
                super::classifier_ref(p);
            }
        }
    }
    p.expect(SyntaxKind::R_PAREN);
    m.complete(p, SyntaxKind::LIST_VALUE); // reuse LIST_VALUE node kind
}

pub(crate) fn array_dimension(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_BRACKET);
    if !p.at(SyntaxKind::R_BRACKET) {
        // size expression
        let s = p.start();
        if p.at(SyntaxKind::INTEGER_LIT) {
            p.bump(SyntaxKind::INTEGER_LIT);
        } else if p.at(SyntaxKind::IDENT) {
            super::classifier_ref(p);
        }
        s.complete(p, SyntaxKind::ARRAY_SIZE);
    }
    p.expect(SyntaxKind::R_BRACKET);
    m.complete(p, SyntaxKind::ARRAY_DIMENSION);
}

fn call_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::CALLS_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while p.at(SyntaxKind::IDENT) {
            call_sequence(p);
        }
    }
    m.complete(p, SyntaxKind::CALL_SECTION);
}

fn call_sequence(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IDENT); // sequence name
    p.expect(SyntaxKind::COLON);
    p.expect(SyntaxKind::L_CURLY);
    while p.at(SyntaxKind::IDENT) {
        subprogram_call(p);
    }
    p.expect(SyntaxKind::R_CURLY);
    // Optional `in modes`
    if p.at(SyntaxKind::IN_KW) {
        super::modes::in_modes(p);
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::CALL_SEQUENCE);
}

fn subprogram_call(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IDENT); // call name
    p.expect(SyntaxKind::COLON);
    p.expect(SyntaxKind::SUBPROGRAM_KW);
    if p.at(SyntaxKind::IDENT) {
        super::classifier_ref(p);
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::SUBPROGRAM_CALL);
}

fn internal_features_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::INTERNAL_KW);
    p.expect(SyntaxKind::FEATURES_KW);
    while p.at(SyntaxKind::IDENT) {
        internal_or_processor_feature(p);
    }
    m.complete(p, SyntaxKind::INTERNAL_FEATURES_SECTION);
}

fn processor_features_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PROCESSOR_KW);
    p.expect(SyntaxKind::FEATURES_KW);
    while p.at(SyntaxKind::IDENT) {
        internal_or_processor_feature(p);
    }
    m.complete(p, SyntaxKind::PROCESSOR_FEATURES_SECTION);
}

/// Parse a single internal or processor feature:
/// `name : event source;` or `name : event data source Type;`
/// or `name : port Type;` or `name : subprogram Type;`
fn internal_or_processor_feature(p: &mut Parser) {
    let f = p.start();
    p.bump(SyntaxKind::IDENT);
    p.expect(SyntaxKind::COLON);
    if p.at(SyntaxKind::EVENT_KW) {
        p.bump(SyntaxKind::EVENT_KW);
        if p.at(SyntaxKind::DATA_KW) {
            p.bump(SyntaxKind::DATA_KW);
            // `event data source Type;` or just `event data;` (as port proxy)
            if p.at(SyntaxKind::SOURCE_KW) {
                p.bump(SyntaxKind::SOURCE_KW);
                if p.at(SyntaxKind::IDENT) {
                    super::classifier_ref(p);
                }
                p.expect(SyntaxKind::SEMICOLON);
                f.complete(p, SyntaxKind::EVENT_DATA_SOURCE);
            } else {
                // event data port (no source keyword)
                if p.at(SyntaxKind::IDENT) {
                    super::classifier_ref(p);
                }
                p.expect(SyntaxKind::SEMICOLON);
                f.complete(p, SyntaxKind::EVENT_DATA_SOURCE);
            }
        } else if p.at(SyntaxKind::SOURCE_KW) {
            p.bump(SyntaxKind::SOURCE_KW);
            p.expect(SyntaxKind::SEMICOLON);
            f.complete(p, SyntaxKind::EVENT_SOURCE);
        } else {
            // bare `event;`
            p.expect(SyntaxKind::SEMICOLON);
            f.complete(p, SyntaxKind::EVENT_SOURCE);
        }
    } else if p.at(SyntaxKind::PORT_KW) {
        // processor feature: `name : port Type;`
        p.bump(SyntaxKind::PORT_KW);
        if p.at(SyntaxKind::IDENT) {
            super::classifier_ref(p);
        }
        p.expect(SyntaxKind::SEMICOLON);
        f.complete(p, SyntaxKind::PORT_PROXY);
    } else if p.at(SyntaxKind::SUBPROGRAM_KW) {
        // processor feature: `name : subprogram Type;`
        p.bump(SyntaxKind::SUBPROGRAM_KW);
        if p.at(SyntaxKind::IDENT) {
            super::classifier_ref(p);
        }
        p.expect(SyntaxKind::SEMICOLON);
        f.complete(p, SyntaxKind::SUBPROGRAM_PROXY);
    } else {
        p.err_and_bump("expected `event`, `port`, or `subprogram`");
        f.abandon(p);
    }
}

/// Parse a feature group type declaration.
pub(crate) fn feature_group_type_decl(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::FEATURE_KW);
    p.expect(SyntaxKind::GROUP_KW);
    if p.at_name() {
        p.bump_any();
    } else {
        p.error("expected feature group type name");
    }

    // Optional extends
    if p.at(SyntaxKind::EXTENDS_KW) {
        type_extension(p);
    }

    // Optional prototypes section
    if p.at(SyntaxKind::PROTOTYPES_KW) {
        prototype_section(p);
    }

    // features section
    if p.at(SyntaxKind::FEATURES_KW) {
        super::features::feature_section(p);
    }

    // Optional inverse of
    if p.at(SyntaxKind::INVERSE_KW) {
        p.bump(SyntaxKind::INVERSE_KW);
        p.expect(SyntaxKind::OF_KW);
        super::classifier_ref(p);
    }

    // properties
    if p.at(SyntaxKind::PROPERTIES_KW) {
        super::properties::property_section(p);
    }

    // annex subclauses
    while p.at(SyntaxKind::ANNEX_KW) {
        super::annexes::annex_subclause(p);
    }

    p.expect(SyntaxKind::END_KW);
    if p.at_name() {
        p.bump_any();
    }
    p.expect(SyntaxKind::SEMICOLON);

    m.complete(p, SyntaxKind::FEATURE_GROUP_TYPE);
}
