//! AADL v2.2 grammar rules.
//!
//! Each function corresponds to a grammar production from AS5506D.
//! Functions take a `&mut Parser` and build nodes via markers.

mod packages;
mod components;
mod features;
mod connections;
mod flows;
mod modes;
mod properties;
mod annexes;

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse a complete AADL source file.
///
/// ```aadl
/// SourceFile = ModelUnit*
/// ModelUnit = AadlPackage | PropertySet
/// ```
pub fn source_file(p: &mut Parser) {
    let m = p.start();
    while !p.at_end() {
        match p.current() {
            SyntaxKind::PACKAGE_KW => packages::aadl_package(p),
            SyntaxKind::PROPERTY_KW => {
                // `property set Name is ...`
                if p.nth(1) == SyntaxKind::SET_KW {
                    properties::property_set(p);
                } else {
                    p.err_and_bump("expected `package` or `property set`");
                }
            }
            SyntaxKind::EOF => break,
            _ => {
                p.err_and_bump("expected `package` or `property set`");
            }
        }
    }
    m.complete(p, SyntaxKind::SOURCE_FILE);
}

/// Parse a component category: `system`, `thread group`, `virtual processor`, etc.
///
/// Returns true if a category was parsed.
pub(crate) fn component_category(p: &mut Parser) -> bool {
    if !p.current().is_component_category_kw() {
        return false;
    }
    let m = p.start();
    match p.current() {
        SyntaxKind::VIRTUAL_KW => {
            p.bump(SyntaxKind::VIRTUAL_KW);
            match p.current() {
                SyntaxKind::BUS_KW => p.bump(SyntaxKind::BUS_KW),
                SyntaxKind::PROCESSOR_KW => p.bump(SyntaxKind::PROCESSOR_KW),
                _ => p.error("expected `bus` or `processor` after `virtual`"),
            }
        }
        SyntaxKind::THREAD_KW => {
            p.bump(SyntaxKind::THREAD_KW);
            if p.at(SyntaxKind::GROUP_KW) {
                p.bump(SyntaxKind::GROUP_KW);
            }
        }
        SyntaxKind::SUBPROGRAM_KW => {
            p.bump(SyntaxKind::SUBPROGRAM_KW);
            if p.at(SyntaxKind::GROUP_KW) {
                p.bump(SyntaxKind::GROUP_KW);
            }
        }
        kind if kind.is_component_category_kw() => {
            p.bump_any();
        }
        _ => unreachable!(),
    }
    m.complete(p, SyntaxKind::COMPONENT_CATEGORY);
    true
}

/// Parse a classifier reference: `Pkg::Type` or `Pkg::Type.Impl`
///
/// In AADL, classifier references can start with component category keywords
/// (e.g., `classifier(processor)` or `reference(bus)`), so we accept both
/// IDENT and component-category keywords.
pub(crate) fn classifier_ref(p: &mut Parser) -> bool {
    if !at_ident_or_kw(p) {
        return false;
    }
    let m = p.start();
    bump_as_ident(p);
    // Optional `::Name` parts
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if at_ident_or_kw(p) {
            bump_as_ident(p);
        } else {
            p.error("expected identifier after `::`");
        }
    }
    // Optional `.Impl`
    if p.at(SyntaxKind::DOT) {
        p.bump(SyntaxKind::DOT);
        if at_ident_or_kw(p) {
            bump_as_ident(p);
        } else {
            p.error("expected implementation name after `.`");
        }
    }
    // Optional prototype bindings: `(name => Category ClassifierRef, ...)`
    if p.at(SyntaxKind::L_PAREN) {
        prototype_binding_list(p);
    }
    m.complete(p, SyntaxKind::CLASSIFIER_REF);
    true
}

/// Parse a prototype binding list: `(name => Category ClassifierRef, ...)`
pub(crate) fn prototype_binding_list(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_PAREN);
    if at_ident_or_kw(p) {
        prototype_binding(p);
        while p.eat(SyntaxKind::COMMA) {
            prototype_binding(p);
        }
    }
    p.expect(SyntaxKind::R_PAREN);
    m.complete(p, SyntaxKind::PROTOTYPE_BINDING_LIST);
}

