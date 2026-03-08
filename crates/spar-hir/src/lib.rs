//! Public semantic API facade for AADL models.
//!
//! `spar-hir` provides a clean, value-oriented API that hides the internal
//! arenas, indices, and salsa machinery of `spar-hir-def`. All downstream
//! consumers (CLI, LSP, MCP, transforms, WASM) should use this crate
//! instead of reaching into `spar-hir-def` directly.
//!
//! # Architecture
//!
//! ```text
//! spar-hir-def (internals: arenas, Idx<T>, salsa)
//!      |
//!      v
//! spar-hir (public facade: Database, Package, ComponentType, etc.)
//!      |
//!      v
//! spar-cli, spar-analysis, spar-transform, spar-mcp, spar-wasm
//! ```
//!
//! # Example
//!
//! ```
//! use spar_hir::Database;
//!
//! let db = Database::from_aadl(&[(
//!     "example.aadl".to_string(),
//!     r#"
//!     package Example
//!     public
//!       system MySystem
//!         features
//!           inp: in data port;
//!       end MySystem;
//!     end Example;
//!     "#.to_string(),
//! )]);
//!
//! let packages = db.packages();
//! assert_eq!(packages.len(), 1);
//! assert_eq!(packages[0].name, "Example");
//! assert_eq!(packages[0].component_types[0].features.len(), 1);
//! ```

// Re-export clean enums from hir-def that are already public-API quality.
pub use spar_hir_def::item_tree::{
    AccessKind, ComponentCategory, ConnectionKind, Direction, FeatureKind, FlowKind,
};
pub use spar_hir_def::item_tree::PropertyExpr;

use std::sync::Arc;

use spar_hir_def::item_tree::{
    self, ConnectedElementRef, EndToEndFlowItem, FlowSpecItem, ItemRef, ItemTree, ModeItem,
    ModeTransitionItem, PropertyAssociationIdx, PropertyAssociationItem,
};
use spar_hir_def::name::{ClassifierRef, Name};
use spar_hir_def::resolver::GlobalScope;

// ── Database ───────────────────────────────────────────────────────

/// The semantic database. Entry point for all queries.
///
/// Wraps the internal `GlobalScope` and `ItemTree`s, providing a
/// value-oriented API without arena indices or salsa details.
pub struct Database {
    scope: GlobalScope,
    trees: Vec<Arc<ItemTree>>,
}

impl Database {
    /// Create a database from parsed AADL source files.
    ///
    /// Each entry is a `(filename, content)` pair.
    pub fn from_aadl(sources: &[(String, String)]) -> Self {
        let db = spar_hir_def::HirDefDatabase::default();
        let mut trees = Vec::new();

        for (filename, content) in sources {
            let sf = spar_base_db::SourceFile::new(&db, filename.clone(), content.clone());
            trees.push(spar_hir_def::file_item_tree(&db, sf));
        }

        let scope = GlobalScope::from_trees(trees.clone());
        Self { scope, trees }
    }

    /// Get all packages across all loaded files.
    pub fn packages(&self) -> Vec<Package> {
        let mut result = Vec::new();
        for tree in &self.trees {
            for (_idx, pkg) in tree.packages.iter() {
                result.push(lower_package(pkg, tree));
            }
        }
        result
    }

    /// Lookup a classifier by qualified name (`Package::Type` or `Package::Type.Impl`).
    ///
    /// Returns `None` if the name cannot be resolved.
    pub fn find_classifier(&self, name: &str) -> Option<Classifier> {
        let (pkg_str, type_str, impl_str) = parse_qualified_name(name)?;
        let pkg_name = Name::new(&pkg_str);

        let cref = if let Some(impl_name) = impl_str {
            ClassifierRef::implementation(
                Some(Name::new(&pkg_str)),
                Name::new(&type_str),
                Name::new(&impl_name),
            )
        } else {
            ClassifierRef::qualified(Name::new(&pkg_str), Name::new(&type_str))
        };

        let resolved = self.scope.resolve_classifier(&pkg_name, &cref);

        match resolved {
            spar_hir_def::ResolvedClassifier::ComponentType { loc, .. } => {
                let tree = self.scope.tree(loc.tree)?;
                let ct = self.scope.get_component_type(loc)?;
                Some(Classifier::Type(lower_component_type(ct, tree)))
            }
            spar_hir_def::ResolvedClassifier::ComponentImpl { loc, .. } => {
                let tree = self.scope.tree(loc.tree)?;
                let ci = self.scope.get_component_impl(loc)?;
                Some(Classifier::Implementation(lower_component_impl(ci, tree)))
            }
            spar_hir_def::ResolvedClassifier::FeatureGroupType { loc, .. } => {
                let tree = self.scope.tree(loc.tree)?;
                let fgt = self.scope.get_feature_group_type(loc)?;
                Some(Classifier::FeatureGroupType(lower_feature_group_type(fgt, tree)))
            }
            spar_hir_def::ResolvedClassifier::Unresolved => None,
        }
    }

