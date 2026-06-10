//! Reconciliation finding type surface.
//!
//! v0.10.0 declares the [`ReconcileFinding`] enum but ships no
//! reconciliation logic — the engine that emits these findings lands
//! in v0.11.0. The five variants correspond to the five deterministic
//! checks described in `docs/designs/v0.10.0-trace-topology.md`
//! §"Five deterministic checks":
//!
//! 1. [`ReconcileFinding::IdentityUnknown`] — a runtime artefact
//!    (frame, LLDP neighbor, Qcc stream) refers to an identity
//!    (`MAC_Address`, `LLDP_Chassis_Id`, `Stream_Handle`,
//!    `Multicast_Group`) that no AADL `Spar_Identity::*` annotation
//!    declares.
//! 2. [`ReconcileFinding::TopologyMissingWiring`] — the LLDP
//!    snapshot reports a neighbor adjacency for which no AADL
//!    `bus access` connection is declared.
//! 3. [`ReconcileFinding::ConfigDrift`] — the Qcc/tc/ethtool
//!    configuration differs from the AADL declaration of the same
//!    surface (`Spar_TSN::Gate_Control_List`,
//!    `Spar_TSN::Bandwidth_Reservation`, `Spar_TSN::Max_Frame_Size`,
//!    …).
//! 4. [`ReconcileFinding::GptpOutOfBudget`] — the observed gPTP
//!    synchronization error exceeds the declared
//!    `Spar_TSN::Sync_Error` per-hop budget for at least one
//!    capture-window sample.
//! 5. [`ReconcileFinding::BinaryMismatch`] — the running image's
//!    digest differs from the AADL `Source_Text` / build-recorded
//!    digest for the same component.
//!
//! Each variant carries minimal placeholder fields so the v0.11.0
//! engine can extend them without churning the SARIF / in-toto
//! attestation predicate URL `https://pulseengine.eu/spar-trace-topology/v1`.

/// One reconciliation finding produced by `spar trace topology`.
///
/// The variants correspond to the five deterministic checks in the
/// v1 design. v0.10.0 ships the type surface only; the
/// [`crate::report::TopologyReport`] aggregates these for emission
/// to SARIF (v0.11.0) and to a signed in-toto attestation (v1.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileFinding {
    /// Runtime artefact references an identity unknown to AADL.
    IdentityUnknown {
        /// What the runtime saw — a MAC, a chassis-id, a stream
        /// handle, etc., serialised in its native form.
        observed: String,
        /// Free-form context (capture file, LLDP snapshot, …).
        context: String,
    },
    /// LLDP neighbor adjacency without a corresponding AADL
    /// `bus access` connection.
    TopologyMissingWiring {
        /// Local end of the unwired adjacency (LLDP chassis-id /
        /// port-id pair, serialised).
        local: String,
        /// Remote end of the unwired adjacency.
        remote: String,
    },
    /// Switch / NIC config drift versus the AADL declaration.
    ConfigDrift {
        /// Property surface that disagrees (e.g.
        /// `"Spar_TSN::Gate_Control_List"`).
        property: String,
        /// AADL-declared value, source-text form.
        declared: String,
        /// Observed runtime value, source-text form.
        observed: String,
    },
    /// gPTP error exceeded the declared per-hop budget.
    GptpOutOfBudget {
        /// AADL identity of the bus / processor whose synchronization
        /// budget was exceeded.
        bus_or_processor: String,
        /// Declared budget in picoseconds (matches
        /// `Spar_TSN::Sync_Error`'s lowering).
        budget_ps: u64,
        /// Worst-case observed error in the capture window, picoseconds.
        observed_ps: u64,
    },
    /// Running image digest disagrees with the build-recorded digest.
    BinaryMismatch {
        /// AADL FQN of the affected component.
        component: String,
        /// Declared digest (e.g. `"sha256:…"`).
        declared_digest: String,
        /// Observed digest at runtime.
        observed_digest: String,
    },
}

impl ReconcileFinding {
    /// Stable kind tag for SARIF rule-id assignment / JSON
    /// serialisation. The v1 contract pins these strings.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::IdentityUnknown { .. } => "IdentityUnknown",
            Self::TopologyMissingWiring { .. } => "TopologyMissingWiring",
            Self::ConfigDrift { .. } => "ConfigDrift",
            Self::GptpOutOfBudget { .. } => "GptpOutOfBudget",
            Self::BinaryMismatch { .. } => "BinaryMismatch",
        }
    }
}
