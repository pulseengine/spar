/// All syntax kinds for the SysML v2 language subset.
///
/// This enum covers tokens and node types needed for SysML v2 parsing,
/// following the same marker-based parser pattern as `spar-parser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(non_camel_case_types)]
#[allow(clippy::manual_non_exhaustive)]
#[repr(u16)]
pub enum SyntaxKind {
    // === Tombstone / Error ===
    /// Placeholder used during parsing, replaced before tree construction.
    TOMBSTONE = 0,
    /// End of file marker.
    EOF,
    /// Error token or error node wrapping unexpected content.
    ERROR,

    // === Trivia ===
    /// Whitespace (spaces, tabs, newlines).
    WHITESPACE,
    /// Line comment: `// ...`
    LINE_COMMENT,
    /// Block comment: `/* ... */`
    BLOCK_COMMENT,
    /// Doc comment: `doc /* ... */`
    DOC_COMMENT,

    // === Punctuation ===
    /// `;`
    SEMICOLON,
    /// `:`
    COLON,
    /// `,`
    COMMA,
    /// `.`
    DOT,
    /// `(`
    L_PAREN,
    /// `)`
    R_PAREN,
    /// `{`
    L_CURLY,
    /// `}`
    R_CURLY,
    /// `[`
    L_BRACKET,
    /// `]`
    R_BRACKET,
    /// `=`
    EQ,
    /// `<=`
    LT_EQ,
    /// `>=`
    GT_EQ,
    /// `<`
    LT,
    /// `>`
    GT,
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `*`
    STAR,
    /// `/`
    SLASH,
    /// `::`
    COLON_COLON,
    /// `..`
    DOT_DOT,

    // === Literals ===
    /// Integer literal: `42`
    INTEGER_LIT,
    /// Real literal: `3.14`, `20.0`
    REAL_LIT,
    /// String literal: `"hello"`
    STRING_LIT,

    // === Identifiers ===
    /// Identifier: `myComponent`
    IDENT,

    // === Keywords ===
    /// `package`
    PACKAGE_KW,
    /// `import`
    IMPORT_KW,
    /// `requirement`
    REQUIREMENT_KW,
    /// `constraint`
    CONSTRAINT_KW,
    /// `def`
    DEF_KW,
    /// `satisfy`
    SATISFY_KW,
    /// `verify`
    VERIFY_KW,
    /// `refine`
    REFINE_KW,
    /// `by`
    BY_KW,
    /// `attribute`
    ATTRIBUTE_KW,
    /// `subject`
    SUBJECT_KW,
    /// `doc`
    DOC_KW,
    /// `part`
    PART_KW,
    /// `port`
    PORT_KW,
    /// `connection`
    CONNECTION_KW,
    /// `action`
    ACTION_KW,
    /// `item`
    ITEM_KW,
    /// `in`
    IN_KW,
    /// `out`
    OUT_KW,
    /// `inout`
    INOUT_KW,
    /// `ref`
    REF_KW,
    /// `assume`
    ASSUME_KW,
    /// `require`
    REQUIRE_KW,

    // === Node kinds ===

    // Top-level
    /// Root node for a source file.
    SOURCE_FILE,

    // Names
    /// Simple or qualified name.
    NAME,
    /// Qualified name with `::` separators.
    QUALIFIED_NAME,

    // Definitions
    /// `requirement def Name { body }` — requirement definition
    REQUIREMENT_DEF,
    /// `requirement name : Type { body }` — requirement usage
    REQUIREMENT_USAGE,
    /// `constraint def Name { body }` — constraint definition
    CONSTRAINT_DEF,
    /// `constraint name : Type { body }` — constraint usage
    CONSTRAINT_USAGE,

    // Relationships
    /// `satisfy req by impl;`
    SATISFY_REQ,
    /// `verify req by test;`
    VERIFY_REQ,
    /// `refine req1 by req2;`
    REFINE_REQ,