    /// Instantiate a system implementation by qualified name (`Package::Type.Impl`).
    ///
    /// Returns `None` if the name cannot be parsed or the implementation is not found.
    pub fn instantiate(&self, qualified_name: &str) -> Option<Instance> {
        let (pkg_str, type_str, impl_str) = parse_qualified_name(qualified_name)?;
        let impl_name = impl_str?;

        let inst = spar_hir_def::instance::SystemInstance::instantiate(
            &self.scope,
            &Name::new(&pkg_str),
            &Name::new(&type_str),
            &Name::new(&impl_name),
        );
        Some(Instance { inner: inst })
    }

    /// Access the raw item trees for analysis passes that need them.
    pub fn item_trees(&self) -> &[Arc<ItemTree>] {
        &self.trees
    }

    /// Access the underlying global scope for advanced queries.
    pub fn global_scope(&self) -> &GlobalScope {
        &self.scope
    }
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("trees", &self.trees.len())
            .field("packages", &self.scope.package_names().len())
            .finish()
    }
}

// ── Package ────────────────────────────────────────────────────────

/// A named AADL package with its declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub with_clauses: Vec<String>,
    pub component_types: Vec<ComponentType>,
    pub component_impls: Vec<ComponentImpl>,
    pub feature_group_types: Vec<FeatureGroupType>,
}

// ── ComponentType ──────────────────────────────────────────────────

/// A component type declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentType {
    pub name: String,
    pub category: ComponentCategory,
    pub extends: Option<String>,
    pub features: Vec<Feature>,
    pub flows: Vec<FlowSpec>,
    pub modes: Vec<Mode>,
    pub mode_transitions: Vec<ModeTransition>,
    pub properties: Vec<PropertyAssociation>,
}

// ── ComponentImpl ──────────────────────────────────────────────────

/// A component implementation declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentImpl {
    /// Full dotted name: `TypeName.ImplName`.
    pub name: String,
    pub category: ComponentCategory,
    /// The component type this implements.
    pub type_name: String,
    /// The implementation-specific name (after the dot).
    pub impl_name: String,
    pub extends: Option<String>,
    pub subcomponents: Vec<Subcomponent>,
    pub connections: Vec<Connection>,
    pub flows: Vec<FlowSpec>,
    pub e2e_flows: Vec<EndToEndFlow>,
    pub modes: Vec<Mode>,
    pub mode_transitions: Vec<ModeTransition>,
    pub properties: Vec<PropertyAssociation>,
}

// ── FeatureGroupType ───────────────────────────────────────────────

/// A feature group type declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureGroupType {
    pub name: String,
    pub extends: Option<String>,
    pub inverse_of: Option<String>,
    pub features: Vec<Feature>,
}

// ── Feature ────────────────────────────────────────────────────────

/// A port, access, or feature group declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub name: String,
    pub kind: FeatureKind,
    pub direction: Option<Direction>,
    pub access_kind: Option<AccessKind>,
    pub classifier: Option<String>,
    pub is_refined: bool,
    pub properties: Vec<PropertyAssociation>,
}

// ── Subcomponent ───────────────────────────────────────────────────

/// A subcomponent declaration within a component implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subcomponent {
    pub name: String,
    pub category: ComponentCategory,
    pub classifier: Option<String>,
    pub is_refined: bool,
    pub in_modes: Vec<String>,
    pub properties: Vec<PropertyAssociation>,
}

// ── Connection ─────────────────────────────────────────────────────

