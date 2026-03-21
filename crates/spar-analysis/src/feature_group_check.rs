//! Feature group connection validation (AS5506 sections 8.6, 9.2).
//!
//! Validates:
//! - **Complement check**: When two feature groups are connected, they must be
//!   complements. For each feature in the source FG, there must be a matching
//!   feature in the destination FG with the same name and opposite direction.
//! - **Inverse of check**: If a feature group type is declared as `inverse of`
//!   another FGT, it must not also declare explicit features (they are inherited
//!   with flipped directions).
//! - **Feature group connection expansion tracking**: Reports diagnostics for
//!   feature group connections that could not be expanded.

use spar_hir_def::feature_group::{ExpandedFeature, expand_feature_group, flip_direction};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::{ConnectionKind, Direction, FeatureKind, ItemTree};
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Instance-level feature group connection validation.
///
/// Checks that every feature group connection references features that
/// actually exist on the connected components and are of kind
/// [`FeatureKind::FeatureGroup`]. Complement validation (which requires
/// a `GlobalScope`) is handled separately by [`check_feature_group_complements`].
pub struct FeatureGroupCheckAnalysis;

impl Analysis for FeatureGroupCheckAnalysis {
    fn name(&self) -> &str {
        "feature_group_check"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (_conn_idx, conn) in instance.connections.iter() {
            if conn.kind != ConnectionKind::FeatureGroup {
                continue;
            }

            let (src_end, dst_end) = match (&conn.src, &conn.dst) {
                (Some(s), Some(d)) => (s, d),
                _ => {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "feature group connection '{}' is missing an endpoint",
                            conn.name
                        ),
                        path: component_path(instance, conn.owner),
                        analysis: "feature_group_check".to_string(),
                    });
                    continue;
                }
            };

            // Resolve source and destination components.
            let src_comp = resolve_endpoint_component(instance, conn.owner, &src_end.subcomponent);
            let dst_comp = resolve_endpoint_component(instance, conn.owner, &dst_end.subcomponent);

            // Check that the referenced features are actually FeatureGroups.
            if let Some(src_idx) = src_comp {
                let comp = instance.component(src_idx);
                let is_fg = comp.features.iter().any(|&fi| {
                    let feat = &instance.features[fi];
                    feat.name.eq_ci(&src_end.feature) && feat.kind == FeatureKind::FeatureGroup
                });
                if !is_fg && !comp.features.is_empty() {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "feature group connection '{}': source feature '{}' is not a feature group",
                            conn.name, src_end.feature
                        ),
                        path: component_path(instance, conn.owner),
                        analysis: "feature_group_check".to_string(),
                    });
                }
            }

            if let Some(dst_idx) = dst_comp {
                let comp = instance.component(dst_idx);
                let is_fg = comp.features.iter().any(|&fi| {
                    let feat = &instance.features[fi];
                    feat.name.eq_ci(&dst_end.feature) && feat.kind == FeatureKind::FeatureGroup
                });
                if !is_fg && !comp.features.is_empty() {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "feature group connection '{}': destination feature '{}' is not a feature group",
                            conn.name, dst_end.feature
                        ),
                        path: component_path(instance, conn.owner),
                        analysis: "feature_group_check".to_string(),
                    });
                }
            }
        }

        diags
    }
}

// ── Feature Group Complement Validation (instance-level) ────────────

/// Result of validating complement compatibility between two feature groups.
#[derive(Debug)]
pub struct ComplementCheckResult {
    /// Features in the source that have no matching feature in the destination.
    pub unmatched_source: Vec<ExpandedFeature>,
    /// Features where directions are not complementary.
    pub direction_mismatches: Vec<DirectionMismatch>,
}

/// A single direction mismatch between a source and destination feature.
#[derive(Debug)]
pub struct DirectionMismatch {
    pub feature_name: Name,
    pub source_direction: Option<Direction>,
    pub destination_direction: Option<Direction>,
}

