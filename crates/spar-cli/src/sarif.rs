//! SARIF (Static Analysis Results Interchange Format) output.
//!
//! Produces SARIF v2.1.0 JSON from analysis diagnostics, compatible with
//! GitHub Code Scanning and other SARIF consumers.

use spar_analysis::{AnalysisDiagnostic, Severity};

/// Convert analysis diagnostics into a SARIF v2.1.0 JSON value.
///
/// Each diagnostic maps to a SARIF result:
/// - `analysis` -> `ruleId` (prefixed with `spar/`)
/// - `severity` -> `level` (error/warning/note)
/// - `message` -> `message.text`
/// - `path` -> logical location (component path)
///
/// `files` is the list of analyzed AADL source files, used for artifact references.
pub fn to_sarif(
    diagnostics: &[AnalysisDiagnostic],
    files: &[String],
) -> serde_json::Value {
    let rules = build_rules(diagnostics);
    let results = build_results(diagnostics, files);
    let artifacts = build_artifacts(files);

    serde_json::json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "spar",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/pulseengine/spar",
                    "rules": rules
                }
            },
            "results": results,
            "artifacts": artifacts
        }]
    })
}

/// Build the unique set of SARIF rule descriptors from diagnostics.
fn build_rules(diagnostics: &[AnalysisDiagnostic]) -> Vec<serde_json::Value> {
    let mut seen = std::collections::BTreeSet::new();
    let mut rules = Vec::new();

    for diag in diagnostics {
        let rule_id = format!("spar/{}", diag.analysis);
        if seen.insert(rule_id.clone()) {
            rules.push(serde_json::json!({
                "id": rule_id,
                "shortDescription": {
                    "text": format!("AADL {} analysis", diag.analysis)
                },
                "defaultConfiguration": {
                    "level": severity_to_sarif_level(diag.severity)
                }
            }));
        }
    }

    rules
}

/// Build SARIF result objects from diagnostics.
fn build_results(
    diagnostics: &[AnalysisDiagnostic],
    files: &[String],
) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diag| {
            let rule_id = format!("spar/{}", diag.analysis);
            let level = severity_to_sarif_level(diag.severity);

            // Build logical location from component path
            let logical_location = if diag.path.is_empty() {
                serde_json::json!(null)
            } else {
                serde_json::json!({
                    "fullyQualifiedName": diag.path.join("/"),
                    "kind": "module"
                })
            };

            // We don't have exact file locations from analysis diagnostics,
            // so use logical locations. If we have source files, reference
            // the first one as a fallback physical location.
            let mut locations = Vec::new();
            let mut location = serde_json::Map::new();

            if !diag.path.is_empty() {
                location.insert(
                    "logicalLocations".to_string(),
                    serde_json::json!([logical_location]),
                );
            }

            // Add a physical location referencing the first file if available
            if !files.is_empty() {
                location.insert(
                    "physicalLocation".to_string(),
                    serde_json::json!({
                        "artifactLocation": {
                            "uri": files[0],
                            "index": 0
                        }
                    }),
                );
            }

            if !location.is_empty() {
                locations.push(serde_json::Value::Object(location));
            }

            serde_json::json!({
                "ruleId": rule_id,
                "level": level,
                "message": {
                    "text": diag.message
                },
                "locations": locations
            })
        })
        .collect()
}

/// Build SARIF artifact entries from the file list.
fn build_artifacts(files: &[String]) -> Vec<serde_json::Value> {
    files
        .iter()
        .map(|f| {
            serde_json::json!({
                "location": {
                    "uri": f
                },
                "sourceLanguage": "aadl"
            })
        })
        .collect()
}

/// Map our Severity to SARIF level strings.
fn severity_to_sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(analysis: &str, severity: Severity, message: &str, path: &[&str]) -> AnalysisDiagnostic {
        AnalysisDiagnostic {
            analysis: analysis.to_string(),
            severity,
            message: message.to_string(),
            path: path.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sarif_schema_present() {
        let sarif = to_sarif(&[], &[]);
        assert_eq!(sarif["version"], "2.1.0");
        assert!(sarif["$schema"].as_str().unwrap().contains("sarif-schema-2.1.0"));
    }

    #[test]
    fn sarif_empty_diagnostics() {
        let sarif = to_sarif(&[], &["test.aadl".to_string()]);
        let runs = sarif["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        let results = runs[0]["results"].as_array().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn sarif_basic_diagnostic() {
        let diags = vec![
            make_diag("connectivity", Severity::Error, "unconnected port", &["root", "cpu"]),
        ];
        let files = vec!["model.aadl".to_string()];
        let sarif = to_sarif(&diags, &files);

        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"], "spar/connectivity");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(results[0]["message"]["text"], "unconnected port");

        // Check logical location
        let locs = results[0]["locations"].as_array().unwrap();
        assert_eq!(locs.len(), 1);
        let logical = &locs[0]["logicalLocations"][0];
        assert_eq!(logical["fullyQualifiedName"], "root/cpu");
    }

    #[test]
    fn sarif_severity_mapping() {
        assert_eq!(severity_to_sarif_level(Severity::Error), "error");
        assert_eq!(severity_to_sarif_level(Severity::Warning), "warning");
        assert_eq!(severity_to_sarif_level(Severity::Info), "note");
    }

    #[test]
    fn sarif_rules_deduplication() {
        let diags = vec![
            make_diag("connectivity", Severity::Error, "msg1", &["a"]),
            make_diag("connectivity", Severity::Warning, "msg2", &["b"]),
            make_diag("scheduling", Severity::Info, "msg3", &["c"]),
        ];
        let sarif = to_sarif(&diags, &[]);
        let rules = sarif["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        // Should have 2 unique rules: connectivity and scheduling
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["id"], "spar/connectivity");
        assert_eq!(rules[1]["id"], "spar/scheduling");
    }

    #[test]
    fn sarif_artifacts() {
        let files = vec!["a.aadl".to_string(), "b.aadl".to_string()];
        let sarif = to_sarif(&[], &files);
        let artifacts = sarif["runs"][0]["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0]["location"]["uri"], "a.aadl");
        assert_eq!(artifacts[1]["sourceLanguage"], "aadl");
    }

    #[test]
    fn sarif_multiple_severities() {
        let diags = vec![
            make_diag("hierarchy", Severity::Warning, "deep nesting", &["root", "a", "b"]),
            make_diag("completeness", Severity::Info, "missing binding", &["root"]),
        ];
        let sarif = to_sarif(&diags, &["test.aadl".to_string()]);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["level"], "warning");
        assert_eq!(results[1]["level"], "note");
    }
}
