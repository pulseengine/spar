//! Package-level grammar rules for SysML v2.
//!
//! ```text
//! Package = 'package' QualifiedName '{' NamespaceMember* '}'
//!         | 'package' QualifiedName ';'
//! ImportDecl = 'import' QualifiedName ('::' '*')? ';'
//! ```

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse a package declaration.
///
/// ```text
/// package Pkg {
///     import ScalarValues::*;
///     part def Vehicle { }
/// }
/// ```
pub(crate) fn package(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::PACKAGE_KW);
    super::qualified_name(p);

    if p.at(SyntaxKind::L_CURLY) {
        namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }

    m.complete(p, SyntaxKind::PACKAGE);
}

/// Parse an import declaration.
///
/// ```text
/// import ScalarValues::*;
/// import Pkg::SubPkg::Name;
/// ```
pub(crate) fn import_decl(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::IMPORT_KW);
    super::qualified_name(p);
    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::IMPORT_DECL);
}

/// Parse a namespace body: `{ member* }`
pub(crate) fn namespace_body(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::L_CURLY);

    while !p.at(SyntaxKind::R_CURLY) && !p.at_end() {
        match p.current() {
            SyntaxKind::PACKAGE_KW => package(p),
            SyntaxKind::IMPORT_KW => import_decl(p),
            k if super::is_member_start(k, p) => super::parts::member(p),
            _ => {
                p.err_and_bump("expected member declaration");
            }
        }
    }

    p.expect(SyntaxKind::R_CURLY);
    m.complete(p, SyntaxKind::NAMESPACE_BODY);
}