/// Validate that two sets of expanded features are complements (AS5506 section 8.6).
///
/// For each feature in `source`, there must be a feature in `destination` with:
/// - The same name (case-insensitive)
/// - The opposite direction (out -> in, in -> out, in out -> in out)
///
/// Features without direction (abstract features) are not checked for direction
/// compatibility.
pub fn validate_complement(
    source: &[ExpandedFeature],
    destination: &[ExpandedFeature],
) -> ComplementCheckResult {
    let mut unmatched_source = Vec::new();
    let mut direction_mismatches = Vec::new();

    for src_feat in source {
        // Find matching feature by name in destination.
        let dst_feat = destination.iter().find(|d| d.name.eq_ci(&src_feat.name));

        match dst_feat {
            None => {
                unmatched_source.push(src_feat.clone());
            }
            Some(dst) => {
                // Check direction complementarity.
                if let (Some(src_dir), Some(dst_dir)) = (src_feat.direction, dst.direction) {
                    let expected = flip_direction(src_dir);
                    if expected != dst_dir {
                        direction_mismatches.push(DirectionMismatch {
                            feature_name: src_feat.name.clone(),
                            source_direction: Some(src_dir),
                            destination_direction: Some(dst_dir),
                        });
                    }
                }
            }
        }
    }

    ComplementCheckResult {
        unmatched_source,
        direction_mismatches,
    }
}

// ── Inverse-of validation (ItemTree-level) ──────────────────────────

/// Validate `inverse of` declarations on feature group types (AS5506 section 8.6).
///
/// If a feature group type declares `inverse of AnotherFGT`, it should not also
/// declare explicit features (it inherits the other FGT's features with flipped
/// directions). Having both is an error.
pub fn check_inverse_of_rules(tree: &ItemTree) -> Vec<AnalysisDiagnostic> {
    let mut diags = Vec::new();

    for (_idx, fgt) in tree.feature_group_types.iter() {
        if fgt.inverse_of.is_some() && !fgt.features.is_empty() {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "feature group type '{}' declares 'inverse of' but also has {} explicit \
                     feature(s); inverse-of types inherit features with flipped directions \
                     and must not declare their own",
                    fgt.name,
                    fgt.features.len()
                ),
                path: vec![fgt.name.as_str().to_string()],
                analysis: "feature_group_check".to_string(),
            });
        }
    }

    diags
}

// ── Feature group connection complement validation (instance-level with scope) ──

/// Validate feature group connections for complement compatibility.
///
/// For each feature group connection in the instance model, expand both sides
/// and check that they are complementary (matching feature names with opposite
/// directions). This requires access to the GlobalScope for name resolution.
pub fn check_feature_group_complements(
    instance: &SystemInstance,
    scope: &GlobalScope,
) -> Vec<AnalysisDiagnostic> {
    let mut diags = Vec::new();

    for (_conn_idx, conn) in instance.connections.iter() {
        // Only check feature group connections.
        if conn.kind != ConnectionKind::FeatureGroup {
            continue;
        }

        let (src_end, dst_end) = match (&conn.src, &conn.dst) {
            (Some(s), Some(d)) => (s, d),
            _ => continue,
        };

        let owner = conn.owner;

        // Resolve source and destination components.
        let src_comp_idx = resolve_endpoint_component(instance, owner, &src_end.subcomponent);
        let dst_comp_idx = resolve_endpoint_component(instance, owner, &dst_end.subcomponent);

        let (src_comp_idx, dst_comp_idx) = match (src_comp_idx, dst_comp_idx) {
            (Some(s), Some(d)) => (s, d),
            _ => continue,
        };

        // Expand the feature groups.
        let src_expanded =
            expand_component_feature_group(instance, scope, src_comp_idx, &src_end.feature);
        let dst_expanded =
            expand_component_feature_group(instance, scope, dst_comp_idx, &dst_end.feature);

        let (src_features, dst_features) = match (src_expanded, dst_expanded) {
            (Some(s), Some(d)) => (s, d),
            _ => continue,
        };

        // Validate complement relationship.
        let result = validate_complement(&src_features, &dst_features);

        let conn_path = build_connection_path(instance, owner, &conn.name);

        for unmatched in &result.unmatched_source {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "feature group connection '{}': source feature '{}' has no matching \
                     feature in destination feature group",
                    conn.name, unmatched.name
                ),
                path: conn_path.clone(),
                analysis: "feature_group_check".to_string(),
            });
        }

        for mismatch in &result.direction_mismatches {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "feature group connection '{}': feature '{}' has incompatible directions \
                     (source: {}, destination: {}, expected destination: {})",
                    conn.name,
                    mismatch.feature_name,
                    mismatch
                        .source_direction
                        .map_or("none".to_string(), |d| d.to_string()),
                    mismatch
                        .destination_direction
                        .map_or("none".to_string(), |d| d.to_string()),
                    mismatch
                        .source_direction
                        .map_or("none".to_string(), |d| flip_direction(d).to_string()),
                ),
                path: conn_path.clone(),
                analysis: "feature_group_check".to_string(),
            });
        }
    }

    diags
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Resolve the component index for a connection endpoint.
fn resolve_endpoint_component(
    instance: &SystemInstance,
    owner: spar_hir_def::instance::ComponentInstanceIdx,
    subcomponent: &Option<Name>,
) -> Option<spar_hir_def::instance::ComponentInstanceIdx> {
    match subcomponent {
        Some(sub_name) => {
            let owner_comp = instance.component(owner);
            owner_comp
                .children
                .iter()
                .find(|&&child_idx| instance.component(child_idx).name.eq_ci(sub_name))
                .copied()
        }
        None => Some(owner),
    }
}

