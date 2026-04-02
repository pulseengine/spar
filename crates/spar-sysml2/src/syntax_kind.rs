/// All syntax kinds for the SysML v2 language.
///
/// This enum covers every token and node type needed for SysML v2 parsing.
/// Follows the same repr(u16) + rowan pattern as spar-parser's SyntaxKind.
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

    // === Punctuation ===
    /// `;`
    SEMICOLON,
    /// `:`
    COLON,
    /// `,`
    COMMA,
    /// `.`
    DOT,
    /// `..`
    DOT_DOT,
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
    /// `=>`
    FAT_ARROW,
    /// `:>`
    COLON_GT,
    /// `:>>`
    COLON_GT_GT,
    /// `~>`
    TILDE_GT,
    /// `::`
    COLON_COLON,
    /// `*`
    STAR,
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `/`
    SLASH,
    /// `>`
    GT,
    /// `<`
    LT,
    /// `>=`
    GT_EQ,
    /// `<=`
    LT_EQ,
    /// `==`
    EQ_EQ,
    /// `!=`
    BANG_EQ,
    /// `..`  (already DOT_DOT, but we keep it for clarity in ranges)
    /// `#`
    HASH,
    /// `@`
    AT,

    // === Literals ===
    /// Integer literal: `42`
    INTEGER_LIT,
    /// Real literal: `3.14`
    REAL_LIT,
    /// String literal: `"hello"`
    STRING_LIT,
    /// Boolean literal: `true` or `false`
    TRUE_KW,
    FALSE_KW,

    // === Identifiers ===
    /// Identifier: `myPart`
    IDENT,

    // === Keywords ===
    // -- Definition keywords --
    PACKAGE_KW,
    IMPORT_KW,
    PART_KW,
    PORT_KW,
    CONNECTION_KW,
    ACTION_KW,
    STATE_KW,
    REQUIREMENT_KW,
    CONSTRAINT_KW,
    INTERFACE_KW,
    ATTRIBUTE_KW,
    ITEM_KW,
    ENUM_KW,
    ALLOCATION_KW,
    ANALYSIS_KW,
    CASE_KW,
    CALC_KW,
    CONCERN_KW,
    DECIDE_KW,
    DEF_KW,
    DOC_KW,
    ENTRY_KW,
    EXHIBIT_KW,
    EXIT_KW,
    EXPOSE_KW,
    FLOW_KW,
    FORK_KW,
    FILTER_KW,
    FIRST_KW,
    IF_KW,
    IN_KW,
    INOUT_KW,
    JOIN_KW,
    MERGE_KW,
    METADATA_KW,
    OCCURRENCE_KW,
    OUT_KW,
    PERFORM_KW,
    PRIVATE_KW,
    PROTECTED_KW,
    PUBLIC_KW,
    READONLY_KW,
    REDEFINES_KW,
    REF_KW,
    RENDERING_KW,
    RETURN_KW,
    SATISFY_KW,
    VERIFY_KW,
    REFINE_KW,
    ALLOCATE_KW,
    DERIVE_KW,
    SEND_KW,
    SNAPSHOT_KW,
    SPECIALIZES_KW,
    STAKEHOLDER_KW,
    SUBJECT_KW,
    SUCCESSION_KW,
    THEN_KW,
    TIMESLICE_KW,
    TO_KW,
    TRANSITION_KW,
    USE_KW,
    VARIANT_KW,
    VERIFICATION_KW,
    VIEW_KW,
    VIEWPOINT_KW,
    ABSTRACT_KW,
    ALIAS_KW,
    ALL_KW,
    AND_KW,
    AS_KW,
    ASSIGN_KW,
    ASSOC_KW,
    BIND_KW,
    BY_KW,
    CHAINS_KW,
    COMMENT_KW,
    CONNECT_KW,
    DEPENDENCY_KW,
    DERIVED_KW,
    DIFFERENCES_KW,
    DISJOINING_KW,
    DISJOINT_KW,
    ELSE_KW,
    END_KW,
    FEATURE_KW,
    FEATURING_KW,
    FROM_KW,
    HASTYPE_KW,
    IMPLIES_KW,
    INDIVIDUAL_KW,
    INTERSECTING_KW,
    INTERSECTS_KW,
    ISTYPE_KW,
    LANGUAGE_KW,
    LIBRARY_KW,
    LOCALE_KW,
    LOOP_KW,
    MESSAGE_KW,
    MULTIPLICITY_KW,
    NAMESPACE_KW,
    NONUNIQUE_KW,
    NOT_KW,
    NULL_KW,
    OF_KW,
    OR_KW,
    ORDERED_KW,
    PARALLEL_KW,
    PORTION_KW,
    PREDICATE_KW,
    RECEPTION_KW,
    RELATIONSHIP_KW,
    REP_KW,
    STRUCT_KW,
    SUBCLASSIFIER_KW,
    SUBSET_KW,
    SUBSETS_KW,
    SUPERSET_KW,
    SUPERSETS_KW,
    TYPED_KW,
    TYPING_KW,
    UNIONING_KW,
    UNIONS_KW,
    WHILE_KW,
    XOR_KW,

    // === Node kinds ===

    // Top-level
    /// Root node for a source file.
    SOURCE_FILE,

    // Packages
    /// `package Name { ... }`
    PACKAGE,
    /// `import Pkg::*;`
    IMPORT_DECL,
    /// Namespace body: list of members inside `{ ... }`
    NAMESPACE_BODY,

    // Names and references
    /// Simple or qualified name.
    NAME,
    /// Qualified name: `Pkg::Sub::Name`
    QUALIFIED_NAME,
    /// Dotted feature chain: `a.b.c`
    FEATURE_CHAIN,

    // Definitions
    /// `part def Name { ... }`
    PART_DEF,
    /// `port def Name { ... }`
    PORT_DEF,
    /// `connection def Name { ... }`
    CONNECTION_DEF,
    /// `action def Name { ... }`
    ACTION_DEF,
    /// `state def Name { ... }`
    STATE_DEF,
    /// `attribute def Name { ... }`
    ATTRIBUTE_DEF,
    /// `item def Name { ... }`
    ITEM_DEF,
    /// `interface def Name { ... }`
    INTERFACE_DEF,
    /// `enum def Name { ... }`
    ENUM_DEF,
    /// `requirement def Name { ... }`
    REQUIREMENT_DEF,
    /// `constraint def Name { ... }`
    CONSTRAINT_DEF,
    /// `calc def Name { ... }`
    CALC_DEF,
    /// `allocation def Name { ... }`
    ALLOCATION_DEF,

    // Usages
    /// `part name : Type { ... }`
    PART_USAGE,
    /// `port name : Type;`
    PORT_USAGE,
    /// `connect a.p to b.p;`
    CONNECTION_USAGE,
    /// `action name { ... }`
    ACTION_USAGE,
    /// `state name { ... }`
    STATE_USAGE,
    /// `attribute name : Type;`
    ATTRIBUTE_USAGE,
    /// `item name : Type;`
    ITEM_USAGE,
    /// `ref part name : Type;`
    REF_USAGE,
    /// `interface name { ... }`
    INTERFACE_USAGE,
    /// `enum name { ... }`
    ENUM_USAGE,

    /// `requirement name : Type { ... }`
    REQUIREMENT_USAGE,
    /// `constraint name : Type { ... }`
    CONSTRAINT_USAGE,

    // Relationships
    /// `satisfy req by impl;`
    SATISFY_REQ,
    /// `verify req by test;`
    VERIFY_REQ,
    /// `refine req1 by req2;`
    REFINE_REQ,
    /// `allocate task to processor;`
    ALLOCATE_REQ,
    /// `derive req1 from req2;`
    DERIVE_REQ,

    // Requirement body members
    /// `subject name : Type;`
    SUBJECT_MEMBER,
    /// `doc /* text */` inside requirement body
    DOC_MEMBER,
    /// Name reference in expression/relationship context
    NAME_REF,

    // Feature declarations
    /// Generic feature declaration: `name : Type;`
    FEATURE_DECL,

    // Specialization / typing
    /// `:>` specialization: `part v :> Vehicle;`
    SPECIALIZATION,
    /// `:` typing: `part v : Vehicle;`
    TYPING,
    /// Conjugated port prefix `~`
    CONJUGATION,

    // Multiplicity
    /// `[0..*]`
    MULTIPLICITY,

    // Connections
    /// `connect a to b` endpoint list
    CONNECT_ENDPOINT,

    // Direction
    /// `in`, `out`, or `inout`
    DIRECTION,

    // Annotations / comments / docs
    /// `comment /* ... */`
    COMMENT_NODE,
    /// `doc /* ... */`
    DOC_NODE,

    // Value expressions
    /// Literal expression
    LITERAL_EXPR,
    /// Operator expression
    OPERATOR_EXPR,
    /// Feature reference expression
    FEATURE_REF_EXPR,

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
        (self as u16) >= (Self::PACKAGE_KW as u16) && (self as u16) <= (Self::XOR_KW as u16)
    }

    /// Returns true if this is a punctuation token.
    #[inline]
    pub fn is_punct(self) -> bool {
        (self as u16) >= (Self::SEMICOLON as u16) && (self as u16) <= (Self::AT as u16)
    }

    /// Returns true if this is a literal token.
    #[inline]
    pub fn is_literal(self) -> bool {
        matches!(self, Self::INTEGER_LIT | Self::REAL_LIT | Self::STRING_LIT)
    }

    /// Returns true if this kind starts a definition keyword
    /// (part, port, connection, action, state, attribute, item, interface, enum, etc.).
    #[inline]
    pub fn is_definition_kw(self) -> bool {
        matches!(
            self,
            Self::PART_KW
                | Self::PORT_KW
                | Self::CONNECTION_KW
                | Self::ACTION_KW
                | Self::STATE_KW
                | Self::ATTRIBUTE_KW
                | Self::ITEM_KW
                | Self::INTERFACE_KW
                | Self::ENUM_KW
                | Self::REQUIREMENT_KW
                | Self::CONSTRAINT_KW
                | Self::CALC_KW
                | Self::ALLOCATION_KW
        )
    }

    /// Lookup a keyword from its text, or return `None` for identifiers.
    pub fn from_keyword(s: &str) -> Option<SyntaxKind> {
        match s {
            "package" => Some(Self::PACKAGE_KW),
            "import" => Some(Self::IMPORT_KW),
            "part" => Some(Self::PART_KW),
            "port" => Some(Self::PORT_KW),
            "connection" => Some(Self::CONNECTION_KW),
            "action" => Some(Self::ACTION_KW),
            "state" => Some(Self::STATE_KW),
            "requirement" => Some(Self::REQUIREMENT_KW),
            "constraint" => Some(Self::CONSTRAINT_KW),
            "interface" => Some(Self::INTERFACE_KW),
            "attribute" => Some(Self::ATTRIBUTE_KW),
            "item" => Some(Self::ITEM_KW),
            "enum" => Some(Self::ENUM_KW),
            "allocation" => Some(Self::ALLOCATION_KW),
            "analysis" => Some(Self::ANALYSIS_KW),
            "case" => Some(Self::CASE_KW),
            "calc" => Some(Self::CALC_KW),
            "concern" => Some(Self::CONCERN_KW),
            "decide" => Some(Self::DECIDE_KW),
            "def" => Some(Self::DEF_KW),
            "doc" => Some(Self::DOC_KW),
            "entry" => Some(Self::ENTRY_KW),
            "exhibit" => Some(Self::EXHIBIT_KW),
            "exit" => Some(Self::EXIT_KW),
            "expose" => Some(Self::EXPOSE_KW),
            "flow" => Some(Self::FLOW_KW),
            "fork" => Some(Self::FORK_KW),
            "filter" => Some(Self::FILTER_KW),
            "first" => Some(Self::FIRST_KW),
            "if" => Some(Self::IF_KW),
            "in" => Some(Self::IN_KW),
            "inout" => Some(Self::INOUT_KW),
            "join" => Some(Self::JOIN_KW),
            "merge" => Some(Self::MERGE_KW),
            "metadata" => Some(Self::METADATA_KW),
            "occurrence" => Some(Self::OCCURRENCE_KW),
            "out" => Some(Self::OUT_KW),
            "perform" => Some(Self::PERFORM_KW),
            "private" => Some(Self::PRIVATE_KW),
            "protected" => Some(Self::PROTECTED_KW),
            "public" => Some(Self::PUBLIC_KW),
            "readonly" => Some(Self::READONLY_KW),
            "redefines" => Some(Self::REDEFINES_KW),
            "ref" => Some(Self::REF_KW),
            "rendering" => Some(Self::RENDERING_KW),
            "return" => Some(Self::RETURN_KW),
            "satisfy" => Some(Self::SATISFY_KW),
            "verify" => Some(Self::VERIFY_KW),
            "refine" => Some(Self::REFINE_KW),
            "allocate" => Some(Self::ALLOCATE_KW),
            "derive" => Some(Self::DERIVE_KW),
            "send" => Some(Self::SEND_KW),
            "snapshot" => Some(Self::SNAPSHOT_KW),
            "specializes" => Some(Self::SPECIALIZES_KW),
            "stakeholder" => Some(Self::STAKEHOLDER_KW),
            "subject" => Some(Self::SUBJECT_KW),
            "succession" => Some(Self::SUCCESSION_KW),
            "then" => Some(Self::THEN_KW),
            "timeslice" => Some(Self::TIMESLICE_KW),
            "to" => Some(Self::TO_KW),
            "transition" => Some(Self::TRANSITION_KW),
            "use" => Some(Self::USE_KW),
            "variant" => Some(Self::VARIANT_KW),
            "verification" => Some(Self::VERIFICATION_KW),
            "view" => Some(Self::VIEW_KW),
            "viewpoint" => Some(Self::VIEWPOINT_KW),
            "abstract" => Some(Self::ABSTRACT_KW),
            "alias" => Some(Self::ALIAS_KW),
            "all" => Some(Self::ALL_KW),
            "and" => Some(Self::AND_KW),
            "as" => Some(Self::AS_KW),
            "assign" => Some(Self::ASSIGN_KW),
            "assoc" => Some(Self::ASSOC_KW),
            "bind" => Some(Self::BIND_KW),
            "by" => Some(Self::BY_KW),
            "chains" => Some(Self::CHAINS_KW),
            "comment" => Some(Self::COMMENT_KW),
            "connect" => Some(Self::CONNECT_KW),
            "dependency" => Some(Self::DEPENDENCY_KW),
            "derived" => Some(Self::DERIVED_KW),
            "differences" => Some(Self::DIFFERENCES_KW),
            "disjoining" => Some(Self::DISJOINING_KW),
            "disjoint" => Some(Self::DISJOINT_KW),
            "else" => Some(Self::ELSE_KW),
            "end" => Some(Self::END_KW),
            "feature" => Some(Self::FEATURE_KW),
            "featuring" => Some(Self::FEATURING_KW),
            "from" => Some(Self::FROM_KW),
            "hastype" => Some(Self::HASTYPE_KW),
            "implies" => Some(Self::IMPLIES_KW),
            "individual" => Some(Self::INDIVIDUAL_KW),
            "intersecting" => Some(Self::INTERSECTING_KW),
            "intersects" => Some(Self::INTERSECTS_KW),
            "istype" => Some(Self::ISTYPE_KW),
            "language" => Some(Self::LANGUAGE_KW),
            "library" => Some(Self::LIBRARY_KW),
            "locale" => Some(Self::LOCALE_KW),
            "loop" => Some(Self::LOOP_KW),
            "message" => Some(Self::MESSAGE_KW),
            "multiplicity" => Some(Self::MULTIPLICITY_KW),
            "namespace" => Some(Self::NAMESPACE_KW),
            "nonunique" => Some(Self::NONUNIQUE_KW),
            "not" => Some(Self::NOT_KW),
            "null" => Some(Self::NULL_KW),
            "of" => Some(Self::OF_KW),
            "or" => Some(Self::OR_KW),
            "ordered" => Some(Self::ORDERED_KW),
            "parallel" => Some(Self::PARALLEL_KW),
            "portion" => Some(Self::PORTION_KW),
            "predicate" => Some(Self::PREDICATE_KW),
            "reception" => Some(Self::RECEPTION_KW),
            "relationship" => Some(Self::RELATIONSHIP_KW),
            "rep" => Some(Self::REP_KW),
            "struct" => Some(Self::STRUCT_KW),
            "subclassifier" => Some(Self::SUBCLASSIFIER_KW),
            "subset" => Some(Self::SUBSET_KW),
            "subsets" => Some(Self::SUBSETS_KW),
            "superset" => Some(Self::SUPERSET_KW),
            "supersets" => Some(Self::SUPERSETS_KW),
            "typed" => Some(Self::TYPED_KW),
            "typing" => Some(Self::TYPING_KW),
            "unioning" => Some(Self::UNIONING_KW),
            "unions" => Some(Self::UNIONS_KW),
            "while" => Some(Self::WHILE_KW),
            "xor" => Some(Self::XOR_KW),
            "true" => Some(Self::TRUE_KW),
            "false" => Some(Self::FALSE_KW),
            _ => None,
        }
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_roundtrip() {
        assert_eq!(
            SyntaxKind::from_keyword("package"),
            Some(SyntaxKind::PACKAGE_KW)
        );
        assert_eq!(SyntaxKind::from_keyword("part"), Some(SyntaxKind::PART_KW));
        assert_eq!(SyntaxKind::from_keyword("port"), Some(SyntaxKind::PORT_KW));
        assert_eq!(SyntaxKind::from_keyword("def"), Some(SyntaxKind::DEF_KW));
        assert_eq!(
            SyntaxKind::from_keyword("connect"),
            Some(SyntaxKind::CONNECT_KW)
        );
        assert_eq!(SyntaxKind::from_keyword("myIdent"), None);
    }

    #[test]
    fn definition_kw_check() {
        assert!(SyntaxKind::PART_KW.is_definition_kw());
        assert!(SyntaxKind::PORT_KW.is_definition_kw());
        assert!(SyntaxKind::CONNECTION_KW.is_definition_kw());
        assert!(!SyntaxKind::PACKAGE_KW.is_definition_kw());
        assert!(!SyntaxKind::IMPORT_KW.is_definition_kw());
    }

    #[test]
    fn trivia_check() {
        assert!(SyntaxKind::WHITESPACE.is_trivia());
        assert!(SyntaxKind::LINE_COMMENT.is_trivia());
        assert!(SyntaxKind::BLOCK_COMMENT.is_trivia());
        assert!(!SyntaxKind::IDENT.is_trivia());
    }
}
