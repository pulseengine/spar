//! Lexer for SysML v2 source text.
//!
//! Produces a flat sequence of `(SyntaxKind, &str)` token pairs from an input
//! string. The lexer is a simple cursor-based implementation following the
//! same pattern as the AADL lexer in spar-parser.
//!
//! SysML v2 is case-sensitive for keywords: `package`, `part`, `def` must be
//! lowercase. Identifiers preserve their original case.

use crate::syntax_kind::SyntaxKind;

// ---------------------------------------------------------------------------
// Cursor -- zero-copy byte-level scanner
// ---------------------------------------------------------------------------

/// A simple cursor over a `&str`, tracking position by byte offset.
struct Cursor<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    /// Peek at the current byte without consuming it. Returns `\0` at EOF.
    #[inline]
    fn current(&self) -> u8 {
        if self.pos < self.input.len() {
            self.input.as_bytes()[self.pos]
        } else {
            0
        }
    }

    /// Peek at the byte `n` positions ahead. Returns `\0` past EOF.
    #[inline]
    fn peek(&self, n: usize) -> u8 {
        let idx = self.pos + n;
        if idx < self.input.len() {
            self.input.as_bytes()[idx]
        } else {
            0
        }
    }

    /// Returns `true` when the cursor is at the end of input.
    #[inline]
    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Advance by one byte.
    #[inline]
    fn bump(&mut self) {
        self.pos += 1;
    }

    /// Advance by one full UTF-8 character (1-4 bytes).
    #[inline]
    fn bump_char(&mut self) {
        if self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            self.pos += ch.len_utf8();
        }
    }

    /// Advance by `n` bytes.
    #[inline]
    fn bump_n(&mut self, n: usize) {
        self.pos += n;
    }
}

// ---------------------------------------------------------------------------
// Token scanning helpers
// ---------------------------------------------------------------------------

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// Scan whitespace (one or more whitespace characters).
fn scan_whitespace(c: &mut Cursor<'_>) {
    while !c.is_eof() && is_whitespace(c.current()) {
        c.bump();
    }
}

/// Scan a line comment starting at `//`, consuming through end-of-line.
fn scan_line_comment(c: &mut Cursor<'_>) {
    c.bump_n(2); // skip `//`
    while !c.is_eof() && c.current() != b'\n' {
        c.bump();
    }
}

/// Scan a block comment starting at `/*`, consuming through `*/`.
fn scan_block_comment(c: &mut Cursor<'_>) {
    c.bump_n(2); // skip `/*`
    let mut depth = 1u32;
    while !c.is_eof() && depth > 0 {
        if c.current() == b'*' && c.peek(1) == b'/' {
            depth -= 1;
            c.bump_n(2);
        } else if c.current() == b'/' && c.peek(1) == b'*' {
            depth += 1;
            c.bump_n(2);
        } else {
            c.bump();
        }
    }
}

/// Scan a string literal starting at `"`.
fn scan_string(c: &mut Cursor<'_>) {
    c.bump(); // skip opening `"`
    while !c.is_eof() {
        match c.current() {
            b'"' => {
                c.bump();
                return;
            }
            b'\\' => {
                // Skip escaped character
                c.bump();
                if !c.is_eof() {
                    c.bump();
                }
            }
            _ => c.bump(),
        }
    }
    // unterminated string -- still produce STRING_LIT for error recovery
}

/// Scan a numeric literal (integer or real).
fn scan_number(c: &mut Cursor<'_>) -> SyntaxKind {
    while !c.is_eof() && is_digit(c.current()) {
        c.bump();
    }

    if !c.is_eof() && c.current() == b'.' && c.peek(1) != b'.' {
        // Real literal
        c.bump(); // skip `.`
        while !c.is_eof() && is_digit(c.current()) {
            c.bump();
        }
        // Optional exponent
        if !c.is_eof() && (c.current() == b'e' || c.current() == b'E') {
            c.bump();
            if !c.is_eof() && (c.current() == b'+' || c.current() == b'-') {
                c.bump();
            }
            while !c.is_eof() && is_digit(c.current()) {
                c.bump();
            }
        }
        SyntaxKind::REAL_LIT
    } else if !c.is_eof() && (c.current() == b'e' || c.current() == b'E') {
        // Real with exponent only
        c.bump();
        if !c.is_eof() && (c.current() == b'+' || c.current() == b'-') {
            c.bump();
        }
        while !c.is_eof() && is_digit(c.current()) {
            c.bump();
        }
        SyntaxKind::REAL_LIT
    } else {
        SyntaxKind::INTEGER_LIT
    }
}

