//! Binding match logic.
//!
//! Implements the "matches the item" half of the contract's
//! §"Binding resolution semantics" — given a HIR item described via
//! [`HasBindingIdentity`] and a [`Binding`], decide whether the binding
//! applies. The "kept iff requires ⊆ features for every match" half
//! lives in [`crate::filter`].

use crate::context::Binding;

/// Identity adapter that HIR items implement to participate in variant
/// filtering.
///
/// We keep this trait deliberately narrow — just enough to evaluate the
/// two binding shapes — so the bridge from spar-hir-def's
/// `ComponentInstance` (and friends) is a thin, mechanical mapping
/// rather than a deep dependency. The actual `impl HasBindingIdentity`
/// for spar-hir-def types lives outside this commit; tests use a local
/// stub.
pub trait HasBindingIdentity {
    /// Project-relative source-file path the item was declared in, or
    /// `None` if the item is synthetic (no source file). Returning
    /// `None` makes [`Binding::Artifact`] always non-match.
    fn artifact_path(&self) -> Option<&str>;

    /// Fully-qualified AADL name, shape `Package::Type` or
    /// `Package::Type.Implementation` (for top-level classifiers) or a
    /// dotted extension thereof for nested items
    /// (`Package::Type.Impl.subcomponent`). `None` for items with no
    /// resolvable FQN, in which case [`Binding::Symbol`] is always
    /// non-match.
    fn fully_qualified_symbol(&self) -> Option<String>;
}

/// Normalize an artifact-binding path or an item path to a canonical
/// form for comparison: drop leading `./`, fold any backslash separators
/// to forward-slash. We do **not** resolve `..` or absolutize — the
/// contract specifies "relative to the project root" on both sides, so
/// a textual normalization is sufficient. Future work: case-folding on
/// case-insensitive filesystems if the need arises.
fn normalize_path(p: &str) -> String {
    let trimmed = p.strip_prefix("./").unwrap_or(p);
    trimmed.replace('\\', "/")
}

impl Binding {
    /// True iff this binding's scope encompasses the given item, per
    /// §"Binding resolution semantics" of the v1 contract.
    ///
    /// For [`Binding::Symbol`], this commit implements prefix-matching:
    /// the binding matches if the item's FQN equals the symbol or
    /// starts with `<symbol>.` / `<symbol>::`. That covers the
    /// "subcomponents, connections, properties, modes, flow specs
    /// textually nested inside its body" clause of the contract.
    ///
    /// TODO(track-b-commit-2+): the contract's §"Symbol granularity"
    /// also notes inheritance is **orthogonal** to variant binding —
    /// classifiers that `extends` a bound symbol must NOT inherit the
    /// binding. Prefix-matching as implemented here cannot tell apart
    /// "nested in body" from "named under" if a project ever uses
    /// dot-separated FQNs that aren't true textual nesting (none today
    /// in spar-hir-def, so this is safe for commit 1). The eventual
    /// fix is to plumb a richer "contains" relation through the HIR
    /// adapter rather than relying on string-shape.
    pub fn matches<I: HasBindingIdentity + ?Sized>(&self, item: &I) -> bool {
        match self {
            Binding::Artifact { artifact, .. } => match item.artifact_path() {
                Some(item_path) => normalize_path(item_path) == normalize_path(artifact),
                None => false,
            },
            Binding::Symbol { symbol, .. } => match item.fully_qualified_symbol() {
                Some(fqn) => symbol_matches(&fqn, symbol),
                None => false,
            },
        }
    }

    /// The `requires` list, regardless of binding kind.
    pub fn requires(&self) -> &[String] {
        match self {
            Binding::Artifact { requires, .. } | Binding::Symbol { requires, .. } => requires,
        }
    }
}

/// Prefix-match an item FQN against a binding's symbol per the
/// "self-or-nested-in-body" rule. Boundary character must be `.` or
/// `::` so that `Engines::Engine.Diesel` does NOT match
/// `Engines::Engine.DieselV2`.
fn symbol_matches(item_fqn: &str, binding_symbol: &str) -> bool {
    if item_fqn == binding_symbol {
        return true;
    }
    let Some(rest) = item_fqn.strip_prefix(binding_symbol) else {
        return false;
    };
    rest.starts_with('.') || rest.starts_with("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test stub — the spar-hir-def adapter ships with the spar-cli
    /// commit (Track B commit 2), per design.
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

    fn artifact_binding(path: &str) -> Binding {
        Binding::Artifact {
            artifact: path.to_string(),
            requires: vec![],
        }
    }

    fn symbol_binding(sym: &str) -> Binding {
        Binding::Symbol {
            symbol: sym.to_string(),
            requires: vec![],
        }
    }

    #[test]
    fn artifact_binding_matches_exact_path() {
        let b = artifact_binding("spec/engines/diesel.aadl");
        let item = StubItem {
            path: Some("spec/engines/diesel.aadl".to_string()),
            fqn: None,
        };
        assert!(b.matches(&item));
    }

    #[test]
    fn artifact_binding_matches_after_dot_slash_normalization() {
        // Defensive: emitter or HIR could carry `./` prefix on either
        // side. Normalization should make both forms equivalent.
        let b = artifact_binding("./spec/engines/diesel.aadl");
        let item = StubItem {
            path: Some("spec/engines/diesel.aadl".to_string()),
            fqn: None,
        };
        assert!(b.matches(&item));
    }

    #[test]
    fn artifact_binding_doesnt_match_other_path() {
        let b = artifact_binding("spec/engines/diesel.aadl");
        let item = StubItem {
            path: Some("spec/engines/electric.aadl".to_string()),
            fqn: None,
        };
        assert!(!b.matches(&item));
    }

    #[test]
    fn artifact_binding_doesnt_match_when_item_has_no_path() {
        let b = artifact_binding("spec/engines/diesel.aadl");
        let item = StubItem {
            path: None,
            fqn: Some("Engines::Engine.Diesel".to_string()),
        };
        assert!(!b.matches(&item));
    }

    #[test]
    fn symbol_binding_matches_self() {
        let b = symbol_binding("Engines::Engine.Diesel");
        let item = StubItem {
            path: None,
            fqn: Some("Engines::Engine.Diesel".to_string()),
        };
        assert!(b.matches(&item));
    }

    #[test]
    fn symbol_binding_matches_nested() {
        // A subcomponent textually nested inside Engine.Diesel — e.g.
        // `cylinder1: device …` declared in the implementation body.
        // The HIR adapter is responsible for assembling this dotted
        // FQN; the binding matcher only sees strings.
        let b = symbol_binding("Engines::Engine.Diesel");
        let item = StubItem {
            path: None,
            fqn: Some("Engines::Engine.Diesel.cylinder1".to_string()),
        };
        assert!(b.matches(&item));
    }

    #[test]
    fn symbol_binding_doesnt_match_sibling_with_shared_prefix() {
        // Boundary check: `Diesel` MUST NOT match `DieselV2` just
        // because the latter starts with the former.
        let b = symbol_binding("Engines::Engine.Diesel");
        let item = StubItem {
            path: None,
            fqn: Some("Engines::Engine.DieselV2".to_string()),
        };
        assert!(!b.matches(&item));
    }
}
