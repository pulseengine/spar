//! Tentative-binding overlay (Track E commit 2/8, v0.8.0).
//!
//! Provides [`BindingOverlay`] — a HIR-level immutable map from
//! [`ComponentInstanceIdx`] to a hypothetical target processor index —
//! so that any analysis can read "as if" the component's
//! `Actual_Processor_Binding` had been rebound, *without* mutating the
//! underlying [`SystemInstance`]. Per the Track E migration design
//! research §6.4.
//!
//! # Why an overlay instead of model mutation?
//!
//! AADL's `Actual_Processor_Binding` lives in the property map of each
//! component instance. Mutating it to test a hypothetical move would
//! invalidate every cached analysis result and force every downstream
//! consumer to either snapshot the model or guard against
//! mid-computation flips. The overlay sidesteps both problems:
//!
//! - **Read-side only.** Nothing in [`SystemInstance`] changes; analyses
//!   that opt in call [`actual_processor_binding_with_overlay`] which
//!   honours the overlay if present and otherwise falls through to the
//!   declared binding.
//! - **Cheap to compose.** `BindingOverlay` is a `Default`-able value
//!   type — a single `FxHashMap` of moves — so an enumerate / verify
//!   driver can spin up thousands of candidates without touching the
//!   parsed model.
//! - **Validates against the platform/application split.** The overlay
//!   knows about [`crate::migration::is_frozen`] from commit 1 and
//!   about the `Spar_Migration::Allowed_Targets` reference list, so
//!   constraint-layer rejections happen *before* an analysis pass even
//!   runs.
//!
//! The CLI surface (`spar moves verify`) lands in commit 3; the solver
//! integration in commits 4–5; the MCP surface in v0.9.0. This commit
//! delivers only the load-bearing in-process API.

use rustc_hash::FxHashMap;

use crate::instance::{ComponentInstanceIdx, SystemInstance};
use crate::item_tree::PropertyExpr;
use crate::migration::is_frozen;
use crate::properties::PropertyMap;

/// Property set name for the Track E migration vocabulary.
const SPAR_MIGRATION: &str = "Spar_Migration";

/// A hypothetical-binding overlay over a [`SystemInstance`].
///
/// Maps a component instance index to a hypothetical target processor
/// instance index. When an analysis routes its
/// `Actual_Processor_Binding` lookup through
/// [`actual_processor_binding_with_overlay`], the overlay is consulted
/// first; if it carries a move for the queried component, that
/// hypothetical target is returned and the declared binding is
/// ignored. Components not present in the overlay fall through to the
/// declared binding unchanged.
///
/// # Scope (v0.8.0)
///
/// - **Processor binding only.** Memory and connection bindings are
///   deliberately out of scope for v0.8.0; they may join the overlay
///   in v0.9.0 once the processor pipeline is shaken out.
/// - **Single-variant.** Variant-aware moves (rivet variants v1, #144)
///   compose at the variant-resolution layer, not inside the overlay.
/// - **No mutation.** The overlay holds intent, not state. A separate
///   commit (`spar moves apply`) is the only way to write back into
///   the model.
#[derive(Debug, Clone, Default)]
pub struct BindingOverlay {
    /// Per-component hypothetical processor binding.
    ///
    /// Key: the component being moved (typically a `process` or
    /// `thread`). Value: the target processor instance.
    pub moves: FxHashMap<ComponentInstanceIdx, ComponentInstanceIdx>,
}

/// Diagnostic emitted when an overlay tries to move a frozen component.
///
/// Per `Spar_Migration::Frozen` semantics (Track E commit 1, REQ-MIGRATION-001),
/// a frozen component's binding may not change in any hypothetical
/// move. This violation is structured (not a string) so callers in
/// commit 3 (`spar moves verify`) can surface it as JSON without
/// re-parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenViolation {
    /// The component the overlay attempted to move.
    pub component: ComponentInstanceIdx,
    /// Human-readable reason — typically the value of
    /// `Spar_Migration::Pinned_Reason`, or a default if unset.
    pub reason: String,
}

/// Diagnostic emitted when an overlay moves a component to a target
/// not present in its `Spar_Migration::Allowed_Targets` reference list.
///
/// An empty `Allowed_Targets` list (or absent property) means "no
/// restriction" per §6.1 of the Track E design research.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedTargetsViolation {
    /// The component the overlay is moving.
    pub component: ComponentInstanceIdx,
    /// The hypothetical target the overlay proposed.
    pub target: ComponentInstanceIdx,
    /// The set of allowed targets declared on the component
    /// (resolved to indices when found in the instance hierarchy).
    pub allowed: Vec<ComponentInstanceIdx>,
}

