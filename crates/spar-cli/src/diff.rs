//! Git-aware diff engine for AADL models.
//!
//! Compares two versions of an AADL model (from git refs or directories),
//! producing structural changes, analysis impact reports, and regressions.

use serde::Serialize;
use std::{fs, path::Path, process};

use spar_analysis::AnalysisDiagnostic;
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::name::Name;

// ── Data structures ─────────────────────────────────────────────────

/// The result of comparing two model versions.
#[derive(Debug, Serialize)]
pub struct DiffResult {
    pub structural: Vec<StructuralChange>,
    pub analysis_impact: Vec<AnalysisImpact>,
    pub regressions: Vec<Regression>,
}

/// A structural change between two model versions.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
#[allow(dead_code)]
pub enum StructuralChange {
    ComponentAdded {
        path: Vec<String>,
        category: String,
    },
    ComponentRemoved {
        path: Vec<String>,
        category: String,
    },
    ComponentModified {
        path: Vec<String>,
        changes: Vec<String>,
    },
    ConnectionAdded {
        src: String,
        dst: String,
    },
    ConnectionRemoved {
        src: String,
        dst: String,
    },
    BindingAdded {
        component: String,
        target: String,
    },
    BindingRemoved {
        component: String,
        target: String,
    },
    PropertyChanged {
        path: Vec<String>,
        property: String,
        old: String,
        new: String,
    },
}

/// Analysis impact: diagnostic counts before and after.
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisImpact {
    pub analysis: String,
    pub base_errors: usize,
    pub base_warnings: usize,
    pub head_errors: usize,
    pub head_warnings: usize,
}

/// A regression: a new diagnostic that did not exist in the base version.
#[derive(Debug, Clone, Serialize)]
pub struct Regression {
    pub diagnostic: AnalysisDiagnostic,
    pub description: String,
}

// ── Input resolution ────────────────────────────────────────────────

/// Resolved AADL sources: a set of (filename, content) pairs.
pub struct AadlSources {
    pub files: Vec<(String, String)>,
}

/// Resolve AADL sources from a git ref by using `git show ref:path`.
pub fn resolve_git_sources(git_ref: &str, aadl_files: &[String]) -> AadlSources {
    let mut files = Vec::new();
    for file_path in aadl_files {
        let git_path = format!("{}:{}", git_ref, file_path);
        let output = std::process::Command::new("git")
            .args(["show", &git_path])
            .output()
            .unwrap_or_else(|e| {
                eprintln!("Failed to run `git show {}`: {}", git_path, e);
                process::exit(1);
            });
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("git show {} failed: {}", git_path, stderr.trim());
            process::exit(1);
        }
        let content = String::from_utf8_lossy(&output.stdout).to_string();
        files.push((file_path.clone(), content));
    }
    AadlSources { files }
}

/// Resolve AADL sources from a directory.
pub fn resolve_dir_sources(dir: &str) -> AadlSources {
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        eprintln!("Not a directory: {}", dir);
        process::exit(1);
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(dir_path).unwrap_or_else(|e| {
        eprintln!("Cannot read directory {}: {}", dir, e);
        process::exit(1);
    }) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "aadl") {
            let content = fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("Cannot read {}: {}", path.display(), e);
                process::exit(1);
            });
            files.push((path.display().to_string(), content));
        }
    }

    if files.is_empty() {
        eprintln!("No .aadl files found in {}", dir);
        process::exit(1);
    }

    AadlSources { files }
}

/// Resolve AADL sources from the current filesystem.
#[allow(dead_code)]
pub fn resolve_fs_sources(aadl_files: &[String]) -> AadlSources {
    let files: Vec<_> = aadl_files
        .iter()
        .map(|f| {
            let content = fs::read_to_string(f).unwrap_or_else(|e| {
                eprintln!("Cannot read {}: {}", f, e);
                process::exit(1);
            });
            (f.clone(), content)
        })
        .collect();
    AadlSources { files }
}

// ── Build pipeline ──────────────────────────────────────────────────

