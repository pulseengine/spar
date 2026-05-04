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

pub mod ai_ml;
pub mod arinc653;
pub mod binding_check;
pub mod binding_rules;
pub mod bus_bandwidth;
pub mod category_check;
pub mod classifier_match;
pub mod completeness;
pub mod connection_rules;
pub mod connectivity;
pub mod direction_rules;
pub mod emv2_analysis;
pub mod emv2_stpa_bridge;
pub mod extends_rules;
pub mod feature_group_check;
pub mod flow_check;
pub mod flow_rules;
pub mod hierarchy;
pub mod latency;
pub mod legality;
pub mod memory_budget;
pub mod modal;
pub mod modal_rules;
pub mod mode_check;
pub mod mode_reachability;
pub mod mode_rules;
pub mod naming_rules;
pub mod property_accessors;
pub mod property_rules;
pub mod resource_budget;
pub mod rta;
pub mod scheduling;
pub mod scheduling_verified;
pub mod subcomponent_rules;
pub mod wctt;
pub mod weight_power;
pub mod wrpc_binding;

pub use wctt::WcttAnalysis;

use serde::Serialize;
use spar_hir_def::instance::{SystemInstance, SystemOperationMode};

/// A single analysis that can be run on an AADL system instance.
pub trait Analysis {
    /// Human-readable name of this analysis.
    fn name(&self) -> &str;

    /// Run the analysis on a system instance. Returns diagnostics.
    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic>;

    /// Return this analysis as a `ModalAnalysis` if it supports per-SOM analysis.
    ///
    /// The default returns `None`, meaning the analysis is mode-independent.
    fn as_modal(&self) -> Option<&dyn ModalAnalysis> {
        None
    }
}

/// A mode-dependent analysis that can be run per System Operation Mode (SOM).
///
/// Analyses implementing this trait will be invoked once per SOM by
/// [`AnalysisRunner::run_all_per_som`], receiving the specific SOM context.
pub trait ModalAnalysis: Analysis {
    /// Run the analysis on a system instance within a specific SOM context.
    fn analyze_in_mode(
        &self,
        instance: &SystemInstance,
        som: &SystemOperationMode,
    ) -> Vec<AnalysisDiagnostic>;
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

    /// Register all built-in instance-level analysis passes.
    ///
    /// This includes all analyses that implement the [`Analysis`] trait:
    /// connectivity, hierarchy, completeness, direction rules, classifier
    /// matching, binding checks, binding rules, flow checks, flow rules,
    /// mode checks, mode rules, modal rules, property rules, connection
    /// rules, subcomponent rules, scheduling, response-time analysis,
    /// latency, memory budget, resource budget, EMV2 fault-tree,
    /// EMV2-STPA bridge, ARINC 653, wRPC binding, weight/power aggregation,
    /// and bus bandwidth.
    pub fn register_all(&mut self) {
        use ai_ml::AiMlAnalysis;
        use arinc653::Arinc653Analysis;
        use binding_check::BindingCheckAnalysis;
        use binding_rules::BindingRuleAnalysis;
        use bus_bandwidth::BusBandwidthAnalysis;
        use classifier_match::ClassifierMatchAnalysis;
        use completeness::CompletenessAnalysis;
        use connection_rules::ConnectionRuleAnalysis;
        use connectivity::ConnectivityAnalysis;
        use direction_rules::DirectionRuleAnalysis;
        use emv2_analysis::Emv2Analysis;
        use emv2_stpa_bridge::Emv2StpaBridgeAnalysis;
        use feature_group_check::FeatureGroupCheckAnalysis;
        use flow_check::FlowCheckAnalysis;
        use flow_rules::FlowRuleAnalysis;
        use hierarchy::HierarchyAnalysis;
        use latency::LatencyAnalysis;
        use memory_budget::MemoryBudgetAnalysis;
        use modal_rules::ModalRuleAnalysis;
        use mode_check::ModeCheckAnalysis;
        use mode_reachability::ModeReachabilityAnalysis;
        use mode_rules::ModeRuleAnalysis;
        use property_rules::PropertyRuleAnalysis;
        use resource_budget::ResourceBudgetAnalysis;
        use rta::RtaAnalysis;
        use scheduling::SchedulingAnalysis;
        use subcomponent_rules::SubcomponentRuleAnalysis;
        use wctt::WcttAnalysis;
        use weight_power::WeightPowerAnalysis;
        use wrpc_binding::WrpcBindingAnalysis;

        self.register(Box::new(AiMlAnalysis));
        self.register(Box::new(ConnectivityAnalysis));
        self.register(Box::new(HierarchyAnalysis));
        self.register(Box::new(CompletenessAnalysis));
        self.register(Box::new(DirectionRuleAnalysis));
        self.register(Box::new(ClassifierMatchAnalysis));
        self.register(Box::new(BindingCheckAnalysis));
        self.register(Box::new(BindingRuleAnalysis));
        self.register(Box::new(FlowCheckAnalysis));
        self.register(Box::new(FlowRuleAnalysis));
        self.register(Box::new(ModeCheckAnalysis));
        self.register(Box::new(ModeRuleAnalysis));
        self.register(Box::new(ModalRuleAnalysis));
        self.register(Box::new(PropertyRuleAnalysis));
        self.register(Box::new(ConnectionRuleAnalysis));
        self.register(Box::new(SubcomponentRuleAnalysis));
        self.register(Box::new(SchedulingAnalysis));
        self.register(Box::new(RtaAnalysis));
        self.register(Box::new(LatencyAnalysis));
        self.register(Box::new(MemoryBudgetAnalysis));
        self.register(Box::new(ResourceBudgetAnalysis));
        self.register(Box::new(Emv2Analysis));
        self.register(Box::new(Emv2StpaBridgeAnalysis));
        self.register(Box::new(Arinc653Analysis));
        self.register(Box::new(WrpcBindingAnalysis));
        self.register(Box::new(ModeReachabilityAnalysis));
        self.register(Box::new(WeightPowerAnalysis));
        self.register(Box::new(BusBandwidthAnalysis));
        self.register(Box::new(FeatureGroupCheckAnalysis));
        self.register(Box::new(WcttAnalysis::default()));
    }

