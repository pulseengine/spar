use crate::event::Event;
use crate::marker::Marker;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// A recursive-descent parser for AADL.
///
/// Grammar functions call methods on `Parser` to inspect tokens and emit
/// [`Event`]s. The events are later consumed by a tree builder to produce a
/// lossless concrete syntax tree.
///
/// Trivia tokens (whitespace and comments) are stored but hidden from grammar
/// functions. They are re-inserted during tree construction.
pub struct Parser<'t> {
    /// All tokens including trivia, exactly as produced by the lexer.
    all_tokens: &'t [(SyntaxKind, usize)],
    /// Indices into `all_tokens` for non-trivia tokens only.
    non_trivia_indices: Vec<usize>,
    /// Current position within `non_trivia_indices`.
    pos: usize,
    /// Accumulated parser events.
    pub(crate) events: Vec<Event>,
    /// Source text (kept for contextual keyword checks).
    source: &'t str,
    /// Byte offsets for the start of each token in `all_tokens`.
    token_starts: Vec<usize>,
}

impl<'t> Parser<'t> {
    /// Create a new parser from tokenized input.
    ///
    /// `tokens` contains `(SyntaxKind, byte_length)` pairs for every token
    /// in the source, including trivia. `source` is the original source text
    /// (needed for contextual keyword checks).
    pub fn new(tokens: &'t [(SyntaxKind, usize)], source: &'t str) -> Parser<'t> {
        // Pre-compute the byte offset of each token.
        let mut token_starts = Vec::with_capacity(tokens.len());
        let mut offset = 0usize;
        for &(_kind, len) in tokens {
            token_starts.push(offset);
            offset += len;
        }