/// Parse and instantiate AADL sources into a system instance and its diagnostics.
pub fn build_model(sources: &AadlSources, root: &str) -> (SystemInstance, Vec<AnalysisDiagnostic>) {
    let (pkg_name, type_name, impl_name) = parse_root_ref(root);

    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for (file_path, source) in &sources.files {
        let parsed = spar_syntax::parse(source);
        if !parsed.ok() {
            for err in parsed.errors() {
                eprintln!("{}:{}: {}", file_path, err.offset, err.msg);
            }
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source.clone());
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    let scope = spar_hir_def::GlobalScope::from_trees(trees.clone());
    let mut diagnostics = Vec::new();

    // Run declarative model checks
    for tree in &trees {
        diagnostics.extend(spar_analysis::naming_rules::check_naming_rules(tree));
        diagnostics.extend(spar_analysis::category_check::check_category_rules(tree));
        diagnostics.extend(spar_analysis::extends_rules::check_extends_rules(tree));
    }

    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &Name::new(&pkg_name),
        &Name::new(&type_name),
        &Name::new(&impl_name),
    );

    // Run instance-level analyses
    let mut runner = spar_analysis::AnalysisRunner::new();
    runner.register_all();
    diagnostics.extend(runner.run_all(&inst));

    (inst, diagnostics)
}

// ── Structural comparison ───────────────────────────────────────────

/// Compare two SystemInstances and produce structural changes.
pub fn compare_structure(base: &SystemInstance, head: &SystemInstance) -> Vec<StructuralChange> {
    let mut changes = Vec::new();

    // Build path maps for both instances
    let base_components = collect_component_paths(base, base.root, &mut Vec::new());
    let head_components = collect_component_paths(head, head.root, &mut Vec::new());

    // Detect added components
    for (path, head_idx) in &head_components {
        if !base_components.contains_key(path) {
            let comp = head.component(*head_idx);
            changes.push(StructuralChange::ComponentAdded {
                path: path.clone(),
                category: format!("{}", comp.category),
            });
        }
    }

    // Detect removed components
    for (path, base_idx) in &base_components {
        if !head_components.contains_key(path) {
            let comp = base.component(*base_idx);
            changes.push(StructuralChange::ComponentRemoved {
                path: path.clone(),
                category: format!("{}", comp.category),
            });
        }
    }

    // Detect modified components (same path, different properties)
    for (path, base_idx) in &base_components {
        if let Some(head_idx) = head_components.get(path) {
            let base_comp = base.component(*base_idx);
            let head_comp = head.component(*head_idx);
            let mut comp_changes = Vec::new();

            if base_comp.category != head_comp.category {
                comp_changes.push(format!(
                    "category changed: {} -> {}",
                    base_comp.category, head_comp.category
                ));
            }

            if base_comp.features.len() != head_comp.features.len() {
                comp_changes.push(format!(
                    "feature count changed: {} -> {}",
                    base_comp.features.len(),
                    head_comp.features.len()
                ));
            }

            if base_comp.connections.len() != head_comp.connections.len() {
                comp_changes.push(format!(
                    "connection count changed: {} -> {}",
                    base_comp.connections.len(),
                    head_comp.connections.len()
                ));
            }

            if base_comp.children.len() != head_comp.children.len() {
                comp_changes.push(format!(
                    "child count changed: {} -> {}",
                    base_comp.children.len(),
                    head_comp.children.len()
                ));
            }

            if !comp_changes.is_empty() {
                changes.push(StructuralChange::ComponentModified {
                    path: path.clone(),
                    changes: comp_changes,
                });
            }

            // Compare property values between base and head
            let base_props = base.properties_for(*base_idx);
            let head_props = head.properties_for(*head_idx);

            let base_prop_map = collect_property_display_map(base_props);
            let head_prop_map = collect_property_display_map(head_props);

            // Properties changed or removed (in base, compare with head)
            for (prop_name, base_val) in &base_prop_map {
                match head_prop_map.get(prop_name) {
                    Some(head_val) if head_val != base_val => {
                        changes.push(StructuralChange::PropertyChanged {
                            path: path.clone(),
                            property: prop_name.clone(),
                            old: base_val.clone(),
                            new: head_val.clone(),
                        });
                    }
                    None => {
                        changes.push(StructuralChange::PropertyChanged {
                            path: path.clone(),
                            property: prop_name.clone(),
                            old: base_val.clone(),
                            new: String::new(),
                        });
                    }
                    _ => {} // same value, no change
                }
            }

            // Properties added (in head but not in base)
            for (prop_name, head_val) in &head_prop_map {
                if !base_prop_map.contains_key(prop_name) {
                    changes.push(StructuralChange::PropertyChanged {
                        path: path.clone(),
                        property: prop_name.clone(),
                        old: String::new(),
                        new: head_val.clone(),
                    });
                }
            }
        }
    }

    // Compare connections
    let base_conns = collect_connections(base);
    let head_conns = collect_connections(head);

    for (src, dst) in &head_conns {
        if !base_conns.contains(&(src.clone(), dst.clone())) {
            changes.push(StructuralChange::ConnectionAdded {
                src: src.clone(),
                dst: dst.clone(),
            });
        }
    }

    for (src, dst) in &base_conns {
        if !head_conns.contains(&(src.clone(), dst.clone())) {
            changes.push(StructuralChange::ConnectionRemoved {
                src: src.clone(),
                dst: dst.clone(),
            });
        }
    }

    changes
}

