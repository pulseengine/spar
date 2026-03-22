use crate::event::Event;
use crate::marker::Marker;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// A recursive-descent parser for SysML v2.
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
    /// in the source, including trivia. `source` is the original source text.
    pub fn new(tokens: &'t [(SyntaxKind, usize)], source: &'t str) -> Parser<'t> {
        let mut token_starts = Vec::with_capacity(tokens.len());
        let mut offset = 0usize;
        for &(_kind, len) in tokens {
            token_starts.push(offset);
            offset += len;
        }

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
    pub fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Look ahead `n` non-trivia tokens from the current position.
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
    pub fn at_contextual_kw(&self, text: &str) -> bool {
        if self.at_end() {
            return false;
        }
        let token_idx = self.non_trivia_indices[self.pos];
        let start = self.token_starts[token_idx];
        let len = self.all_tokens[token_idx].1;
        &self.source[start..start + len] == text
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
    pub fn bump(&mut self, kind: SyntaxKind) {
        assert!(
            self.at(kind),
            "expected {:?}, got {:?}",
            kind,
            self.current()
        );
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
        self.push_event(Event::Token { kind, n_raw_tokens });
        self.pos += n_raw_tokens as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tokens() -> Vec<(SyntaxKind, usize)> {
        vec![
            (SyntaxKind::REQUIREMENT_KW, 11), // "requirement"
            (SyntaxKind::WHITESPACE, 1),       // " "
            (SyntaxKind::DEF_KW, 3),           // "def"
            (SyntaxKind::WHITESPACE, 1),       // " "
            (SyntaxKind::IDENT, 4),            // "Test"
            (SyntaxKind::WHITESPACE, 1),       // " "
            (SyntaxKind::L_CURLY, 1),          // "{"
            (SyntaxKind::WHITESPACE, 1),       // " "
            (SyntaxKind::R_CURLY, 1),          // "}"
        ]
    }

    #[test]
    fn skips_trivia() {
        let source = "requirement def Test { }";
        let tokens = make_tokens();
        let p = Parser::new(&tokens, source);
        assert_eq!(p.non_trivia_indices.len(), 5);
        assert_eq!(p.current(), SyntaxKind::REQUIREMENT_KW);
        assert_eq!(p.nth(1), SyntaxKind::DEF_KW);
        assert_eq!(p.nth(2), SyntaxKind::IDENT);
    }

    #[test]
    fn bump_and_eat() {
        let source = "requirement def Test { }";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        p.bump(SyntaxKind::REQUIREMENT_KW);
        assert_eq!(p.current(), SyntaxKind::DEF_KW);
        assert!(p.eat(SyntaxKind::DEF_KW));
        assert_eq!(p.current(), SyntaxKind::IDENT);
    }

    #[test]
    fn expect_emits_error() {
        let source = "requirement def Test { }";
        let tokens = make_tokens();
        let mut p = Parser::new(&tokens, source);
        assert!(!p.expect(SyntaxKind::IDENT));
        let events = p.finish();
        assert!(matches!(events.last(), Some(Event::Error { .. })));
    }

    #[test]
    fn contextual_keyword() {
        let source = "requirement def Test { }";
        let tokens = make_tokens();
        let p = Parser::new(&tokens, source);
        assert!(p.at_contextual_kw("requirement"));
        assert!(!p.at_contextual_kw("constraint"));
    }
}