/// Parse a single prototype binding.
///
/// Covers component, feature, and feature group prototype bindings:
/// - `name => Category ClassifierRef? PrototypeBindings?` (component)
/// - `name => Direction? PortCategory port ClassifierRef?` (port feature)
/// - `name => Direction? data access ClassifierRef?` (access feature)
/// - `name => feature ClassifierRef?` (abstract feature)
/// - `name => feature group ClassifierRef? PrototypeBindings?` (feature group)
fn prototype_binding(p: &mut Parser) {
    let m = p.start();
    bump_as_ident(p); // prototype formal name
    p.expect(SyntaxKind::FAT_ARROW);

    // Optional direction for feature prototype bindings:
    // `in`, `out`, `in out`, `provides`, `requires`
    if p.at(SyntaxKind::IN_KW) || p.at(SyntaxKind::OUT_KW)
        || p.at(SyntaxKind::PROVIDES_KW) || p.at(SyntaxKind::REQUIRES_KW)
    {
        let d = p.start();
        if p.at(SyntaxKind::IN_KW) {
            p.bump(SyntaxKind::IN_KW);
            if p.at(SyntaxKind::OUT_KW) {
                p.bump(SyntaxKind::OUT_KW);
            }
        } else if p.at(SyntaxKind::OUT_KW) {
            p.bump(SyntaxKind::OUT_KW);
        } else {
            // provides/requires
            p.bump_any();
        }
        d.complete(p, SyntaxKind::DIRECTION);
    }

    // Feature prototype bindings may have port/access category after direction
    if p.at(SyntaxKind::DATA_KW) && p.nth(1) == SyntaxKind::PORT_KW {
        p.bump(SyntaxKind::DATA_KW);
        p.bump(SyntaxKind::PORT_KW);
    } else if p.at(SyntaxKind::EVENT_KW) && p.nth(1) == SyntaxKind::DATA_KW {
        p.bump(SyntaxKind::EVENT_KW);
        p.bump(SyntaxKind::DATA_KW);
        p.eat(SyntaxKind::PORT_KW);
    } else if p.at(SyntaxKind::EVENT_KW) && p.nth(1) == SyntaxKind::PORT_KW {
        p.bump(SyntaxKind::EVENT_KW);
        p.bump(SyntaxKind::PORT_KW);
    } else if p.at(SyntaxKind::EVENT_KW) {
        p.bump(SyntaxKind::EVENT_KW);
    } else if p.at(SyntaxKind::DATA_KW) && p.nth(1) == SyntaxKind::ACCESS_KW {
        p.bump(SyntaxKind::DATA_KW);
        p.bump(SyntaxKind::ACCESS_KW);
    } else if p.at(SyntaxKind::BUS_KW) && p.nth(1) == SyntaxKind::ACCESS_KW {
        p.bump(SyntaxKind::BUS_KW);
        p.bump(SyntaxKind::ACCESS_KW);
    } else if p.at(SyntaxKind::FEATURE_KW) {
        // `feature` or `feature group`
        p.bump(SyntaxKind::FEATURE_KW);
        if p.at(SyntaxKind::GROUP_KW) {
            p.bump(SyntaxKind::GROUP_KW);
        }
    } else if p.current().is_component_category_kw() {
        // Component category (system, data, abstract, etc.)
        component_category(p);
    } else if p.at(SyntaxKind::PORT_KW) {
        // bare `port`
        p.bump(SyntaxKind::PORT_KW);
    } else if p.at(SyntaxKind::ACCESS_KW) {
        // bare `access`
        p.bump(SyntaxKind::ACCESS_KW);
    }

    // Optional classifier reference (which itself may have nested bindings)
    if at_ident_or_kw(p) && !p.at(SyntaxKind::R_PAREN) && !p.at(SyntaxKind::COMMA) {
        classifier_ref(p);
    }
    m.complete(p, SyntaxKind::PROTOTYPE_BINDING);
}

/// Returns true if the current token is an identifier or a keyword that can
/// appear as an identifier in classifier/name contexts.
///
/// AADL is case-insensitive and identifiers can clash with keywords
/// (e.g., `Types::Compute` where `compute` is a keyword). We accept
/// any keyword as a potential identifier in name positions.
fn at_ident_or_kw(p: &Parser) -> bool {
    p.at(SyntaxKind::IDENT) || p.current().is_keyword()
}

/// Bump the current token, treating component-category keywords as IDENT.
fn bump_as_ident(p: &mut Parser) {
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        // Bump the keyword but emit it — the tree builder will use the
        // original token text regardless of what kind we record.
        p.bump_any();
    }
}

/// Parse a name (simple identifier or dotted path for package names).
pub(crate) fn name(p: &mut Parser) -> bool {
    if !p.at(SyntaxKind::IDENT) {
        p.error("expected name");
        return false;
    }
    let m = p.start();
    p.bump(SyntaxKind::IDENT);
    while p.at(SyntaxKind::COLON_COLON) {
        p.bump(SyntaxKind::COLON_COLON);
        if p.at(SyntaxKind::IDENT) {
            p.bump(SyntaxKind::IDENT);
        } else {
            p.error("expected identifier after `::`");
            break;
        }
    }
    m.complete(p, SyntaxKind::NAME);
    true
}

/// Parse a dotted package name (e.g., `My_Package::Sub`).
pub(crate) fn package_name(p: &mut Parser) -> bool {
    name(p)
}
