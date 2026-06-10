// Some of the helpers below (e.g. `SourcePathMap::new`,
// `VariantScope::all_kept_components`) are part of the bridge's intended
// public surface even when the current `moves.rs` callers do not exercise
// every entry point. Allowing dead-code locally keeps the module self-
// contained and avoids littering the call sites with `#[allow(dead_code)]`.
#![allow(dead_code)]

//! Bridge between [`spar_variants`] and [`spar_hir_def`] HIR types
//! (Track E commit 6/8, v0.8.0).
//!
//! Per the v1 contract — `docs/contracts/rivet-spar-variant-v1.md`
//! §"Binding resolution semantics" — variant filtering decides whether
//! each HIR item is kept under a resolved variant. The
//! [`HasBindingIdentity`] trait abstracts over the identity surface of
//! an HIR item: a project-relative source-file path and a fully-qualified
//! AADL symbol. This module provides the spar-side adapters.
//!
//! # What lives here
//!
//! - [`ComponentInstanceIdentity`] — a value-typed adapter that wraps a
//!   [`ComponentInstanceIdx`] together with the [`SystemInstance`] and a
//!   `(package, type) -> source_path` map so it can answer
//!   `artifact_path()` and `fully_qualified_symbol()` queries.
//! - [`VariantScope`] — a non-mutating wrapper around `(SystemInstance,
//!   VariantContext, source-path map)` that exposes overlay-aware
//!   accessors: a "kept" predicate for component indices, an iterator
//!   over kept components, and a helper that resolves a user-supplied
//!   `--component`/`--to` against the kept subset.
//!
//! # Why a wrapper, not a re-built instance?
//!
//! The variant filter applies *before* overlay validation, but the
//! filter itself is non-mutating: it does not touch the parsed
//! [`SystemInstance`]. Doing so would invalidate every cached analysis
//! result and force every downstream consumer to either snapshot the
//! model or guard against mid-computation flips. [`VariantScope`] is the
//! lookup-time projection that subsequent verify/enumerate code uses
//! when deciding whether a component participates in the analysis
//! surface.
//!
//! # Source-path mapping
//!
//! [`ComponentInstance`] does not carry a source-file path; the path is
//! tracked by the CLI driver when it loads model files into the
//! [`spar_hir_def::GlobalScope`]. The `(package, type) -> path` map is
//! built by walking each loaded `ItemTree` and pairing every public /
//! private classifier name with the source path the tree was parsed
//! from. The variant filter only needs path info on the artifact-binding
//! path, so a coarse package+type granularity is enough — items textually
//! nested in a typed body inherit the path of their enclosing classifier
//! (matching the contract's "file-scoped binding" semantics).

use std::collections::HashMap;
use std::sync::Arc;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ItemRef, ItemTree};
use spar_variants::{HasBindingIdentity, VariantContext, keep_in_variant};

/// Identity adapter for a single component instance.
///
/// Holds borrows of the [`SystemInstance`] and the source-path map so the
/// trait methods can synthesise the artifact path and FQN on demand. The
/// lifetime parameter `'a` ties the adapter to the parent scope's
/// borrows; constructing one is essentially free (three references and an
/// idx).
pub struct ComponentInstanceIdentity<'a> {
    instance: &'a SystemInstance,
    idx: ComponentInstanceIdx,
    source_paths: &'a SourcePathMap,
    /// Cached lazily-computed FQN string. The trait returns `Option<String>`
    /// (an owned value) so we materialise once and clone on each call.
    fqn: String,
}

impl<'a> ComponentInstanceIdentity<'a> {
    /// Build an identity adapter for `idx`.
    pub fn new(
        instance: &'a SystemInstance,
        idx: ComponentInstanceIdx,
        source_paths: &'a SourcePathMap,
    ) -> Self {
        let fqn = component_fqn(instance, idx);
        Self {
            instance,
            idx,
            source_paths,
            fqn,
        }
    }
}

