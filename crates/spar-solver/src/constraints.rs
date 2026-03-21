//! Constraint extraction from AADL instance models.
//!
//! Converts AADL component instances into solver-friendly constraint
//! structures (threads with timing properties, processors with capacity).

use spar_hir_def::instance::ComponentInstanceIdx;
use serde::Serialize;

/// Timing and binding constraints for a single thread.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadConstraint {
    /// Arena index of the thread component instance.
    #[serde(skip)]
    pub idx: ComponentInstanceIdx,
    /// Fully-qualified instance name (e.g., "app.controller").
    pub name: String,
    /// Period in picoseconds (0 means not specified).
    pub period_ps: u64,
    /// Worst-case execution time in picoseconds.
    pub wcet_ps: u64,
    /// Deadline in picoseconds (defaults to period if not specified).
    pub deadline_ps: u64,
    /// Existing processor binding, if any.
    pub current_binding: Option<String>,
    /// Thread priority (lower number = higher priority, platform-dependent).
    pub priority: Option<u64>,
}

/// Capacity constraints for a single processor.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessorConstraint {
    /// Arena index of the processor component instance.
    #[serde(skip)]
    pub idx: ComponentInstanceIdx,
    /// Fully-qualified instance name (e.g., "platform.cpu1").
    pub name: String,
    /// Available memory in bytes (if specified).
    pub memory_bytes: Option<u64>,
}

/// All constraints extracted from an AADL model, ready for the solver.
#[derive(Debug, Clone, Serialize)]
pub struct ModelConstraints {
    /// Thread constraints (one per thread instance).
    pub threads: Vec<ThreadConstraint>,
    /// Processor constraints (one per processor instance).
    pub processors: Vec<ProcessorConstraint>,
    /// Warnings generated during constraint extraction.
    pub warnings: Vec<String>,
}
