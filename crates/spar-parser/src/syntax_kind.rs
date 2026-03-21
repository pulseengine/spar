/// All syntax kinds for the AADL language.
///
/// This enum covers every token and node type in AADL v2.2 (AS5506D).
/// Option B: component categories use generic COMPONENT_TYPE/COMPONENT_IMPL
/// nodes with a COMPONENT_CATEGORY child, rather than per-category variants.
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
    /// Line comment: `-- ...`
    COMMENT,

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
    /// `->`
    ARROW,
    /// `<->` bidirectional connection
    BIDI_ARROW,
    /// `=>`
    FAT_ARROW,
    /// `+=>`
    PLUS_ARROW,
    /// `+`
    PLUS,
    /// `-`
    MINUS,
    /// `*`
    STAR,
    /// `::`
    COLON_COLON,
    /// `{**`
    ANNEX_OPEN,
    /// `**}`
    ANNEX_CLOSE,
    /// `#`
    HASH,
    /// `-[`
    DASH_BRACKET,
    /// `]->`
    BRACKET_ARROW,

    // === Literals ===
    /// Integer literal: `42`, `16#FF#`
    INTEGER_LIT,
    /// Real literal: `3.14`, `1.0e-3`
    REAL_LIT,
    /// String literal: `"hello"`
    STRING_LIT,

    // === Identifiers ===
    /// Identifier: `my_component`
    IDENT,

    // === Keywords ===
    PACKAGE_KW,
    PUBLIC_KW,
    PRIVATE_KW,
    WITH_KW,
    END_KW,
    NONE_KW,
    RENAMES_KW,

    // Component categories
    SYSTEM_KW,
    PROCESS_KW,
    THREAD_KW,
    GROUP_KW,
    PROCESSOR_KW,
    VIRTUAL_KW,
    BUS_KW,
    MEMORY_KW,
    DEVICE_KW,
    SUBPROGRAM_KW,
    DATA_KW,
    ABSTRACT_KW,

    // Structure keywords
    IMPLEMENTATION_KW,
    EXTENDS_KW,
    PROTOTYPES_KW,
    FEATURES_KW,
    FLOWS_KW,
    CONNECTIONS_KW,
    MODES_KW,
    PROPERTIES_KW,
    SUBCOMPONENTS_KW,
    ANNEX_KW,
    CALLS_KW,
    INTERNAL_KW,

    // Feature keywords
    IN_KW,
    OUT_KW,
    PORT_KW,
    EVENT_KW,
    ACCESS_KW,
    PROVIDES_KW,
    REQUIRES_KW,
    FEATURE_KW,
    PARAMETER_KW,
    INVERSE_KW,
    OF_KW,

    // Flow keywords
    FLOW_KW,
    SOURCE_KW,
    SINK_KW,
    PATH_KW,

    // Mode keywords
    INITIAL_KW,
    MODE_KW,
    TRANSITION_KW,

    // Property keywords
    CONSTANT_KW,
    APPLIES_KW,
    TO_KW,
    INHERIT_KW,
    DELTA_KW,
    IS_KW,
    ALL_KW,
    BINDING_KW,
    CLASSIFIER_KW,
    REFERENCE_KW,
    RECORD_KW,
    COMPUTE_KW,
    TYPE_KW,
    SET_KW,
    RANGE_KW,
    UNITS_KW,
    ENUMERATION_KW,
    LIST_KW,
    AADLBOOLEAN_KW,
    AADLINTEGER_KW,
    AADLREAL_KW,
    AADLSTRING_KW,
    PROPERTY_KW,

    // Boolean keywords
    TRUE_KW,
    FALSE_KW,

    // Logical keywords
    NOT_KW,
    AND_KW,
    OR_KW,

    // Other keywords
    REFINED_KW,
    SELF_KW,

    // AADL v2.3 (AS5506D) keywords
    /// `interface` — contextual keyword for feature group type definitions.
    INTERFACE_KW,
    /// `file` — keyword for annex file references: `annex Name {* file("path") *};`
    FILE_KW,

    // === Node kinds ===

    // Top-level
    /// Root node for a source file.
    SOURCE_FILE,
    /// `package Name ... end Name;`
    AADL_PACKAGE,
    /// `public ...`
    PUBLIC_SECTION,
    /// `private ...`
    PRIVATE_SECTION,
    /// `with Pkg1, Pkg2;`
    WITH_CLAUSE,
    /// Simple or dotted name.
    NAME,
    /// Dot-separated qualified name: `Pkg::Name`
    QUALIFIED_NAME,
    /// Comma-separated list of names.
    NAME_LIST,
    /// `properties ...` at package level.
    PACKAGE_PROPERTIES,
    /// `renames ...;`
    RENAMES_CLAUSE,

    // Components
    /// Component type declaration (any category).
    COMPONENT_TYPE,
    /// Component implementation declaration (any category).
    COMPONENT_IMPL,
    /// The category keyword(s): `system`, `thread group`, `virtual processor`, etc.
    COMPONENT_CATEGORY,
    /// `extends ParentType`
    TYPE_EXTENSION,
    /// `extends ParentImpl`
    IMPL_EXTENSION,
    /// Realization reference: the type name in `system implementation TypeName.ImplName`
    REALIZATION,

    // Prototypes
    /// `prototypes ...`
    PROTOTYPE_SECTION,
    /// Single prototype declaration.
    PROTOTYPE,
    /// Prototype binding in parentheses.
    PROTOTYPE_BINDING,
    /// List of prototype bindings: `(name => Category ClassifierRef, ...)`
    PROTOTYPE_BINDING_LIST,

    // Features
    /// `features ...`
    FEATURE_SECTION,
    /// `name : in data port Type;`
    DATA_PORT,
    /// `name : in event port;`
    EVENT_PORT,
    /// `name : in event data port Type;`
    EVENT_DATA_PORT,
    /// `name : in parameter Type;`
    PARAMETER,
    /// `name : provides data access Type;`
    DATA_ACCESS,
    /// `name : requires bus access Type;`
    BUS_ACCESS,
    /// `name : provides subprogram access Type;`
    SUBPROGRAM_ACCESS,
    /// `name : provides subprogram group access Type;`
    SUBPROGRAM_GROUP_ACCESS,
    /// `name : feature group Type;`
    FEATURE_GROUP,
    /// `name : feature;`
    ABSTRACT_FEATURE,
    /// `feature group TypeName ...`
    FEATURE_GROUP_TYPE,
    /// `in`, `out`, or `in out`
    DIRECTION,

    // Subcomponents
    /// `subcomponents ...`
    SUBCOMPONENT_SECTION,
    /// Single subcomponent declaration.
    SUBCOMPONENT,

    // Connections
    /// `connections ...`
    CONNECTION_SECTION,
    /// `name : port src -> dst;`
    PORT_CONNECTION,
    /// `name : access src -> dst;`
    ACCESS_CONNECTION,
    /// `name : feature group src -> dst;`
    FEATURE_GROUP_CONNECTION,
    /// `name : feature src -> dst;`
    FEATURE_CONNECTION,
    /// `name : parameter src -> dst;`
    PARAMETER_CONNECTION,
    /// Reference to a connected element: `sub.port`
    CONNECTED_ELEMENT,

    // Flows
    /// `flows ...` in component type
    FLOW_SPEC_SECTION,
    /// Flow specification: `name : flow source/sink/path ...`
    FLOW_SPEC,
    /// `source`, `sink`, or `path`
    FLOW_KIND,
    /// Reference to a flow endpoint feature.
    FLOW_END,
    /// `flows ...` in component implementation
    FLOW_IMPL_SECTION,
    /// Flow implementation.
    FLOW_IMPL,
    /// End-to-end flow declaration.
    END_TO_END_FLOW,
    /// Segment in a flow implementation.
    FLOW_SEGMENT,

    // Modes
    /// `modes ...`
    MODE_SECTION,
    /// `name : initial mode;`
    MODE,
    /// `name : src -[ trigger ]-> dst;`
    MODE_TRANSITION,
    /// Trigger in a mode transition.
    MODE_TRIGGER,

    // Properties
    /// `properties ...`
    PROPERTY_SECTION,
    /// `prop => value;` or `prop +=> value;`
    PROPERTY_ASSOCIATION,
    /// Property reference: `PropSet::PropName`
    PROPERTY_REF,
    /// `applies to path1, path2`
    APPLIES_TO,
    /// Containment path element.
    CONTAINMENT_PATH,
    /// `in binding (Classifier)`
    IN_BINDING,
    /// `value in modes (m1, m2)`
    MODAL_PROPERTY_VALUE,

    // Property set declarations
    /// `property set Name is ... end Name;`
    PROPERTY_SET,
    /// Property definition within a property set.
    PROPERTY_DEFINITION,
    /// Property type declaration.
    PROPERTY_TYPE,
    /// Property constant.
    PROPERTY_CONSTANT,
    /// Property type declaration: `Name : type enumeration (...);`
    PROPERTY_TYPE_DECL,

    // Property expressions
    /// Any property expression (parent node).
    PROPERTY_EXPRESSION,
    /// Boolean literal value node.
    BOOLEAN_VALUE,
    /// Integer literal value node.
    INTEGER_VALUE,
    /// Real literal value node.
    REAL_VALUE,
    /// String literal value node.
    STRING_VALUE,
    /// Record value: `[ field => value; ... ]`
    RECORD_VALUE,
    /// Record field: `field => value;`
    RECORD_FIELD,
    /// List value: `(val1, val2, ...)`
    LIST_VALUE,
    /// Range value: `min .. max`
    RANGE_VALUE,
    /// `delta` value in a range.
    DELTA_VALUE,
    /// Numeric value with units: `20 ms`
    UNIT_VALUE,
    /// Reference value: `reference (path)`
    REFERENCE_VALUE,
    /// Classifier value: `classifier (path)`
    CLASSIFIER_VALUE,
    /// Computed value: `compute (name)`
    COMPUTED_VALUE,
    /// File reference value in annex: `file("path/to/file")`
    FILE_REFERENCE,

    // Classifier references
    /// Reference to a classifier: `Pkg::Type.Impl`
    CLASSIFIER_REF,

    // Annexes
    /// `annex Name {** ... **};`
    ANNEX_SUBCLAUSE,
    /// Annex library declaration.
    ANNEX_LIBRARY,
    /// Raw text content between `{**` and `**}`.
    ANNEX_TEXT,

    // Arrays
    /// `[size]` array dimension.
    ARRAY_DIMENSION,
    /// Array size expression.
    ARRAY_SIZE,

    // Calls (in subprogram implementations)
    /// `calls ...`
    CALL_SECTION,
    /// Subprogram call sequence.
    CALL_SEQUENCE,
    /// Single subprogram call.
    SUBPROGRAM_CALL,

    // Internal/processor features
    /// `internal features ...`
    INTERNAL_FEATURES_SECTION,
    /// `processor features ...`
    PROCESSOR_FEATURES_SECTION,
    /// Event source.
    EVENT_SOURCE,
    /// Event data source.
    EVENT_DATA_SOURCE,
    /// Port proxy.
    PORT_PROXY,
    /// Subprogram proxy.
    SUBPROGRAM_PROXY,

    // Misc
    /// `refined to` clause.
    REFINED_TO,

    // Keep this last — used for bounds checking.
    #[doc(hidden)]
    __LAST,
}

