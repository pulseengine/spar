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
use rustc_hash::FxHashMap;

use crate::feature_group::{expand_feature_group, ExpandedFeature};
use crate::item_tree::{ComponentCategory, ConnectionKind, Direction, FeatureKind, FlowKind};
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
}

/// A feature instance.
#[derive(Debug)]
pub struct FeatureInstance {
    pub name: Name,
    pub kind: FeatureKind,
    pub direction: Option<Direction>,
    pub owner: ComponentInstanceIdx,
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
            max_depth: 32,
        };

        let root_name = Name::new(&format!("{}.{}", root_type, root_impl));
        let root_idx =
            builder.instantiate_component(&root_name, root_package, root_type, root_impl, None, None);

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

    /// Total number of component instances.
    pub fn component_count(&self) -> usize {
        self.components.len()
    }

    /// Iterate all component instances.
    pub fn all_components(&self) -> impl Iterator<Item = (ComponentInstanceIdx, &ComponentInstance)> {
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
        static EMPTY: std::sync::LazyLock<PropertyMap> =
            std::sync::LazyLock::new(PropertyMap::new);
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
            let mut next = Vec::with_capacity(
                (soms.len() * mode_indices.len()).min(Self::MAX_SOMS + 1),
            );
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
    pub fn compute_semantic_connections(&mut self) {
        /// Maximum recursion depth to prevent infinite loops.
        const MAX_TRACE_DEPTH: usize = 20;

        let mut semantic = Vec::new();

        // Collect all connection indices so we can iterate without borrowing self.
        let all_conn_indices: Vec<ConnectionInstanceIdx> =
            self.connections.iter().map(|(idx, _)| idx).collect();

        for conn_idx in &all_conn_indices {
            let conn = &self.connections[*conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s.clone(), d.clone()),
                // Skip connections with missing endpoints.
                _ => continue,
            };
            let conn_owner = conn.owner;
            let conn_name = conn.name.clone();
            let conn_kind = conn.kind;

            match (&src.subcomponent, &dst.subcomponent) {
                // ── Across connection: sub_a.port -> sub_b.port ──
                (Some(src_sub_name), Some(dst_sub_name)) => {
                    let owner = &self.components[conn_owner];
                    let src_idx = owner.children.iter().find(|&&child_idx| {
                        self.components[child_idx].name.as_str() == src_sub_name.as_str()
                    }).copied();
                    let dst_idx = owner.children.iter().find(|&&child_idx| {
                        self.components[child_idx].name.as_str() == dst_sub_name.as_str()
                    }).copied();

                    if let (Some(src_component), Some(dst_component)) = (src_idx, dst_idx) {
                        let mut path = vec![*conn_idx];

                        // Trace source deeper: look for up connections inside
                        // the source subcomponent that feed this port.
                        let ultimate_src =
                            self.trace_source(src_component, &src.feature, &mut path, MAX_TRACE_DEPTH);

                        // Trace destination deeper: look for down connections inside
                        // the destination subcomponent that distribute from this port.
                        let ultimate_dst =
                            self.trace_destination(dst_component, &dst.feature, &mut path, MAX_TRACE_DEPTH);

                        semantic.push(SemanticConnection {
                            name: conn_name,
                            kind: conn_kind,
                            ultimate_source: ultimate_src,
                            ultimate_destination: ultimate_dst,
                            connection_path: path,
                        });
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
                        let src_idx = owner.children.iter().find(|&&child_idx| {
                            self.components[child_idx].name.as_str() == src_sub_name.as_str()
                        }).copied();

                        if let Some(src_component) = src_idx {
                            let mut path = vec![*conn_idx];
                            let ultimate_src =
                                self.trace_source(src_component, &src.feature, &mut path, MAX_TRACE_DEPTH);

                            semantic.push(SemanticConnection {
                                name: conn_name,
                                kind: conn_kind,
                                ultimate_source: ultimate_src,
                                ultimate_destination: (conn_owner, dst.feature.clone()),
                                connection_path: path,
                            });
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
                        let dst_idx = owner.children.iter().find(|&&child_idx| {
                            self.components[child_idx].name.as_str() == dst_sub_name.as_str()
                        }).copied();

                        if let Some(dst_component) = dst_idx {
                            let mut path = vec![*conn_idx];
                            let ultimate_dst =
                                self.trace_destination(dst_component, &dst.feature, &mut path, MAX_TRACE_DEPTH);

                            semantic.push(SemanticConnection {
                                name: conn_name,
                                kind: conn_kind,
                                ultimate_source: (conn_owner, src.feature.clone()),
                                ultimate_destination: ultimate_dst,
                                connection_path: path,
                            });
                        }
                    }
                }

                // Both endpoints on the enclosing component (no subcomponents) — skip.
                (None, None) => {}
            }
        }

        self.semantic_connections = semantic;
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
            let src_expanded = self.expand_endpoint_feature_group(
                scope, src_comp_idx, &src_end.feature,
            );
            let dst_expanded = self.expand_endpoint_feature_group(
                scope, dst_comp_idx, &dst_end.feature,
            );

            let (src_features, dst_features) = match (src_expanded, dst_expanded) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Match features by name and create individual semantic connections.
            for src_feat in &src_features {
                for dst_feat in &dst_features {
                    if src_feat.name.eq_ci(&dst_feat.name) {
                        // Build the dotted feature name: group_prefix.feature_name or just feature_name
                        let src_full_name = make_expanded_name(
                            &src_end.feature, &src_feat.group_prefix, &src_feat.name,
                        );
                        let dst_full_name = make_expanded_name(
                            &dst_end.feature, &dst_feat.group_prefix, &dst_feat.name,
                        );

                        expanded.push(SemanticConnection {
                            name: Name::new(&format!(
                                "{}.{}", conn_name, src_feat.name
                            )),
                            kind: feature_kind_to_connection_kind(src_feat.kind),
                            ultimate_source: (src_comp_idx, src_full_name),
                            ultimate_destination: (dst_comp_idx, dst_full_name),
                            connection_path: vec![*conn_idx],
                        });
                        break; // matched; move to next src feature
                    }
                }
            }
        }

        self.semantic_connections.extend(expanded);
    }

    /// Resolve the component index for a connection endpoint.
    ///
    /// If `subcomponent` is Some, look for a child with that name.
    /// If None, return the owner itself.
    fn resolve_endpoint_component(
        &self,
        owner: ComponentInstanceIdx,
        subcomponent: &Option<Name>,
    ) -> Option<ComponentInstanceIdx> {
        match subcomponent {
            Some(sub_name) => {
                let owner_comp = &self.components[owner];
                owner_comp.children.iter().find(|&&child_idx| {
                    self.components[child_idx].name.eq_ci(sub_name)
                }).copied()
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

    /// Trace the ultimate source of a connection by following up connections
    /// inside a subcomponent.
    ///
    /// Given a component instance and a feature name on that component, look for
    /// an "up" connection inside it of the form `inner_sub.port -> feature_name`.
    /// If found, recurse into `inner_sub` to find the deepest source.
    ///
    /// Returns `(component_idx, feature_name)` for the deepest source found.
    fn trace_source(
        &self,
        component: ComponentInstanceIdx,
        feature: &Name,
        path: &mut Vec<ConnectionInstanceIdx>,
        depth_remaining: usize,
    ) -> (ComponentInstanceIdx, Name) {
        if depth_remaining == 0 {
            return (component, feature.clone());
        }

        // Clone the connection indices to avoid borrow conflicts.
        let conn_indices: Vec<ConnectionInstanceIdx> =
            self.components[component].connections.clone();

        // Look through connections owned by this component for an up connection
        // whose destination feature matches (i.e., `sub.port -> feature`).
        for conn_idx in conn_indices {
            let conn = &self.connections[conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Up connection: source has subcomponent, destination does not,
            // and destination feature matches the one we're tracing.
            if let (Some(src_sub_name), None) = (&src.subcomponent, &dst.subcomponent) {
                if dst.feature.as_str() == feature.as_str() {
                    // Found an up connection feeding this port.
                    // Resolve the source subcomponent.
                    let inner_sub = self.components[component]
                        .children
                        .iter()
                        .find(|&&child_idx| {
                            self.components[child_idx].name.as_str() == src_sub_name.as_str()
                        })
                        .copied();

                    if let Some(inner_component) = inner_sub {
                        let src_feature = src.feature.clone();
                        path.push(conn_idx);
                        return self.trace_source(
                            inner_component,
                            &src_feature,
                            path,
                            depth_remaining - 1,
                        );
                    }
                }
            }
        }

        // No further up connection found — this is the ultimate source.
        (component, feature.clone())
    }

    /// Trace the ultimate destination of a connection by following down connections
    /// inside a subcomponent.
    ///
    /// Given a component instance and a feature name on that component, look for
    /// a "down" connection inside it of the form `feature_name -> inner_sub.port`.
    /// If found, recurse into `inner_sub` to find the deepest destination.
    ///
    /// Returns `(component_idx, feature_name)` for the deepest destination found.
    fn trace_destination(
        &self,
        component: ComponentInstanceIdx,
        feature: &Name,
        path: &mut Vec<ConnectionInstanceIdx>,
        depth_remaining: usize,
    ) -> (ComponentInstanceIdx, Name) {
        if depth_remaining == 0 {
            return (component, feature.clone());
        }

        // Clone the connection indices to avoid borrow conflicts.
        let conn_indices: Vec<ConnectionInstanceIdx> =
            self.components[component].connections.clone();

        // Look through connections owned by this component for a down connection
        // whose source feature matches (i.e., `feature -> sub.port`).
        for conn_idx in conn_indices {
            let conn = &self.connections[conn_idx];
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Down connection: source has no subcomponent, destination has subcomponent,
            // and source feature matches the one we're tracing.
            if let (None, Some(dst_sub_name)) = (&src.subcomponent, &dst.subcomponent) {
                if src.feature.as_str() == feature.as_str() {
                    // Found a down connection distributing from this port.
                    // Resolve the destination subcomponent.
                    let inner_sub = self.components[component]
                        .children
                        .iter()
                        .find(|&&child_idx| {
                            self.components[child_idx].name.as_str() == dst_sub_name.as_str()
                        })
                        .copied();

                    if let Some(inner_component) = inner_sub {
                        let dst_feature = dst.feature.clone();
                        path.push(conn_idx);
                        return self.trace_destination(
                            inner_component,
                            &dst_feature,
                            path,
                            depth_remaining - 1,
                        );
                    }
                }
            }
        }

        // No further down connection found — this is the ultimate destination.
        (component, feature.clone())
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
        FeatureKind::DataPort
        | FeatureKind::EventPort
        | FeatureKind::EventDataPort => ConnectionKind::Port,
        FeatureKind::Parameter => ConnectionKind::Parameter,
        FeatureKind::DataAccess
        | FeatureKind::BusAccess
        | FeatureKind::SubprogramAccess
        | FeatureKind::SubprogramGroupAccess => ConnectionKind::Access,
        FeatureKind::FeatureGroup => ConnectionKind::FeatureGroup,
        FeatureKind::AbstractFeature => ConnectionKind::Feature,
    }
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
    fn instantiate_component(
        &mut self,
        instance_name: &Name,
        package: &Name,
        type_name: &Name,
        impl_name: &Name,
        parent: Option<ComponentInstanceIdx>,
        subcomponent_loc: Option<(usize, crate::item_tree::SubcomponentIdx)>,
    ) -> ComponentInstanceIdx {
        // Resolve the implementation
        let ref_ = ClassifierRef::implementation(
            Some(package.clone()),
            type_name.clone(),
            impl_name.clone(),
        );
        let resolved = self.scope.resolve_classifier(package, &ref_);

        let (category, impl_loc) = match &resolved {
            ResolvedClassifier::ComponentImpl { loc, .. } => {
                let ci = self.scope.get_component_impl(*loc);
                let cat = ci.map(|c| c.category).unwrap_or(ComponentCategory::System);
                (cat, Some(*loc))
            }
            _ => {
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!("unresolved implementation: {}", ref_),
                    path: vec![instance_name.clone()],
                });
                (ComponentCategory::System, None)
            }
        };

        // Resolve the type to get features
        let type_ref = ClassifierRef::qualified(package.clone(), type_name.clone());
        let type_resolved = self.scope.resolve_classifier(package, &type_ref);
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
            package: package.clone(),
            parent,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
        });

        // Build property map: type → impl → subcomponent layering
        self.build_property_map(idx, type_loc, impl_loc, subcomponent_loc);

        // Instantiate features and flows from the type
        if let Some(loc) = type_loc {
            if let Some(ct) = self.scope.get_component_type(loc) {
                let mut feat_indices = Vec::new();
                for &feat_idx in &ct.features {
                    if let Some(feat) = self.scope.get_feature(loc.tree, feat_idx) {
                        let fi = self.features.alloc(FeatureInstance {
                            name: feat.name.clone(),
                            kind: feat.kind,
                            direction: feat.direction,
                            owner: idx,
                        });
                        feat_indices.push(fi);
                    }
                }
                self.components[idx].features = feat_indices;

                // Instantiate flow specs from the type
                let mut flow_indices = Vec::new();
                for &flow_idx in &ct.flow_specs {
                    if let Some(tree) = self.scope.tree(loc.tree) {
                        let flow_spec = &tree.flow_specs[flow_idx];
                        let fi = self.flow_instances.alloc(FlowInstance {
                            name: flow_spec.name.clone(),
                            kind: flow_spec.kind,
                            owner: idx,
                        });
                        flow_indices.push(fi);
                    }
                }
                self.components[idx].flows = flow_indices;

                // Instantiate modes from the type
                let mut mode_indices = Vec::new();
                for &mode_idx in &ct.modes {
                    if let Some(tree) = self.scope.tree(loc.tree) {
                        let mode = &tree.modes[mode_idx];
                        let mi = self.mode_instances.alloc(ModeInstance {
                            name: mode.name.clone(),
                            is_initial: mode.is_initial,
                            owner: idx,
                        });
                        mode_indices.push(mi);
                    }
                }

                // Instantiate mode transitions from the type
                let mut mt_indices = Vec::new();
                for &mt_idx in &ct.mode_transitions {
                    if let Some(tree) = self.scope.tree(loc.tree) {
                        let mt = &tree.mode_transitions[mt_idx];
                        let mti = self.mode_transition_instances.alloc(ModeTransitionInstance {
                            name: mt.name.clone(),
                            source: mt.source.clone(),
                            destination: mt.destination.clone(),
                            triggers: mt.triggers.clone(),
                            owner: idx,
                        });
                        mt_indices.push(mti);
                    }
                }
                self.components[idx].modes = mode_indices;
                self.components[idx].mode_transitions = mt_indices;
            }
        }

        // Instantiate subcomponents (recursive)
        if let Some(loc) = impl_loc {
            if self.depth < self.max_depth {
                self.depth += 1;

                if let Some(ci) = self.scope.get_component_impl(loc) {
                    let sub_data: Vec<_> = ci
                        .subcomponents
                        .iter()
                        .filter_map(|&sub_idx| {
                            let tree = self.scope.tree(loc.tree)?;
                            let sub = &tree.subcomponents[sub_idx];
                            Some((
                                sub.name.clone(),
                                sub.category,
                                sub.classifier.clone(),
                                sub_idx,
                            ))
                        })
                        .collect();

                    let conn_data: Vec<_> = ci
                        .connections
                        .iter()
                        .filter_map(|&conn_idx| {
                            let tree = self.scope.tree(loc.tree)?;
                            let conn = &tree.connections[conn_idx];
                            let src = conn.src.as_ref().map(|ce| ConnectionEnd {
                                subcomponent: ce.subcomponent.clone(),
                                feature: ce.feature.clone(),
                            });
                            let dst = conn.dst.as_ref().map(|ce| ConnectionEnd {
                                subcomponent: ce.subcomponent.clone(),
                                feature: ce.feature.clone(),
                            });
                            Some((
                                conn.name.clone(),
                                conn.kind,
                                conn.is_bidirectional,
                                src,
                                dst,
                            ))
                        })
                        .collect();

                    // Collect end-to-end flow data from the implementation
                    let e2e_data: Vec<_> = ci
                        .end_to_end_flows
                        .iter()
                        .filter_map(|&e2e_idx| {
                            let tree = self.scope.tree(loc.tree)?;
                            let e2e = &tree.end_to_end_flows[e2e_idx];
                            Some((e2e.name.clone(), e2e.segments.clone()))
                        })
                        .collect();

                    // Collect modes from the implementation (supplement type modes)
                    let impl_mode_data: Vec<_> = ci
                        .modes
                        .iter()
                        .filter_map(|&mode_idx| {
                            let tree = self.scope.tree(loc.tree)?;
                            let mode = &tree.modes[mode_idx];
                            Some((mode.name.clone(), mode.is_initial))
                        })
                        .collect();

                    let impl_mt_data: Vec<_> = ci
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

                    let mut child_indices = Vec::new();
                    for (sub_name, _sub_cat, sub_classifier, sub_idx) in sub_data {
                        if let Some(cls_ref) = sub_classifier {
                            // If the classifier has package + type + impl, instantiate recursively
                            let sub_pkg = cls_ref.package.as_ref().unwrap_or(package);
                            if let Some(sub_impl) = &cls_ref.impl_name {
                                let child_idx = self.instantiate_component(
                                    &sub_name,
                                    sub_pkg,
                                    &cls_ref.type_name,
                                    sub_impl,
                                    Some(idx),
                                    Some((loc.tree, sub_idx)),
                                );
                                child_indices.push(child_idx);
                            } else {
                                // Type-only reference — leaf subcomponent
                                let child_idx = self.components.alloc(ComponentInstance {
                                    name: sub_name,
                                    category: _sub_cat,
                                    type_name: cls_ref.type_name.clone(),
                                    impl_name: None,
                                    package: sub_pkg.clone(),
                                    parent: Some(idx),
                                    children: Vec::new(),
                                    features: Vec::new(),
                                    connections: Vec::new(),
                                    flows: Vec::new(),
                                    modes: Vec::new(),
                                    mode_transitions: Vec::new(),
                                });
                                // Build property map for leaf subcomponent (type only)
                                self.build_leaf_property_map(child_idx, sub_pkg, &cls_ref.type_name, loc.tree, sub_idx);
                                child_indices.push(child_idx);
                            }
                        } else {
                            // No classifier — anonymous subcomponent
                            let child_idx = self.components.alloc(ComponentInstance {
                                name: sub_name,
                                category: _sub_cat,
                                type_name: Name::default(),
                                impl_name: None,
                                package: package.clone(),
                                parent: Some(idx),
                                children: Vec::new(),
                                features: Vec::new(),
                                connections: Vec::new(),
                                flows: Vec::new(),
                                modes: Vec::new(),
                                mode_transitions: Vec::new(),
                            });
                            // Build property map for anonymous subcomponent
                            self.build_anon_property_map(child_idx, loc.tree, sub_idx);
                            child_indices.push(child_idx);
                        }
                    }
                    self.components[idx].children = child_indices;

                    // Instantiate connections
                    let mut conn_indices = Vec::new();
                    for (conn_name, conn_kind, bidi, src, dst) in conn_data {
                        let ci = self.connections.alloc(ConnectionInstance {
                            name: conn_name,
                            kind: conn_kind,
                            is_bidirectional: bidi,
                            owner: idx,
                            src,
                            dst,
                        });
                        conn_indices.push(ci);
                    }
                    self.components[idx].connections = conn_indices;

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
                        if !existing_mode_names.iter().any(|n| n.as_str() == mode_name.as_str()) {
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
                        let mti = self.mode_transition_instances.alloc(ModeTransitionInstance {
                            name: mt_name,
                            source: mt_source,
                            destination: mt_dest,
                            triggers: mt_triggers,
                            owner: idx,
                        });
                        self.components[idx].mode_transitions.push(mti);
                    }
                }

                self.depth -= 1;
            } else {
                self.diagnostics.push(InstanceDiagnostic {
                    message: format!(
                        "maximum instantiation depth ({}) exceeded",
                        self.max_depth
                    ),
                    path: vec![instance_name.clone()],
                });
            }
        }

        idx
    }

    /// Build a property map for a component instance with type + impl + subcomponent layering.
    fn build_property_map(
        &mut self,
        idx: ComponentInstanceIdx,
        type_loc: Option<crate::resolver::ItemLoc>,
        impl_loc: Option<crate::resolver::ItemLoc>,
        subcomponent_loc: Option<(usize, crate::item_tree::SubcomponentIdx)>,
    ) {
        use crate::item_tree::{ComponentImplIdx, ComponentTypeIdx};

        let mut map = PropertyMap::new();

        // 1. Type-level properties
        if let Some(loc) = type_loc {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let ct_idx: ComponentTypeIdx =
                    la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
                let ct = &tree.component_types[ct_idx];
                for &pa_idx in &ct.property_associations {
                    let pa = &tree.property_associations[pa_idx];
                    map.add(crate::properties::PropertyValue {
                        name: pa.name.clone(),
                        value: pa.value.clone(),
                        is_append: pa.is_append,
                    });
                }
            }
        }

        // 2. Implementation-level properties (override type)
        if let Some(loc) = impl_loc {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let ci_idx: ComponentImplIdx =
                    la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
                let ci = &tree.component_impls[ci_idx];
                for &pa_idx in &ci.property_associations {
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
        if let Some((tree_idx, sub_idx)) = subcomponent_loc {
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

        // Resolve type to get type-level properties
        let type_ref = ClassifierRef::qualified(package.clone(), type_name.clone());
        let type_resolved = self.scope.resolve_classifier(package, &type_ref);
        if let ResolvedClassifier::ComponentType { loc, .. } = &type_resolved {
            if let Some(tree) = self.scope.tree(loc.tree) {
                let ct_idx: crate::item_tree::ComponentTypeIdx =
                    la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
                let ct = &tree.component_types[ct_idx];
                for &pa_idx in &ct.property_associations {
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
}