    /// Register all instance-level analyses **except** [`wctt::WcttAnalysis`].
    ///
    /// Used by the CLI's `--pmoo` flag path to register a custom-
    /// configured `WcttAnalysis` (with PMOO/LUDB enabled) without
    /// duplicating the v0.9.2 SFA pass. See
    /// `spar-cli/src/main.rs::run_all_analyses_with_pmoo`.
    pub fn register_all_except_wctt(&mut self) {
        use ai_ml::AiMlAnalysis;
        use arinc653::Arinc653Analysis;
        use binding_check::BindingCheckAnalysis;
        use binding_rules::BindingRuleAnalysis;
        use bus_bandwidth::BusBandwidthAnalysis;
        use classifier_match::ClassifierMatchAnalysis;
        use completeness::CompletenessAnalysis;
        use connection_rules::ConnectionRuleAnalysis;
        use connectivity::ConnectivityAnalysis;
        use direction_rules::DirectionRuleAnalysis;
        use emv2_analysis::Emv2Analysis;
        use emv2_stpa_bridge::Emv2StpaBridgeAnalysis;
        use feature_group_check::FeatureGroupCheckAnalysis;
        use flow_check::FlowCheckAnalysis;
        use flow_rules::FlowRuleAnalysis;
        use hierarchy::HierarchyAnalysis;
        use latency::LatencyAnalysis;
        use memory_budget::MemoryBudgetAnalysis;
        use modal_rules::ModalRuleAnalysis;
        use mode_check::ModeCheckAnalysis;
        use mode_reachability::ModeReachabilityAnalysis;
        use mode_rules::ModeRuleAnalysis;
        use property_rules::PropertyRuleAnalysis;
        use resource_budget::ResourceBudgetAnalysis;
        use rta::RtaAnalysis;
        use scheduling::SchedulingAnalysis;
        use subcomponent_rules::SubcomponentRuleAnalysis;
        use weight_power::WeightPowerAnalysis;
        use wrpc_binding::WrpcBindingAnalysis;

        self.register(Box::new(AiMlAnalysis));
        self.register(Box::new(ConnectivityAnalysis));
        self.register(Box::new(HierarchyAnalysis));
        self.register(Box::new(CompletenessAnalysis));
        self.register(Box::new(DirectionRuleAnalysis));
        self.register(Box::new(ClassifierMatchAnalysis));
        self.register(Box::new(BindingCheckAnalysis));
        self.register(Box::new(BindingRuleAnalysis));
        self.register(Box::new(FlowCheckAnalysis));
        self.register(Box::new(FlowRuleAnalysis));
        self.register(Box::new(ModeCheckAnalysis));
        self.register(Box::new(ModeRuleAnalysis));
        self.register(Box::new(ModalRuleAnalysis));
        self.register(Box::new(PropertyRuleAnalysis));
        self.register(Box::new(ConnectionRuleAnalysis));
        self.register(Box::new(SubcomponentRuleAnalysis));
        self.register(Box::new(SchedulingAnalysis));
        self.register(Box::new(RtaAnalysis));
        self.register(Box::new(LatencyAnalysis));
        self.register(Box::new(MemoryBudgetAnalysis));
        self.register(Box::new(ResourceBudgetAnalysis));
        self.register(Box::new(Emv2Analysis));
        self.register(Box::new(Emv2StpaBridgeAnalysis));
        self.register(Box::new(Arinc653Analysis));
        self.register(Box::new(WrpcBindingAnalysis));
        self.register(Box::new(ModeReachabilityAnalysis));
        self.register(Box::new(WeightPowerAnalysis));
        self.register(Box::new(BusBandwidthAnalysis));
        self.register(Box::new(FeatureGroupCheckAnalysis));
        // WcttAnalysis intentionally omitted — caller registers a
        // PMOO-configured variant.
    }

    /// Return the number of registered analyses.
    pub fn count(&self) -> usize {
        self.analyses.len()
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

    /// Run mode-independent analyses once, then run mode-dependent analyses
    /// once per System Operation Mode (SOM).
    ///
    /// Diagnostics from per-SOM analyses are prefixed with `[mode: <name>]`.
    pub fn run_all_per_som(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut all = Vec::new();

        // Run mode-independent analyses once (the normal path).
        all.extend(self.run_all(instance));

        // Run mode-dependent analyses per SOM.
        for som in &instance.system_operation_modes {
            let som_name = &som.name;
            for analysis in &self.analyses {
                if let Some(modal) = analysis.as_modal() {
                    let diags = modal.analyze_in_mode(instance, som);
                    for mut d in diags {
                        d.message = format!("[mode: {som_name}] {}", d.message);
                        all.push(d);
                    }
                }
            }
        }

        all
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
mod regression_tests;
#[cfg(test)]
mod tests;