impl SyntaxKind {
    /// Returns true if this is a trivia token (whitespace or comment).
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, Self::WHITESPACE | Self::COMMENT)
    }

    /// Returns true if this is a keyword.
    #[inline]
    pub fn is_keyword(self) -> bool {
        (self as u16) >= (Self::PACKAGE_KW as u16) && (self as u16) <= (Self::FILE_KW as u16)
    }

    /// Returns true if this is a punctuation token.
    #[inline]
    pub fn is_punct(self) -> bool {
        (self as u16) >= (Self::SEMICOLON as u16) && (self as u16) <= (Self::BRACKET_ARROW as u16)
    }

    /// Returns true if this is a literal token.
    #[inline]
    pub fn is_literal(self) -> bool {
        matches!(self, Self::INTEGER_LIT | Self::REAL_LIT | Self::STRING_LIT)
    }

    /// Returns true if this is a component category keyword.
    #[inline]
    pub fn is_component_category_kw(self) -> bool {
        matches!(
            self,
            Self::SYSTEM_KW
                | Self::PROCESS_KW
                | Self::THREAD_KW
                | Self::PROCESSOR_KW
                | Self::VIRTUAL_KW
                | Self::BUS_KW
                | Self::MEMORY_KW
                | Self::DEVICE_KW
                | Self::SUBPROGRAM_KW
                | Self::DATA_KW
                | Self::ABSTRACT_KW
        )
    }

    /// Lookup a keyword from its text, or return `None` for identifiers.
    pub fn from_keyword(s: &str) -> Option<SyntaxKind> {
        match s {
            "package" => Some(Self::PACKAGE_KW),
            "public" => Some(Self::PUBLIC_KW),
            "private" => Some(Self::PRIVATE_KW),
            "with" => Some(Self::WITH_KW),
            "end" => Some(Self::END_KW),
            "none" => Some(Self::NONE_KW),
            "renames" => Some(Self::RENAMES_KW),
            "system" => Some(Self::SYSTEM_KW),
            "process" => Some(Self::PROCESS_KW),
            "thread" => Some(Self::THREAD_KW),
            "group" => Some(Self::GROUP_KW),
            "processor" => Some(Self::PROCESSOR_KW),
            "virtual" => Some(Self::VIRTUAL_KW),
            "bus" => Some(Self::BUS_KW),
            "memory" => Some(Self::MEMORY_KW),
            "device" => Some(Self::DEVICE_KW),
            "subprogram" => Some(Self::SUBPROGRAM_KW),
            "data" => Some(Self::DATA_KW),
            "abstract" => Some(Self::ABSTRACT_KW),
            "implementation" => Some(Self::IMPLEMENTATION_KW),
            "extends" => Some(Self::EXTENDS_KW),
            "prototypes" => Some(Self::PROTOTYPES_KW),
            "features" => Some(Self::FEATURES_KW),
            "flows" => Some(Self::FLOWS_KW),
            "connections" => Some(Self::CONNECTIONS_KW),
            "modes" => Some(Self::MODES_KW),
            "properties" => Some(Self::PROPERTIES_KW),
            "subcomponents" => Some(Self::SUBCOMPONENTS_KW),
            "annex" => Some(Self::ANNEX_KW),
            "calls" => Some(Self::CALLS_KW),
            "internal" => Some(Self::INTERNAL_KW),
            "in" => Some(Self::IN_KW),
            "out" => Some(Self::OUT_KW),
            "port" => Some(Self::PORT_KW),
            "event" => Some(Self::EVENT_KW),
            "access" => Some(Self::ACCESS_KW),
            "provides" => Some(Self::PROVIDES_KW),
            "requires" => Some(Self::REQUIRES_KW),
            "feature" => Some(Self::FEATURE_KW),
            "parameter" => Some(Self::PARAMETER_KW),
            "inverse" => Some(Self::INVERSE_KW),
            "of" => Some(Self::OF_KW),
            "flow" => Some(Self::FLOW_KW),
            "source" => Some(Self::SOURCE_KW),
            "sink" => Some(Self::SINK_KW),
            "path" => Some(Self::PATH_KW),
            "initial" => Some(Self::INITIAL_KW),
            "mode" => Some(Self::MODE_KW),
            "transition" => Some(Self::TRANSITION_KW),
            "constant" => Some(Self::CONSTANT_KW),
            "applies" => Some(Self::APPLIES_KW),
            "to" => Some(Self::TO_KW),
            "inherit" => Some(Self::INHERIT_KW),
            "delta" => Some(Self::DELTA_KW),
            "is" => Some(Self::IS_KW),
            "all" => Some(Self::ALL_KW),
            "binding" => Some(Self::BINDING_KW),
            "classifier" => Some(Self::CLASSIFIER_KW),
            "reference" => Some(Self::REFERENCE_KW),
            "record" => Some(Self::RECORD_KW),
            "compute" => Some(Self::COMPUTE_KW),
            "type" => Some(Self::TYPE_KW),
            "set" => Some(Self::SET_KW),
            "range" => Some(Self::RANGE_KW),
            "units" => Some(Self::UNITS_KW),
            "enumeration" => Some(Self::ENUMERATION_KW),
            "list" => Some(Self::LIST_KW),
            "aadlboolean" => Some(Self::AADLBOOLEAN_KW),
            "aadlinteger" => Some(Self::AADLINTEGER_KW),
            "aadlreal" => Some(Self::AADLREAL_KW),
            "aadlstring" => Some(Self::AADLSTRING_KW),
            "property" => Some(Self::PROPERTY_KW),
            "true" => Some(Self::TRUE_KW),
            "false" => Some(Self::FALSE_KW),
            "not" => Some(Self::NOT_KW),
            "and" => Some(Self::AND_KW),
            "or" => Some(Self::OR_KW),
            "refined" => Some(Self::REFINED_KW),
            "self" => Some(Self::SELF_KW),
            "interface" => Some(Self::INTERFACE_KW),
            "file" => Some(Self::FILE_KW),
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
        assert_eq!(
            SyntaxKind::from_keyword("system"),
            Some(SyntaxKind::SYSTEM_KW)
        );
        assert_eq!(SyntaxKind::from_keyword("my_ident"), None);
    }

    #[test]
    fn category_check() {
        assert!(SyntaxKind::SYSTEM_KW.is_component_category_kw());
        assert!(SyntaxKind::VIRTUAL_KW.is_component_category_kw());
        assert!(!SyntaxKind::PACKAGE_KW.is_component_category_kw());
    }

    #[test]
    fn trivia_check() {
        assert!(SyntaxKind::WHITESPACE.is_trivia());
        assert!(SyntaxKind::COMMENT.is_trivia());
        assert!(!SyntaxKind::IDENT.is_trivia());
    }
}
