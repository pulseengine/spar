//! Package-level grammar rules.
//!
//! ```aadl
//! AadlPackage =
//!   'package' Name
//!   PublicSection?
//!   PrivateSection?
//!   PackageProperties?
//!   'end' Name ';'
//! ```

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;
use super::{component_category, name, properties};

const SECTION_RECOVERY: TokenSet = TokenSet::new(&[
    SyntaxKind::PUBLIC_KW,
    SyntaxKind::PRIVATE_KW,
    SyntaxKind::PROPERTIES_KW,
    SyntaxKind::END_KW,
    SyntaxKind::PACKAGE_KW,
]);

pub(crate) fn aadl_package(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PACKAGE_KW);
    name(p);

    // Optional public section
    if p.at(SyntaxKind::PUBLIC_KW) {
        public_section(p);
    }

    // Optional private section
    if p.at(SyntaxKind::PRIVATE_KW) {
        private_section(p);
    }

    // Optional package-level properties
    if p.at(SyntaxKind::PROPERTIES_KW) {
        package_properties(p);
    }

    // `end Name ;`
    p.expect(SyntaxKind::END_KW);
    name(p);
    p.expect(SyntaxKind::SEMICOLON);

    m.complete(p, SyntaxKind::AADL_PACKAGE);
}

fn public_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PUBLIC_KW);
    section_body(p);
    m.complete(p, SyntaxKind::PUBLIC_SECTION);
}

fn private_section(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PRIVATE_KW);
    section_body(p);
    m.complete(p, SyntaxKind::PRIVATE_SECTION);
}

fn section_body(p: &mut Parser) {
    // with clauses, renames, and declarations can be interleaved
    loop {
        match p.current() {
            SyntaxKind::WITH_KW => {
                with_clause(p);
            }
            SyntaxKind::RENAMES_KW => {
                renames_all_clause(p);
            }
            // Component type or implementation
            k if k.is_component_category_kw() => {
                super::components::classifier_decl(p);
            }
            // Feature group type
            SyntaxKind::FEATURE_KW => {
                if p.nth(1) == SyntaxKind::GROUP_KW {
                    super::components::feature_group_type_decl(p);
                } else {
                    break;
                }
            }
            // Annex library
            SyntaxKind::ANNEX_KW => {
                super::annexes::annex_library(p);
            }
            // Named renames: `alias renames ...`
            SyntaxKind::IDENT if p.nth(1) == SyntaxKind::RENAMES_KW => {
                renames_clause(p);
            }
            _ => break,
        }
    }
}

fn with_clause(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::WITH_KW);
    // name list
    name(p);
    while p.eat(SyntaxKind::COMMA) {
        name(p);
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::WITH_CLAUSE);
}

fn renames_clause(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IDENT); // the alias name
    p.bump(SyntaxKind::RENAMES_KW);
    // The rest of the renames clause
    if component_category(p) {
        // `renames Category Pkg::Name;`
        super::classifier_ref(p);
    } else if p.at(SyntaxKind::PACKAGE_KW) {
        // `renames package Pkg::Name;`
        p.bump(SyntaxKind::PACKAGE_KW);
        name(p);
    } else if p.at(SyntaxKind::FEATURE_KW) && p.nth(1) == SyntaxKind::GROUP_KW {
        // `renames feature group Pkg::FG;`
        p.bump(SyntaxKind::FEATURE_KW);
        p.bump(SyntaxKind::GROUP_KW);
        super::classifier_ref(p);
    } else {
        p.error("expected component category or `package` after `renames`");
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::RENAMES_CLAUSE);
}

/// Parse unnamed renames: `renames Pkg::all;` or `renames Category Classifier;`
/// or `renames package Pkg;` or `renames feature group Pkg::FG;`
fn renames_all_clause(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::RENAMES_KW);

    // Check for component category: `renames abstract pack5::a6;`
    if p.current().is_component_category_kw() {
        super::component_category(p);
        super::classifier_ref(p);
    } else if p.at(SyntaxKind::FEATURE_KW) && p.nth(1) == SyntaxKind::GROUP_KW {
        // `renames feature group Pkg::FG;`
        p.bump(SyntaxKind::FEATURE_KW);
        p.bump(SyntaxKind::GROUP_KW);
        super::classifier_ref(p);
    } else if p.at(SyntaxKind::PACKAGE_KW) {
        // `renames package Pkg;`
        p.bump(SyntaxKind::PACKAGE_KW);
        super::name(p);
    } else if p.at(SyntaxKind::IDENT) || p.current().is_keyword() {
        // `renames Pkg::all;` or `renames Pkg::Name;`
        p.bump_any();
        while p.at(SyntaxKind::COLON_COLON) {
            p.bump(SyntaxKind::COLON_COLON);
            if p.at(SyntaxKind::IDENT) || p.at(SyntaxKind::ALL_KW) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::RENAMES_CLAUSE);
}

fn package_properties(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PROPERTIES_KW);
    if p.at(SyntaxKind::NONE_KW) {
        p.bump(SyntaxKind::NONE_KW);
        p.expect(SyntaxKind::SEMICOLON);
    } else {
        while !p.at(SyntaxKind::END_KW) && !p.at_end() {
            if p.at(SyntaxKind::IDENT) {
                properties::property_association(p);
            } else {
                break;
            }
        }
    }
    m.complete(p, SyntaxKind::PACKAGE_PROPERTIES);
}
