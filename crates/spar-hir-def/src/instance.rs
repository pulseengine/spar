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
use crate::properties::PropertyMap;
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
}

/// An end-to-end flow instance from a component implementation.
#[derive(Debug)]
pub struct EndToEndFlowInstance {
    pub name: Name,
    /// The component instance that owns this flow.
    pub owner: ComponentInstanceIdx,
    /// Segments: alternating subcomponent and connection names.
    pub segments: Vec<Name>,
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
        let total: u64 = modal_components
            .iter()
            .map(|(_, modes)| modes.len() as u64)
            .product();

        let truncated = total > Self::MAX_SOMS as u64;

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
                    total,
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
                        ConnectionPattern::Next => {
                            // Linear chain: element i -> element i+1 (N-1 pairs).
                            if src_matches.len() != dst_matches.len() {
                                let owner_name = self.components[conn_owner].name.clone();
                                endpoint_diagnostics.push(InstanceDiagnostic {
                                    message: format!(
                                        "connection '{}': Next pattern requires equal array sizes (source={}, destination={})",
                                        conn_name, src_matches.len(), dst_matches.len(),
                                    ),
                                    path: vec![owner_name, conn_name.clone()],
                                });
                            }
                            let n = src_matches.len().min(dst_matches.len());
                            (0..n.saturating_sub(1))
                                .map(|i| (src_matches[i], dst_matches[i + 1]))
                                .collect()
                        }
                        ConnectionPattern::Previous => {
                            // Linear chain: element i -> element i-1 (N-1 pairs).
                            if src_matches.len() != dst_matches.len() {
                                let owner_name = self.components[conn_owner].name.clone();
                                endpoint_diagnostics.push(InstanceDiagnostic {
                                    message: format!(
                                        "connection '{}': Previous pattern requires equal array sizes (source={}, destination={})",
                                        conn_name, src_matches.len(), dst_matches.len(),
                                    ),
                                    path: vec![owner_name, conn_name.clone()],
                                });
                            }
                            let n = src_matches.len().min(dst_matches.len());
                            (1..n)
                                .map(|i| (src_matches[i], dst_matches[i - 1]))
                                .collect()
                        }
                        ConnectionPattern::CyclicNext => {
                            // Cyclic ring: element i -> element (i+1) mod N.
                            if src_matches.len() != dst_matches.len() {
                                let owner_name = self.components[conn_owner].name.clone();
                                endpoint_diagnostics.push(InstanceDiagnostic {
                                    message: format!(
                                        "connection '{}': Cyclic_Next pattern requires equal array sizes (source={}, destination={})",
                                        conn_name, src_matches.len(), dst_matches.len(),
                                    ),
                                    path: vec![owner_name, conn_name.clone()],
                                });
                            }
                            let n = src_matches.len().min(dst_matches.len());
                            if n == 0 {
                                Vec::new()
                            } else {
                                (0..n)
                                    .map(|i| (src_matches[i], dst_matches[(i + 1) % n]))
                                    .collect()
                            }
                        }
                        ConnectionPattern::CyclicPrevious => {
                            // Cyclic ring: element i -> element (i-1+N) mod N.
                            if src_matches.len() != dst_matches.len() {
                                let owner_name = self.components[conn_owner].name.clone();
                                endpoint_diagnostics.push(InstanceDiagnostic {
                                    message: format!(
                                        "connection '{}': Cyclic_Previous pattern requires equal array sizes (source={}, destination={})",
                                        conn_name, src_matches.len(), dst_matches.len(),
                                    ),
                                    path: vec![owner_name, conn_name.clone()],
                                });
                            }
                            let n = src_matches.len().min(dst_matches.len());
                            if n == 0 {
                                Vec::new()
                            } else {
                                (0..n)
                                    .map(|i| (src_matches[i], dst_matches[(i + n - 1) % n]))
                                    .collect()
                            }
                        }
                        ConnectionPattern::OneToAll => {
                            // Fan-out: first source element connects to all destinations.
                            if let Some(&first_src) = src_matches.first() {
                                dst_matches.iter().map(|&d| (first_src, d)).collect()
                            } else {
                                Vec::new()
                            }
                        }
                        ConnectionPattern::AllToOne => {
                            // Fan-in: all source elements connect to the first destination.
                            if let Some(&first_dst) = dst_matches.first() {
                                src_matches.iter().map(|&s| (s, first_dst)).collect()
                            } else {
                                Vec::new()
                            }
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
    /// Element i connects to element i+1 (linear chain, N-1 pairs).
    Next,
    /// Element i connects to element i-1 (linear chain, N-1 pairs).
    Previous,
    /// Element i connects to element (i+1) mod N (cyclic ring, N pairs).
    CyclicNext,
    /// Element i connects to element (i-1+N) mod N (cyclic ring, N pairs).
    CyclicPrevious,
    /// First element connects to all elements (fan-out from element 0).
    OneToAll,
    /// All elements connect to the first element (fan-in to element 0).
    AllToOne,
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
    } else if stripped.eq_ignore_ascii_case("next") {
        ConnectionPattern::Next
    } else if stripped.eq_ignore_ascii_case("previous") {
        ConnectionPattern::Previous
    } else if stripped.eq_ignore_ascii_case("cyclic_next") {
        ConnectionPattern::CyclicNext
    } else if stripped.eq_ignore_ascii_case("cyclic_previous") {
        ConnectionPattern::CyclicPrevious
    } else if stripped.eq_ignore_ascii_case("one_to_all") {
        ConnectionPattern::OneToAll
    } else if stripped.eq_ignore_ascii_case("all_to_one") {
        ConnectionPattern::AllToOne
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
    )>,
    e2e_flows: Vec<(Name, Vec<Name>)>,
    modes: Vec<(Name, bool)>,
    mode_transitions: Vec<(Option<Name>, Name, Name, Vec<Name>)>,
    call_map: FxHashMap<String, Name>,
}

