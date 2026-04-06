//! Lexer for AADL v2.2 source text.
//!
//! Produces a flat sequence of `(SyntaxKind, &str)` token pairs from an input
//! string. The lexer is a simple cursor-based implementation with no external
//! dependencies.
//!
//! AADL is case-insensitive for keywords: `Package`, `PACKAGE`, and `package`
//! all produce `PACKAGE_KW`. Identifiers preserve their original case.

use crate::syntax_kind::SyntaxKind;

// ---------------------------------------------------------------------------
// Cursor — zero-copy byte-level scanner
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

    /// Returns the remaining (unconsumed) slice.
    #[inline]
    #[allow(dead_code)]
    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
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

    /// Advance by one full UTF-8 character (1–4 bytes).
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

    /// Returns `true` if the remaining input starts with `prefix`.
    #[inline]
    #[allow(dead_code)]
    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().as_bytes().starts_with(prefix.as_bytes())
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

fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Scan whitespace (one or more whitespace characters).
fn scan_whitespace(c: &mut Cursor<'_>) {
    while !c.is_eof() && is_whitespace(c.current()) {
        c.bump();
    }
}

/// Scan a line comment starting at `--`, consuming through end-of-line
/// (the newline itself is NOT included).
fn scan_comment(c: &mut Cursor<'_>) {
    // skip the `--`
    c.bump_n(2);
    while !c.is_eof() && c.current() != b'\n' {
        c.bump();
    }
}

/// Scan a string literal starting at `"`.
/// AADL string literals have no escape sequences.
/// Returns `true` if the string was properly terminated.
fn scan_string(c: &mut Cursor<'_>) -> bool {
    // skip opening `"`
    c.bump();
    while !c.is_eof() {
        if c.current() == b'"' {
            // AS5506 §15.5: doubled-quote `""` inside a string is a literal quote
            if c.peek(1) == b'"' {
                c.bump(); // skip first "
                c.bump(); // skip second "
                continue;
            }
            c.bump();
            return true;
        }
        c.bump();
    }
    // unterminated string — still produce STRING_LIT (error recovery)
    false
}

/// Scan a numeric literal (integer or real).
///
/// AADL numeric literals:
///   integer: `42`, `16#FF#`
///   real:    `3.14`, `1.0e-3`, `1.0E+5`, `16#F.E#E+2`
///
/// Returns the `SyntaxKind` for the scanned literal.
fn scan_number(c: &mut Cursor<'_>) -> SyntaxKind {
    // Consume leading decimal digits.
    while !c.is_eof() && is_digit(c.current()) {
        c.bump();
    }

    // Check for based literal: digits followed by `#`.
    if !c.is_eof() && c.current() == b'#' {
        // Based literal — consume `#`, then hex digits (possibly with a `.`),
        // then closing `#`, then optional exponent.
        c.bump(); // skip `#`
        let mut is_real = false;
        while !c.is_eof() && (is_hex_digit(c.current()) || c.current() == b'.') {
            if c.current() == b'.' {
                is_real = true;
            }
            c.bump();
        }
        // closing `#`
        if !c.is_eof() && c.current() == b'#' {
            c.bump();
        }
        // optional exponent
        if !c.is_eof() && (c.current() == b'e' || c.current() == b'E') {
            is_real = true;
            c.bump();
            if !c.is_eof() && (c.current() == b'+' || c.current() == b'-') {
                c.bump();
            }
            while !c.is_eof() && is_digit(c.current()) {
                c.bump();
            }
        }
        if is_real {
            SyntaxKind::REAL_LIT
        } else {
            SyntaxKind::INTEGER_LIT
        }
    } else if !c.is_eof() && c.current() == b'.' && c.peek(1) != b'.' {
        // Decimal real literal — `digits.digits[E[+-]digits]`
        // We must check `peek(1) != b'.'` so we don't eat `..` as part of a number.
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
        // optional `#` suffix for based real that started with decimal base
        // (already handled above, shouldn't reach here, but be safe)
        SyntaxKind::REAL_LIT
    } else if !c.is_eof() && (c.current() == b'e' || c.current() == b'E') {
        // Integer with exponent like `1E5` — this is actually a real in AADL
        // if it has an exponent.  But AADL spec says integer literals don't
        // have exponents.  Let's treat bare `1E5` as REAL_LIT for robustness.
        c.bump();
        if !c.is_eof() && (c.current() == b'+' || c.current() == b'-') {
            c.bump();
        }
        while !c.is_eof() && is_digit(c.current()) {
            c.bump();
        }
        SyntaxKind::REAL_LIT
    } else {
        // Plain decimal integer, possibly with underscores.
        // (AADL allows underscores in numeric literals.)
        while !c.is_eof() && (is_digit(c.current()) || c.current() == b'_') {
            c.bump();
        }
        SyntaxKind::INTEGER_LIT
    }
}

