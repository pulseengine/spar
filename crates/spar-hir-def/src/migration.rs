//! Spar_Migration property helpers (Track E commit 1/8, v0.8.0).
//!
//! Provides the thin boolean accessors over the `Spar_Migration::Frozen`
//! and `Spar_Migration::Mobile` properties registered in
//! [`standard_properties`](crate::standard_properties).
//!
//! These helpers are intentionally minimal — no salsa query, no
//! per-instance enum cache yet. They read the property at lookup time
//! using the same typed-first / string-fallback pattern as
//! `spar-analysis::property_accessors`. The HIR-level cache lands in
//! Track E commit 2 once the hypothetical-binding overlay needs it.
//! Per the design research §6.2 tradeoff analysis: minimal surface
//! now, expand on demand.

use crate::item_tree::PropertyExpr;
use crate::properties::PropertyMap;

/// The `Spar_Migration` property set name.
const SPAR_MIGRATION: &str = "Spar_Migration";

/// Return `true` if `Spar_Migration::Frozen` is set on this property map.
///
/// Returns `false` if the property is unset, set to `false`, or set to a
/// non-boolean value. Defaults to `false` so unannotated components are
/// treated as eligible for hypothetical rebinding (i.e., the overlay
/// only rejects moves that touch components which were *explicitly*
/// declared platform).
///
/// Reads via [`PropertyMap::get_typed`] first (preferred — exact
/// boolean), falling back to [`PropertyMap::get`] with case-insensitive
/// "true"/"false" parsing for compatibility with raw-string properties
/// (e.g., assertion fixtures that pre-date typed lowering).
pub fn is_frozen(props: &PropertyMap) -> bool {
    read_bool_property(props, "Frozen").unwrap_or(false)
}

/// Return `true` if `Spar_Migration::Mobile` is set on this property map
/// **and** `Frozen` is *not* set.
///
/// When both `Frozen=true` and `Mobile=true` are declared on the same
/// item the relation is contradictory; this helper resolves it
/// defensively in favour of `Frozen` (refuse to rebind) rather than
/// `Mobile` (allow rebind). Per §6.1 of the design research: "mutually
/// inconsistent with `Frozen=true`".
///
/// Returns `false` if `Mobile` is unset, set to `false`, set to a
/// non-boolean value, or if `Frozen=true` overrides it.
pub fn is_mobile(props: &PropertyMap) -> bool {
    if is_frozen(props) {
        return false;
    }
    read_bool_property(props, "Mobile").unwrap_or(false)
}

/// Read a `Spar_Migration::<name>` boolean property.
///
/// Returns `Some(true)` / `Some(false)` if the property is present and
/// parses as a boolean; returns `None` if the property is absent or has
/// a non-boolean value.
fn read_bool_property(props: &PropertyMap, name: &str) -> Option<bool> {
    // Typed path: prefer the structured PropertyExpr.
    if let Some(expr) = props.get_typed(SPAR_MIGRATION, name)
        && let PropertyExpr::Boolean(b) = expr
    {
        return Some(*b);
    }

    // String fallback: tolerate raw "true"/"false" (case-insensitive).
    let raw = props.get(SPAR_MIGRATION, name)?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{Name, PropertyRef};
    use crate::properties::PropertyValue;

    /// Build a PropertyMap with a typed expression value.
    fn make_typed_props(set: &str, name: &str, value: &str, expr: PropertyExpr) -> PropertyMap {
        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: if set.is_empty() {
                    None
                } else {
                    Some(Name::new(set))
                },
                property_name: Name::new(name),
            },
            value: value.to_string(),
            typed_expr: Some(expr),
            is_append: false,
        });
        props
    }

    /// Build a PropertyMap with only a raw string value (no typed expr).
    fn make_string_props(set: &str, name: &str, value: &str) -> PropertyMap {
        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: if set.is_empty() {
                    None
                } else {
                    Some(Name::new(set))
                },
                property_name: Name::new(name),
            },
            value: value.to_string(),
            typed_expr: None,
            is_append: false,
        });
        props
    }

    // ── is_frozen ─────────────────────────────────────────────────

    #[test]
    fn test_is_frozen_default_false() {
        // Unannotated property map: not frozen.
        let props = PropertyMap::new();
        assert!(!is_frozen(&props));
    }

    #[test]
    fn test_is_frozen_when_property_set() {
        // Spar_Migration::Frozen => true (typed) → frozen.
        let props = make_typed_props(
            "Spar_Migration",
            "Frozen",
            "true",
            PropertyExpr::Boolean(true),
        );
        assert!(is_frozen(&props));

        // Spar_Migration::Frozen => false (typed) → not frozen.
        let props = make_typed_props(
            "Spar_Migration",
            "Frozen",
            "false",
            PropertyExpr::Boolean(false),
        );
        assert!(!is_frozen(&props));

        // Raw-string fallback (no typed expr) is also accepted.
        let props = make_string_props("Spar_Migration", "Frozen", "true");
        assert!(is_frozen(&props));
        let props = make_string_props("Spar_Migration", "Frozen", "FALSE");
        assert!(!is_frozen(&props));
    }

    #[test]
    fn test_is_frozen_non_boolean_value_treated_as_false() {
        // Defensive: a malformed value must not be silently treated as
        // "frozen" (which is the safe-against-rebinding side); the
        // analysis's job is to flag malformed properties via the type
        // checker, not for this helper to invent a result.
        let props = make_string_props("Spar_Migration", "Frozen", "yes-please");
        assert!(!is_frozen(&props));
    }

    // ── is_mobile ─────────────────────────────────────────────────

    #[test]
    fn test_is_mobile_default_false() {
        // Unannotated property map: not mobile.
        let props = PropertyMap::new();
        assert!(!is_mobile(&props));
    }

    #[test]
    fn test_is_mobile_when_property_set() {
        // Spar_Migration::Mobile => true (typed) → mobile.
        let props = make_typed_props(
            "Spar_Migration",
            "Mobile",
            "true",
            PropertyExpr::Boolean(true),
        );
        assert!(is_mobile(&props));

        // Spar_Migration::Mobile => false (typed) → not mobile.
        let props = make_typed_props(
            "Spar_Migration",
            "Mobile",
            "false",
            PropertyExpr::Boolean(false),
        );
        assert!(!is_mobile(&props));

        // Raw-string fallback also accepted.
        let props = make_string_props("Spar_Migration", "Mobile", "True");
        assert!(is_mobile(&props));
    }

    #[test]
    fn test_is_mobile_loses_to_frozen_when_both_set() {
        // Defensive: if a model declares both Frozen=true and
        // Mobile=true, prefer Frozen — refuse to rebind. Per §6.1.
        let mut props = PropertyMap::new();
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Spar_Migration")),
                property_name: Name::new("Frozen"),
            },
            value: "true".to_string(),
            typed_expr: Some(PropertyExpr::Boolean(true)),
            is_append: false,
        });
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Spar_Migration")),
                property_name: Name::new("Mobile"),
            },
            value: "true".to_string(),
            typed_expr: Some(PropertyExpr::Boolean(true)),
            is_append: false,
        });

        assert!(is_frozen(&props), "Frozen=true must be honoured");
        assert!(
            !is_mobile(&props),
            "Mobile must lose to Frozen when both are set",
        );
    }
}
