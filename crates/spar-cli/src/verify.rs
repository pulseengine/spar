//! Requirements verification command.
//!
//! Parses a `requirements.toml` file, runs AADL analyses on the given files,
//! filters diagnostics by analysis name and severity, and produces a pass/fail
//! report with evidence.

use std::{fmt, fs, process};

use serde::{Deserialize, Serialize};
use spar_analysis::{AnalysisDiagnostic, Severity};

use crate::assertion::{Assertion, AssertionResult};

// ── TOML schema ─────────────────────────────────────────────────────

/// Top-level requirements file.
#[derive(Debug, Deserialize)]
pub(crate) struct RequirementsFile {
    #[serde(default)]
    pub requirement: Vec<Requirement>,
    #[serde(default)]
    pub assertion: Vec<Assertion>,
}

/// A single requirement entry from the TOML file.
#[derive(Debug, Deserialize)]
pub(crate) struct Requirement {
    /// Unique requirement identifier, e.g. `"REQ-LATENCY-001"`.
    pub id: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Name of the analysis pass to filter on (e.g. `"latency"`).
    pub analysis: String,
    /// Only count diagnostics at this severity or above.
    #[serde(default = "default_severity")]
    pub severity: SeverityFilter,
    /// Maximum allowed diagnostics that match the filter (default 0 = none allowed).
    #[serde(default)]
    pub max_count: usize,
}

/// Severity filter that deserializes from TOML strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SeverityFilter {
    Error,
    Warning,
    Info,
}

fn default_severity() -> SeverityFilter {
    SeverityFilter::Error
}

impl SeverityFilter {
    /// Returns `true` if `diag_severity` is at or above this filter level.
    pub fn matches(self, diag_severity: Severity) -> bool {
        match self {
            SeverityFilter::Error => diag_severity == Severity::Error,
            SeverityFilter::Warning => matches!(diag_severity, Severity::Error | Severity::Warning),
            SeverityFilter::Info => true,
        }
    }
}

impl fmt::Display for SeverityFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeverityFilter::Error => write!(f, "error"),
            SeverityFilter::Warning => write!(f, "warning"),
            SeverityFilter::Info => write!(f, "info"),
        }
    }
}

// ── Report types ────────────────────────────────────────────────────

/// Outcome of checking one requirement.
#[derive(Debug, Serialize)]
pub(crate) struct RequirementResult {
    pub id: String,
    pub description: String,
    pub analysis: String,
    pub status: Status,
    pub matched_count: usize,
    pub max_count: usize,
    /// Evidence: the matching diagnostics.
    pub evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Pass,
    Fail,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Pass => write!(f, "PASS"),
            Status::Fail => write!(f, "FAIL"),
        }
    }
}

/// One piece of evidence (a matching diagnostic).
#[derive(Debug, Serialize)]
pub(crate) struct EvidenceItem {
    pub severity: String,
    pub message: String,
    pub path: Vec<String>,
}

/// Full verification report.
#[derive(Debug, Serialize)]
pub(crate) struct VerifyReport {
    pub root: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<RequirementResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<AssertionResult>,
}

// ── Core logic ──────────────────────────────────────────────────────

/// Parse a requirements.toml file.
pub(crate) fn parse_requirements(path: &str) -> RequirementsFile {
    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Cannot read {path}: {e}");
        process::exit(1);
    });
    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Cannot parse {path}: {e}");
        process::exit(1);
    })
}

