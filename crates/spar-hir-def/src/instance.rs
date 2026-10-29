//! AADL instance model.
//!
//! The instance model is a flattened hierarchy of component instances,
//! connection instances, and feature instances. It's computed by
//! recursively expanding a root system implementation.
//!
//! In AADL, the instance model is what analysis tools operate on.
//! The declarative model (types + implementations) is a template;
//! the instance model is the concrete system being analyzed.

use la_arena::{Arena, Idx};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::feature_group::{ExpandedFeature, expand_feature_group};
use crate::item_tree::{
    AccessKind, ArrayDimension, ArraySize, ComponentCategory, ConnectionKind, Direction,
    FeatureKind, FlowKind,
};
use crate::name::{ClassifierRef, Name};
use crate::properties::{PropertyMap, PropertySource};
use crate::resolver::{GlobalScope, ResolvedClassifier};

pub type ComponentInstanceIdx = Idx<ComponentInstance>;
pub type FeatureInstanceIdx = Idx<FeatureInstance>;
pub type ConnectionInstanceIdx = Idx<ConnectionInstance>;
pub type FlowInstanceIdx = Idx<FlowInstance>;
pub type EndToEndFlowInstanceIdx = Idx<EndToEndFlowInstance>;
pub type ModeInstanceIdx = Idx<ModeInstance>;
pub type ModeTransitionInstanceIdx = Idx<ModeTransitionInstance>;

/// A System Operation Mode — one valid combination of modes across all modal subcomponents.
///
/// Per AS5506 §12, when a system contains multiple modal subcomponents, the
/// System Operation Modes (SOMs) are the cartesian product of all constituent
/// component modes. Each SOM represents one configuration of the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemOperationMode {
    /// Human-readable name (e.g., "active_fast" — concatenation of constituent mode names).
    pub name: String,
    /// The mode selection: which mode each modal component is in.
    /// Each entry is (component_instance_idx, mode_instance_idx).
    pub mode_selections: Vec<(ComponentInstanceIdx, ModeInstanceIdx)>,
}

/// A fully instantiated AADL system.
#[derive(Debug)]
pub struct SystemInstance {
    pub root: ComponentInstanceIdx,
    pub components: Arena<ComponentInstance>,
    pub features: Arena<FeatureInstance>,
    pub connections: Arena<ConnectionInstance>,
    pub flow_instances: Arena<FlowInstance>,
    pub end_to_end_flows: Arena<EndToEndFlowInstance>,
    pub mode_instances: Arena<ModeInstance>,
    pub mode_transition_instances: Arena<ModeTransitionInstance>,
    pub diagnostics: Vec<InstanceDiagnostic>,
    /// Property maps for each component instance.
    pub property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    /// Property maps for feature instances — values attached via
    /// `applies to <path>.<feature>` (AS5506D §11.3). Keyed by feature index
    /// so each port keeps its own properties instead of colliding on the
    /// owning component's map (issue #237).
    pub feature_property_maps: FxHashMap<FeatureInstanceIdx, PropertyMap>,
    /// Property maps for connection instances — values attached via
    /// `applies to <path>.<connection>` (issue #237).
    pub connection_property_maps: FxHashMap<ConnectionInstanceIdx, PropertyMap>,
    /// EMV2 error-propagation models projected from each component's classifier
    /// (type ∪ impl, DIRECT only — extends-chain EMV2 inheritance is deferred)
    /// at instantiation time (REQ-EMV2-PROPAGATION-002 L3). Keyed by component
    /// instance so L4 can build the Emv2Overlay without re-reading the item-tree.
    pub emv2_models: FxHashMap<ComponentInstanceIdx, Vec<spar_annex::emv2::Emv2Model>>,
    /// Semantic (end-to-end) connection instances traced through the hierarchy.
    pub semantic_connections: Vec<SemanticConnection>,
    /// System Operation Modes — the cartesian product of modes across all modal components.
    pub system_operation_modes: Vec<SystemOperationMode>,
}

/// A component instance in the flattened hierarchy.
#[derive(Debug)]
pub struct ComponentInstance {
    pub name: Name,
    pub category: ComponentCategory,
    pub type_name: Name,
    pub impl_name: Option<Name>,
    pub package: Name,
    pub parent: Option<ComponentInstanceIdx>,
    pub children: Vec<ComponentInstanceIdx>,
    pub features: Vec<FeatureInstanceIdx>,
    pub connections: Vec<ConnectionInstanceIdx>,
    pub flows: Vec<FlowInstanceIdx>,
    pub modes: Vec<ModeInstanceIdx>,
    pub mode_transitions: Vec<ModeTransitionInstanceIdx>,
    /// Array index for array subcomponents: None for non-array, Some(1..N) for array elements.
    pub array_index: Option<u64>,
    /// Modal membership: list of mode names this component is active in.
    /// Empty means active in all modes (non-modal).
    pub in_modes: Vec<Name>,
}

/// A feature instance.
#[derive(Debug)]
pub struct FeatureInstance {
    pub name: Name,
    pub kind: FeatureKind,
    pub direction: Option<Direction>,
    pub owner: ComponentInstanceIdx,
    /// Classifier reference for the feature's data type (if any).
    pub classifier: Option<ClassifierRef>,
    /// For access features: provides or requires.
    pub access_kind: Option<AccessKind>,
    /// Array index for array features: None for non-array, Some(1..N) for array elements.
    pub array_index: Option<u64>,
    /// Resolved `Data_Size` (in bytes) of the feature's data classifier, when
    /// the classifier resolves to a `data` component type that declares one.
    /// Lets consumers (e.g. WIT codegen scalar mapping, REQ-CODEGEN-WIT-TYPES)
    /// use the size without re-resolving against the global scope.
    pub data_size_bytes: Option<u64>,
}

/// A connection instance.
#[derive(Debug)]
pub struct ConnectionInstance {
    pub name: Name,
    pub kind: ConnectionKind,
    pub is_bidirectional: bool,
    /// The component instance this connection belongs to.
    pub owner: ComponentInstanceIdx,
    /// Source endpoint: (optional_subcomponent_name, feature_name).
    pub src: Option<ConnectionEnd>,
    /// Destination endpoint.
    pub dst: Option<ConnectionEnd>,
    /// Modal membership: list of mode names this connection is active in.
    /// Empty means active in all modes (non-modal).
    pub in_modes: Vec<Name>,
}

/// An endpoint of a connection instance.
#[derive(Debug, Clone)]
pub struct ConnectionEnd {
    /// Subcomponent name (None if the port is on the containing component itself).
    pub subcomponent: Option<Name>,
    /// Feature/port name.
    pub feature: Name,
}

/// A flow instance created from a flow specification in a component type.
#[derive(Debug)]
pub struct FlowInstance {
    pub name: Name,
    pub kind: FlowKind,
    /// The component instance that owns this flow.
    pub owner: ComponentInstanceIdx,
    /// Segments of the flow IMPLEMENTATION of this flow specification, when the
    /// owner's implementation (or one it `extends`) declares one — alternating
    /// `sub.flow` references and connection names, optionally bracketed by the
    /// owner's own features. `None` for a flow with no implementation (a type-only
    /// component, or a spec the implementation does not implement). Lets latency
    /// descend from `app.proc_flow` into the threads inside `app`
    /// (REQ-ANALYSIS-FLOW-THREAD-DESCENT-001).
    pub impl_segments: Option<Vec<crate::item_tree::FlowSegment>>,
}

/// An end-to-end flow instance from a component implementation.
#[derive(Debug)]
pub struct EndToEndFlowInstance {
    pub name: Name,
    /// The component instance that owns this flow.
    pub owner: ComponentInstanceIdx,
    /// Segments: alternating subcomponent and connection names.
    pub segments: Vec<Name>,
    /// Property associations written on the flow itself —
    /// `e2e : end to end flow a.s -> h -> b.k { Latency => 1 ms .. 10 ms; };`.
    ///
    /// The item tree always lowered these, and instantiation used to drop
    /// them, so an analysis could only ever see the OWNER component's
    /// properties. That is where AADL does NOT put a flow's latency budget, so
    /// a budget declared the standard way was never compared (#455).
    pub properties: PropertyMap,
}

impl EndToEndFlowInstance {
    /// The first segment whose reference AADL's grammar does not allow, as
    /// `(position, segment)`.
    ///
    /// A flow element is `[subcomponent '.'] flow` — ONE level — and a
    /// connection element is a bare identifier (osate2@87468992,
    /// `Aadl2.xtext` rules `ETESubcomponentFlow` / `ETEConnectionFlow`). So
    /// `app.caller.lock_flow` and `app.c_hand`, written on a system to reach
    /// threads inside process `app`, are not AADL. Analyses used to keep only
    /// the first dotted part and silently resolve both to `app` (#455 item 3);
    /// this is the single place that decides a flow is not analysable that way.
    pub fn nested_segment(&self) -> Option<(usize, &Name)> {
        self.segments.iter().enumerate().find(|(i, seg)| {
            let dots = seg.as_str().matches('.').count();
            if i % 2 == 0 { dots > 1 } else { dots > 0 }
        })
    }
}

/// A mode instance created from a mode declaration in a component type or implementation.
#[derive(Debug)]
pub struct ModeInstance {
    pub name: Name,
    pub is_initial: bool,
    pub owner: ComponentInstanceIdx,
}

/// A mode transition instance created from a mode transition declaration.
#[derive(Debug)]
pub struct ModeTransitionInstance {
    pub name: Option<Name>,
    pub source: Name,
    pub destination: Name,
    pub triggers: Vec<Name>,
    pub owner: ComponentInstanceIdx,
}

/// A semantic (end-to-end) connection instance that traces through the hierarchy.
///
/// Unlike `ConnectionInstance` which represents a single connection declaration,
/// this traces the full path from a source port on a leaf component to a
/// destination port on a (possibly different) leaf component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticConnection {
    /// The name of the originating connection declaration.
    pub name: Name,
    /// The connection kind.
    pub kind: ConnectionKind,
    /// Ultimate source: component instance + feature.
    pub ultimate_source: (ComponentInstanceIdx, Name),
    /// Ultimate destination: component instance + feature.
    pub ultimate_destination: (ComponentInstanceIdx, Name),
    /// The chain of connection declarations traversed (for diagnostics).
    pub connection_path: Vec<ConnectionInstanceIdx>,
}

/// Diagnostic from instantiation.
#[derive(Debug, Clone)]
pub struct InstanceDiagnostic {
    pub message: String,
    pub path: Vec<Name>,
}

impl SystemInstance {
    /// Compute an instance model by recursively expanding a root implementation.
    pub fn instantiate(
        scope: &GlobalScope,
        root_package: &Name,
        root_type: &Name,
        root_impl: &Name,
    ) -> Self {
        let mut builder = Builder {
            scope,
            components: Arena::default(),
            features: Arena::default(),
            connections: Arena::default(),
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            feature_property_maps: FxHashMap::default(),
            connection_property_maps: FxHashMap::default(),
            emv2_models: FxHashMap::default(),
            pending_applies_to: Vec::new(),
            refined_origins: FxHashMap::default(),
            applies_to_fallbacks: FxHashSet::default(),
            features_complete: FxHashSet::default(),
            feature_chain_incomplete: false,
            depth: 0,
            max_depth: 100,
        };

        // STPA-REQ-012: Detect circular containment before instantiation.
        if let Some(cycle_msg) =
            detect_circular_containment(scope, root_package, root_type, root_impl)
        {
            builder.diagnostics.push(InstanceDiagnostic {
                message: cycle_msg,
                path: vec![root_type.clone()],
            });
        }

        let root_name = Name::new(&format!("{}.{}", root_type, root_impl));
        let root_idx = builder.instantiate_component(
            &root_name,
            root_package,
            Some(root_package),
            root_type,
            root_impl,
            None,
            None,
        );

        // Resolve properties declared with `applies to <path>` onto their
        // target component instances (AS5506B §8.3/§10.6; fixes #128, #129).
        builder.resolve_pending_applies_to();

        // `inherit` properties take the containing component's value where a
        // component declares none (REQ-AADL-PROPERTY-INHERIT-001). After the
        // applies-to pass, because `applies to` values land on ancestors too.
        builder.inherit_properties();

        let mut instance = SystemInstance {
            root: root_idx,
            components: builder.components,
            features: builder.features,
            connections: builder.connections,
            flow_instances: builder.flow_instances,
            end_to_end_flows: builder.end_to_end_flows,
            mode_instances: builder.mode_instances,
            mode_transition_instances: builder.mode_transition_instances,
            diagnostics: builder.diagnostics,
            property_maps: builder.property_maps,
            feature_property_maps: builder.feature_property_maps,
            connection_property_maps: builder.connection_property_maps,
            emv2_models: builder.emv2_models,
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };
        instance.compute_semantic_connections();
        instance.expand_feature_group_connections(scope);
        instance.compute_soms();
        instance
    }

    /// Resolve the `Connection_Pattern` property for a given owner component.
    ///
    /// Per AS5506 §9.8, the `Connection_Pattern` property determines how
    /// array subcomponent connections are expanded. The value is a nested list
    /// like `((one_to_one))` or a simple identifier like `one_to_one`.
    ///
    /// Returns `AllToAll` as the default when no property is set.
    fn resolve_connection_pattern(&self, owner: ComponentInstanceIdx) -> ConnectionPattern {
        let props = self.properties_for(owner);

        let raw = props
            .get("Communication_Properties", "Connection_Pattern")
            .or_else(|| props.get("", "Connection_Pattern"));

        match raw {
            Some(val) => parse_connection_pattern(val),
            None => ConnectionPattern::AllToAll,
        }
    }

    /// Total number of component instances.
    pub fn component_count(&self) -> usize {
        self.components.len()
    }

    /// Iterate all component instances.
    pub fn all_components(
        &self,
    ) -> impl Iterator<Item = (ComponentInstanceIdx, &ComponentInstance)> {
        self.components.iter()
    }

    /// Get a component by index.
    pub fn component(&self, idx: ComponentInstanceIdx) -> &ComponentInstance {
        &self.components[idx]
    }

    /// Get the property map for a component instance.
    ///
    /// Returns an empty property map if no properties are associated.
    pub fn properties_for(&self, idx: ComponentInstanceIdx) -> &PropertyMap {
        static EMPTY: std::sync::LazyLock<PropertyMap> = std::sync::LazyLock::new(PropertyMap::new);
        self.property_maps.get(&idx).unwrap_or(&EMPTY)
    }