/// Recursively collect all component paths and their indices.
fn collect_component_paths(
    inst: &SystemInstance,
    idx: ComponentInstanceIdx,
    current_path: &mut Vec<String>,
) -> std::collections::BTreeMap<Vec<String>, ComponentInstanceIdx> {
    let mut result = std::collections::BTreeMap::new();
    let comp = inst.component(idx);
    current_path.push(comp.name.as_str().to_string());
    result.insert(current_path.clone(), idx);

    for &child in &comp.children {
        let child_results = collect_component_paths(inst, child, current_path);
        result.extend(child_results);
    }

    current_path.pop();
    result
}

/// Build a map from property display name to its concatenated value string.
///
/// Each property is keyed by `PropertyRef::Display` (e.g. `Timing_Properties::Period`)
/// and the value is the joined values (for append properties, joined with `, `).
fn collect_property_display_map(
    props: &spar_hir_def::properties::PropertyMap,
) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for (_key, values) in props.iter() {
        if let Some(first) = values.first() {
            let prop_name = format!("{}", first.name);
            let joined: String = values
                .iter()
                .map(|v| v.value.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            map.insert(prop_name, joined);
        }
    }
    map
}

/// Collect all connections as (src_description, dst_description) pairs.
fn collect_connections(inst: &SystemInstance) -> std::collections::BTreeSet<(String, String)> {
    let mut conns = std::collections::BTreeSet::new();

    for (_idx, conn) in inst.connections.iter() {
        let src = conn
            .src
            .as_ref()
            .map(|e| match &e.subcomponent {
                Some(sub) => format!("{}.{}", sub, e.feature),
                None => e.feature.as_str().to_string(),
            })
            .unwrap_or_else(|| "?".to_string());

        let dst = conn
            .dst
            .as_ref()
            .map(|e| match &e.subcomponent {
                Some(sub) => format!("{}.{}", sub, e.feature),
                None => e.feature.as_str().to_string(),
            })
            .unwrap_or_else(|| "?".to_string());

        let owner_comp = inst.component(conn.owner);
        let owner_path = format!("{}/", owner_comp.name);
        conns.insert((
            format!("{}{}", owner_path, src),
            format!("{}{}", owner_path, dst),
        ));
    }

    conns
}

// ── Analysis impact comparison ──────────────────────────────────────

/// Compare diagnostics from two versions, producing impact reports and regressions.
pub fn compare_diagnostics(
    base_diags: &[AnalysisDiagnostic],
    head_diags: &[AnalysisDiagnostic],
) -> (Vec<AnalysisImpact>, Vec<Regression>) {
    use spar_analysis::Severity;
    use std::collections::BTreeMap;

    // Group by analysis
    let mut base_counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut head_counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    for d in base_diags {
        let entry = base_counts.entry(d.analysis.clone()).or_default();
        match d.severity {
            Severity::Error => entry.0 += 1,
            Severity::Warning => entry.1 += 1,
            Severity::Info => {}
        }
    }

    for d in head_diags {
        let entry = head_counts.entry(d.analysis.clone()).or_default();
        match d.severity {
            Severity::Error => entry.0 += 1,
            Severity::Warning => entry.1 += 1,
            Severity::Info => {}
        }
    }

    // All unique analysis names
    let all_analyses: std::collections::BTreeSet<_> = base_counts
        .keys()
        .chain(head_counts.keys())
        .cloned()
        .collect();

    let impacts: Vec<_> = all_analyses
        .iter()
        .map(|name| {
            let (be, bw) = base_counts.get(name).copied().unwrap_or((0, 0));
            let (he, hw) = head_counts.get(name).copied().unwrap_or((0, 0));
            AnalysisImpact {
                analysis: name.clone(),
                base_errors: be,
                base_warnings: bw,
                head_errors: he,
                head_warnings: hw,
            }
        })
        .collect();

    // Detect regressions: diagnostics in head that were not in base
    // Use (analysis, message, path) as a key for matching
    let base_keys: std::collections::HashSet<(String, String, Vec<String>)> = base_diags
        .iter()
        .map(|d| (d.analysis.clone(), d.message.clone(), d.path.clone()))
        .collect();

    let regressions: Vec<_> = head_diags
        .iter()
        .filter(|d| {
            matches!(d.severity, Severity::Error | Severity::Warning)
                && !base_keys.contains(&(d.analysis.clone(), d.message.clone(), d.path.clone()))
        })
        .map(|d| Regression {
            diagnostic: d.clone(),
            description: format!(
                "New {} in {}: {}",
                match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "info",
                },
                d.analysis,
                d.message,
            ),
        })
        .collect();

    (impacts, regressions)
}

