//! HIR-filter predicate.
//!
//! Implements the "kept iff requires ⊆ features for every matching
//! binding" rule from §"Binding resolution semantics" of
//! `docs/contracts/rivet-spar-variant-v1.md`.
//!
//! Per §"Why intersection, not union", v1 is intersection-only —
//! multiple bindings that match the same item are treated conjunctively.
//! The conservative choice: a stricter binding can only ever drop an
//! item, never reintroduce it.

use std::collections::HashSet;

use crate::binding::HasBindingIdentity;
use crate::context::VariantContext;

/// Returns `true` if `item` should be kept under `context`.
///
/// Algorithm:
///
/// 1. Walk every binding in the context and collect the matchers (per
///    [`crate::binding::Binding::matches`]).
/// 2. If no binding matches, the item is variant-independent
///    infrastructure — keep it unconditionally.
/// 3. Otherwise, every matching binding's `requires` must be a subset
///    of `context.features`. A single unmet `requires` drops the item.
pub fn keep_in_variant<I: HasBindingIdentity + ?Sized>(item: &I, context: &VariantContext) -> bool {
    let features: HashSet<&str> = context.features.iter().map(String::as_str).collect();

    let mut matched_any = false;
    for binding in &context.bindings {
        if !binding.matches(item) {
            continue;
        }
        matched_any = true;
        for required in binding.requires() {
            if !features.contains(required.as_str()) {
                return false;
            }
        }
    }

    // Either no binding scoped this item (keep unconditionally) or all
    // matching bindings had their requires satisfied (keep).
    let _ = matched_any;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Binding;

    /// Test stub — see binding.rs for the rationale.
    struct StubItem {
        path: Option<String>,
        fqn: Option<String>,
    }

    impl HasBindingIdentity for StubItem {
        fn artifact_path(&self) -> Option<&str> {
            self.path.as_deref()
        }

        fn fully_qualified_symbol(&self) -> Option<String> {
            self.fqn.clone()
        }
    }

    fn ctx(features: &[&str], bindings: Vec<Binding>) -> VariantContext {
        VariantContext {
            rivet_spar_context_version: "1".to_string(),
            variant: "test".to_string(),
            features: features.iter().map(|s| s.to_string()).collect(),
            bindings,
            feature_model_hash: "sha256:test".to_string(),
            resolved_at: "2026-04-23T12:00:00Z".to_string(),
            generated_by: "spar-variants tests".to_string(),
        }
    }

    #[test]
    fn keep_unbound_item_unconditional() {
        // §"Binding resolution semantics" rule 4: zero matching
        // bindings means "variant-independent infrastructure" — kept
        // regardless of which features are or aren't active.
        let context = ctx(
            &[],
            vec![Binding::Artifact {
                artifact: "spec/other.aadl".to_string(),
                requires: vec!["never_active".to_string()],
            }],
        );
        let item = StubItem {
            path: Some("spec/infra.aadl".to_string()),
            fqn: Some("Infra::Bus".to_string()),
        };
        assert!(keep_in_variant(&item, &context));
    }

    #[test]
    fn keep_intersection_semantics() {
        // Two matching bindings: one's requires satisfied, the other's
        // not. Conjunctive semantics → drop. This is the v1
        // intersection rule's whole reason for existing — see
        // §"Why intersection, not union".
        let context = ctx(
            &["engine_diesel"],
            vec![
                Binding::Artifact {
                    artifact: "spec/engines/diesel.aadl".to_string(),
                    requires: vec!["engine_diesel".to_string()],
                },
                Binding::Symbol {
                    symbol: "Engines::Engine.Diesel".to_string(),
                    requires: vec!["emissions_eu5".to_string()],
                },
            ],
        );
        let item = StubItem {
            path: Some("spec/engines/diesel.aadl".to_string()),
            fqn: Some("Engines::Engine.Diesel".to_string()),
        };
        assert!(!keep_in_variant(&item, &context));
    }

    #[test]
    fn keep_when_all_matching_bindings_satisfied() {
        let context = ctx(
            &["engine_diesel", "emissions_eu5"],
            vec![
                Binding::Artifact {
                    artifact: "spec/engines/diesel.aadl".to_string(),
                    requires: vec!["engine_diesel".to_string()],
                },
                Binding::Symbol {
                    symbol: "Engines::Engine.Diesel".to_string(),
                    requires: vec!["emissions_eu5".to_string()],
                },
            ],
        );
        let item = StubItem {
            path: Some("spec/engines/diesel.aadl".to_string()),
            fqn: Some("Engines::Engine.Diesel".to_string()),
        };
        assert!(keep_in_variant(&item, &context));
    }

    #[test]
    fn drop_when_lone_binding_unsatisfied() {
        let context = ctx(
            &["engine_electric"],
            vec![Binding::Artifact {
                artifact: "spec/engines/diesel.aadl".to_string(),
                requires: vec!["engine_diesel".to_string()],
            }],
        );
        let item = StubItem {
            path: Some("spec/engines/diesel.aadl".to_string()),
            fqn: None,
        };
        assert!(!keep_in_variant(&item, &context));
    }

    #[test]
    fn empty_requires_is_satisfied_vacuously() {
        // Per the contract: `requires: []` means "no feature
        // requirement — equivalent to no binding at all". So a binding
        // that matches but requires nothing keeps the item.
        let context = ctx(
            &[],
            vec![Binding::Artifact {
                artifact: "spec/x.aadl".to_string(),
                requires: vec![],
            }],
        );
        let item = StubItem {
            path: Some("spec/x.aadl".to_string()),
            fqn: None,
        };
        assert!(keep_in_variant(&item, &context));
    }
}