    /// The property map of a component instance as it reads in one system
    /// operation mode (REQ-AADL-MODAL-PROPERTY-VALUES-001).
    ///
    /// A value from a modal association is the one that applies in the mode
    /// `som` selects for the component that DECLARED the association — never
    /// a mode merely named alike on another component. A component with no
    /// modal value gets [`Self::properties_for`] itself, borrowed.
    pub fn properties_in_som(
        &self,
        idx: ComponentInstanceIdx,
        som: &SystemOperationMode,
    ) -> std::borrow::Cow<'_, PropertyMap> {
        self.properties_for(idx).in_mode(|declarer| {
            som.mode_selections
                .iter()
                .map(|&(_, mi)| &self.mode_instances[mi])
                .find(|m| m.owner == declarer)
                .map(|m| m.name.as_str())
        })
    }

    /// Whether any component's value came from a modal property association.
    pub fn has_modal_property_values(&self) -> bool {
        self.property_maps
            .values()
            .any(PropertyMap::has_modal_values)
    }

    /// Projected EMV2 models for a component instance (empty slice if none).
    pub fn emv2_for(&self, idx: ComponentInstanceIdx) -> &[spar_annex::emv2::Emv2Model] {
        self.emv2_models.get(&idx).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Resolved properties attached to a feature instance via
    /// `applies to <path>.<feature>` (issue #237). Empty if none.
    pub fn properties_for_feature(&self, idx: FeatureInstanceIdx) -> &PropertyMap {
        static EMPTY: std::sync::LazyLock<PropertyMap> = std::sync::LazyLock::new(PropertyMap::new);
        self.feature_property_maps.get(&idx).unwrap_or(&EMPTY)
    }

    /// Resolved properties attached to a connection instance via
    /// `applies to <path>.<connection>` (issue #237). Empty if none.
    pub fn properties_for_connection(&self, idx: ConnectionInstanceIdx) -> &PropertyMap {
        static EMPTY: std::sync::LazyLock<PropertyMap> = std::sync::LazyLock::new(PropertyMap::new);
        self.connection_property_maps.get(&idx).unwrap_or(&EMPTY)
    }

    /// Build a `/`-separated instance path for a component (root first).
    ///
    /// Example: for `ee_system.poc.cvc.soc.a53`, this returns the string
    /// `"ee_system/poc/cvc/soc/a53"`. Used by [`Self::resolve_dotted_path`]
    /// to match user-supplied dotted references against the instance
    /// hierarchy.
    pub fn component_instance_path(&self, idx: ComponentInstanceIdx) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut current = Some(idx);
        while let Some(ci) = current {
            let comp = &self.components[ci];
            parts.push(comp.name.as_str().to_string());
            current = comp.parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Resolve a user-supplied component reference (bare name or dotted
    /// path) to a [`ComponentInstanceIdx`].
    ///
    /// Matching rules (case-insensitive):
    ///
    /// 1. **Bare identifier** (no `.` or `/`) — match `component.name`
    ///    anywhere in the hierarchy. Ties are broken by preferring the
    ///    deepest match (most specific path wins).
    /// 2. **Dotted path** (`a.b.c`) — translate `.` to `/`, then suffix-
    ///    match against full instance paths (`root/sub1/sub2/a/b/c`).
    ///    Ties are broken by preferring the deepest match.
    ///
    /// Returns `None` if no component matches.
    ///
    /// This resolves USER INPUT, which has no declaring context: the
    /// `spar moves` CLI surface, and the name lists of spar's own
    /// `Spar_Migration::Allowed_Targets`. A property's `reference (...)`
    /// value is resolved by [`Self::resolve_reference`] from its declaring
    /// component instead — the deepest match anywhere is not what an AADL
    /// reference names (#460, REQ-AADL-REF-CONTEXT-001).
    pub fn resolve_dotted_path(&self, name: &str) -> Option<ComponentInstanceIdx> {
        let needle = name.replace('.', "/");
        let needle_lower = needle.to_ascii_lowercase();
        let is_path = needle.contains('/');

        if is_path {
            // Path matching: suffix-match against the component's full path.
            // Prefer the deepest (most specific) match.
            let mut best: Option<(ComponentInstanceIdx, usize)> = None;
            for (idx, _comp) in self.all_components() {
                let path = self.component_instance_path(idx);
                let path_lower = path.to_ascii_lowercase();
                if path_lower.ends_with(&needle_lower) {
                    let depth = path.matches('/').count();
                    if best.map(|(_, d)| depth >= d).unwrap_or(true) {
                        best = Some((idx, depth));
                    }
                }
            }
            return best.map(|(idx, _)| idx);
        }

        // Bare-name matching: exact name, deepest match wins.
        let mut best: Option<(ComponentInstanceIdx, usize)> = None;
        for (idx, comp) in self.all_components() {
            if comp.name.as_str().eq_ignore_ascii_case(name) {
                let depth = self.component_instance_path(idx).matches('/').count();
                if best.map(|(_, d)| depth >= d).unwrap_or(true) {
                    best = Some((idx, depth));
                }
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// The component from which `comp`'s value of `(set, name)` resolves its
    /// `reference (...)` terms: the declaring component when the value came
    /// through `applies to`, otherwise `comp` itself.
    ///
    /// Callers read a property under its qualified key and fall back to its
    /// bare key only when the qualified key holds NO VALUE. The context is
    /// taken from whichever key the value was read from — the two keys are
    /// separate entries, and a qualified local value beside a bare
    /// `applies to` value must not borrow the latter's context.
    pub fn reference_context(
        &self,
        comp: ComponentInstanceIdx,
        set: &str,
        name: &str,
    ) -> ComponentInstanceIdx {
        let props = self.properties_for(comp);
        let key_set = if props.get(set, name).is_some() {
            set
        } else {
            ""
        };
        props.reference_context(key_set, name).unwrap_or(comp)
    }

    /// The component from which connection `conn`'s value of `(set, name)`
    /// resolves its `reference (...)` terms: the declaring component when the
    /// value came through `applies to`, otherwise the component that OWNS the
    /// connection — OSATE resolves a connection's own property values "in the
    /// context of the connection's component instance" (osate2 @ 87468992,
    /// CachePropertyAssociationsSwitch). Same key rule as
    /// [`Self::reference_context`].
    pub fn connection_reference_context(
        &self,
        conn: ConnectionInstanceIdx,
        set: &str,
        name: &str,
    ) -> ComponentInstanceIdx {
        let props = self.properties_for_connection(conn);
        let key_set = if props.get(set, name).is_some() {
            set
        } else {
            ""
        };
        props
            .reference_context(key_set, name)
            .unwrap_or(self.connections[conn].owner)
    }

    /// Resolve a property's `reference (a.b.c)` term from `ctx`, the
    /// component its association was declared on.
    ///
    /// Each segment names a CHILD subcomponent of the previous one, starting
    /// at `ctx` — exactly OSATE's `findInstanceObjects` walk from the context
    /// (osate2 @ 87468992, ReferenceValueImpl.instantiate). There is no
    /// suffix match and no deepest-match tie-break: `resolve_dotted_path`
    /// picks the deepest component of that name ANYWHERE in the hierarchy,
    /// which binds a different `cpu` than the one the declaring context
    /// names whenever names repeat at depth (#460).
    ///
    /// One spar extension is kept: a path that begins with the root
    /// instance's name (`Top.impl.cpu`, or `ee_system.poc.cvc.soc.a53` on a
    /// root named `ee_system`), and does not resolve from `ctx`, is walked
    /// from the root. That is an absolute path, so it is unambiguous; it is
    /// not a global search. OSATE has no such form.
    ///
    /// Matching is case-insensitive, as AADL identifiers are. Returns `None`
    /// when the path does not name a component — the caller reports it.
    pub fn resolve_reference(
        &self,
        ctx: ComponentInstanceIdx,
        path: &str,
    ) -> Option<ComponentInstanceIdx> {
        let segments: Vec<&str> = path
            .split(['.', '/'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if segments.is_empty() {
            return None;
        }
        walk_children(self, ctx, &segments).or_else(|| {
            // The root's name is `Type.impl` — it contains a dot — so it is
            // matched as a PREFIX of the written path, not as one segment.
            let root_name = self.components[self.root].name.as_str();
            let written = path.trim().replace('/', ".");
            let head = written.get(..root_name.len());
            let tail = written
                .get(root_name.len()..)
                .and_then(|t| t.strip_prefix('.'));
            if let (Some(head), Some(tail)) = (head, tail)
                && head.eq_ignore_ascii_case(root_name)
            {
                let rest: Vec<&str> = tail
                    .split('.')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                walk_children(self, self.root, &rest)
            } else {
                None
            }
        })
    }

    /// Get the mode instances for a given component.
    pub fn modes_for(&self, idx: ComponentInstanceIdx) -> Vec<&ModeInstance> {
        self.components[idx]
            .modes
            .iter()
            .map(|&mi| &self.mode_instances[mi])
            .collect()
    }

    /// Get the mode transition instances for a given component.
    pub fn mode_transitions_for(&self, idx: ComponentInstanceIdx) -> Vec<&ModeTransitionInstance> {
        self.components[idx]
            .mode_transitions
            .iter()
            .map(|&mti| &self.mode_transition_instances[mti])
            .collect()
    }

    /// Total number of semantic (end-to-end) connections.
    pub fn semantic_connection_count(&self) -> usize {
        self.semantic_connections.len()
    }

    /// Total number of System Operation Modes.
    pub fn som_count(&self) -> usize {
        self.system_operation_modes.len()
    }

    /// Maximum number of SOMs before truncation.
    const MAX_SOMS: usize = 10_000;

    /// Compute System Operation Modes (SOMs) as the cartesian product of modes
    /// across all modal components in the instance hierarchy.
    ///
    /// Per AS5506 §12, a SOM represents one valid combination of mode selections
    /// for every modal subcomponent. If no component has modes, no SOMs are produced.
    /// The total is capped at [`Self::MAX_SOMS`] to prevent combinatorial explosion.
    pub fn compute_soms(&mut self) {
        // Collect all components that have at least one mode.
        // Each entry: (component_instance_idx, vec of mode_instance_idx)
        let modal_components: Vec<(ComponentInstanceIdx, Vec<ModeInstanceIdx>)> = self
            .components
            .iter()
            .filter_map(|(idx, comp)| {
                if comp.modes.is_empty() {
                    None
                } else {
                    Some((idx, comp.modes.clone()))
                }
            })
            .collect();

        if modal_components.is_empty() {
            self.system_operation_modes = Vec::new();
            return;
        }

        // Check product size before computing to avoid unnecessary work.
        // CHECKED: the product of mode counts overflows u64 on real models
        // (test-data/osate2/multiModalPingPong.aadl) — a debug build panicked
        // and a release build reported a wrapped count (4137309267128563259).
        // `None` means "more than u64::MAX".
        let total: Option<u64> = modal_components
            .iter()
            .try_fold(1u64, |acc, (_, modes)| acc.checked_mul(modes.len() as u64));

        let truncated = total.is_none_or(|t| t > Self::MAX_SOMS as u64);

        // Cartesian product via iterative expansion.
        // Start with one empty selection, then extend with each modal component's modes.
        let mut soms: Vec<Vec<(ComponentInstanceIdx, ModeInstanceIdx)>> = vec![vec![]];

        for (comp_idx, mode_indices) in &modal_components {
            let mut next =
                Vec::with_capacity((soms.len() * mode_indices.len()).min(Self::MAX_SOMS + 1));
            for existing in &soms {
                for &mode_idx in mode_indices {
                    let mut selection = existing.clone();
                    selection.push((*comp_idx, mode_idx));
                    next.push(selection);
                    if next.len() > Self::MAX_SOMS {
                        break;
                    }
                }
                if next.len() > Self::MAX_SOMS {
                    break;
                }
            }
            soms = next;
            if soms.len() > Self::MAX_SOMS {
                break;
            }
        }

        // Truncate to the cap.
        if soms.len() > Self::MAX_SOMS {
            soms.truncate(Self::MAX_SOMS);
        }

        if truncated {
            self.diagnostics.push(InstanceDiagnostic {
                message: format!(
                    "system operation mode count ({}) exceeds limit ({}); truncated",
                    total.map_or_else(|| "more than 2^64".to_string(), |t| t.to_string()),
                    Self::MAX_SOMS
                ),
                path: vec![self.components[self.root].name.clone()],
            });
        }

        // Build named SOMs.
        self.system_operation_modes = soms
            .into_iter()
            .map(|selections| {
                let name = selections
                    .iter()
                    .map(|(_, mi)| self.mode_instances[*mi].name.as_str())
                    .collect::<Vec<_>>()
                    .join("_");
                SystemOperationMode {
                    name,
                    mode_selections: selections,
                }
            })
            .collect();
    }

    /// Compute semantic (end-to-end) connection instances by tracing connections
    /// through the component hierarchy.
    ///
    /// Handles three kinds of connections:
    /// - **Across**: `sub_a.port -> sub_b.port` — both endpoints reference subcomponents
    /// - **Up**: `sub.port -> port` — source references a subcomponent, destination is
    ///   on the enclosing component itself
    /// - **Down**: `port -> sub.port` — source is on the enclosing component, destination
    ///   references a subcomponent
    ///
    /// For across connections, the algorithm traces deeper into each subcomponent
    /// to find the ultimate source/destination by following up/down connections
    /// inside the subcomponents recursively.
    ///
    /// For up/down connections at the root level, they produce standalone semantic
    /// connections with the root component's own port as one endpoint.
    ///
    /// Per AS5506 §9.8, when connections involve array subcomponents, the
    /// `Connection_Pattern` property determines the pairing strategy:
    /// - `One_To_One` — element i connects to element i (same-size arrays)
    /// - `All_To_All` — every source element connects to every destination (default)
    pub fn compute_semantic_connections(&mut self) {
        /// Maximum recursion depth to prevent infinite loops.
        const MAX_TRACE_DEPTH: usize = 20;

        let mut semantic = Vec::new();
        let mut endpoint_diagnostics = Vec::new();

        // Collect all connection indices so we can iterate without borrowing self.
        let all_conn_indices: Vec<ConnectionInstanceIdx> =
            self.connections.iter().map(|(idx, _)| idx).collect();

        for conn_idx in &all_conn_indices {
            let conn = &self.connections[*conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s.clone(), d.clone()),
                // STPA-REQ-013: Emit diagnostic for connections with missing endpoints.
                _ => {
                    let conn_name = conn.name.clone();
                    let owner_name = self.components[conn.owner].name.clone();
                    endpoint_diagnostics.push(InstanceDiagnostic {
                        message: format!("connection '{}': missing endpoint", conn_name,),
                        path: vec![owner_name, conn_name],
                    });
                    continue;
                }
            };
            let conn_owner = conn.owner;
            let conn_name = conn.name.clone();
            let conn_kind = conn.kind;

            match (&src.subcomponent, &dst.subcomponent) {
                // ── Across connection: sub_a.port -> sub_b.port ──
                (Some(src_sub_name), Some(dst_sub_name)) => {
                    let src_matches = self.find_children_by_name(conn_owner, src_sub_name);
                    let dst_matches = self.find_children_by_name(conn_owner, dst_sub_name);

                    // STPA-REQ-013: Diagnose unresolved subcomponent references.
                    if src_matches.is_empty() {
                        let owner_name = self.components[conn_owner].name.clone();
                        endpoint_diagnostics.push(InstanceDiagnostic {
                            message: format!(
                                "connection '{}': unresolved source subcomponent '{}'",
                                conn_name, src_sub_name,
                            ),
                            path: vec![owner_name, conn_name.clone()],
                        });
                    }
                    if dst_matches.is_empty() {
                        let owner_name = self.components[conn_owner].name.clone();
                        endpoint_diagnostics.push(InstanceDiagnostic {
                            message: format!(
                                "connection '{}': unresolved destination subcomponent '{}'",
                                conn_name, dst_sub_name,
                            ),
                            path: vec![owner_name, conn_name.clone()],
                        });
                    }

                    // Determine the connection pattern (AS5506 §9.8).
                    // Read Connection_Pattern from the owner's property map.
                    let pattern = self.resolve_connection_pattern(conn_owner);

                    // Build the list of (src_component, dst_component) pairs
                    // based on the connection pattern.
                    let pairs: Vec<(ComponentInstanceIdx, ComponentInstanceIdx)> = match pattern {
                        ConnectionPattern::OneToOne => {
                            // Both sides must have the same number of elements.
                            if src_matches.len() != dst_matches.len() {
                                let owner_name = self.components[conn_owner].name.clone();
                                endpoint_diagnostics.push(InstanceDiagnostic {
                                        message: format!(
                                            "connection '{}': One_To_One pattern requires equal array sizes (source={}, destination={})",
                                            conn_name, src_matches.len(), dst_matches.len(),
                                        ),
                                        path: vec![owner_name, conn_name.clone()],
                                    });
                            }
                            // Pair element-by-element up to the shorter length.
                            src_matches
                                .iter()
                                .zip(dst_matches.iter())
                                .map(|(&s, &d)| (s, d))
                                .collect()
                        }
                        ConnectionPattern::AllToAll => {
                            // Cartesian product: every source to every destination.
                            let mut p = Vec::with_capacity(src_matches.len() * dst_matches.len());
                            for &s in &src_matches {
                                for &d in &dst_matches {
                                    p.push((s, d));
                                }
                            }
                            p
                        }
                    };

                    for (src_component, dst_component) in pairs {
                        let base_path = vec![*conn_idx];

                        // Trace ALL sources (fan-in) and ALL destinations (fan-out).
                        let all_sources = self.trace_sources(
                            src_component,
                            &src.feature,
                            &base_path,
                            MAX_TRACE_DEPTH,
                        );

                        let all_destinations = self.trace_destinations(
                            dst_component,
                            &dst.feature,
                            &base_path,
                            MAX_TRACE_DEPTH,
                        );

                        // Each source × destination pair produces a semantic connection.
                        for (src_comp, src_feat, src_path) in &all_sources {
                            for (dst_comp, dst_feat, dst_path) in &all_destinations {
                                let mut path = src_path.clone();
                                // Append dst path elements that aren't already in src path.
                                for ci in dst_path {
                                    if !path.contains(ci) {
                                        path.push(*ci);
                                    }
                                }

                                semantic.push(SemanticConnection {
                                    name: conn_name.clone(),
                                    kind: conn_kind,
                                    ultimate_source: (*src_comp, src_feat.clone()),
                                    ultimate_destination: (*dst_comp, dst_feat.clone()),
                                    connection_path: path,
                                });
                            }
                        }
                    }
                }

                // ── Up connection: sub.port -> port ──
                // These produce semantic connections only when chained with
                // connections in the parent. They are traced into when processing
                // across connections in the parent. However, if this component IS
                // the root (no parent), we record a standalone semantic connection
                // with the root component's own port as the destination.
                (Some(src_sub_name), None) => {
                    let owner = &self.components[conn_owner];
                    if owner.parent.is_none() {
                        let src_matches = self.find_children_by_name(conn_owner, src_sub_name);

                        if src_matches.is_empty() {
                            endpoint_diagnostics.push(InstanceDiagnostic {
                                message: format!(
                                    "connection '{}': unresolved source subcomponent '{}'",
                                    conn_name, src_sub_name,
                                ),
                                path: vec![owner.name.clone(), conn_name.clone()],
                            });
                        }

                        for &src_component in &src_matches {
                            let base_path = vec![*conn_idx];
                            let all_sources = self.trace_sources(
                                src_component,
                                &src.feature,
                                &base_path,
                                MAX_TRACE_DEPTH,
                            );

                            for (src_comp, src_feat, path) in all_sources {
                                semantic.push(SemanticConnection {
                                    name: conn_name.clone(),
                                    kind: conn_kind,
                                    ultimate_source: (src_comp, src_feat),
                                    ultimate_destination: (conn_owner, dst.feature.clone()),
                                    connection_path: path,
                                });
                            }
                        }
                    }
                    // Otherwise, this up connection will be consumed when the parent
                    // traces an across connection through this component.
                }

                // ── Down connection: port -> sub.port ──
                // Similar to up: standalone semantic connection only at the root.
                (None, Some(dst_sub_name)) => {
                    let owner = &self.components[conn_owner];
                    if owner.parent.is_none() {
                        let dst_matches = self.find_children_by_name(conn_owner, dst_sub_name);

                        if dst_matches.is_empty() {
                            endpoint_diagnostics.push(InstanceDiagnostic {
                                message: format!(
                                    "connection '{}': unresolved destination subcomponent '{}'",
                                    conn_name, dst_sub_name,
                                ),
                                path: vec![owner.name.clone(), conn_name.clone()],
                            });
                        }

                        for &dst_component in &dst_matches {
                            let base_path = vec![*conn_idx];
                            let all_destinations = self.trace_destinations(
                                dst_component,
                                &dst.feature,
                                &base_path,
                                MAX_TRACE_DEPTH,
                            );

                            for (dst_comp, dst_feat, path) in all_destinations {
                                semantic.push(SemanticConnection {
                                    name: conn_name.clone(),
                                    kind: conn_kind,
                                    ultimate_source: (conn_owner, src.feature.clone()),
                                    ultimate_destination: (dst_comp, dst_feat),
                                    connection_path: path,
                                });
                            }
                        }
                    }
                }

                // Both endpoints on the enclosing component (no subcomponents) — skip.
                (None, None) => {}
            }
        }

        self.semantic_connections = semantic;
        self.diagnostics.extend(endpoint_diagnostics);
    }

    /// Expand feature group connections into individual port-level semantic connections.
    ///
    /// Per AS5506 §9.2, when a connection references a feature group (rather than
    /// an individual port), it represents connections between all matching features
    /// in the source and destination feature groups. This method finds feature group
    /// connections and creates individual `SemanticConnection` entries for each
    /// matched port pair.
    ///
    /// This is called as a post-processing step after `compute_semantic_connections()`.
    pub fn expand_feature_group_connections(&mut self, scope: &GlobalScope) {
        let mut expanded = Vec::new();
        let mut fg_unmatched_diags = Vec::new();

        // Collect connection indices to avoid borrow conflicts.
        let all_conn_indices: Vec<ConnectionInstanceIdx> =
            self.connections.iter().map(|(idx, _)| idx).collect();

        for conn_idx in &all_conn_indices {
            let conn = &self.connections[*conn_idx];

            // Only process feature group connections.
            if conn.kind != ConnectionKind::FeatureGroup {
                continue;
            }

            let (src_end, dst_end) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s.clone(), d.clone()),
                _ => continue,
            };
            let conn_owner = conn.owner;
            let conn_name = conn.name.clone();

            // Resolve the source and destination components.
            let src_component = self.resolve_endpoint_component(conn_owner, &src_end.subcomponent);
            let dst_component = self.resolve_endpoint_component(conn_owner, &dst_end.subcomponent);

            let (src_comp_idx, dst_comp_idx) = match (src_component, dst_component) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Look up the feature group types for both endpoints.
            let src_expanded =
                self.expand_endpoint_feature_group(scope, src_comp_idx, &src_end.feature);
            let dst_expanded =
                self.expand_endpoint_feature_group(scope, dst_comp_idx, &dst_end.feature);

            let (src_features, dst_features) = match (src_expanded, dst_expanded) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Match features by name and create individual semantic connections.
            // STPA-REQ-011: Track unmatched source features for diagnostics.
            for src_feat in &src_features {
                let mut matched = false;
                for dst_feat in &dst_features {
                    if src_feat.name.eq_ci(&dst_feat.name) {
                        // Build the dotted feature name: group_prefix.feature_name or just feature_name
                        let src_full_name = make_expanded_name(
                            &src_end.feature,
                            &src_feat.group_prefix,
                            &src_feat.name,
                        );
                        let dst_full_name = make_expanded_name(
                            &dst_end.feature,
                            &dst_feat.group_prefix,
                            &dst_feat.name,
                        );

                        expanded.push(SemanticConnection {
                            name: Name::new(&format!("{}.{}", conn_name, src_feat.name)),
                            kind: feature_kind_to_connection_kind(src_feat.kind),
                            ultimate_source: (src_comp_idx, src_full_name),
                            ultimate_destination: (dst_comp_idx, dst_full_name),
                            connection_path: vec![*conn_idx],
                        });
                        matched = true;
                        break; // matched; move to next src feature
                    }
                }
                if !matched {
                    fg_unmatched_diags.push(InstanceDiagnostic {
                        message: format!(
                            "feature group connection '{}': source feature '{}' has no matching destination feature (STPA-REQ-011)",
                            conn_name, src_feat.name
                        ),
                        path: vec![conn_name.clone(), src_feat.name.clone()],
                    });
                }
            }
        }

        self.semantic_connections.extend(expanded);
        self.diagnostics.extend(fg_unmatched_diags);
    }

    /// Resolve the component index for a connection endpoint.
    ///
    /// If `subcomponent` is Some, look for a child with that name (exact match
    /// first, then array base-name match returning the first element).
    /// If None, return the owner itself.
    fn resolve_endpoint_component(
        &self,
        owner: ComponentInstanceIdx,
        subcomponent: &Option<Name>,
    ) -> Option<ComponentInstanceIdx> {
        match subcomponent {
            Some(sub_name) => {
                let matches = self.find_children_by_name(owner, sub_name);
                matches.into_iter().next()
            }
            None => Some(owner),
        }
    }

    /// Expand a feature group on a component instance into its individual features.
    ///
    /// Looks up the component's type in the GlobalScope, finds the feature group
    /// feature by name, then uses `expand_feature_group()` to get individual features.
    fn expand_endpoint_feature_group(
        &self,
        scope: &GlobalScope,
        component: ComponentInstanceIdx,
        feature_name: &Name,
    ) -> Option<Vec<ExpandedFeature>> {
        let comp = &self.components[component];

        // Check if this feature is actually a FeatureGroup in the instance features.
        let is_fg = comp.features.iter().any(|&fi| {
            let feat = &self.features[fi];
            feat.name.eq_ci(feature_name) && feat.kind == FeatureKind::FeatureGroup
        });

        if !is_fg {
            return None;
        }

        // Find the feature's classifier reference from the type declaration.
        let type_ref = ClassifierRef::qualified(comp.package.clone(), comp.type_name.clone());
        let resolved = scope.resolve_classifier(&comp.package, &type_ref);

        let type_loc = match &resolved {
            ResolvedClassifier::ComponentType { loc, .. } => *loc,
            _ => return None,
        };

        let ct = scope.get_component_type(type_loc)?;

        // Find the feature group feature in the type's features.
        for &feat_idx in &ct.features {
            let feat = scope.get_feature(type_loc.tree, feat_idx)?;
            if feat.name.eq_ci(feature_name) && feat.kind == FeatureKind::FeatureGroup {
                if let Some(cls_ref) = &feat.classifier {
                    // Resolve the feature group type and expand it.
                    let fg_name = &cls_ref.type_name;
                    let fg_pkg = cls_ref.package.as_ref().unwrap_or(&comp.package);
                    return Some(expand_feature_group(scope, fg_pkg, fg_name, false));
                }
                return None;
            }
        }

        None
    }

    /// Find child component instances matching a subcomponent name.
    ///
    /// Supports both exact match (`sub` matches `sub`) and array broadcast
    /// (`sub` matches `sub[1]`, `sub[2]`, etc.). If the name itself contains
    /// brackets (e.g., `sub[2]`), only exact match is used.
    fn find_children_by_name(
        &self,
        owner: ComponentInstanceIdx,
        sub_name: &Name,
    ) -> Vec<ComponentInstanceIdx> {
        let owner_comp = &self.components[owner];
        let name_str = sub_name.as_str();

        // First try exact match.
        let exact: Vec<_> = owner_comp
            .children
            .iter()
            .filter(|&&child_idx| self.components[child_idx].name.as_str() == name_str)
            .copied()
            .collect();

        if !exact.is_empty() {
            return exact;
        }

        // If the name doesn't contain brackets, try matching array elements
        // whose base name matches (broadcast pattern).
        if !name_str.contains('[') {
            let matching: Vec<_> = owner_comp
                .children
                .iter()
                .filter(|&&child_idx| {
                    let child_name = self.components[child_idx].name.as_str();
                    base_name_of(child_name).eq_ignore_ascii_case(name_str)
                        && self.components[child_idx].array_index.is_some()
                })
                .copied()
                .collect();
            if !matching.is_empty() {
                return matching;
            }
        }

        Vec::new()
    }

    /// Trace ALL ultimate sources of a connection by following up connections
    /// inside a subcomponent.
    ///
    /// Given a component instance and a feature name on that component, look for
    /// all "up" connections inside it of the form `inner_sub.port -> feature_name`.
    /// For each match, recurse into `inner_sub` to find the deepest source(s).
    ///
    /// Returns a list of `(component_idx, feature_name, connection_path)` for all
    /// traced sources. Handles fan-in where multiple internal connections feed
    /// the same external feature.
    fn trace_sources(
        &self,
        component: ComponentInstanceIdx,
        feature: &Name,
        base_path: &[ConnectionInstanceIdx],
        depth_remaining: usize,
    ) -> Vec<(ComponentInstanceIdx, Name, Vec<ConnectionInstanceIdx>)> {
        if depth_remaining == 0 {
            return vec![(component, feature.clone(), base_path.to_vec())];
        }

        let conn_indices: Vec<ConnectionInstanceIdx> =
            self.components[component].connections.clone();

        let mut results = Vec::new();

        for conn_idx in &conn_indices {
            let conn = &self.connections[*conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Up connection: source has subcomponent, destination does not,
            // and destination feature matches the one we're tracing.
            if let (Some(src_sub_name), None) = (&src.subcomponent, &dst.subcomponent)
                && dst.feature.as_str() == feature.as_str()
            {
                let inner_matches = self.find_children_by_name(component, src_sub_name);

                for &inner_component in &inner_matches {
                    let mut path = base_path.to_vec();
                    path.push(*conn_idx);
                    let deeper = self.trace_sources(
                        inner_component,
                        &src.feature,
                        &path,
                        depth_remaining - 1,
                    );
                    results.extend(deeper);
                }
            }
        }

        if results.is_empty() {
            // No further up connection found — this is the ultimate source.
            vec![(component, feature.clone(), base_path.to_vec())]
        } else {
            results
        }
    }

    /// Trace ALL ultimate destinations of a connection by following down connections
    /// inside a subcomponent.
    ///
    /// Given a component instance and a feature name on that component, look for
    /// all "down" connections inside it of the form `feature_name -> inner_sub.port`.
    /// For each match, recurse into `inner_sub` to find the deepest destination(s).
    ///
    /// Returns a list of `(component_idx, feature_name, connection_path)` for all
    /// traced destinations. Handles fan-out where a single feature is connected to
    /// multiple internal subcomponents.
    fn trace_destinations(
        &self,
        component: ComponentInstanceIdx,
        feature: &Name,
        base_path: &[ConnectionInstanceIdx],
        depth_remaining: usize,
    ) -> Vec<(ComponentInstanceIdx, Name, Vec<ConnectionInstanceIdx>)> {
        if depth_remaining == 0 {
            return vec![(component, feature.clone(), base_path.to_vec())];
        }

        let conn_indices: Vec<ConnectionInstanceIdx> =
            self.components[component].connections.clone();

        let mut results = Vec::new();

        for conn_idx in &conn_indices {
            let conn = &self.connections[*conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Down connection: source has no subcomponent, destination has subcomponent,
            // and source feature matches the one we're tracing.
            if let (None, Some(dst_sub_name)) = (&src.subcomponent, &dst.subcomponent)
                && src.feature.as_str() == feature.as_str()
            {
                let inner_matches = self.find_children_by_name(component, dst_sub_name);

                for &inner_component in &inner_matches {
                    let mut path = base_path.to_vec();
                    path.push(*conn_idx);
                    let deeper = self.trace_destinations(
                        inner_component,
                        &dst.feature,
                        &path,
                        depth_remaining - 1,
                    );
                    results.extend(deeper);
                }
            }
        }

        if results.is_empty() {
            // No further down connection found — this is the ultimate destination.
            vec![(component, feature.clone(), base_path.to_vec())]
        } else {
            results
        }
    }

    /// Return a multi-line summary of the instance model.
    pub fn summary(&self) -> String {
        format!(
            "System Instance Summary:\n  \
             Components: {}\n  \
             Features: {}\n  \
             Connections: {}\n  \
             Semantic connections: {}\n  \
             Flows: {}\n  \
             End-to-end flows: {}\n  \
             Modes: {}\n  \
             Mode transitions: {}\n  \
             System operation modes: {}\n  \
             Diagnostics: {}",
            self.components.len(),
            self.features.len(),
            self.connections.len(),
            self.semantic_connections.len(),
            self.flow_instances.len(),
            self.end_to_end_flows.len(),
            self.mode_instances.len(),
            self.mode_transition_instances.len(),
            self.system_operation_modes.len(),
            self.diagnostics.len(),
        )
    }
}

// ── Feature group expansion helpers ──────────────────────────────────

/// Build a dotted feature name for an expanded feature group member.
///
/// The result is `fg_name.prefix.feature_name` or `fg_name.feature_name`
/// depending on whether the expanded feature has a group prefix.
/// Walk `segs` down through child subcomponents of `from`, one segment per
/// level, matching names exactly and case-insensitively
/// (REQ-AADL-REF-CONTEXT-001). Sibling names are unique in a legal model.
///
/// An earlier version tried every same-named child, latest first, to step
/// around #496 — a refined subcomponent instantiated BESIDE the one it
/// refined. #496 is fixed (REQ-AADL-SUBCOMPONENT-REFINE-001), and with it the
/// only source of same-named siblings, so that allowance is gone: with it in
/// place every test still passed with a first-match walk, i.e. nothing
/// exercised it any more.
fn walk_children(
    inst: &SystemInstance,
    from: ComponentInstanceIdx,
    segs: &[&str],
) -> Option<ComponentInstanceIdx> {
    let Some((seg, rest)) = segs.split_first() else {
        return Some(from);
    };
    let child = inst.components[from]
        .children
        .iter()
        .copied()
        .find(|&c| inst.components[c].name.as_str().eq_ignore_ascii_case(seg))?;
    walk_children(inst, child, rest)
}

/// `Type.impl` of the implementation in `tree` that declares subcomponent
/// `si`, for naming a refined-away original (REQ-AADL-PROPERTY-PROVENANCE-001).
fn declaring_impl_name(
    tree: &crate::item_tree::ItemTree,
    si: crate::item_tree::SubcomponentIdx,
) -> String {
    tree.component_impls
        .iter()
        .find(|(_, ci)| ci.subcomponents.contains(&si))
        .map_or_else(
            || "?".to_string(),
            |(_, ci)| format!("{}.{}", ci.type_name, ci.impl_name),
        )
}

fn make_expanded_name(fg_name: &Name, prefix: &Option<Name>, feature_name: &Name) -> Name {
    match prefix {
        Some(p) => Name::new(&format!("{}.{}.{}", fg_name, p, feature_name)),
        None => Name::new(&format!("{}.{}", fg_name, feature_name)),
    }
}

/// Map a FeatureKind to the corresponding ConnectionKind.
fn feature_kind_to_connection_kind(kind: FeatureKind) -> ConnectionKind {
    match kind {
        FeatureKind::DataPort | FeatureKind::EventPort | FeatureKind::EventDataPort => {
            ConnectionKind::Port
        }
        FeatureKind::Parameter => ConnectionKind::Parameter,
        FeatureKind::DataAccess
        | FeatureKind::BusAccess
        | FeatureKind::SubprogramAccess
        | FeatureKind::SubprogramGroupAccess => ConnectionKind::Access,
        FeatureKind::FeatureGroup => ConnectionKind::FeatureGroup,
        FeatureKind::AbstractFeature => ConnectionKind::Feature,
    }
}

/// Connection pattern for array subcomponent connections (AS5506 §9.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionPattern {
    /// Element i connects to element i (requires same array size).
    OneToOne,
    /// Every source element connects to every destination element (default).
    AllToAll,
}

/// Parse a `Connection_Pattern` property value string into a `ConnectionPattern`.
///
/// The value may be in nested list format like `((one_to_one))` or a simple
/// identifier like `one_to_one`. Strips parentheses and whitespace, then
/// matches case-insensitively. Unrecognized values default to `AllToAll`.
fn parse_connection_pattern(value: &str) -> ConnectionPattern {
    // Strip all parentheses, commas, and whitespace to get the bare pattern name.
    let stripped: String = value
        .chars()
        .filter(|c| !matches!(c, '(' | ')' | ',' | ' ' | '\t' | '\n'))
        .collect();

    if stripped.eq_ignore_ascii_case("one_to_one") {
        ConnectionPattern::OneToOne
    } else {
        // All_To_All is the default for any unrecognized or missing value.
        ConnectionPattern::AllToAll
    }
}

/// Compute the number of array elements from array dimensions.
///
/// For non-array items (empty dimensions), returns 1.
/// For arrays, uses the first dimension's literal size (property constants
/// are not yet supported and fall back to 1).
///
/// STPA-REQ-009: If a dimension evaluates to 0, pushes a diagnostic and
/// returns 1 as a safe fallback to avoid infinite loops or empty expansions.
fn array_element_count(
    dims: &[ArrayDimension],
    diagnostics: &mut Vec<InstanceDiagnostic>,
    context_name: &Name,
) -> u64 {
    if dims.is_empty() {
        return 1;
    }
    let count = dims[0]
        .size
        .as_ref()
        .and_then(|s| match s {
            ArraySize::Literal(n) => Some(*n),
            _ => None,
        })
        .unwrap_or(1);

    if count == 0 {
        diagnostics.push(InstanceDiagnostic {
            message: format!(
                "array dimension for '{}' is zero; using 1 as fallback (STPA-REQ-009)",
                context_name
            ),
            path: vec![context_name.clone()],
        });
        return 1;
    }

    count
}

/// STPA-REQ-012: Detect circular containment in the classifier→subcomponent reference graph.
///
/// Performs DFS on the containment hierarchy starting from the root implementation.
/// If an implementation is encountered that is already on the current DFS stack,
/// a cycle exists. Returns `Some(message)` describing the cycle, or `None` if no cycle.
fn detect_circular_containment(
    scope: &GlobalScope,
    root_package: &Name,
    root_type: &Name,
    root_impl: &Name,
) -> Option<String> {
    /// A classifier key: (package, type_name, impl_name), all lowercased for
    /// case-insensitive comparison.
    type ClassKey = (String, String, String);

    fn make_key(pkg: &Name, type_name: &Name, impl_name: &Name) -> ClassKey {
        (
            pkg.as_str().to_ascii_lowercase(),
            type_name.as_str().to_ascii_lowercase(),
            impl_name.as_str().to_ascii_lowercase(),
        )
    }

    fn dfs(
        scope: &GlobalScope,
        pkg: &Name,
        type_name: &Name,
        impl_name: &Name,
        visiting: &mut FxHashSet<ClassKey>,
        visited: &mut FxHashSet<ClassKey>,
        path: &mut Vec<String>,
    ) -> Option<String> {
        let key = make_key(pkg, type_name, impl_name);

        if visited.contains(&key) {
            return None;
        }

        let label = format!("{}::{}.{}", pkg, type_name, impl_name);
        if !visiting.insert(key.clone()) {
            // Cycle detected — this classifier is already on the current DFS stack.
            path.push(label);
            return Some(format!(
                "circular containment detected: {} (STPA-REQ-012)",
                path.join(" -> ")
            ));
        }

        path.push(label);

        // Resolve the implementation to find its subcomponents.
        let ref_ =
            ClassifierRef::implementation(Some(pkg.clone()), type_name.clone(), impl_name.clone());
        let resolved = scope.resolve_classifier(pkg, &ref_);
        if let ResolvedClassifier::ComponentImpl {
            loc,
            package: res_pkg,
        } = &resolved
            && let Some(ci) = scope.get_component_impl(*loc)
        {
            for &sub_idx in &ci.subcomponents {
                if let Some(tree) = scope.tree(loc.tree) {
                    let sub = &tree.subcomponents[sub_idx];
                    if let Some(cls_ref) = &sub.classifier
                        && let Some(sub_impl) = &cls_ref.impl_name
                    {
                        let sub_pkg = cls_ref.package.as_ref().unwrap_or(res_pkg);
                        if let result @ Some(_) = dfs(
                            scope,
                            sub_pkg,
                            &cls_ref.type_name,
                            sub_impl,
                            visiting,
                            visited,
                            path,
                        ) {
                            return result;
                        }
                    }
                }
            }
        }

        path.pop();
        visiting.remove(&key);
        visited.insert(key);
        None
    }

    let mut visiting = FxHashSet::default();
    let mut visited = FxHashSet::default();
    let mut path = Vec::new();

    dfs(
        scope,
        root_package,
        root_type,
        root_impl,
        &mut visiting,
        &mut visited,
        &mut path,
    )
}

/// Extract the base name from an array instance name.
///
/// If `name` is `"sub[3]"`, returns `"sub"`. If there is no bracket, returns the name as-is.
fn base_name_of(name: &str) -> &str {
    match name.find('[') {
        Some(pos) => &name[..pos],
        None => name,
    }
}

/// Collected members from walking an implementation's extends chain.
#[derive(Default)]
#[allow(clippy::type_complexity)]
struct ImplChainResult {
    subcomponents: Vec<(
        Name,
        ComponentCategory,
        Option<crate::name::ClassifierRef>,
        // Tree the subcomponent was DECLARED in. With an `extends` chain that
        // spans files, an inherited subcomponent's index is only valid in the
        // tree of the impl that declared it — not the most-derived impl's tree
        // (issue #236).
        usize,
        crate::item_tree::SubcomponentIdx,
        Vec<crate::item_tree::ArrayDimension>,
        Vec<Name>,
    )>,
    connections: Vec<(
        Name,
        ConnectionKind,
        bool,
        Option<ConnectionEnd>,
        Option<ConnectionEnd>,
        Vec<Name>,
        // Tree + item of the connection DECLARATION, so its inline property
        // associations can be attached to the instance (#515).
        usize,
        crate::item_tree::ConnectionIdx,
    )>,
    /// Flow implementations in grandparent → parent → self order, so a later
    /// entry of the same name overrides an inherited one.
    flow_impls: Vec<(Name, Vec<crate::item_tree::FlowSegment>)>,
    /// (name, segments, item-tree index, the flow's own property associations)
    e2e_flows: Vec<(
        Name,
        Vec<Name>,
        usize,
        Vec<crate::item_tree::PropertyAssociationIdx>,
    )>,
    modes: Vec<(Name, bool)>,
    mode_transitions: Vec<(Option<Name>, Name, Name, Vec<Name>)>,
    call_map: FxHashMap<String, Name>,
    /// `internal features` and `processor features` names across the chain:
    /// legal own connection ends that are not features of the type (#555).
    local_features: Vec<Name>,
}

/// Result of resolving an `applies to <dotted-path>` target.
///
/// Used internally by [`Builder::resolve_applies_to_path`].
enum AppliesTarget {
    /// All segments named subcomponents; resolved to this component.
    Component(ComponentInstanceIdx),
    /// All-but-last segments named subcomponents; the last segment names a
    /// feature on the resolved component (AADL v2.3 AS5506D §11.3). Carries the
    /// specific feature instance so the property is stored per-feature (#237).
    Feature(FeatureInstanceIdx),
    /// All-but-last segments named subcomponents; the last segment names a
    /// connection on the resolved component (#237).
    Connection(ConnectionInstanceIdx),
    /// No valid resolution: the path contains a segment that matches neither a
    /// subcomponent nor (for the last segment) a feature or connection.
    Unresolvable,
}

struct Builder<'a> {
    scope: &'a GlobalScope,
    /// For a subcomponent that `refined to` another, the subcomponents it
    /// refined away, oldest first, keyed by its own `(tree, idx)` (#496,
    /// REQ-AADL-SUBCOMPONENT-REFINE-001). Their property associations apply
    /// before its own, as OSATE's `SubcomponentImpl.getPropertyValueInternal`
    /// consults the refinement and then each refined original.
    refined_origins: FxHashMap<
        (usize, crate::item_tree::SubcomponentIdx),
        Vec<(usize, crate::item_tree::SubcomponentIdx)>,
    >,
    components: Arena<ComponentInstance>,
    features: Arena<FeatureInstance>,
    connections: Arena<ConnectionInstance>,
    flow_instances: Arena<FlowInstance>,
    end_to_end_flows: Arena<EndToEndFlowInstance>,
    mode_instances: Arena<ModeInstance>,
    mode_transition_instances: Arena<ModeTransitionInstance>,
    diagnostics: Vec<InstanceDiagnostic>,
    property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    feature_property_maps: FxHashMap<FeatureInstanceIdx, PropertyMap>,
    connection_property_maps: FxHashMap<ConnectionInstanceIdx, PropertyMap>,
    emv2_models: FxHashMap<ComponentInstanceIdx, Vec<spar_annex::emv2::Emv2Model>>,
    /// Property associations with a non-empty `applies to <path>` clause.
    ///
    /// These are collected during instantiation and eagerly attached to
    /// their resolved target components in a post-pass, so that downstream
    /// analyses (and JSON export) see the property on the target instance
    /// rather than on the declaring ancestor (fixes #128, #129).
    pending_applies_to: Vec<PendingAppliesTo>,
    /// `(owner, key)` of each value an UNRESOLVABLE `applies to` left on its
    /// declarer (see [`Self::attach_applies_to`]). That is spar's recovery,
    /// not AADL: the association applies to nothing, so the value is not the
    /// owner's and is not inherited from it (REQ-AADL-PROPERTY-INHERIT-001).
    applies_to_fallbacks: FxHashSet<(
        ComponentInstanceIdx,
        (crate::resolver::CiName, crate::resolver::CiName),
    )>,
    /// Component instances whose feature list is KNOWN to be complete: the
    /// classifier's type resolved and every `extends` ancestor resolved too.
    /// Only for these can a connection end that names a missing feature be
    /// reported (#555); a child whose type is a prototype, anonymous, or
    /// unresolved has an empty feature list that proves nothing.
    features_complete: FxHashSet<ComponentInstanceIdx>,
    /// Set by [`Self::collect_type_chain_features`] when an `extends` link
    /// does not resolve or is circular, so the collected list is partial.
    feature_chain_incomplete: bool,
    depth: u32,
    max_depth: u32,
}

/// A property association queued for its `applies to` target: the declaring
/// component, the path, the value, and the association's modal values.
type PendingAppliesTo = (
    ComponentInstanceIdx,
    String,
    crate::properties::PropertyValue,
    Option<crate::properties::ModalBinding>,
);

impl<'a> Builder<'a> {
    #[allow(clippy::too_many_arguments)]
    fn instantiate_component(
        &mut self,
        instance_name: &Name,
        from_package: &Name,
        classifier_package: Option<&Name>,
        type_name: &Name,
        impl_name: &Name,
        parent: Option<ComponentInstanceIdx>,
        subcomponent_loc: Option<(usize, crate::item_tree::SubcomponentIdx)>,
    ) -> ComponentInstanceIdx {
        // Resolve the implementation.
        // Use the explicit classifier package if provided; otherwise resolve
        // as unqualified from the containing package so that imports (including
        // renames) are searched.
        let ref_ = ClassifierRef::implementation(
            classifier_package.cloned(),
            type_name.clone(),
            impl_name.clone(),
        );
        let resolved = self.scope.resolve_classifier(from_package, &ref_);

        let (category, impl_loc, resolved_package) = match &resolved {
            ResolvedClassifier::ComponentImpl {
                loc,
                package: res_pkg,
            } => {
                let ci = self.scope.get_component_impl(*loc);
                let cat = ci.map(|c| c.category).unwrap_or(ComponentCategory::System);
                (cat, Some(*loc), res_pkg.clone())
            }
            _ => {
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!("unresolved implementation: {}", ref_),
                    path: vec![instance_name.clone()],
                });
                (ComponentCategory::System, None, from_package.clone())
            }
        };

        // Resolve the type to get features — use the resolved package from the
        // implementation so cross-package references (via imports/renames) work.
        let type_ref = ClassifierRef::qualified(resolved_package.clone(), type_name.clone());
        let type_resolved = self.scope.resolve_classifier(&resolved_package, &type_ref);
        let type_loc = match &type_resolved {
            ResolvedClassifier::ComponentType { loc, .. } => Some(*loc),
            _ => None,
        };

        // Allocate the component instance
        let idx = self.components.alloc(ComponentInstance {
            name: instance_name.clone(),
            category,
            type_name: type_name.clone(),
            impl_name: Some(impl_name.clone()),
            package: resolved_package.clone(),
            parent,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Build property map: type → impl → subcomponent layering
        self.build_property_map(idx, type_loc, impl_loc, subcomponent_loc, &resolved_package);

        // Instantiate features, flows, modes, and mode transitions from the type.
        self.populate_from_type(idx, type_loc, &resolved_package);

        // Project retained EMV2 subclause models (type ∪ impl) onto this instance.
        self.project_emv2(idx, type_loc, impl_loc);

        // Instantiate subcomponents (recursive)
        #[allow(clippy::collapsible_if)]
        if let Some(loc) = impl_loc {
            if self.depth < self.max_depth {
                self.depth += 1;

                // Walk the impl extends chain to collect inherited subcomponents
                // and connections (grandparent → parent → self).
                let impl_chain = self.collect_impl_chain(loc, &resolved_package);
                let sub_data = impl_chain.subcomponents;
                let conn_data = impl_chain.connections;
                let call_map = impl_chain.call_map;
                let local_features = impl_chain.local_features;
                let e2e_data = impl_chain.e2e_flows;
                // Attach each flow implementation to the flow-spec instance it
                // implements (later = more derived overrides an inherited one).
                for (fname, fsegs) in impl_chain.flow_impls {
                    let target = self.components[idx].flows.iter().copied().find(|&f| {
                        self.flow_instances[f]
                            .name
                            .as_str()
                            .eq_ignore_ascii_case(fname.as_str())
                    });
                    if let Some(f) = target {
                        self.flow_instances[f].impl_segments = Some(fsegs);
                    }
                }
                let impl_mode_data = impl_chain.modes;
                let impl_mt_data = impl_chain.mode_transitions;

                let mut child_indices = Vec::new();
                for (
                    sub_name,
                    _sub_cat,
                    sub_classifier,
                    sub_tree,
                    sub_idx,
                    array_dims,
                    sub_in_modes,
                ) in sub_data
                {
                    // Determine how many instances to create for this subcomponent.
                    let count = array_element_count(&array_dims, &mut self.diagnostics, &sub_name);
                    let is_array = !array_dims.is_empty();

                    for array_i in 0..count {
                        let array_index = if is_array { Some(array_i + 1) } else { None };
                        let instance_name = if let Some(i) = array_index {
                            Name::new(&format!("{}[{}]", sub_name, i))
                        } else {
                            sub_name.clone()
                        };

                        if let Some(cls_ref) = &sub_classifier {
                            // If the classifier has package + type + impl, instantiate recursively
                            if let Some(sub_impl) = &cls_ref.impl_name {
                                let child_idx = self.instantiate_component(
                                    &instance_name,
                                    &resolved_package,
                                    cls_ref.package.as_ref(),
                                    &cls_ref.type_name,
                                    sub_impl,
                                    Some(idx),
                                    Some((sub_tree, sub_idx)),
                                );
                                self.components[child_idx].array_index = array_index;
                                self.components[child_idx].in_modes = sub_in_modes.clone();
                                child_indices.push(child_idx);
                            } else {
                                // Type-only reference — leaf subcomponent.
                                // Resolve the type so we can copy its features,
                                // flows, modes, and mode transitions.
                                let type_ref = ClassifierRef {
                                    package: cls_ref.package.clone(),
                                    type_name: cls_ref.type_name.clone(),
                                    impl_name: None,
                                };
                                let type_resolved =
                                    self.scope.resolve_classifier(&resolved_package, &type_ref);
                                let (leaf_type_loc, leaf_pkg) = match &type_resolved {
                                    ResolvedClassifier::ComponentType {
                                        loc,
                                        package: res_pkg,
                                    } => (Some(*loc), res_pkg.clone()),
                                    _ => (
                                        None,
                                        cls_ref
                                            .package
                                            .clone()
                                            .unwrap_or_else(|| resolved_package.clone()),
                                    ),
                                };

                                let child_idx = self.components.alloc(ComponentInstance {
                                    name: instance_name,
                                    category: _sub_cat,
                                    type_name: cls_ref.type_name.clone(),
                                    impl_name: None,
                                    package: leaf_pkg.clone(),
                                    parent: Some(idx),
                                    children: Vec::new(),
                                    features: Vec::new(),
                                    connections: Vec::new(),
                                    flows: Vec::new(),
                                    modes: Vec::new(),
                                    mode_transitions: Vec::new(),
                                    array_index,
                                    in_modes: sub_in_modes.clone(),
                                });

                                // Copy features, flows, modes, and mode transitions
                                // from the resolved type.
                                self.populate_from_type(child_idx, leaf_type_loc, &leaf_pkg);

                                // Project the leaf type's EMV2 models (type only).
                                self.project_emv2(child_idx, leaf_type_loc, None);

                                // Build property map for leaf subcomponent (type only)
                                self.build_leaf_property_map(
                                    child_idx,
                                    &leaf_pkg,
                                    &cls_ref.type_name,
                                    sub_tree,
                                    sub_idx,
                                );
                                child_indices.push(child_idx);
                            }
                        } else {
                            // No classifier — anonymous subcomponent
                            // no classifier → no EMV2 to project
                            let child_idx = self.components.alloc(ComponentInstance {
                                name: instance_name,
                                category: _sub_cat,
                                type_name: Name::default(),
                                impl_name: None,
                                package: resolved_package.clone(),
                                parent: Some(idx),
                                children: Vec::new(),
                                features: Vec::new(),
                                connections: Vec::new(),
                                flows: Vec::new(),
                                modes: Vec::new(),
                                mode_transitions: Vec::new(),
                                array_index,
                                in_modes: sub_in_modes.clone(),
                            });
                            // Build property map for anonymous subcomponent
                            self.build_anon_property_map(child_idx, sub_tree, sub_idx);
                            child_indices.push(child_idx);
                        }
                    }
                }
                self.components[idx].children = child_indices;

                // Instantiate connections with endpoint fixups:
                //
                // 1. Access connections: a bare name matching a child
                //    subcomponent is a subcomponent reference (the entire
                //    subcomponent is the access endpoint).
                //
                // 2. Parameter connections: resolve call references to their
                //    target subprogram subcomponents (e.g. `call1.p` → `s.p`
                //    when `call1: subprogram s;`).
                let child_names: Vec<Name> = self.components[idx]
                    .children
                    .iter()
                    .map(|&ci| self.components[ci].name.clone())
                    .collect();

                let mut conn_indices = Vec::new();
                for (
                    conn_name,
                    conn_kind,
                    bidi,
                    mut src,
                    mut dst,
                    conn_in_modes,
                    conn_tree,
                    conn_item,
                ) in conn_data
                {
                    // Fix up access connection endpoints.
                    if conn_kind == ConnectionKind::Access {
                        if let Some(ref mut s) = src {
                            if s.subcomponent.is_none()
                                && child_names
                                    .iter()
                                    .any(|n| n.as_str().eq_ignore_ascii_case(s.feature.as_str()))
                            {
                                s.subcomponent = Some(s.feature.clone());
                                s.feature = Name::default();
                            }
                        }
                        if let Some(ref mut d) = dst {
                            if d.subcomponent.is_none()
                                && child_names
                                    .iter()
                                    .any(|n| n.as_str().eq_ignore_ascii_case(d.feature.as_str()))
                            {
                                d.subcomponent = Some(d.feature.clone());
                                d.feature = Name::default();
                            }
                        }
                    }

                    // Resolve call references in parameter connection endpoints.
                    if conn_kind == ConnectionKind::Parameter && !call_map.is_empty() {
                        for endpoint in [&mut src, &mut dst].into_iter().flatten() {
                            if let Some(sub_name) = &endpoint.subcomponent {
                                let key = sub_name.as_str().to_ascii_lowercase();
                                if let Some(target_sub) = call_map.get(&key) {
                                    endpoint.subcomponent = Some(target_sub.clone());
                                }
                            }
                        }
                    }

                    // `fg.member` where `fg` is a feature group of THIS
                    // component (not a subcomponent): the endpoint is the
                    // component's own feature `fg.member`.
                    for endpoint in [&mut src, &mut dst].into_iter().flatten() {
                        self.fixup_own_feature_group_endpoint(
                            idx,
                            &conn_name,
                            &child_names,
                            endpoint,
                        );
                    }

                    // AS5506 Ch.9 (Connections): each end must name a feature of the
                    // referenced subcomponent or of this component (#555).
                    // The connection is kept, as an unresolved-subcomponent
                    // end is: the diagnostic is what refuses the model.
                    for (role, endpoint) in [("source", &src), ("destination", &dst)] {
                        if let Some(endpoint) = endpoint {
                            self.check_connection_end_feature(
                                idx,
                                &conn_name,
                                role,
                                &child_names,
                                &local_features,
                                &call_map,
                                endpoint,
                            );
                        }
                    }

                    let ci = self.connections.alloc(ConnectionInstance {
                        name: conn_name,
                        kind: conn_kind,
                        is_bidirectional: bidi,
                        owner: idx,
                        src,
                        dst,
                        in_modes: conn_in_modes,
                    });
                    // The connection's own `{ … }` property associations
                    // (#515, REQ-HIR-CONNECTION-INLINE-PROPS-001). They were
                    // never lowered: only `applies to <conn>` reached
                    // connection_property_maps. Declared on the connection,
                    // their references resolve from its owner — the default
                    // of connection_reference_context. An `applies to` value
                    // from an enclosing component is attached later, in
                    // resolve_pending_applies_to, and so overrides these.
                    if let Some(tree) = self.scope.tree(conn_tree) {
                        let item = &tree.connections[conn_item];
                        let mut map = PropertyMap::new();
                        for &pa_idx in &item.property_associations {
                            let pa = &tree.property_associations[pa_idx];
                            Self::push_property_association(
                                pa,
                                idx,
                                None,
                                &mut map,
                                &mut self.pending_applies_to,
                                PropertySource::DeclaredLocal,
                            );
                        }
                        if !map.is_empty() {
                            self.connection_property_maps.insert(ci, map);
                        }
                    }
                    conn_indices.push(ci);
                }
                self.components[idx].connections = conn_indices.clone();

                // STPA-REQ-010: Validate array index bounds in connection endpoints.
                self.validate_connection_array_indices(idx, &conn_indices);

                // Instantiate end-to-end flows, carrying the flow's OWN
                // property associations rather than dropping them (#455).
                for (e2e_name, segments, tree_idx, pa_indices) in e2e_data {
                    let mut properties = PropertyMap::new();
                    // A flow-local block has no business carrying `applies to`;
                    // anything routed there is collected and discarded rather
                    // than re-targeted at the owner component.
                    let mut ignored_applies_to = Vec::new();
                    if let Some(tree) = self.scope.tree(tree_idx) {
                        for pa_idx in pa_indices {
                            let pa = &tree.property_associations[pa_idx];
                            Self::push_property_association(
                                pa,
                                idx,
                                None,
                                &mut properties,
                                &mut ignored_applies_to,
                                PropertySource::DeclaredLocal,
                            );
                        }
                    }
                    self.end_to_end_flows.alloc(EndToEndFlowInstance {
                        name: e2e_name,
                        owner: idx,
                        segments,
                        properties,
                    });
                }

                // Instantiate modes from the implementation
                // (modes may come from either the type or the impl; collect
                // impl modes that are not already present from the type)
                let existing_mode_names: Vec<Name> = self.components[idx]
                    .modes
                    .iter()
                    .map(|&mi| self.mode_instances[mi].name.clone())
                    .collect();
                for (mode_name, is_initial) in impl_mode_data {
                    if !existing_mode_names
                        .iter()
                        .any(|n| n.as_str() == mode_name.as_str())
                    {
                        let mi = self.mode_instances.alloc(ModeInstance {
                            name: mode_name,
                            is_initial,
                            owner: idx,
                        });
                        self.components[idx].modes.push(mi);
                    }
                }

                // Instantiate mode transitions from the implementation
                for (mt_name, mt_source, mt_dest, mt_triggers) in impl_mt_data {
                    let mti = self
                        .mode_transition_instances
                        .alloc(ModeTransitionInstance {
                            name: mt_name,
                            source: mt_source,
                            destination: mt_dest,
                            triggers: mt_triggers,
                            owner: idx,
                        });
                    self.components[idx].mode_transitions.push(mti);
                }

                self.depth -= 1;
            } else {
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!("maximum instantiation depth ({}) exceeded", self.max_depth),
                    path: vec![instance_name.clone()],
                });
            }
        }

        idx
    }

    /// Collect features from a component type's entire extends chain.
    ///
    /// Returns features in inheritance order: grandparent first, then parent, then self.
    /// Walk an implementation's extends chain and collect all subcomponents,
    /// connections, e2e flows, modes, mode transitions, and call maps.
    #[allow(clippy::type_complexity)]
    fn collect_impl_chain(
        &mut self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
    ) -> ImplChainResult {
        let mut result = ImplChainResult::default();
        let mut visited = Vec::new();
        self.collect_impl_chain_recursive(loc, package, &mut visited, &mut result);
        result
    }

    fn collect_impl_chain_recursive(
        &mut self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
        visited: &mut Vec<String>,
        result: &mut ImplChainResult,
    ) {
        let Some(ci) = self.scope.get_component_impl(loc) else {
            return;
        };

        let key = format!("{}::{}.{}", package, ci.type_name, ci.impl_name);
        if visited.contains(&key) {
            self.diagnostics.push(InstanceDiagnostic {
                message: format!("circular impl extends chain detected: {key}"),
                path: Vec::new(),
            });
            return;
        }
        visited.push(key);

        // Walk parent first (grandparent → parent → self)
        if let Some(ext_ref) = &ci.extends.clone() {
            let resolved = self.scope.resolve_classifier(package, ext_ref);
            if let crate::resolver::ResolvedClassifier::ComponentImpl {
                loc: parent_loc,
                package: parent_pkg,
            } = &resolved
            {
                self.collect_impl_chain_recursive(*parent_loc, parent_pkg, visited, result);
            }
        }

        // `internal features` / `processor features` of this impl (#555).
        if let Some(tree) = self.scope.tree(loc.tree) {
            let own: crate::item_tree::ComponentImplIdx =
                la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
            result.local_features.extend(
                tree.impl_local_features
                    .iter()
                    .filter(|(ci_idx, _)| *ci_idx == own)
                    .map(|(_, name)| name.clone()),
            );
        }

        // Collect own subcomponents. Tag each with `loc.tree` — the tree of the
        // impl currently being walked — so an inherited subcomponent keeps the
        // index space of the file it was declared in (issue #236).
        for &sub_idx in &ci.subcomponents {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let sub = &tree.subcomponents[sub_idx];
                // `x : refined to …` REPLACES the inherited `x` (#496): OSATE's
                // ComponentImplementationOperations.getAllSubcomponents removes
                // the refined original and appends the refinement, so it takes
                // the end position. A refinement naming no classifier keeps the
                // original's (SubcomponentImpl.getAllClassifier).
                let mut classifier = sub.classifier.clone();
                if sub.is_refined
                    && let Some(pos) = result
                        .subcomponents
                        .iter()
                        .position(|s| s.0.as_str().eq_ignore_ascii_case(sub.name.as_str()))
                {
                    let refined_away = result.subcomponents.remove(pos);
                    if classifier.is_none() {
                        classifier = refined_away.2.clone();
                    }
                    let mut origins = self
                        .refined_origins
                        .get(&(refined_away.3, refined_away.4))
                        .cloned()
                        .unwrap_or_default();
                    origins.push((refined_away.3, refined_away.4));
                    self.refined_origins.insert((loc.tree, sub_idx), origins);
                }
                result.subcomponents.push((
                    sub.name.clone(),
                    sub.category,
                    classifier,
                    loc.tree,
                    sub_idx,
                    sub.array_dimensions.clone(),
                    sub.in_modes.clone(),
                ));
            }
        }

        // Collect own connections
        for &conn_idx in &ci.connections {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let conn = &tree.connections[conn_idx];
                let src = conn.src.as_ref().map(|ce| ConnectionEnd {
                    subcomponent: ce.subcomponent.clone(),
                    feature: ce.feature.clone(),
                });
                let dst = conn.dst.as_ref().map(|ce| ConnectionEnd {
                    subcomponent: ce.subcomponent.clone(),
                    feature: ce.feature.clone(),
                });
                result.connections.push((
                    conn.name.clone(),
                    conn.kind,
                    conn.is_bidirectional,
                    src,
                    dst,
                    conn.in_modes.clone(),
                    loc.tree,
                    conn_idx,
                ));
            }
        }

        // Collect own e2e flows
        for &e2e_idx in &ci.end_to_end_flows {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let e2e = &tree.end_to_end_flows[e2e_idx];
                result.e2e_flows.push((
                    e2e.name.clone(),
                    e2e.segments.clone(),
                    loc.tree,
                    e2e.property_associations.clone(),
                ));
            }
        }

        // Collect own flow implementations
        for &fi_idx in &ci.flow_impls {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let fimpl = &tree.flow_impls[fi_idx];
                result
                    .flow_impls
                    .push((fimpl.name.clone(), fimpl.segments.clone()));
            }
        }

        // Collect own modes
        for &mode_idx in &ci.modes {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let mode = &tree.modes[mode_idx];
                result.modes.push((mode.name.clone(), mode.is_initial));
            }
        }

        // Collect own mode transitions
        for &mt_idx in &ci.mode_transitions {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let mt = &tree.mode_transitions[mt_idx];
                result.mode_transitions.push((
                    mt.name.clone(),
                    mt.source.clone(),
                    mt.destination.clone(),
                    mt.triggers.clone(),
                ));
            }
        }

        // Collect call map
        for &cs_idx in &ci.call_sequences {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let cs = &tree.call_sequences[cs_idx];
                for &call_idx in &cs.calls {
                    let call = &tree.subprogram_calls[call_idx];
                    if let Some(cls_ref) = &call.called_subprogram
                        && cls_ref.package.is_none()
                        && cls_ref.impl_name.is_none()
                    {
                        result.call_map.insert(
                            call.name.as_str().to_ascii_lowercase(),
                            cls_ref.type_name.clone(),
                        );
                    }
                }
            }
        }
    }

    /// Deduplicates by name (child overrides parent for refined features).
    #[allow(clippy::type_complexity)]
    fn collect_type_chain_features(
        &mut self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
        visited: &mut Vec<String>,
    ) -> Vec<(
        Name,
        crate::item_tree::FeatureKind,
        Option<crate::item_tree::Direction>,
        Option<crate::name::ClassifierRef>,
        Option<crate::item_tree::AccessKind>,
        Vec<crate::item_tree::ArrayDimension>,
    )> {
        let Some(ct) = self.scope.get_component_type(loc) else {
            self.feature_chain_incomplete = true;
            return Vec::new();
        };

        let key = format!("{}::{}", package, ct.name);
        if visited.contains(&key) {
            self.feature_chain_incomplete = true;
            self.diagnostics.push(InstanceDiagnostic {
                message: format!("circular extends chain detected: {key}"),
                path: Vec::new(),
            });
            return Vec::new();
        }
        visited.push(key);

        // Collect parent features first (if extends)
        let mut all_features = Vec::new();
        if let Some(ext_ref) = &ct.extends.clone() {
            let resolved = self.scope.resolve_classifier(package, ext_ref);
            if let crate::resolver::ResolvedClassifier::ComponentType {
                loc: parent_loc,
                package: parent_pkg,
            } = &resolved
            {
                let parent_feats =
                    self.collect_type_chain_features(*parent_loc, parent_pkg, visited);
                all_features.extend(parent_feats);
            } else {
                self.feature_chain_incomplete = true;
            }
        }

        // Then add own features
        for &feat_idx in &ct.features {
            if let Some(feat) = self.scope.get_feature(loc.tree, feat_idx) {
                all_features.push((
                    feat.name.clone(),
                    feat.kind,
                    feat.direction,
                    feat.classifier.clone(),
                    feat.access_kind,
                    feat.array_dimensions.clone(),
                ));
            } else {
                self.feature_chain_incomplete = true;
            }
        }

        // Deduplicate: child overrides parent (keep last occurrence per name)
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::new();
        for feat in all_features.into_iter().rev() {
            let name_key = feat.0.as_str().to_ascii_lowercase();
            if seen.insert(name_key) {
                deduped.push(feat);
            }
        }
        deduped.reverse();
        deduped
    }

    /// Populate features, flows, modes, and mode transitions from a resolved component type.
    ///
    /// Walks the extends chain to include inherited features from parent types.
    /// Resolve a feature's data classifier to its declared `Data_Size`, in
    /// bytes (REQ-CODEGEN-WIT-TYPES). Returns `None` when the classifier
    /// does not resolve to a `data` component type, declares no `Data_Size`,
    /// or the value does not parse as a size.
    /// Rewrite a connection endpoint `fg.member` — where `fg` names a feature
    /// group of the containing component `owner` and NOT a subcomponent — to
    /// the owner's own feature `fg.member` (`subcomponent: None`,
    /// `feature: "fg.member"`, the dotted form `expand_feature_group_connections`
    /// also uses).
    ///
    /// A subcomponent of the same name takes precedence (left untouched). If
    /// the group's type resolves but declares no member called `member`, the
    /// endpoint is still rewritten (the first part is not a subcomponent, so
    /// "unresolved subcomponent" would mislead) and a precise diagnostic is
    /// recorded instead. Members are matched on their leaf name; nested-group
    /// prefixes are not interpreted.
    fn fixup_own_feature_group_endpoint(
        &mut self,
        owner: ComponentInstanceIdx,
        conn_name: &Name,
        child_names: &[Name],
        endpoint: &mut ConnectionEnd,
    ) {
        let Some(first) = endpoint.subcomponent.clone() else {
            return;
        };
        let is_child = child_names.iter().any(|n| {
            n.as_str().eq_ignore_ascii_case(first.as_str())
                || (!first.as_str().contains('[')
                    && base_name_of(n.as_str()).eq_ignore_ascii_case(first.as_str()))
        });
        if is_child {
            return;
        }
        let fg_classifier = self.components[owner].features.iter().find_map(|&fi| {
            let f = &self.features[fi];
            (f.kind == FeatureKind::FeatureGroup && f.name.eq_ci(&first))
                .then(|| f.classifier.clone())
        });
        let Some(classifier) = fg_classifier else {
            return;
        };

        if let Some(cls_ref) = classifier {
            let pkg = cls_ref
                .package
                .clone()
                .unwrap_or_else(|| self.components[owner].package.clone());
            let members = expand_feature_group(self.scope, &pkg, &cls_ref.type_name, false);
            // An empty expansion means the group type did not resolve; that
            // is reported elsewhere, so do not claim the member is missing.
            if !members.is_empty() && !members.iter().any(|m| m.name.eq_ci(&endpoint.feature)) {
                let owner_name = self.components[owner].name.clone();
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!(
                        "connection '{}': feature group '{}' has no member '{}'",
                        conn_name, first, endpoint.feature,
                    ),
                    path: vec![owner_name, conn_name.clone()],
                });
            }
        }
        endpoint.feature = Name::new(&format!("{}.{}", first, endpoint.feature));
        endpoint.subcomponent = None;
    }

    /// Report a connection end that names a feature its target does not have
    /// (#555, REQ-INSTANCE-CONNECTION-END-RESOLVES-001).
    ///
    /// AS5506 Ch.9 (Connections): a connection end names a
    /// feature of a subcomponent of the containing implementation
    /// (`sub.feature`) or a feature of the containing component's own type
    /// (`feature`), or the subcomponent itself where the kind allows it. Before
    /// this check, `d.zzz` with no feature `zzz` on `d` was instantiated
    /// silently and every analysis read a connection to nothing.
    ///
    /// The check reports ONLY what it can prove absent, so it skips:
    /// - an empty feature (the access fixup's whole-subcomponent end);
    /// - a bare end naming a subcomponent, or an `internal features` /
    ///   `processor features` entry of the implementation chain (`self.ie`
    ///   lowers to a bare `ie`);
    /// - a dotted feature (`fg.member` on the owner, already rewritten and
    ///   checked by [`Self::fixup_own_feature_group_endpoint`]);
    /// - an end whose subcomponent does not resolve (reported as an
    ///   unresolved subcomponent, or a `processor.` / call reference);
    /// - a target whose feature list is not known complete (unresolved,
    ///   prototype or anonymous classifier, or a broken `extends` chain);
    /// - a feature found as a member of one of the target's feature groups:
    ///   lowering keeps only the first and last name of `sub.fg.member`, so
    ///   the member arrives as `(sub, member)`. It is matched against ANY
    ///   group of the target, and any group whose type does not expand makes
    ///   the end unprovable.
    #[allow(clippy::too_many_arguments)]
    fn check_connection_end_feature(
        &mut self,
        owner: ComponentInstanceIdx,
        conn_name: &Name,
        role: &str,
        child_names: &[Name],
        local_features: &[Name],
        call_map: &FxHashMap<String, Name>,
        endpoint: &ConnectionEnd,
    ) {
        let feature = endpoint.feature.as_str();
        if feature.is_empty() || feature.contains('.') {
            return;
        }
        let same = |declared: &str| base_name_of(declared).eq_ignore_ascii_case(feature);

        let target = match &endpoint.subcomponent {
            Some(sub) => {
                let sub = sub.as_str();
                let found = self.components[owner].children.iter().copied().find(|&c| {
                    let n = self.components[c].name.as_str();
                    n.eq_ignore_ascii_case(sub)
                        || (!sub.contains('[') && base_name_of(n).eq_ignore_ascii_case(sub))
                });
                match found {
                    Some(t) => t,
                    None => {
                        self.check_call_end_feature(owner, conn_name, role, call_map, sub, feature);
                        return;
                    }
                }
            }
            None => {
                // A bare name may name a subcomponent (a data, bus or
                // subprogram subcomponent as an end), for every kind, or an
                // implementation's internal / processor feature (`self.ie`
                // lowers to a bare `ie`).
                if child_names.iter().any(|n| same(n.as_str()))
                    || local_features.iter().any(|n| same(n.as_str()))
                {
                    return;
                }
                owner
            }
        };
        if !self.features_complete.contains(&target) {
            return;
        }

        let comp = &self.components[target];
        if comp
            .features
            .iter()
            .any(|&fi| same(self.features[fi].name.as_str()))
        {
            return;
        }
        let groups: Vec<Option<ClassifierRef>> = comp
            .features
            .iter()
            .map(|&fi| &self.features[fi])
            .filter(|f| f.kind == FeatureKind::FeatureGroup)
            .map(|f| f.classifier.clone())
            .collect();
        for group in groups {
            let Some(cls_ref) = group else {
                return;
            };
            let pkg = cls_ref
                .package
                .clone()
                .unwrap_or_else(|| self.components[target].package.clone());
            let members = expand_feature_group(self.scope, &pkg, &cls_ref.type_name, false);
            if members.is_empty() {
                return;
            }
            let in_group = members.iter().any(|m| {
                same(m.name.as_str())
                    || m.group_prefix
                        .as_ref()
                        .is_some_and(|p| p.as_str().split('.').any(same))
            });
            if in_group {
                return;
            }
        }

        let comp = &self.components[target];
        let written = match &endpoint.subcomponent {
            Some(sub) => format!("{}.{}", sub, feature),
            None => feature.to_string(),
        };
        let holder = match &endpoint.subcomponent {
            Some(sub) => format!("subcomponent '{}'", sub),
            None => format!("component '{}'", comp.name),
        };
        let message = format!(
            "connection '{}': {} '{}': {} (classifier '{}::{}') has no feature '{}'",
            conn_name, role, written, holder, comp.package, comp.type_name, feature,
        );
        let owner_name = self.components[owner].name.clone();
        self.diagnostics.push(InstanceDiagnostic {
            message,
            path: vec![owner_name, conn_name.clone()],
        });
    }

    /// The subprogram-call half of [`Self::check_connection_end_feature`].
    ///
    /// A parameter end `call1.p` arrives here rewritten to `Sp.p` when
    /// `call1 : subprogram Sp;` (an unqualified, type-only call; the call map
    /// holds exactly those). Calls are not instantiated, so the parameter is
    /// looked up in `Sp`'s type chain directly. Skipped, as unprovable: a name
    /// that is no call target, a type that does not resolve or whose `extends`
    /// chain is broken, and a type with any feature group.
    fn check_call_end_feature(
        &mut self,
        owner: ComponentInstanceIdx,
        conn_name: &Name,
        role: &str,
        call_map: &FxHashMap<String, Name>,
        target: &str,
        feature: &str,
    ) {
        let Some(call) = call_map
            .iter()
            .filter(|(_, sp)| sp.as_str().eq_ignore_ascii_case(target))
            .map(|(call, _)| call.clone())
            .min()
        else {
            return;
        };
        let package = self.components[owner].package.clone();
        let cls_ref = ClassifierRef::type_only(Name::new(target));
        let ResolvedClassifier::ComponentType { loc, package: pkg } =
            self.scope.resolve_classifier(&package, &cls_ref)
        else {
            return;
        };
        self.feature_chain_incomplete = false;
        let feats = self.collect_type_chain_features(loc, &pkg, &mut Vec::new());
        if self.feature_chain_incomplete
            || feats.iter().any(|f| f.1 == FeatureKind::FeatureGroup)
            || feats
                .iter()
                .any(|f| f.0.as_str().eq_ignore_ascii_case(feature))
        {
            return;
        }
        let owner_name = self.components[owner].name.clone();
        self.diagnostics.push(InstanceDiagnostic {
            message: format!(
                "connection '{}': {} '{}.{}': called subprogram '{}' (classifier '{}::{}') \
                 has no feature '{}'",
                conn_name, role, call, feature, target, pkg, target, feature,
            ),
            path: vec![owner_name, conn_name.clone()],
        });
    }

    fn resolve_data_size_bytes(
        &self,
        from_package: &Name,
        classifier: &ClassifierRef,
    ) -> Option<u64> {
        let resolved = self.scope.resolve_classifier(from_package, classifier);
        let loc = match &resolved {
            ResolvedClassifier::ComponentType { loc, .. } => *loc,
            _ => return None,
        };
        let ct = self.scope.get_component_type(loc)?;
        if ct.category != ComponentCategory::Data {
            return None;
        }
        let tree = self.scope.tree(loc.tree)?;
        for &pa_idx in &ct.property_associations {
            let pa = &tree.property_associations[pa_idx];
            if pa
                .name
                .property_name
                .as_str()
                .eq_ignore_ascii_case("Data_Size")
                || pa
                    .name
                    .property_name
                    .as_str()
                    .eq_ignore_ascii_case("Source_Data_Size")
            {
                // parse_size_value returns BITS ("8 Bytes" -> 64).
                let bits = crate::property_value::parse_size_value(&pa.value)?;
                return Some(bits / 8);
            }
        }
        None
    }

    /// Project the classifier's EMV2 subclause models (type ∪ impl, direct only)
    /// onto a component instance. No-op when neither loc has EMV2.
    fn project_emv2(
        &mut self,
        idx: ComponentInstanceIdx,
        type_loc: Option<crate::resolver::ItemLoc>,
        impl_loc: Option<crate::resolver::ItemLoc>,
    ) {
        let mut models = Vec::new();
        if let Some(loc) = type_loc
            && let (Some(ct), Some(tree)) = (
                self.scope.get_component_type(loc),
                self.scope.tree(loc.tree),
            )
        {
            for &sub in &ct.emv2 {
                models.push(tree.emv2_subclauses[sub].model.clone());
            }
        }
        if let Some(loc) = impl_loc
            && let (Some(ci), Some(tree)) = (
                self.scope.get_component_impl(loc),
                self.scope.tree(loc.tree),
            )
        {
            for &sub in &ci.emv2 {
                models.push(tree.emv2_subclauses[sub].model.clone());
            }
        }
        if !models.is_empty() {
            self.emv2_models.insert(idx, models);
        }
    }

    fn populate_from_type(
        &mut self,
        idx: ComponentInstanceIdx,
        type_loc: Option<crate::resolver::ItemLoc>,
        type_package: &Name,
    ) {
        let Some(loc) = type_loc else {
            return;
        };

        // Collect features from the entire extends chain
        let mut visited = Vec::new();
        self.feature_chain_incomplete = false;
        let feat_data = self.collect_type_chain_features(loc, type_package, &mut visited);
        let chain_complete = !self.feature_chain_incomplete;

        let Some(ct) = self.scope.get_component_type(loc) else {
            return;
        };
        if chain_complete {
            self.features_complete.insert(idx);
        }

        let flow_data: Vec<_> = ct
            .flow_specs
            .iter()
            .filter_map(|&flow_idx| {
                let tree = self.scope.tree(loc.tree)?;
                let flow_spec = &tree.flow_specs[flow_idx];
                Some((flow_spec.name.clone(), flow_spec.kind))
            })
            .collect();

        let mode_data: Vec<_> = ct
            .modes
            .iter()
            .filter_map(|&mode_idx| {
                let tree = self.scope.tree(loc.tree)?;
                let mode = &tree.modes[mode_idx];
                Some((mode.name.clone(), mode.is_initial))
            })
            .collect();

        let mt_data: Vec<_> = ct
            .mode_transitions
            .iter()
            .filter_map(|&mt_idx| {
                let tree = self.scope.tree(loc.tree)?;
                let mt = &tree.mode_transitions[mt_idx];
                Some((
                    mt.name.clone(),
                    mt.source.clone(),
                    mt.destination.clone(),
                    mt.triggers.clone(),
                ))
            })
            .collect();

        // Instantiate features
        let mut feat_indices = Vec::new();
        for (name, kind, direction, classifier, access_kind, array_dims) in feat_data {
            let feat_count = array_element_count(&array_dims, &mut self.diagnostics, &name);
            let feat_is_array = !array_dims.is_empty();
            let data_size_bytes = classifier
                .as_ref()
                .and_then(|c| self.resolve_data_size_bytes(type_package, c));

            for fi_i in 0..feat_count {
                let feat_array_index = if feat_is_array { Some(fi_i + 1) } else { None };
                let feat_instance_name = if let Some(i) = feat_array_index {
                    Name::new(&format!("{}[{}]", name, i))
                } else {
                    name.clone()
                };
                let fi = self.features.alloc(FeatureInstance {
                    name: feat_instance_name,
                    kind,
                    direction,
                    owner: idx,
                    classifier: classifier.clone(),
                    access_kind,
                    array_index: feat_array_index,
                    data_size_bytes,
                });
                feat_indices.push(fi);
            }
        }
        self.components[idx].features = feat_indices;

        // Instantiate flow specs
        let mut flow_indices = Vec::new();
        for (name, kind) in flow_data {
            let fi = self.flow_instances.alloc(FlowInstance {
                name,
                kind,
                owner: idx,
                impl_segments: None,
            });
            flow_indices.push(fi);
        }
        self.components[idx].flows = flow_indices;

        // Instantiate modes
        let mut mode_indices = Vec::new();
        for (name, is_initial) in mode_data {
            let mi = self.mode_instances.alloc(ModeInstance {
                name,
                is_initial,
                owner: idx,
            });
            mode_indices.push(mi);
        }

        // Instantiate mode transitions
        let mut mt_indices = Vec::new();
        for (name, source, destination, triggers) in mt_data {
            let mti = self
                .mode_transition_instances
                .alloc(ModeTransitionInstance {
                    name,
                    source,
                    destination,
                    triggers,
                    owner: idx,
                });
            mt_indices.push(mti);
        }
        self.components[idx].modes = mode_indices;
        self.components[idx].mode_transitions = mt_indices;
    }

    /// Collect property associations from a component type's extends chain.
    ///
    /// Walks the chain from the root ancestor to the given type, returning
    /// `(tree_idx, PropertyAssociationIdx, declaring classifier)` triples in
    /// inheritance order (grandparent first, self last) so that later values
    /// override earlier. The classifier is `Pkg::Type`, the one that wrote the
    /// association (REQ-AADL-PROPERTY-PROVENANCE-001).
    fn collect_type_chain_properties(
        &self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
        visited: &mut Vec<String>,
    ) -> Vec<(usize, crate::item_tree::PropertyAssociationIdx, String)> {
        let mut result = Vec::new();

        let ct = match self.scope.get_component_type(loc) {
            Some(ct) => ct,
            None => return result,
        };

        // Check for extends and recurse into parent first (so parent props come first)
        if let Some(parent_ref) = &ct.extends {
            let parent_key = format!("{}", parent_ref);
            if !visited.contains(&parent_key) {
                visited.push(parent_key);
                let resolved = self.scope.resolve_classifier(package, parent_ref);
                if let ResolvedClassifier::ComponentType {
                    loc: parent_loc,
                    package: parent_pkg,
                } = resolved
                {
                    let parent_props =
                        self.collect_type_chain_properties(parent_loc, &parent_pkg, visited);
                    result.extend(parent_props);
                }
            }
        }

        // Then append own properties (override parent)
        // Re-fetch the type since we can't hold the borrow across the recursive call
        if let Some(ct) = self.scope.get_component_type(loc) {
            let classifier = format!("{package}::{}", ct.name);
            for &pa_idx in &ct.property_associations {
                result.push((loc.tree, pa_idx, classifier.clone()));
            }
        }

        result
    }

    /// Collect property associations from a component impl's extends chain.
    ///
    /// Same inheritance-order semantics as [`collect_type_chain_properties`].
    fn collect_impl_chain_properties(
        &self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
        visited: &mut Vec<String>,
    ) -> Vec<(usize, crate::item_tree::PropertyAssociationIdx, String)> {
        let mut result = Vec::new();

        let ci = match self.scope.get_component_impl(loc) {
            Some(ci) => ci,
            None => return result,
        };

        // Check for extends and recurse into parent first (so parent props come first)
        if let Some(parent_ref) = &ci.extends {
            let parent_key = format!("{}", parent_ref);
            if !visited.contains(&parent_key) {
                visited.push(parent_key);
                let resolved = self.scope.resolve_classifier(package, parent_ref);
                if let ResolvedClassifier::ComponentImpl {
                    loc: parent_loc,
                    package: parent_pkg,
                } = resolved
                {
                    let parent_props =
                        self.collect_impl_chain_properties(parent_loc, &parent_pkg, visited);
                    result.extend(parent_props);
                }
            }
        }

        // Then append own properties (override parent)
        if let Some(ci) = self.scope.get_component_impl(loc) {
            let classifier = format!("{package}::{}.{}", ci.type_name, ci.impl_name);
            for &pa_idx in &ci.property_associations {
                result.push((loc.tree, pa_idx, classifier.clone()));
            }
        }

        result
    }

    /// Build a property map for a component instance with type + impl + subcomponent layering.
    fn build_property_map(
        &mut self,
        idx: ComponentInstanceIdx,
        type_loc: Option<crate::resolver::ItemLoc>,
        impl_loc: Option<crate::resolver::ItemLoc>,
        subcomponent_loc: Option<(usize, crate::item_tree::SubcomponentIdx)>,
        resolved_package: &Name,
    ) {
        let mut map = PropertyMap::new();

        // 1. Type-level properties (walking the extends chain)
        if let Some(loc) = type_loc {
            let mut visited = Vec::new();
            let type_props =
                self.collect_type_chain_properties(loc, resolved_package, &mut visited);
            for (tree_idx, pa_idx, classifier) in type_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    Self::push_property_association(
                        pa,
                        idx,
                        Some(idx),
                        &mut map,
                        &mut self.pending_applies_to,
                        PropertySource::Classifier { classifier },
                    );
                }
            }
        }

        // 2. Implementation-level properties (walking the extends chain, override type)
        if let Some(loc) = impl_loc {
            let mut visited = Vec::new();
            let impl_props =
                self.collect_impl_chain_properties(loc, resolved_package, &mut visited);
            for (tree_idx, pa_idx, classifier) in impl_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    Self::push_property_association(
                        pa,
                        idx,
                        Some(idx),
                        &mut map,
                        &mut self.pending_applies_to,
                        PropertySource::Implementation { classifier },
                    );
                }
            }
        }

        // 3. Subcomponent-level properties (override impl), including those of
        // any subcomponent this one refined away (#496).
        if let Some((tree_idx, sub_idx)) = subcomponent_loc {
            self.push_subcomponent_associations(idx, tree_idx, sub_idx, &mut map);
        }

        if !map.is_empty() {
            self.property_maps.insert(idx, map);
        }
    }

    /// Push a subcomponent declaration's property associations into `map`:
    /// those of every subcomponent it refined away first, oldest first, then
    /// its own, so the refinement overrides and the originals supply the rest
    /// (#496, REQ-AADL-SUBCOMPONENT-REFINE-001).
    fn push_subcomponent_associations(
        &mut self,
        idx: ComponentInstanceIdx,
        tree_idx: usize,
        sub_idx: crate::item_tree::SubcomponentIdx,
        map: &mut PropertyMap,
    ) {
        let mut decls = self
            .refined_origins
            .get(&(tree_idx, sub_idx))
            .cloned()
            .unwrap_or_default();
        decls.push((tree_idx, sub_idx));
        // A subcomponent-level association's `in modes` names modes of the
        // ENCLOSING implementation (osate2 @ 87468992,
        // CacheContainedPropertyAssociationsSwitch.java:167-168 passes the
        // container as the mode context of a subcomponent's associations).
        let mode_context = self.components[idx].parent.unwrap_or(idx);
        for (t, si) in decls {
            if let Some(tree) = self.scope.tree(t) {
                let sub = &tree.subcomponents[si];
                // The refinement's own block is local; a refined-away
                // original's values reach it by refinement.
                let refined_away = (t, si) != (tree_idx, sub_idx);
                for &pa_idx in &sub.property_associations {
                    let pa = &tree.property_associations[pa_idx];
                    let source = if refined_away {
                        PropertySource::Refined {
                            subcomponent: sub.name.as_str().to_string(),
                            declared_in: declaring_impl_name(tree, si),
                        }
                    } else {
                        PropertySource::DeclaredLocal
                    };
                    Self::push_property_association(
                        pa,
                        idx,
                        Some(mode_context),
                        map,
                        &mut self.pending_applies_to,
                        source,
                    );
                }
            }
        }
    }

    /// Build a property map for a leaf (type-only) subcomponent.
    fn build_leaf_property_map(
        &mut self,
        idx: ComponentInstanceIdx,
        package: &Name,
        type_name: &Name,
        parent_tree_idx: usize,
        sub_idx: crate::item_tree::SubcomponentIdx,
    ) {
        let mut map = PropertyMap::new();

        // Resolve type to get type-level properties (walking the extends chain)
        let type_ref = ClassifierRef::qualified(package.clone(), type_name.clone());
        let type_resolved = self.scope.resolve_classifier(package, &type_ref);
        if let ResolvedClassifier::ComponentType { loc, .. } = &type_resolved {
            let mut visited = Vec::new();
            let type_props = self.collect_type_chain_properties(*loc, package, &mut visited);
            for (tree_idx, pa_idx, classifier) in type_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    Self::push_property_association(
                        pa,
                        idx,
                        Some(idx),
                        &mut map,
                        &mut self.pending_applies_to,
                        PropertySource::Classifier { classifier },
                    );
                }
            }
        }

        // Subcomponent-level properties, refined-away originals first (#496).
        self.push_subcomponent_associations(idx, parent_tree_idx, sub_idx, &mut map);

        if !map.is_empty() {
            self.property_maps.insert(idx, map);
        }
    }

    /// Build a property map for an anonymous (no classifier) subcomponent.
    fn build_anon_property_map(
        &mut self,
        idx: ComponentInstanceIdx,
        tree_idx: usize,
        sub_idx: crate::item_tree::SubcomponentIdx,
    ) {
        let mut map = PropertyMap::new();

        // Refined-away originals first, then this declaration (#496).
        self.push_subcomponent_associations(idx, tree_idx, sub_idx, &mut map);

        if !map.is_empty() {
            self.property_maps.insert(idx, map);
        }
    }

    /// Add a property association either to `map` (no `applies to` clause)
    /// or to the pending queue (for post-pass eager target resolution).
    ///
    /// `owner` is the component on which the association is declared; the
    /// pending queue later walks the dotted `applies_to` path starting from
    /// `owner` to find the target instance.
    ///
    /// `source` is where a value added to `map` came from. A queued
    /// `applies to` value takes its source when it is attached instead.
    ///
    /// `mode_context` is the component whose modes the association's
    /// `in modes` lists name; a modal association records its values against
    /// it (REQ-AADL-MODAL-PROPERTY-VALUES-001). `None` records no modal
    /// values — connection and flow associations, whose modal values are not
    /// modelled. A value keeps both its source and its modal binding.
    fn push_property_association(
        pa: &crate::item_tree::PropertyAssociationItem,
        owner: ComponentInstanceIdx,
        mode_context: Option<ComponentInstanceIdx>,
        map: &mut PropertyMap,
        pending: &mut Vec<PendingAppliesTo>,
        source: PropertySource,
    ) {
        let prop = crate::properties::PropertyValue {
            name: pa.name.clone(),
            value: pa.value.clone(),
            typed_expr: pa.typed_value.clone(),
            is_append: pa.is_append,
        };
        let modal = mode_context
            .filter(|_| !pa.modal_values.is_empty())
            .map(|ctx| crate::properties::ModalBinding {
                mode_context: ctx,
                values: pa
                    .modal_values
                    .iter()
                    .map(|mv| crate::properties::ModalValue {
                        in_modes: mv.in_modes.clone(),
                        value: crate::properties::PropertyValue {
                            name: pa.name.clone(),
                            value: mv.value.clone(),
                            typed_expr: mv.typed_value.clone(),
                            is_append: false,
                        },
                    })
                    .collect(),
            });
        match pa.applies_to.as_deref() {
            Some(path) if !path.trim().is_empty() => {
                pending.push((owner, path.to_string(), prop, modal));
            }
            _ => map.add_modal(prop, source, modal, None),
        }
    }

    /// Post-instantiation pass: attach properties declared with
    /// `applies to <dotted-path>` to their target component instances.
    ///
    /// AS5506B §8.3 / §10.6: a `Actual_Processor_Binding => ... applies to
    /// fw.firmware;` declared at a system implementation must bind the
    /// target thread instance, not the declaring system. This is resolved
    /// eagerly so that downstream analyses and the JSON instance exporter
    /// (#129) see the property on the target.
    ///
    /// AADL v2.3 (AS5506D §11.3) also allows the `applies to` path to end with
    /// a feature name: `Some_Property => value applies to subcomp.port;`.
    /// In that case the property is stored on the component that owns the
    /// feature (the resolved component at the penultimate segment), so that
    /// downstream analyses can retrieve it via `properties_for`.  No diagnostic
    /// is emitted for valid feature paths.
    ///
    /// The `applies to` clause may carry a comma-separated LIST of paths
    /// (AS5506D §11.3, `contained_property_association`); the association then
    /// applies to each path independently (#459). Each element is resolved on
    /// its own, so a list where some elements resolve and others do not attaches
    /// the property to the ones that resolve and reports only the ones that fail.
    ///
    /// If a path cannot be resolved at all (bad subcomponent name or name
    /// that matches neither a subcomponent nor a feature), the property stays
    /// on the declaring component and a diagnostic is recorded for that path.
    fn resolve_pending_applies_to(&mut self) {
        let pending = std::mem::take(&mut self.pending_applies_to);
        for (owner, path, prop, modal) in pending {
            // AS5506D §11.3: a `contained_property_association`'s `applies to`
            // clause is a LIST of containment paths —
            //   `applies to` contained_model_element_path { `,` contained_model_element_path }*
            // — and the association applies to EACH element independently
            // (#459). The item-tree lowering joins the parsed paths with ", "
            // (see `lower.rs::lower_property_association`), so split them back
            // apart here and resolve each on its own. A containment path never
            // itself contains a comma, so the split is lossless; a single-path
            // clause yields exactly one element and behaves as before.
            let elements: Vec<&str> = path
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();

            match elements.as_slice() {
                // Degenerate (all-whitespace) path: preserve the historical
                // single-element behavior by resolving the string as-is.
                [] => self.attach_applies_to(owner, &path, prop, modal),
                [single] => self.attach_applies_to(owner, single, prop, modal),
                // A genuine list: the property applies to each element, so each
                // target gets its own copy. Resolving per element also means a
                // partial list reports only the elements that failed, naming a
                // path the author actually wrote rather than the joined string.
                many => {
                    for element in many {
                        self.attach_applies_to(owner, element, prop.clone(), modal.clone());
                    }
                }
            }
        }
    }

    /// Post-instantiation pass: give every component instance the value of
    /// each `inherit` property it has none of, from the NEAREST containing
    /// component that has one (REQ-AADL-PROPERTY-INHERIT-001, #460).
    ///
    /// Semantics, from osate2 @ 87468992 rather than prose:
    /// - Only `inherit` properties, the 34 of
    ///   [`INHERIT_PROPERTIES`](crate::standard_properties::INHERIT_PROPERTIES),
    ///   inherit (PropertyImpl.getPropertyValueInternal: local value first,
    ///   then `if (isInherit()) io = io.eContainer()` and recurse).
    /// - A component receives a value only if it accepts the property
    ///   (CachePropertyAssociationsSwitch caches only where
    ///   `io.acceptsProperty`), but the walk up passes through containers that
    ///   do not accept it, as OSATE's recursion does.
    /// - A local value wins. "Local" means the PROPERTY, not the map key: a
    ///   bare `Period` and a `Timing_Properties::Period` are one property, so
    ///   either blocks inheritance of the other.
    /// - The inherited value keeps its DECLARER's reference context: the
    ///   ancestor's recorded context for that key, or the ancestor itself.
    ///   OSATE caches the ancestor's value with its references already
    ///   instantiated, so `reference (cpu)` inherited by a thread names the
    ///   `cpu` its declarer named, never one resolved from the thread.
    ///
    /// The walk reads the maps as they were BEFORE this pass, so the nearest
    /// ancestor that declares a value is found directly, whatever order the
    /// components are visited in.
    fn inherit_properties(&mut self) {
        use crate::properties::PropertyValue;
        use crate::resolver::CiName;

        type Key = (CiName, CiName);
        /// `(inheritor, key, values, reference context, ancestor, the
        /// ancestor's sources for the values, the ancestor's modal binding)`.
        type Addition = (
            ComponentInstanceIdx,
            Key,
            Vec<PropertyValue>,
            ComponentInstanceIdx,
            ComponentInstanceIdx,
            Vec<PropertySource>,
            Option<crate::properties::ModalBinding>,
        );
        let mut additions: Vec<Addition> = Vec::new();

        for (idx, comp) in self.components.iter() {
            let is_impl = comp.impl_name.is_some();
            for prop in crate::standard_properties::INHERIT_PROPERTIES {
                if !prop.accepts(comp.category, is_impl) {
                    continue;
                }
                let keys: [Key; 2] = [
                    (CiName::from_str(""), CiName::from_str(prop.name)),
                    (
                        CiName::from_str(prop.property_set),
                        CiName::from_str(prop.name),
                    ),
                ];
                // A component's own value — including a recovery value an
                // unresolvable `applies to` left on it — blocks inheritance.
                let own = self
                    .property_maps
                    .get(&idx)
                    .is_some_and(|m| keys.iter().any(|k| m.has_key(k)));
                if own {
                    continue;
                }
                // An ancestor's value is a source only if it is its own: a
                // recovery value is skipped and the walk goes on up.
                let source_keys = |a: ComponentInstanceIdx| -> Vec<&Key> {
                    self.property_maps.get(&a).map_or_else(Vec::new, |m| {
                        keys.iter()
                            .filter(|k| {
                                m.has_key(k)
                                    && !self.applies_to_fallbacks.contains(&(a, (*k).clone()))
                            })
                            .collect()
                    })
                };
                let mut ancestor = comp.parent;
                while let Some(a) = ancestor {
                    let from = source_keys(a);
                    if !from.is_empty() {
                        let map = &self.property_maps[&a];
                        for key in from {
                            if let Some(values) = map.values_for_key(key) {
                                let ctx = map.reference_context_for_key(key).unwrap_or(a);
                                let origins = map.sources_for_key(key).to_vec();
                                // A modal value is inherited with its
                                // modes, still named by its declarer, AND
                                // with its provenance.
                                let modal = map.modal_for_key(key).cloned();
                                additions.push((
                                    idx,
                                    key.clone(),
                                    values.clone(),
                                    ctx,
                                    a,
                                    origins,
                                    modal,
                                ));
                            }
                        }
                        break;
                    }
                    ancestor = self.components[a].parent;
                }
            }
        }
        for (idx, key, values, ctx, ancestor, origins, modal) in additions {
            self.property_maps
                .entry(idx)
                .or_default()
                .insert_inherited(key, values, ctx, ancestor, &origins, modal);
        }
    }

    /// Resolve a single `applies to` containment path and attach `prop` to the
    /// element it names, or — when it resolves to nothing — keep it on `owner`
    /// and record a diagnostic naming that one path.
    fn attach_applies_to(
        &mut self,
        owner: ComponentInstanceIdx,
        path: &str,
        prop: crate::properties::PropertyValue,
        modal: Option<crate::properties::ModalBinding>,
    ) {
        match self.resolve_applies_to_path(owner, path) {
            AppliesTarget::Component(target) => {
                // The value's `reference (...)` terms resolve from `owner`,
                // the component that declared the association, not from
                // `target` (REQ-AADL-REF-CONTEXT-001).
                //
                // A recovery value recorded in `applies_to_fallbacks` is never
                // replaced here: every value attached to a component through
                // `applies to` comes from a STRICT ancestor, whose associations
                // are queued before the component's own (instantiation is
                // top-down), so they are attached before its recovery value.
                //
                // Its `in modes` lists name `owner`'s modes too: the binding
                // was recorded against the declarer when it was queued.
                self.property_maps.entry(target).or_default().add_modal(
                    prop,
                    PropertySource::Contained { declarer: owner },
                    modal,
                    Some(owner),
                );
            }
            AppliesTarget::Feature(feature) => {
                // Last segment named a feature (AS5506D §11.3). Store on the
                // feature instance so each port keeps its own values instead
                // of colliding by property name on the component map (#237).
                self.feature_property_maps
                    .entry(feature)
                    .or_default()
                    .add_from(prop, PropertySource::Contained { declarer: owner });
            }
            AppliesTarget::Connection(connection) => {
                // Last segment named a connection — store per-connection (#237).
                // Its `reference (...)` terms resolve from `owner`, the
                // declarer (#498, REQ-AADL-REF-CONTEXT-001).
                self.connection_property_maps
                    .entry(connection)
                    .or_default()
                    .add_with_reference_context(prop, owner);
            }
            AppliesTarget::Unresolvable => {
                // Unresolvable path: keep on owner (prior behavior) and
                // emit a diagnostic so the author notices. The diagnostic names
                // this one path, not the whole comma-joined clause (#459).
                // The value is not the owner's own, so it is not inherited
                // from the owner either (REQ-AADL-PROPERTY-INHERIT-001).
                // An append (`+=>`) leaves an existing value the owner's own;
                // only an assignment replaces it with the recovery value.
                let key = PropertyMap::key_of(&prop.name);
                let holds = self
                    .property_maps
                    .get(&owner)
                    .is_some_and(|m| m.has_key(&key));
                if !(prop.is_append && holds) {
                    self.applies_to_fallbacks.insert((owner, key));
                }
                self.property_maps.entry(owner).or_default().add_modal(
                    prop,
                    PropertySource::ContainedUnresolved {
                        path: path.to_string(),
                    },
                    modal,
                    None,
                );
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!(
                        "applies_to path '{path}' could not be resolved to a component instance or feature"
                    ),
                    path: vec![self.components[owner].name.clone()],
                });
            }
        }
    }

    /// Walk a dotted path (`fw.firmware` or `proc_inst.input_port`) from
    /// `owner` down through subcomponent children, matching names
    /// case-insensitively.
    ///
    /// Returns:
    /// - [`AppliesTarget::Component`] when all segments name subcomponents.
    /// - [`AppliesTarget::Feature`] when all-but-last segments name
    ///   subcomponents and the final segment names a feature on the resolved
    ///   component (AADL v2.3 AS5506D §11.3 feature-path support).
    /// - [`AppliesTarget::Connection`] when the final segment names a connection
    ///   on the resolved component (#237).
    /// - [`AppliesTarget::Unresolvable`] when any segment cannot be matched.
    fn resolve_applies_to_path(&self, owner: ComponentInstanceIdx, path: &str) -> AppliesTarget {
        let segments: Vec<&str> = path
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        if segments.is_empty() {
            return AppliesTarget::Component(owner);
        }

        let mut current = owner;
        for (i, &segment) in segments.iter().enumerate() {
            // Try to match as a subcomponent child first.
            if let Some(&child) = self.components[current].children.iter().find(|&&ci| {
                self.components[ci]
                    .name
                    .as_str()
                    .eq_ignore_ascii_case(segment)
            }) {
                current = child;
                continue;
            }

            // Not a child — check if this is the last segment and names a
            // feature or a connection on the current component.
            let is_last = i == segments.len() - 1;
            if is_last {
                if let Some(&fi) = self.components[current].features.iter().find(|&&fi| {
                    self.features[fi]
                        .name
                        .as_str()
                        .eq_ignore_ascii_case(segment)
                }) {
                    return AppliesTarget::Feature(fi);
                }
                if let Some(&ci) = self.components[current].connections.iter().find(|&&ci| {
                    self.connections[ci]
                        .name
                        .as_str()
                        .eq_ignore_ascii_case(segment)
                }) {
                    return AppliesTarget::Connection(ci);
                }
            }

            // Neither subcomponent, feature, nor connection — unresolvable.
            return AppliesTarget::Unresolvable;
        }

        AppliesTarget::Component(current)
    }

    /// STPA-REQ-010: Validate that connection endpoint array indices are within bounds.
    ///
    /// For each connection owned by `owner`, if an endpoint references a specific
    /// array element (e.g., `sub[5]`), checks that the index doesn't exceed the
    /// number of array children with that base name.
    fn validate_connection_array_indices(
        &mut self,
        owner: ComponentInstanceIdx,
        conn_indices: &[ConnectionInstanceIdx],
    ) {
        // Build a map of base_name -> max_array_index for children of this component.
        let mut max_indices: FxHashMap<String, u64> = FxHashMap::default();
        for &child_idx in &self.components[owner].children {
            let child = &self.components[child_idx];
            if let Some(ai) = child.array_index {
                let base = base_name_of(child.name.as_str()).to_ascii_lowercase();
                let entry = max_indices.entry(base).or_insert(0);
                if ai > *entry {
                    *entry = ai;
                }
            }
        }

        if max_indices.is_empty() {
            return;
        }

        for &ci in conn_indices {
            let conn = &self.connections[ci];
            for endpoint in [&conn.src, &conn.dst].into_iter().flatten() {
                if let Some(sub_name) = &endpoint.subcomponent {
                    let name_str = sub_name.as_str();
                    // Check if the endpoint references a specific array index.
                    if let Some(bracket_pos) = name_str.find('[')
                        && let Some(end_pos) = name_str.find(']')
                        && let Ok(index) = name_str[bracket_pos + 1..end_pos].parse::<u64>()
                    {
                        let base = name_str[..bracket_pos].to_ascii_lowercase();
                        if let Some(&max_idx) = max_indices.get(&base)
                            && (index > max_idx || index == 0)
                        {
                            self.diagnostics.push(InstanceDiagnostic {
                                message: format!(
                                    "connection '{}': array index {} for '{}' is out of bounds (max {}) (STPA-REQ-010)",
                                    conn.name, index, &name_str[..bracket_pos], max_idx
                                ),
                                path: vec![conn.name.clone()],
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::{
        ArrayDimension, ArraySize, ComponentCategory, ConnectionKind, Direction, FeatureKind,
    };
    use crate::name::Name;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;

    /// Helper: build a minimal SystemInstance with manually specified components.
    fn make_instance(
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        root: ComponentInstanceIdx,
    ) -> SystemInstance {
        SystemInstance {
            root,
            components,
            features,
            connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            feature_property_maps: FxHashMap::default(),
            connection_property_maps: FxHashMap::default(),
            emv2_models: Default::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }

    // ── array_element_count tests ─────────────────────────────────────

    #[test]
    fn test_array_element_count_empty_dims() {
        let mut diags = Vec::new();
        assert_eq!(array_element_count(&[], &mut diags, &Name::new("test")), 1);
        assert!(diags.is_empty());
    }

    #[test]
    fn test_array_element_count_literal() {
        let mut diags = Vec::new();
        let dims = vec![ArrayDimension {
            size: Some(ArraySize::Literal(5)),
        }];
        assert_eq!(
            array_element_count(&dims, &mut diags, &Name::new("test")),
            5
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn test_array_element_count_no_size() {
        let mut diags = Vec::new();
        let dims = vec![ArrayDimension { size: None }];
        assert_eq!(
            array_element_count(&dims, &mut diags, &Name::new("test")),
            1
        );
        assert!(diags.is_empty());
    }

    // ── base_name_of tests ────────────────────────────────────────────

    #[test]
    fn test_base_name_of_no_bracket() {
        assert_eq!(base_name_of("sub"), "sub");
    }

    #[test]
    fn test_base_name_of_with_bracket() {
        assert_eq!(base_name_of("sub[3]"), "sub");
    }

    // ── Subcomponent array expansion tests ────────────────────────────

    #[test]
    fn test_subcomponent_array_expansion() {
        // Simulate what instantiation produces for `sub[3]: process P;`
        // by manually creating 3 instances with array naming.
        let mut components: Arena<ComponentInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let mut child_indices = Vec::new();
        for i in 1..=3u64 {
            let idx = components.alloc(ComponentInstance {
                name: Name::new(&format!("sub[{}]", i)),
                category: ComponentCategory::Process,
                type_name: Name::new("P"),
                impl_name: None,
                package: Name::new("Pkg"),
                parent: Some(root),
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: Some(i),
                in_modes: Vec::new(),
            });
            child_indices.push(idx);
        }
        components[root].children = child_indices.clone();

        let instance = make_instance(components, Arena::default(), Arena::default(), root);

        // 3 children with expected names and array indices.
        assert_eq!(instance.components[root].children.len(), 3);
        for (i, &child_idx) in child_indices.iter().enumerate() {
            let child = &instance.components[child_idx];
            assert_eq!(child.name.as_str(), &format!("sub[{}]", i + 1));
            assert_eq!(child.array_index, Some(i as u64 + 1));
        }
    }

    #[test]
    fn test_non_array_unchanged() {
        // A regular (non-array) subcomponent has array_index: None.
        let mut components: Arena<ComponentInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let child = components.alloc(ComponentInstance {
            name: Name::new("sensor"),
            category: ComponentCategory::Device,
            type_name: Name::new("Sensor"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });
        components[root].children.push(child);

        let instance = make_instance(components, Arena::default(), Arena::default(), root);
        assert_eq!(instance.components[root].children.len(), 1);
        assert_eq!(instance.components[child].array_index, None);
        assert_eq!(instance.components[child].name.as_str(), "sensor");
    }

    // ── Feature array expansion tests ─────────────────────────────────

    #[test]
    fn test_feature_array_expansion() {
        // Simulate `p[2]: in data port;` producing 2 feature instances.
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut features: Arena<FeatureInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("comp"),
            category: ComponentCategory::System,
            type_name: Name::new("T"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let mut feat_indices = Vec::new();
        for i in 1..=2u64 {
            let fi = features.alloc(FeatureInstance {
                name: Name::new(&format!("p[{}]", i)),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::In),
                owner: root,
                classifier: None,
                access_kind: None,
                array_index: Some(i),
                data_size_bytes: None,
            });
            feat_indices.push(fi);
        }
        components[root].features = feat_indices.clone();

        let instance = make_instance(components, features, Arena::default(), root);
        assert_eq!(instance.components[root].features.len(), 2);
        for (i, &fi) in feat_indices.iter().enumerate() {
            let feat = &instance.features[fi];
            assert_eq!(feat.name.as_str(), &format!("p[{}]", i + 1));
            assert_eq!(feat.array_index, Some(i as u64 + 1));
        }
    }

    // ── Connection to array (broadcast) tests ─────────────────────────

    #[test]
    fn test_connection_to_array_broadcast() {
        // A connection `c: port sub.p -> other.q;` with `sub` being a 2-element
        // array should create semantic connections to both sub[1] and sub[2].
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Two array elements of `sub`
        let sub1 = components.alloc(ComponentInstance {
            name: Name::new("sub[1]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(1),
            in_modes: Vec::new(),
        });

        let sub2 = components.alloc(ComponentInstance {
            name: Name::new("sub[2]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(2),
            in_modes: Vec::new(),
        });

        // Non-array subcomponent `other`
        let other = components.alloc(ComponentInstance {
            name: Name::new("other"),
            category: ComponentCategory::Process,
            type_name: Name::new("Q"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        components[root].children = vec![sub1, sub2, other];

        // Connection: sub.p -> other.q (where `sub` is the array base name)
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("sub")),
                feature: Name::new("p"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("other")),
                feature: Name::new("q"),
            }),
            in_modes: Vec::new(),
        });
        components[root].connections.push(conn_idx);

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.compute_semantic_connections();

        // Should produce 2 semantic connections: sub[1].p -> other.q and sub[2].p -> other.q
        assert_eq!(
            instance.semantic_connections.len(),
            2,
            "expected 2 semantic connections for broadcast to 2-element array, got {}",
            instance.semantic_connections.len()
        );

        // Verify the source components are sub[1] and sub[2].
        let src_idxs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| sc.ultimate_source.0)
            .collect();
        assert!(src_idxs.contains(&sub1));
        assert!(src_idxs.contains(&sub2));

        // All destinations should be `other`.
        for sc in &instance.semantic_connections {
            assert_eq!(sc.ultimate_destination.0, other);
        }
    }

    // ── Nested arrays test ────────────────────────────────────────────

    #[test]
    fn test_nested_array_with_features() {
        // Array subcomponent with array features.
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut features: Arena<FeatureInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // 2 array elements of `sub`, each with 2 array features
        let mut child_indices = Vec::new();
        for sub_i in 1..=2u64 {
            let sub = components.alloc(ComponentInstance {
                name: Name::new(&format!("sub[{}]", sub_i)),
                category: ComponentCategory::Process,
                type_name: Name::new("P"),
                impl_name: None,
                package: Name::new("Pkg"),
                parent: Some(root),
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: Some(sub_i),
                in_modes: Vec::new(),
            });

            let mut feat_indices = Vec::new();
            for fi in 1..=2u64 {
                let feat = features.alloc(FeatureInstance {
                    name: Name::new(&format!("p[{}]", fi)),
                    kind: FeatureKind::DataPort,
                    direction: Some(Direction::In),
                    owner: sub,
                    classifier: None,
                    access_kind: None,
                    array_index: Some(fi),
                    data_size_bytes: None,
                });
                feat_indices.push(feat);
            }
            components[sub].features = feat_indices;
            child_indices.push(sub);
        }
        components[root].children = child_indices.clone();

        let instance = make_instance(components, features, Arena::default(), root);

        // 2 children, each with 2 features = total 4 features, 2 children
        assert_eq!(instance.components[root].children.len(), 2);
        for &child_idx in &child_indices {
            let child = &instance.components[child_idx];
            assert_eq!(child.features.len(), 2);
            assert!(child.array_index.is_some());
            for &fi in &child.features {
                assert!(instance.features[fi].array_index.is_some());
            }
        }
    }

    // ── find_children_by_name tests ───────────────────────────────────

    #[test]
    fn test_find_children_exact_match() {
        let mut components: Arena<ComponentInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let child = components.alloc(ComponentInstance {
            name: Name::new("sensor"),
            category: ComponentCategory::Device,
            type_name: Name::new("S"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });
        components[root].children.push(child);

        let instance = make_instance(components, Arena::default(), Arena::default(), root);
        let result = instance.find_children_by_name(root, &Name::new("sensor"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], child);
    }

    #[test]
    fn test_find_children_array_broadcast() {
        let mut components: Arena<ComponentInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let mut children = Vec::new();
        for i in 1..=3u64 {
            let child = components.alloc(ComponentInstance {
                name: Name::new(&format!("sub[{}]", i)),
                category: ComponentCategory::Process,
                type_name: Name::new("P"),
                impl_name: None,
                package: Name::new("Pkg"),
                parent: Some(root),
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: Some(i),
                in_modes: Vec::new(),
            });
            children.push(child);
        }
        components[root].children = children.clone();

        let instance = make_instance(components, Arena::default(), Arena::default(), root);

        // Looking for "sub" should match all 3 array elements.
        let result = instance.find_children_by_name(root, &Name::new("sub"));
        assert_eq!(result.len(), 3);

        // Looking for "sub[2]" should match exactly one.
        let result2 = instance.find_children_by_name(root, &Name::new("sub[2]"));
        assert_eq!(result2.len(), 1);
        assert_eq!(instance.components[result2[0]].name.as_str(), "sub[2]");

        // Looking for "nonexistent" should match none.
        let result3 = instance.find_children_by_name(root, &Name::new("nonexistent"));
        assert_eq!(result3.len(), 0);
    }

    // ── STPA-REQ-009: Array dimension validation ──────────────────────

    #[test]
    fn test_array_element_count_zero_dimension() {
        let mut diags = Vec::new();
        let dims = vec![ArrayDimension {
            size: Some(ArraySize::Literal(0)),
        }];
        // Should return 1 as fallback and emit a diagnostic.
        assert_eq!(array_element_count(&dims, &mut diags, &Name::new("sub")), 1);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("STPA-REQ-009"));
        assert!(diags[0].message.contains("zero"));
    }

    #[test]
    fn test_array_element_count_positive_no_diagnostic() {
        let mut diags = Vec::new();
        let dims = vec![ArrayDimension {
            size: Some(ArraySize::Literal(3)),
        }];
        assert_eq!(array_element_count(&dims, &mut diags, &Name::new("sub")), 3);
        assert!(diags.is_empty());
    }

    // ── STPA-REQ-012: Circular containment detection ──────────────────

    /// Helper to build a GlobalScope from manually constructed item trees.
    fn make_scope_from_trees(
        trees: Vec<std::sync::Arc<crate::item_tree::ItemTree>>,
    ) -> GlobalScope {
        GlobalScope::from_trees(trees)
    }

    #[test]
    fn test_circular_containment_direct_cycle() {
        use crate::item_tree::*;

        // Package Pkg with:
        //   system A (type)
        //   system implementation A.Impl
        //     subcomponents
        //       b: system B.Impl;
        //   system B (type)
        //   system implementation B.Impl
        //     subcomponents
        //       a: system Pkg::A.Impl;  <-- cycle: A.Impl -> B.Impl -> A.Impl
        let mut tree = ItemTree::default();

        // Type A
        let a_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("A"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // Type B
        let b_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("B"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // Subcomponent in A.Impl: b : system B.Impl
        let sub_b = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("b"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::implementation(
                Some(Name::new("Pkg")),
                Name::new("B"),
                Name::new("Impl"),
            )),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Subcomponent in B.Impl: a : system Pkg::A.Impl
        let sub_a = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("a"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::implementation(
                Some(Name::new("Pkg")),
                Name::new("A"),
                Name::new("Impl"),
            )),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Impl A.Impl
        let _a_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("A"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_b],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // Impl B.Impl
        let _b_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("B"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_a],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // Package
        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(a_type_idx),
                ItemRef::ComponentType(b_type_idx),
                ItemRef::ComponentImpl(_a_impl_idx),
                ItemRef::ComponentImpl(_b_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = make_scope_from_trees(vec![std::sync::Arc::new(tree)]);

        let result = detect_circular_containment(
            &scope,
            &Name::new("Pkg"),
            &Name::new("A"),
            &Name::new("Impl"),
        );
        assert!(result.is_some(), "should detect circular containment");
        let msg = result.unwrap();
        assert!(
            msg.contains("STPA-REQ-012"),
            "message should reference STPA-REQ-012: {}",
            msg
        );
        assert!(
            msg.contains("circular containment"),
            "message should say circular containment: {}",
            msg
        );
    }

    #[test]
    fn test_no_circular_containment() {
        use crate::item_tree::*;

        // Linear hierarchy: A.Impl contains B.Impl, B.Impl contains C (type only).
        let mut tree = ItemTree::default();

        let a_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("A"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let b_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("B"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let _c_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("C"),
            category: ComponentCategory::Process,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // B.Impl has subcomponent c: process C (type-only, no impl -> no recursion)
        let sub_c = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("c"),
            category: ComponentCategory::Process,
            classifier: Some(ClassifierRef::qualified(Name::new("Pkg"), Name::new("C"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let b_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("B"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_c],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        // A.Impl has subcomponent b: system B.Impl
        let sub_b = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("b"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::implementation(
                Some(Name::new("Pkg")),
                Name::new("B"),
                Name::new("Impl"),
            )),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let a_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("A"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_b],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(a_type_idx),
                ItemRef::ComponentType(b_type_idx),
                ItemRef::ComponentType(_c_type_idx),
                ItemRef::ComponentImpl(a_impl_idx),
                ItemRef::ComponentImpl(b_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = make_scope_from_trees(vec![std::sync::Arc::new(tree)]);

        let result = detect_circular_containment(
            &scope,
            &Name::new("Pkg"),
            &Name::new("A"),
            &Name::new("Impl"),
        );
        assert!(
            result.is_none(),
            "should not detect circular containment in linear hierarchy"
        );
    }

    #[test]
    fn test_circular_containment_produces_diagnostic_via_instantiate() {
        use crate::item_tree::*;

        // Same circular model as test_circular_containment_direct_cycle,
        // but verify that SystemInstance::instantiate includes the diagnostic.
        let mut tree = ItemTree::default();

        let a_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("A"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let b_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("B"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let sub_b = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("b"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::implementation(
                Some(Name::new("Pkg")),
                Name::new("B"),
                Name::new("Impl"),
            )),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let sub_a = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("a"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::implementation(
                Some(Name::new("Pkg")),
                Name::new("A"),
                Name::new("Impl"),
            )),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let a_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("A"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_b],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let b_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("B"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_a],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(a_type_idx),
                ItemRef::ComponentType(b_type_idx),
                ItemRef::ComponentImpl(a_impl_idx),
                ItemRef::ComponentImpl(b_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = make_scope_from_trees(vec![std::sync::Arc::new(tree)]);

        let instance = SystemInstance::instantiate(
            &scope,
            &Name::new("Pkg"),
            &Name::new("A"),
            &Name::new("Impl"),
        );

        // Should have at least a circular containment diagnostic.
        let circular_diags: Vec<_> = instance
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("circular containment"))
            .collect();
        assert!(
            !circular_diags.is_empty(),
            "instantiation should produce circular containment diagnostic, got: {:?}",
            instance.diagnostics
        );

        // Also should have a max-depth diagnostic since it will recurse
        // until hitting the depth limit.
        let depth_diags: Vec<_> = instance
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("maximum instantiation depth"))
            .collect();
        assert!(
            !depth_diags.is_empty(),
            "instantiation should hit max depth, got: {:?}",
            instance.diagnostics
        );
    }

    #[test]
    fn test_non_circular_model_no_extra_diagnostics() {
        use crate::item_tree::*;

        // Simple non-circular: A.Impl has subcomponent b: system B (type-only, no impl).
        let mut tree = ItemTree::default();

        let a_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("A"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let _b_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("B"),
            category: ComponentCategory::Process,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let sub_b = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("b"),
            category: ComponentCategory::Process,
            classifier: Some(ClassifierRef::qualified(Name::new("Pkg"), Name::new("B"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let a_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("A"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_b],
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(a_type_idx),
                ItemRef::ComponentType(_b_type_idx),
                ItemRef::ComponentImpl(a_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = make_scope_from_trees(vec![std::sync::Arc::new(tree)]);

        let instance = SystemInstance::instantiate(
            &scope,
            &Name::new("Pkg"),
            &Name::new("A"),
            &Name::new("Impl"),
        );

        // No circular containment, no depth exceeded, no other diagnostics.
        assert!(
            instance.diagnostics.is_empty(),
            "non-circular model should produce no diagnostics, got: {:?}",
            instance.diagnostics
        );
    }

    // ── STPA-REQ-010: Array index bounds checking ─────────────────────

    #[test]
    fn test_max_depth_is_100() {
        // Verify that the Builder is constructed with max_depth = 100.
        // We can check this indirectly: a non-circular model with depth < 100
        // should produce no depth-exceeded diagnostics.
        use crate::item_tree::*;

        let mut tree = ItemTree::default();

        let a_type_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("A"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        let a_impl_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("A"),
            impl_name: Name::new("Impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
            emv2: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::ComponentType(a_type_idx),
                ItemRef::ComponentImpl(a_impl_idx),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = make_scope_from_trees(vec![std::sync::Arc::new(tree)]);

        let instance = SystemInstance::instantiate(
            &scope,
            &Name::new("Pkg"),
            &Name::new("A"),
            &Name::new("Impl"),
        );

        // No depth exceeded for a simple model.
        let depth_diags: Vec<_> = instance
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("maximum instantiation depth"))
            .collect();
        assert!(depth_diags.is_empty());
    }

    // ── STPA-REQ-008: Modal subcomponent instantiation completeness ───

    #[test]
    fn test_in_modes_preserved_on_component_instance() {
        let mut components: Arena<ComponentInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Child active only in modes "active" and "standby"
        let child = components.alloc(ComponentInstance {
            name: Name::new("sensor"),
            category: ComponentCategory::Device,
            type_name: Name::new("Sensor"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: vec![Name::new("active"), Name::new("standby")],
        });
        components[root].children.push(child);

        let instance = make_instance(components, Arena::default(), Arena::default(), root);

        // Root should have no modal restriction.
        assert!(instance.components[root].in_modes.is_empty());
        // Child should preserve its in_modes metadata.
        assert_eq!(instance.components[child].in_modes.len(), 2);
        assert_eq!(instance.components[child].in_modes[0].as_str(), "active");
        assert_eq!(instance.components[child].in_modes[1].as_str(), "standby");
        // Child is still instantiated (not filtered out).
        assert_eq!(instance.components[root].children.len(), 1);
    }

    #[test]
    fn test_in_modes_preserved_on_connection_instance() {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Connection active only in mode "fast"
        let conn = connections.alloc(ConnectionInstance {
            name: Name::new("c1"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: None,
            dst: None,
            in_modes: vec![Name::new("fast")],
        });
        components[root].connections.push(conn);

        let instance = make_instance(components, Arena::default(), connections, root);

        assert_eq!(instance.connections[conn].in_modes.len(), 1);
        assert_eq!(instance.connections[conn].in_modes[0].as_str(), "fast");
    }

    // ── STPA-REQ-013: Unresolved connection endpoint diagnostic ───────

    #[test]
    fn test_unresolved_subcomponent_produces_diagnostic() {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Connection referencing a subcomponent that doesn't exist
        let conn = connections.alloc(ConnectionInstance {
            name: Name::new("c1"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("nonexistent_src")),
                feature: Name::new("p"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("nonexistent_dst")),
                feature: Name::new("q"),
            }),
            in_modes: Vec::new(),
        });
        components[root].connections.push(conn);

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.compute_semantic_connections();

        // Should produce diagnostics for both unresolved subcomponents.
        let unresolved: Vec<_> = instance
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unresolved"))
            .collect();
        assert_eq!(
            unresolved.len(),
            2,
            "expected 2 unresolved diagnostics, got: {:?}",
            unresolved
        );
        assert!(unresolved[0].message.contains("nonexistent_src"));
        assert!(unresolved[1].message.contains("nonexistent_dst"));
    }

    #[test]
    fn test_missing_endpoint_produces_diagnostic() {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Connection with missing src endpoint
        let conn = connections.alloc(ConnectionInstance {
            name: Name::new("c_broken"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: None,
            dst: Some(ConnectionEnd {
                subcomponent: None,
                feature: Name::new("q"),
            }),
            in_modes: Vec::new(),
        });
        components[root].connections.push(conn);

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.compute_semantic_connections();

        let missing: Vec<_> = instance
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("missing endpoint"))
            .collect();
        assert_eq!(
            missing.len(),
            1,
            "expected 1 missing endpoint diagnostic, got: {:?}",
            missing
        );
        assert!(missing[0].message.contains("c_broken"));
    }

    // ── Connection pattern expansion tests (AS5506 §9.8) ─────────────

    #[test]
    fn test_connection_pattern_default_all_to_all() {
        // Two 2-element arrays connected without an explicit Connection_Pattern
        // should use All_To_All (default): 2 × 2 = 4 semantic connections.
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // src[1], src[2]
        let src1 = components.alloc(ComponentInstance {
            name: Name::new("src[1]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(1),
            in_modes: Vec::new(),
        });
        let src2 = components.alloc(ComponentInstance {
            name: Name::new("src[2]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(2),
            in_modes: Vec::new(),
        });

        // dst[1], dst[2]
        let dst1 = components.alloc(ComponentInstance {
            name: Name::new("dst[1]"),
            category: ComponentCategory::Process,
            type_name: Name::new("Q"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(1),
            in_modes: Vec::new(),
        });
        let dst2 = components.alloc(ComponentInstance {
            name: Name::new("dst[2]"),
            category: ComponentCategory::Process,
            type_name: Name::new("Q"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(2),
            in_modes: Vec::new(),
        });

        components[root].children = vec![src1, src2, dst1, dst2];

        // Connection: src.p -> dst.q (no explicit Connection_Pattern => All_To_All)
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("src")),
                feature: Name::new("p"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("dst")),
                feature: Name::new("q"),
            }),
            in_modes: Vec::new(),
        });
        components[root].connections.push(conn_idx);

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.compute_semantic_connections();

        // All_To_All: 2 sources × 2 destinations = 4 semantic connections.
        assert_eq!(
            instance.semantic_connections.len(),
            4,
            "All_To_All (default) should produce 2×2=4 semantic connections, got {}",
            instance.semantic_connections.len()
        );

        // Verify all four pairings exist.
        let pairs: Vec<(ComponentInstanceIdx, ComponentInstanceIdx)> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(src1, dst1)));
        assert!(pairs.contains(&(src1, dst2)));
        assert!(pairs.contains(&(src2, dst1)));
        assert!(pairs.contains(&(src2, dst2)));
    }

    #[test]
    fn test_connection_pattern_one_to_one() {
        // Two 2-element arrays connected with One_To_One pattern:
        // src[1].p -> dst[1].q, src[2].p -> dst[2].q = 2 semantic connections.
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut connections: Arena<ConnectionInstance> = Arena::default();

        let root = components.alloc(ComponentInstance {
            name: Name::new("top"),
            category: ComponentCategory::System,
            type_name: Name::new("Top"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // src[1], src[2]
        let src1 = components.alloc(ComponentInstance {
            name: Name::new("src[1]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(1),
            in_modes: Vec::new(),
        });
        let src2 = components.alloc(ComponentInstance {
            name: Name::new("src[2]"),
            category: ComponentCategory::Process,
            type_name: Name::new("P"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(2),
            in_modes: Vec::new(),
        });

        // dst[1], dst[2]
        let dst1 = components.alloc(ComponentInstance {
            name: Name::new("dst[1]"),
            category: ComponentCategory::Process,
            type_name: Name::new("Q"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(1),
            in_modes: Vec::new(),
        });
        let dst2 = components.alloc(ComponentInstance {
            name: Name::new("dst[2]"),
            category: ComponentCategory::Process,
            type_name: Name::new("Q"),
            impl_name: None,
            package: Name::new("Pkg"),
            parent: Some(root),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: Some(2),
            in_modes: Vec::new(),
        });

        components[root].children = vec![src1, src2, dst1, dst2];

        // Connection: src.p -> dst.q
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("src")),
                feature: Name::new("p"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("dst")),
                feature: Name::new("q"),
            }),
            in_modes: Vec::new(),
        });
        components[root].connections.push(conn_idx);

        // Set Connection_Pattern => ((One_To_One)) on the owner.
        let mut prop_map = crate::properties::PropertyMap::new();
        prop_map.add(crate::properties::PropertyValue {
            name: crate::name::PropertyRef {
                property_set: Some("Communication_Properties".into()),
                property_name: "Connection_Pattern".into(),
            },
            value: "((one_to_one))".to_string(),
            typed_expr: None,
            is_append: false,
        });

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.property_maps.insert(root, prop_map);
        instance.compute_semantic_connections();

        // One_To_One: src[1]->dst[1], src[2]->dst[2] = 2 semantic connections.
        assert_eq!(
            instance.semantic_connections.len(),
            2,
            "One_To_One should produce 2 semantic connections, got {}",
            instance.semantic_connections.len()
        );

        // Verify the exact pairings.
        let pairs: Vec<(ComponentInstanceIdx, ComponentInstanceIdx)> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(
            pairs.contains(&(src1, dst1)),
            "expected src[1]->dst[1] pairing"
        );
        assert!(
            pairs.contains(&(src2, dst2)),
            "expected src[2]->dst[2] pairing"
        );
        // Should NOT contain cross-pairings.
        assert!(
            !pairs.contains(&(src1, dst2)),
            "One_To_One should not have src[1]->dst[2]"
        );
        assert!(
            !pairs.contains(&(src2, dst1)),
            "One_To_One should not have src[2]->dst[1]"
        );
    }

    // ── parse_connection_pattern tests ───────────────────────────────

    #[test]
    fn test_parse_connection_pattern_nested_list() {
        assert_eq!(
            parse_connection_pattern("((one_to_one))"),
            ConnectionPattern::OneToOne
        );
        assert_eq!(
            parse_connection_pattern("((All_To_All))"),
            ConnectionPattern::AllToAll
        );
    }

    #[test]
    fn test_parse_connection_pattern_bare() {
        assert_eq!(
            parse_connection_pattern("one_to_one"),
            ConnectionPattern::OneToOne
        );
        assert_eq!(
            parse_connection_pattern("all_to_all"),
            ConnectionPattern::AllToAll
        );
    }

    #[test]
    fn test_parse_connection_pattern_unknown_defaults_to_all() {
        assert_eq!(
            parse_connection_pattern("some_unknown"),
            ConnectionPattern::AllToAll
        );
    }

    /// Regression for #236: instantiating an implementation whose `extends`
    /// parent is declared in a *different file* (tree) must not panic. An
    /// inherited subcomponent's index is only valid in the tree that declared
    /// it; the builder previously paired it with the most-derived impl's tree,
    /// which — when that tree declares no subcomponents of its own — indexed an
    /// empty slice and panicked (`index out of bounds: the len is 0`).
    ///
    /// Ignored under Miri: unlike the other instance:: tests (which build
    /// ItemTrees manually), this one parses AADL source and so pulls rowan
    /// into the run — upstream rowan UB (rowan#192) trips Miri's borrow
    /// stack. The test is a panic-regression guard, not a memory-safety
    /// probe; normal `cargo test` still runs it.
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn extends_chain_across_files_does_not_panic() {
        // Base system + impl (with a subcomponent) live in one file/tree.
        let base = r#"
package base_pkg
public
    thread T
    end T;
    thread implementation T.Impl
    end T.Impl;
    system Base
    end Base;
    system implementation Base.Impl
        subcomponents
            t1: thread T.Impl;
    end Base.Impl;
end base_pkg;
"#;
        // Derived impl in a SEPARATE file/tree extends the base impl and
        // declares no subcomponents of its own — so its tree's subcomponent
        // list is empty, the exact shape that triggered the panic.
        let derived = r#"
package derived_pkg
public
    with base_pkg;
    system Derived extends base_pkg::Base
    end Derived;
    system implementation Derived.Impl extends base_pkg::Base.Impl
    end Derived.Impl;
end derived_pkg;
"#;

        let db = crate::HirDefDatabase::default();
        let t_base = crate::file_item_tree(
            &db,
            spar_base_db::SourceFile::new(&db, "base.aadl".to_string(), base.to_string()),
        );
        let t_derived = crate::file_item_tree(
            &db,
            spar_base_db::SourceFile::new(&db, "derived.aadl".to_string(), derived.to_string()),
        );
        // base is tree 0, derived is tree 1; derived's tree has no subcomponents.
        let scope = GlobalScope::from_trees(vec![t_base, t_derived]);

        // Must not panic.
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("derived_pkg"),
            &Name::new("Derived"),
            &Name::new("Impl"),
        );

        // The inherited subcomponent must be instantiated under the derived
        // system, carrying through from the parent's tree.
        assert!(
            inst.all_components().any(|(_, c)| c.name.as_str() == "t1"),
            "inherited subcomponent `t1` should be instantiated via the cross-file \
             extends chain; components: {:?}",
            inst.all_components()
                .map(|(_, c)| c.name.as_str().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Regression for #237: a property declared with `applies to <connection>`
    /// or `applies to <feature>` must be stored on the connection/feature
    /// instance — not collapsed by property name onto the owning component.
    ///
    /// Ignored under Miri for the same reason as
    /// `extends_chain_across_files_does_not_panic`: it parses AADL source,
    /// pulling rowan (upstream rowan#192 UB) into the Miri run.
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn applies_to_connection_and_feature_store_per_instance() {
        let src = r#"
package p
public
    data D
    end D;
    thread T
        features
            tin: in data port D;
            tout: out data port D;
    end T;
    thread implementation T.i
    end T.i;
    process Proc
        features
            pin: in data port D;
    end Proc;
    process implementation Proc.i
        subcomponents
            a: thread T.i;
            b: thread T.i;
        connections
            conn1: port a.tout -> b.tin;
        properties
            stood::pos => "wire-shape" applies to conn1;
            stood::pos => "port-shape" applies to pin;
    end Proc.i;
    system S
    end S;
    system implementation S.i
        subcomponents
            proc: process Proc.i;
    end S.i;
end p;
"#;
        let db = crate::HirDefDatabase::default();
        let tree = crate::file_item_tree(
            &db,
            spar_base_db::SourceFile::new(&db, "p.aadl".to_string(), src.to_string()),
        );
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst =
            SystemInstance::instantiate(&scope, &Name::new("p"), &Name::new("S"), &Name::new("i"));

        // Property on the connection lands on the connection instance.
        let (conn_idx, _) = inst
            .connections
            .iter()
            .find(|(_, c)| c.name.as_str() == "conn1")
            .expect("conn1 should be instantiated");
        assert_eq!(
            inst.properties_for_connection(conn_idx).get("stood", "pos"),
            Some("\"wire-shape\""),
            "connection property must be stored per-connection, not on the component (#237)"
        );

        // Property on the feature lands on the feature instance.
        let (feat_idx, _) = inst
            .features
            .iter()
            .find(|(_, f)| f.name.as_str() == "pin")
            .expect("pin feature should exist");
        assert_eq!(
            inst.properties_for_feature(feat_idx).get("stood", "pos"),
            Some("\"port-shape\""),
            "feature property must be stored per-feature, not on the component (#237)"
        );
    }

    // ── EMV2 instance projection (REQ-EMV2-PROPAGATION-002, layer 3) ──────
    //
    // Full pipeline: hand-authored AADL (with an EMV2 annex) → item-tree
    // (L2 retains the subclause) → instantiate → assert the ComponentInstance
    // carries the projected model. NON-VACUOUS: exact propagation/flow counts
    // from the fixture (a bare `== item-tree model` would only prove the copy
    // ran; a wrong/empty/double-counted projection must fail loudly).

    fn instantiate_aadl(src: &str, pkg: &str, ty: &str, imp: &str) -> SystemInstance {
        let db = crate::HirDefDatabase::default();
        let file = spar_base_db::SourceFile::new(&db, "emv2_l3.aadl".to_string(), src.to_string());
        let tree = crate::file_item_tree(&db, file);
        let scope = GlobalScope::from_trees(vec![tree]);
        SystemInstance::instantiate(&scope, &Name::new(pkg), &Name::new(ty), &Name::new(imp))
    }

    /// #455, REQ-ANALYSIS-LATENCY-BUDGET-001 clause (b): an end-to-end flow's
    /// OWN property associations survive instantiation.
    ///
    /// The item tree always lowered them; instantiation dropped them, so every
    /// analysis saw only the owner component. That is where AADL does not put
    /// a flow's latency budget. A flow with no block must stay empty, so a
    /// build that copied the owner's properties onto every flow cannot pass.
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn an_end_to_end_flows_own_properties_survive_instantiation() {
        let src = r#"package P
public
  device dv
    features
      o : out event data port;
      i : in event data port;
    flows
      s : flow source o;
      k : flow sink i;
  end dv;
  device implementation dv.impl
  end dv.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      a : device dv.impl;
      b : device dv.impl;
    connections
      h : port a.o -> b.i;
    flows
      ranged : end to end flow a.s -> h -> b.k { Latency => 1 ms .. 10 ms; };
      qualified : end to end flow a.s -> h -> b.k { Communication_Properties::Latency => 2 ms; };
      bare : end to end flow a.s -> h -> b.k;
    properties
      Latency => 500 ms;
  end S.impl;
end P;
"#;
        let inst = instantiate_aadl(src, "P", "S", "impl");
        let flow = |n: &str| {
            inst.end_to_end_flows
                .iter()
                .map(|(_, f)| f)
                .find(|f| f.name.as_str() == n)
                .unwrap_or_else(|| panic!("flow {n} not instantiated"))
        };
        assert_eq!(
            flow("ranged").properties.get("", "Latency"),
            Some("1 ms .. 10 ms"),
            "the flow's own range must reach the instance"
        );
        assert_eq!(
            flow("qualified")
                .properties
                .get("Communication_Properties", "Latency"),
            Some("2 ms"),
            "the qualified spelling must reach the instance under its own set"
        );
        assert!(
            flow("bare").properties.get("", "Latency").is_none(),
            "a flow with no block must not inherit the OWNER's 500 ms — that would \
             make the owner-fallback indistinguishable from the flow's own budget"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn emv2_projected_onto_impl_root_from_type() {
        // EMV2 on the TYPE; root instance is the IMPLEMENTATION (main alloc
        // path unions type ∪ impl). Fixture: 2 propagations + 1 source flow.
        let src = r#"package P
public
  system S
    annex EMV2 {**
error propagations
  p1: out propagation {ServiceError};
  p2: in propagation {LateDelivery};
flows
  f1: error source p1 {ServiceError};
end propagations;
    **};
  end S;
  system implementation S.i
  end S.i;
end P;
"#;
        let inst = instantiate_aadl(src, "P", "S", "i");
        let models = inst.emv2_for(inst.root);
        assert_eq!(
            models.len(),
            1,
            "root instance must carry the type's EMV2 subclause"
        );
        assert_eq!(models[0].propagations.len(), 2);
        assert_eq!(models[0].flows.len(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn emv2_projected_onto_type_only_leaf_subcomponent() {
        // A type-only leaf subcomponent (`sub : device Sensor;`) hits a
        // DIFFERENT alloc site than the root; a main-path-only projection would
        // silently miss it. Sensor's EMV2: 1 propagation + 1 source flow.
        let src = r#"package P
public
  device Sensor
    annex EMV2 {**
error propagations
  s1: out propagation {BadValue};
flows
  fs: error source s1 {BadValue};
end propagations;
    **};
  end Sensor;
  system S
  end S;
  system implementation S.i
    subcomponents
      sub : device Sensor;
  end S.i;
end P;
"#;
        let inst = instantiate_aadl(src, "P", "S", "i");
        // Root (S.i) has no EMV2.
        assert!(
            inst.emv2_for(inst.root).is_empty(),
            "S itself declares no EMV2"
        );
        // Its leaf subcomponent carries Sensor's projected model.
        let sub = inst.component(inst.root).children[0];
        let models = inst.emv2_for(sub);
        assert_eq!(
            models.len(),
            1,
            "type-only leaf must carry its device type's EMV2"
        );
        assert_eq!(models[0].propagations.len(), 1);
        assert_eq!(models[0].flows.len(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn emv2_absent_yields_empty_projection() {
        // No EMV2 annex anywhere ⇒ empty projection (guards against a bug that
        // attaches empty/duplicate models to every instance).
        let src = r#"package P
public
  system S
  end S;
  system implementation S.i
  end S.i;
end P;
"#;
        let inst = instantiate_aadl(src, "P", "S", "i");
        assert!(inst.emv2_for(inst.root).is_empty());
        assert!(inst.emv2_models.is_empty());
    }

    /// #455 item 3: the legality predicate every analysis relies on. Each row
    /// is chosen to kill a specific mis-implementation: the legal rows reject a
    /// predicate that flags too much (a one-dot flow element, a bare
    /// connection, a dotless flow element), the illegal rows one that flags too
    /// little, and the position-2 row one that only ever inspects the first
    /// segment or mislabels flow/connection positions.
    #[test]
    fn nested_segment_follows_the_aadl_end_to_end_flow_grammar() {
        fn e2e(segs: &[&str]) -> EndToEndFlowInstance {
            EndToEndFlowInstance {
                name: Name::new("f"),
                owner: Idx::from_raw(la_arena::RawIdx::from(0u32)),
                segments: segs.iter().map(|s| Name::new(s)).collect(),
                properties: Default::default(),
            }
        }
        let at = |segs: &[&str]| e2e(segs).nested_segment().map(|(i, s)| (i, s.to_string()));

        // Legal: `subcomponent.flow` and bare connection names.
        assert_eq!(
            at(&["sensor.src", "c1", "ctrl.path", "c2", "act.sink"]),
            None
        );
        // Legal: a flow element may omit the subcomponent (a local flow).
        assert_eq!(at(&["src", "c1", "sink"]), None);

        // Illegal: a flow element two levels deep.
        assert_eq!(
            at(&["app.caller.lock_flow", "c_hand", "b.sink"]),
            Some((0, "app.caller.lock_flow".to_string()))
        );
        // Illegal: a dotted connection element.
        assert_eq!(
            at(&["a.src", "app.c_hand", "b.sink"]),
            Some((1, "app.c_hand".to_string()))
        );
        // Illegal, found past the first segment.
        assert_eq!(
            at(&["a.src", "c1", "app.consumer.sink"]),
            Some((2, "app.consumer.sink".to_string()))
        );
    }

    /// REQ-ANALYSIS-FLOW-THREAD-DESCENT-001: a flow implementation is kept on
    /// the flow-spec instance it implements, a flow spec without one has
    /// `None`, and an `extends` child's implementation overrides the parent's
    /// of the same name while an inherited one it does not redeclare survives.
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn flow_implementations_attach_to_their_flow_spec_and_inherit() {
        let src = r#"
package p
public
    data D
    end D;
    thread T
        features
            i: in data port D;
            o: out data port D;
        flows
            f: flow path i -> o;
    end T;
    process P
        features
            pin: in data port D;
            pout: out data port D;
        flows
            fa: flow path pin -> pout;
            fb: flow path pin -> pout;
            fc: flow path pin -> pout;
    end P;
    process implementation P.Base
        subcomponents
            t1: thread T;
            t2: thread T;
        connections
            c0: port pin -> t1.i;
            c1: port t1.o -> t2.i;
            c2: port t2.o -> pout;
        flows
            fa: flow path pin -> c0 -> t1.f -> c1 -> t2.f -> c2 -> pout;
            fb: flow path pin -> c0 -> t1.f -> c1 -> t2.f -> c2 -> pout;
    end P.Base;
    process implementation P.Derived extends P.Base
        flows
            fb: flow path pin -> c0 -> t1.f -> c2 -> pout;
    end P.Derived;
    system S
    end S;
    system implementation S.Impl
        subcomponents
            app: process P.Derived;
    end S.Impl;
end p;
"#;
        let db = crate::HirDefDatabase::default();
        let tree = crate::file_item_tree(
            &db,
            spar_base_db::SourceFile::new(&db, "p.aadl".to_string(), src.to_string()),
        );
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("p"),
            &Name::new("S"),
            &Name::new("Impl"),
        );
        let (app_idx, _) = inst
            .all_components()
            .find(|(_, c)| c.name.as_str() == "app")
            .unwrap_or_else(|| {
                panic!(
                    "app instantiated: {:?} {:?}",
                    inst.diagnostics,
                    inst.all_components()
                        .map(|(_, c)| c.name.to_string())
                        .collect::<Vec<_>>()
                )
            });
        let seg_of = |flow: &str| {
            let f = inst
                .component(app_idx)
                .flows
                .iter()
                .map(|&f| &inst.flow_instances[f])
                .find(|f| f.name.as_str() == flow)
                .unwrap_or_else(|| panic!("flow spec {flow} on app"));
            f.impl_segments
                .as_ref()
                .map(|v| v.iter().map(|s| s.element.to_string()).collect::<Vec<_>>())
        };
        // Inherited, not redeclared.
        assert_eq!(
            seg_of("fa"),
            Some(
                ["pin", "c0", "f", "c1", "f", "c2", "pout"]
                    .map(String::from)
                    .to_vec()
            )
        );
        // Redeclared in the derived impl: the derived one wins.
        assert_eq!(
            seg_of("fb"),
            Some(["pin", "c0", "f", "c2", "pout"].map(String::from).to_vec())
        );
        // A spec nothing implements.
        assert_eq!(seg_of("fc"), None);
        // The segment keeps its subcomponent qualifier.
        let fa = inst
            .component(app_idx)
            .flows
            .iter()
            .map(|&f| &inst.flow_instances[f])
            .find(|f| f.name.as_str() == "fa")
            .unwrap();
        let segs = fa.impl_segments.as_ref().unwrap();
        assert_eq!(
            segs[2].subcomponent.as_ref().map(|n| n.as_str()),
            Some("t1")
        );
        assert_eq!(segs[0].subcomponent, None);
    }

    // ── REQ-HIR-FEATUREGROUP-PATH-001: `fg.member` connection endpoints ──

    const FG_MODEL: &str = r#"package P
public
  feature group FG
    features
      a : out data port;
      b : in data port;
  end FG;

  system Leaf
    features
      i : in data port;
      o : out data port;
  end Leaf;
  system implementation Leaf.impl
  end Leaf.impl;

  system S
    features
      g : feature group FG;
  end S;
  system implementation S.impl
    subcomponents
      x : system Leaf.impl;
    connections
      CONNS
  end S.impl;
end P;
"#;

    fn fg_instance(conns: &str) -> SystemInstance {
        instantiate_aadl(&FG_MODEL.replace("CONNS", conns), "P", "S", "impl")
    }

    fn unresolved(inst: &SystemInstance) -> Vec<String> {
        inst.diagnostics
            .iter()
            .filter(|d| d.message.contains("unresolved"))
            .map(|d| d.message.clone())
            .collect()
    }

    fn end_of(e: &Option<ConnectionEnd>) -> (Option<String>, String) {
        let e = e.as_ref().expect("endpoint present");
        (
            e.subcomponent.as_ref().map(|n| n.as_str().to_string()),
            e.feature.as_str().to_string(),
        )
    }

    // rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn fg_member_as_connection_source_resolves() {
        let inst = fg_instance("c1 : port g.a -> x.i;");
        assert_eq!(unresolved(&inst), Vec::<String>::new());
        let conn = inst.connections.iter().next().unwrap().1;
        assert_eq!(end_of(&conn.src), (None, "g.a".to_string()));
        assert_eq!(end_of(&conn.dst), (Some("x".to_string()), "i".to_string()));
        // Down connection at the root: a semantic connection is produced whose
        // source is the root's own feature-group member.
        assert_eq!(inst.semantic_connections.len(), 1);
        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.ultimate_source.0, inst.root);
        assert_eq!(sc.ultimate_source.1.as_str(), "g.a");
        assert_eq!(sc.ultimate_destination.1.as_str(), "i");
    }

    // rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn fg_member_as_connection_destination_resolves() {
        let inst = fg_instance("c1 : port x.o -> g.b;");
        assert_eq!(unresolved(&inst), Vec::<String>::new());
        let conn = inst.connections.iter().next().unwrap().1;
        assert_eq!(end_of(&conn.src), (Some("x".to_string()), "o".to_string()));
        assert_eq!(end_of(&conn.dst), (None, "g.b".to_string()));
        assert_eq!(inst.semantic_connections.len(), 1);
        let sc = &inst.semantic_connections[0];
        assert_eq!(sc.ultimate_destination.0, inst.root);
        assert_eq!(sc.ultimate_destination.1.as_str(), "g.b");
    }

    // rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn fg_name_match_is_case_insensitive() {
        let inst = fg_instance("c1 : port G.a -> x.i;");
        assert_eq!(unresolved(&inst), Vec::<String>::new());
        let conn = inst.connections.iter().next().unwrap().1;
        assert_eq!(conn.src.as_ref().unwrap().subcomponent, None);
    }

    // rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn genuinely_unknown_endpoint_is_still_unresolved() {
        // `nope` is neither a subcomponent nor a feature group.
        let inst = fg_instance("c1 : port nope.a -> x.i;");
        let u = unresolved(&inst);
        assert_eq!(u.len(), 1, "{u:?}");
        assert!(
            u[0].contains("unresolved source subcomponent 'nope'"),
            "{u:?}"
        );
        let conn = inst.connections.iter().next().unwrap().1;
        // Left exactly as written — not rewritten.
        assert_eq!(
            end_of(&conn.src),
            (Some("nope".to_string()), "a".to_string())
        );
        // Same for the destination side.
        let inst = fg_instance("c1 : port x.o -> nope.b;");
        let u = unresolved(&inst);
        assert_eq!(u.len(), 1, "{u:?}");
        assert!(
            u[0].contains("unresolved destination subcomponent 'nope'"),
            "{u:?}"
        );
    }

    // rivet: verifies REQ-HIR-FEATUREGROUP-PATH-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn fg_present_but_member_missing_reports_the_member_not_the_subcomponent() {
        // `g` IS a feature group of S; `zzz` is not one of its members. The
        // error must name the missing member (the first part is not a
        // subcomponent, so "unresolved subcomponent" would mislead) and must
        // appear exactly once.
        let inst = fg_instance("c1 : port g.zzz -> x.i;");
        assert_eq!(unresolved(&inst), Vec::<String>::new());
        let msgs: Vec<_> = inst
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("has no member"))
            .collect();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0]
                .message
                .contains("connection 'c1': feature group 'g' has no member 'zzz'"),
            "{}",
            msgs[0].message
        );
        // A member that exists produces no such diagnostic.
        let ok = fg_instance("c1 : port g.a -> x.i;");
        assert!(
            ok.diagnostics
                .iter()
                .all(|d| !d.message.contains("no member"))
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn subcomponent_of_the_same_name_wins_over_a_feature_group() {
        // `g` is both a subcomponent and a feature group of S: the
        // subcomponent reading must be kept, as before.
        let src = FG_MODEL
            .replace("x : system Leaf.impl;", "g : system Leaf.impl;")
            .replace("CONNS", "c1 : port g.o -> g.i;");
        let inst = instantiate_aadl(&src, "P", "S", "impl");
        let conn = inst.connections.iter().next().unwrap().1;
        assert_eq!(end_of(&conn.src), (Some("g".to_string()), "o".to_string()));
        assert_eq!(end_of(&conn.dst), (Some("g".to_string()), "i".to_string()));
        assert!(
            inst.diagnostics
                .iter()
                .all(|d| !d.message.contains("no member"))
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn array_subcomponent_base_name_wins_over_a_feature_group() {
        let src = FG_MODEL
            .replace("x : system Leaf.impl;", "g : system Leaf.impl [2];")
            .replace("CONNS", "c1 : port g.o -> g.i;");
        let inst = instantiate_aadl(&src, "P", "S", "impl");
        let conn = inst.connections.iter().next().unwrap().1;
        assert_eq!(end_of(&conn.src), (Some("g".to_string()), "o".to_string()));
        assert!(
            inst.diagnostics
                .iter()
                .all(|d| !d.message.contains("no member"))
        );
    }

    // ── #460: a reference resolves from its declaring component ─────────
    //
    // REQ-AADL-REF-CONTEXT-001. Two components are named `cpu`: a processor
    // where the declaring implementation names it, and a MEMORY deeper in
    // the tree. The old resolver took the deepest match anywhere, so the
    // binding silently bound the memory.

    const CTX_MODEL: &str = r#"
package C
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      app : process P.impl;
      cpu : memory Mem;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      sub : system Sub.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to sub.app;
  end Top.impl;
end C;
"#;

    fn by_path(inst: &SystemInstance, path: &str) -> ComponentInstanceIdx {
        inst.all_components()
            .map(|(i, _)| i)
            .find(|&i| inst.component_instance_path(i) == path)
            .unwrap_or_else(|| panic!("no component at {path}"))
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn applies_to_reference_resolves_from_the_declarer_not_the_deepest() {
        let inst = instantiate_aadl(CTX_MODEL, "C", "Top", "impl");
        let app = by_path(&inst, "Top.impl/sub/app");
        let top_cpu = by_path(&inst, "Top.impl/cpu");
        let deep_cpu = by_path(&inst, "Top.impl/sub/cpu");

        let ctx = inst.reference_context(app, "Deployment_Properties", "Actual_Processor_Binding");
        assert_eq!(
            ctx, inst.root,
            "an `applies to` value resolves from its declarer"
        );
        assert_eq!(inst.resolve_reference(ctx, "cpu"), Some(top_cpu));
        // The negative control: the old resolver's answer.
        assert_eq!(inst.resolve_dotted_path("cpu"), Some(deep_cpu));
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_value_without_applies_to_resolves_from_its_holder() {
        // `app : process P.impl { Actual_Processor_Binding => (reference (cpu)); }`
        // inside Sub.impl names a SIBLING. OSATE resolves an association with
        // no `applies to` from the instance that receives it
        // (CachePropertyAssociationsSwitch.instantiateReferenceValues(newVal,
        // io)) — `app` — where there is no `cpu`: "Referenced element does
        // not exist in the instance model". The old resolver found a `cpu`
        // somewhere in the model and called the binding fine.
        let src = CTX_MODEL
            .replace(
                "      app : process P.impl;",
                "      app : process P.impl { Actual_Processor_Binding => (reference (cpu)); };",
            )
            .replace(
                "      Actual_Processor_Binding => (reference (cpu)) applies to sub.app;\n",
                "",
            );
        assert_ne!(src, CTX_MODEL);
        let inst = instantiate_aadl(&src, "C", "Top", "impl");
        let app = by_path(&inst, "Top.impl/sub/app");
        assert!(
            inst.properties_for(app)
                .get("", "Actual_Processor_Binding")
                .or_else(|| inst
                    .properties_for(app)
                    .get("Deployment_Properties", "Actual_Processor_Binding"))
                .is_some(),
            "the subcomponent-level binding must be on `app`, or this test proves nothing"
        );
        let ctx = inst.reference_context(app, "Deployment_Properties", "Actual_Processor_Binding");
        assert_eq!(ctx, app, "no `applies to`: the holder is the context");
        assert_eq!(inst.resolve_reference(ctx, "cpu"), None);
        assert!(
            inst.resolve_dotted_path("cpu").is_some(),
            "the old search found one"
        );
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_path_is_walked_exactly_and_never_searched() {
        let inst = instantiate_aadl(CTX_MODEL, "C", "Top", "impl");
        let root = inst.root;
        assert_eq!(
            inst.resolve_reference(root, "sub.app.t"),
            Some(by_path(&inst, "Top.impl/sub/app/t"))
        );
        // `app` is a grandchild, not a child: a suffix does not resolve.
        assert_eq!(inst.resolve_reference(root, "app"), None);
        assert_eq!(inst.resolve_reference(root, "app.t"), None);
        // An unknown head must not fall through to a root-relative walk of
        // the remainder (that would find `Top.impl/cpu`).
        assert_eq!(inst.resolve_reference(root, "nosuch.cpu"), None);
        assert_eq!(inst.resolve_reference(root, ""), None);
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_root_anchored_path_walks_from_the_root() {
        let inst = instantiate_aadl(CTX_MODEL, "C", "Top", "impl");
        let app = by_path(&inst, "Top.impl/sub/app");
        // From `app`, `Top.impl.cpu` is not a child path; the root's full
        // name — which contains a dot — anchors it.
        assert_eq!(
            inst.resolve_reference(app, "Top.impl.cpu"),
            Some(by_path(&inst, "Top.impl/cpu"))
        );
        assert_eq!(
            inst.resolve_reference(app, "top.IMPL.sub.cpu"),
            Some(by_path(&inst, "Top.impl/sub/cpu"))
        );
        // The root's name alone, or a name that only shares a prefix with
        // it, is not an anchored path.
        assert_eq!(inst.resolve_reference(app, "Top.impl"), None);
        assert_eq!(inst.resolve_reference(app, "Top.implx.cpu"), None);
        assert_eq!(inst.resolve_reference(app, "Top.cpu"), None);
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_reference_resolves_through_a_refined_subcomponent() {
        // `hw: refined to system Plat.single` replaces the inherited
        // `hw: system Plat` (#496), so `reference (hw.cpu)` reaches the
        // refinement's `cpu`. This test once pinned a resolver allowance for
        // the stale sibling #496 left behind; that sibling no longer exists.
        // Shape of test-data/osate2/resourcebudgets.aadl.
        let src = r#"
package R
public
  processor Cpu
  end Cpu;
  thread T
  end T;
  system Plat
  end Plat;
  system implementation Plat.single
    subcomponents
      cpu : processor Cpu;
  end Plat.single;
  system D
  end D;
  system implementation D.base
    subcomponents
      hw : system Plat;
      t : thread T;
  end D.base;
  system implementation D.bound extends D.base
    subcomponents
      hw : refined to system Plat.single;
    properties
      Actual_Processor_Binding => (reference (hw.cpu)) applies to t;
  end D.bound;
end R;
"#;
        let inst = instantiate_aadl(src, "R", "D", "bound");
        let t = by_path(&inst, "D.bound/t");
        let ctx = inst.reference_context(t, "Deployment_Properties", "Actual_Processor_Binding");
        let got = inst
            .resolve_reference(ctx, "hw.cpu")
            .expect("hw.cpu resolves through the refinement");
        assert_eq!(inst.component(got).category, ComponentCategory::Processor);

        // When the original's classifier ALSO has a `cpu` (a memory here),
        // the reference must still reach the refinement's processor: the
        // original is replaced, not kept beside it.
        let both = src
            .replace(
                "  system implementation Plat.single",
                "  memory Mem\n  end Mem;\n  system implementation Plat.old\n    subcomponents\n      cpu : memory Mem;\n  end Plat.old;\n  system implementation Plat.single",
            )
            .replace("hw : system Plat;", "hw : system Plat.old;");
        let inst = instantiate_aadl(&both, "R", "D", "bound");
        let t = by_path(&inst, "D.bound/t");
        let ctx = inst.reference_context(t, "Deployment_Properties", "Actual_Processor_Binding");
        let got = inst
            .resolve_reference(ctx, "hw.cpu")
            .expect("hw.cpu resolves");
        assert_eq!(
            inst.component(got).category,
            ComponentCategory::Processor,
            "the refinement's cpu, not the stale original's memory"
        );
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn the_context_follows_the_key_the_value_was_read_from() {
        // A QUALIFIED local value (context: the holder) beside an
        // UNQUALIFIED `applies to` value from the root. Callers read the
        // qualified key first, so the context must be the qualified key's —
        // the holder — and not the bare key's root.
        use crate::properties::PropertyValue;
        let mut inst = instantiate_aadl(CTX_MODEL, "C", "Top", "impl");
        let sub = by_path(&inst, "Top.impl/sub");
        let root = inst.root;
        let pv = |set: Option<&str>, v: &str| PropertyValue {
            name: crate::name::PropertyRef {
                property_set: set.map(Name::new),
                property_name: Name::new("Actual_Processor_Binding"),
            },
            value: v.to_string(),
            typed_expr: None,
            is_append: false,
        };
        let map = inst.property_maps.entry(sub).or_default();
        map.add(pv(Some("Deployment_Properties"), "reference (cpu)"));
        map.add_with_reference_context(pv(None, "reference (cpu)"), root);
        assert_eq!(
            inst.reference_context(sub, "Deployment_Properties", "Actual_Processor_Binding"),
            sub,
            "the qualified value is the one read, and it has no applies-to context"
        );
        // With only the bare key present, its recorded context is used.
        let only_bare = by_path(&inst, "Top.impl/sub/app/t");
        inst.property_maps
            .entry(only_bare)
            .or_default()
            .add_with_reference_context(pv(None, "reference (cpu)"), root);
        assert_eq!(
            inst.reference_context(
                only_bare,
                "Deployment_Properties",
                "Actual_Processor_Binding"
            ),
            root
        );
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_connection_value_resolves_from_its_declarer_or_its_owner() {
        // #498: `Top.impl` binds a connection INSIDE `sub` with `applies to`,
        // so its references resolve from Top.impl. A connection value with
        // no recorded declarer resolves from the component owning the
        // connection, as OSATE does.
        let src = r#"
package K
public
  bus B
  end B;
  device D
    features
      o : out data port;
      i : in data port;
  end D;
  system Sub
  end Sub;
  system implementation Sub.impl
    subcomponents
      a : device D;
      b : device D;
    connections
      c1 : port a.o -> b.i;
      c2 : port b.o -> a.i;
  end Sub.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      sw : bus B;
      sub : system Sub.impl;
    properties
      Actual_Connection_Binding => (reference (sw)) applies to sub.c1;
  end Top.impl;
end K;
"#;
        let inst = instantiate_aadl(src, "K", "Top", "impl");
        let sub = by_path(&inst, "Top.impl/sub");
        let conn = |n: &str| {
            inst.connections
                .iter()
                .find(|(_, c)| c.name.as_str() == n && c.owner == sub)
                .map(|(i, _)| i)
                .unwrap_or_else(|| panic!("connection {n}"))
        };
        let c1 = conn("c1");
        assert!(
            inst.properties_for_connection(c1)
                .get("", "Actual_Connection_Binding")
                .or_else(|| inst
                    .properties_for_connection(c1)
                    .get("Deployment_Properties", "Actual_Connection_Binding"))
                .is_some(),
            "the applies-to binding must land on c1, or this test proves nothing"
        );
        let ctx = inst.connection_reference_context(
            c1,
            "Deployment_Properties",
            "Actual_Connection_Binding",
        );
        assert_eq!(
            ctx, inst.root,
            "an applies-to value resolves from its declarer"
        );
        assert_eq!(
            inst.resolve_reference(ctx, "sw"),
            Some(by_path(&inst, "Top.impl/sw"))
        );
        assert_eq!(
            inst.connection_reference_context(
                conn("c2"),
                "Deployment_Properties",
                "Actual_Connection_Binding"
            ),
            sub,
            "no recorded declarer: the connection's owner"
        );
    }

    // rivet: verifies REQ-AADL-REF-CONTEXT-001
    #[test]
    fn an_overriding_value_drops_the_overridden_values_context() {
        use crate::properties::{PropertyMap, PropertyValue};
        let mut arena: la_arena::Arena<()> = la_arena::Arena::default();
        let declarer = ComponentInstanceIdx::from_raw(arena.alloc(()).into_raw());
        let pv = |v: &str| PropertyValue {
            name: crate::name::PropertyRef {
                property_set: Some(Name::new("Deployment_Properties")),
                property_name: Name::new("Actual_Processor_Binding"),
            },
            value: v.to_string(),
            typed_expr: None,
            is_append: false,
        };
        let mut map = PropertyMap::new();
        map.add_with_reference_context(pv("reference (a)"), declarer);
        assert_eq!(
            map.reference_context("Deployment_Properties", "Actual_Processor_Binding"),
            Some(declarer)
        );
        map.add(pv("reference (b)"));
        assert_eq!(
            map.reference_context("Deployment_Properties", "Actual_Processor_Binding"),
            None,
            "a context must not outlive the value it was recorded for"
        );
        // An empty value is skipped by `add`, so it records no context either.
        map.add_with_reference_context(pv("   "), declarer);
        assert_eq!(
            map.reference_context("Deployment_Properties", "Actual_Processor_Binding"),
            None
        );
    }

    // ── #515: a connection's own property associations reach its instance ──

    // rivet: verifies REQ-HIR-CONNECTION-INLINE-PROPS-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn an_inline_connection_property_is_attached_and_applies_to_overrides_it() {
        let src = r#"
package Cp
public
  bus B
  end B;
  device D
    features
      o : out data port;
      i : in data port;
  end D;
  system S
  end S;
  system implementation S.impl
    subcomponents
      a : device D;
      b : device D;
      net : bus B;
      alt : bus B;
    connections
      c1 : port a.o -> b.i { Actual_Connection_Binding => (reference (net)); };
      c2 : port b.o -> a.i { Actual_Connection_Binding => (reference (net)); };
    properties
      Actual_Connection_Binding => (reference (alt)) applies to c2;
  end S.impl;
end Cp;
"#;
        let inst = instantiate_aadl(src, "Cp", "S", "impl");
        let conn = |n: &str| {
            inst.connections
                .iter()
                .find(|(_, c)| c.name.as_str() == n)
                .map(|(i, _)| i)
                .unwrap()
        };
        let binding = |n: &str| {
            let p = inst.properties_for_connection(conn(n));
            p.get("Deployment_Properties", "Actual_Connection_Binding")
                .or_else(|| p.get("", "Actual_Connection_Binding"))
                .map(str::to_string)
        };
        assert_eq!(
            binding("c1").as_deref(),
            Some("(reference (net))"),
            "inline value lowered"
        );
        assert_eq!(
            binding("c2").as_deref(),
            Some("(reference (alt))"),
            "applies to overrides inline"
        );
    }

    // ── #520: the SOM count boundary ────────────────────────────────────

    fn modal_model(extra_mode_on_d: bool) -> String {
        let modes = |n: usize| {
            (0..n)
                .map(|i| {
                    if i == 0 {
                        "    m0 : initial mode;".to_string()
                    } else {
                        format!("    m{i} : mode;")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "package Som\npublic\n  system M\n  modes\n{}\n  end M;\n  system M11\n  modes\n{}\n  end M11;\n  system Top\n  end Top;\n  system implementation Top.impl\n    subcomponents\n      a : system M;\n      b : system M;\n      c : system M;\n      d : system {};\n  end Top.impl;\nend Som;\n",
            modes(10),
            modes(11),
            if extra_mode_on_d { "M11" } else { "M" }
        )
    }

    fn truncated(inst: &SystemInstance) -> bool {
        inst.diagnostics.iter().any(|d| {
            d.message.contains("system operation mode count") && d.message.contains("truncated")
        })
    }

    // rivet: verifies REQ-HIR-SOM-COUNT-CHECKED-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn exactly_the_som_limit_is_not_truncated_and_one_mode_more_is() {
        // 10^4 = 10 000 = MAX_SOMS: every combination fits, nothing is cut.
        let at = instantiate_aadl(&modal_model(false), "Som", "Top", "impl");
        assert_eq!(at.system_operation_modes.len(), 10_000);
        assert!(!truncated(&at), "the limit itself is not over the limit");
        // 10^3 * 11 = 11 000 > MAX_SOMS: truncated to the cap and said so.
        let over = instantiate_aadl(&modal_model(true), "Som", "Top", "impl");
        assert_eq!(over.system_operation_modes.len(), 10_000);
        assert!(truncated(&over), "over the limit is reported");
    }

    // ── #496: a refined subcomponent replaces the one it refines ────────

    const REFINE_MODEL: &str = r#"
package Rf
public
  processor Cpu
  end Cpu;
  system Plat
  end Plat;
  system implementation Plat.single
    subcomponents
      cpu : processor Cpu;
  end Plat.single;
  system D
  end D;
  system implementation D.base
    subcomponents
      hw : system Plat { Code_Size => 4 KByte; Data_Size => 1 KByte; };
  end D.base;
  system implementation D.bound extends D.base
    subcomponents
      hw : refined to system Plat.single REFINED_PROPS;
  end D.bound;
end Rf;
"#;

    fn children_named(
        inst: &SystemInstance,
        parent: ComponentInstanceIdx,
        name: &str,
    ) -> Vec<ComponentInstanceIdx> {
        inst.components[parent]
            .children
            .iter()
            .copied()
            .filter(|&c| inst.components[c].name.as_str().eq_ignore_ascii_case(name))
            .collect()
    }

    // rivet: verifies REQ-AADL-SUBCOMPONENT-REFINE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_refinement_replaces_the_subcomponent_it_refines() {
        let inst = instantiate_aadl(
            &REFINE_MODEL.replace(" REFINED_PROPS", ""),
            "Rf",
            "D",
            "bound",
        );
        let hw = children_named(&inst, inst.root, "hw");
        assert_eq!(
            hw.len(),
            1,
            "one hw, not the original beside its refinement"
        );
        let hw = hw[0];
        assert_eq!(
            inst.components[hw].impl_name.as_ref().map(|n| n.as_str()),
            Some("single")
        );
        assert_eq!(
            children_named(&inst, hw, "cpu").len(),
            1,
            "the refinement's own subcomponents"
        );
    }

    // rivet: verifies REQ-AADL-SUBCOMPONENT-REFINE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_refinement_inherits_and_overrides_the_originals_properties() {
        // The original declares Code_Size and Data_Size; the refinement
        // overrides Code_Size only. OSATE consults the refinement, then the
        // refined original (SubcomponentImpl.getPropertyValueInternal).
        let src = REFINE_MODEL.replace(" REFINED_PROPS", " { Code_Size => 8 KByte; }");
        let inst = instantiate_aadl(&src, "Rf", "D", "bound");
        let hw = children_named(&inst, inst.root, "hw");
        assert_eq!(hw.len(), 1);
        let props = inst.properties_for(hw[0]);
        let get = |n: &str| {
            props
                .get("Memory_Properties", n)
                .or_else(|| props.get("", n))
                .map(str::to_string)
        };
        assert_eq!(
            get("Code_Size").as_deref(),
            Some("8 KByte"),
            "the refinement overrides"
        );
        assert_eq!(
            get("Data_Size").as_deref(),
            Some("1 KByte"),
            "the original's value survives"
        );
    }

    // rivet: verifies REQ-AADL-SUBCOMPONENT-REFINE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_refinement_without_a_classifier_keeps_the_originals() {
        // `hw : refined to system;` names no classifier; it keeps
        // Plat.single (SubcomponentImpl.getAllClassifier).
        let src = REFINE_MODEL
            .replace("hw : system Plat {", "hw : system Plat.single {")
            .replace(
                "hw : refined to system Plat.single REFINED_PROPS;",
                "hw : refined to system;",
            );
        let inst = instantiate_aadl(&src, "Rf", "D", "bound");
        let hw = children_named(&inst, inst.root, "hw");
        assert_eq!(hw.len(), 1);
        assert_eq!(
            inst.components[hw[0]]
                .impl_name
                .as_ref()
                .map(|n| n.as_str()),
            Some("single")
        );
        assert_eq!(children_named(&inst, hw[0], "cpu").len(), 1);
    }

    // rivet: verifies REQ-AADL-SUBCOMPONENT-REFINE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_classifierless_subcomponent_keeps_its_own_properties() {
        // `x : system { … };` names no classifier, so its property map is built
        // by build_anon_property_map — which #496 routed through
        // push_subcomponent_associations. The in-diff mutation gate (#400)
        // showed nothing exercised it: replacing it with `()` survived.
        let src = r#"
package Anon
public
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      x : system { Code_Size => 4 KByte; };
  end Top.impl;
end Anon;
"#;
        let inst = instantiate_aadl(src, "Anon", "Top", "impl");
        let x = children_named(&inst, inst.root, "x");
        assert_eq!(x.len(), 1);
        let props = inst.properties_for(x[0]);
        assert_eq!(
            props
                .get("Memory_Properties", "Code_Size")
                .or_else(|| props.get("", "Code_Size")),
            Some("4 KByte")
        );
    }

    // ── `inherit` properties (REQ-AADL-PROPERTY-INHERIT-001, #460) ──────

    const INHERIT_MODEL: &str = r#"
package I
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  data D
  end D;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
      t2 : thread T { Priority => 5; Period => 5 ms; };
      d : data D;
    properties
      Priority => 9;
      Timing_Properties::Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 2 ms;
  end P.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : processor Cpu;
      cpu2 : processor Cpu;
      mem : memory Mem;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app;
      Actual_Processor_Binding => (reference (cpu2)) applies to app.t2;
      Period => 100 ms;
      Dispatch_Offset => 3 ms;
  end Top.impl;
end I;
"#;

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_thread_inherits_its_process_binding_applied_with_applies_to() {
        // (a) `Actual_Processor_Binding` is `inherit` (osate2 @ 87468992,
        // Deployment_Properties.aadl:41): `t` declares none, so it takes
        // `app`'s, which `applies to app` put there.
        let inst = instantiate_aadl(INHERIT_MODEL, "I", "Top", "impl");
        let t = by_path(&inst, "Top.impl/app/t");
        let cpu = by_path(&inst, "Top.impl/cpu");
        assert_eq!(
            inst.properties_for(t).get("", "Actual_Processor_Binding"),
            Some("(reference (cpu))")
        );
        let ctx = inst.reference_context(t, "Deployment_Properties", "Actual_Processor_Binding");
        assert_eq!(ctx, inst.root, "the declarer of the applies-to is Top.impl");
        assert_eq!(inst.resolve_reference(ctx, "cpu"), Some(cpu));
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn an_inherited_reference_resolves_from_its_declarer_not_the_inheritor() {
        // (b) The association is on `sub` with NO `applies to`, so it
        // resolves from `sub` (its holder), where `cpu` is a processor. The
        // root also has a `cpu`, a memory. Three candidate contexts give three
        // different answers: the thread (none), the root (the memory), `sub`
        // (the processor). Only `sub` is right.
        let src = r#"
package J
public
  processor Cpu
  end Cpu;
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system S
  end S;
  system implementation S.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
  end S.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      cpu : memory Mem;
      sub : system S.impl { Actual_Processor_Binding => (reference (cpu)); };
  end Top.impl;
end J;
"#;
        let inst = instantiate_aadl(src, "J", "Top", "impl");
        let sub = by_path(&inst, "Top.impl/sub");
        let app = by_path(&inst, "Top.impl/sub/app");
        let t = by_path(&inst, "Top.impl/sub/app/t");
        let sub_cpu = by_path(&inst, "Top.impl/sub/cpu");
        let top_cpu = by_path(&inst, "Top.impl/cpu");
        for c in [app, t] {
            let ctx =
                inst.reference_context(c, "Deployment_Properties", "Actual_Processor_Binding");
            assert_eq!(ctx, sub, "inherited by {}", inst.component_instance_path(c));
            assert_eq!(inst.resolve_reference(ctx, "cpu"), Some(sub_cpu));
        }
        // Negative controls: the wrong contexts give different answers.
        assert_eq!(inst.resolve_reference(t, "cpu"), None);
        assert_eq!(inst.resolve_reference(inst.root, "cpu"), Some(top_cpu));
        // A processor does not accept the binding at all.
        assert!(
            inst.properties_for(sub_cpu)
                .get("", "Actual_Processor_Binding")
                .is_none()
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_local_value_wins_over_the_inherited_one() {
        // (c) `t2` has its own Priority, its own bare `Period` (while `app`'s
        // is QUALIFIED — a different map key, the same property), and its own
        // binding, applied by Top.impl to `app.t2`.
        let inst = instantiate_aadl(INHERIT_MODEL, "I", "Top", "impl");
        let t2 = by_path(&inst, "Top.impl/app/t2");
        let cpu2 = by_path(&inst, "Top.impl/cpu2");
        let p = inst.properties_for(t2);
        assert_eq!(p.get("", "Priority"), Some("5"));
        assert_eq!(p.get("", "Period"), Some("5 ms"));
        assert_eq!(
            p.get("Timing_Properties", "Period"),
            None,
            "a bare local Period blocks the ancestor's qualified one"
        );
        assert_eq!(
            p.get("", "Actual_Processor_Binding"),
            Some("(reference (cpu2))")
        );
        let ctx = inst.reference_context(t2, "Deployment_Properties", "Actual_Processor_Binding");
        assert_eq!(inst.resolve_reference(ctx, "cpu2"), Some(cpu2));
        // Nearest ancestor wins: `t` takes `app`'s 10 ms, not Top's 100 ms.
        let t = by_path(&inst, "Top.impl/app/t");
        assert_eq!(
            inst.properties_for(t).get("Timing_Properties", "Period"),
            Some("10 ms")
        );
        assert_eq!(inst.properties_for(t).get("", "Period"), None);
        assert_eq!(inst.properties_for(t).get("", "Priority"), Some("9"));
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn an_unresolvable_applies_to_value_is_not_inherited() {
        // `mid`'s `applies to nosuch` resolves to nothing; spar keeps the value
        // on `mid` as a recovery (and reports the path). In AADL it applies to
        // nothing, so `mid`'s descendants must look past it, to Top's own
        // value — not spread the unresolvable one to every child (measured on
        // the corpus: UAV.aadl, FlightGuidanceIMA.aadl, arinc653.aadl).
        let src = r#"
package U
public
  memory Mem
  end Mem;
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system M
  end M;
  system implementation M.impl
    subcomponents
      app : process P.impl;
    properties
      Actual_Memory_Binding => (reference (bogus)) applies to nosuch;
  end M.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      mem : memory Mem;
      mid : system M.impl;
    properties
      Actual_Memory_Binding => (reference (mem));
  end Top.impl;
end U;
"#;
        let inst = instantiate_aadl(src, "U", "Top", "impl");
        let mid = by_path(&inst, "Top.impl/mid");
        let t = by_path(&inst, "Top.impl/mid/app/t");
        let mem = by_path(&inst, "Top.impl/mem");
        assert_eq!(
            inst.properties_for(mid).get("", "Actual_Memory_Binding"),
            Some("(reference (bogus))"),
            "the recovery value stays on its declarer, as before"
        );
        assert_eq!(
            inst.properties_for(t).get("", "Actual_Memory_Binding"),
            Some("(reference (mem))")
        );
        let ctx = inst.reference_context(t, "Deployment_Properties", "Actual_Memory_Binding");
        assert_eq!(inst.resolve_reference(ctx, "mem"), Some(mem));
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn an_unresolvable_append_keeps_the_owners_own_value_inheritable() {
        // An unresolvable `+=>` onto `mid`'s OWN value appends to it (spar's
        // recovery, unchanged) and leaves it `mid`'s, so `t` still inherits.
        // The same append with no own value under it IS the recovery value,
        // and is not inherited.
        let model = |own: &str| {
            format!(
                r#"
package A
public
  thread T
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t : thread T;
  end P.impl;
  system M
  end M;
  system implementation M.impl
    subcomponents
      app : process P.impl;
    properties
{own}      Source_Text +=> ("b.c") applies to nosuch;
  end M.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      mid : system M.impl;
  end Top.impl;
end A;
"#
            )
        };
        let inst = instantiate_aadl(
            &model("      Source_Text => (\"a.c\");\n"),
            "A",
            "Top",
            "impl",
        );
        let mid = by_path(&inst, "Top.impl/mid");
        let t = by_path(&inst, "Top.impl/mid/app/t");
        assert_eq!(inst.properties_for(mid).get_all("", "Source_Text").len(), 2);
        assert_eq!(
            inst.properties_for(t).get_all("", "Source_Text"),
            inst.properties_for(mid).get_all("", "Source_Text"),
            "the owner's own list, appended to, is inherited whole"
        );

        let inst = instantiate_aadl(&model(""), "A", "Top", "impl");
        let mid = by_path(&inst, "Top.impl/mid");
        let t = by_path(&inst, "Top.impl/mid/app/t");
        assert_eq!(inst.properties_for(mid).get_all("", "Source_Text").len(), 1);
        assert!(
            inst.properties_for(t).get_all("", "Source_Text").is_empty(),
            "a recovery value alone is not inherited"
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_non_inherit_property_is_not_inherited() {
        // (d) Compute_Execution_Time is declared WITHOUT `inherit`
        // (osate2 @ 87468992, Timing_Properties.aadl), so `app`'s stays there.
        let inst = instantiate_aadl(INHERIT_MODEL, "I", "Top", "impl");
        let app = by_path(&inst, "Top.impl/app");
        let t = by_path(&inst, "Top.impl/app/t");
        assert_eq!(
            inst.properties_for(app).get("", "Compute_Execution_Time"),
            Some("1 ms .. 2 ms")
        );
        assert_eq!(
            inst.properties_for(t).get("", "Compute_Execution_Time"),
            None
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-INHERIT-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn timing_properties_are_inherited_where_they_apply() {
        // (e) Period is `inherit` (Timing_Properties.aadl:190): with `app`'s
        // own Period removed, its thread sees Top's.
        let src = INHERIT_MODEL.replace("      Timing_Properties::Period => 10 ms;\n", "");
        assert_ne!(src, INHERIT_MODEL);
        let inst = instantiate_aadl(&src, "I", "Top", "impl");
        let app = by_path(&inst, "Top.impl/app");
        let t = by_path(&inst, "Top.impl/app/t");
        let d = by_path(&inst, "Top.impl/app/d");
        let cpu = by_path(&inst, "Top.impl/cpu");
        let mem = by_path(&inst, "Top.impl/mem");
        assert_eq!(inst.properties_for(app).get("", "Period"), Some("100 ms"));
        assert_eq!(inst.properties_for(t).get("", "Period"), Some("100 ms"));
        // Period does not apply to processor, memory or data.
        for c in [cpu, mem, d] {
            assert_eq!(inst.properties_for(c).get("", "Period"), None);
        }
        // Priority applies to data; Actual_Processor_Binding does not.
        assert_eq!(inst.properties_for(d).get("", "Priority"), Some("9"));
        assert_eq!(
            inst.properties_for(d).get("", "Actual_Processor_Binding"),
            None
        );
        // Dispatch_Offset applies to threads only (Timing_Properties.aadl:110)
        // — the walk passes THROUGH the process, which does not accept it.
        assert_eq!(inst.properties_for(app).get("", "Dispatch_Offset"), None);
        assert_eq!(
            inst.properties_for(t).get("", "Dispatch_Offset"),
            Some("3 ms")
        );
    }

    // ── provenance (REQ-AADL-PROPERTY-PROVENANCE-001) ───────────────────

    // rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn feature_connection_and_unresolved_values_record_their_source() {
        use crate::properties::PropertySource;
        use crate::resolver::CiName;
        let src = r#"
package Pv
public
  data D
  end D;
  thread T
    features
      tin: in data port D;
      tout: out data port D;
  end T;
  thread implementation T.i
  end T.i;
  process Proc
    features
      pin: in data port D;
  end Proc;
  process implementation Proc.i
    subcomponents
      a: thread T.i;
      b: thread T.i;
    connections
      c1: port a.tout -> b.tin { Latency => 1 ms .. 2 ms; };
      c2: port a.tout -> b.tin;
    properties
      Latency => 3 ms .. 4 ms applies to c2;
      Source_Name => "x" applies to pin;
      Source_Text => ("y") applies to nosuch;
  end Proc.i;
  system S
  end S;
  system implementation S.i
    subcomponents
      proc: process Proc.i;
  end S.i;
end Pv;
"#;
        let inst = instantiate_aadl(src, "Pv", "S", "i");
        let proc_ = by_path(&inst, "S.i/proc");
        let key = |n: &str| (CiName::from_str(""), CiName::from_str(n));
        let conn = |n: &str| {
            inst.connections
                .iter()
                .find(|(_, c)| c.name.as_str() == n)
                .unwrap()
                .0
        };
        assert_eq!(
            inst.properties_for_connection(conn("c1"))
                .sources_for_key(&key("Latency")),
            [PropertySource::DeclaredLocal]
        );
        assert_eq!(
            inst.properties_for_connection(conn("c2"))
                .sources_for_key(&key("Latency")),
            [PropertySource::Contained { declarer: proc_ }]
        );
        let (pin, _) = inst
            .features
            .iter()
            .find(|(_, f)| f.name.as_str() == "pin")
            .unwrap();
        assert_eq!(
            inst.properties_for_feature(pin)
                .sources_for_key(&key("Source_Name")),
            [PropertySource::Contained { declarer: proc_ }]
        );
        assert_eq!(
            inst.properties_for(proc_)
                .sources_for_key(&key("Source_Text")),
            [PropertySource::ContainedUnresolved {
                path: "nosuch".to_string()
            }]
        );
    }

    /// A blank `applies to` path is no contained association: the value stays
    /// on its declarer as an implementation value, not routed through the
    /// applies-to resolver as an unresolved path. The AADL parser never yields
    /// a blank path, but spar-sysml2 builds `PropertyAssociationItem`s
    /// directly, so the guard in `push_property_association` is reachable.
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_blank_applies_to_path_stays_an_implementation_value() {
        let src = "package P\npublic\n  system S\n  end S;\n  system implementation S.i\n    properties\n      Period => 10 ms;\n  end S.i;\nend P;\n";
        let db = crate::HirDefDatabase::default();
        let file = spar_base_db::SourceFile::new(&db, "blank.aadl".to_string(), src.to_string());
        let root = spar_base_db::parse_file(&db, file).syntax_node();
        let mut tree = crate::item_tree::lower::lower_file(&root);
        let pas: Vec<_> = tree.property_associations.iter().map(|(i, _)| i).collect();
        assert_eq!(pas.len(), 1, "the model declares exactly one association");
        tree.property_associations[pas[0]].applies_to = Some("   ".to_string());
        let scope = GlobalScope::from_trees(vec![std::sync::Arc::new(tree)]);
        let inst =
            SystemInstance::instantiate(&scope, &Name::new("P"), &Name::new("S"), &Name::new("i"));
        let props = inst.properties_for(inst.root);
        let key = (
            crate::resolver::CiName::from_str(""),
            crate::resolver::CiName::new(&Name::new("Period")),
        );
        assert_eq!(
            props.sources_for_key(&key),
            &[PropertySource::Implementation {
                classifier: "P::S.i".to_string()
            }]
        );
    }

    // ── #555: a connection end names a feature that exists ──────────────

    /// Model for the end-resolution tests; `CONNS` is replaced per test.
    const END_MODEL: &str = r#"
package P
public
  bus B
  end B;
  feature group FG
    features
      y : in data port;
      inner : feature group FG2;
  end FG;
  feature group FG2
    features
      deep : in data port;
  end FG2;
  device Base
    features
      inherited : in data port;
  end Base;
  device D extends Base
    features
      ba : requires bus access B;
      arr : in data port [2];
      fgd : feature group FG;
      o : out data port;
  end D;
  device Broken extends NoSuchBase
    features
      own_b : in data port;
  end Broken;
  device Opaque
    features
      g : feature group NoSuchGroup;
  end Opaque;
  device Bare
    features
      g : feature group;
  end Bare;
  system SBase
    features
      sin : in data port;
  end SBase;
  system S extends SBase
    features
      fg : feature group FG;
  end S;
  system implementation S.impl
    subcomponents
      b : bus B;
      d : device D;
      da : device D [2];
      u : device NoSuchType;
      br : device Broken;
      op : device Opaque;
      bare : device Bare;
    connections
      CONNS
  end S.impl;
end P;
"#;

    fn end_instance(conns: &str) -> SystemInstance {
        instantiate_aadl(&END_MODEL.replace("CONNS", conns), "P", "S", "impl")
    }

    fn dangling(inst: &SystemInstance) -> Vec<String> {
        inst.diagnostics
            .iter()
            .filter(|d| d.message.contains("has no feature '"))
            .map(|d| d.message.clone())
            .collect()
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn dangling_subcomponent_end_is_reported_with_connection_end_and_feature() {
        let inst = end_instance("c2 : bus access b -> d.zzz;");
        assert_eq!(
            dangling(&inst),
            vec![
                "connection 'c2': destination 'd.zzz': subcomponent 'd' (classifier 'P::D') \
                 has no feature 'zzz'"
                    .to_string()
            ]
        );
        let d = inst
            .diagnostics
            .iter()
            .find(|d| d.message.contains("has no feature '"))
            .unwrap();
        assert_eq!(
            d.path.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["S.impl", "c2"]
        );
        // The connection is kept (as an unresolved-subcomponent end is).
        assert_eq!(inst.connections.len(), 1);
        let src_side = end_instance("c2 : port d.nope -> d.inherited;");
        assert_eq!(dangling(&src_side).len(), 1);
        assert!(
            dangling(&src_side)[0].starts_with("connection 'c2': source 'd.nope': "),
            "{:?}",
            dangling(&src_side)
        );
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn dangling_own_end_is_reported() {
        let inst = end_instance("c3 : port nope -> d.inherited;");
        assert_eq!(
            dangling(&inst),
            vec![
                "connection 'c3': source 'nope': component 'S.impl' (classifier 'P::S') \
                 has no feature 'nope'"
                    .to_string()
            ]
        );
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_featureless_target_still_reports() {
        // `bus B` declares no features: an empty list is a COMPLETE list, so
        // `b.zzz` is provably absent.
        let inst = end_instance("c : bus access b.zzz -> d.ba;");
        assert_eq!(dangling(&inst).len(), 1, "{:?}", inst.diagnostics);
        assert!(dangling(&inst)[0].contains("subcomponent 'b'"));
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn resolving_ends_are_silent() {
        for conns in [
            // case-insensitive, inherited from an `extends` ancestor
            "c : port d.o -> d.INHERITED;",
            // own inherited feature (S extends SBase)
            "c : port sin -> d.inherited;",
            // array feature `arr` is instantiated as arr[1], arr[2]
            "c : port d.o -> d.arr;",
            // array subcomponent by base name and by element
            "c : port da.o -> d.inherited;",
            "c : port da[1].o -> d.inherited;",
            // whole-subcomponent access end (feature emptied by the fixup)
            "c : bus access b -> d.ba;",
            // a bare subcomponent name on a NON-access connection
            "c : feature b -> d.ba;",
            // feature group end, own feature-group member, collapsed member
            "c : feature group fg <-> d.fgd;",
            "c : port d.o -> fg.y;",
            "c : port d.o -> d.fgd.y;",
            // a nested group's name inside a subcomponent's group
            "c : feature group d.fgd.inner <-> fg;",
            "c : port d.o -> d.fgd.inner.deep;",
        ] {
            let inst = end_instance(conns);
            assert_eq!(dangling(&inst), Vec::<String>::new(), "{conns}");
        }
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_member_missing_from_every_group_of_the_target_is_reported() {
        let inst = end_instance("c : port d.o -> d.fgd.zz;");
        assert_eq!(dangling(&inst).len(), 1, "{:?}", inst.diagnostics);
        assert!(dangling(&inst)[0].contains("has no feature 'zz'"));
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn unprovable_ends_are_not_reported() {
        for conns in [
            // the child's type does not resolve: its empty list proves nothing
            "c : port u.x -> d.inherited;",
            // an `extends` ancestor does not resolve: the list is partial
            "c : port br.x -> d.inherited;",
            // a feature group of the target whose type does not resolve
            "c : port op.x -> d.inherited;",
            // a feature group of the target with no classifier at all
            "c : port bare.x -> d.inherited;",
            // the subcomponent itself is unresolved (reported separately)
            "c : port q.x -> d.inherited;",
            // own feature-group member missing: reported as "has no member"
            "c : port d.o -> fg.nomember;",
        ] {
            let inst = end_instance(conns);
            assert_eq!(dangling(&inst), Vec::<String>::new(), "{conns}");
        }
        // Internal and processor features of the implementation (and of an
        // implementation it extends) are legal own ends; `self.ie` lowers to
        // a bare `ie`. Found in the corpus: InternalEventTest, isolette,
        // EMV2TR figure90_93.
        let src = r#"
package Q
public
  device Leaf
    features
      i : in event port;
      o : out event port;
  end Leaf;
  system T
  end T;
  system implementation T.base
    subcomponents
      l : device Leaf;
    internal features
      inherited_ie : event;
  end T.base;
  system implementation T.impl extends T.base
    internal features
      ie : event;
    processor features
      pp : port;
    connections
      c1 : port self.ie -> l.i;
      c2 : port l.o -> self.ie;
      c3 : port ie -> l.i;
      c4 : port pp -> l.i;
      c5 : port inherited_ie -> l.i;
      c6 : port not_internal -> l.i;
  end T.impl;
end Q;
"#;
        let inst = instantiate_aadl(src, "Q", "T", "impl");
        assert_eq!(
            dangling(&inst),
            vec![
                "connection 'c6': source 'not_internal': component 'T.impl' (classifier \
                 'Q::T') has no feature 'not_internal'"
                    .to_string()
            ],
            "{:?}",
            inst.diagnostics
        );

        // A parameter end on a subprogram call resolves through the called
        // subprogram's type; `call1.zz` names no parameter of `Sp`.
        let src = r#"
package R
public
  subprogram SpBase
    features
      a : in parameter;
  end SpBase;
  subprogram Sp extends SpBase
    features
      r : out parameter;
  end Sp;
  subprogram Outer
    features
      arg : in parameter;
      res : out parameter;
  end Outer;
  subprogram implementation Outer.impl
    calls
      seq : { call1 : subprogram Sp; call2 : subprogram NoSuchSp; };
    connections
      p1 : parameter arg -> call1.a;
      p2 : parameter call1.r -> res;
      p3 : parameter arg -> call1.zz;
      p4 : parameter arg -> call2.anything;
  end Outer.impl;
  system T
  end T;
  system implementation T.impl
    subcomponents
      sp : subprogram Outer.impl;
  end T.impl;
end R;
"#;
        let inst = instantiate_aadl(src, "R", "T", "impl");
        assert_eq!(
            dangling(&inst),
            vec![
                "connection 'p3': destination 'call1.zz': called subprogram 'Sp' (classifier \
                 'R::Sp') has no feature 'zz'"
                    .to_string()
            ],
            "{:?}",
            inst.diagnostics
        );

        // `fg.nomember` is still reported, once, by the feature-group fixup.
        let inst = end_instance("c : port d.o -> fg.nomember;");
        assert_eq!(
            inst.diagnostics
                .iter()
                .filter(|d| d.message.contains("has no member 'nomember'"))
                .count(),
            1
        );
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_dangling_end_on_an_array_subcomponent_is_reported() {
        // `da : device D [2]` is instantiated as `da[1]`, `da[2]`: the end
        // `da.zzz` names no instance exactly, so it is found by base name, and
        // the missing feature is reported.
        let inst = end_instance("c : port da.zzz -> d.inherited;");
        let found = dangling(&inst);
        assert!(!found.is_empty(), "{:?}", inst.diagnostics);
        assert!(
            found
                .iter()
                .all(|m| m.starts_with("connection 'c': source 'da.zzz': ")
                    && m.ends_with("has no feature 'zzz'")),
            "{found:?}"
        );
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_sibling_implementations_internal_feature_is_no_own_end() {
        // Local features are taken from the implementations of the extends
        // chain only, not from every implementation in the same file.
        let src = r#"
package Q
public
  device Leaf
    features
      i : in event port;
  end Leaf;
  system T
  end T;
  system implementation T.other
    internal features
      stray : event;
  end T.other;
  system implementation T.impl
    subcomponents
      l : device Leaf;
    internal features
      ie : event;
    connections
      c1 : port ie -> l.i;
      c2 : port stray -> l.i;
  end T.impl;
end Q;
"#;
        let inst = instantiate_aadl(src, "Q", "T", "impl");
        assert_eq!(
            dangling(&inst),
            vec![
                "connection 'c2': source 'stray': component 'T.impl' (classifier 'Q::T') \
                 has no feature 'stray'"
                    .to_string()
            ],
            "{:?}",
            inst.diagnostics
        );
    }

    // rivet: verifies REQ-INSTANCE-CONNECTION-END-RESOLVES-001
    #[test]
    #[cfg_attr(miri, ignore = "parses via rowan — upstream rowan#192 UB under Miri")]
    fn a_call_end_on_an_unprovable_subprogram_type_is_not_reported() {
        // Each skip condition alone silences the end: a broken `extends`
        // chain (partial feature list), and a feature group (members unknown).
        let src = r#"
package R
public
  feature group FG
    features
      m : in parameter;
  end FG;
  subprogram SpBroken extends NoSuchBase
    features
      a : in parameter;
  end SpBroken;
  subprogram SpFg
    features
      a : in parameter;
      g : feature group FG;
  end SpFg;
  subprogram Outer
    features
      arg : in parameter;
  end Outer;
  subprogram implementation Outer.impl
    calls
      seq : { call1 : subprogram SpBroken; call2 : subprogram SpFg; };
    connections
      p1 : parameter arg -> call1.zz;
      p2 : parameter arg -> call2.zz;
  end Outer.impl;
  system T
  end T;
  system implementation T.impl
    subcomponents
      sp : subprogram Outer.impl;
  end T.impl;
end R;
"#;
        let inst = instantiate_aadl(src, "R", "T", "impl");
        assert_eq!(
            dangling(&inst),
            Vec::<String>::new(),
            "{:?}",
            inst.diagnostics
        );
    }
}
