//! Syntax kinds and rowan language definition for the assertion expression language.

/// All syntax kinds for the assertion expression mini-language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(u16)]
pub(crate) enum ExprSyntaxKind {
    // ── Tokens ──────────────────────────────────────────────────────
    /// Whitespace (spaces, tabs).
    WHITESPACE = 0,
    /// An identifier: `components`, `category`, `where`, `has`, `connected`, etc.
    IDENT,
    /// Single-quoted string literal: `'thread'`, `'Timing_Properties::Period'`.
    STRING_LIT,
    /// `.`
    DOT,
    /// `(`
    L_PAREN,
    /// `)`
    R_PAREN,
    /// `==`
    EQ_EQ,
    /// `,`
    COMMA,

    // ── Keywords (contextual) ───────────────────────────────────────
    /// `and`
    AND_KW,
    /// `or`
    OR_KW,
    /// `not`
    NOT_KW,

    // ── Error token ─────────────────────────────────────────────────
    ERROR,

    // ── Composite nodes ─────────────────────────────────────────────
    /// Top-level root node wrapping the entire expression.
    ROOT,
    /// A pipeline expression: `source.method1().method2()`.
    PIPELINE_EXPR,
    /// A `.method(...)` or `.field` call in a pipeline.
    DOT_CALL,
    /// Argument list inside parentheses: `(pred)`.
    CALL_ARGS,
    /// Binary boolean expression: `a and b`, `a or b`.
    BINARY_EXPR,
    /// Unary boolean expression: `not x`.
    UNARY_EXPR,
    /// Comparison expression: `category == 'thread'`.
    COMPARE_EXPR,
    /// Function call expression: `has('...')`, `analysis('...')`.
    CALL_EXPR,
    /// Parenthesized expression: `(expr)`.
    PAREN_EXPR,
    /// A string literal node.
    LITERAL,
    /// An identifier used as an expression: `connected`, `components`.
    IDENT_EXPR,
    /// `message.contains('text')` expression.
    CONTAINS_EXPR,

    /// Sentinel: must be last.
    #[doc(hidden)]
    __LAST,
}

impl From<ExprSyntaxKind> for rowan::SyntaxKind {
    fn from(kind: ExprSyntaxKind) -> Self {
        Self(kind as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ExprLanguage {}

impl rowan::Language for ExprLanguage {
    type Kind = ExprSyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(
            (raw.0 as usize) < ExprSyntaxKind::__LAST as usize,
            "raw SyntaxKind {} out of range",
            raw.0
        );
        // SAFETY: ExprSyntaxKind is repr(u16) and we bounds-checked above.
        unsafe { std::mem::transmute::<u16, ExprSyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub(crate) type SyntaxNode = rowan::SyntaxNode<ExprLanguage>;