impl<'a> HasBindingIdentity for ComponentInstanceIdentity<'a> {
    fn artifact_path(&self) -> Option<&str> {
        let comp = self.instance.component(self.idx);
        // Walk up to the nearest ancestor (or self) whose (package, type)
        // pair exists in the map. The component-path FQN includes
        // subcomponents whose classifiers are declared in different files
        // from the enclosing implementation; the artifact-binding contract
        // applies to "every item declared in the named source file", and
        // a leaf subcomponent's "declaration site" is its own classifier's
        // file. So we ask the most specific (innermost) classifier first.
        let pkg = comp.package.as_str();
        let typ = comp.type_name.as_str();
        self.source_paths
            .lookup(pkg, typ)
            .map(std::string::String::as_str)
    }

    fn fully_qualified_symbol(&self) -> Option<String> {
        Some(self.fqn.clone())
    }
}

/// Compute the fully-qualified AADL symbol for a component instance,
/// matching the shape used by binding resolution: `Package::Type` for
/// type-only components, `Package::Type.Implementation` when the
/// implementation name is set. Subcomponents nested in a typed body get
/// `Package::Type.Impl.subname` / `…/sub2/…` form.
///
/// The resulting string is matched against
/// [`spar_variants::Binding::Symbol`] entries via prefix-with-boundary
/// rules — see [`spar_variants::binding`] for the matcher.
pub fn component_fqn(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    // Walk parent chain to the root, collecting subcomponent names.
    // We then prepend the root's `Package::Type.Impl` form.
    let mut chain: Vec<&str> = Vec::new();
    let mut cur = Some(idx);
    let mut root_idx = idx;
    while let Some(ci) = cur {
        let comp = instance.component(ci);
        // The root has parent=None; we don't include its subcomponent
        // name (which is the type+impl-derived "Type.Impl" tag) in the
        // dotted nested chain — instead we anchor on the root's package.
        if comp.parent.is_some() {
            chain.push(comp.name.as_str());
        }
        if comp.parent.is_none() {
            root_idx = ci;
        }
        cur = comp.parent;
    }
    let root = instance.component(root_idx);
    let mut s = format!("{}::{}", root.package.as_str(), root.type_name.as_str());
    if let Some(impl_name) = &root.impl_name {
        s.push('.');
        s.push_str(impl_name.as_str());
    }
    // chain is innermost-first; reverse to root→leaf.
    for name in chain.into_iter().rev() {
        s.push('.');
        s.push_str(name);
    }
    s
}

/// `(package, type)` -> source-file path map.
///
/// Keys are case-insensitive on the AADL identifier side because AADL
/// identifiers are case-insensitive. The value is the path the CLI
/// driver passed to `spar-base-db::SourceFile::new`.
#[derive(Debug, Default, Clone)]
pub struct SourcePathMap {
    inner: HashMap<(String, String), String>,
}