    // Body elements
    /// `attribute name : Type = value;`
    ATTRIBUTE_USAGE,
    /// `subject name : Type;`
    SUBJECT_MEMBER,
    /// `doc /* text */`
    DOC_MEMBER,

    // Expressions
    /// A general expression node.
    EXPRESSION,
    /// Binary expression: `a <= b`, `a + b`
    BINARY_EXPR,
    /// Name reference in expression context.
    NAME_REF,
    /// Literal value in expression context.
    LITERAL,

    // Type references
    /// Type reference: `LatencyReq`, `Real`
    TYPE_REF,

    // Blocks
    /// `{ ... }` body block.
    BODY_BLOCK,

    // Keep this last -- used for bounds checking.
    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    /// Returns true if this is a trivia token (whitespace or comment).
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::WHITESPACE | Self::LINE_COMMENT | Self::BLOCK_COMMENT
        )
    }

    /// Returns true if this is a keyword.
    #[inline]
    pub fn is_keyword(self) -> bool {
        (self as u16) >= (Self::PACKAGE_KW as u16) && (self as u16) <= (Self::REQUIRE_KW as u16)
    }

    /// Returns true if this is a punctuation token.
    #[inline]
    pub fn is_punct(self) -> bool {
        (self as u16) >= (Self::SEMICOLON as u16) && (self as u16) <= (Self::DOT_DOT as u16)
    }

    /// Returns true if this is a literal token.
    #[inline]
    pub fn is_literal(self) -> bool {
        matches!(self, Self::INTEGER_LIT | Self::REAL_LIT | Self::STRING_LIT)
    }

    /// Lookup a keyword from its text, or return `None` for identifiers.
    pub fn from_keyword(s: &str) -> Option<SyntaxKind> {
        match s {
            "package" => Some(Self::PACKAGE_KW),
            "import" => Some(Self::IMPORT_KW),
            "requirement" => Some(Self::REQUIREMENT_KW),
            "constraint" => Some(Self::CONSTRAINT_KW),
            "def" => Some(Self::DEF_KW),
            "satisfy" => Some(Self::SATISFY_KW),
            "verify" => Some(Self::VERIFY_KW),
            "refine" => Some(Self::REFINE_KW),
            "by" => Some(Self::BY_KW),
            "attribute" => Some(Self::ATTRIBUTE_KW),
            "subject" => Some(Self::SUBJECT_KW),
            "doc" => Some(Self::DOC_KW),
            "part" => Some(Self::PART_KW),
            "port" => Some(Self::PORT_KW),
            "connection" => Some(Self::CONNECTION_KW),
            "action" => Some(Self::ACTION_KW),
            "item" => Some(Self::ITEM_KW),
            "in" => Some(Self::IN_KW),
            "out" => Some(Self::OUT_KW),
            "inout" => Some(Self::INOUT_KW),
            "ref" => Some(Self::REF_KW),
            "assume" => Some(Self::ASSUME_KW),
            "require" => Some(Self::REQUIRE_KW),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_roundtrip() {
        assert_eq!(
            SyntaxKind::from_keyword("requirement"),
            Some(SyntaxKind::REQUIREMENT_KW)
        );
        assert_eq!(
            SyntaxKind::from_keyword("constraint"),
            Some(SyntaxKind::CONSTRAINT_KW)
        );
        assert_eq!(SyntaxKind::from_keyword("my_ident"), None);
    }

    #[test]
    fn trivia_check() {
        assert!(SyntaxKind::WHITESPACE.is_trivia());
        assert!(SyntaxKind::LINE_COMMENT.is_trivia());
        assert!(SyntaxKind::BLOCK_COMMENT.is_trivia());
        assert!(!SyntaxKind::IDENT.is_trivia());
    }

    #[test]
    fn keyword_check() {
        assert!(SyntaxKind::REQUIREMENT_KW.is_keyword());
        assert!(SyntaxKind::SATISFY_KW.is_keyword());
        assert!(!SyntaxKind::IDENT.is_keyword());
        assert!(!SyntaxKind::SEMICOLON.is_keyword());
    }
}