/// Evaluate requirements against a set of diagnostics.
pub(crate) fn evaluate(
    requirements: &[Requirement],
    diagnostics: &[AnalysisDiagnostic],
    root: &str,
) -> VerifyReport {
    let mut results = Vec::new();

    for req in requirements {
        let matching: Vec<&AnalysisDiagnostic> = diagnostics
            .iter()
            .filter(|d| d.analysis == req.analysis && req.severity.matches(d.severity))
            .collect();

        let status = if matching.len() > req.max_count {
            Status::Fail
        } else {
            Status::Pass
        };

        let evidence: Vec<EvidenceItem> = matching
            .iter()
            .map(|d| EvidenceItem {
                severity: format!("{:?}", d.severity).to_lowercase(),
                message: d.message.clone(),
                path: d.path.clone(),
            })
            .collect();

        results.push(RequirementResult {
            id: req.id.clone(),
            description: req.description.clone(),
            analysis: req.analysis.clone(),
            status,
            matched_count: matching.len(),
            max_count: req.max_count,
            evidence,
        });
    }

    let passed = results.iter().filter(|r| r.status == Status::Pass).count();
    let failed = results.iter().filter(|r| r.status == Status::Fail).count();

    VerifyReport {
        root: root.to_string(),
        total: results.len(),
        passed,
        failed,
        results,
        assertions: Vec::new(),
    }
}