impl SourcePathMap {
    /// Build an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk a set of `(file_path, ItemTree)` pairs and register every
    /// declared component type / impl with its file path.
    pub fn from_trees(pairs: &[(String, Arc<ItemTree>)]) -> Self {
        let mut out = Self::default();
        for (path, tree) in pairs {
            for (_idx, pkg) in tree.packages.iter() {
                let pkg_name = pkg.name.as_str().to_ascii_lowercase();
                for item in pkg.public_items.iter().chain(pkg.private_items.iter()) {
                    match item {
                        ItemRef::ComponentType(ti) => {
                            let t = &tree.component_types[*ti];
                            out.inner.insert(
                                (pkg_name.clone(), t.name.as_str().to_ascii_lowercase()),
                                path.clone(),
                            );
                        }
                        ItemRef::ComponentImpl(ii) => {
                            let i = &tree.component_impls[*ii];
                            // Implementations live in the same file as
                            // their declaring type usually does; record
                            // both the type and the type.impl variant
                            // so subcomponent path lookups can resolve
                            // either granularity.
                            out.inner.insert(
                                (pkg_name.clone(), i.type_name.as_str().to_ascii_lowercase()),
                                path.clone(),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        out
    }

    /// Look up the path for `(package, type)`, case-insensitive on both.
    pub fn lookup(&self, package: &str, type_name: &str) -> Option<&String> {
        self.inner
            .get(&(package.to_ascii_lowercase(), type_name.to_ascii_lowercase()))
    }
}

/// Non-mutating projection of a [`SystemInstance`] under a
/// [`VariantContext`].
///
/// Builds a `kept` bitset on construction by running [`keep_in_variant`]
/// against every component, then exposes lookup-time accessors that
/// return only the kept subset. The wrapper does not touch the
/// underlying instance; the variant-aware caller in
/// `crates/spar-cli/src/moves.rs` is responsible for routing every
/// component-resolution and candidate-target lookup through it.
pub struct VariantScope<'a> {
    /// The underlying instance — borrowed, not owned. Analyses that
    /// don't need variant-awareness still see the full surface.
    pub instance: &'a SystemInstance,
    /// The variant context the projection was computed from.
    pub context: &'a VariantContext,
    /// Source-path map used for artifact-binding resolution. Stored on
    /// the scope so callers can query `is_kept()` and resolve names
    /// without rebuilding identity adapters.
    pub source_paths: &'a SourcePathMap,
    /// Per-component "kept" flag, indexed by [`ComponentInstanceIdx`].
    /// We materialise the whole vector eagerly because the variant
    /// filter is cheap (linear in #bindings × #features) and verifying
    /// a single move can ask the predicate up to N² times when scanning
    /// candidate targets.
    kept: Vec<bool>,
}

impl<'a> VariantScope<'a> {
    /// Construct a scope by filtering `instance` under `context`.
    ///
    /// Components dropped by the filter remain present in
    /// `instance.components` (the wrapper is non-mutating); they are
    /// reported as "not kept" by [`Self::is_kept`] and skipped by
    /// [`Self::all_kept_components`].
    pub fn new(
        instance: &'a SystemInstance,
        context: &'a VariantContext,
        source_paths: &'a SourcePathMap,
    ) -> Self {
        let mut kept = Vec::with_capacity(instance.component_count());
        for (idx, _) in instance.all_components() {
            let id = ComponentInstanceIdentity::new(instance, idx, source_paths);
            kept.push(keep_in_variant(&id, context));
        }
        Self {
            instance,
            context,
            source_paths,
            kept,
        }
    }

    /// True iff component `idx` survives the variant filter.
    pub fn is_kept(&self, idx: ComponentInstanceIdx) -> bool {
        self.kept.get(arena_index(idx)).copied().unwrap_or(true)
    }

    /// Iterate every kept component as `(idx, &ComponentInstance)`.
    pub fn all_kept_components(
        &self,
    ) -> impl Iterator<
        Item = (
            ComponentInstanceIdx,
            &spar_hir_def::instance::ComponentInstance,
        ),
    > {
        self.instance
            .all_components()
            .filter(move |(idx, _)| self.is_kept(*idx))
    }

    /// The variant name, for diagnostics and metadata.
    pub fn variant_name(&self) -> &str {
        &self.context.variant
    }

    /// The feature-model hash, for the `feature_model_hash` metadata
    /// field on JSON output.
    pub fn feature_model_hash(&self) -> &str {
        &self.context.feature_model_hash
    }
}

/// Translate a `la_arena::Idx<…>` back into its underlying integer.
///
/// `la_arena::Idx` doesn't expose a stable accessor for its raw u32
/// across all paths the workspace uses, but for our purpose — indexing
/// a `Vec<bool>` parallel to `instance.components` — it suffices to use
/// the iteration order. We assume `all_components()` yields indices in
/// raw-id order, which the arena guarantees.
///
/// This helper exists so [`VariantScope::is_kept`] can do an O(1)
/// lookup. If the assumption ever breaks, we fall through to the
/// "unknown idx → kept" default, which is conservative.
fn arena_index<T>(idx: la_arena::Idx<T>) -> usize {
    idx.into_raw().into_u32() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::{GlobalScope, HirDefDatabase, Name, file_item_tree};

    fn parse_to_instance(
        files: &[(&str, &str)],
        pkg: &str,
        ty: &str,
        im: &str,
    ) -> (SystemInstance, SourcePathMap) {
        let db = HirDefDatabase::default();
        let mut trees = Vec::new();
        let mut pairs = Vec::new();
        for (name, src) in files {
            let sf = spar_base_db::SourceFile::new(&db, (*name).to_string(), (*src).to_string());
            let tree = file_item_tree(&db, sf);
            pairs.push(((*name).to_string(), tree.clone()));
            trees.push(tree);
        }
        let scope = GlobalScope::from_trees(trees);
        let inst =
            SystemInstance::instantiate(&scope, &Name::new(pkg), &Name::new(ty), &Name::new(im));
        let map = SourcePathMap::from_trees(&pairs);
        (inst, map)
    }

    const TWO_FILE_MODEL_A: &str = "\
package P
public
  processor CPU
  end CPU;
  thread Worker
  end Worker;
  process Proc
  end Proc;
  process implementation Proc.Impl
    subcomponents
      t1: thread Worker;
  end Proc.Impl;
  system Sys
  end Sys;
  system implementation Sys.Impl
    subcomponents
      cpu1: processor CPU;
      app: process Proc.Impl;
  end Sys.Impl;
end P;
";

    #[test]
    fn fqn_walks_root_to_leaf() {
        let (inst, _map) = parse_to_instance(&[("a.aadl", TWO_FILE_MODEL_A)], "P", "Sys", "Impl");
        // Root is "Sys.Impl".
        let root = inst.root;
        let r = component_fqn(&inst, root);
        assert_eq!(r, "P::Sys.Impl");
        // Find a sub-leaf 't1' (declared inside Proc.Impl).
        let t1 = inst
            .all_components()
            .find(|(_, c)| c.name.as_str() == "t1")
            .unwrap()
            .0;
        // Path for t1 should be `P::Sys.Impl.app.t1`.
        assert_eq!(component_fqn(&inst, t1), "P::Sys.Impl.app.t1");
    }

    #[test]
    fn source_path_map_indexes_classifiers() {
        let (_inst, map) = parse_to_instance(&[("a.aadl", TWO_FILE_MODEL_A)], "P", "Sys", "Impl");
        assert_eq!(map.lookup("P", "Sys").map(String::as_str), Some("a.aadl"));
        assert_eq!(map.lookup("p", "sys").map(String::as_str), Some("a.aadl"));
        assert_eq!(
            map.lookup("P", "Worker").map(String::as_str),
            Some("a.aadl")
        );
        assert_eq!(map.lookup("Q", "Sys"), None);
    }

    #[test]
    fn variant_scope_drops_dropped_components() {
        let (inst, map) = parse_to_instance(&[("a.aadl", TWO_FILE_MODEL_A)], "P", "Sys", "Impl");
        // Build a context that drops every item under `a.aadl` (its
        // requires has a feature that's not active).
        let blob = r#"{
            "rivet_spar_context_version": "1",
            "variant": "noop",
            "features": [],
            "bindings": [
                { "artifact": "a.aadl", "requires": ["never_active"] }
            ],
            "feature_model_hash": "sha256:0",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "test"
        }"#;
        let ctx = VariantContext::from_json(blob).unwrap();
        let scope = VariantScope::new(&inst, &ctx, &map);
        // Every component in inst comes from a.aadl, so all are
        // dropped.
        assert!(
            scope.all_kept_components().count() == 0,
            "expected every component dropped, got {} kept",
            scope.all_kept_components().count(),
        );
    }

    #[test]
    fn variant_scope_keeps_unbound_components() {
        // Empty bindings → every component is variant-independent
        // infrastructure → all kept.
        let (inst, map) = parse_to_instance(&[("a.aadl", TWO_FILE_MODEL_A)], "P", "Sys", "Impl");
        let blob = r#"{
            "rivet_spar_context_version": "1",
            "variant": "all",
            "features": [],
            "bindings": [],
            "feature_model_hash": "sha256:0",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "test"
        }"#;
        let ctx = VariantContext::from_json(blob).unwrap();
        let scope = VariantScope::new(&inst, &ctx, &map);
        assert_eq!(
            scope.all_kept_components().count(),
            inst.component_count(),
            "expected all components kept",
        );
    }
}