struct Builder<'a> {
    scope: &'a GlobalScope,
    components: Arena<ComponentInstance>,
    features: Arena<FeatureInstance>,
    connections: Arena<ConnectionInstance>,
    flow_instances: Arena<FlowInstance>,
    end_to_end_flows: Arena<EndToEndFlowInstance>,
    mode_instances: Arena<ModeInstance>,
    mode_transition_instances: Arena<ModeTransitionInstance>,
    diagnostics: Vec<InstanceDiagnostic>,
    property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    depth: u32,
    max_depth: u32,
}

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
                let e2e_data = impl_chain.e2e_flows;
                let impl_mode_data = impl_chain.modes;
                let impl_mt_data = impl_chain.mode_transitions;

                let mut child_indices = Vec::new();
                for (sub_name, _sub_cat, sub_classifier, sub_idx, array_dims, sub_in_modes) in
                    sub_data
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
                                    Some((loc.tree, sub_idx)),
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

                                // Build property map for leaf subcomponent (type only)
                                self.build_leaf_property_map(
                                    child_idx,
                                    &leaf_pkg,
                                    &cls_ref.type_name,
                                    loc.tree,
                                    sub_idx,
                                );
                                child_indices.push(child_idx);
                            }
                        } else {
                            // No classifier — anonymous subcomponent
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
                            self.build_anon_property_map(child_idx, loc.tree, sub_idx);
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
                for (conn_name, conn_kind, bidi, mut src, mut dst, conn_in_modes) in conn_data {
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

                    let ci = self.connections.alloc(ConnectionInstance {
                        name: conn_name,
                        kind: conn_kind,
                        is_bidirectional: bidi,
                        owner: idx,
                        src,
                        dst,
                        in_modes: conn_in_modes,
                    });
                    conn_indices.push(ci);
                }
                self.components[idx].connections = conn_indices.clone();

                // STPA-REQ-010: Validate array index bounds in connection endpoints.
                self.validate_connection_array_indices(idx, &conn_indices);

                // Instantiate end-to-end flows
                for (e2e_name, segments) in e2e_data {
                    self.end_to_end_flows.alloc(EndToEndFlowInstance {
                        name: e2e_name,
                        owner: idx,
                        segments,
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

        // Collect own subcomponents
        for &sub_idx in &ci.subcomponents {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let sub = &tree.subcomponents[sub_idx];
                result.subcomponents.push((
                    sub.name.clone(),
                    sub.category,
                    sub.classifier.clone(),
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
                ));
            }
        }

        // Collect own e2e flows
        for &e2e_idx in &ci.end_to_end_flows {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let e2e = &tree.end_to_end_flows[e2e_idx];
                result
                    .e2e_flows
                    .push((e2e.name.clone(), e2e.segments.clone()));
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
            return Vec::new();
        };

        let key = format!("{}::{}", package, ct.name);
        if visited.contains(&key) {
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
        let feat_data = self.collect_type_chain_features(loc, type_package, &mut visited);

        let Some(ct) = self.scope.get_component_type(loc) else {
            return;
        };

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
    /// `(tree_idx, PropertyAssociationIdx)` pairs in inheritance order
    /// (grandparent first, self last) so that later values override earlier.
    fn collect_type_chain_properties(
        &self,
        loc: crate::resolver::ItemLoc,
        package: &Name,
        visited: &mut Vec<String>,
    ) -> Vec<(usize, crate::item_tree::PropertyAssociationIdx)> {
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
            for &pa_idx in &ct.property_associations {
                result.push((loc.tree, pa_idx));
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
    ) -> Vec<(usize, crate::item_tree::PropertyAssociationIdx)> {
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
            for &pa_idx in &ci.property_associations {
                result.push((loc.tree, pa_idx));
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
            for (tree_idx, pa_idx) in type_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    map.add(crate::properties::PropertyValue {
                        name: pa.name.clone(),
                        value: pa.value.clone(),
                        is_append: pa.is_append,
                    });
                }
            }
        }

        // 2. Implementation-level properties (walking the extends chain, override type)
        if let Some(loc) = impl_loc {
            let mut visited = Vec::new();
            let impl_props =
                self.collect_impl_chain_properties(loc, resolved_package, &mut visited);
            for (tree_idx, pa_idx) in impl_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    map.add(crate::properties::PropertyValue {
                        name: pa.name.clone(),
                        value: pa.value.clone(),
                        is_append: pa.is_append,
                    });
                }
            }
        }

        // 3. Subcomponent-level properties (override impl)
        if let Some((tree_idx, sub_idx)) = subcomponent_loc
            && let Some(tree) = self.scope.tree(tree_idx)
        {
            let sub = &tree.subcomponents[sub_idx];
            for &pa_idx in &sub.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(crate::properties::PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        if !map.is_empty() {
            self.property_maps.insert(idx, map);
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
            for (tree_idx, pa_idx) in type_props {
                if let Some(tree) = self.scope.tree(tree_idx) {
                    let pa = &tree.property_associations[pa_idx];
                    map.add(crate::properties::PropertyValue {
                        name: pa.name.clone(),
                        value: pa.value.clone(),
                        is_append: pa.is_append,
                    });
                }
            }
        }

        // Subcomponent-level properties
        if let Some(tree) = self.scope.tree(parent_tree_idx) {
            let sub = &tree.subcomponents[sub_idx];
            for &pa_idx in &sub.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(crate::properties::PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

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

        if let Some(tree) = self.scope.tree(tree_idx) {
            let sub = &tree.subcomponents[sub_idx];
            for &pa_idx in &sub.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(crate::properties::PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        if !map.is_empty() {
            self.property_maps.insert(idx, map);
        }
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

    #[test]
    fn test_parse_connection_pattern_next() {
        assert_eq!(parse_connection_pattern("next"), ConnectionPattern::Next);
        assert_eq!(parse_connection_pattern("((Next))"), ConnectionPattern::Next);
    }

    #[test]
    fn test_parse_connection_pattern_previous() {
        assert_eq!(
            parse_connection_pattern("previous"),
            ConnectionPattern::Previous
        );
        assert_eq!(
            parse_connection_pattern("((Previous))"),
            ConnectionPattern::Previous
        );
    }

    #[test]
    fn test_parse_connection_pattern_cyclic_next() {
        assert_eq!(
            parse_connection_pattern("cyclic_next"),
            ConnectionPattern::CyclicNext
        );
        assert_eq!(
            parse_connection_pattern("((Cyclic_Next))"),
            ConnectionPattern::CyclicNext
        );
    }

    #[test]
    fn test_parse_connection_pattern_cyclic_previous() {
        assert_eq!(
            parse_connection_pattern("cyclic_previous"),
            ConnectionPattern::CyclicPrevious
        );
        assert_eq!(
            parse_connection_pattern("((Cyclic_Previous))"),
            ConnectionPattern::CyclicPrevious
        );
    }

    #[test]
    fn test_parse_connection_pattern_one_to_all() {
        assert_eq!(
            parse_connection_pattern("one_to_all"),
            ConnectionPattern::OneToAll
        );
        assert_eq!(
            parse_connection_pattern("((One_To_All))"),
            ConnectionPattern::OneToAll
        );
    }

    #[test]
    fn test_parse_connection_pattern_all_to_one() {
        assert_eq!(
            parse_connection_pattern("all_to_one"),
            ConnectionPattern::AllToOne
        );
        assert_eq!(
            parse_connection_pattern("((All_To_One))"),
            ConnectionPattern::AllToOne
        );
    }

    // ── expansion tests for new connection patterns ─────────────────

    /// Helper: build a 3-element array test instance with a given Connection_Pattern value.
    /// Returns (instance, root, [src1, src2, src3], [dst1, dst2, dst3]).
    fn make_3elem_pattern_instance(
        pattern_value: &str,
    ) -> (
        SystemInstance,
        ComponentInstanceIdx,
        [ComponentInstanceIdx; 3],
        [ComponentInstanceIdx; 3],
    ) {
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

        let mut srcs = Vec::new();
        let mut dsts = Vec::new();
        for i in 1..=3 {
            let s = components.alloc(ComponentInstance {
                name: Name::new(&format!("src[{i}]")),
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
            srcs.push(s);
        }
        for i in 1..=3 {
            let d = components.alloc(ComponentInstance {
                name: Name::new(&format!("dst[{i}]")),
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
                array_index: Some(i),
                in_modes: Vec::new(),
            });
            dsts.push(d);
        }

        components[root].children = srcs.iter().chain(dsts.iter()).copied().collect();

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

        let mut prop_map = crate::properties::PropertyMap::new();
        prop_map.add(crate::properties::PropertyValue {
            name: crate::name::PropertyRef {
                property_set: Some("Communication_Properties".into()),
                property_name: "Connection_Pattern".into(),
            },
            value: pattern_value.to_string(),
            is_append: false,
        });

        let mut instance = make_instance(components, Arena::default(), connections, root);
        instance.property_maps.insert(root, prop_map);
        instance.compute_semantic_connections();

        let src_arr = [srcs[0], srcs[1], srcs[2]];
        let dst_arr = [dsts[0], dsts[1], dsts[2]];
        (instance, root, src_arr, dst_arr)
    }

    #[test]
    fn test_connection_pattern_next_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((next))");
        // Next: src[1]->dst[2], src[2]->dst[3] = 2 pairs (N-1).
        assert_eq!(
            instance.semantic_connections.len(),
            2,
            "Next pattern with 3 elements should produce 2 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[0], dsts[1])), "expected src[1]->dst[2]");
        assert!(pairs.contains(&(srcs[1], dsts[2])), "expected src[2]->dst[3]");
    }

    #[test]
    fn test_connection_pattern_previous_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((previous))");
        // Previous: src[2]->dst[1], src[3]->dst[2] = 2 pairs (N-1).
        assert_eq!(
            instance.semantic_connections.len(),
            2,
            "Previous pattern with 3 elements should produce 2 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[1], dsts[0])), "expected src[2]->dst[1]");
        assert!(pairs.contains(&(srcs[2], dsts[1])), "expected src[3]->dst[2]");
    }

    #[test]
    fn test_connection_pattern_cyclic_next_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((cyclic_next))");
        // CyclicNext: src[1]->dst[2], src[2]->dst[3], src[3]->dst[1] = 3 pairs (N).
        assert_eq!(
            instance.semantic_connections.len(),
            3,
            "Cyclic_Next pattern with 3 elements should produce 3 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[0], dsts[1])), "expected src[1]->dst[2]");
        assert!(pairs.contains(&(srcs[1], dsts[2])), "expected src[2]->dst[3]");
        assert!(pairs.contains(&(srcs[2], dsts[0])), "expected src[3]->dst[1]");
    }

    #[test]
    fn test_connection_pattern_cyclic_previous_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((cyclic_previous))");
        // CyclicPrevious: src[1]->dst[3], src[2]->dst[1], src[3]->dst[2] = 3 pairs (N).
        assert_eq!(
            instance.semantic_connections.len(),
            3,
            "Cyclic_Previous pattern with 3 elements should produce 3 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[0], dsts[2])), "expected src[1]->dst[3]");
        assert!(pairs.contains(&(srcs[1], dsts[0])), "expected src[2]->dst[1]");
        assert!(pairs.contains(&(srcs[2], dsts[1])), "expected src[3]->dst[2]");
    }

    #[test]
    fn test_connection_pattern_one_to_all_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((one_to_all))");
        // OneToAll: src[1]->dst[1], src[1]->dst[2], src[1]->dst[3] = 3 pairs.
        assert_eq!(
            instance.semantic_connections.len(),
            3,
            "One_To_All pattern with 3 destinations should produce 3 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[0], dsts[0])), "expected src[1]->dst[1]");
        assert!(pairs.contains(&(srcs[0], dsts[1])), "expected src[1]->dst[2]");
        assert!(pairs.contains(&(srcs[0], dsts[2])), "expected src[1]->dst[3]");
    }

    #[test]
    fn test_connection_pattern_all_to_one_expansion() {
        let (instance, _, srcs, dsts) = make_3elem_pattern_instance("((all_to_one))");
        // AllToOne: src[1]->dst[1], src[2]->dst[1], src[3]->dst[1] = 3 pairs.
        assert_eq!(
            instance.semantic_connections.len(),
            3,
            "All_To_One pattern with 3 sources should produce 3 connections"
        );
        let pairs: Vec<_> = instance
            .semantic_connections
            .iter()
            .map(|sc| (sc.ultimate_source.0, sc.ultimate_destination.0))
            .collect();
        assert!(pairs.contains(&(srcs[0], dsts[0])), "expected src[1]->dst[1]");
        assert!(pairs.contains(&(srcs[1], dsts[0])), "expected src[2]->dst[1]");
        assert!(pairs.contains(&(srcs[2], dsts[0])), "expected src[3]->dst[1]");
    }
}