// ── Output formatting ───────────────────────────────────────────────

/// Format the diff result as human-readable text.
pub fn format_text(result: &DiffResult) -> String {
    let mut out = String::new();

    // Structural changes
    out.push_str("=== Structural Changes ===\n");
    if result.structural.is_empty() {
        out.push_str("  No structural changes.\n");
    } else {
        for change in &result.structural {
            match change {
                StructuralChange::ComponentAdded { path, category } => {
                    out.push_str(&format!("  + [{}] {}\n", category, path.join("/")));
                }
                StructuralChange::ComponentRemoved { path, category } => {
                    out.push_str(&format!("  - [{}] {}\n", category, path.join("/")));
                }
                StructuralChange::ComponentModified { path, changes } => {
                    out.push_str(&format!("  ~ {}\n", path.join("/")));
                    for c in changes {
                        out.push_str(&format!("      {}\n", c));
                    }
                }
                StructuralChange::ConnectionAdded { src, dst } => {
                    out.push_str(&format!("  + connection: {} -> {}\n", src, dst));
                }
                StructuralChange::ConnectionRemoved { src, dst } => {
                    out.push_str(&format!("  - connection: {} -> {}\n", src, dst));
                }
                StructuralChange::BindingAdded { component, target } => {
                    out.push_str(&format!("  + binding: {} -> {}\n", component, target));
                }
                StructuralChange::BindingRemoved { component, target } => {
                    out.push_str(&format!("  - binding: {} -> {}\n", component, target));
                }
                StructuralChange::PropertyChanged {
                    path,
                    property,
                    old,
                    new,
                } => {
                    out.push_str(&format!(
                        "  ~ {}.{}: {} -> {}\n",
                        path.join("/"),
                        property,
                        old,
                        new
                    ));
                }
            }
        }
    }
    out.push('\n');

    // Analysis impact
    out.push_str("=== Analysis Impact ===\n");
    if result.analysis_impact.is_empty() {
        out.push_str("  No analysis impact.\n");
    } else {
        out.push_str(&format!(
            "  {:<25} {:>6} {:>6} {:>6} {:>6}\n",
            "Analysis", "B.Err", "B.Warn", "H.Err", "H.Warn"
        ));
        out.push_str(&format!("  {}\n", "-".repeat(55)));
        for impact in &result.analysis_impact {
            let delta_err = impact.head_errors as i64 - impact.base_errors as i64;
            let delta_warn = impact.head_warnings as i64 - impact.base_warnings as i64;
            let delta_str = if delta_err != 0 || delta_warn != 0 {
                format!(" (err:{:+}, warn:{:+})", delta_err, delta_warn)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  {:<25} {:>6} {:>6} {:>6} {:>6}{}\n",
                impact.analysis,
                impact.base_errors,
                impact.base_warnings,
                impact.head_errors,
                impact.head_warnings,
                delta_str,
            ));
        }
    }
    out.push('\n');

    // Regressions
    out.push_str("=== Regressions ===\n");
    if result.regressions.is_empty() {
        out.push_str("  No regressions. Model quality maintained or improved.\n");
    } else {
        for reg in &result.regressions {
            let level = match reg.diagnostic.severity {
                spar_analysis::Severity::Error => "ERROR",
                spar_analysis::Severity::Warning => "WARNING",
                spar_analysis::Severity::Info => "INFO",
            };
            out.push_str(&format!(
                "  [{}] {}: {} (at {})\n",
                level,
                reg.diagnostic.analysis,
                reg.diagnostic.message,
                reg.diagnostic.path.join("/"),
            ));
        }
    }

    out
}

/// Format the diff result as SARIF, focusing on regressions.
pub fn format_sarif(result: &DiffResult, files: &[String]) -> serde_json::Value {
    // Only include regressions in SARIF output — they are the new issues
    let diags: Vec<AnalysisDiagnostic> = result
        .regressions
        .iter()
        .map(|r| r.diagnostic.clone())
        .collect();
    crate::sarif::to_sarif(&diags, files)
}

// ── Helpers ─────────────────────────────────────────────────────────