/// Union of overlay diagnostics returned by [`BindingOverlay::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayDiagnostic {
    /// The overlay tried to move a frozen component.
    Frozen(FrozenViolation),
    /// The overlay's target is not in the component's `Allowed_Targets`.
    AllowedTargets(AllowedTargetsViolation),
}

impl BindingOverlay {
    /// Construct an empty overlay (no hypothetical moves).
    ///
    /// An empty overlay is the identity: every property lookup falls
    /// through to the declared binding, and [`Self::validate`] returns
    /// no diagnostics. This is the non-regression baseline — analyses
    /// that take an `Option<&BindingOverlay>` see the same behaviour
    /// for `None` and `Some(&BindingOverlay::new())`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a hypothetical move: `comp` is to be re-bound to `target`.
    ///
    /// If `comp` is already in the overlay, the previous target is
    /// replaced. The overlay does *not* validate at insert time —
    /// frozen / allowed-targets checks are a separate pass via
    /// [`Self::validate`] or the per-rule helpers below — so callers
    /// can stage a complete plan before deciding whether to surface
    /// any violations.
    pub fn add_move(&mut self, comp: ComponentInstanceIdx, target: ComponentInstanceIdx) {
        self.moves.insert(comp, target);
    }

    /// Number of hypothetical moves currently staged.
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    /// Returns `true` if no moves are staged.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// Look up the overlay's hypothetical target for a component.
    ///
    /// Returns `None` if the overlay does not carry a move for
    /// `comp` — distinct from "the declared binding is None", which
    /// is the caller's concern.
    pub fn target_for(&self, comp: ComponentInstanceIdx) -> Option<ComponentInstanceIdx> {
        self.moves.get(&comp).copied()
    }

    /// Validate every move in the overlay against the platform /
    /// application split declared on the corresponding components.
    ///
    /// Returns the union of [`FrozenViolation`] (overlay touches a
    /// component with `Spar_Migration::Frozen => true`) and
    /// [`AllowedTargetsViolation`] (target not in the component's
    /// `Allowed_Targets` reference list, when that list is non-empty).
    ///
    /// An empty overlay returns an empty vector (non-regression
    /// baseline).
    pub fn validate(&self, instance: &SystemInstance) -> Vec<OverlayDiagnostic> {
        let mut diags = Vec::new();
        for v in self.is_frozen_violation(instance) {
            diags.push(OverlayDiagnostic::Frozen(v));
        }
        for v in self.allowed_targets_violation(instance) {
            diags.push(OverlayDiagnostic::AllowedTargets(v));
        }
        diags
    }

    /// Subset of [`Self::validate`]: only frozen-component violations.
    ///
    /// Returns one [`FrozenViolation`] per move that touches a frozen
    /// component. The reason is sourced from
    /// `Spar_Migration::Pinned_Reason` when set, else a generic
    /// "component is marked Spar_Migration::Frozen" string.
    pub fn is_frozen_violation(&self, instance: &SystemInstance) -> Vec<FrozenViolation> {
        let mut out = Vec::new();
        for &comp in self.moves.keys() {
            let props = instance.properties_for(comp);
            if is_frozen(props) {
                out.push(FrozenViolation {
                    component: comp,
                    reason: pinned_reason(props).unwrap_or_else(|| {
                        "component is marked Spar_Migration::Frozen".to_string()
                    }),
                });
            }
        }
        out
    }

    /// Subset of [`Self::validate`]: only allowed-targets violations.
    ///
    /// For each move whose component declares a non-empty
    /// `Spar_Migration::Allowed_Targets` reference list, checks that
    /// the proposed target's name appears in that list. An empty (or
    /// absent) list is treated as "no restriction" per §6.1 of the
    /// design research.
    pub fn allowed_targets_violation(
        &self,
        instance: &SystemInstance,
    ) -> Vec<AllowedTargetsViolation> {
        let mut out = Vec::new();
        for (&comp, &target) in &self.moves {
            let props = instance.properties_for(comp);
            let allowed_names = read_allowed_targets(props);
            if allowed_names.is_empty() {
                continue; // empty list means "no restriction"
            }
            let target_name = instance.component(target).name.as_str();
            let target_in_allowed = allowed_names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(target_name));
            if !target_in_allowed {
                let allowed_idx: Vec<ComponentInstanceIdx> = allowed_names
                    .iter()
                    .filter_map(|n| find_component_by_name(instance, n))
                    .collect();
                out.push(AllowedTargetsViolation {
                    component: comp,
                    target,
                    allowed: allowed_idx,
                });
            }
        }
        out
    }
}

