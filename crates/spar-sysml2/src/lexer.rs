//! Lexer for SysML v2 source text.
//!
//! Produces a flat sequence of `(SyntaxKind, byte_length)` token pairs from
//! an input string. SysML v2 uses C-style comments (`//` and `/* */`) and
//! has a `doc /* ... */` pattern for documentation comments.

use crate::syntax_kind::SyntaxKind;

// ---------------------------------------------------------------------------
// Cursor -- zero-copy byte-level scanner
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    #[inline]
    fn current(&self) -> u8 {
        if self.pos < self.input.len() {
            self.input.as_bytes()[self.pos]
        } else {
            0
        }
    }

    #[inline]
    fn peek(&self, n: usize) -> u8 {
        let idx = self.pos + n;
        if idx < self.input.len() {
            self.input.as_bytes()[idx]
        } else {
            0
        }
    }

    #[inline]
    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    #[inline]
    fn bump(&mut self) {
        self.pos += 1;
    }

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

fn scan_whitespace(c: &mut Cursor<'_>) {
    while !c.is_eof() && is_whitespace(c.current()) {
        c.bump();
    }
}

fn scan_line_comment(c: &mut Cursor<'_>) {
    c.bump_n(2); // skip `//`
    while !c.is_eof() && c.current() != b'\n' {
        c.bump();
    }
}

fn scan_block_comment(c: &mut Cursor<'_>) {
    c.bump_n(2); // skip `/*`
    let mut depth = 1;
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

fn scan_string(c: &mut Cursor<'_>) -> bool {
    c.bump(); // skip opening `"`
    while !c.is_eof() {
        if c.current() == b'\\' {
            c.bump(); // skip escape char
            if !c.is_eof() {
                c.bump();
            }
        } else if c.current() == b'"' {
            c.bump();
            return true;
        } else {
            c.bump();
        }
    }
    false
}

fn scan_number(c: &mut Cursor<'_>) -> SyntaxKind {
    while !c.is_eof() && is_digit(c.current()) {
        c.bump();
    }
    if !c.is_eof() && c.current() == b'.' && c.peek(1) != b'.' {
        c.bump(); // skip `.`
        while !c.is_eof() && is_digit(c.current()) {
            c.bump();
        }
        // optional exponent
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
    } else {
        SyntaxKind::INTEGER_LIT
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Tokenize SysML v2 source text into `(SyntaxKind, byte_length)` pairs.
///
/// Every byte in the source is covered by exactly one token (lossless).
pub fn tokenize(source: &str) -> Vec<(SyntaxKind, usize)> {
    let mut tokens = Vec::new();
    let mut c = Cursor::new(source);

    while !c.is_eof() {
        let start = c.pos;
        let kind = scan_token(&mut c, source);
        let len = c.pos - start;
        debug_assert!(len > 0, "lexer made no progress at byte {}", start);
        tokens.push((kind, len));
    }

    tokens
}

fn scan_token(c: &mut Cursor<'_>, source: &str) -> SyntaxKind {
    let b = c.current();

    // Whitespace
    if is_whitespace(b) {
        scan_whitespace(c);
        return SyntaxKind::WHITESPACE;
    }

    // Comments
    if b == b'/' {
        if c.peek(1) == b'/' {
            scan_line_comment(c);
            return SyntaxKind::LINE_COMMENT;
        }
        if c.peek(1) == b'*' {
            scan_block_comment(c);
            return SyntaxKind::BLOCK_COMMENT;
        }
        // Just a `/` operator
        c.bump();
        return SyntaxKind::SLASH;
    }

    // String literals
    if b == b'"' {
        scan_string(c);
        return SyntaxKind::STRING_LIT;
    }

    // Numeric literals
    if is_digit(b) {
        return scan_number(c);
    }

    // Identifiers and keywords
    if is_ident_start(b) {
        let start = c.pos;
        while !c.is_eof() && is_ident_continue(c.current()) {
            c.bump();
        }
        let text = &source[start..c.pos];
        return SyntaxKind::from_keyword(text).unwrap_or(SyntaxKind::IDENT);
    }

    // Punctuation
    match b {
        b';' => {
            c.bump();
            SyntaxKind::SEMICOLON
        }
        b':' => {
            if c.peek(1) == b':' {
                c.bump_n(2);
                SyntaxKind::COLON_COLON
            } else {
                c.bump();
                SyntaxKind::COLON
            }
        }
        b',' => {
            c.bump();
            SyntaxKind::COMMA
        }
        b'.' => {
            if c.peek(1) == b'.' {
                c.bump_n(2);
                SyntaxKind::DOT_DOT
            } else {
                c.bump();
                SyntaxKind::DOT
            }
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
        b'<' => {
            if c.peek(1) == b'=' {
                c.bump_n(2);
                SyntaxKind::LT_EQ
            } else {
                c.bump();
                SyntaxKind::LT
            }
        }
        b'>' => {
            if c.peek(1) == b'=' {
                c.bump_n(2);
                SyntaxKind::GT_EQ
            } else {
                c.bump();
                SyntaxKind::GT
            }
        }
        b'+' => {
            c.bump();
            SyntaxKind::PLUS
        }
        b'-' => {
            c.bump();
            SyntaxKind::MINUS
        }
        b'*' => {
            c.bump();
            SyntaxKind::STAR
        }
        _ => {
            // Unknown character -- produce an error token.
            c.bump();
            SyntaxKind::ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_requirement_def() {
        let source = "requirement def LatencyReq { }";
        let tokens = tokenize(source);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::REQUIREMENT_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::DEF_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::L_CURLY,
                SyntaxKind::WHITESPACE,
                SyntaxKind::R_CURLY,
            ]
        );
    }

    #[test]
    fn tokenize_satisfy() {
        let source = "satisfy sensorLatency by ecu.controller;";
        let tokens = tokenize(source);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::SATISFY_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::BY_KW,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
                SyntaxKind::DOT,
                SyntaxKind::IDENT,
                SyntaxKind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn tokenize_constraint_expression() {
        let source = "totalLatency <= bound;";
        let tokens = tokenize(source);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::IDENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::LT_EQ,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
                SyntaxKind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn tokenize_block_comment() {
        let source = "/* hello world */";
        let tokens = tokenize(source);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, SyntaxKind::BLOCK_COMMENT);
        assert_eq!(tokens[0].1, source.len());
    }

    #[test]
    fn tokenize_real_literal() {
        let source = "20.0";
        let tokens = tokenize(source);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, SyntaxKind::REAL_LIT);
    }

    #[test]
    fn tokenize_line_comment() {
        let source = "// comment\nident";
        let tokens = tokenize(source);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::LINE_COMMENT,
                SyntaxKind::WHITESPACE,
                SyntaxKind::IDENT,
            ]
        );
    }
}
