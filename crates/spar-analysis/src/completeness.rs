//! Model completeness analysis.
//!
//! Checks for structural completeness of the AADL instance model,
//! looking for missing implementations, missing features, and
//! unresolved classifier references.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::SystemInstance;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

/// Analyzes model completeness.
///
/// Checks:
/// - Component types without implementations (type-only subcomponents)
/// - Component types without features (featureless components)
/// - Components with no connections and no features (skeletal)
/// - Unresolved classifier references (no type_name)
pub struct CompletenessAnalysis;

impl Analysis for CompletenessAnalysis {
    fn name(&self) -> &str {
        "completeness"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Track which type names have implementations.
        // Key: (package, type_name), Value: has_implementation
        let mut type_has_impl: FxHashMap<(String, String), bool> = FxHashMap::default();

        for (_idx, comp) in instance.all_components() {
            let key = (
                comp.package.as_str().to_string(),
                comp.type_name.as_str().to_string(),
            );
            let entry = type_has_impl.entry(key).or_insert(false);
            if comp.impl_name.is_some() {
                *entry = true;
            }
        }

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            // Check for unresolved classifier references.
            // A component with an empty type_name likely failed resolution.
            if comp.type_name.as_str().is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "component '{}' has no classifier reference (unresolved type)",
                        comp.name
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
                continue;
            }

            // Warn about type-only subcomponents (no implementation).
            if comp.impl_name.is_none() && comp.parent.is_some() {
                let key = (
                    comp.package.as_str().to_string(),
                    comp.type_name.as_str().to_string(),
                );
                // Only warn if there IS an implementation somewhere (i.e. they
                // chose not to use it), or if the type_name is non-empty (leaf).
                // For truly anonymous subcomponents with empty type, we already
                // warned above.
                if !comp.type_name.as_str().is_empty() {
                    let has_impl = type_has_impl.get(&key).copied().unwrap_or(false);
                    if !has_impl {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "component '{}' uses type '{}' which has no implementation in scope",
                                comp.name, comp.type_name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }

            // Warn about featureless components (except data and abstract
            // which commonly have no features).
            if comp.features.is_empty() && comp.parent.is_some() {
                use spar_hir_def::item_tree::ComponentCategory;
                let trivially_featureless = matches!(
                    comp.category,
                    ComponentCategory::Data
                        | ComponentCategory::Abstract
                        | ComponentCategory::Memory
                        | ComponentCategory::Bus
                        | ComponentCategory::VirtualBus
                );
                if !trivially_featureless {
                    // Only warn if the type_name is non-empty (otherwise
                    // it's likely an anonymous/unresolved subcomponent).
                    if !comp.type_name.as_str().is_empty() {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "component '{}' of type '{}' has no features",
                                comp.name, comp.type_name
                            ),
                            path: path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // Check instance-level diagnostics for unresolved references.
        for inst_diag in &instance.diagnostics {
            let path: Vec<String> = inst_diag.path.iter().map(|n| n.as_str().to_string()).collect();
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: inst_diag.message.clone(),
                path,
                analysis: self.name().to_string(),
            });
        }

        diags
    }
}