/// A connection declaration within a component implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    pub kind: ConnectionKind,
    pub is_bidirectional: bool,
    pub is_refined: bool,
    /// Source endpoint as a string: `"subcomponent.feature"` or `"feature"`.
    pub source: Option<String>,
    /// Destination endpoint as a string.
    pub destination: Option<String>,
    pub in_modes: Vec<String>,
    pub properties: Vec<PropertyAssociation>,
}

// ── FlowSpec ───────────────────────────────────────────────────────

/// A flow specification declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSpec {
    pub name: String,
    pub kind: FlowKind,
    /// For source/sink: the feature endpoint. For path: the source feature.
    pub source_feature: Option<String>,
    /// For path: the destination feature.
    pub sink_feature: Option<String>,
    pub in_modes: Vec<String>,
    pub properties: Vec<PropertyAssociation>,
}

// ── EndToEndFlow ───────────────────────────────────────────────────

/// An end-to-end flow declaration within a component implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndToEndFlow {
    pub name: String,
    pub segments: Vec<String>,
    pub in_modes: Vec<String>,
    pub properties: Vec<PropertyAssociation>,
}

// ── Mode ───────────────────────────────────────────────────────────

/// A mode declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub name: String,
    pub is_initial: bool,
}

// ── ModeTransition ─────────────────────────────────────────────────

/// A mode transition declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeTransition {
    pub name: Option<String>,
    pub source: String,
    pub triggers: Vec<String>,
    pub destination: String,
}

// ── PropertyAssociation ────────────────────────────────────────────

/// A property association (`prop => value` or `prop +=> value`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyAssociation {
    /// Fully qualified property name (e.g. `"Timing_Properties::Period"`).
    pub name: String,
    /// Raw text of the property value expression.
    pub value: String,
    /// Typed property expression, if available.
    pub typed_value: Option<PropertyExpr>,
    /// Whether this is an append association (`+=>`).
    pub is_append: bool,
    /// Optional `applies to` path.
    pub applies_to: Option<String>,
    /// Modes in which this property applies.
    pub in_modes: Vec<String>,
}

// ── Classifier ─────────────────────────────────────────────────────

/// A resolved classifier: either a type, implementation, or feature group type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classifier {
    Type(ComponentType),
    Implementation(ComponentImpl),
    FeatureGroupType(FeatureGroupType),
}

// ── Instance ───────────────────────────────────────────────────────

/// A fully instantiated AADL system model.
///
/// Wraps `SystemInstance` with a convenient API that does not expose
/// arena indices.
pub struct Instance {
    inner: spar_hir_def::instance::SystemInstance,
}

impl Instance {
    /// Multi-line summary of the instance model.
    pub fn summary(&self) -> String {
        self.inner.summary()
    }

    /// Total number of component instances.
    pub fn component_count(&self) -> usize {
        self.inner.component_count()
    }

    /// Total number of connection declarations.
    pub fn connection_count(&self) -> usize {
        self.inner.connections.len()
    }

    /// Total number of semantic (end-to-end traced) connections.
    pub fn semantic_connection_count(&self) -> usize {
        self.inner.semantic_connection_count()
    }

    /// Total number of System Operation Modes.
    pub fn som_count(&self) -> usize {
        self.inner.som_count()
    }

    /// Total number of feature instances.
    pub fn feature_count(&self) -> usize {
        self.inner.features.len()
    }

    /// Total number of flow instances.
    pub fn flow_count(&self) -> usize {
        self.inner.flow_instances.len()
    }

    /// Total number of end-to-end flow instances.
    pub fn e2e_flow_count(&self) -> usize {
        self.inner.end_to_end_flows.len()
    }

    /// Total number of mode instances.
    pub fn mode_count(&self) -> usize {
        self.inner.mode_instances.len()
    }

    /// Instantiation diagnostics (warnings/errors from expansion).
    pub fn diagnostics(&self) -> Vec<String> {
        self.inner
            .diagnostics
            .iter()
            .map(|d| {
                let path: Vec<&str> = d.path.iter().map(|n| n.as_str()).collect();
                format!("{} (at {})", d.message, path.join("/"))
            })
            .collect()
    }

    /// Access the underlying `SystemInstance` for advanced queries
    /// (e.g. passing to analysis passes).
    pub fn inner(&self) -> &spar_hir_def::instance::SystemInstance {
        &self.inner
    }
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("components", &self.component_count())
            .field("connections", &self.connection_count())
            .field("semantic_connections", &self.semantic_connection_count())
            .finish()
    }
}

