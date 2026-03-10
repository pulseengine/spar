//! Per-file item tree — a condensed representation of declarations.
//!
//! The item tree strips function bodies and expression details,
//! keeping only the "shape" of each file: packages, component types,
//! component implementations, feature group types, property sets,
//! and their signatures.
//!
//! This is the input to name resolution and is derived from the CST
//! via a salsa tracked function.

pub mod lower;

use la_arena::{Arena, Idx};
use serde::{Deserialize, Serialize};

use crate::name::{ClassifierRef, Name, PropertyRef};

// ── Arena index type aliases ───────────────────────────────────────

pub type PackageIdx = Idx<Package>;
pub type ComponentTypeIdx = Idx<ComponentTypeItem>;
pub type ComponentImplIdx = Idx<ComponentImplItem>;
pub type FeatureGroupTypeIdx = Idx<FeatureGroupTypeItem>;
pub type PropertySetIdx = Idx<PropertySetItem>;
pub type FeatureIdx = Idx<Feature>;
pub type SubcomponentIdx = Idx<SubcomponentItem>;
pub type ConnectionIdx = Idx<ConnectionItem>;
pub type FlowSpecIdx = Idx<FlowSpecItem>;
pub type EndToEndFlowIdx = Idx<EndToEndFlowItem>;
pub type PropertyAssociationIdx = Idx<PropertyAssociationItem>;
pub type ModeIdx = Idx<ModeItem>;
pub type ModeTransitionIdx = Idx<ModeTransitionItem>;
pub type PrototypeIdx = Idx<PrototypeItem>;
pub type PrototypeBindingIdx = Idx<PrototypeBindingItem>;
pub type FlowImplIdx = Idx<FlowImplItem>;
pub type CallSequenceIdx = Idx<CallSequenceItem>;
pub type SubprogramCallIdx = Idx<SubprogramCallItem>;
pub type RenamesIdx = Idx<RenamesItem>;

// ── Lowering diagnostics ──────────────────────────────────────────

/// Diagnostic produced during CST→ItemTree lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringDiagnostic {
    pub message: String,
    pub severity: LoweringSeverity,
}

/// Severity for lowering diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringSeverity {
    Warning,
    Error,
}

// ── Item Tree ──────────────────────────────────────────────────────

/// The item tree for a single source file.
///
/// Contains condensed representations of all top-level and nested
/// declarations, using arena allocation for efficient indexing.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ItemTree {
    pub packages: Arena<Package>,
    pub component_types: Arena<ComponentTypeItem>,
    pub component_impls: Arena<ComponentImplItem>,
    pub feature_group_types: Arena<FeatureGroupTypeItem>,
    pub property_sets: Arena<PropertySetItem>,
    pub features: Arena<Feature>,
    pub subcomponents: Arena<SubcomponentItem>,
    pub connections: Arena<ConnectionItem>,
    pub flow_specs: Arena<FlowSpecItem>,
    pub end_to_end_flows: Arena<EndToEndFlowItem>,
    pub property_associations: Arena<PropertyAssociationItem>,
    pub modes: Arena<ModeItem>,
    pub mode_transitions: Arena<ModeTransitionItem>,
    pub prototypes: Arena<PrototypeItem>,
    pub prototype_bindings: Arena<PrototypeBindingItem>,
    pub flow_impls: Arena<FlowImplItem>,
    pub call_sequences: Arena<CallSequenceItem>,
    pub subprogram_calls: Arena<SubprogramCallItem>,
    pub renames: Arena<RenamesItem>,
    /// Diagnostics produced during lowering (STPA-REQ-002, STPA-REQ-004).
    pub diagnostics: Vec<LoweringDiagnostic>,
}

// ── Top-level items ────────────────────────────────────────────────

/// AADL component category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentCategory {
    System,
    Process,
    Thread,
    ThreadGroup,
    Processor,
    VirtualProcessor,
    Memory,
    Bus,
    VirtualBus,
    Device,
    Subprogram,
    SubprogramGroup,
    Data,
    Abstract,
}

impl std::fmt::Display for ComponentCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::System => "system",
            Self::Process => "process",
            Self::Thread => "thread",
            Self::ThreadGroup => "thread group",
            Self::Processor => "processor",
            Self::VirtualProcessor => "virtual processor",
            Self::Memory => "memory",
            Self::Bus => "bus",
            Self::VirtualBus => "virtual bus",
            Self::Device => "device",
            Self::Subprogram => "subprogram",
            Self::SubprogramGroup => "subprogram group",
            Self::Data => "data",
            Self::Abstract => "abstract",
        };
        f.write_str(s)
    }
}

/// A package declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub name: Name,
    /// Packages imported via `with` clauses.
    pub with_clauses: Vec<Name>,
    /// Items in the public section.
    pub public_items: Vec<ItemRef>,
    /// Items in the private section.
    pub private_items: Vec<ItemRef>,
    /// Renames declarations within this package.
    pub renames: Vec<RenamesIdx>,
}