        // Build the non-trivia index list.
        let non_trivia_indices: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, (kind, _))| !kind.is_trivia())
            .map(|(i, _)| i)
            .collect();

        Parser {
            all_tokens: tokens,
            non_trivia_indices,
            pos: 0,
            events: Vec::new(),
            source,
            token_starts,
        }
    }

    // ------------------------------------------------------------------
    // Token inspection
    // ------------------------------------------------------------------

    /// Peek at the current token's kind.
    ///
    /// Returns [`SyntaxKind::EOF`] when the parser has consumed all tokens.
    pub fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Look ahead `n` non-trivia tokens from the current position.
    ///
    /// `nth(0)` is equivalent to `current()`.
    pub fn nth(&self, n: usize) -> SyntaxKind {
        let idx = self.pos + n;
        if idx >= self.non_trivia_indices.len() {
            return SyntaxKind::EOF;
        }
        let token_idx = self.non_trivia_indices[idx];
        self.all_tokens[token_idx].0
    }

    /// Check whether the current token is `kind`.
    pub fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// Check whether the current token is a contextual keyword matching `text`.
    ///
    /// Contextual keywords are identifiers that act as keywords only in
    /// specific syntactic positions. This compares the source text of the
    /// current token against the given string.
    pub fn at_contextual_kw(&self, text: &str) -> bool {
        if self.at_end() {
            return false;
        }
        let token_idx = self.non_trivia_indices[self.pos];
        let start = self.token_starts[token_idx];
        let len = self.all_tokens[token_idx].1;
        &self.source[start..start + len] == text
    }

    /// Check whether the current token can serve as a declaration name.
    ///
    /// AADL is case-insensitive and names can collide with keywords
    /// (e.g., a flow named `compute`, a type named `Processor`). For
    /// keywords, we only treat them as names if followed by `:`, which
    /// is the universal pattern for AADL declarations (`name : ...`).
    /// This prevents section keywords like `end`, `flows`, etc. from
    /// being consumed as names.
    pub fn at_name(&self) -> bool {
        if self.at(SyntaxKind::IDENT) {
            return true;
        }
        // A keyword followed by `:` is likely a declaration name
        self.current().is_keyword() && self.nth(1) == SyntaxKind::COLON
    }

    /// Check whether the parser has consumed all non-trivia tokens.
    pub fn at_end(&self) -> bool {
        self.pos >= self.non_trivia_indices.len()
    }

    /// Return the source text of the current token, or `""` if at end.
    pub fn current_text(&self) -> &str {
        if self.at_end() {
            return "";
        }
        let token_idx = self.non_trivia_indices[self.pos];
        let start = self.token_starts[token_idx];
        let len = self.all_tokens[token_idx].1;
        &self.source[start..start + len]
    }

    // ------------------------------------------------------------------
    // Token consumption
    // ------------------------------------------------------------------

    /// Consume the current token, asserting that it matches `kind`.
    ///
    /// # Panics
    ///
    /// Panics if the current token is not `kind`. Use [`Parser::eat`] or
    /// [`Parser::expect`] for fallible consumption.
    pub fn bump(&mut self, kind: SyntaxKind) {
        assert!(self.at(kind), "expected {:?}, got {:?}", kind, self.current());
        self.do_bump(kind, 1);
    }

    /// Consume the current token regardless of its kind.
    pub fn bump_any(&mut self) {
        let kind = self.current();
        if kind == SyntaxKind::EOF {
            return;
        }
        self.do_bump(kind, 1);
    }

    /// If the current token is `kind`, consume it and return `true`.
    /// Otherwise return `false` without consuming anything.
    pub fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.do_bump(kind, 1);
            true
        } else {
            false
        }
    }

    /// If the current token is `kind`, consume it and return `true`.
    /// Otherwise, emit an error and return `false`.
    pub fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.error(format!("expected {:?}", kind));
            false
        }
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    /// Begin a new syntax node. Returns a [`Marker`] that must be completed
    /// or abandoned.
    pub fn start(&mut self) -> Marker {
        let pos = self.start_event();
        Marker::new(pos)
    }

    /// Emit an error event with the given message.
    pub fn error(&mut self, msg: impl Into<String>) {
        self.push_event(Event::Error { msg: msg.into() });
    }

    /// Wrap the current token in an `ERROR` node and emit the given error
    /// message. Advances past the token.
    pub fn err_and_bump(&mut self, msg: &str) {
        self.err_recover(msg, TokenSet::EMPTY);
    }

    /// Emit an error, then skip tokens until a token in `recovery` is found
    /// (or EOF is reached). Skipped tokens are wrapped in an `ERROR` node.
    pub fn err_recover(&mut self, msg: &str, recovery: TokenSet) {
        if self.at(SyntaxKind::EOF) {
            self.error(msg.to_string());
            return;
        }

        if recovery.contains(self.current()) {
            self.error(msg.to_string());
            return;
        }

        let m = self.start();
        self.error(msg.to_string());
        self.bump_any();
        // Skip tokens until we hit a recovery token or EOF.
        while !self.at_end() && !recovery.contains(self.current()) {
            self.bump_any();
        }
        m.complete(self, SyntaxKind::ERROR);
    }

    /// Consume the parser and return the accumulated event list.
    pub fn finish(self) -> Vec<Event> {
        self.events
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Push a `Start { TOMBSTONE }` event and return its index.
    pub(crate) fn start_event(&mut self) -> u32 {
        let pos = self.events.len() as u32;
        self.push_event(Event::Start {
            kind: SyntaxKind::TOMBSTONE,
            forward_parent: None,
        });
        pos
    }

    /// Push an event onto the event list.
    pub(crate) fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Internal bump: emit a Token event and advance the position.
    fn do_bump(&mut self, kind: SyntaxKind, n_raw_tokens: u8) {
        self.push_event(Event::Token {
            kind,
            n_raw_tokens,
        });
        self.pos += n_raw_tokens as usize;
    }
}

// ------------------------------------------------------------------
// Accessors for tree building
// ------------------------------------------------------------------

impl<'t> Parser<'t> {
    /// Return a reference to the full token list (including trivia).
    pub fn all_tokens(&self) -> &[(SyntaxKind, usize)] {
        self.all_tokens
    }

    /// Return the non-trivia index mapping.
    pub fn non_trivia_indices(&self) -> &[usize] {
        &self.non_trivia_indices
    }

    /// Return the source text.
    pub fn source(&self) -> &str {
        self.source
    }