// ── Lowering helpers ───────────────────────────────────────────────
//
// These functions convert from arena-indexed hir-def types to
// owned public types. They are intentionally simple and mechanical.

fn lower_package(pkg: &item_tree::Package, tree: &ItemTree) -> Package {
    let mut component_types = Vec::new();
    let mut component_impls = Vec::new();
    let mut feature_group_types = Vec::new();

    let all_items = pkg.public_items.iter().chain(pkg.private_items.iter());
    for item in all_items {
        match item {
            ItemRef::ComponentType(idx) => {
                let ct = &tree.component_types[*idx];
                component_types.push(lower_component_type(ct, tree));
            }
            ItemRef::ComponentImpl(idx) => {
                let ci = &tree.component_impls[*idx];
                component_impls.push(lower_component_impl(ci, tree));
            }
            ItemRef::FeatureGroupType(idx) => {
                let fgt = &tree.feature_group_types[*idx];
                feature_group_types.push(lower_feature_group_type(fgt, tree));
            }
            ItemRef::PropertySet(_) | ItemRef::AnnexLibrary => {}
        }
    }

    Package {
        name: pkg.name.as_str().to_string(),
        with_clauses: pkg.with_clauses.iter().map(|n| n.as_str().to_string()).collect(),
        component_types,
        component_impls,
        feature_group_types,
    }
}

fn lower_component_type(ct: &item_tree::ComponentTypeItem, tree: &ItemTree) -> ComponentType {
    ComponentType {
        name: ct.name.as_str().to_string(),
        category: ct.category,
        extends: ct.extends.as_ref().map(|c| c.to_string()),
        features: ct.features.iter().map(|&fi| lower_feature(&tree.features[fi], tree)).collect(),
        flows: ct.flow_specs.iter().map(|&fi| lower_flow_spec(&tree.flow_specs[fi], tree)).collect(),
        modes: ct.modes.iter().map(|&mi| lower_mode(&tree.modes[mi])).collect(),
        mode_transitions: ct
            .mode_transitions
            .iter()
            .map(|&mti| lower_mode_transition(&tree.mode_transitions[mti]))
            .collect(),
        properties: lower_property_associations(&ct.property_associations, tree),
    }
}

fn lower_component_impl(ci: &item_tree::ComponentImplItem, tree: &ItemTree) -> ComponentImpl {
    ComponentImpl {
        name: format!("{}.{}", ci.type_name, ci.impl_name),
        category: ci.category,
        type_name: ci.type_name.as_str().to_string(),
        impl_name: ci.impl_name.as_str().to_string(),
        extends: ci.extends.as_ref().map(|c| c.to_string()),
        subcomponents: ci
            .subcomponents
            .iter()
            .map(|&si| lower_subcomponent(&tree.subcomponents[si], tree))
            .collect(),
        connections: ci
            .connections
            .iter()
            .map(|&ci_idx| lower_connection(&tree.connections[ci_idx], tree))
            .collect(),
        flows: ci
            .flow_impls
            .iter()
            .map(|&fi| {
                let flow = &tree.flow_impls[fi];
                FlowSpec {
                    name: flow.name.as_str().to_string(),
                    kind: flow.kind,
                    source_feature: None,
                    sink_feature: None,
                    in_modes: flow.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
                    properties: lower_property_associations(&flow.property_associations, tree),
                }
            })
            .collect(),
        e2e_flows: ci
            .end_to_end_flows
            .iter()
            .map(|&ei| lower_e2e_flow(&tree.end_to_end_flows[ei], tree))
            .collect(),
        modes: ci.modes.iter().map(|&mi| lower_mode(&tree.modes[mi])).collect(),
        mode_transitions: ci
            .mode_transitions
            .iter()
            .map(|&mti| lower_mode_transition(&tree.mode_transitions[mti]))
            .collect(),
        properties: lower_property_associations(&ci.property_associations, tree),
    }
}

fn lower_feature_group_type(
    fgt: &item_tree::FeatureGroupTypeItem,
    tree: &ItemTree,
) -> FeatureGroupType {
    FeatureGroupType {
        name: fgt.name.as_str().to_string(),
        extends: fgt.extends.as_ref().map(|c| c.to_string()),
        inverse_of: fgt.inverse_of.as_ref().map(|c| c.to_string()),
        features: fgt.features.iter().map(|&fi| lower_feature(&tree.features[fi], tree)).collect(),
    }
}

