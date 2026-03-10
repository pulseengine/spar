//! Pluggable analysis framework for AADL models.
//!
//! This crate provides a trait-based analysis framework that operates on
//! the AADL instance model (`SystemInstance`). Built-in analyses check
//! connectivity, hierarchy validity, and model completeness.
//!
//! # Usage
//!
//! ```ignore
//! let mut runner = AnalysisRunner::new();
//! runner.register(Box::new(ConnectivityAnalysis));
//! runner.register(Box::new(HierarchyAnalysis));
//! runner.register(Box::new(CompletenessAnalysis));
//! let diagnostics = runner.run_all(&instance);
//! ```

pub mod arinc653;
pub mod binding_check;
pub mod binding_rules;
pub mod category_check;
pub mod classifier_match;
pub mod completeness;
pub mod connection_rules;
pub mod connectivity;
pub mod direction_rules;
pub mod emv2_analysis;
pub mod extends_rules;
pub mod flow_check;
pub mod flow_rules;
pub mod hierarchy;
pub mod latency;
pub mod legality;
pub mod modal_rules;
pub mod mode_check;
pub mod mode_rules;
pub mod naming_rules;
pub mod property_rules;
pub mod resource_budget;
pub mod scheduling;
pub mod subcomponent_rules;

use serde::Serialize;
use spar_hir_def::instance::SystemInstance;

/// A single analysis that can be run on an AADL system instance.
pub trait Analysis {
    /// Human-readable name of this analysis.
    fn name(&self) -> &str;

    /// Run the analysis on a system instance. Returns diagnostics.
    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic>;
}

/// A diagnostic produced by an analysis pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalysisDiagnostic {
    pub severity: Severity,
    pub message: String,
    /// Path to the element (e.g., `["root", "subsystem", "cpu"]`).
    pub path: Vec<String>,
    /// Which analysis produced this diagnostic.
    pub analysis: String,
}

/// Severity level for an analysis diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Collects analyses and runs them against a system instance.
pub struct AnalysisRunner {
    analyses: Vec<Box<dyn Analysis>>,
}

impl AnalysisRunner {
    /// Create a new empty runner.
    pub fn new() -> Self {
        Self {
            analyses: Vec::new(),
        }
    }

    /// Register an analysis to be run.
    pub fn register(&mut self, analysis: Box<dyn Analysis>) {
        self.analyses.push(analysis);
    }

    /// Run all registered analyses and collect their diagnostics.
    pub fn run_all(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut all_diagnostics = Vec::new();
        for analysis in &self.analyses {
            let diags = analysis.analyze(instance);
            all_diagnostics.extend(diags);
        }
        all_diagnostics
    }
}

impl Default for AnalysisRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

use spar_hir_def::instance::ComponentInstanceIdx;

/// Build the element path for a component instance by walking up through parents.
pub(crate) fn component_path(instance: &SystemInstance, idx: ComponentInstanceIdx) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = Some(idx);
    while let Some(ci) = current {
        let comp = instance.component(ci);
        path.push(comp.name.as_str().to_string());
        current = comp.parent;
    }
    path.reverse();
    path
}

/// Compute the depth of a component in the hierarchy (root = 0).
pub(crate) fn component_depth(instance: &SystemInstance, idx: ComponentInstanceIdx) -> usize {
    let mut depth = 0;
    let mut current = instance.component(idx).parent;
    while let Some(parent) = current {
        depth += 1;
        current = instance.component(parent).parent;
    }
    depth
}

#[cfg(test)]
mod tests;
