//! BA parser infrastructure.
//!
//! Event/marker recursive descent parser following the rust-analyzer pattern.
//! Produces an event stream that the tree builder converts to a Rowan CST.

use super::syntax_kind::BaKind;

// -- Events --

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Start {
        kind: BaKind,
        forward_parent: Option<u32>,
    },
    Token {
        kind: BaKind,
        n_raw_tokens: u8,
    },
    Finish,
    Error { msg: String },
    Tombstone,
}

// -- Markers --

pub(crate) struct Marker {
    pos: u32,
    completed: bool,
}

impl Marker {
    fn new(pos: u32) -> Self {
        Marker { pos, completed: false }
    }

    pub(crate) fn complete(mut self, p: &mut Parser<'_>, kind: BaKind) -> CompletedMarker {
        self.completed = true;
        match &mut p.events[self.pos as usize] {
            Event::Start { kind: slot, .. } => *slot = kind,
            _ => unreachable!(),
        }
        p.events.push(Event::Finish);
        CompletedMarker { pos: self.pos }
    }

    pub(crate) fn abandon(mut self, p: &mut Parser<'_>) {
        self.completed = true;
        let idx = self.pos as usize;
        if idx == p.events.len() - 1 {
            match p.events.pop() {
                Some(Event::Start { kind: BaKind::TOMBSTONE, forward_parent: None }) => {}
                _ => unreachable!(),
            }
        } else {
            p.events[idx] = Event::Tombstone;
        }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        if !self.completed && !std::thread::panicking() {
            panic!("Marker must be completed or abandoned");
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
}

impl CompletedMarker {
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let new_pos = p.events.len() as u32;
        p.events.push(Event::Start {
            kind: BaKind::TOMBSTONE,
            forward_parent: None,
        });
        let delta = new_pos - self.pos;
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(delta),
            _ => unreachable!(),
        }
        Marker::new(new_pos)
    }
}

// -- Parser --

pub(crate) struct Parser<'t> {
    all_tokens: &'t [(BaKind, usize)],
    non_trivia: Vec<usize>,
    pos: usize,
    pub(crate) events: Vec<Event>,
    source: &'t str,
    token_starts: Vec<usize>,
}

impl<'t> Parser<'t> {
    pub(crate) fn new(tokens: &'t [(BaKind, usize)], source: &'t str) -> Self {
        let mut token_starts = Vec::with_capacity(tokens.len());
        let mut offset = 0usize;
        for &(_, len) in tokens {
            token_starts.push(offset);
            offset += len;
        }

        let non_trivia: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, (kind, _))| !kind.is_trivia())
            .map(|(i, _)| i)
            .collect();

        Parser {
            all_tokens: tokens,
            non_trivia,
            pos: 0,
            events: Vec::new(),
            source,
            token_starts,
        }
    }

    // -- Token inspection --

    pub(crate) fn current(&self) -> BaKind {
        self.nth(0)
    }

    pub(crate) fn nth(&self, n: usize) -> BaKind {
        let idx = self.pos + n;
        if idx >= self.non_trivia.len() {
            return BaKind::EOF;
        }
        self.all_tokens[self.non_trivia[idx]].0
    }

    pub(crate) fn at(&self, kind: BaKind) -> bool {
        self.current() == kind
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.non_trivia.len()
    }

    /// Get the source text of the current token.
    pub(crate) fn current_text(&self) -> &str {
        if self.at_end() {
            return "";
        }
        let idx = self.non_trivia[self.pos];
        let start = self.token_starts[idx];
        let len = self.all_tokens[idx].1;
        &self.source[start..start + len]
    }

    // -- Token consumption --

    pub(crate) fn bump(&mut self, kind: BaKind) {
        assert!(self.at(kind), "expected {:?}, got {:?}", kind, self.current());
        self.do_bump(kind);
    }

    pub(crate) fn bump_any(&mut self) {
        let kind = self.current();
        if kind != BaKind::EOF {
            self.do_bump(kind);
        }
    }

    pub(crate) fn eat(&mut self, kind: BaKind) -> bool {
        if self.at(kind) {
            self.do_bump(kind);
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: BaKind) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.error(format!("expected {:?}", kind));
            false
        }
    }

    // -- Events --

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::Start {
            kind: BaKind::TOMBSTONE,
            forward_parent: None,
        });
        Marker::new(pos)
    }

    pub(crate) fn error(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Error { msg: msg.into() });
    }

    pub(crate) fn err_and_bump(&mut self, msg: &str) {
        if self.at_end() {
            self.error(msg.to_string());
            return;
        }
        let m = self.start();
        self.error(msg.to_string());
        self.bump_any();
        m.complete(self, BaKind::ERROR);
    }

    fn do_bump(&mut self, kind: BaKind) {
        self.events.push(Event::Token {
            kind,
            n_raw_tokens: 1,
        });
        self.pos += 1;
    }

    // -- Accessors for tree builder --

    pub(crate) fn all_tokens(&self) -> &[(BaKind, usize)] {
        self.all_tokens
    }

    pub(crate) fn non_trivia_indices(&self) -> &[usize] {
        &self.non_trivia
    }

    pub(crate) fn token_starts(&self) -> &[usize] {
        &self.token_starts
    }

    pub(crate) fn source(&self) -> &str {
        self.source
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }
}