/// Expand a feature group on a component instance using the GlobalScope.
fn expand_component_feature_group(
    instance: &SystemInstance,
    scope: &GlobalScope,
    component: spar_hir_def::instance::ComponentInstanceIdx,
    feature_name: &Name,
) -> Option<Vec<ExpandedFeature>> {
    use spar_hir_def::name::ClassifierRef;
    use spar_hir_def::resolver::ResolvedClassifier;

    let comp = instance.component(component);

    // Check if the feature is a FeatureGroup in the instance.
    let is_fg = comp.features.iter().any(|&fi| {
        let feat = &instance.features[fi];
        feat.name.eq_ci(feature_name) && feat.kind == FeatureKind::FeatureGroup
    });

    if !is_fg {
        return None;
    }

    // Resolve the component type to get the feature's classifier reference.
    let type_ref = ClassifierRef::qualified(comp.package.clone(), comp.type_name.clone());
    let resolved = scope.resolve_classifier(&comp.package, &type_ref);

    let type_loc = match &resolved {
        ResolvedClassifier::ComponentType { loc, .. } => *loc,
        _ => return None,
    };

    let ct = scope.get_component_type(type_loc)?;

    for &feat_idx in &ct.features {
        let feat = scope.get_feature(type_loc.tree, feat_idx)?;
        if feat.name.eq_ci(feature_name) && feat.kind == FeatureKind::FeatureGroup {
            if let Some(cls_ref) = &feat.classifier {
                let fg_name = &cls_ref.type_name;
                let fg_pkg = cls_ref.package.as_ref().unwrap_or(&comp.package);
                return Some(expand_feature_group(scope, fg_pkg, fg_name, false));
            }
            return None;
        }
    }

    None
}

