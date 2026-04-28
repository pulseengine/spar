//! [`DiscrepancyReport`] container + JSON / text renderers.

use serde::{Deserialize, Serialize};

use crate::discrepancy::{Discrepancy, DiscrepancySeverity, ProbeCoverage, TraceSummary};

/// Top-level output of [`crate::analyze`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscrepancyReport {
    pub trace_summary: TraceSummary,
    pub discrepancies: Vec<Discrepancy>,
    pub coverage: ProbeCoverage,
}

impl DiscrepancyReport {
    /// True iff any discrepancy in the report has Error severity.
    pub fn has_errors(&self) -> bool {
        self.discrepancies
            .iter()
            .any(|d| d.severity == DiscrepancySeverity::Error)
    }

    /// Render as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Render as a human-readable text summary.
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "trace: {} events, {} kernel, {} custom, {}/{} probe enter/exit",
            self.trace_summary.event_count,
            self.trace_summary.kernel_event_count,
            self.trace_summary.custom_event_count,
            self.trace_summary.probe_enter_count,
            self.trace_summary.probe_exit_count,
        );
        let _ = writeln!(
            s,
            "coverage: declared={} observed={} matched={} unobserved={} missing={}",
            self.coverage.declared.len(),
            self.coverage.observed.len(),
            self.coverage.matched.len(),
            self.coverage.unobserved.len(),
            self.coverage.missing.len(),
        );
        if self.discrepancies.is_empty() {
            let _ = writeln!(s, "no discrepancies");
        } else {
            let _ = writeln!(s, "discrepancies:");
            for d in &self.discrepancies {
                let tag = match d.severity {
                    DiscrepancySeverity::Error => "error",
                    DiscrepancySeverity::Warn => "warn ",
                    DiscrepancySeverity::Info => "info ",
                };
                let _ = writeln!(s, "  [{tag}] {}: {}", d.probe_id, d.message);
            }
        }
        s
    }
}