/// Resolve the effective `Actual_Processor_Binding` for a component,
/// honouring an optional [`BindingOverlay`].
///
/// Lookup order:
///
/// 1. If `overlay` is `Some` and carries a move for `comp`, return the
///    overlay's hypothetical target index. The declared binding is
///    *not* consulted in this case.
/// 2. Otherwise, read the declared `Actual_Processor_Binding` from
///    the component's property map. The reference target is matched
///    case-insensitively against component names anywhere in the
///    instance hierarchy (matching the `find_component_by_name`
///    pattern already used in `spar-analysis::arinc653`).
/// 3. Returns `None` when no binding is declared and the overlay is
///    silent for this component, or when the declared binding's
///    target name does not resolve to a known component.
///
/// This is the load-bearing accessor for the overlay surface. Commit
/// 3 (`spar moves verify`) and commits 4–5 (`spar moves enumerate`)
/// route every analysis lookup through this function so that an empty
/// overlay is a strict no-op.
pub fn actual_processor_binding_with_overlay(
    instance: &SystemInstance,
    comp: ComponentInstanceIdx,
    overlay: Option<&BindingOverlay>,
) -> Option<ComponentInstanceIdx> {
    if let Some(o) = overlay
        && let Some(target) = o.target_for(comp)
    {
        return Some(target);
    }
    let props = instance.properties_for(comp);
    let target_name = read_actual_processor_binding_name(props)?;
    find_component_by_name(instance, &target_name)
}

// ── helpers (private) ────────────────────────────────────────────────