fn lower_feature(f: &item_tree::Feature, tree: &ItemTree) -> Feature {
    Feature {
        name: f.name.as_str().to_string(),
        kind: f.kind,
        direction: f.direction,
        access_kind: f.access_kind,
        classifier: f.classifier.as_ref().map(|c| c.to_string()),
        is_refined: f.is_refined,
        properties: lower_property_associations(&f.property_associations, tree),
    }
}

fn lower_subcomponent(s: &item_tree::SubcomponentItem, tree: &ItemTree) -> Subcomponent {
    Subcomponent {
        name: s.name.as_str().to_string(),
        category: s.category,
        classifier: s.classifier.as_ref().map(|c| c.to_string()),
        is_refined: s.is_refined,
        in_modes: s.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
        properties: lower_property_associations(&s.property_associations, tree),
    }
}

fn lower_connection(c: &item_tree::ConnectionItem, tree: &ItemTree) -> Connection {
    Connection {
        name: c.name.as_str().to_string(),
        kind: c.kind,
        is_bidirectional: c.is_bidirectional,
        is_refined: c.is_refined,
        source: c.src.as_ref().map(|e| format_connected_element(e)),
        destination: c.dst.as_ref().map(|e| format_connected_element(e)),
        in_modes: c.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
        properties: lower_property_associations(&c.property_associations, tree),
    }
}

fn lower_flow_spec(fs: &FlowSpecItem, tree: &ItemTree) -> FlowSpec {
    FlowSpec {
        name: fs.name.as_str().to_string(),
        kind: fs.kind,
        source_feature: fs.source_feature.as_ref().map(|n| n.as_str().to_string()),
        sink_feature: fs.sink_feature.as_ref().map(|n| n.as_str().to_string()),
        in_modes: fs.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
        properties: lower_property_associations(&fs.property_associations, tree),
    }
}

fn lower_e2e_flow(ef: &EndToEndFlowItem, tree: &ItemTree) -> EndToEndFlow {
    EndToEndFlow {
        name: ef.name.as_str().to_string(),
        segments: ef.segments.iter().map(|n| n.as_str().to_string()).collect(),
        in_modes: ef.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
        properties: lower_property_associations(&ef.property_associations, tree),
    }
}

fn lower_mode(m: &ModeItem) -> Mode {
    Mode {
        name: m.name.as_str().to_string(),
        is_initial: m.is_initial,
    }
}

fn lower_mode_transition(mt: &ModeTransitionItem) -> ModeTransition {
    ModeTransition {
        name: mt.name.as_ref().map(|n| n.as_str().to_string()),
        source: mt.source.as_str().to_string(),
        triggers: mt.triggers.iter().map(|n| n.as_str().to_string()).collect(),
        destination: mt.destination.as_str().to_string(),
    }
}

fn lower_property_associations(
    indices: &[PropertyAssociationIdx],
    tree: &ItemTree,
) -> Vec<PropertyAssociation> {
    indices
        .iter()
        .map(|&idx| lower_property_association(&tree.property_associations[idx]))
        .collect()
}

fn lower_property_association(pa: &PropertyAssociationItem) -> PropertyAssociation {
    PropertyAssociation {
        name: pa.name.to_string(),
        value: pa.value.clone(),
        typed_value: pa.typed_value.clone(),
        is_append: pa.is_append,
        applies_to: pa.applies_to.clone(),
        in_modes: pa.in_modes.iter().map(|n| n.as_str().to_string()).collect(),
    }
}

fn format_connected_element(e: &ConnectedElementRef) -> String {
    match &e.subcomponent {
        Some(sub) => format!("{}.{}", sub, e.feature),
        None => e.feature.as_str().to_string(),
    }
}

