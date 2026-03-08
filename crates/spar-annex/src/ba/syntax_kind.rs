//! BA syntax kinds -- every token and node type in the Behavior Annex.
//!
//! Specification: SAE AS5506/2 Annex D (Behavior Annex)
//! Reference impl: OSATE2 org.osate.ba

/// All syntax kinds for the Behavior Annex sublanguage.
///
/// This single enum covers both tokens (leaves) and nodes (inner nodes),
/// following the same Rowan pattern as the core AADL parser and EMV2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(non_camel_case_types)]
#[repr(u16)]
pub enum BaKind {
    // === Meta ===
    TOMBSTONE = 0,
    EOF,
    ERROR,

    // === Trivia tokens ===
    WHITESPACE,
    /// Line comment: `-- ...`
    COMMENT,

    // === Literal tokens ===
    IDENT,
    STRING_LIT,
    INT_LIT,
    REAL_LIT,

    // === Punctuation tokens ===
    /// `:`
    COLON,
    /// `::`
    COLON_COLON,
    /// `:=`
    COLON_EQ,
    /// `;`
    SEMICOLON,
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
    L_BRACK,
    /// `]`
    R_BRACK,
    /// `-[`
    TRANS_OPEN,
    /// `]->`
    TRANS_CLOSE,
    /// `=>`
    FAT_ARROW,
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `*`
    STAR,
    /// `**`
    STAR_STAR,
    /// `/`
    SLASH,
    /// `=`
    EQ,
    /// `!=`
    BANG_EQ,
    /// `<`
    L_ANGLE,
    /// `<=`
    L_ANGLE_EQ,
    /// `>`
    R_ANGLE,
    /// `>=`
    R_ANGLE_EQ,
    /// `!`
    BANG,
    /// `?`
    QUESTION,
    /// `>>`
    R_ANGLE_R_ANGLE,
    /// `!<`
    BANG_L_ANGLE,
    /// `!>`
    BANG_R_ANGLE,
    /// `&`
    AMP,
    /// `'` (tick for port properties like `port'count`)
    TICK,
    /// `#` (for property references)
    HASH,
    /// `->`
    ARROW,

    // === Keyword tokens ===
    ABS_KW,
    AND_KW,
    ANY_KW,
    BINDING_KW,
    CLASSIFIER_KW,
    COMPLETE_KW,
    COMPUTATION_KW,
    COUNT_KW,
    DISPATCH_KW,
    DO_KW,
    ELSE_KW,
    ELSIF_KW,
    END_KW,
    FALSE_KW,
    FINAL_KW,
    FOR_KW,
    FORALL_KW,
    FRESH_KW,
    FROZEN_KW,
    IF_KW,
    IN_KW,
    INITIAL_KW,
    LOWER_BOUND_KW,
    MOD_KW,
    NOT_KW,
    ON_KW,
    OR_KW,
    OTHERWISE_KW,
    REFERENCE_KW,
    REM_KW,
    STATE_KW,
    STATES_KW,
    STOP_KW,
    TIMEOUT_KW,
    TRANSITIONS_KW,
    TRUE_KW,
    UNTIL_KW,
    UPPER_BOUND_KW,
    VARIABLES_KW,
    WHILE_KW,
    XOR_KW,

    // === Node kinds ===

    // Root
    BA_ROOT,

    // Sections
    BEHAVIOR_VARIABLES_SECTION,
    BEHAVIOR_STATES_SECTION,
    BEHAVIOR_TRANSITIONS_SECTION,

    // Declarations
    BEHAVIOR_VARIABLE,
    BEHAVIOR_STATE,
    BEHAVIOR_TRANSITION,

    // State qualifiers
    STATE_KIND_LIST,

    // Transition parts
    SOURCE_STATE_LIST,
    DISPATCH_CONDITION,
    EXECUTE_CONDITION,
    TRANSITION_GUARD,

    // Dispatch condition elements
    DISPATCH_TRIGGER_LOGICAL_EXPR,
    DISPATCH_TRIGGER_CONJUNCTION,
    FROZEN_PORT_LIST,

    // Action block
    ACTION_BLOCK,
    BEHAVIOR_ACTIONS,
    BEHAVIOR_ACTION,

    // Actions
    ASSIGNMENT_ACTION,
    COMMUNICATION_ACTION,
    COMPUTATION_ACTION,
    TIMED_ACTION,

    // Control flow
    IF_STATEMENT,
    ELSIF_CLAUSE,
    ELSE_CLAUSE,
    FOR_STATEMENT,
    FORALL_STATEMENT,
    WHILE_STATEMENT,
    DO_UNTIL_STATEMENT,

