//! Property evaluation for AADL models.
//!
//! Implements the AADL property inheritance rules from AS5506 Section 11:
//!
//! 1. Properties on a component type apply to all instances of that type.
//! 2. Properties on a component implementation override/extend type properties.
//! 3. Properties on subcomponent declarations override implementation-level.
//! 4. `+=>` appends to inherited values (for list properties).
//! 5. Modal values (`v1 in modes (m1), v2 in modes (m2), v3`): the map keeps
//!    the FIRST value as the key's value — the reading every non-modal query
//!    gets — and records every value with its modes beside it, so
//!    [`PropertyMap::in_mode`] can give the value of one mode
//!    (REQ-AADL-MODAL-PROPERTY-VALUES-001).

use rustc_hash::FxHashMap;

use std::borrow::Cow;

use crate::item_tree::{
    ComponentImplIdx, ComponentTypeIdx, ItemTree, PropertyAssociationIdx, PropertyExpr,
    SubcomponentIdx,
};
use crate::name::{Name, PropertyRef};
use crate::resolver::CiName;

/// A single property value, either assigned (`=>`) or appended (`+=>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValue {
    /// The property reference (possibly qualified with a property set).
    pub name: PropertyRef,
    /// Raw text of the property value expression.
    pub value: String,
    /// Typed property expression (when available from the parser).
    pub typed_expr: Option<PropertyExpr>,
    /// Whether this is an append association (`+=>`).
    pub is_append: bool,
}

impl PropertyValue {
    /// Is this value arithmetic, which is not AADL and has no value (#546)?
    pub fn is_arithmetic(&self) -> bool {
        self.typed_expr
            .as_ref()
            .is_some_and(crate::property_arith::is_arithmetic)
    }
}

/// WHERE a property value came from: the mechanism that put it in a map
/// (REQ-AADL-PROPERTY-PROVENANCE-001).
///
/// Recorded at the moment the value is inserted, never reconstructed: each
/// variant names one of the ways instantiation layers values onto a
/// component instance. It is metadata only — no analysis reads it, and it
/// takes no part in [`PropertyMap`] equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertySource {
    /// Written on the component's classifier: its component TYPE, or a type
    /// that type extends. `classifier` names the type that wrote it.
    Classifier { classifier: String },
    /// Written on the component's IMPLEMENTATION, or an implementation it
    /// extends. `classifier` names the implementation that wrote it.
    Implementation { classifier: String },
    /// Written on the declaration itself: the subcomponent's `{ ... }` block,
    /// or a connection's / end-to-end flow's own block.
    DeclaredLocal,
    /// Written on a subcomponent that this one `refined to` (#496): the
    /// refinement inherits the original's values. `subcomponent` is the
    /// original's name, `declared_in` the implementation that declared it.
    Refined {
        subcomponent: String,
        declared_in: String,
    },
    /// A contained association, `... applies to <path>`, declared on
    /// `declarer` — also the component its `reference (...)` terms resolve
    /// from (REQ-AADL-REF-CONTEXT-001).
    Contained {
        declarer: crate::instance::ComponentInstanceIdx,
    },
    /// A contained association whose `applies to` path named nothing, kept on
    /// its declarer as spar's recovery (not AADL semantics).
    ContainedUnresolved { path: String },
    /// An `inherit` property taken from the nearest containing component that
    /// has a value (#460, REQ-AADL-PROPERTY-INHERIT-001). `origin` is how the
    /// value reached `ancestor`.
    Inherited {
        ancestor: crate::instance::ComponentInstanceIdx,
        origin: Box<PropertySource>,
    },
    /// Inserted through [`PropertyMap::add`] by code that states no source:
    /// tests, overlays, analysis-side rebinding. Never a guess.
    Unrecorded,
}