/// Reference to an item in the item tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemRef {
    ComponentType(ComponentTypeIdx),
    ComponentImpl(ComponentImplIdx),
    FeatureGroupType(FeatureGroupTypeIdx),
    PropertySet(PropertySetIdx),
    AnnexLibrary,
}

/// A component type declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct ComponentTypeItem {
    pub name: Name,
    pub category: ComponentCategory,
    /// Whether this declaration is in the public section of its package.
    /// Defaults to `true` when the section cannot be determined.
    pub is_public: bool,
    pub extends: Option<ClassifierRef>,
    pub features: Vec<FeatureIdx>,
    pub flow_specs: Vec<FlowSpecIdx>,
    pub modes: Vec<ModeIdx>,
    pub mode_transitions: Vec<ModeTransitionIdx>,
    pub prototypes: Vec<PrototypeIdx>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// A component implementation declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct ComponentImplItem {
    /// The type this implements (realization).
    pub type_name: Name,
    /// Implementation-specific name (after the dot).
    pub impl_name: Name,
    pub category: ComponentCategory,
    /// Whether this declaration is in the public section of its package.
    /// Defaults to `true` when the section cannot be determined.
    pub is_public: bool,
    pub extends: Option<ClassifierRef>,
    pub subcomponents: Vec<SubcomponentIdx>,
    pub connections: Vec<ConnectionIdx>,
    pub end_to_end_flows: Vec<EndToEndFlowIdx>,
    pub flow_impls: Vec<FlowImplIdx>,
    pub modes: Vec<ModeIdx>,
    pub mode_transitions: Vec<ModeTransitionIdx>,
    pub prototypes: Vec<PrototypeIdx>,
    pub call_sequences: Vec<CallSequenceIdx>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// A feature group type declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct FeatureGroupTypeItem {
    pub name: Name,
    /// Whether this declaration is in the public section of its package.
    /// Defaults to `true` when the section cannot be determined.
    pub is_public: bool,
    pub extends: Option<ClassifierRef>,
    pub inverse_of: Option<ClassifierRef>,
    pub features: Vec<FeatureIdx>,
    pub prototypes: Vec<PrototypeIdx>,
}

/// A property set declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct PropertySetItem {
    pub name: Name,
    pub property_defs: Vec<PropertyDefItem>,
    pub property_type_defs: Vec<PropertyTypeDefItem>,
    pub property_constants: Vec<PropertyConstantItem>,
}

// ── Nested items ───────────────────────────────────────────────────

/// Direction of a port or access feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    In,
    Out,
    InOut,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::In => f.write_str("in"),
            Self::Out => f.write_str("out"),
            Self::InOut => f.write_str("in out"),
        }
    }
}

/// Kind of feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureKind {
    DataPort,
    EventPort,
    EventDataPort,
    Parameter,
    DataAccess,
    BusAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
}

impl std::fmt::Display for FeatureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DataPort => f.write_str("data port"),
            Self::EventPort => f.write_str("event port"),
            Self::EventDataPort => f.write_str("event data port"),
            Self::Parameter => f.write_str("parameter"),
            Self::DataAccess => f.write_str("data access"),
            Self::BusAccess => f.write_str("bus access"),
            Self::SubprogramAccess => f.write_str("subprogram access"),
            Self::SubprogramGroupAccess => f.write_str("subprogram group access"),
            Self::FeatureGroup => f.write_str("feature group"),
            Self::AbstractFeature => f.write_str("abstract feature"),
        }
    }
}

/// Access kind: provides or requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessKind {
    Provides,
    Requires,
}

impl std::fmt::Display for AccessKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provides => f.write_str("provides"),
            Self::Requires => f.write_str("requires"),
        }
    }
}

/// A feature declaration within a component type or feature group.
#[derive(Debug, PartialEq, Eq)]
pub struct Feature {
    pub name: Name,
    pub kind: FeatureKind,
    pub direction: Option<Direction>,
    /// For access features: provides or requires.
    pub access_kind: Option<AccessKind>,
    /// Classifier reference for the feature's type (if any).
    pub classifier: Option<ClassifierRef>,
    pub is_refined: bool,
    /// Array dimensions (empty if not an array feature).
    pub array_dimensions: Vec<ArrayDimension>,
    /// Property associations on this feature.
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// A subcomponent declaration within a component implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct SubcomponentItem {
    pub name: Name,
    pub category: ComponentCategory,
    pub classifier: Option<ClassifierRef>,
    pub is_refined: bool,
    /// Array dimensions (empty if not an array subcomponent).
    pub array_dimensions: Vec<ArrayDimension>,
    /// Modes in which this subcomponent exists.
    pub in_modes: Vec<Name>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// Kind of connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionKind {
    Port,
    Access,
    FeatureGroup,
    Feature,
    Parameter,
}

