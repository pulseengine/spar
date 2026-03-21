//! Annex grammar rules.
//!
//! Annexes are parsed as opaque text blocks. The content between
//! `{**` and `**}` is captured as a single ANNEX_TEXT token.
//! Annex-specific parsers (EMV2, BA, etc.) parse the content later.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `annex Name {** ... **};` as a subclause.
///
/// AADL v2.3 also allows file-reference form: `annex Name {** file("path") **};`
pub(crate) fn annex_subclause(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ANNEX_KW);

    // Annex name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else if p.at(SyntaxKind::FILE_KW) {
        // v2.3: `file` could appear as an annex name if someone names their annex "file"
        p.bump_any();
    } else {
        p.error("expected annex name");
    }

    // {** ... **}
    if p.at(SyntaxKind::ANNEX_OPEN) {
        p.bump(SyntaxKind::ANNEX_OPEN);
        // v2.3: Check for file reference form: `file("path")`
        if p.at(SyntaxKind::FILE_KW) && p.nth(1) == SyntaxKind::L_PAREN {
            let fr = p.start();
            p.bump(SyntaxKind::FILE_KW);
            p.bump(SyntaxKind::L_PAREN);
            if p.at(SyntaxKind::STRING_LIT) {
                p.bump(SyntaxKind::STRING_LIT);
            } else {
                p.error("expected file path string");
            }
            p.expect(SyntaxKind::R_PAREN);
            fr.complete(p, SyntaxKind::FILE_REFERENCE);
        } else {
            // Everything until **} is annex text
            while !p.at(SyntaxKind::ANNEX_CLOSE) && !p.at_end() {
                p.bump_any();
            }
        }
        p.expect(SyntaxKind::ANNEX_CLOSE);
    } else {
        p.error("expected `{**`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ANNEX_SUBCLAUSE);
}

/// Parse an annex library declaration at package level.
///
/// AADL v2.3 also allows file-reference form: `annex Name {** file("path") **};`
pub(crate) fn annex_library(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ANNEX_KW);

    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else if p.at(SyntaxKind::FILE_KW) {
        p.bump_any();
    } else {
        p.error("expected annex name");
    }

    if p.at(SyntaxKind::ANNEX_OPEN) {
        p.bump(SyntaxKind::ANNEX_OPEN);
        // v2.3: Check for file reference form: `file("path")`
        if p.at(SyntaxKind::FILE_KW) && p.nth(1) == SyntaxKind::L_PAREN {
            let fr = p.start();
            p.bump(SyntaxKind::FILE_KW);
            p.bump(SyntaxKind::L_PAREN);
            if p.at(SyntaxKind::STRING_LIT) {
                p.bump(SyntaxKind::STRING_LIT);
            } else {
                p.error("expected file path string");
            }
            p.expect(SyntaxKind::R_PAREN);
            fr.complete(p, SyntaxKind::FILE_REFERENCE);
        } else {
            while !p.at(SyntaxKind::ANNEX_CLOSE) && !p.at_end() {
                p.bump_any();
            }
        }
        p.expect(SyntaxKind::ANNEX_CLOSE);
    } else {
        p.error("expected `{**`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ANNEX_LIBRARY);
}