/// Format the report as human-readable text and print to stderr/stdout.
pub(crate) fn print_text_report(report: &VerifyReport) {
    if !report.results.is_empty() {
        eprintln!("Requirements verification: {}", report.root);
        eprintln!();

        for result in &report.results {
            let icon = match result.status {
                Status::Pass => "\x1b[1;32mPASS\x1b[0m",
                Status::Fail => "\x1b[1;31mFAIL\x1b[0m",
            };
            eprintln!(
                "  [{}] {} - {} (analysis={}, found={}, max={})",
                icon,
                result.id,
                result.description,
                result.analysis,
                result.matched_count,
                result.max_count,
            );
            for ev in &result.evidence {
                eprintln!(
                    "         [{:>7}] {} (at {})",
                    ev.severity,
                    ev.message,
                    ev.path.join("/")
                );
            }
        }
    }

    if !report.assertions.is_empty() {
        eprintln!();
        eprintln!("Assertions: {}", report.root);
        eprintln!();

        for result in &report.assertions {
            let icon = match result.status {
                Status::Pass => "\x1b[1;32mPASS\x1b[0m",
                Status::Fail => "\x1b[1;31mFAIL\x1b[0m",
            };
            eprintln!(
                "  [{}] {} - {} ({})",
                icon, result.id, result.description, result.detail,
            );
        }
    }

    eprintln!();
    eprintln!(
        "Results: {} passed, {} failed, {} total",
        report.passed, report.failed, report.total,
    );
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(analysis: &str, severity: Severity, message: &str) -> AnalysisDiagnostic {
        AnalysisDiagnostic {
            severity,
            message: message.to_string(),
            path: vec!["root".to_string()],
            analysis: analysis.to_string(),
        }
    }

    #[test]
    fn parse_minimal_toml() {
        let toml_str = r#"
[[requirement]]
id = "REQ-001"
analysis = "latency"
"#;
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.requirement.len(), 1);
        assert_eq!(file.requirement[0].id, "REQ-001");
        assert_eq!(file.requirement[0].analysis, "latency");
        assert_eq!(file.requirement[0].severity, SeverityFilter::Error);
        assert_eq!(file.requirement[0].max_count, 0);
        assert!(file.requirement[0].description.is_empty());
    }

    #[test]
    fn parse_full_toml() {
        let toml_str = r#"
[[requirement]]
id = "REQ-LATENCY-001"
description = "Sensor-to-actuator latency < 20ms"
analysis = "latency"
severity = "error"
max_count = 0

[[requirement]]
id = "REQ-CONN-001"
description = "All ports must be connected"
analysis = "connectivity"
severity = "warning"
max_count = 2
"#;
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.requirement.len(), 2);

        assert_eq!(file.requirement[0].id, "REQ-LATENCY-001");
        assert_eq!(
            file.requirement[0].description,
            "Sensor-to-actuator latency < 20ms"
        );
        assert_eq!(file.requirement[0].analysis, "latency");
        assert_eq!(file.requirement[0].severity, SeverityFilter::Error);
        assert_eq!(file.requirement[0].max_count, 0);

        assert_eq!(file.requirement[1].id, "REQ-CONN-001");
        assert_eq!(file.requirement[1].severity, SeverityFilter::Warning);
        assert_eq!(file.requirement[1].max_count, 2);
    }

    #[test]
    fn parse_empty_file() {
        let toml_str = "";
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert!(file.requirement.is_empty());
    }

    #[test]
    fn severity_filter_error_only_matches_errors() {
        let f = SeverityFilter::Error;
        assert!(f.matches(Severity::Error));
        assert!(!f.matches(Severity::Warning));
        assert!(!f.matches(Severity::Info));
    }

    #[test]
    fn severity_filter_warning_matches_error_and_warning() {
        let f = SeverityFilter::Warning;
        assert!(f.matches(Severity::Error));
        assert!(f.matches(Severity::Warning));
        assert!(!f.matches(Severity::Info));
    }

    #[test]
    fn severity_filter_info_matches_all() {
        let f = SeverityFilter::Info;
        assert!(f.matches(Severity::Error));
        assert!(f.matches(Severity::Warning));
        assert!(f.matches(Severity::Info));
    }

    #[test]
    fn evaluate_pass_no_diagnostics() {
        let reqs = vec![Requirement {
            id: "REQ-001".into(),
            description: "No latency errors".into(),
            analysis: "latency".into(),
            severity: SeverityFilter::Error,
            max_count: 0,
        }];
        let diags = vec![];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.total, 1);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.results[0].status, Status::Pass);
        assert_eq!(report.results[0].matched_count, 0);
    }

    #[test]
    fn evaluate_fail_exceeded_max() {
        let reqs = vec![Requirement {
            id: "REQ-001".into(),
            description: "No latency errors".into(),
            analysis: "latency".into(),
            severity: SeverityFilter::Error,
            max_count: 0,
        }];
        let diags = vec![make_diag("latency", Severity::Error, "too slow")];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.failed, 1);
        assert_eq!(report.results[0].status, Status::Fail);
        assert_eq!(report.results[0].matched_count, 1);
        assert_eq!(report.results[0].evidence.len(), 1);
        assert_eq!(report.results[0].evidence[0].message, "too slow");
    }

    #[test]
    fn evaluate_pass_within_threshold() {
        let reqs = vec![Requirement {
            id: "REQ-CONN".into(),
            description: "Allow up to 2 warnings".into(),
            analysis: "connectivity".into(),
            severity: SeverityFilter::Warning,
            max_count: 2,
        }];
        let diags = vec![
            make_diag("connectivity", Severity::Warning, "unconnected port A"),
            make_diag("connectivity", Severity::Warning, "unconnected port B"),
        ];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.passed, 1);
        assert_eq!(report.results[0].status, Status::Pass);
        assert_eq!(report.results[0].matched_count, 2);
    }

    #[test]
    fn evaluate_filters_by_analysis_name() {
        let reqs = vec![Requirement {
            id: "REQ-001".into(),
            description: "No latency errors".into(),
            analysis: "latency".into(),
            severity: SeverityFilter::Error,
            max_count: 0,
        }];
        // diagnostic from a different analysis should not match
        let diags = vec![make_diag(
            "connectivity",
            Severity::Error,
            "unconnected port",
        )];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.passed, 1);
        assert_eq!(report.results[0].status, Status::Pass);
        assert_eq!(report.results[0].matched_count, 0);
    }

    #[test]
    fn evaluate_filters_by_severity() {
        let reqs = vec![Requirement {
            id: "REQ-001".into(),
            description: "No latency errors".into(),
            analysis: "latency".into(),
            severity: SeverityFilter::Error,
            max_count: 0,
        }];
        // warning from the right analysis should not match error filter
        let diags = vec![make_diag("latency", Severity::Warning, "slow path")];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.passed, 1);
        assert_eq!(report.results[0].status, Status::Pass);
    }

    #[test]
    fn evaluate_multiple_requirements() {
        let reqs = vec![
            Requirement {
                id: "REQ-001".into(),
                description: "No latency errors".into(),
                analysis: "latency".into(),
                severity: SeverityFilter::Error,
                max_count: 0,
            },
            Requirement {
                id: "REQ-002".into(),
                description: "No connectivity errors".into(),
                analysis: "connectivity".into(),
                severity: SeverityFilter::Error,
                max_count: 0,
            },
        ];
        let diags = vec![make_diag("connectivity", Severity::Error, "bad connection")];
        let report = evaluate(&reqs, &diags, "Pkg::Sys.Impl");

        assert_eq!(report.total, 2);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.results[0].status, Status::Pass); // latency: no match
        assert_eq!(report.results[1].status, Status::Fail); // connectivity: 1 > 0
    }

    #[test]
    fn report_serializes_to_json() {
        let report = VerifyReport {
            root: "Pkg::Sys.Impl".into(),
            total: 1,
            passed: 1,
            failed: 0,
            results: vec![RequirementResult {
                id: "REQ-001".into(),
                description: "test".into(),
                analysis: "latency".into(),
                status: Status::Pass,
                matched_count: 0,
                max_count: 0,
                evidence: vec![],
            }],
            assertions: vec![],
        };
        let json = serde_json::to_string_pretty(&report).unwrap();
        assert!(json.contains("\"status\": \"pass\""));
        assert!(json.contains("\"REQ-001\""));
    }

    #[test]
    fn severity_filter_display() {
        assert_eq!(format!("{}", SeverityFilter::Error), "error");
        assert_eq!(format!("{}", SeverityFilter::Warning), "warning");
        assert_eq!(format!("{}", SeverityFilter::Info), "info");
    }

    #[test]
    fn status_display() {
        assert_eq!(format!("{}", Status::Pass), "PASS");
        assert_eq!(format!("{}", Status::Fail), "FAIL");
    }

    #[test]
    fn parse_toml_with_assertions() {
        let toml_str = r#"
[[requirement]]
id = "REQ-001"
analysis = "latency"

[[assertion]]
id = "ASSERT-001"
description = "All threads have Period"
check = "components.where(category == 'thread').all(has('Timing_Properties::Period'))"
severity = "error"

[[assertion]]
id = "ASSERT-002"
description = "At least one thread exists"
check = "components.where(category == 'thread').count()"
"#;
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.requirement.len(), 1);
        assert_eq!(file.assertion.len(), 2);
        assert_eq!(file.assertion[0].id, "ASSERT-001");
        assert_eq!(file.assertion[0].severity, SeverityFilter::Error);
        assert_eq!(file.assertion[1].id, "ASSERT-002");
        // Default severity
        assert_eq!(file.assertion[1].severity, SeverityFilter::Error);
    }

    #[test]
    fn parse_empty_toml_has_no_assertions() {
        let toml_str = "";
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert!(file.requirement.is_empty());
        assert!(file.assertion.is_empty());
    }

    #[test]
    fn parse_toml_assertions_only() {
        let toml_str = r#"
[[assertion]]
id = "ASSERT-001"
check = "components.count()"
"#;
        let file: RequirementsFile = toml::from_str(toml_str).unwrap();
        assert!(file.requirement.is_empty());
        assert_eq!(file.assertion.len(), 1);
    }

    #[test]
    fn report_with_assertions_serializes_to_json() {
        let report = VerifyReport {
            root: "Pkg::Sys.Impl".into(),
            total: 2,
            passed: 1,
            failed: 1,
            results: vec![],
            assertions: vec![
                AssertionResult {
                    id: "ASSERT-001".into(),
                    description: "test assertion".into(),
                    check: "components.count()".into(),
                    severity: "error".into(),
                    status: Status::Pass,
                    detail: "count = 5".into(),
                },
                AssertionResult {
                    id: "ASSERT-002".into(),
                    description: "failing assertion".into(),
                    check: "components.where(category == 'thread').all(has('Timing_Properties::Period'))".into(),
                    severity: "warning".into(),
                    status: Status::Fail,
                    detail: "assertion failed".into(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&report).unwrap();
        assert!(json.contains("\"ASSERT-001\""));
        assert!(json.contains("\"ASSERT-002\""));
        assert!(json.contains("\"assertion failed\""));
        assert!(json.contains("\"count = 5\""));
    }
}