/// A reference to a connected element: `subcomponent.feature` or just `feature`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedElementRef {
    /// Subcomponent name (None if the feature is on the containing component itself).
    pub subcomponent: Option<Name>,
    /// Feature/port name.
    pub feature: Name,
}

/// A connection declaration within a component implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct ConnectionItem {
    pub name: Name,
    pub kind: ConnectionKind,
    pub is_bidirectional: bool,
    pub is_refined: bool,
    /// Source endpoint.
    pub src: Option<ConnectedElementRef>,
    /// Destination endpoint.
    pub dst: Option<ConnectedElementRef>,
    /// Modes in which this connection is active.
    pub in_modes: Vec<Name>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// Kind of flow specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowKind {
    Source,
    Sink,
    Path,
}

/// A flow specification declaration within a component type.
#[derive(Debug, PartialEq, Eq)]
pub struct FlowSpecItem {
    pub name: Name,
    pub kind: FlowKind,
    /// For source/sink: the feature endpoint.
    /// For path: the source feature.
    pub source_feature: Option<Name>,
    /// For path: the destination feature.
    pub sink_feature: Option<Name>,
    /// Modes in which this flow spec is active.
    pub in_modes: Vec<Name>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// A flow implementation in a component implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct FlowImplItem {
    pub name: Name,
    pub kind: FlowKind,
    /// Segments of the flow implementation path.
    /// Alternates between subcomponent.flow_spec and connection references.
    pub segments: Vec<FlowSegment>,
    /// Modes in which this flow implementation is active.
    pub in_modes: Vec<Name>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

/// A segment in a flow implementation path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSegment {
    /// Subcomponent name (None if referring to a connection or own feature).
    pub subcomponent: Option<Name>,
    /// The flow spec or connection name.
    pub element: Name,
}

/// An end-to-end flow declaration within a component implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct EndToEndFlowItem {
    pub name: Name,
    /// Segments: alternating subcomponent/flow and connection names.
    pub segments: Vec<Name>,
    /// Modes in which this end-to-end flow is active.
    pub in_modes: Vec<Name>,
    pub property_associations: Vec<PropertyAssociationIdx>,
}

// ── Mode items ─────────────────────────────────────────────────────

/// A mode declaration within a component type or implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct ModeItem {
    pub name: Name,
    /// Whether this is the initial mode of the component.
    pub is_initial: bool,
}

/// A mode transition declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct ModeTransitionItem {
    /// Optional name for the transition.
    pub name: Option<Name>,
    /// Source mode name.
    pub source: Name,
    /// Trigger port/event references.
    pub triggers: Vec<Name>,
    /// Destination mode name.
    pub destination: Name,
}

// ── Prototype items ────────────────────────────────────────────────

/// A prototype declaration within a component type/impl.
#[derive(Debug, PartialEq, Eq)]
pub struct PrototypeItem {
    pub name: Name,
    /// The category constraint (e.g., `data`, `system`).
    pub category: Option<ComponentCategory>,
    /// The constraining classifier reference (if any).
    pub constraining_classifier: Option<ClassifierRef>,
}

/// A prototype binding (actuals provided in extends or subcomponent declarations).
#[derive(Debug, PartialEq, Eq)]
pub struct PrototypeBindingItem {
    /// The formal prototype name being bound.
    pub formal: Name,
    /// The actual classifier reference.
    pub actual: Option<ClassifierRef>,
    /// The actual category (if binding to a category rather than a classifier).
    pub actual_category: Option<ComponentCategory>,
}

// ── Call sequence items ────────────────────────────────────────────

/// A subprogram call sequence in a subprogram/thread implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct CallSequenceItem {
    pub name: Option<Name>,
    /// Individual subprogram calls in this sequence.
    pub calls: Vec<SubprogramCallIdx>,
    /// Modes in which this call sequence is active.
    pub in_modes: Vec<Name>,
}

/// A single subprogram call within a call sequence.
#[derive(Debug, PartialEq, Eq)]
pub struct SubprogramCallItem {
    pub name: Name,
    /// The subprogram being called (classifier reference).
    pub called_subprogram: Option<ClassifierRef>,
}

// ── Renames ────────────────────────────────────────────────────────

/// Kind of renames declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RenamesKind {
    /// `alias renames package Other_Package;`
    Package,
    /// `alias renames <category> Pkg::Classifier;`
    Classifier,
    /// `alias renames feature group Pkg::FGT;`
    FeatureGroup,
}

/// A renames declaration within a package (AS5506 section 4.2).
///
/// Example: `OtherPkg renames package Other_Package;`
#[derive(Debug, PartialEq, Eq)]
pub struct RenamesItem {
    /// The alias name being introduced.
    pub alias: Name,
    /// The original name being aliased.
    pub original: Name,
    /// What kind of entity is being renamed.
    pub kind: RenamesKind,
}

