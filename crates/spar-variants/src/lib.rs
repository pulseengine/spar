//! Consumer-side adapter for rivet's variant context blob, v1 contract.
//!
//! This crate is the spar side of the rivet ↔ spar variant binding
//! contract documented in
//! [`docs/contracts/rivet-spar-variant-v1.md`](../../../docs/contracts/rivet-spar-variant-v1.md).
//!
//! Rivet owns the entire product-line model — feature model, constraints,
//! variant definitions, bindings, SAT resolution. spar consumes a
//! resolved JSON context blob (emitted by `rivet resolve --variant <name>
//! --format spar-context-json`) and uses it to filter HIR items down to
//! those that are valid in the chosen variant. spar does **not** parse
//! rivet artifacts and does **not** solve feature constraints.
//!
//! # Crate boundary
//!
//! This is the lowest layer of the consumer side. It depends on
//! [`spar-hir-def`] only for the eventual `HasBindingIdentity` adapter
//! impls — but those adapters do **not** live in this commit; they will
//! ship alongside the spar-cli wiring (Track B commit 2). For now the
//! crate exposes the trait and the `keep_in_variant` predicate, and
//! tests use a local stub type. There is no CLI integration in this
//! commit.
//!
//! # Public API
//!
//! - [`VariantContext`] — deserialize a rivet v1 blob with
//!   [`VariantContext::from_json`].
//! - [`Binding`] — file- or symbol-scoped binding entry.
//! - [`HasBindingIdentity`] — trait HIR items implement to participate
//!   in variant filtering.
//! - [`keep_in_variant`] — apply intersection-semantics rules to decide
//!   whether an item is kept for a given context.
//!
//! # Versioning
//!
//! v1 readers strictly accept `rivet_spar_context_version == "1"`. Any
//! other value (including a future `"2"`) is rejected with
//! [`ContextError::UnknownVersion`], per the contract's "Compatibility
//! and versioning" section: a v2 reader breaking-change is announced via
//! the version bump, and a v1 reader refusing v2 is the correct
//! behaviour.

#![forbid(unsafe_code)]

pub mod binding;
pub mod context;
pub mod filter;

pub use binding::HasBindingIdentity;
pub use context::{Binding, ContextError, VariantContext};
pub use filter::keep_in_variant;