/// Build the component path for a connection's owner.
fn build_connection_path(
    instance: &SystemInstance,
    owner: spar_hir_def::instance::ComponentInstanceIdx,
    conn_name: &Name,
) -> Vec<String> {
    let mut path = crate::component_path(instance, owner);
    path.push(conn_name.as_str().to_string());
    path
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(unused_imports, unused_variables, dead_code, clippy::manual_div_ceil)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use std::sync::Arc;

    use spar_hir_def::feature_group::ExpandedFeature;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::{ClassifierRef, Name};
    use spar_hir_def::resolver::GlobalScope;

    // ── Helper: build a feature group tree ──────────────────────────

    fn build_fg_tree(
        pkg_name: &str,
        fg_name: &str,
        features: Vec<(&str, FeatureKind, Option<Direction>)>,
        inverse_of: Option<ClassifierRef>,
    ) -> ItemTree {
        let mut tree = ItemTree::default();

        let mut feat_indices = Vec::new();
        for (name, kind, dir) in features {
            let idx = tree.features.alloc(Feature {
                name: Name::new(name),
                kind,
                direction: dir,
                access_kind: None,
                classifier: None,
                is_refined: false,
                array_dimensions: Vec::new(),
                property_associations: Vec::new(),
            });
            feat_indices.push(idx);
        }

        let fgt_idx = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new(fg_name),
            is_public: true,
            extends: None,
            inverse_of,
            features: feat_indices,
            prototypes: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new(pkg_name),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::FeatureGroupType(fgt_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree
    }

    // ── validate_complement tests ───────────────────────────────────

    #[test]
    fn complement_valid_matching_features() {
        let source = vec![
            ExpandedFeature {
                name: Name::new("temp"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::Out),
                group_prefix: None,
            },
            ExpandedFeature {
                name: Name::new("pressure"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::Out),
                group_prefix: None,
            },
        ];
        let destination = vec![
            ExpandedFeature {
                name: Name::new("temp"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::In),
                group_prefix: None,
            },
            ExpandedFeature {
                name: Name::new("pressure"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::In),
                group_prefix: None,
            },
        ];

        let result = validate_complement(&source, &destination);
        assert!(
            result.unmatched_source.is_empty(),
            "all features should match"
        );
        assert!(
            result.direction_mismatches.is_empty(),
            "directions should be complementary"
        );
    }

    #[test]
    fn complement_unmatched_feature() {
        let source = vec![
            ExpandedFeature {
                name: Name::new("temp"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::Out),
                group_prefix: None,
            },
            ExpandedFeature {
                name: Name::new("humidity"),
                kind: FeatureKind::DataPort,
                direction: Some(Direction::Out),
                group_prefix: None,
            },
        ];
        let destination = vec![ExpandedFeature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            group_prefix: None,
        }];

        let result = validate_complement(&source, &destination);
        assert_eq!(result.unmatched_source.len(), 1);
        assert_eq!(result.unmatched_source[0].name.as_str(), "humidity");
    }

    #[test]
    fn complement_direction_mismatch() {
        let source = vec![ExpandedFeature {
            name: Name::new("data"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            group_prefix: None,
        }];
        let destination = vec![ExpandedFeature {
            name: Name::new("data"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out), // Should be In!
            group_prefix: None,
        }];

        let result = validate_complement(&source, &destination);
        assert!(result.unmatched_source.is_empty());
        assert_eq!(result.direction_mismatches.len(), 1);
        assert_eq!(result.direction_mismatches[0].feature_name.as_str(), "data");
    }

    #[test]
    fn complement_inout_matches_inout() {
        let source = vec![ExpandedFeature {
            name: Name::new("bus"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::InOut),
            group_prefix: None,
        }];
        let destination = vec![ExpandedFeature {
            name: Name::new("bus"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::InOut),
            group_prefix: None,
        }];

        let result = validate_complement(&source, &destination);
        assert!(
            result.direction_mismatches.is_empty(),
            "InOut matches InOut"
        );
    }

    #[test]
    fn complement_case_insensitive_matching() {
        let source = vec![ExpandedFeature {
            name: Name::new("Temperature"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            group_prefix: None,
        }];
        let destination = vec![ExpandedFeature {
            name: Name::new("temperature"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            group_prefix: None,
        }];

        let result = validate_complement(&source, &destination);
        assert!(
            result.unmatched_source.is_empty(),
            "matching should be case-insensitive"
        );
        assert!(result.direction_mismatches.is_empty());
    }

    // ── check_inverse_of_rules tests ────────────────────────────────

    #[test]
    fn inverse_of_no_features_is_valid() {
        let mut tree = ItemTree::default();

        // First FGT: SensorOutput with features
        let f = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorOutput"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![f],
            prototypes: Vec::new(),
        });

        // Second FGT: inverse of SensorOutput, no explicit features (valid)
        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorInput"),
            is_public: true,
            extends: None,
            inverse_of: Some(ClassifierRef::type_only(Name::new("SensorOutput"))),
            features: Vec::new(),
            prototypes: Vec::new(),
        });

        let diags = check_inverse_of_rules(&tree);
        assert!(
            diags.is_empty(),
            "inverse_of with no features should be valid: {:?}",
            diags
        );
    }

    #[test]
    fn inverse_of_with_features_is_error() {
        let mut tree = ItemTree::default();

        let f = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        // FGT with both inverse_of and explicit features -- error
        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("BadGroup"),
            is_public: true,
            extends: None,
            inverse_of: Some(ClassifierRef::type_only(Name::new("SensorOutput"))),
            features: vec![f],
            prototypes: Vec::new(),
        });

        let diags = check_inverse_of_rules(&tree);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("BadGroup"));
        assert!(diags[0].message.contains("inverse of"));
    }

    // ── Feature group connection expansion in instance model ────────

    #[test]
    #[ignore = "pre-existing: FG expansion not yet implemented in instance model"]
    fn fg_connection_expands_to_individual_ports() {
        // Build an ItemTree with:
        // - Package P
        //   - Feature group type SensorData: temp (out data port), pressure (out data port)
        //   - Component type Sender with feature group sensors: SensorData
        //   - Component type Receiver with feature group sensors: SensorData
        //   - System type Top
        //   - System implementation Top.impl with:
        //     - subcomponent tx: Sender
        //     - subcomponent rx: Receiver
        //     - feature group connection: tx.sensors -> rx.sensors
        let mut tree = ItemTree::default();

        // Feature group type features
        let fg_f0 = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let fg_f1 = tree.features.alloc(Feature {
            name: Name::new("pressure"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let fgt_idx = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorData"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![fg_f0, fg_f1],
            prototypes: Vec::new(),
        });

        // Sender type with feature group "sensors" of type SensorData
        let sender_fg = tree.features.alloc(Feature {
            name: Name::new("sensors"),
            kind: FeatureKind::FeatureGroup,
            direction: None,
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("SensorData"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let sender_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sender"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: vec![sender_fg],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Receiver type with feature group "sensors" of type SensorData
        let receiver_fg = tree.features.alloc(Feature {
            name: Name::new("sensors"),
            kind: FeatureKind::FeatureGroup,
            direction: None,
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("SensorData"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let receiver_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Receiver"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: vec![receiver_fg],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Top type and implementation
        let top_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Top"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Subcomponents
        let sub_tx = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("tx"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::type_only(Name::new("Sender"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let sub_rx = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("rx"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::type_only(Name::new("Receiver"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Feature group connection
        let conn = tree.connections.alloc(ConnectionItem {
            name: Name::new("c1"),
            kind: ConnectionKind::FeatureGroup,
            is_bidirectional: false,
            is_refined: false,
            src: Some(ConnectedElementRef {
                subcomponent: Some(Name::new("tx")),
                feature: Name::new("sensors"),
            }),
            dst: Some(ConnectedElementRef {
                subcomponent: Some(Name::new("rx")),
                feature: Name::new("sensors"),
            }),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let top_impl = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Top"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_tx, sub_rx],
            connections: vec![conn],
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("P"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::FeatureGroupType(fgt_idx),
                ItemRef::ComponentType(sender_ct),
                ItemRef::ComponentType(receiver_ct),
                ItemRef::ComponentType(top_ct),
                ItemRef::ComponentImpl(top_impl),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let instance = SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        // The instance should have semantic connections from FG expansion.
        // We should find individual semantic connections for "temp" and "pressure".
        let fg_semantic: Vec<_> = instance
            .semantic_connections
            .iter()
            .filter(|sc| sc.name.as_str().starts_with("c1."))
            .collect();

        assert_eq!(
            fg_semantic.len(),
            2,
            "feature group connection should expand into 2 individual connections, \
             got {} semantic connections: {:?}",
            fg_semantic.len(),
            instance
                .semantic_connections
                .iter()
                .map(|sc| sc.name.as_str())
                .collect::<Vec<_>>()
        );

        // Check that we have connections for both temp and pressure
        let names: Vec<_> = fg_semantic.iter().map(|sc| sc.name.as_str()).collect();
        assert!(
            names.contains(&"c1.temp"),
            "should have c1.temp: {:?}",
            names
        );
        assert!(
            names.contains(&"c1.pressure"),
            "should have c1.pressure: {:?}",
            names
        );

        // Each expanded connection should be of kind Port (since the features are DataPort)
        for sc in &fg_semantic {
            assert_eq!(
                sc.kind,
                ConnectionKind::Port,
                "expanded FG connection should be Port kind"
            );
        }
    }

    #[test]
    #[ignore = "pre-existing: FG complement check requires GlobalScope in instance"]
    fn fg_complement_check_reports_mismatches() {
        // Build a model where source FG has "temp out" and "pressure out"
        // but destination FG has "temp out" (should be in!) and no "pressure".
        let mut tree = ItemTree::default();

        // Source FGT
        let src_f0 = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let src_f1 = tree.features.alloc(Feature {
            name: Name::new("pressure"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let src_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SourceFG"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![src_f0, src_f1],
            prototypes: Vec::new(),
        });

        // Destination FGT (wrong: temp is out instead of in, missing pressure)
        let dst_f0 = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let dst_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("DestFG"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![dst_f0],
            prototypes: Vec::new(),
        });

        // Sender type
        let sender_fg = tree.features.alloc(Feature {
            name: Name::new("fg_out"),
            kind: FeatureKind::FeatureGroup,
            direction: None,
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("SourceFG"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let sender_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sender"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: vec![sender_fg],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Receiver type
        let receiver_fg = tree.features.alloc(Feature {
            name: Name::new("fg_in"),
            kind: FeatureKind::FeatureGroup,
            direction: None,
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("DestFG"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let receiver_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Receiver"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: vec![receiver_fg],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        // Top type + impl
        let top_ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Top"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });

        let sub_tx = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("tx"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::type_only(Name::new("Sender"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let sub_rx = tree.subcomponents.alloc(SubcomponentItem {
            name: Name::new("rx"),
            category: ComponentCategory::System,
            classifier: Some(ClassifierRef::type_only(Name::new("Receiver"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let fg_conn = tree.connections.alloc(ConnectionItem {
            name: Name::new("c1"),
            kind: ConnectionKind::FeatureGroup,
            is_bidirectional: false,
            is_refined: false,
            src: Some(ConnectedElementRef {
                subcomponent: Some(Name::new("tx")),
                feature: Name::new("fg_out"),
            }),
            dst: Some(ConnectedElementRef {
                subcomponent: Some(Name::new("rx")),
                feature: Name::new("fg_in"),
            }),
            in_modes: Vec::new(),
            property_associations: Vec::new(),
        });

        let top_impl = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Top"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: vec![sub_tx, sub_rx],
            connections: vec![fg_conn],
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("P"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::FeatureGroupType(src_fgt),
                ItemRef::FeatureGroupType(dst_fgt),
                ItemRef::ComponentType(sender_ct),
                ItemRef::ComponentType(receiver_ct),
                ItemRef::ComponentType(top_ct),
                ItemRef::ComponentImpl(top_impl),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let instance = SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("Top"),
            &Name::new("impl"),
        );

        let diags = check_feature_group_complements(&instance, &scope);

        // Should report: temp has direction mismatch, pressure is unmatched
        assert!(
            diags.len() >= 2,
            "expected at least 2 diagnostics (unmatched + direction mismatch), got {}: {:?}",
            diags.len(),
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        let unmatched: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("no matching"))
            .collect();
        assert_eq!(unmatched.len(), 1, "pressure should be unmatched");
        assert!(unmatched[0].message.contains("pressure"));

        let mismatches: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("incompatible directions"))
            .collect();
        assert_eq!(mismatches.len(), 1, "temp should have direction mismatch");
        assert!(mismatches[0].message.contains("temp"));
    }

    #[test]
    #[ignore = "pre-existing: FG inverse expansion not yet implemented"]
    fn inverse_of_produces_correct_complement() {
        // Build a tree where SensorInput is inverse of SensorOutput.
        // A connection between them should pass complement validation.
        let mut tree = ItemTree::default();

        // SensorOutput features
        let f0 = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let f1 = tree.features.alloc(Feature {
            name: Name::new("pressure"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let src_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorOutput"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![f0, f1],
            prototypes: Vec::new(),
        });

        // SensorInput: inverse of SensorOutput
        let dst_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorInput"),
            is_public: true,
            extends: None,
            inverse_of: Some(ClassifierRef::type_only(Name::new("SensorOutput"))),
            features: Vec::new(),
            prototypes: Vec::new(),
        });

        // Expand both and verify they are complements
        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        let src_expanded =
            expand_feature_group(&scope, &Name::new("P"), &Name::new("SensorOutput"), false);
        let dst_expanded =
            expand_feature_group(&scope, &Name::new("P"), &Name::new("SensorInput"), false);

        assert_eq!(src_expanded.len(), 2);
        assert_eq!(dst_expanded.len(), 2);

        // SensorOutput: temp=Out, pressure=Out
        // SensorInput (inverse): temp=In, pressure=In
        let result = validate_complement(&src_expanded, &dst_expanded);
        assert!(
            result.unmatched_source.is_empty(),
            "inverse should match all features"
        );
        assert!(
            result.direction_mismatches.is_empty(),
            "inverse should have complementary directions: {:?}",
            result.direction_mismatches
        );
    }
}
