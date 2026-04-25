//! Schema and parsing for the v1 variant context blob.
//!
//! Mirrors the JSON shape documented in §"The variant context blob"
//! of `docs/contracts/rivet-spar-variant-v1.md`. The reader is strict
//! on the contract version (see [`VariantContext::from_json`]) and
//! tolerant — per `serde` defaults — of unknown sibling fields, so v2
//! emitters that add new optional fields without bumping semantics
//! still parse cleanly. Semantic-changing v2 emitters are required by
//! the contract to bump the version, which v1 readers refuse.

use std::fmt;

use serde::Deserialize;

/// The current (and only) supported contract version.
///
/// Per the contract, v1 readers MUST accept `"1"` and MAY reject other
/// values. spar takes the strict route: anything else returns
/// [`ContextError::UnknownVersion`].
pub const SUPPORTED_VERSION: &str = "1";

/// One resolved variant context, deserialized from the rivet blob.
///
/// See `docs/contracts/rivet-spar-variant-v1.md` §"Fields" for the
/// authoritative description of each member.
#[derive(Debug, Clone, Deserialize)]
pub struct VariantContext {
    /// Contract version; MUST equal [`SUPPORTED_VERSION`] for v1
    /// readers. Stored as a `String` rather than parsed-and-discarded
    /// so downstream diagnostics can echo it back.
    pub rivet_spar_context_version: String,
    /// Name of the resolved variant — matches a `variants/<name>.yaml`
    /// in rivet.
    pub variant: String,
    /// Flat list of feature names active in this variant. Order- and
    /// duplicate-insensitive at the contract level; spar treats it as
    /// a set.
    pub features: Vec<String>,
    /// File- or symbol-scoped bindings. May be empty.
    pub bindings: Vec<Binding>,
    /// Stable hash of the feature model that produced this resolution.
    /// Salsa cache key for variant-aware queries.
    pub feature_model_hash: String,
    /// RFC 3339 timestamp of resolution. Audit-only.
    pub resolved_at: String,
    /// Emitter tool + version. Diagnostics-only.
    pub generated_by: String,
}

/// One binding entry. Either file-scoped (`artifact`) or symbol-scoped
/// (`symbol`), never both. Discriminated structurally by which key is
/// present in the JSON object — `serde(untagged)` performs the
/// disambiguation.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Binding {
    /// File-scoped: applies to every item declared in the named source
    /// file, after path normalization.
    Artifact {
        /// Project-relative path to the source file.
        artifact: String,
        /// Feature names that MUST all appear in
        /// [`VariantContext::features`] for this binding to be
        /// satisfied.
        requires: Vec<String>,
    },
    /// Symbol-scoped: applies to the named AADL classifier and every
    /// item textually nested inside its body.
    Symbol {
        /// Fully-qualified AADL name, shape `Package::Type` or
        /// `Package::Type.Implementation`.
        symbol: String,
        /// Feature names that MUST all appear in
        /// [`VariantContext::features`] for this binding to be
        /// satisfied.
        requires: Vec<String>,
    },
}

/// Error returned by [`VariantContext::from_json`].
#[derive(Debug)]
pub enum ContextError {
    /// `rivet_spar_context_version` was something other than `"1"`.
    /// Per the contract, v1 readers refuse v2 blobs (and any other
    /// value) — this is correct behaviour, not a bug.
    UnknownVersion(String),
    /// Underlying JSON parse failure.
    JsonError(serde_json::Error),
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextError::UnknownVersion(v) => write!(
                f,
                "unsupported rivet_spar_context_version {v:?}; \
                 this build of spar speaks v1 only \
                 (see docs/contracts/rivet-spar-variant-v1.md)"
            ),
            ContextError::JsonError(err) => write!(f, "invalid variant context JSON: {err}"),
        }
    }
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ContextError::JsonError(err) => Some(err),
            ContextError::UnknownVersion(_) => None,
        }
    }
}

impl From<serde_json::Error> for ContextError {
    fn from(err: serde_json::Error) -> Self {
        ContextError::JsonError(err)
    }
}

impl VariantContext {
    /// Parse a JSON blob into a [`VariantContext`], rejecting any
    /// `rivet_spar_context_version` other than [`SUPPORTED_VERSION`].
    pub fn from_json(s: &str) -> Result<Self, ContextError> {
        let ctx: VariantContext = serde_json::from_str(s)?;
        if ctx.rivet_spar_context_version != SUPPORTED_VERSION {
            return Err(ContextError::UnknownVersion(ctx.rivet_spar_context_version));
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn minimal_v1_blob() -> &'static str {
        r#"{
            "rivet_spar_context_version": "1",
            "variant": "diesel-eu5",
            "features": ["engine_diesel"],
            "bindings": [],
            "feature_model_hash": "sha256:abc",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "rivet 0.3.x"
        }"#
    }