    // Expressions
    VALUE_EXPRESSION,
    BINARY_EXPR,
    UNARY_EXPR,
    PAREN_EXPR,

    // References
    NAME_REF,
    QUALIFIED_NAME,
    ARRAY_INDEX,
    PORT_PROPERTY_REF,
    PROPERTY_REF,
    CLASSIFIER_REF,
    COMPONENT_REF,

    // Subprogram call params
    SUBPROGRAM_CALL_PARAMS,

    // Timeout clause
    TIMEOUT_CLAUSE,

    // Range
    RANGE_EXPR,

    // Literal nodes
    BOOLEAN_LITERAL,
    INTEGER_LITERAL,
    REAL_LITERAL,
    STRING_LITERAL,

    #[doc(hidden)]
    __LAST,
}

impl BaKind {
    /// Returns true if this is a trivia token (whitespace or comment).
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, Self::WHITESPACE | Self::COMMENT)
    }

    /// Returns true if this is a keyword token.
    #[inline]
    pub fn is_keyword(self) -> bool {
        (self as u16) >= (Self::ABS_KW as u16) && (self as u16) <= (Self::XOR_KW as u16)
    }

    /// Match keyword text (case-insensitive) to BaKind.
    pub fn from_keyword(text: &str) -> Option<BaKind> {
        let lower: String = text.chars().map(|c| c.to_ascii_lowercase()).collect();
        match lower.as_str() {
            "abs" => Some(Self::ABS_KW),
            "and" => Some(Self::AND_KW),
            "any" => Some(Self::ANY_KW),
            "binding" => Some(Self::BINDING_KW),
            "classifier" => Some(Self::CLASSIFIER_KW),
            "complete" => Some(Self::COMPLETE_KW),
            "computation" => Some(Self::COMPUTATION_KW),
            "count" => Some(Self::COUNT_KW),
            "dispatch" => Some(Self::DISPATCH_KW),
            "do" => Some(Self::DO_KW),
            "else" => Some(Self::ELSE_KW),
            "elsif" => Some(Self::ELSIF_KW),
            "end" => Some(Self::END_KW),
            "false" => Some(Self::FALSE_KW),
            "final" => Some(Self::FINAL_KW),
            "for" => Some(Self::FOR_KW),
            "forall" => Some(Self::FORALL_KW),
            "fresh" => Some(Self::FRESH_KW),
            "frozen" => Some(Self::FROZEN_KW),
            "if" => Some(Self::IF_KW),
            "in" => Some(Self::IN_KW),
            "initial" => Some(Self::INITIAL_KW),
            "lower_bound" => Some(Self::LOWER_BOUND_KW),
            "mod" => Some(Self::MOD_KW),
            "not" => Some(Self::NOT_KW),
            "on" => Some(Self::ON_KW),
            "or" => Some(Self::OR_KW),
            "otherwise" => Some(Self::OTHERWISE_KW),
            "reference" => Some(Self::REFERENCE_KW),
            "rem" => Some(Self::REM_KW),
            "state" => Some(Self::STATE_KW),
            "states" => Some(Self::STATES_KW),
            "stop" => Some(Self::STOP_KW),
            "timeout" => Some(Self::TIMEOUT_KW),
            "transitions" => Some(Self::TRANSITIONS_KW),
            "true" => Some(Self::TRUE_KW),
            "until" => Some(Self::UNTIL_KW),
            "upper_bound" => Some(Self::UPPER_BOUND_KW),
            "variables" => Some(Self::VARIABLES_KW),
            "while" => Some(Self::WHILE_KW),
            "xor" => Some(Self::XOR_KW),
            _ => None,
        }
    }
}

impl From<BaKind> for rowan::SyntaxKind {
    fn from(kind: BaKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

// -- Rowan Language impl --

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BaLanguage {}

impl rowan::Language for BaLanguage {
    type Kind = BaKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> BaKind {
        assert!(
            (raw.0 as usize) < BaKind::__LAST as usize,
            "raw BaKind {} out of range",
            raw.0
        );
        // SAFETY: BaKind is repr(u16) and we bounds-checked above.
        unsafe { std::mem::transmute::<u16, BaKind>(raw.0) }
    }

    fn kind_to_raw(kind: BaKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type BaSyntaxNode = rowan::SyntaxNode<BaLanguage>;
pub type BaSyntaxToken = rowan::SyntaxToken<BaLanguage>;
