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
//! - **Phase 1 (this milestone, v0.8.0):** types only. The crate is a
//!   skeleton placeholder. The Spar_Network property set surface
//!   landed in `spar-hir-def::standard_properties` alongside this
//!   crate. Subsequent commits in Track D add the algorithm pieces:
//!   network-graph extraction (commit 2), Network Calculus primitives
//!   (commit 3), the `wctt.rs` analysis pass (commit 4), and Lean
//!   theorems (commit 5).
//! - **Phase 2 (v0.8.x or v0.9.0):** Network Calculus primitives —
//!   arrival/service curves, min-plus convolution and deconvolution,
//!   horizontal/vertical distance bounds. Exposed under
//!   [`types`] and (later) `curve` modules.
//! - **Phase 3:** TSN-shaped service curves (TAS/Qbv, Qbu preemption,
//!   Qcr ATS). These build on Phase 2's primitives.
//!
//! # What is intentionally NOT in this crate yet
//!
//! - No `wctt.rs` — that lives in `spar-analysis` and lands later in
//!   Track D.
//! - No Network Calculus algebra — Phase 2 will introduce
//!   `ArrivalCurve`, `ServiceCurve`, and the min-plus operators.
//! - No TSN-specific service-curve generators — Phase 3.
//! - No AADL-specific knowledge: this crate stays free of
//!   `spar-hir-def`/`spar-hir`/diagnostic dependencies. Adapters
//!   between `SystemInstance` and the network graph live in
//!   `spar-analysis`.
//!
//! See the design doc for the full scope and the rationale behind the
//! crate split.

#![forbid(unsafe_code)]

/// Placeholder module for network-domain types.
///
/// Phase 1 leaves this empty. Phase 2 will introduce
/// `ArrivalCurve`, `ServiceCurve`, server-graph node/edge types, and
/// the WCTT result record. The module is exposed today only to fix
/// the public path so that downstream crates can already write
/// `use spar_network::types::*;` without a future breaking change.
pub mod types {}