/// A collection of resolved property values for a model element.
///
/// Properties are keyed by `(property_set_ci, property_name_ci)` where
/// the property set key is empty string for unqualified properties.
/// Multiple values for the same key are possible via `+=>` append.
///
/// Equality compares the values, their reference contexts and their modal
/// bindings; the provenance of a value is metadata and never makes two maps
/// differ.
#[derive(Debug, Clone, Default)]
pub struct PropertyMap {
    props: FxHashMap<(CiName, CiName), Vec<PropertyValue>>,
    /// The component a key's `reference (...)` values resolve from, when
    /// that is not the component holding this map — i.e. when the value
    /// arrived through `applies to` (REQ-AADL-REF-CONTEXT-001). Absent means
    /// the holder itself, which is OSATE's context for every association
    /// that has no `applies to`.
    ref_ctx: FxHashMap<(CiName, CiName), crate::instance::ComponentInstanceIdx>,
    /// Where each value came from, index-aligned with the key's value list
    /// in `props` (REQ-AADL-PROPERTY-PROVENANCE-001).
    sources: FxHashMap<(CiName, CiName), Vec<PropertySource>>,
    /// The modal values of the association a key's value came from, when
    /// that association is modal (REQ-AADL-MODAL-PROPERTY-VALUES-001).
    /// Absent for every non-modal value, so a model without modal values has
    /// an empty table and every query reads exactly what it read before.
    modal: FxHashMap<(CiName, CiName), ModalBinding>,
}

/// One value of a modal property association and the modes it applies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalValue {
    /// The modes this value applies in; empty for the value that applies in
    /// every mode no other value of the association names.
    pub in_modes: Vec<Name>,
    /// The value.
    pub value: PropertyValue,
}

/// Every value of one modal property association, with the component whose
/// modes its `in modes` lists name.
///
/// That component is the one that DECLARED the association: the component
/// itself for an association in its own type or implementation, the
/// enclosing component for a subcomponent-level association or an
/// `applies to` one, and the declarer's for an inherited value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalBinding {
    /// The component whose modes the values' `in modes` lists name.
    pub mode_context: crate::instance::ComponentInstanceIdx,
    /// The values in source order.
    pub values: Vec<ModalValue>,
}

impl ModalBinding {
    /// The value that applies in `mode`, by OSATE's rule
    /// (osate2 @ 87468992, PropertyAssociationImpl.java:510-524,
    /// `valueInMode`): the first value whose `in modes` names `mode`,
    /// otherwise the value with no `in modes`, otherwise none.
    pub fn value_in_mode(&self, mode: &str) -> Option<&PropertyValue> {
        let mut default = None;
        for mv in &self.values {
            if mv.in_modes.is_empty() {
                default = Some(&mv.value);
            } else if mv
                .in_modes
                .iter()
                .any(|m| m.as_str().eq_ignore_ascii_case(mode))
            {
                return Some(&mv.value);
            }
        }
        default
    }
}

/// Provenance (`sources`) is metadata and takes no part in equality. The
/// modal table DOES: it decides the value a key has in each system operation
/// mode, which `--per-som` analyses read, so two maps that agree on every
/// first value but not on a modal binding read differently and are not equal.
/// For a map without modal values the table is empty and equality is what it
/// was before modal values were recorded.
impl PartialEq for PropertyMap {
    fn eq(&self, other: &Self) -> bool {
        self.props == other.props && self.ref_ctx == other.ref_ctx && self.modal == other.modal
    }
}

impl Eq for PropertyMap {}

impl PropertyMap {
    /// Create a new empty property map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a property value.
    ///
    /// If `is_append` is true, the value is appended to any existing values.
    /// Otherwise, all existing values for that key are replaced.
    ///
    /// Properties whose value is empty (or whitespace-only) are silently
    /// skipped so that downstream analysis passes do not have to guard
    /// against blank strings.
    pub fn add(&mut self, prop: PropertyValue) {
        self.add_from(prop, PropertySource::Unrecorded);
    }

    /// [`Self::add`], recording where the value came from.
    pub fn add_from(&mut self, prop: PropertyValue, source: PropertySource) {
        let value = prop.value.trim().to_string();
        if value.is_empty() {
            return; // Skip empty property values
        }
        let prop = PropertyValue { value, ..prop };

        let set_key = match &prop.name.property_set {
            Some(ps) => CiName::new(ps),
            None => CiName::from_str(""),
        };
        let name_key = CiName::new(&prop.name.property_name);
        let key = (set_key, name_key);

        // A modal binding belongs to the one association it came from: a later
        // assignment replaces it, and an append to a modal value is not
        // modelled (OSATE rejects it), so either leaves the key non-modal.
        self.modal.remove(&key);
        if prop.is_append {
            self.sources.entry(key.clone()).or_default().push(source);
            self.props.entry(key).or_default().push(prop);
        } else {
            // Override: replace all existing values. The replaced values'
            // reference context and sources go with them, so neither can
            // outlive the value it was recorded for.
            self.ref_ctx.remove(&key);
            self.sources.insert(key.clone(), vec![source]);
            self.props.insert(key, vec![prop]);
        }
    }

