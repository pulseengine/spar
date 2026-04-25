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
//!     - **Commit 2 (this commit):** [`types`] — `NetworkNode`,
//!       `NetworkLink`, `SwitchType`, `NetworkGraph`. [`extract`] —
//!       extractor that walks a `SystemInstance` and emits a
//!       `NetworkGraph` for downstream WCTT analysis.
//!     - **Commit 3:** Network Calculus primitives (`ArrivalCurve`,
//!       `ServiceCurve`, min-plus operators).
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
//! can read from `SystemInstance`. Future Network Calculus primitives
//! (commit 3) and the `wctt.rs` analysis pass (commit 4) keep this
//! direction: lower-level math types are AADL-aware only via the typed
//! `NetworkGraph` produced here. `spar-analysis` will pull from
//! `spar-network`, never the reverse.

#![forbid(unsafe_code)]

pub mod extract;
pub mod types;

pub use extract::extract_network_graph;
pub use types::{NetworkGraph, NetworkLink, NetworkNode, NodeKind, SwitchType};