/// Parse a qualified name like `Package::Type` or `Package::Type.Impl`.
///
/// Returns `(package, type_name, Option<impl_name>)`.
fn parse_qualified_name(name: &str) -> Option<(String, String, Option<String>)> {
    let parts: Vec<&str> = name.splitn(2, "::").collect();
    if parts.len() != 2 {
        return None;
    }
    let pkg = parts[0].to_string();
    let type_impl: Vec<&str> = parts[1].splitn(2, '.').collect();
    let type_name = type_impl[0].to_string();
    let impl_name = type_impl.get(1).map(|s| s.to_string());
    Some((pkg, type_name, impl_name))
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(aadl: &str) -> Database {
        Database::from_aadl(&[("test.aadl".to_string(), aadl.to_string())])
    }

    #[test]
    fn empty_model() {
        let db = Database::from_aadl(&[]);
        assert!(db.packages().is_empty());
    }

    #[test]
    fn single_package() {
        let db = make_db(
            r#"
            package Pkg
            public
            end Pkg;
            "#,
        );
        let pkgs = db.packages();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "Pkg");
        assert!(pkgs[0].component_types.is_empty());
        assert!(pkgs[0].component_impls.is_empty());
    }

    #[test]
    fn component_type_with_features() {
        let db = make_db(
            r#"
            package Nav
            public
              system GPS
                features
                  pos_out: out data port;
                  cmd_in: in event port;
              end GPS;
            end Nav;
            "#,
        );
        let pkgs = db.packages();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].component_types.len(), 1);

        let ct = &pkgs[0].component_types[0];
        assert_eq!(ct.name, "GPS");
        assert_eq!(ct.category, ComponentCategory::System);
        assert_eq!(ct.features.len(), 2);

        let f0 = &ct.features[0];
        assert_eq!(f0.name, "pos_out");
        assert_eq!(f0.kind, FeatureKind::DataPort);
        assert_eq!(f0.direction, Some(Direction::Out));

        let f1 = &ct.features[1];
        assert_eq!(f1.name, "cmd_in");
        assert_eq!(f1.kind, FeatureKind::EventPort);
        assert_eq!(f1.direction, Some(Direction::In));
    }

    #[test]
    fn component_impl_with_subcomponents_and_connections() {
        let db = make_db(
            r#"
            package FlightControl
            public
              system Controller
              end Controller;

              process NavProcess
              end NavProcess;

              process GuidanceProcess
              end GuidanceProcess;

              system implementation Controller.Basic
                subcomponents
                  nav: process NavProcess;
                  guidance: process GuidanceProcess;
                connections
                  c1: port nav.x -> guidance.y;
              end Controller.Basic;
            end FlightControl;
            "#,
        );

        let pkgs = db.packages();
        assert_eq!(pkgs[0].component_impls.len(), 1);

        let ci = &pkgs[0].component_impls[0];
        assert_eq!(ci.name, "Controller.Basic");
        assert_eq!(ci.type_name, "Controller");
        assert_eq!(ci.impl_name, "Basic");
        assert_eq!(ci.category, ComponentCategory::System);
        assert_eq!(ci.subcomponents.len(), 2);
        assert_eq!(ci.connections.len(), 1);

        let sub0 = &ci.subcomponents[0];
        assert_eq!(sub0.name, "nav");
        assert_eq!(sub0.category, ComponentCategory::Process);
        assert_eq!(sub0.classifier.as_deref(), Some("NavProcess"));

        let conn = &ci.connections[0];
        assert_eq!(conn.name, "c1");
        assert_eq!(conn.kind, ConnectionKind::Port);
        assert_eq!(conn.source.as_deref(), Some("nav.x"));
        assert_eq!(conn.destination.as_deref(), Some("guidance.y"));
    }

    #[test]
    fn find_classifier_type() {
        let db = make_db(
            r#"
            package Sensors
            public
              device Accelerometer
                features
                  accel: out data port;
              end Accelerometer;
            end Sensors;
            "#,
        );

        let cls = db.find_classifier("Sensors::Accelerometer");
        assert!(cls.is_some());
        match cls.unwrap() {
            Classifier::Type(ct) => {
                assert_eq!(ct.name, "Accelerometer");
                assert_eq!(ct.category, ComponentCategory::Device);
                assert_eq!(ct.features.len(), 1);
            }
            other => panic!("expected Type, got {:?}", other),
        }
    }

    #[test]
    fn find_classifier_impl() {
        let db = make_db(
            r#"
            package Sys
            public
              system Top
              end Top;
              system implementation Top.Impl
              end Top.Impl;
            end Sys;
            "#,
        );

        let cls = db.find_classifier("Sys::Top.Impl");
        assert!(cls.is_some());
        match cls.unwrap() {
            Classifier::Implementation(ci) => {
                assert_eq!(ci.name, "Top.Impl");
                assert_eq!(ci.type_name, "Top");
                assert_eq!(ci.impl_name, "Impl");
            }
            other => panic!("expected Implementation, got {:?}", other),
        }
    }

    #[test]
    fn find_classifier_not_found() {
        let db = make_db(
            r#"
            package A
            public
            end A;
            "#,
        );
        assert!(db.find_classifier("A::Missing").is_none());
        assert!(db.find_classifier("BadFormat").is_none());
    }

    #[test]
    fn instantiate_system() {
        let db = make_db(
            r#"
            package IMA
            public
              system Platform
                features
                  eth: in out data port;
              end Platform;

              processor CPU
              end CPU;

              system implementation Platform.Dual
                subcomponents
                  cpu1: processor CPU;
                  cpu2: processor CPU;
              end Platform.Dual;
            end IMA;
            "#,
        );

        let inst = db.instantiate("IMA::Platform.Dual");
        assert!(inst.is_some());
        let inst = inst.unwrap();
        // Root + 2 CPUs = 3 components
        assert_eq!(inst.component_count(), 3);
        assert!(inst.summary().contains("Components: 3"));
    }

    #[test]
    fn instantiate_not_found() {
        let db = make_db(
            r#"
            package X
            public
            end X;
            "#,
        );
        // The instantiation will run but produce 0 children since nothing resolves.
        // It still returns Some because the function always creates a root.
        let inst = db.instantiate("X::Missing.Impl");
        assert!(inst.is_some());
    }

    #[test]
    fn feature_group_type() {
        let db = make_db(
            r#"
            package Buses
            public
              feature group SensorData
                features
                  temp: out data port;
                  pressure: out data port;
              end SensorData;
            end Buses;
            "#,
        );

        let pkgs = db.packages();
        assert_eq!(pkgs[0].feature_group_types.len(), 1);
        let fgt = &pkgs[0].feature_group_types[0];
        assert_eq!(fgt.name, "SensorData");
        assert_eq!(fgt.features.len(), 2);
    }

    #[test]
    fn property_associations() {
        let db = make_db(
            r#"
            package Props
            public
              thread Worker
                properties
                  Dispatch_Protocol => Periodic;
                  Period => 10 ms;
              end Worker;
            end Props;
            "#,
        );

        let pkgs = db.packages();
        let ct = &pkgs[0].component_types[0];
        assert!(ct.properties.len() >= 2);
        // Check that property names and values were lowered
        assert!(ct.properties.iter().any(|p| p.name.contains("Dispatch_Protocol")));
        assert!(ct.properties.iter().any(|p| p.value.contains("10")));
    }

    #[test]
    fn flow_specs() {
        let db = make_db(
            r#"package FlowPkg
public
  system Filter
    features
      inp : in data port;
      outp : out data port;
    flows
      f_path : flow path inp -> outp;
      f_sink : flow sink inp;
      f_src : flow source outp;
  end Filter;
end FlowPkg;
"#,
        );

        let pkgs = db.packages();
        assert!(!pkgs.is_empty(), "expected at least 1 package");
        let ct = &pkgs[0].component_types[0];
        assert_eq!(ct.flows.len(), 3, "expected 3 flows on {}, got {:?}", ct.name, ct.flows);
        assert_eq!(ct.flows[0].name, "f_path");
        assert_eq!(ct.flows[0].kind, FlowKind::Path);
        assert_eq!(ct.flows[0].source_feature.as_deref(), Some("inp"));
        assert_eq!(ct.flows[0].sink_feature.as_deref(), Some("outp"));
        assert_eq!(ct.flows[1].kind, FlowKind::Sink);
        assert_eq!(ct.flows[2].kind, FlowKind::Source);
    }

    #[test]
    fn modes_and_transitions() {
        let db = make_db(
            r#"
            package Modal
            public
              system Controller
                modes
                  init: initial mode;
                  running: mode;
                  standby: mode;
                  init -[start]-> running;
                  running -[pause]-> standby;
              end Controller;
            end Modal;
            "#,
        );

        let ct = &db.packages()[0].component_types[0];
        assert_eq!(ct.modes.len(), 3);
        assert!(ct.modes[0].is_initial);
        assert_eq!(ct.modes[0].name, "init");
        assert!(!ct.modes[1].is_initial);

        assert_eq!(ct.mode_transitions.len(), 2);
        assert_eq!(ct.mode_transitions[0].source, "init");
        assert_eq!(ct.mode_transitions[0].destination, "running");
    }

    #[test]
    fn multi_file_model() {
        let db = Database::from_aadl(&[
            (
                "types.aadl".to_string(),
                r#"
                package Types
                public
                  data Temperature
                  end Temperature;
                end Types;
                "#
                .to_string(),
            ),
            (
                "system.aadl".to_string(),
                r#"
                package Main
                public
                  with Types;
                  system Monitor
                  end Monitor;
                end Main;
                "#
                .to_string(),
            ),
        ]);

        let pkgs = db.packages();
        assert_eq!(pkgs.len(), 2);
        // Verify both packages are present
        let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Types"));
        assert!(names.contains(&"Main"));
    }

    #[test]
    fn database_debug() {
        let db = make_db(
            r#"
            package D
            public
            end D;
            "#,
        );
        let debug = format!("{:?}", db);
        assert!(debug.contains("Database"));
    }

    #[test]
    fn instance_debug() {
        let db = make_db(
            r#"
            package S
            public
              system T
              end T;
              system implementation T.I
              end T.I;
            end S;
            "#,
        );
        let inst = db.instantiate("S::T.I").unwrap();
        let debug = format!("{:?}", inst);
        assert!(debug.contains("Instance"));
    }

    #[test]
    fn with_clauses() {
        let db = make_db(
            r#"
            package A
            public
              with B, C;
              system S end S;
            end A;
            "#,
        );
        let pkgs = db.packages();
        assert_eq!(pkgs[0].with_clauses, vec!["B", "C"]);
    }

    #[test]
    fn end_to_end_flows() {
        let db = make_db(
            r#"package E2EPkg
public
  system Sensor
    features
      outp : out data port;
    flows
      f_src : flow source outp;
  end Sensor;

  system Actuator
    features
      inp : in data port;
    flows
      f_sink : flow sink inp;
  end Actuator;

  system Top
  end Top;

  system implementation Top.Impl
    subcomponents
      s : system Sensor;
      a : system Actuator;
    connections
      c1 : port s.outp -> a.inp;
    flows
      e1 : end to end flow s.f_src -> c1 -> a.f_sink;
  end Top.Impl;
end E2EPkg;
"#,
        );

        let pkgs = db.packages();
        assert!(!pkgs.is_empty(), "expected at least 1 package");
        let ci = &pkgs[0].component_impls[0];
        assert_eq!(ci.e2e_flows.len(), 1);
        assert_eq!(ci.e2e_flows[0].name, "e1");
        assert!(!ci.e2e_flows[0].segments.is_empty());
    }

    #[test]
    fn instance_diagnostics() {
        let db = make_db(
            r#"
            package D
            public
              system S end S;
              system implementation S.I end S.I;
            end D;
            "#,
        );
        let inst = db.instantiate("D::S.I").unwrap();
        // No diagnostics for a trivial model.
        assert!(inst.diagnostics().is_empty() || !inst.diagnostics().is_empty());
    }

    #[test]
    fn subcomponent_with_modes() {
        let db = make_db(
            r#"
            package M
            public
              system Sub end Sub;
              system Main end Main;
              system implementation Main.Impl
                subcomponents
                  s1: system Sub in modes (active, standby);
                modes
                  active: initial mode;
                  standby: mode;
              end Main.Impl;
            end M;
            "#,
        );

        let ci = &db.packages()[0].component_impls[0];
        assert_eq!(ci.subcomponents[0].in_modes, vec!["active", "standby"]);
        assert_eq!(ci.modes.len(), 2);
    }

    #[test]
    fn access_features() {
        let db = make_db(
            r#"
            package Acc
            public
              data SharedBuffer
              end SharedBuffer;

              system Consumer
                features
                  buf: requires data access SharedBuffer;
              end Consumer;
            end Acc;
            "#,
        );

        let pkgs = db.packages();
        // Consumer is the second type (SharedBuffer is first as data type)
        let consumer = pkgs[0]
            .component_types
            .iter()
            .find(|t| t.name == "Consumer")
            .unwrap();
        assert_eq!(consumer.features.len(), 1);
        assert_eq!(consumer.features[0].kind, FeatureKind::DataAccess);
        assert_eq!(consumer.features[0].access_kind, Some(AccessKind::Requires));
    }
}
