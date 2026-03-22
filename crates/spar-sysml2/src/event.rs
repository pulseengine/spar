use crate::syntax_kind::SyntaxKind;

/// Events produced by the parser, consumed by the tree builder.
#[derive(Debug, Clone)]
pub enum Event {
    /// Start a new node.
    Start {
        kind: SyntaxKind,
        /// Used for `precede()`: delta to a forward parent event.
        forward_parent: Option<u32>,
    },
    /// Consume the next token.
    Token {
        kind: SyntaxKind,
        /// Number of input tokens consumed (usually 1).
        n_raw_tokens: u8,
    },
    /// Finish the current node.
    Finish,
    /// Attach an error message to the current position.
    Error { msg: String },
    /// Tombstone placeholder, will be removed.
    Tombstone,
}
