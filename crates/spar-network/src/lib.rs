//! Network Calculus primitives for AADL WCTT (Worst-Case Traversal Time)
//! analysis.
//!
//! This crate is the home of spar's Track D effort (issue #149,
//! research PR #152, design doc
//! [`docs/designs/track-d-tsn-wctt-research.md`]). Its purpose is to
//! host network-domain types and algorithms that are reused across
//! analyses but kept separate from `spar-analysis` for the same reason
//! `spar-solver` is a sibling crate: the math is general, has no
//! AADL-specific diagnostic dependencies, and is independently
//! testable.
//!
//! # Phasing
//!
//! - **Phase 1 (this milestone, v0.8.0):**
//!     - **Commit 1 (#155):** `Spar_Network` property set surface in
//!       `spar-hir-def::standard_properties`.
//!     - **Commit 2 (#157):** [`types`] — `NetworkNode`,
//!       `NetworkLink`, `SwitchType`, `NetworkGraph`. [`extract`] —
//!       extractor that walks a `SystemInstance` and emits a
//!       `NetworkGraph` for downstream WCTT analysis.
//!     - **Commit 3 (this commit):** [`curves`] — Network Calculus
//!       primitives ([`ArrivalCurve`], [`ServiceCurve`]) plus the
//!       four closed-form min-plus operators ([`backlog_bound`],
//!       [`delay_bound`], [`residual_service`], [`output_bound`]).
//!       Pure math kernel — no AADL coupling, no analysis pass yet.
//!     - **Commit 4:** `wctt.rs` analysis pass in `spar-analysis`.
//!     - **Commit 5:** Lean theorems in `proofs/Proofs/Network/`.
//!     - **Commit 6:** `latency.rs` integration + COMPLIANCE.md update.
//! - **Phase 2 (v0.8.x or v0.9.0):** TSN-shaped service curves
//!   (TAS/Qbv, Qbu preemption, Qcr ATS) under a separate `Spar_TSN`
//!   property set.
//!
//! # Crate boundary
//!
//! Commit 2 introduces a dependency on `spar-hir-def` so the extractor
//! can read from `SystemInstance`. The Network Calculus primitives
//! added in commit 3 ([`curves`]) keep that direction: they are pure
//! math (no AADL coupling) and `spar-analysis` will pull `spar-network`
//! from `wctt.rs` to compose them, never the reverse. The `curves`
//! module deliberately does not consume from [`extract`]; downstream
//! `wctt.rs` is the place where a [`NetworkGraph`] becomes
//! [`ArrivalCurve`]/[`ServiceCurve`] inputs.

#![forbid(unsafe_code)]

pub mod curves;
pub mod extract;
pub mod types;

pub use curves::{
    ArrivalCurve, NcError, ServiceCurve, backlog_bound, delay_bound, output_bound, residual_service,
};
pub use extract::extract_network_graph;
pub use types::{NetworkGraph, NetworkLink, NetworkNode, NodeKind, SwitchType};
