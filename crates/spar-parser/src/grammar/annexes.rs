//! Annex grammar rules.
//!
//! Annexes are parsed as opaque text blocks. The content between
//! `{**` and `**}` is captured as a single ANNEX_TEXT token.
//! Annex-specific parsers (EMV2, BA, etc.) parse the content later.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// Parse `annex Name {** ... **};` as a subclause.
pub(crate) fn annex_subclause(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ANNEX_KW);

    // Annex name
    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected annex name");
    }

    // {** ... **}
    if p.at(SyntaxKind::ANNEX_OPEN) {
        p.bump(SyntaxKind::ANNEX_OPEN);
        // Everything until **} is annex text
        while !p.at(SyntaxKind::ANNEX_CLOSE) && !p.at_end() {
            p.bump_any();
        }
        p.expect(SyntaxKind::ANNEX_CLOSE);
    } else {
        p.error("expected `{**`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ANNEX_SUBCLAUSE);
}

/// Parse an annex library declaration at package level.
pub(crate) fn annex_library(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::ANNEX_KW);

    if p.at(SyntaxKind::IDENT) {
        p.bump(SyntaxKind::IDENT);
    } else {
        p.error("expected annex name");
    }

    if p.at(SyntaxKind::ANNEX_OPEN) {
        p.bump(SyntaxKind::ANNEX_OPEN);
        while !p.at(SyntaxKind::ANNEX_CLOSE) && !p.at_end() {
            p.bump_any();
        }
        p.expect(SyntaxKind::ANNEX_CLOSE);
    } else {
        p.error("expected `{**`");
    }

    p.expect(SyntaxKind::SEMICOLON);
    m.complete(p, SyntaxKind::ANNEX_LIBRARY);
}
