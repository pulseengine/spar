//! EMV2 syntax kinds — every token and node type in the EMV2 annex.
//!
//! Specification: SAE AS5506/1 Annex E (Error Model V2)
//! Reference impl: OSATE2 org.osate.xtext.aadl2.errormodel/ErrorModel.xtext

/// All syntax kinds for the EMV2 annex sublanguage.
///
/// This single enum covers both tokens (leaves) and nodes (inner nodes),
/// following the same Rowan pattern as the core AADL parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(non_camel_case_types)]
#[repr(u16)]
pub enum Emv2Kind {
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
    /// `;`
    SEMICOLON,
    /// `,`
    COMMA,
    /// `.`
    DOT,
    /// `*`
    STAR,
    /// `{`
    L_CURLY,
    /// `}`
    R_CURLY,
    /// `(`
    L_PAREN,
    /// `)`
    R_PAREN,
    /// `[`
    L_BRACK,
    /// `]`
    R_BRACK,
    /// `=>`
    FAT_ARROW,
    /// `->`
    ARROW,
    /// `-[`
    TRANS_OPEN,
    /// `]->`
    TRANS_CLOSE,
    /// `!`
    BANG,
    /// `-`
    MINUS,
    /// `^`
    CARET,
    /// `@`
    AT,

    // === Keyword tokens ===
    ACCESS_KW,
    ALL_KW,
    AND_KW,
    APPLIES_KW,
    BEHAVIOR_KW,
    BINDING_KW,
    BINDINGS_KW,
    COMPONENT_KW,
    COMPOSITE_KW,
    CONNECTION_KW,
    DETECTIONS_KW,
    END_KW,
    EQUIVALENCE_KW,
    ERROR_KW,
    EVENT_KW,
    EVENTS_KW,
    EXTENDS_KW,
    FLOWS_KW,
    IF_KW,
    IN_KW,
    INITIAL_KW,
    MAPPINGS_KW,
    MEMORY_KW,
    MODE_KW,
    NOERROR_KW,
    NOT_KW,
    OR_KW,
    ORLESS_KW,
    ORMORE_KW,
    OTHERS_KW,
    OUT_KW,
    PATH_KW,
    PATHS_KW,
    POINT_KW,
    PROCESSOR_KW,
    PROPAGATION_KW,
    PROPAGATIONS_KW,
    PROPERTIES_KW,
    RECOVER_KW,
    RENAMES_KW,
    REPAIR_KW,
    SAME_KW,
    SET_KW,
    SINK_KW,
    SOURCE_KW,
    STATE_KW,
    STATES_KW,
    TO_KW,
    TRANSFORMATIONS_KW,
    TRANSITIONS_KW,
    TYPE_KW,
    TYPES_KW,
    USE_KW,
    WHEN_KW,
    WITH_KW,

    // === Node kinds ===

    // Root
    EMV2_ROOT,
    EMV2_LIBRARY,
    EMV2_SUBCLAUSE,

    // Error types
    ERROR_TYPES_SECTION,
    TYPE_DEFINITION,
    TYPE_SET_DEFINITION,
    TYPE_SET_CONSTRUCTOR,
    TYPE_SET_ELEMENT,

    // Error behavior state machine
    ERROR_BEHAVIOR_SM,
    ERROR_EVENT,
    REPAIR_EVENT,
    RECOVER_EVENT,
    ERROR_BEHAVIOR_STATE,
    ERROR_BEHAVIOR_TRANSITION,
    TRANSITION_BRANCH,
    BRANCH_VALUE,

    // Error propagation
    ERROR_PROPAGATIONS_SECTION,
    ERROR_PROPAGATION,
    ERROR_SOURCE,
    ERROR_SINK,
    ERROR_PATH,
    PROPAGATION_KIND_NODE,
    FEATURE_OR_PP_REF,

    // Component error behavior
    COMPONENT_ERROR_BEHAVIOR,
    OUTGOING_PROPAGATION_CONDITION,
    ERROR_DETECTION,
    ERROR_STATE_TO_MODE_MAPPING,

    // Composite error behavior
    COMPOSITE_ERROR_BEHAVIOR,
    COMPOSITE_STATE,

    // Connection error
    CONNECTION_ERROR,

    // Propagation paths
    PROPAGATION_PATHS_SECTION,
    PROPAGATION_POINT,
    PROPAGATION_PATH_DECL,
    QUALIFIED_PROPAGATION_POINT,

    // Type transformations and mappings
    TYPE_TRANSFORMATION_SET,
    TYPE_TRANSFORMATION,
    TYPE_MAPPING_SET,
    TYPE_MAPPING,

    // Condition expressions
    OR_EXPRESSION,
    AND_EXPRESSION,
    ORMORE_EXPRESSION,
    ORLESS_EXPRESSION,
    ALL_EXPRESSION,
    CONDITION_ELEMENT,

    // Composite condition
    S_CONDITION_ELEMENT,
    QUALIFIED_ERROR_BEHAVIOR_STATE,
    SUBCOMPONENT_ELEMENT,

    // References
    QEMREF,
    EMV2_PATH,
    EMV2_PATH_ELEMENT,

    // Properties
    EMV2_PROPERTY_ASSOCIATION,
    EMV2_PROPERTIES_SECTION,

