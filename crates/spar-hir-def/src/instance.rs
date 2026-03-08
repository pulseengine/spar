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
        };
        instance.compute_semantic_connections();
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

    /// Compute semantic (end-to-end) connection instances by tracing connections
    /// through the component hierarchy.
    ///
    /// Currently handles the common case: "across" connections within a single
    /// implementation level (sub_a.port -> sub_b.port). These are already fully
    /// resolved — we just convert them to `SemanticConnection` by resolving the
    /// subcomponent names to `ComponentInstanceIdx`.
    pub fn compute_semantic_connections(&mut self) {
        let mut semantic = Vec::new();

        for (conn_idx, conn) in self.connections.iter() {
            let (src, dst) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                // Skip connections with missing endpoints.
                _ => continue,
            };

            // Only handle "across" connections: both endpoints reference a subcomponent.
            let (src_sub_name, dst_sub_name) = match (&src.subcomponent, &dst.subcomponent) {
                (Some(s), Some(d)) => (s, d),
                // "up" or "down" connections require multi-level tracing — skip for now.
                _ => continue,
            };

            // Resolve subcomponent names to ComponentInstanceIdx by looking at
            // the owner's children.
            let owner = &self.components[conn.owner];
            let src_idx = owner.children.iter().find(|&&child_idx| {
                self.components[child_idx].name.as_str() == src_sub_name.as_str()
            });
            let dst_idx = owner.children.iter().find(|&&child_idx| {
                self.components[child_idx].name.as_str() == dst_sub_name.as_str()
            });

            if let (Some(&src_component), Some(&dst_component)) = (src_idx, dst_idx) {
                semantic.push(SemanticConnection {
                    name: conn.name.clone(),
                    kind: conn.kind,
                    ultimate_source: (src_component, src.feature.clone()),
                    ultimate_destination: (dst_component, dst.feature.clone()),
                    connection_path: vec![conn_idx],
                });
            }
        }

        self.semantic_connections = semantic;
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
             Diagnostics: {}",
            self.components.len(),
            self.features.len(),
            self.connections.len(),
            self.semantic_connections.len(),
            self.flow_instances.len(),
            self.end_to_end_flows.len(),
            self.mode_instances.len(),
            self.mode_transition_instances.len(),
            self.diagnostics.len(),
        )
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