/// Scan an identifier or keyword.
fn scan_ident_or_keyword(c: &mut Cursor<'_>, start: usize) -> SyntaxKind {
    while !c.is_eof() && is_ident_continue(c.current()) {
        c.bump();
    }
    let text = &c.input[start..c.pos];
    // Case-insensitive keyword lookup: convert to lowercase, then match.
    let lower: String = text.to_ascii_lowercase();
    SyntaxKind::from_keyword(&lower).unwrap_or(SyntaxKind::IDENT)
}

// ---------------------------------------------------------------------------
// Main lexer entry point — scan one token
// ---------------------------------------------------------------------------

/// Scan the next token from the cursor position.
/// Returns `(SyntaxKind, token_text)`.
fn scan_token<'a>(c: &mut Cursor<'a>) -> (SyntaxKind, &'a str) {
    let start = c.pos;

    let kind = match c.current() {
        // Whitespace
        b if is_whitespace(b) => {
            scan_whitespace(c);
            SyntaxKind::WHITESPACE
        }

        // Comment `--`
        b'-' if c.peek(1) == b'-' => {
            scan_comment(c);
            SyntaxKind::COMMENT
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
            c.bump(); // consume the first character
            scan_ident_or_keyword(c, start)
        }

        // Multi-character punctuation — order matters for longest match.

        // `{**` annex open
        b'{' if c.peek(1) == b'*' && c.peek(2) == b'*' => {
            c.bump_n(3);
            SyntaxKind::ANNEX_OPEN
        }

        // `**}` annex close
        b'*' if c.peek(1) == b'*' && c.peek(2) == b'}' => {
            c.bump_n(3);
            SyntaxKind::ANNEX_CLOSE
        }

        // `+=>` plus arrow
        b'+' if c.peek(1) == b'=' && c.peek(2) == b'>' => {
            c.bump_n(3);
            SyntaxKind::PLUS_ARROW
        }

        // `]->` bracket arrow
        b']' if c.peek(1) == b'-' && c.peek(2) == b'>' => {
            c.bump_n(3);
            SyntaxKind::BRACKET_ARROW
        }

        // `<->` bidirectional arrow
        b'<' if c.peek(1) == b'-' && c.peek(2) == b'>' => {
            c.bump_n(3);
            SyntaxKind::BIDI_ARROW
        }

        // `->` arrow
        b'-' if c.peek(1) == b'>' => {
            c.bump_n(2);
            SyntaxKind::ARROW
        }

        // `-[` dash bracket (mode transition)
        b'-' if c.peek(1) == b'[' => {
            c.bump_n(2);
            SyntaxKind::DASH_BRACKET
        }

        // `=>` fat arrow
        b'=' if c.peek(1) == b'>' => {
            c.bump_n(2);
            SyntaxKind::FAT_ARROW
        }

        // `..` dot dot
        b'.' if c.peek(1) == b'.' => {
            c.bump_n(2);
            SyntaxKind::DOT_DOT
        }

        // `::` colon colon
        b':' if c.peek(1) == b':' => {
            c.bump_n(2);
            SyntaxKind::COLON_COLON
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
        b'#' => {
            c.bump();
            SyntaxKind::HASH
        }

        // Unknown / error character — advance a full UTF-8 character
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

/// Tokenize an AADL source string into `(SyntaxKind, &str)` pairs.
///
/// Every byte of the input is covered by exactly one token — whitespace,
/// comments, and errors are all represented.
pub fn lex(input: &str) -> Vec<(SyntaxKind, &str)> {
    let mut c = Cursor::new(input);
    let mut tokens = Vec::new();
    while !c.is_eof() {
        tokens.push(scan_token(&mut c));
    }
    tokens
}

/// Tokenize an AADL source string into `(SyntaxKind, len)` pairs.
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

// ---------------------------------------------------------------------------
// LexedStr — tokens with text ranges
// ---------------------------------------------------------------------------

/// A fully lexed source string that stores tokens together with their byte
/// ranges into the original source.
///
/// This is the primary interface between the lexer and higher layers (parser,
/// syntax tree builder). It owns nothing but the index data; the source text
/// is borrowed.
#[derive(Debug)]
pub struct LexedStr<'a> {
    input: &'a str,
    /// Each entry is `(SyntaxKind, start_byte_offset)`. An implicit final
    /// entry at `input.len()` marks the end.
    tokens: Vec<(SyntaxKind, u32)>,
}

impl<'a> LexedStr<'a> {
    /// Lex the entire input string.
    pub fn new(input: &'a str) -> Self {
        let mut tokens = Vec::new();
        let mut c = Cursor::new(input);
        while !c.is_eof() {
            let start = c.pos as u32;
            let (kind, _) = scan_token(&mut c);
            tokens.push((kind, start));
        }
        Self { input, tokens }
    }

    /// Number of tokens (excluding EOF).
    #[inline]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Returns `true` when the token sequence is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The `SyntaxKind` of token at `index`.
    #[inline]
    pub fn kind(&self, index: usize) -> SyntaxKind {
        self.tokens[index].0
    }

    /// The source text of token at `index`.
    #[inline]
    pub fn text(&self, index: usize) -> &'a str {
        let start = self.tokens[index].1 as usize;
        let end = if index + 1 < self.tokens.len() {
            self.tokens[index + 1].1 as usize
        } else {
            self.input.len()
        };
        &self.input[start..end]
    }

    /// Byte range `(start, end)` of token at `index`.
    #[inline]
    pub fn text_range(&self, index: usize) -> (usize, usize) {
        let start = self.tokens[index].1 as usize;
        let end = if index + 1 < self.tokens.len() {
            self.tokens[index + 1].1 as usize
        } else {
            self.input.len()
        };
        (start, end)
    }

    /// The start byte offset of token at `index`.
    #[inline]
    pub fn text_start(&self, index: usize) -> usize {
        self.tokens[index].1 as usize
    }

    /// The full source text.
    #[inline]
    pub fn source(&self) -> &'a str {
        self.input
    }

    /// Iterator over `(SyntaxKind, &str)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (SyntaxKind, &'a str)> + '_ {
        (0..self.len()).map(move |i| (self.kind(i), self.text(i)))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxKind::*;

    /// Helper: lex input and return just the kinds (filtering whitespace).
    fn lex_kinds(input: &str) -> Vec<SyntaxKind> {
        lex(input)
            .into_iter()
            .filter(|(k, _)| *k != WHITESPACE)
            .map(|(k, _)| k)
            .collect()
    }

    /// Helper: lex input and return `(kind, text)` pairs (filtering whitespace).
    fn lex_tokens(input: &str) -> Vec<(SyntaxKind, &str)> {
        lex(input)
            .into_iter()
            .filter(|(k, _)| *k != WHITESPACE)
            .collect()
    }

    // -- Whitespace & comments --

    #[test]
    fn whitespace() {
        let tokens = lex("  \t\n\r\n ");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, WHITESPACE);
        assert_eq!(tokens[0].1, "  \t\n\r\n ");
    }

    #[test]
    fn comment_basic() {
        let tokens = lex("-- this is a comment\n");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], (COMMENT, "-- this is a comment"));
        assert_eq!(tokens[1], (WHITESPACE, "\n"));
    }

    #[test]
    fn comment_eof() {
        let tokens = lex("-- comment at eof");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], (COMMENT, "-- comment at eof"));
    }

    // -- String literals --

    #[test]
    fn string_literal() {
        let tokens = lex_tokens("\"hello world\"");
        assert_eq!(tokens, vec![(STRING_LIT, "\"hello world\"")]);
    }

    #[test]
    fn string_literal_empty() {
        let tokens = lex_tokens("\"\"");
        assert_eq!(tokens, vec![(STRING_LIT, "\"\"")]);
    }

    #[test]
    fn string_literal_unterminated() {
        let tokens = lex_tokens("\"oops");
        assert_eq!(tokens, vec![(STRING_LIT, "\"oops")]);
    }

    // -- Integer literals --

    #[test]
    fn integer_decimal() {
        let tokens = lex_tokens("42");
        assert_eq!(tokens, vec![(INTEGER_LIT, "42")]);
    }

    #[test]
    fn integer_based() {
        let tokens = lex_tokens("16#FF#");
        assert_eq!(tokens, vec![(INTEGER_LIT, "16#FF#")]);
    }

    #[test]
    fn integer_based_lower() {
        let tokens = lex_tokens("16#ff#");
        assert_eq!(tokens, vec![(INTEGER_LIT, "16#ff#")]);
    }

    #[test]
    fn integer_binary() {
        let tokens = lex_tokens("2#1010#");
        assert_eq!(tokens, vec![(INTEGER_LIT, "2#1010#")]);
    }

    // -- Real literals --

    #[test]
    fn real_decimal() {
        let tokens = lex_tokens("3.14");
        assert_eq!(tokens, vec![(REAL_LIT, "3.14")]);
    }

    #[test]
    fn real_with_exponent() {
        let tokens = lex_tokens("1.0e-3");
        assert_eq!(tokens, vec![(REAL_LIT, "1.0e-3")]);
    }

    #[test]
    fn real_with_positive_exponent() {
        let tokens = lex_tokens("1.0E+5");
        assert_eq!(tokens, vec![(REAL_LIT, "1.0E+5")]);
    }

    #[test]
    fn real_based() {
        let tokens = lex_tokens("16#F.E#E+2");
        assert_eq!(tokens, vec![(REAL_LIT, "16#F.E#E+2")]);
    }

    // -- Punctuation --

    #[test]
    fn punctuation_single() {
        let tokens = lex_tokens("; : , . ( ) { } [ ] + - * #");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SEMICOLON, COLON, COMMA, DOT, L_PAREN, R_PAREN, L_CURLY, R_CURLY, L_BRACKET,
                R_BRACKET, PLUS, MINUS, STAR, HASH,
            ]
        );
    }

    #[test]
    fn punctuation_multi() {
        let tokens = lex_tokens(".. -> <-> => +=> :: -[ ]->");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                DOT_DOT,
                ARROW,
                BIDI_ARROW,
                FAT_ARROW,
                PLUS_ARROW,
                COLON_COLON,
                DASH_BRACKET,
                BRACKET_ARROW
            ]
        );
    }

    #[test]
    fn annex_delimiters() {
        let tokens = lex_tokens("{** **}");
        assert_eq!(tokens, vec![(ANNEX_OPEN, "{**"), (ANNEX_CLOSE, "**}")]);
    }

    // -- Identifiers --

    #[test]
    fn identifier_simple() {
        let tokens = lex_tokens("my_component");
        assert_eq!(tokens, vec![(IDENT, "my_component")]);
    }

    #[test]
    fn identifier_with_digits() {
        let tokens = lex_tokens("port_1");
        assert_eq!(tokens, vec![(IDENT, "port_1")]);
    }

    #[test]
    fn identifier_starts_with_underscore() {
        let tokens = lex_tokens("_hidden");
        assert_eq!(tokens, vec![(IDENT, "_hidden")]);
    }

    // -- Keywords --

    #[test]
    fn keyword_lowercase() {
        let tokens = lex_tokens("package");
        assert_eq!(tokens, vec![(PACKAGE_KW, "package")]);
    }

    #[test]
    fn keyword_uppercase() {
        let tokens = lex_tokens("PACKAGE");
        assert_eq!(tokens, vec![(PACKAGE_KW, "PACKAGE")]);
    }

    #[test]
    fn keyword_mixed_case() {
        let tokens = lex_tokens("Package");
        assert_eq!(tokens, vec![(PACKAGE_KW, "Package")]);
    }

    #[test]
    fn all_keywords_recognized() {
        let keywords = vec![
            ("package", PACKAGE_KW),
            ("public", PUBLIC_KW),
            ("private", PRIVATE_KW),
            ("with", WITH_KW),
            ("end", END_KW),
            ("none", NONE_KW),
            ("renames", RENAMES_KW),
            ("system", SYSTEM_KW),
            ("process", PROCESS_KW),
            ("thread", THREAD_KW),
            ("group", GROUP_KW),
            ("processor", PROCESSOR_KW),
            ("virtual", VIRTUAL_KW),
            ("bus", BUS_KW),
            ("memory", MEMORY_KW),
            ("device", DEVICE_KW),
            ("subprogram", SUBPROGRAM_KW),
            ("data", DATA_KW),
            ("abstract", ABSTRACT_KW),
            ("implementation", IMPLEMENTATION_KW),
            ("extends", EXTENDS_KW),
            ("prototypes", PROTOTYPES_KW),
            ("features", FEATURES_KW),
            ("flows", FLOWS_KW),
            ("connections", CONNECTIONS_KW),
            ("modes", MODES_KW),
            ("properties", PROPERTIES_KW),
            ("subcomponents", SUBCOMPONENTS_KW),
            ("annex", ANNEX_KW),
            ("calls", CALLS_KW),
            ("internal", INTERNAL_KW),
            ("in", IN_KW),
            ("out", OUT_KW),
            ("port", PORT_KW),
            ("event", EVENT_KW),
            ("access", ACCESS_KW),
            ("provides", PROVIDES_KW),
            ("requires", REQUIRES_KW),
            ("feature", FEATURE_KW),
            ("parameter", PARAMETER_KW),
            ("inverse", INVERSE_KW),
            ("of", OF_KW),
            ("flow", FLOW_KW),
            ("source", SOURCE_KW),
            ("sink", SINK_KW),
            ("path", PATH_KW),
            ("initial", INITIAL_KW),
            ("mode", MODE_KW),
            ("transition", TRANSITION_KW),
            ("constant", CONSTANT_KW),
            ("applies", APPLIES_KW),
            ("to", TO_KW),
            ("inherit", INHERIT_KW),
            ("delta", DELTA_KW),
            ("is", IS_KW),
            ("all", ALL_KW),
            ("binding", BINDING_KW),
            ("classifier", CLASSIFIER_KW),
            ("reference", REFERENCE_KW),
            ("record", RECORD_KW),
            ("compute", COMPUTE_KW),
            ("type", TYPE_KW),
            ("set", SET_KW),
            ("range", RANGE_KW),
            ("units", UNITS_KW),
            ("enumeration", ENUMERATION_KW),
            ("list", LIST_KW),
            ("aadlboolean", AADLBOOLEAN_KW),
            ("aadlinteger", AADLINTEGER_KW),
            ("aadlreal", AADLREAL_KW),
            ("aadlstring", AADLSTRING_KW),
            ("true", TRUE_KW),
            ("false", FALSE_KW),
            ("not", NOT_KW),
            ("and", AND_KW),
            ("or", OR_KW),
            ("refined", REFINED_KW),
            ("self", SELF_KW),
            ("interface", INTERFACE_KW),
            ("file", FILE_KW),
        ];
        for (text, expected_kind) in keywords {
            let tokens = lex_tokens(text);
            assert_eq!(
                tokens,
                vec![(expected_kind, text)],
                "keyword `{}` should produce {:?}",
                text,
                expected_kind,
            );
        }
    }

    // -- Case insensitivity --

    #[test]
    fn case_insensitive_keywords() {
        assert_eq!(lex_kinds("System"), vec![SYSTEM_KW]);
        assert_eq!(lex_kinds("SYSTEM"), vec![SYSTEM_KW]);
        assert_eq!(lex_kinds("sYsTeM"), vec![SYSTEM_KW]);
        assert_eq!(lex_kinds("AadlBoolean"), vec![AADLBOOLEAN_KW]);
        assert_eq!(lex_kinds("AADLBOOLEAN"), vec![AADLBOOLEAN_KW]);
    }

    // -- Complex sequences --

    #[test]
    fn package_declaration() {
        let input = "package Sensors\npublic\nend Sensors;";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![PACKAGE_KW, IDENT, PUBLIC_KW, END_KW, IDENT, SEMICOLON]
        );
    }

    #[test]
    fn port_declaration() {
        let input = "sensor_data : in data port Base_Types::Integer_32;";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                IDENT,
                COLON,
                IN_KW,
                DATA_KW,
                PORT_KW,
                IDENT,
                COLON_COLON,
                IDENT,
                SEMICOLON,
            ]
        );
    }

    #[test]
    fn property_with_value() {
        let input = "Period => 20 ms;";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![IDENT, FAT_ARROW, INTEGER_LIT, IDENT, SEMICOLON]);
    }

    #[test]
    fn range_expression() {
        let input = "1 .. 10";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![INTEGER_LIT, DOT_DOT, INTEGER_LIT]);
    }

    #[test]
    fn number_before_dot_dot() {
        // Ensure `42..50` lexes as INTEGER DOT_DOT INTEGER, not REAL DOT etc.
        let tokens = lex_tokens("42..50");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![INTEGER_LIT, DOT_DOT, INTEGER_LIT]);
    }

    #[test]
    fn connection_arrow() {
        let input = "src -> dst";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![IDENT, ARROW, IDENT]);
    }

    #[test]
    fn mode_transition_syntax() {
        let input = "m1 -[ trigger ]-> m2";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![IDENT, DASH_BRACKET, IDENT, BRACKET_ARROW, IDENT]
        );
    }

    #[test]
    fn annex_subclause_delimiters() {
        let input = "annex EMV2 {** some annex text **};";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ANNEX_KW,
                IDENT,
                ANNEX_OPEN,
                IDENT,
                ANNEX_KW,
                IDENT,
                ANNEX_CLOSE,
                SEMICOLON
            ]
        );
    }

    #[test]
    fn append_property() {
        let input = "prop +=> 42;";
        let tokens = lex_tokens(input);
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![IDENT, PLUS_ARROW, INTEGER_LIT, SEMICOLON]);
    }

    #[test]
    fn error_char() {
        let tokens = lex_tokens("@");
        assert_eq!(tokens, vec![(ERROR, "@")]);
    }

    // -- tokenize() --

    #[test]
    fn tokenize_returns_lengths() {
        let input = "package Foo;";
        let tokens = tokenize(input);
        // "package" = 7, " " = 1, "Foo" = 3, ";" = 1
        assert_eq!(
            tokens,
            vec![(PACKAGE_KW, 7), (WHITESPACE, 1), (IDENT, 3), (SEMICOLON, 1),]
        );
    }

    #[test]
    fn tokenize_total_length() {
        let input = "system implementation Ctrl.impl";
        let tokens = tokenize(input);
        let total: usize = tokens.iter().map(|(_, len)| len).sum();
        assert_eq!(total, input.len());
    }

    // -- LexedStr --

    #[test]
    fn lexed_str_basic() {
        let input = "package Foo;";
        let lexed = LexedStr::new(input);
        assert_eq!(lexed.len(), 4);
        assert_eq!(lexed.kind(0), PACKAGE_KW);
        assert_eq!(lexed.text(0), "package");
        assert_eq!(lexed.kind(1), WHITESPACE);
        assert_eq!(lexed.text(1), " ");
        assert_eq!(lexed.kind(2), IDENT);
        assert_eq!(lexed.text(2), "Foo");
        assert_eq!(lexed.kind(3), SEMICOLON);
        assert_eq!(lexed.text(3), ";");
    }

    #[test]
    fn lexed_str_text_range() {
        let input = "end Foo;";
        let lexed = LexedStr::new(input);
        assert_eq!(lexed.text_range(0), (0, 3)); // "end"
        assert_eq!(lexed.text_range(1), (3, 4)); // " "
        assert_eq!(lexed.text_range(2), (4, 7)); // "Foo"
        assert_eq!(lexed.text_range(3), (7, 8)); // ";"
    }

    #[test]
    fn lexed_str_iter() {
        let input = "in out";
        let lexed = LexedStr::new(input);
        let pairs: Vec<_> = lexed.iter().collect();
        assert_eq!(
            pairs,
            vec![(IN_KW, "in"), (WHITESPACE, " "), (OUT_KW, "out")]
        );
    }

    #[test]
    fn lexed_str_source() {
        let input = "data port";
        let lexed = LexedStr::new(input);
        assert_eq!(lexed.source(), input);
    }

    #[test]
    fn lexed_str_empty() {
        let lexed = LexedStr::new("");
        assert!(lexed.is_empty());
        assert_eq!(lexed.len(), 0);
    }

    // -- Coverage: full input is lossless --

    #[test]
    fn lossless_roundtrip() {
        let input = "package Sensors\n  public\n    with Base_Types;\n  end Sensors;\n";
        let tokens = lex(input);
        let reassembled: String = tokens.iter().map(|(_, text)| *text).collect();
        assert_eq!(reassembled, input);
    }

    #[test]
    fn lossless_roundtrip_complex() {
        let input = concat!(
            "system implementation Ctrl.impl\n",
            "  subcomponents\n",
            "    s : system Sensor.impl;\n",
            "  connections\n",
            "    c1 : port s.out_val -> actuator_cmd;\n",
            "  properties\n",
            "    Period => 20 ms;\n",
            "end Ctrl.impl;\n",
        );
        let tokens = lex(input);
        let reassembled: String = tokens.iter().map(|(_, text)| *text).collect();
        assert_eq!(reassembled, input);
    }
}