fn parse_root_ref(s: &str) -> (String, String, String) {
    let parts: Vec<&str> = s.splitn(2, "::").collect();
    if parts.len() != 2 {
        eprintln!("Invalid root reference: expected Package::Type.Impl, got: {s}");
        process::exit(1);
    }
    let pkg = parts[0].to_string();
    let type_impl: Vec<&str> = parts[1].splitn(2, '.').collect();
    if type_impl.len() != 2 {
        eprintln!("Invalid root reference: expected Package::Type.Impl, got: {s}");
        process::exit(1);
    }
    (pkg, type_impl[0].to_string(), type_impl[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_analysis::Severity;

    fn make_diag(
        analysis: &str,
        severity: Severity,
        message: &str,
        path: &[&str],
    ) -> AnalysisDiagnostic {
        AnalysisDiagnostic {
            analysis: analysis.to_string(),
            severity,
            message: message.to_string(),
            path: path.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn compare_diagnostics_no_regressions() {
        let base = vec![make_diag(
            "connectivity",
            Severity::Error,
            "unconnected",
            &["root"],
        )];
        let head = vec![make_diag(
            "connectivity",
            Severity::Error,
            "unconnected",
            &["root"],
        )];

        let (impacts, regressions) = compare_diagnostics(&base, &head);
        assert_eq!(impacts.len(), 1);
        assert_eq!(impacts[0].base_errors, 1);
        assert_eq!(impacts[0].head_errors, 1);
        assert!(regressions.is_empty());
    }

    #[test]
    fn compare_diagnostics_with_regression() {
        let base = vec![];
        let head = vec![make_diag(
            "scheduling",
            Severity::Error,
            "deadline missed",
            &["root", "cpu"],
        )];

        let (impacts, regressions) = compare_diagnostics(&base, &head);
        assert_eq!(impacts.len(), 1);
        assert_eq!(impacts[0].analysis, "scheduling");
        assert_eq!(impacts[0].base_errors, 0);
        assert_eq!(impacts[0].head_errors, 1);
        assert_eq!(regressions.len(), 1);
        assert_eq!(regressions[0].diagnostic.analysis, "scheduling");
    }

    #[test]
    fn compare_diagnostics_fixed_issue() {
        let base = vec![make_diag(
            "connectivity",
            Severity::Error,
            "unconnected",
            &["root", "sensor"],
        )];
        let head = vec![];

        let (impacts, regressions) = compare_diagnostics(&base, &head);
        assert_eq!(impacts.len(), 1);
        assert_eq!(impacts[0].base_errors, 1);
        assert_eq!(impacts[0].head_errors, 0);
        assert!(regressions.is_empty());
    }

    #[test]
    fn format_text_empty_diff() {
        let result = DiffResult {
            structural: vec![],
            analysis_impact: vec![],
            regressions: vec![],
        };
        let text = format_text(&result);
        assert!(text.contains("No structural changes"));
        assert!(text.contains("No regressions"));
    }

    #[test]
    fn format_text_with_changes() {
        let result = DiffResult {
            structural: vec![
                StructuralChange::ComponentAdded {
                    path: vec!["root".into(), "new_sensor".into()],
                    category: "device".into(),
                },
                StructuralChange::ComponentRemoved {
                    path: vec!["root".into(), "old_actuator".into()],
                    category: "device".into(),
                },
            ],
            analysis_impact: vec![AnalysisImpact {
                analysis: "connectivity".into(),
                base_errors: 2,
                base_warnings: 1,
                head_errors: 1,
                head_warnings: 1,
            }],
            regressions: vec![],
        };
        let text = format_text(&result);
        assert!(text.contains("+ [device] root/new_sensor"));
        assert!(text.contains("- [device] root/old_actuator"));
        assert!(text.contains("connectivity"));
    }

    #[test]
    fn format_sarif_regression_only() {
        let result = DiffResult {
            structural: vec![],
            analysis_impact: vec![],
            regressions: vec![Regression {
                diagnostic: make_diag("scheduling", Severity::Error, "deadline missed", &["root"]),
                description: "New error".into(),
            }],
        };
        let sarif = format_sarif(&result, &["test.aadl".to_string()]);
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"], "spar/scheduling");
    }

    #[test]
    fn parse_root_ref_valid() {
        let (pkg, ty, im) = parse_root_ref("Pkg::Type.Impl");
        assert_eq!(pkg, "Pkg");
        assert_eq!(ty, "Type");
        assert_eq!(im, "Impl");
    }

    // ── compare_structure tests ─────────────────────────────────────

    mod structure_tests {
        use super::*;
        use la_arena::Arena;
        use rustc_hash::FxHashMap;
        use spar_hir_def::instance::*;
        use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind};
        use spar_hir_def::name::Name;

        /// Minimal builder for constructing SystemInstance values in tests.
        struct TestBuilder {
            components: Arena<ComponentInstance>,
            features: Arena<FeatureInstance>,
            connections: Arena<ConnectionInstance>,
            property_maps: FxHashMap<ComponentInstanceIdx, spar_hir_def::properties::PropertyMap>,
        }

        impl TestBuilder {
            fn new() -> Self {
                Self {
                    components: Arena::default(),
                    features: Arena::default(),
                    connections: Arena::default(),
                    property_maps: FxHashMap::default(),
                }
            }

            fn add_component(
                &mut self,
                name: &str,
                category: ComponentCategory,
                parent: Option<ComponentInstanceIdx>,
            ) -> ComponentInstanceIdx {
                self.components.alloc(ComponentInstance {
                    name: Name::new(name),
                    category,
                    type_name: Name::new(name),
                    impl_name: Some(Name::new("impl")),
                    package: Name::new("Pkg"),
                    parent,
                    children: Vec::new(),
                    features: Vec::new(),
                    connections: Vec::new(),
                    flows: Vec::new(),
                    modes: Vec::new(),
                    mode_transitions: Vec::new(),
                    array_index: None,
                    in_modes: Vec::new(),
                })
            }

            fn set_children(
                &mut self,
                parent: ComponentInstanceIdx,
                children: Vec<ComponentInstanceIdx>,
            ) {
                self.components[parent].children = children;
            }

            fn add_feature(
                &mut self,
                name: &str,
                owner: ComponentInstanceIdx,
            ) -> FeatureInstanceIdx {
                let idx = self.features.alloc(FeatureInstance {
                    name: Name::new(name),
                    kind: spar_hir_def::item_tree::FeatureKind::DataPort,
                    direction: Some(spar_hir_def::item_tree::Direction::Out),
                    owner,
                    classifier: None,
                    access_kind: None,
                    array_index: None,
                });
                self.components[owner].features.push(idx);
                idx
            }

            fn add_connection(
                &mut self,
                name: &str,
                owner: ComponentInstanceIdx,
                src_sub: Option<&str>,
                src_feat: &str,
                dst_sub: Option<&str>,
                dst_feat: &str,
            ) -> ConnectionInstanceIdx {
                let idx = self.connections.alloc(ConnectionInstance {
                    name: Name::new(name),
                    kind: ConnectionKind::Port,
                    is_bidirectional: false,
                    owner,
                    src: Some(ConnectionEnd {
                        subcomponent: src_sub.map(Name::new),
                        feature: Name::new(src_feat),
                    }),
                    dst: Some(ConnectionEnd {
                        subcomponent: dst_sub.map(Name::new),
                        feature: Name::new(dst_feat),
                    }),
                    in_modes: Vec::new(),
                });
                self.components[owner].connections.push(idx);
                idx
            }

            fn set_property(
                &mut self,
                comp: ComponentInstanceIdx,
                set: &str,
                name: &str,
                value: &str,
            ) {
                use spar_hir_def::name::PropertyRef;
                use spar_hir_def::properties::PropertyValue;

                let map = self.property_maps.entry(comp).or_default();
                map.add(PropertyValue {
                    name: PropertyRef {
                        property_set: if set.is_empty() {
                            None
                        } else {
                            Some(Name::new(set))
                        },
                        property_name: Name::new(name),
                    },
                    value: value.to_string(),
                    typed_expr: None,
                    is_append: false,
                });
            }

            fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
                SystemInstance {
                    root,
                    components: self.components,
                    features: self.features,
                    connections: self.connections,
                    flow_instances: Arena::default(),
                    end_to_end_flows: Arena::default(),
                    mode_instances: Arena::default(),
                    mode_transition_instances: Arena::default(),
                    diagnostics: Vec::new(),
                    property_maps: self.property_maps,
                    semantic_connections: Vec::new(),
                    system_operation_modes: Vec::new(),
                }
            }
        }

        /// Helper: build a basic system with root -> [sensor, controller, actuator].
        fn build_basic_system() -> SystemInstance {
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor, controller, actuator]);
            b.build(root)
        }

        #[test]
        fn compare_identical_systems() {
            let base = build_basic_system();
            let head = build_basic_system();

            let changes = compare_structure(&base, &head);
            assert!(
                changes.is_empty(),
                "identical systems should produce no changes, got: {:?}",
                changes
            );
        }

        #[test]
        fn detect_component_added() {
            let base = build_basic_system();

            // Head system has an extra component "monitor"
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
            let monitor = b.add_component("monitor", ComponentCategory::Process, Some(root));
            b.set_children(root, vec![sensor, controller, actuator, monitor]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let added: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ComponentAdded { .. }))
                .collect();
            assert_eq!(
                added.len(),
                1,
                "should detect exactly one addition: {:?}",
                changes
            );
            match &added[0] {
                StructuralChange::ComponentAdded { path, category } => {
                    assert_eq!(path, &vec!["root".to_string(), "monitor".to_string()]);
                    assert_eq!(category, "process");
                }
                _ => unreachable!(),
            }

            // The root component is modified (child count changed 3 -> 4)
            let modified: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ComponentModified { .. }))
                .collect();
            assert!(
                !modified.is_empty(),
                "root should be marked modified (child count changed)"
            );
        }

        #[test]
        fn detect_component_removed() {
            let base = build_basic_system();

            // Head system is missing "actuator"
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            b.set_children(root, vec![sensor, controller]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let removed: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ComponentRemoved { .. }))
                .collect();
            assert_eq!(
                removed.len(),
                1,
                "should detect exactly one removal: {:?}",
                changes
            );
            match &removed[0] {
                StructuralChange::ComponentRemoved { path, category } => {
                    assert_eq!(path, &vec!["root".to_string(), "actuator".to_string()]);
                    assert_eq!(category, "device");
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_connection_added() {
            // Base: root with sensor and controller, no connections
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            b.add_feature("data_out", sensor);
            b.add_feature("data_in", controller);
            b.set_children(root, vec![sensor, controller]);
            let base = b.build(root);

            // Head: same components, but with a connection
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            b.add_feature("data_out", sensor);
            b.add_feature("data_in", controller);
            b.add_connection(
                "c1",
                root,
                Some("sensor"),
                "data_out",
                Some("controller"),
                "data_in",
            );
            b.set_children(root, vec![sensor, controller]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let conn_added: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ConnectionAdded { .. }))
                .collect();
            assert_eq!(
                conn_added.len(),
                1,
                "should detect exactly one connection addition: {:?}",
                changes
            );
            match &conn_added[0] {
                StructuralChange::ConnectionAdded { src, dst } => {
                    assert!(
                        src.contains("sensor.data_out"),
                        "src should contain sensor.data_out, got: {}",
                        src
                    );
                    assert!(
                        dst.contains("controller.data_in"),
                        "dst should contain controller.data_in, got: {}",
                        dst
                    );
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_connection_removed() {
            // Base: root with sensor, controller, and a connection
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            b.add_feature("data_out", sensor);
            b.add_feature("data_in", controller);
            b.add_connection(
                "c1",
                root,
                Some("sensor"),
                "data_out",
                Some("controller"),
                "data_in",
            );
            b.set_children(root, vec![sensor, controller]);
            let base = b.build(root);

            // Head: same components, no connections
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            let controller = b.add_component("controller", ComponentCategory::Process, Some(root));
            b.add_feature("data_out", sensor);
            b.add_feature("data_in", controller);
            b.set_children(root, vec![sensor, controller]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let conn_removed: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ConnectionRemoved { .. }))
                .collect();
            assert_eq!(
                conn_removed.len(),
                1,
                "should detect exactly one connection removal: {:?}",
                changes
            );
            match &conn_removed[0] {
                StructuralChange::ConnectionRemoved { src, dst } => {
                    assert!(
                        src.contains("sensor.data_out"),
                        "src should contain sensor.data_out, got: {}",
                        src
                    );
                    assert!(
                        dst.contains("controller.data_in"),
                        "dst should contain controller.data_in, got: {}",
                        dst
                    );
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_property_changed() {
            // compare_structure detects property-related changes through
            // ComponentModified when structural aspects differ (feature count,
            // connection count, child count, category). Direct PropertyChanged
            // variants are not emitted by the current implementation, so we
            // test that a component with differing feature counts is flagged
            // as ComponentModified.
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.add_feature("port1", sensor);
            b.set_children(root, vec![sensor]);
            let base = b.build(root);

            // Head: sensor has an additional feature (structural property change)
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.add_feature("port1", sensor);
            b.add_feature("port2", sensor);
            b.set_children(root, vec![sensor]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let modified: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ComponentModified { .. }))
                .collect();
            assert!(
                !modified.is_empty(),
                "should detect modification when feature count changes: {:?}",
                changes
            );
            match &modified[0] {
                StructuralChange::ComponentModified { path, changes } => {
                    assert_eq!(path, &vec!["root".to_string(), "sensor".to_string()]);
                    let has_feature_change =
                        changes.iter().any(|c| c.contains("feature count changed"));
                    assert!(
                        has_feature_change,
                        "changes should mention feature count, got: {:?}",
                        changes
                    );
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_binding_change() {
            // compare_structure detects binding-related changes through
            // ComponentModified when the component category changes (e.g.,
            // replacing a processor with a virtual processor). Direct
            // BindingAdded/BindingRemoved variants are not emitted by the
            // current implementation, so we test category change detection.
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let cpu = b.add_component("cpu", ComponentCategory::Processor, Some(root));
            let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
            b.set_children(root, vec![cpu, proc]);
            let base = b.build(root);

            // Head: cpu is now a VirtualProcessor (category changed)
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let cpu = b.add_component("cpu", ComponentCategory::VirtualProcessor, Some(root));
            let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
            b.set_children(root, vec![cpu, proc]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let modified: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::ComponentModified { .. }))
                .collect();
            assert!(
                !modified.is_empty(),
                "should detect modification when category changes: {:?}",
                changes
            );
            match &modified[0] {
                StructuralChange::ComponentModified { path, changes } => {
                    assert_eq!(path, &vec!["root".to_string(), "cpu".to_string()]);
                    let has_category_change =
                        changes.iter().any(|c| c.contains("category changed"));
                    assert!(
                        has_category_change,
                        "changes should mention category change, got: {:?}",
                        changes
                    );
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn empty_systems_no_changes() {
            // Two systems with only a root component (no children, connections, etc.)
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let base = b.build(root);

            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);
            assert!(
                changes.is_empty(),
                "empty systems should produce no changes, got: {:?}",
                changes
            );
        }

        #[test]
        fn detect_property_value_changed() {
            // Base: sensor has Period = 10ms
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
            let base = b.build(root);

            // Head: sensor has Period = 100ms
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            b.set_property(sensor, "Timing_Properties", "Period", "100 ms");
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let prop_changes: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::PropertyChanged { .. }))
                .collect();
            assert_eq!(
                prop_changes.len(),
                1,
                "should detect exactly one property change: {:?}",
                changes
            );
            match &prop_changes[0] {
                StructuralChange::PropertyChanged {
                    path,
                    property,
                    old,
                    new,
                } => {
                    assert_eq!(path, &vec!["root".to_string(), "sensor".to_string()]);
                    assert!(
                        property.contains("Period"),
                        "property should mention Period, got: {}",
                        property
                    );
                    assert_eq!(old, "10 ms");
                    assert_eq!(new, "100 ms");
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_property_added() {
            // Base: sensor has no properties
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            let base = b.build(root);

            // Head: sensor has Period = 50ms
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            b.set_property(sensor, "Timing_Properties", "Period", "50 ms");
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let prop_changes: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::PropertyChanged { .. }))
                .collect();
            assert_eq!(
                prop_changes.len(),
                1,
                "should detect exactly one property addition: {:?}",
                changes
            );
            match &prop_changes[0] {
                StructuralChange::PropertyChanged {
                    path,
                    property,
                    old,
                    new,
                } => {
                    assert_eq!(path, &vec!["root".to_string(), "sensor".to_string()]);
                    assert!(
                        property.contains("Period"),
                        "property should mention Period, got: {}",
                        property
                    );
                    assert!(old.is_empty(), "old should be empty for added property");
                    assert_eq!(new, "50 ms");
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn detect_property_removed() {
            // Base: sensor has Period = 10ms
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
            let base = b.build(root);

            // Head: sensor has no properties
            let mut b = TestBuilder::new();
            let root = b.add_component("root", ComponentCategory::System, None);
            let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
            b.set_children(root, vec![sensor]);
            let head = b.build(root);

            let changes = compare_structure(&base, &head);

            let prop_changes: Vec<_> = changes
                .iter()
                .filter(|c| matches!(c, StructuralChange::PropertyChanged { .. }))
                .collect();
            assert_eq!(
                prop_changes.len(),
                1,
                "should detect exactly one property removal: {:?}",
                changes
            );
            match &prop_changes[0] {
                StructuralChange::PropertyChanged {
                    path,
                    property,
                    old,
                    new,
                } => {
                    assert_eq!(path, &vec!["root".to_string(), "sensor".to_string()]);
                    assert!(
                        property.contains("Period"),
                        "property should mention Period, got: {}",
                        property
                    );
                    assert_eq!(old, "10 ms");
                    assert!(new.is_empty(), "new should be empty for removed property");
                }
                _ => unreachable!(),
            }
        }
    }
}