    #[test]
    fn parse_minimal_v1_context() {
        let ctx = VariantContext::from_json(minimal_v1_blob()).expect("parses");
        assert_eq!(ctx.rivet_spar_context_version, "1");
        assert_eq!(ctx.variant, "diesel-eu5");
        assert_eq!(ctx.features, vec!["engine_diesel".to_string()]);
        assert!(ctx.bindings.is_empty());
        assert_eq!(ctx.feature_model_hash, "sha256:abc");
        assert_eq!(ctx.resolved_at, "2026-04-23T12:00:00Z");
        assert_eq!(ctx.generated_by, "rivet 0.3.x");
    }

    #[test]
    fn reject_unknown_version() {
        let blob = r#"{
            "rivet_spar_context_version": "2",
            "variant": "x",
            "features": [],
            "bindings": [],
            "feature_model_hash": "sha256:0",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "rivet 99"
        }"#;
        match VariantContext::from_json(blob) {
            Err(ContextError::UnknownVersion(v)) => assert_eq!(v, "2"),
            other => panic!("expected UnknownVersion(\"2\"), got {other:?}"),
        }
    }

    #[test]
    fn parse_artifact_binding() {
        let blob = r#"{
            "rivet_spar_context_version": "1",
            "variant": "v",
            "features": [],
            "bindings": [
                { "artifact": "spec/engines/diesel.aadl", "requires": ["engine_diesel"] }
            ],
            "feature_model_hash": "sha256:0",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "rivet 0.3.x"
        }"#;
        let ctx = VariantContext::from_json(blob).expect("parses");
        assert_eq!(ctx.bindings.len(), 1);
        match &ctx.bindings[0] {
            Binding::Artifact { artifact, requires } => {
                assert_eq!(artifact, "spec/engines/diesel.aadl");
                assert_eq!(requires, &vec!["engine_diesel".to_string()]);
            }
            other => panic!("expected Artifact, got {other:?}"),
        }
    }

    #[test]
    fn parse_symbol_binding() {
        let blob = r#"{
            "rivet_spar_context_version": "1",
            "variant": "v",
            "features": [],
            "bindings": [
                { "symbol": "Engines::Engine.Diesel", "requires": ["engine_diesel"] }
            ],
            "feature_model_hash": "sha256:0",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "rivet 0.3.x"
        }"#;
        let ctx = VariantContext::from_json(blob).expect("parses");
        assert_eq!(ctx.bindings.len(), 1);
        match &ctx.bindings[0] {
            Binding::Symbol { symbol, requires } => {
                assert_eq!(symbol, "Engines::Engine.Diesel");
                assert_eq!(requires, &vec!["engine_diesel".to_string()]);
            }
            other => panic!("expected Symbol, got {other:?}"),
        }
    }

    #[test]
    fn parse_contract_example_blob() {
        // This is the §"Example" blob from
        // docs/contracts/rivet-spar-variant-v1.md, verbatim. Acts as a
        // canary that the documented schema actually deserializes under
        // the types we ship.
        let blob = r#"{
            "rivet_spar_context_version": "1",
            "variant": "diesel-eu5",
            "features": [
                "engine_diesel",
                "emissions_eu5",
                "platform_zephyr_v3",
                "target_cortex_m4"
            ],
            "bindings": [
                { "artifact": "spec/engines/diesel.aadl",   "requires": ["engine_diesel"] },
                { "artifact": "spec/engines/electric.aadl", "requires": ["engine_electric"] },
                { "symbol":   "Engines::Engine.Diesel",     "requires": ["engine_diesel"] }
            ],
            "feature_model_hash": "sha256:abc123",
            "resolved_at": "2026-04-23T12:00:00Z",
            "generated_by": "rivet 0.3.x"
        }"#;
        let ctx = VariantContext::from_json(blob).expect("contract example parses");
        assert_eq!(ctx.variant, "diesel-eu5");
        assert_eq!(ctx.features.len(), 4);
        assert_eq!(ctx.bindings.len(), 3);
        assert!(matches!(ctx.bindings[0], Binding::Artifact { .. }));
        assert!(matches!(ctx.bindings[1], Binding::Artifact { .. }));
        assert!(matches!(ctx.bindings[2], Binding::Symbol { .. }));
    }
}