    /// Add a property value whose `reference (...)` terms resolve from
    /// `ctx` rather than from the component holding this map.
    ///
    /// OSATE resolves a contained association's references "in the context
    /// of the contained PA's owner" (osate2 @ 87468992,
    /// CacheContainedPropertyAssociationsSwitch), so a value attached through
    /// `applies to` records the declaring component here, and records the
    /// value's source as [`PropertySource::Contained`] from `ctx`.
    pub fn add_with_reference_context(
        &mut self,
        prop: PropertyValue,
        ctx: crate::instance::ComponentInstanceIdx,
    ) {
        let key = (
            match &prop.name.property_set {
                Some(ps) => CiName::new(ps),
                None => CiName::from_str(""),
            },
            CiName::new(&prop.name.property_name),
        );
        let had_value = !prop.value.trim().is_empty();
        self.add_from(prop, PropertySource::Contained { declarer: ctx });
        if had_value {
            self.ref_ctx.insert(key, ctx);
        }
    }

    /// [`Self::add_from`], also recording `modal` — the association's modal
    /// values — beside the value, and `reference_context` as
    /// [`Self::add_with_reference_context`] does when it is given. The value
    /// keeps BOTH its provenance (`source`) and its modal binding.
    ///
    /// `prop` is the reading every non-modal query gets: the association's
    /// first value. The binding is recorded only when `prop` is a non-empty
    /// assignment, the case in which it becomes the key's value.
    pub fn add_modal(
        &mut self,
        prop: PropertyValue,
        source: PropertySource,
        modal: Option<ModalBinding>,
        reference_context: Option<crate::instance::ComponentInstanceIdx>,
    ) {
        let key = Self::key_of(&prop.name);
        let has_value = !prop.value.trim().is_empty();
        let assigns = !prop.is_append && has_value;
        self.add_from(prop, source);
        if has_value && let Some(ctx) = reference_context {
            self.ref_ctx.insert(key.clone(), ctx);
        }
        if assigns && let Some(binding) = modal {
            self.modal.insert(key, binding);
        }
    }

    /// Whether any value in this map came from a modal association.
    pub fn has_modal_values(&self) -> bool {
        !self.modal.is_empty()
    }

    /// The modal binding recorded for the key `(set, name)` exactly.
    pub(crate) fn modal_for_key(&self, key: &(CiName, CiName)) -> Option<&ModalBinding> {
        self.modal.get(key)
    }