// ── Array dimensions ───────────────────────────────────────────────

/// An array dimension on a feature or subcomponent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayDimension {
    /// The size of the dimension (None if unspecified).
    pub size: Option<ArraySize>,
}

/// Size specification for an array dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArraySize {
    /// A literal integer size.
    Literal(u64),
    /// A reference to a property constant.
    PropertyConstant(PropertyRef),
}

// ── Property expression types (T3: typed property values) ──────────

/// A typed property expression, replacing opaque string values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyExpr {
    /// Integer literal, possibly with units.
    Integer(i64, Option<Name>),
    /// Real literal, possibly with units (stored as string to preserve precision).
    Real(String, Option<Name>),
    /// String literal.
    StringLit(String),
    /// Boolean value.
    Boolean(bool),
    /// Enumeration literal.
    Enum(Name),
    /// List of property expressions.
    List(Vec<PropertyExpr>),
    /// Record: field name → value.
    Record(Vec<(Name, PropertyExpr)>),
    /// Range: min .. max [delta d].
    Range {
        min: Box<PropertyExpr>,
        max: Box<PropertyExpr>,
        delta: Option<Box<PropertyExpr>>,
    },
    /// Classifier reference value: `classifier (Pkg::Type)`.
    ClassifierValue(ClassifierRef),
    /// Reference value: `reference (path)`.
    ReferenceValue(String),
    /// Computed value: `compute (name)`.
    ComputedValue(Name),
    /// Value with unit: wraps another expr with an explicit unit.
    UnitValue(Box<PropertyExpr>, Name),
    /// Unparsed/opaque value (fallback for expressions not yet typed).
    Opaque(String),
}

/// Property type definition as declared in a property set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyTypeDef {
    /// `aadlinteger [range low .. high] [units UnitType]`
    AadlInteger {
        range: Option<(i64, i64)>,
        units: Option<Name>,
    },
    /// `aadlreal [range low .. high] [units UnitType]`
    AadlReal {
        range: Option<(String, String)>,
        units: Option<Name>,
    },
    /// `aadlstring`
    AadlString,
    /// `aadlboolean`
    AadlBoolean,
    /// `enumeration (val1, val2, ...)`
    Enumeration(Vec<Name>),
    /// `range of NumericType`
    Range(Box<PropertyTypeDef>),
    /// `classifier (Category)`
    Classifier(Option<ComponentCategory>),
    /// `reference (Category)`
    Reference(Option<ComponentCategory>),
    /// `record (field1: Type1; ...)`
    RecordType(Vec<(Name, PropertyTypeDef)>),
    /// `type TypeName` — reference to a named type
    TypeRef(Name),
    /// `list of ElementType`
    ListOf(Box<PropertyTypeDef>),
    /// Units type: `units (base_unit, derived_unit => base * factor, ...)`
    UnitsType(Vec<(Name, Option<(Name, String)>)>),
}

/// A property definition within a property set (enriched with type info).
#[derive(Debug, PartialEq, Eq)]
pub struct PropertyDefItem {
    pub name: Name,
    /// The declared property type (None if not yet parsed).
    pub type_def: Option<PropertyTypeDef>,
    /// Default value expression (if any).
    pub default_value: Option<PropertyExpr>,
    /// What this property applies to (empty = applies to all).
    pub applies_to: Vec<AppliesToKind>,
}

/// What a property definition applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppliesToKind {
    All,
    Category(ComponentCategory),
    FeatureKind(FeatureKind),
    Connection,
    Flow,
    Mode,
    Port,
    Access,
    Named(Name),
}

/// A property type definition within a property set.
#[derive(Debug, PartialEq, Eq)]
pub struct PropertyTypeDefItem {
    pub name: Name,
    /// The type being defined.
    pub type_def: Option<PropertyTypeDef>,
}

/// A property constant within a property set.
#[derive(Debug, PartialEq, Eq)]
pub struct PropertyConstantItem {
    pub name: Name,
    /// The type of the constant.
    pub type_def: Option<PropertyTypeDef>,
    /// The constant value.
    pub value: Option<PropertyExpr>,
}

/// A property association (`prop => value;` or `prop +=> value;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyAssociationItem {
    /// The property reference (possibly qualified with a property set).
    pub name: PropertyRef,
    /// Raw text of the property value expression.
    pub value: String,
    /// Typed property expression (None if not yet parsed).
    pub typed_value: Option<PropertyExpr>,
    /// Whether this is an append association (`+=>`).
    pub is_append: bool,
    /// Optional `applies to` path (e.g., `sub1.feat1`).
    pub applies_to: Option<String>,
    /// Modes in which this property value applies.
    pub in_modes: Vec<Name>,
}
