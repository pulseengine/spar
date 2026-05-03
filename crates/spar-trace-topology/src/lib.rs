//! Runtime/declared topology reconciliation for spar.
//!
//! v0.10.0 trace-topology foundation (Track G). This crate provides
//! the surface that, in subsequent commits, lets `spar trace topology`
//! consume the runtime artefact set an OEM produces from a real
//! deployment — PCAPNG captures, LLDP topology snapshots, Qcc YANG
//! switch configs, tc/ethtool dumps, gPTP synchronization logs — and
//! reconcile them against the AADL declaration of "what should be on
//! the wire".
//!
//! The v1 design — input artefacts, the five deterministic checks
//! (`IdentityUnknown`, `TopologyMissingWiring`, `ConfigDrift`,
//! `GptpOutOfBudget`, `BinaryMismatch`), the SARIF + signed in-toto
//! attestation output shape, and the implementation phasing — is laid
//! out in `docs/designs/v0.10.0-trace-topology.md`. The
//! external-integrator contract (predicate URL, JSON schema reference,
//! stability promise) lives in `docs/contracts/spar-trace-topology-v1.md`.
//!
//! v0.10.0 ships only the foundation:
//!
//! - The [`identity`] module exposes typed accessors for the new
//!   `Spar_Identity::*` property surface (`MAC_Address`, `VLAN_ID`,
//!   `Stream_Handle`, `Multicast_Group`, `LLDP_Chassis_Id`,
//!   `LLDP_Port_Id`).
//! - The [`ingest`] module declares trait skeletons for the four
//!   parsers — frame source (PCAPNG), topology source (LLDP), switch
//!   config source (Qcc YANG), and PTP-time source (gPTP). Real
//!   parsing lands in v0.10.x sibling commits.
//! - The [`reconcile`] module declares the `ReconcileFinding` enum
//!   carrying the five deterministic check kinds. The reconciliation
//!   engine itself ships in v0.11.0.
//! - The [`report`] module declares the `TopologyReport` struct that
//!   collects findings. SARIF emission and the signed in-toto
//!   attestation predicate (`https://pulseengine.eu/spar-trace-topology/v1`)
//!   land in v0.11.0 / v1.0 respectively.
//!
//! Out of scope for v1: PCAP-classic, BLF, OPC-UA, deep packet
//! inspection. See the design doc §"Out-of-scope for v1".

pub mod identity;
pub mod ingest;
pub mod reconcile;
pub mod report;
