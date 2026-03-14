//! Connection completeness analysis.
//!
//! Checks that ports have connections and that there are no dangling
//! connection endpoints.

use rustc_hash::FxHashSet;

use spar_hir_def::instance::{FeatureInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::Direction;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Analyzes connection completeness across the instance model.
///
/// Checks:
/// - Every required (in/inout) port has at least one incoming connection
/// - Every outgoing (out/inout) port has at least one connection
/// - Warns about features with no connections at all
pub struct ConnectivityAnalysis;

impl Analysis for ConnectivityAnalysis {
    fn name(&self) -> &str {
        "connectivity"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — unconnected required/provided port, or featureless component with connections
        //   Info    — feature with no direction and no connections
        let mut diags = Vec::new();

        // Collect all feature indices that participate in connections.
        // In the current instance model, connections are declared at the
        // component level but don't carry endpoint feature indices. We
        // use a heuristic: a feature is "connected" if there is at least
        // one connection on the component or its parent.
        //
        // We gather the set of components that *own* connections.
        let mut components_with_connections: FxHashSet<_> = FxHashSet::default();
        for (_idx, conn) in instance.connections.iter() {
            components_with_connections.insert(conn.owner);
        }

        // For each component, check its features.
        for (comp_idx, comp) in instance.all_components() {
            // A feature is "covered" if:
            // - The owning component has connections, OR
            // - The parent component has connections (connections flow between
            //   parent and child features in AADL)
            let owner_has_conns = components_with_connections.contains(&comp_idx);
            let parent_has_conns = comp
                .parent
                .map(|p| components_with_connections.contains(&p))
                .unwrap_or(false);
            let has_conns = owner_has_conns || parent_has_conns;

            // Only check features that are ports (data, event, event data).
            for &feat_idx in &comp.features {
                let feat = &instance.features[feat_idx];

                // Only check directional port-like features.
                if !is_port_feature(feat_idx, instance) {
                    continue;
                }

                if !has_conns {
                    // Check if this feature is annotated as intentionally unconnected
                    // via the SPAR_Properties::Intentionally_Unconnected property.
                    if is_intentionally_unconnected(instance, comp_idx, feat.name.as_str()) {
                        continue;
                    }

                    // No connections at all on this component or parent.
                    let path = component_path(instance, comp_idx);
                    match feat.direction {
                        Some(Direction::In) | Some(Direction::InOut) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "input port '{}' has no incoming connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        Some(Direction::Out) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Warning,
                                message: format!(
                                    "output port '{}' has no outgoing connection",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                        None => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "feature '{}' has no direction and no connections",
                                    feat.name
                                ),
                                path,
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Check for components that have connections but no features at all.
        // This may indicate dangling connections.
        for (comp_idx, comp) in instance.all_components() {
            if !comp.connections.is_empty() {
                // Check children — if a child has no features, connections
                // referencing it are dangling.
                for &child_idx in &comp.children {
                    let child = instance.component(child_idx);
                    if child.features.is_empty() {
                        let path = component_path(instance, child_idx);
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "component '{}' has no features but parent '{}' declares connections",
                                child.name, comp.name
                            ),
                            path,
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // If the component itself has no features and no parent features
                // to connect to, warn.
                if comp.features.is_empty() && comp.children.is_empty() {
                    let path = component_path(instance, comp_idx);
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "component '{}' declares connections but has no features or subcomponents",
                            comp.name
                        ),
                        path,
                        analysis: self.name().to_string(),
                    });
                }
            }
        }

        diags
    }
}

/// Check if a feature is a port-like feature (data port, event port, event data port).
fn is_port_feature(_feat_idx: FeatureInstanceIdx, instance: &SystemInstance) -> bool {
    use spar_hir_def::item_tree::FeatureKind;
    let feat = &instance.features[_feat_idx];
    matches!(
        feat.kind,
        FeatureKind::DataPort | FeatureKind::EventPort | FeatureKind::EventDataPort
    )
}

/// Check if a feature is annotated as intentionally unconnected.
///
/// Looks for the `SPAR_Properties::Intentionally_Unconnected` property on the
/// component that owns the feature. The property value can be:
/// - `"all"` — all ports on the component are intentionally unconnected
/// - `"true"` — same as "all"
/// - A comma-separated list of feature names (case-insensitive), e.g. `"(feat1, feat2)"`
///   or `"feat1, feat2"`
fn is_intentionally_unconnected(
    instance: &SystemInstance,
    comp_idx: spar_hir_def::instance::ComponentInstanceIdx,
    feature_name: &str,
) -> bool {
    let props = instance.properties_for(comp_idx);
    let value = match props.get("SPAR_Properties", "Intentionally_Unconnected") {
        Some(v) => v,
        None => return false,
    };

    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    // "all" or "true" means every port is intentionally unconnected.
    if lower == "all" || lower == "true" {
        return true;
    }

    // Strip optional surrounding parentheses.
    let inner = lower
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(&lower);

    // Split by comma and check if the feature name appears in the list.
    let feat_lower = feature_name.to_ascii_lowercase();
    inner.split(',').any(|item| item.trim() == feat_lower)
}