    /// This map as it reads in one system operation mode.
    ///
    /// `mode_of(c)` is the mode the SOM selects for component `c`. Each key
    /// whose value came from a modal association takes the value that applies
    /// in the mode selected for that association's declarer
    /// ([`ModalBinding::value_in_mode`]); a key no value of which applies in
    /// that mode has no value in it. A key whose declarer has no selected
    /// mode keeps its first value, the non-modal path's reading. Every other key is unchanged, and a
    /// map with no modal value is returned as it is.
    pub fn in_mode<'m>(
        &self,
        mode_of: impl Fn(crate::instance::ComponentInstanceIdx) -> Option<&'m str>,
    ) -> Cow<'_, PropertyMap> {
        if self.modal.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut out = self.clone();
        for (key, binding) in &self.modal {
            let Some(mode) = mode_of(binding.mode_context) else {
                continue;
            };
            match binding.value_in_mode(mode) {
                Some(value) => {
                    out.props.insert(key.clone(), vec![value.clone()]);
                }
                None => {
                    out.props.remove(key);
                    out.ref_ctx.remove(key);
                }
            }
        }
        Cow::Owned(out)
    }

    /// The map key a property reference is stored under: `(set, name)`,
    /// case-folded, with `""` as the set of an unqualified reference.
    pub(crate) fn key_of(name: &PropertyRef) -> (CiName, CiName) {
        (
            match &name.property_set {
                Some(ps) => CiName::new(ps),
                None => CiName::from_str(""),
            },
            CiName::new(&name.property_name),
        )
    }

    /// Whether the map holds a value under the key `(set, name)` exactly.
    pub(crate) fn has_key(&self, key: &(CiName, CiName)) -> bool {
        self.props.contains_key(key)
    }

    /// Install a value INHERITED from a containing component (`inherit`
    /// properties, REQ-AADL-PROPERTY-INHERIT-001): the ancestor's values for
    /// `key`, verbatim — including any `+=>` list it built — together with
    /// the component its `reference (...)` terms resolve from, which is the
    /// DECLARER's context, never the inheriting component. OSATE's inherited
    /// value is already instantiated in the ancestor's context.
    ///
    /// `origins` are the ancestor's sources for those values, index-aligned;
    /// each value is recorded as [`PropertySource::Inherited`] from
    /// `ancestor`, carrying how it reached the ancestor. `modal` is the
    /// ancestor's modal binding for the key, if any: an inherited modal value
    /// keeps its modes, still named by its declarer.
    pub(crate) fn insert_inherited(
        &mut self,
        key: (CiName, CiName),
        values: Vec<PropertyValue>,
        ctx: crate::instance::ComponentInstanceIdx,
        ancestor: crate::instance::ComponentInstanceIdx,
        origins: &[PropertySource],
        modal: Option<ModalBinding>,
    ) {
        let sources = (0..values.len())
            .map(|i| PropertySource::Inherited {
                ancestor,
                origin: Box::new(
                    origins
                        .get(i)
                        .cloned()
                        .unwrap_or(PropertySource::Unrecorded),
                ),
            })
            .collect();
        self.ref_ctx.insert(key.clone(), ctx);
        self.sources.insert(key.clone(), sources);
        match modal {
            Some(binding) => self.modal.insert(key.clone(), binding),
            None => self.modal.remove(&key),
        };
        self.props.insert(key, values);
    }

    /// Where each value stored under the key `(set, name)` exactly came
    /// from, index-aligned with [`Self::values_for_key`]
    /// (REQ-AADL-PROPERTY-PROVENANCE-001).
    pub fn sources_for_key(&self, key: &(CiName, CiName)) -> &[PropertySource] {
        self.sources.get(key).map_or(&[], Vec::as_slice)
    }

    /// The values stored under the key `(set, name)` exactly.
    pub fn values_for_key(&self, key: &(CiName, CiName)) -> Option<&Vec<PropertyValue>> {
        self.props.get(key)
    }

    /// The reference context recorded for the key `(set, name)` exactly,
    /// when it is not the holder of this map.
    pub fn reference_context_for_key(
        &self,
        key: &(CiName, CiName),
    ) -> Option<crate::instance::ComponentInstanceIdx> {
        self.ref_ctx.get(key).copied()
    }

    /// The component the value at `(set, name)` resolves its references
    /// from, if it is not the holder of this map.
    pub fn reference_context(
        &self,
        set: &str,
        name: &str,
    ) -> Option<crate::instance::ComponentInstanceIdx> {
        self.ref_ctx
            .get(&(CiName::from_str(set), CiName::from_str(name)))
            .copied()
    }

    /// Look up a property value by property set and name.
    ///
    /// Returns the value text of the most recent assignment, or `None`.
    /// For append properties, returns the first value; use `get_all` for
    /// the complete list.
    ///
    /// A value that is arithmetic (`5 * 8 ms`) is not AADL and reads as
    /// ABSENT here, in [`Self::get_typed`] and in [`Self::get_all`] (#546,
    /// [`crate::property_arith`]): no analysis, typed or text-parsing, can
    /// compute anything from it. [`Self::iter`] still yields it, so
    /// `property_rules` can report it and instance output can show it.
    pub fn get(&self, set: &str, name: &str) -> Option<&str> {
        let set_key = CiName::from_str(set);
        let name_key = CiName::from_str(name);
        self.props
            .get(&(set_key, name_key))
            .and_then(|vals| vals.first())
            .filter(|pv| !pv.is_arithmetic())
            .map(|pv| pv.value.as_str())
    }

    /// Look up the typed property expression by property set and name.
    ///
    /// Returns the typed expression of the most recent assignment, or `None`
    /// if no value exists or if the value has no typed expression.
    pub fn get_typed(&self, set: &str, name: &str) -> Option<&PropertyExpr> {
        let set_key = CiName::from_str(set);
        let name_key = CiName::from_str(name);
        self.props
            .get(&(set_key, name_key))
            .and_then(|vals| vals.first())
            .filter(|pv| !pv.is_arithmetic())
            .and_then(|pv| pv.typed_expr.as_ref())
    }

    /// Look up all property values for a given property set and name.
    ///
    /// Returns all values including appended ones, in order.
    pub fn get_all(&self, set: &str, name: &str) -> Vec<&str> {
        let set_key = CiName::from_str(set);
        let name_key = CiName::from_str(name);
        self.props
            .get(&(set_key, name_key))
            .map(|vals| {
                vals.iter()
                    .filter(|pv| !pv.is_arithmetic())
                    .map(|pv| pv.value.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Return the number of distinct property keys.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    /// Check if the property map is empty.
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Iterate over all property entries.
    pub fn iter(&self) -> impl Iterator<Item = (&(CiName, CiName), &Vec<PropertyValue>)> {
        self.props.iter()
    }

    /// Collect properties for a component from its type and implementation.
    ///
    /// Applies the AADL inheritance rules:
    /// 1. Start with type-level properties.
    /// 2. Implementation properties override or append.
    /// 3. Subcomponent-level properties override or append.
    pub fn collect_for_component(
        tree: &ItemTree,
        component_type_idx: Option<ComponentTypeIdx>,
        component_impl_idx: Option<ComponentImplIdx>,
    ) -> PropertyMap {
        let mut map = PropertyMap::new();

        // 1. Collect from component type
        if let Some(ct_idx) = component_type_idx {
            let ct = &tree.component_types[ct_idx];
            for &pa_idx in &ct.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    typed_expr: pa.typed_value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        // 2. Collect from component implementation (overrides type)
        if let Some(ci_idx) = component_impl_idx {
            let ci = &tree.component_impls[ci_idx];
            for &pa_idx in &ci.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    typed_expr: pa.typed_value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        map
    }

    /// Collect properties for a subcomponent, layering on top of an
    /// inherited property map from the subcomponent's type/impl.
    pub fn collect_for_subcomponent(
        tree: &ItemTree,
        base: PropertyMap,
        subcomponent_idx: SubcomponentIdx,
    ) -> PropertyMap {
        let mut map = base;

        let sub = &tree.subcomponents[subcomponent_idx];
        for &pa_idx in &sub.property_associations {
            let pa = &tree.property_associations[pa_idx];
            map.add(PropertyValue {
                name: pa.name.clone(),
                value: pa.value.clone(),
                typed_expr: pa.typed_value.clone(),
                is_append: pa.is_append,
            });
        }

        map
    }

    /// Collect properties from a list of property association indices.
    pub fn from_associations(tree: &ItemTree, indices: &[PropertyAssociationIdx]) -> PropertyMap {
        let mut map = PropertyMap::new();
        for &pa_idx in indices {
            let pa = &tree.property_associations[pa_idx];
            map.add(PropertyValue {
                name: pa.name.clone(),
                value: pa.value.clone(),
                typed_expr: pa.typed_value.clone(),
                is_append: pa.is_append,
            });
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::PropertyRef;

    fn make_prop(set: Option<&str>, name: &str, value: &str, is_append: bool) -> PropertyValue {
        PropertyValue {
            name: PropertyRef {
                property_set: set.map(|s| s.into()),
                property_name: name.into(),
            },
            value: value.to_string(),
            typed_expr: None,
            is_append,
        }
    }

    #[test]
    fn add_skips_empty_value() {
        let mut map = PropertyMap::new();
        map.add(make_prop(None, "Period", "", false));
        assert!(map.is_empty());
    }

    #[test]
    fn add_skips_whitespace_only_value() {
        let mut map = PropertyMap::new();
        map.add(make_prop(Some("Timing"), "Period", "   ", false));
        assert!(map.is_empty());
    }

    #[test]
    fn add_trims_whitespace() {
        let mut map = PropertyMap::new();
        map.add(make_prop(Some("Timing"), "Period", "  10 ms  ", false));
        assert_eq!(map.get("Timing", "Period"), Some("10 ms"));
    }

    #[test]
    fn add_skips_empty_append() {
        let mut map = PropertyMap::new();
        map.add(make_prop(None, "Ports", "p1", false));
        map.add(make_prop(None, "Ports", "", true));
        assert_eq!(map.get_all("", "Ports"), vec!["p1"]);
    }

    // ── Modal values (REQ-AADL-MODAL-PROPERTY-VALUES-001) ──────────────
    // (`idx` is the provenance section's helper below.)

    fn mv(modes: &[&str], value: &str) -> ModalValue {
        ModalValue {
            in_modes: modes.iter().map(|m| Name::new(m)).collect(),
            value: make_prop(None, "Period", value, false),
        }
    }

    /// `Period => 10 ms in modes (fast), 40 ms in modes (slow, Idle), 25 ms;`
    /// declared by component 7.
    fn period_binding() -> ModalBinding {
        ModalBinding {
            mode_context: idx(7),
            values: vec![
                mv(&["fast"], "10 ms"),
                mv(&["slow", "Idle"], "40 ms"),
                mv(&[], "25 ms"),
            ],
        }
    }

    fn period_in(b: &ModalBinding, mode: &str) -> Option<String> {
        b.value_in_mode(mode).map(|v| v.value.clone())
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn value_in_mode_follows_osate() {
        let b = period_binding();
        assert_eq!(period_in(&b, "fast").as_deref(), Some("10 ms"));
        // Any listed mode, case-insensitively.
        assert_eq!(period_in(&b, "slow").as_deref(), Some("40 ms"));
        assert_eq!(period_in(&b, "idle").as_deref(), Some("40 ms"));
        // A mode no value names: the non-modal value.
        assert_eq!(period_in(&b, "boost").as_deref(), Some("25 ms"));
        // The non-modal value may come FIRST: a later modal match still wins.
        let first = ModalBinding {
            mode_context: idx(7),
            values: vec![mv(&[], "25 ms"), mv(&["fast"], "10 ms")],
        };
        assert_eq!(period_in(&first, "fast").as_deref(), Some("10 ms"));
        assert_eq!(period_in(&first, "slow").as_deref(), Some("25 ms"));
        // No non-modal value: none in an unnamed mode.
        let no_default = ModalBinding {
            mode_context: idx(7),
            values: vec![mv(&["fast"], "10 ms")],
        };
        assert_eq!(period_in(&no_default, "slow"), None);
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn in_mode_reads_the_declarers_mode() {
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        map.add(make_prop(None, "Deadline", "5 ms", false));
        assert!(map.has_modal_values());
        // The non-modal reading is the first value.
        assert_eq!(map.get("", "Period"), Some("10 ms"));

        // Only the declarer's (component 7's) mode is consulted.
        let slow = map.in_mode(|c| (c == idx(7)).then_some("slow"));
        assert_eq!(slow.get("", "Period"), Some("40 ms"));
        assert_eq!(slow.get("", "Deadline"), Some("5 ms"));
        assert!(matches!(slow, Cow::Owned(_)));
        let other = map.in_mode(|c| (c == idx(7)).then_some("boost"));
        assert_eq!(other.get("", "Period"), Some("25 ms"));
        // The declarer has no selected mode: the non-modal reading.
        let none = map.in_mode(|c| (c == idx(8)).then_some("slow"));
        assert_eq!(none.get("", "Period"), Some("10 ms"));
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn a_mode_no_value_applies_in_has_no_value() {
        let mut map = PropertyMap::new();
        let binding = ModalBinding {
            mode_context: idx(7),
            values: vec![mv(&["fast"], "10 ms")],
        };
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            Some(binding),
            Some(idx(3)),
        );
        assert_eq!(map.reference_context("", "Period"), Some(idx(3)));
        let fast = map.in_mode(|_| Some("fast"));
        assert_eq!(fast.get("", "Period"), Some("10 ms"));
        assert_eq!(fast.reference_context("", "Period"), Some(idx(3)));
        let slow = map.in_mode(|_| Some("slow"));
        assert_eq!(slow.get("", "Period"), None);
        // The value's reference context goes with it.
        assert_eq!(slow.reference_context("", "Period"), None);
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn a_map_without_modal_values_is_borrowed_unchanged() {
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            None,
            None,
        );
        assert!(!map.has_modal_values());
        assert!(matches!(map.in_mode(|_| Some("slow")), Cow::Borrowed(_)));
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn a_later_association_replaces_the_modal_binding() {
        // A later assignment overrides the whole modal association.
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        map.add(make_prop(None, "Period", "99 ms", false));
        assert!(!map.has_modal_values());
        assert_eq!(
            map.in_mode(|_| Some("slow")).get("", "Period"),
            Some("99 ms")
        );

        // An append to a modal value is not modelled: the key goes non-modal.
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        map.add(make_prop(None, "Period", "1 ms", true));
        assert!(!map.has_modal_values());

        // A modal APPEND records no binding.
        let mut map = PropertyMap::new();
        map.add(make_prop(None, "Period", "10 ms", false));
        map.add_modal(
            make_prop(None, "Period", "1 ms", true),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        assert!(!map.has_modal_values());
        assert_eq!(map.get_all("", "Period"), vec!["10 ms", "1 ms"]);

        // An empty value records no binding either.
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "  ", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        assert!(!map.has_modal_values());
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn an_inherited_value_keeps_or_drops_its_modal_binding() {
        let key = PropertyMap::key_of(&PropertyRef {
            property_set: None,
            property_name: "Period".into(),
        });
        let mut map = PropertyMap::new();
        map.insert_inherited(
            key.clone(),
            vec![make_prop(None, "Period", "10 ms", false)],
            idx(1),
            idx(2),
            &[cls("T")],
            Some(period_binding()),
        );
        assert_eq!(map.modal_for_key(&key), Some(&period_binding()));
        // … and its provenance, beside the binding.
        assert_eq!(
            map.sources_for_key(&key),
            [PropertySource::Inherited {
                ancestor: idx(2),
                origin: Box::new(cls("T")),
            }]
        );
        assert_eq!(
            map.in_mode(|_| Some("slow")).get("", "Period"),
            Some("40 ms")
        );
        map.insert_inherited(
            key.clone(),
            vec![make_prop(None, "Period", "10 ms", false)],
            idx(1),
            idx(2),
            &[cls("T")],
            None,
        );
        assert_eq!(map.modal_for_key(&key), None);
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn a_modal_value_keeps_its_provenance_and_reference_context() {
        let mut map = PropertyMap::new();
        map.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Contained { declarer: idx(3) },
            Some(period_binding()),
            Some(idx(3)),
        );
        let k = key("", "Period");
        assert_eq!(
            map.sources_for_key(&k),
            [PropertySource::Contained { declarer: idx(3) }]
        );
        assert_eq!(map.reference_context_for_key(&k), Some(idx(3)));
        assert_eq!(map.modal_for_key(&k), Some(&period_binding()));
        // An empty value records neither a context nor a binding.
        let mut empty = PropertyMap::new();
        empty.add_modal(
            make_prop(None, "Period", " ", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            Some(idx(3)),
        );
        assert!(empty.is_empty());
        assert_eq!(empty.reference_context_for_key(&k), None);
        // A modal append keeps its context and source, but no binding.
        let mut append = PropertyMap::new();
        append.add_modal(
            make_prop(None, "Period", "1 ms", true),
            cls("A"),
            Some(period_binding()),
            Some(idx(3)),
        );
        assert_eq!(append.reference_context_for_key(&k), Some(idx(3)));
        assert_eq!(append.sources_for_key(&k), [cls("A")]);
        assert!(!append.has_modal_values());
    }

    // rivet: verifies REQ-AADL-MODAL-PROPERTY-VALUES-001
    #[test]
    fn a_modal_binding_takes_part_in_equality() {
        let mut a = PropertyMap::new();
        let mut b = PropertyMap::new();
        a.add(make_prop(None, "Period", "10 ms", false));
        b.add_modal(
            make_prop(None, "Period", "10 ms", false),
            PropertySource::Unrecorded,
            Some(period_binding()),
            None,
        );
        assert_ne!(a, b, "same first value, but b reads 40 ms in slow");
        a.add_modal(
            make_prop(None, "Period", "10 ms", false),
            cls("other source"),
            Some(period_binding()),
            None,
        );
        assert_eq!(a, b, "same values and bindings; provenance is ignored");
    }

    #[test]
    fn add_normal_value_works() {
        let mut map = PropertyMap::new();
        map.add(make_prop(Some("Timing"), "Period", "10 ms", false));
        assert_eq!(map.get("Timing", "Period"), Some("10 ms"));
        assert_eq!(map.len(), 1);
    }

    // ── provenance (REQ-AADL-PROPERTY-PROVENANCE-001) ───────────────────

    fn key(set: &str, name: &str) -> (CiName, CiName) {
        (CiName::from_str(set), CiName::from_str(name))
    }

    fn cls(c: &str) -> PropertySource {
        PropertySource::Classifier {
            classifier: c.to_string(),
        }
    }

    fn idx(n: u32) -> crate::instance::ComponentInstanceIdx {
        la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(n))
    }

    // rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
    #[test]
    fn sources_follow_override_and_append() {
        let mut map = PropertyMap::new();
        map.add_from(make_prop(None, "Ports", "p1", false), cls("A"));
        map.add_from(
            make_prop(None, "Ports", "p2", true),
            PropertySource::DeclaredLocal,
        );
        assert_eq!(
            map.sources_for_key(&key("", "Ports")),
            [cls("A"), PropertySource::DeclaredLocal],
            "an append adds its own source beside the existing one"
        );
        map.add_from(make_prop(None, "Ports", "p3", false), cls("B"));
        assert_eq!(
            map.sources_for_key(&key("", "Ports")),
            [cls("B")],
            "an override replaces every source with its own"
        );
        // An empty value records nothing, so the sources stay aligned.
        map.add_from(make_prop(None, "Ports", "  ", true), cls("C"));
        assert_eq!(map.sources_for_key(&key("", "Ports")), [cls("B")]);
        assert_eq!(map.sources_for_key(&key("", "Other")), []);
        // Plain `add` states no source.
        map.add(make_prop(None, "Other", "x", false));
        assert_eq!(
            map.sources_for_key(&key("", "Other")),
            [PropertySource::Unrecorded]
        );
    }

    // rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
    #[test]
    fn a_contained_value_records_its_declarer_and_an_inherited_one_its_ancestor() {
        let mut map = PropertyMap::new();
        map.add_with_reference_context(make_prop(None, "B", "reference (x)", false), idx(3));
        assert_eq!(
            map.sources_for_key(&key("", "B")),
            [PropertySource::Contained { declarer: idx(3) }]
        );
        map.insert_inherited(
            key("", "I"),
            vec![
                make_prop(None, "I", "a", false),
                make_prop(None, "I", "b", true),
            ],
            idx(1),
            idx(2),
            &[cls("T"), PropertySource::DeclaredLocal],
            None,
        );
        assert_eq!(
            map.sources_for_key(&key("", "I")),
            [
                PropertySource::Inherited {
                    ancestor: idx(2),
                    origin: Box::new(cls("T")),
                },
                PropertySource::Inherited {
                    ancestor: idx(2),
                    origin: Box::new(PropertySource::DeclaredLocal),
                },
            ]
        );
        assert_eq!(map.reference_context_for_key(&key("", "I")), Some(idx(1)));
    }

    // rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
    #[test]
    fn provenance_takes_no_part_in_equality() {
        let mut a = PropertyMap::new();
        let mut b = PropertyMap::new();
        a.add_from(make_prop(None, "P", "1", false), cls("A"));
        b.add_from(make_prop(None, "P", "1", false), cls("B"));
        assert_eq!(a, b, "the same values from different sources are equal");
        b.add(make_prop(None, "P", "2", false));
        assert_ne!(a, b, "different values are not");
        let mut c = PropertyMap::new();
        c.add_with_reference_context(make_prop(None, "P", "1", false), idx(0));
        assert_ne!(a, c, "a different reference context is not");
    }
}