/// Read `Spar_Migration::Pinned_Reason` as a string, if set.
fn pinned_reason(props: &PropertyMap) -> Option<String> {
    if let Some(PropertyExpr::StringLit(s)) = props.get_typed(SPAR_MIGRATION, "Pinned_Reason") {
        return Some(s.clone());
    }
    let raw = props.get(SPAR_MIGRATION, "Pinned_Reason")?;
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read `Spar_Migration::Allowed_Targets` as a list of reference
/// target names.
///
/// Honours both the typed path (`PropertyExpr::List` of
/// `PropertyExpr::ReferenceValue`) and the raw-string fallback
/// (`(reference (a), reference (b))`-style text). An absent or empty
/// property returns an empty vector.
fn read_allowed_targets(props: &PropertyMap) -> Vec<String> {
    // Typed path: a list of ReferenceValue items.
    if let Some(expr) = props.get_typed(SPAR_MIGRATION, "Allowed_Targets") {
        let mut out = Vec::new();
        collect_reference_names(expr, &mut out);
        if !out.is_empty() {
            return out;
        }
    }
    // String fallback: parse `reference (X)` substrings out of the raw value.
    let Some(raw) = props.get(SPAR_MIGRATION, "Allowed_Targets") else {
        return Vec::new();
    };
    parse_reference_list(raw)
}

/// Recursively pluck `ReferenceValue` payloads from a typed expression.
fn collect_reference_names(expr: &PropertyExpr, out: &mut Vec<String>) {
    match expr {
        PropertyExpr::ReferenceValue(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        PropertyExpr::List(items) => {
            for item in items {
                collect_reference_names(item, out);
            }
        }
        _ => {}
    }
}

/// Parse a raw string of `reference (X)` items into a vector of names.
///
/// Tolerates surrounding parens and comma separators; mirrors the
/// permissive parsing in `spar-analysis::property_accessors`.
fn parse_reference_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(idx) = rest.find("reference") {
        rest = &rest[idx + "reference".len()..];
        let Some(open) = rest.find('(') else { break };
        let Some(close_rel) = rest[open + 1..].find(')') else {
            break;
        };
        let inner = rest[open + 1..open + 1 + close_rel].trim();
        if !inner.is_empty() {
            out.push(inner.to_string());
        }
        rest = &rest[open + 1 + close_rel + 1..];
    }
    out
}

/// Read the `Actual_Processor_Binding` reference target name.
///
/// Mirrors `spar-analysis::property_accessors::get_processor_binding`
/// but inlined here to avoid an upstream dependency from spar-hir-def
/// onto spar-analysis. Honours the typed path first, raw-string
/// fallback second.
fn read_actual_processor_binding_name(props: &PropertyMap) -> Option<String> {
    if let Some(expr) = props.get_typed("Deployment_Properties", "Actual_Processor_Binding") {
        let mut names = Vec::new();
        collect_reference_names(expr, &mut names);
        if let Some(first) = names.into_iter().next() {
            return Some(first);
        }
    }
    let raw = props
        .get("Deployment_Properties", "Actual_Processor_Binding")
        .or_else(|| props.get("", "Actual_Processor_Binding"))?;
    parse_reference_list(raw).into_iter().next()
}

/// Find a component by name anywhere in the instance hierarchy
/// (case-insensitive). First match wins; ties are unlikely in
/// well-formed AADL because instance paths disambiguate.
fn find_component_by_name(instance: &SystemInstance, name: &str) -> Option<ComponentInstanceIdx> {
    instance
        .all_components()
        .find(|(_, c)| c.name.as_str().eq_ignore_ascii_case(name))
        .map(|(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::ComponentInstance;
    use crate::item_tree::ComponentCategory;
    use crate::name::{Name, PropertyRef};
    use crate::properties::PropertyValue;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;

    /// Build a minimal SystemInstance with a root system and a list of
    /// child components with given names + categories. Returns the
    /// instance plus the indices of `root` and each child in order.
    fn make_instance(
        children: &[(&str, ComponentCategory)],
    ) -> (SystemInstance, Vec<ComponentInstanceIdx>) {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let root = components.alloc(ComponentInstance {
            name: Name::new("root"),
            category: ComponentCategory::System,
            type_name: Name::new("Root"),
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
        let mut child_idx = Vec::new();
        for (name, cat) in children {
            let idx = components.alloc(ComponentInstance {
                name: Name::new(name),
                category: *cat,
                type_name: Name::new("T"),
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
            child_idx.push(idx);
        }
        components[root].children = child_idx.clone();
        let instance = SystemInstance {
            root,
            components,
            features: Arena::default(),
            connections: Arena::default(),
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };
        let mut all = vec![root];
        all.extend(child_idx);
        (instance, all)
    }

    /// Set a typed property on a component instance.
    fn set_typed(
        instance: &mut SystemInstance,
        comp: ComponentInstanceIdx,
        set: &str,
        name: &str,
        value: &str,
        expr: PropertyExpr,
    ) {
        let map = instance.property_maps.entry(comp).or_default();
        map.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new(set)),
                property_name: Name::new(name),
            },
            value: value.to_string(),
            typed_expr: Some(expr),
            is_append: false,
        });
    }

    /// Convenience: declare `Actual_Processor_Binding => reference (X)`
    /// on a component via typed expression.
    fn set_actual_processor_binding(
        instance: &mut SystemInstance,
        comp: ComponentInstanceIdx,
        target_name: &str,
    ) {
        set_typed(
            instance,
            comp,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            &format!("reference ({target_name})"),
            PropertyExpr::ReferenceValue(target_name.to_string()),
        );
    }

    /// Set `Spar_Migration::Frozen => true` on a component.
    fn set_frozen(instance: &mut SystemInstance, comp: ComponentInstanceIdx) {
        set_typed(
            instance,
            comp,
            "Spar_Migration",
            "Frozen",
            "true",
            PropertyExpr::Boolean(true),
        );
    }

    /// Set `Spar_Migration::Allowed_Targets => (reference (a), reference (b), …)`
    /// via a typed list expression.
    fn set_allowed_targets(
        instance: &mut SystemInstance,
        comp: ComponentInstanceIdx,
        targets: &[&str],
    ) {
        let exprs: Vec<PropertyExpr> = targets
            .iter()
            .map(|t| PropertyExpr::ReferenceValue((*t).to_string()))
            .collect();
        let raw = targets
            .iter()
            .map(|t| format!("reference ({t})"))
            .collect::<Vec<_>>()
            .join(", ");
        set_typed(
            instance,
            comp,
            "Spar_Migration",
            "Allowed_Targets",
            &format!("({raw})"),
            PropertyExpr::List(exprs),
        );
    }

    // ── 1. empty_overlay_matches_declared ────────────────────────────

    #[test]
    fn empty_overlay_matches_declared() {
        // An empty overlay must be a strict no-op: lookup with
        // `None` overlay returns the same idx as lookup with
        // `Some(&BindingOverlay::new())`.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
        ]);
        let (t1, cpu1) = (idxs[1], idxs[2]);
        set_actual_processor_binding(&mut instance, t1, "cpu1");

        let declared = actual_processor_binding_with_overlay(&instance, t1, None);
        assert_eq!(declared, Some(cpu1));

        let overlay = BindingOverlay::new();
        let through_empty = actual_processor_binding_with_overlay(&instance, t1, Some(&overlay));
        assert_eq!(through_empty, Some(cpu1));
        assert_eq!(through_empty, declared);
    }

    // ── 2. single_move_returns_overlay_target ────────────────────────

    #[test]
    fn single_move_returns_overlay_target() {
        // After `add_move(t1 -> cpu2)`, lookup must return cpu2
        // regardless of what the declared binding says.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);
        set_actual_processor_binding(&mut instance, t1, "cpu1");

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let result = actual_processor_binding_with_overlay(&instance, t1, Some(&overlay));
        assert_eq!(result, Some(cpu2));
    }

    // ── 3. unrelated_component_unaffected ────────────────────────────

    #[test]
    fn unrelated_component_unaffected() {
        // An overlay carrying a move for t1 must not affect t2's lookup.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("t2", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, t2, cpu1, cpu2) = (idxs[1], idxs[2], idxs[3], idxs[4]);
        set_actual_processor_binding(&mut instance, t1, "cpu1");
        set_actual_processor_binding(&mut instance, t2, "cpu1");

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let t1_eff = actual_processor_binding_with_overlay(&instance, t1, Some(&overlay));
        let t2_eff = actual_processor_binding_with_overlay(&instance, t2, Some(&overlay));
        assert_eq!(t1_eff, Some(cpu2), "t1 follows the overlay");
        assert_eq!(t2_eff, Some(cpu1), "t2 keeps its declared binding");
    }

    // ── 4. frozen_component_violation ────────────────────────────────

    #[test]
    fn frozen_component_violation() {
        // Overlay tries to move a frozen component → FrozenViolation.
        let (mut instance, idxs) = make_instance(&[
            ("plat_thread", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (plat_thread, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);
        set_frozen(&mut instance, plat_thread);
        // Add a Pinned_Reason so the structured violation carries it through.
        set_typed(
            &mut instance,
            plat_thread,
            "Spar_Migration",
            "Pinned_Reason",
            "ASIL-D platform partition",
            PropertyExpr::StringLit("ASIL-D platform partition".to_string()),
        );

        let mut overlay = BindingOverlay::new();
        overlay.add_move(plat_thread, cpu2);

        let diags = overlay.validate(&instance);
        assert_eq!(diags.len(), 1);
        match &diags[0] {
            OverlayDiagnostic::Frozen(v) => {
                assert_eq!(v.component, plat_thread);
                assert_eq!(v.reason, "ASIL-D platform partition");
            }
            other => panic!("expected FrozenViolation, got {other:?}"),
        }

        // The dedicated subset accessor returns the same single violation.
        let frozen_only = overlay.is_frozen_violation(&instance);
        assert_eq!(frozen_only.len(), 1);
        assert_eq!(frozen_only[0].component, plat_thread);
    }

    // ── 5. non_frozen_component_no_violation ─────────────────────────

    #[test]
    fn non_frozen_component_no_violation() {
        // Overlay moves a non-frozen component → no FrozenViolation.
        let (instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);
        // No Frozen property set on t1.

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let frozen = overlay.is_frozen_violation(&instance);
        assert!(
            frozen.is_empty(),
            "non-frozen components must not raise FrozenViolation"
        );

        let diags = overlay.validate(&instance);
        assert!(
            !diags
                .iter()
                .any(|d| matches!(d, OverlayDiagnostic::Frozen(_))),
            "validate() must agree with is_frozen_violation()",
        );
    }

    // ── 6. allowed_targets_satisfied ─────────────────────────────────

    #[test]
    fn allowed_targets_satisfied() {
        // Allowed_Targets includes the overlay's target → no violation.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);
        set_allowed_targets(&mut instance, t1, &["cpu1", "cpu2"]);

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let viols = overlay.allowed_targets_violation(&instance);
        assert!(
            viols.is_empty(),
            "cpu2 is in Allowed_Targets, expected no violation, got {viols:?}",
        );
    }

    // ── 7. allowed_targets_violated ──────────────────────────────────

    #[test]
    fn allowed_targets_violated() {
        // Allowed_Targets does NOT include the overlay's target → violation.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
            ("cpu3", ComponentCategory::Processor),
        ]);
        let (t1, cpu1, _cpu2, cpu3) = (idxs[1], idxs[2], idxs[3], idxs[4]);
        // Allow only cpu1 — but the overlay aims at cpu3.
        set_allowed_targets(&mut instance, t1, &["cpu1"]);

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu3);

        let viols = overlay.allowed_targets_violation(&instance);
        assert_eq!(viols.len(), 1);
        let v = &viols[0];
        assert_eq!(v.component, t1);
        assert_eq!(v.target, cpu3);
        assert_eq!(v.allowed, vec![cpu1]);
    }

    // ── 8. empty_allowed_targets_means_no_restriction ────────────────

    #[test]
    fn empty_allowed_targets_means_no_restriction() {
        // An empty (or absent) Allowed_Targets means "anywhere is OK".
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);
        // Explicit empty list. Per PropertyMap::add, blank values are
        // dropped, so we add an empty typed List (which read_allowed_targets
        // treats as "no restriction").
        set_typed(
            &mut instance,
            t1,
            "Spar_Migration",
            "Allowed_Targets",
            "()",
            PropertyExpr::List(Vec::new()),
        );

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let viols = overlay.allowed_targets_violation(&instance);
        assert!(
            viols.is_empty(),
            "empty Allowed_Targets must be treated as 'no restriction'",
        );

        // And again with the property entirely absent.
        let (instance2, idxs2) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1b, _, cpu2b) = (idxs2[1], idxs2[2], idxs2[3]);
        let mut overlay2 = BindingOverlay::new();
        overlay2.add_move(t1b, cpu2b);
        // No Allowed_Targets at all — also no restriction.
        let viols2 = overlay2.allowed_targets_violation(&instance2);
        assert!(
            viols2.is_empty(),
            "absent Allowed_Targets must be 'no restriction'"
        );
    }

    // ── 9. multiple_moves_independent ────────────────────────────────

    #[test]
    fn multiple_moves_independent() {
        // Three moves staged together → each is evaluated against its
        // own component's properties; one's violation does not poison
        // the others.
        let (mut instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread), // mobile
            ("t2", ComponentCategory::Thread), // frozen
            ("t3", ComponentCategory::Thread), // mobile, restricted
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
            ("cpu3", ComponentCategory::Processor),
        ]);
        let (t1, t2, t3, _cpu1, cpu2, cpu3) =
            (idxs[1], idxs[2], idxs[3], idxs[4], idxs[5], idxs[6]);
        // t2 is frozen.
        set_frozen(&mut instance, t2);
        // t3 is restricted to cpu2 only — so a move to cpu3 is illegal.
        set_allowed_targets(&mut instance, t3, &["cpu2"]);

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2); // OK
        overlay.add_move(t2, cpu3); // Frozen violation
        overlay.add_move(t3, cpu3); // Allowed_Targets violation

        let frozen = overlay.is_frozen_violation(&instance);
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].component, t2);

        let allowed = overlay.allowed_targets_violation(&instance);
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].component, t3);
        assert_eq!(allowed[0].target, cpu3);

        // t1 stays clean across both validators.
        assert!(!frozen.iter().any(|v| v.component == t1));
        assert!(!allowed.iter().any(|v| v.component == t1));

        // validate() returns the union, exactly two diagnostics.
        let union = overlay.validate(&instance);
        assert_eq!(union.len(), 2);
    }

    // ── 10. overlay_clone_preserves_state ────────────────────────────

    #[test]
    fn overlay_clone_preserves_state() {
        // Clone must produce an independent overlay with the same
        // moves; mutating the clone must not affect the original.
        let (instance, idxs) = make_instance(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, _cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);
        assert_eq!(overlay.len(), 1);

        let cloned = overlay.clone();
        assert_eq!(cloned.len(), 1);
        assert_eq!(cloned.target_for(t1), Some(cpu2));

        // Mutate the clone and confirm the original is untouched.
        let mut cloned_mut = cloned.clone();
        cloned_mut.moves.clear();
        assert!(cloned_mut.is_empty());
        assert_eq!(
            overlay.len(),
            1,
            "original must not change after clone mutation"
        );
        assert_eq!(overlay.target_for(t1), Some(cpu2));

        // Lookup through the cloned overlay sees the same effective binding.
        let through_clone = actual_processor_binding_with_overlay(&instance, t1, Some(&cloned));
        assert_eq!(through_clone, Some(cpu2));
    }
}