/// Scan an identifier or keyword.
fn scan_ident_or_keyword(c: &mut Cursor<'_>, start: usize) -> SyntaxKind {
    while !c.is_eof() && is_ident_continue(c.current()) {
        c.bump();
    }
    let text = &c.input[start..c.pos];
    // SysML v2 is case-sensitive for keywords
    SyntaxKind::from_keyword(text).unwrap_or(SyntaxKind::IDENT)
}

// ---------------------------------------------------------------------------
// Main lexer entry point -- scan one token
// ---------------------------------------------------------------------------

/// Scan the next token from the cursor position.
fn scan_token<'a>(c: &mut Cursor<'a>) -> (SyntaxKind, &'a str) {
    let start = c.pos;

    let kind = match c.current() {
        // Whitespace
        b if is_whitespace(b) => {
            scan_whitespace(c);
            SyntaxKind::WHITESPACE
        }

        // Line comment `//`
        b'/' if c.peek(1) == b'/' => {
            scan_line_comment(c);
            SyntaxKind::LINE_COMMENT
        }

        // Block comment `/* ... */`
        b'/' if c.peek(1) == b'*' => {
            scan_block_comment(c);
            SyntaxKind::BLOCK_COMMENT
        }

        // String literal
        b'"' => {
            scan_string(c);
            SyntaxKind::STRING_LIT
        }

        // Numeric literal
        b if is_digit(b) => scan_number(c),

        // Identifier or keyword
        b if is_ident_start(b) => {
            c.bump();
            scan_ident_or_keyword(c, start)
        }

        // Multi-character punctuation -- order matters for longest match.

        // `:>>` colon-greater-greater
        b':' if c.peek(1) == b'>' && c.peek(2) == b'>' => {
            c.bump_n(3);
            SyntaxKind::COLON_GT_GT
        }

        // `:>` colon-greater (specialization)
        b':' if c.peek(1) == b'>' => {
            c.bump_n(2);
            SyntaxKind::COLON_GT
        }

        // `::` colon-colon (namespace separator)
        b':' if c.peek(1) == b':' => {
            c.bump_n(2);
            SyntaxKind::COLON_COLON
        }

        // `=>` fat arrow
        b'=' if c.peek(1) == b'>' => {
            c.bump_n(2);
            SyntaxKind::FAT_ARROW
        }

        // `==` equality
        b'=' if c.peek(1) == b'=' => {
            c.bump_n(2);
            SyntaxKind::EQ_EQ
        }

        // `!=` not-equal
        b'!' if c.peek(1) == b'=' => {
            c.bump_n(2);
            SyntaxKind::BANG_EQ
        }

        // `>=` greater-equal
        b'>' if c.peek(1) == b'=' => {
            c.bump_n(2);
            SyntaxKind::GT_EQ
        }

        // `<=` less-equal
        b'<' if c.peek(1) == b'=' => {
            c.bump_n(2);
            SyntaxKind::LT_EQ
        }

        // `~>` tilde-arrow (conjugation)
        b'~' if c.peek(1) == b'>' => {
            c.bump_n(2);
            SyntaxKind::TILDE_GT
        }

        // `..` dot-dot (range)
        b'.' if c.peek(1) == b'.' => {
            c.bump_n(2);
            SyntaxKind::DOT_DOT
        }

        // Single-character punctuation
        b';' => {
            c.bump();
            SyntaxKind::SEMICOLON
        }
        b':' => {
            c.bump();
            SyntaxKind::COLON
        }
        b',' => {
            c.bump();
            SyntaxKind::COMMA
        }
        b'.' => {
            c.bump();
            SyntaxKind::DOT
        }
        b'(' => {
            c.bump();
            SyntaxKind::L_PAREN
        }
        b')' => {
            c.bump();
            SyntaxKind::R_PAREN
        }
        b'{' => {
            c.bump();
            SyntaxKind::L_CURLY
        }
        b'}' => {
            c.bump();
            SyntaxKind::R_CURLY
        }
        b'[' => {
            c.bump();
            SyntaxKind::L_BRACKET
        }
        b']' => {
            c.bump();
            SyntaxKind::R_BRACKET
        }
        b'=' => {
            c.bump();
            SyntaxKind::EQ
        }
        b'*' => {
            c.bump();
            SyntaxKind::STAR
        }
        b'+' => {
            c.bump();
            SyntaxKind::PLUS
        }
        b'-' => {
            c.bump();
            SyntaxKind::MINUS
        }
        b'/' => {
            c.bump();
            SyntaxKind::SLASH
        }
        b'>' => {
            c.bump();
            SyntaxKind::GT
        }
        b'<' => {
            c.bump();
            SyntaxKind::LT
        }
        b'#' => {
            c.bump();
            SyntaxKind::HASH
        }
        b'@' => {
            c.bump();
            SyntaxKind::AT
        }

        // Unknown / error character -- advance a full UTF-8 character
        _ => {
            c.bump_char();
            SyntaxKind::ERROR
        }
    };

    let text = &c.input[start..c.pos];
    (kind, text)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Tokenize a SysML v2 source string into `(SyntaxKind, &str)` pairs.
///
/// Every byte of the input is covered by exactly one token -- whitespace,
/// comments, and errors are all represented.
pub fn lex(input: &str) -> Vec<(SyntaxKind, &str)> {
    let mut c = Cursor::new(input);
    let mut tokens = Vec::new();
    while !c.is_eof() {
        tokens.push(scan_token(&mut c));
    }
    tokens
}

/// Tokenize a SysML v2 source string into `(SyntaxKind, len)` pairs.
///
/// This is the allocation-free variant suitable for feeding into a parser:
/// each entry records the token kind and the byte length of the token text.
pub fn tokenize(input: &str) -> Vec<(SyntaxKind, usize)> {
    let mut c = Cursor::new(input);
    let mut tokens = Vec::new();
    while !c.is_eof() {
        let start = c.pos;
        let (kind, _) = scan_token(&mut c);
        tokens.push((kind, c.pos - start));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_empty() {
        assert_eq!(lex(""), vec![]);
    }

    #[test]
    fn lex_keywords() {
        let tokens = lex("package part def port connect");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::PACKAGE_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::PART_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::DEF_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::PORT_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::CONNECT_KW,
            ]
        );
    }

    #[test]
    fn lex_operators() {
        let tokens = lex(":> :: .. => ~>");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::COLON_GT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::COLON_COLON,
                SyntaxKind::WHITESPACE,
                SyntaxKind::DOT_DOT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::FAT_ARROW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::TILDE_GT,
            ]
        );
    }

    #[test]
    fn lex_line_comment() {
        let tokens = lex("part // comment\ndef");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::PART_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::LINE_COMMENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::DEF_KW,
            ]
        );
    }

    #[test]
    fn lex_block_comment() {
        let tokens = lex("part /* block */ def");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::PART_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::BLOCK_COMMENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::DEF_KW,
            ]
        );
    }

    #[test]
    fn lex_string() {
        let tokens = lex(r#""hello world""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, SyntaxKind::STRING_LIT);
        assert_eq!(tokens[0].1, r#""hello world""#);
    }

    #[test]
    fn lex_numbers() {
        let tokens = lex("42 3.14");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::INTEGER_LIT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::REAL_LIT,
            ]
        );
    }

    #[test]
    fn lex_multiplicity() {
        let tokens = lex("[0..*]");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::L_BRACKET,
                SyntaxKind::INTEGER_LIT,
                SyntaxKind::DOT_DOT,
                SyntaxKind::STAR,
                SyntaxKind::R_BRACKET,
            ]
        );
    }

    #[test]
    fn lex_colon_gt_gt() {
        let tokens = lex(":>>");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, SyntaxKind::COLON_GT_GT);
    }

    #[test]
    fn lex_case_sensitive() {
        // SysML v2 is case-sensitive: `Package` is not a keyword
        let tokens = lex("Package");
        assert_eq!(tokens[0].0, SyntaxKind::IDENT);
    }
}