    // Use clauses
    USE_TYPES,
    USE_BEHAVIOR,
    USE_MAPPINGS,
    USE_TRANSFORMATIONS,
    USE_TYPE_EQUIVALENCE,

    // Misc
    IF_CONDITION,
    ERROR_CODE_VALUE,
    REPORTING_PORT_REF,
    NOERROR_TYPE_SET,

    #[doc(hidden)]
    __LAST,
}

impl Emv2Kind {
    /// Returns true if this is a trivia token (whitespace or comment).
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, Self::WHITESPACE | Self::COMMENT)
    }

    /// Returns true if this is a keyword token.
    #[inline]
    pub fn is_keyword(self) -> bool {
        (self as u16) >= (Self::ACCESS_KW as u16)
            && (self as u16) <= (Self::WITH_KW as u16)
    }

    /// Returns true if this is a propagation kind keyword.
    #[inline]
    pub fn is_propagation_kind(self) -> bool {
        matches!(
            self,
            Self::PROCESSOR_KW
                | Self::MEMORY_KW
                | Self::CONNECTION_KW
                | Self::BINDING_KW
                | Self::BINDINGS_KW
                | Self::ACCESS_KW
        )
    }

    /// Match keyword text (case-insensitive) to Emv2Kind.
    pub fn from_keyword(text: &str) -> Option<Emv2Kind> {
        let lower: String = text.chars().map(|c| c.to_ascii_lowercase()).collect();
        match lower.as_str() {
            "access" => Some(Self::ACCESS_KW),
            "all" => Some(Self::ALL_KW),
            "and" => Some(Self::AND_KW),
            "applies" => Some(Self::APPLIES_KW),
            "behavior" => Some(Self::BEHAVIOR_KW),
            "binding" => Some(Self::BINDING_KW),
            "bindings" => Some(Self::BINDINGS_KW),
            "component" => Some(Self::COMPONENT_KW),
            "composite" => Some(Self::COMPOSITE_KW),
            "connection" => Some(Self::CONNECTION_KW),
            "detections" => Some(Self::DETECTIONS_KW),
            "end" => Some(Self::END_KW),
            "equivalence" => Some(Self::EQUIVALENCE_KW),
            "error" => Some(Self::ERROR_KW),
            "event" => Some(Self::EVENT_KW),
            "events" => Some(Self::EVENTS_KW),
            "extends" => Some(Self::EXTENDS_KW),
            "flows" => Some(Self::FLOWS_KW),
            "if" => Some(Self::IF_KW),
            "in" => Some(Self::IN_KW),
            "initial" => Some(Self::INITIAL_KW),
            "mappings" => Some(Self::MAPPINGS_KW),
            "memory" => Some(Self::MEMORY_KW),
            "mode" => Some(Self::MODE_KW),
            "noerror" => Some(Self::NOERROR_KW),
            "not" => Some(Self::NOT_KW),
            "or" => Some(Self::OR_KW),
            "orless" => Some(Self::ORLESS_KW),
            "ormore" => Some(Self::ORMORE_KW),
            "others" => Some(Self::OTHERS_KW),
            "out" => Some(Self::OUT_KW),
            "path" => Some(Self::PATH_KW),
            "paths" => Some(Self::PATHS_KW),
            "point" => Some(Self::POINT_KW),
            "processor" => Some(Self::PROCESSOR_KW),
            "propagation" => Some(Self::PROPAGATION_KW),
            "propagations" => Some(Self::PROPAGATIONS_KW),
            "properties" => Some(Self::PROPERTIES_KW),
            "recover" => Some(Self::RECOVER_KW),
            "renames" => Some(Self::RENAMES_KW),
            "repair" => Some(Self::REPAIR_KW),
            "same" => Some(Self::SAME_KW),
            "set" => Some(Self::SET_KW),
            "sink" => Some(Self::SINK_KW),
            "source" => Some(Self::SOURCE_KW),
            "state" => Some(Self::STATE_KW),
            "states" => Some(Self::STATES_KW),
            "to" => Some(Self::TO_KW),
            "transformations" => Some(Self::TRANSFORMATIONS_KW),
            "transitions" => Some(Self::TRANSITIONS_KW),
            "type" => Some(Self::TYPE_KW),
            "types" => Some(Self::TYPES_KW),
            "use" => Some(Self::USE_KW),
            "when" => Some(Self::WHEN_KW),
            "with" => Some(Self::WITH_KW),
            _ => None,
        }
    }
}

impl From<Emv2Kind> for rowan::SyntaxKind {
    fn from(kind: Emv2Kind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

// -- Rowan Language impl --

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Emv2Language {}

impl rowan::Language for Emv2Language {
    type Kind = Emv2Kind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Emv2Kind {
        assert!(
            (raw.0 as usize) < Emv2Kind::__LAST as usize,
            "raw Emv2Kind {} out of range",
            raw.0
        );
        // SAFETY: Emv2Kind is repr(u16) and we bounds-checked above.
        unsafe { std::mem::transmute::<u16, Emv2Kind>(raw.0) }
    }

    fn kind_to_raw(kind: Emv2Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type Emv2SyntaxNode = rowan::SyntaxNode<Emv2Language>;
pub type Emv2SyntaxToken = rowan::SyntaxToken<Emv2Language>;
