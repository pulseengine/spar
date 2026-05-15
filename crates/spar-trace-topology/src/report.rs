//! Topology-report aggregation surface.
//!
//! v0.10.0 ships only the [`TopologyReport`] container. SARIF
//! emission and the signed in-toto attestation predicate URL
//! `https://pulseengine.eu/spar-trace-topology/v1` land in v0.11.0
//! and v1.0 respectively per
//! `docs/designs/v0.10.0-trace-topology.md` §"Implementation
//! phasing".

use crate::reconcile::ReconcileFinding;

/// Aggregated reconciliation findings for one
/// `spar trace topology` run.
///
/// v0.10.0 carries only the findings list. v0.11.0 widens this to
/// include capture metadata (input artefact hashes, capture window
/// timestamps), the SARIF emitter target, and the in-toto
/// attestation envelope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopologyReport {
    /// Every reconciliation finding raised over the capture window.
    /// Empty when the runtime artefacts are byte-clean against the
    /// AADL declaration.
    pub findings: Vec<ReconcileFinding>,
}

impl TopologyReport {
    /// Create an empty report — no findings yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one [`ReconcileFinding`].
    pub fn push(&mut self, finding: ReconcileFinding) {
        self.findings.push(finding);
    }

    /// `true` when no reconciliation finding was raised. The
    /// v0.11.0 engine exits with status 0 when this is the case
    /// and the in-toto attestation declares the run "verified".
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}