    /// Return the byte offsets for each token.
    pub fn token_starts(&self) -> &[usize] {
        &self.token_starts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tokens() -> Vec<(SyntaxKind, usize)> {
        vec![
            (SyntaxKind::PACKAGE_KW, 7),   // "package"
            (SyntaxKind::WHITESPACE, 1),    // " "
            (SyntaxKind::IDENT, 4),         // "Test"
            (SyntaxKind::WHITESPACE, 1),    // " "
            (SyntaxKind::SEMICOLON, 1),     // ";"
        ]
    }

    #[test]
    fn skips_trivia() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let p = Parser::new(&tokens, source);
        // Non-trivia tokens: PACKAGE_KW, IDENT, SEMICOLON
        assert_eq!(p.non_trivia_indices.len(), 3);
        assert_eq!(p.current(), SyntaxKind::PACKAGE_KW);
        assert_eq!(p.nth(1), SyntaxKind::IDENT);
        assert_eq!(p.nth(2), SyntaxKind::SEMICOLON);
        assert_eq!(p.nth(3), SyntaxKind::EOF);
    }

    #[test]
    fn bump_and_eat() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        assert!(p.at(SyntaxKind::PACKAGE_KW));
        p.bump(SyntaxKind::PACKAGE_KW);
        assert_eq!(p.current(), SyntaxKind::IDENT);
        assert!(p.eat(SyntaxKind::IDENT));
        assert!(!p.eat(SyntaxKind::IDENT)); // already consumed
        assert_eq!(p.current(), SyntaxKind::SEMICOLON);
        p.bump_any();
        assert!(p.at_end());
    }

    #[test]
    fn expect_emits_error() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        // Expect IDENT but current is PACKAGE_KW => error
        assert!(!p.expect(SyntaxKind::IDENT));
        // The error event should be in the list
        let events = p.finish();
        assert!(matches!(events.last(), Some(Event::Error { .. })));
    }

    #[test]
    fn contextual_keyword() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let p = Parser::new(&tokens, source);
        assert!(p.at_contextual_kw("package"));
        assert!(!p.at_contextual_kw("system"));
    }

    #[test]
    fn marker_complete() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        let m = p.start();
        p.bump(SyntaxKind::PACKAGE_KW);
        m.complete(&mut p, SyntaxKind::AADL_PACKAGE);
        let events = p.finish();
        assert!(matches!(
            events[0],
            Event::Start {
                kind: SyntaxKind::AADL_PACKAGE,
                ..
            }
        ));
        assert!(matches!(events[1], Event::Token { kind: SyntaxKind::PACKAGE_KW, .. }));
        assert!(matches!(events[2], Event::Finish));
    }

    #[test]
    fn marker_abandon() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        let m = p.start();
        // Decide we don't want this node after all.
        m.abandon(&mut p);
        // Should not have left a Start event.
        let events = p.finish();
        assert!(events.is_empty());
    }

    #[test]
    fn completed_marker_precede() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);

        let m = p.start();
        p.bump(SyntaxKind::PACKAGE_KW);
        let cm = m.complete(&mut p, SyntaxKind::NAME);

        // Now wrap the completed NAME in a QUALIFIED_NAME.
        let m2 = cm.precede(&mut p);
        p.bump(SyntaxKind::IDENT);
        m2.complete(&mut p, SyntaxKind::QUALIFIED_NAME);

        let events = p.finish();
        // The original Start event should have a forward_parent set.
        match &events[0] {
            Event::Start { forward_parent, .. } => {
                assert!(forward_parent.is_some());
            }
            _ => panic!("expected Start event"),
        }
    }

    #[test]
    fn err_recover_skips_to_recovery() {
        let source = "package Test ;";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        let recovery = TokenSet::new(&[SyntaxKind::SEMICOLON]);
        p.err_recover("unexpected token", recovery);
        // Should have consumed PACKAGE_KW and IDENT, stopping before SEMICOLON.
        assert_eq!(p.current(), SyntaxKind::SEMICOLON);
    }

    #[test]
    fn at_end_when_empty() {
        let tokens: Vec<(SyntaxKind, usize)> = vec![];
        let p = Parser::new(&tokens, "");
        assert!(p.at_end());
        assert_eq!(p.current(), SyntaxKind::EOF);
    }
}
