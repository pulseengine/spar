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
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;

    use spar_hir_def::feature_group::ExpandedFeature;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::{ClassifierRef, Name};

    // ── TestBuilder ─────────────────────────────────────────────────

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
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

        fn add_feature(
            &mut self,
            name: &str,
            kind: FeatureKind,
            direction: Option<Direction>,
            owner: ComponentInstanceIdx,
        ) -> FeatureInstanceIdx {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction,
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
            kind: ConnectionKind,
            owner: ComponentInstanceIdx,
            src: Option<ConnectionEnd>,
            dst: Option<ConnectionEnd>,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind,
                is_bidirectional: false,
                owner,
                src,
                dst,
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
            idx
        }

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
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
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    fn end(sub: Option<&str>, feat: &str) -> ConnectionEnd {
        ConnectionEnd {
            subcomponent: sub.map(Name::new),
            feature: Name::new(feat),
        }
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

    // ── FeatureGroupCheckAnalysis::analyze tests ─────────────────────

    #[test]
    fn analysis_name_is_feature_group_check() {
        let analysis = FeatureGroupCheckAnalysis;
        assert_eq!(analysis.name(), "feature_group_check");
    }

    #[test]
    fn analyze_skips_non_feature_group_connections() {
        // A port connection should not trigger any FG diagnostics.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("p_out", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("p_in", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            Some(end(Some("a"), "p_out")),
            Some(end(Some("b"), "p_in")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "port connections should be skipped");
    }

    #[test]
    fn analyze_valid_fg_connection_no_diagnostics() {
        // Both endpoints are FeatureGroup kind -- no warnings.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, a);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "valid FG connection should have no warnings: {diags:?}"
        );
    }

    #[test]
    fn analyze_missing_dst_endpoint_reports_error() {
        // Connection with a missing destination endpoint.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_connection(
            "c_broken",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(None, "fg_out")),
            None,
        );
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("missing an endpoint"));
        assert!(diags[0].message.contains("c_broken"));
    }

    #[test]
    fn analyze_missing_src_endpoint_reports_error() {
        // Connection with a missing source endpoint.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_connection(
            "c_no_src",
            ConnectionKind::FeatureGroup,
            root,
            None,
            Some(end(None, "fg_in")),
        );
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("missing an endpoint"));
    }

    #[test]
    fn analyze_both_endpoints_missing_reports_error() {
        // Connection with both endpoints missing.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_connection("c_none", ConnectionKind::FeatureGroup, root, None, None);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("missing an endpoint"));
    }

    #[test]
    fn analyze_src_not_feature_group_warns() {
        // Source feature is a DataPort, not a FeatureGroup.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("out_port", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "out_port")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1, "expected 1 warning: {diags:?}");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("source feature"));
        assert!(diags[0].message.contains("out_port"));
        assert!(diags[0].message.contains("not a feature group"));
    }

    #[test]
    fn analyze_dst_not_feature_group_warns() {
        // Destination feature is an EventPort, not a FeatureGroup.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, a);
        b.add_feature("evt", FeatureKind::EventPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "evt")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1, "expected 1 warning: {diags:?}");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("destination feature"));
        assert!(diags[0].message.contains("evt"));
        assert!(diags[0].message.contains("not a feature group"));
    }

    #[test]
    fn analyze_both_endpoints_not_fg_warns_twice() {
        // Both source and destination features are non-FG.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("dp_out", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("dp_in", FeatureKind::DataPort, Some(Direction::In), bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "dp_out")),
            Some(end(Some("b"), "dp_in")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 2, "expected 2 warnings: {diags:?}");
        assert!(diags.iter().all(|d| d.severity == Severity::Warning));
        assert!(diags[0].message.contains("source feature"));
        assert!(diags[1].message.contains("destination feature"));
    }

    #[test]
    fn analyze_empty_features_no_warning() {
        // If the component has no features at all, the check is skipped
        // (can't say "not a FG" when there are no features to check).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Intentionally add NO features to a or bb.
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "no features on component means no warning: {diags:?}"
        );
    }

    #[test]
    fn analyze_self_reference_connection_no_subcomponent() {
        // Connection where endpoints reference the owner itself (no subcomponent).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, root);
        b.add_feature("sensors_out", FeatureKind::FeatureGroup, None, root);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(None, "sensors")),
            Some(end(None, "sensors_out")),
        );
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(diags.is_empty(), "both features are FG: {diags:?}");
    }

    #[test]
    fn analyze_unresolved_subcomponent_no_crash() {
        // Connection references a subcomponent that doesn't exist as a child.
        // resolve_endpoint_component returns None, so no feature check happens.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("nonexistent"), "sensors")),
            Some(end(Some("also_missing"), "sensors")),
        );
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "unresolved subcomponents produce no warning: {diags:?}"
        );
    }

    #[test]
    fn analyze_case_insensitive_feature_match() {
        // Feature name matching for FG check should be case-insensitive.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // Feature names use different casing than the connection references.
        b.add_feature("Sensors", FeatureKind::FeatureGroup, None, a);
        b.add_feature("SENSORS", FeatureKind::FeatureGroup, None, bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "case-insensitive match should find FG: {diags:?}"
        );
    }

    #[test]
    fn analyze_multiple_fg_connections() {
        // Two FG connections: one valid, one with src not being FG.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, a);
        b.add_feature("data_out", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, bb);
        // Valid connection
        b.add_connection(
            "c_ok",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        // Bad connection: source is a DataPort
        b.add_connection(
            "c_bad",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "data_out")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(
            diags.len(),
            1,
            "only c_bad should produce a warning: {diags:?}"
        );
        assert!(diags[0].message.contains("c_bad"));
        assert!(diags[0].message.contains("source feature"));
    }

    #[test]
    fn analyze_path_includes_owner_component() {
        // Verify the diagnostic path includes the owning component.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        b.add_feature("dp", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "dp")),
            None,
        );
        b.set_children(root, vec![a]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        // Path should contain the root component name.
        assert!(
            diags[0].path.iter().any(|p| p == "root"),
            "path should include owner: {:?}",
            diags[0].path
        );
    }

    #[test]
    fn analyze_analysis_field_is_set() {
        // All diagnostics should have analysis = "feature_group_check".
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(None, "fg")),
            None,
        );
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].analysis, "feature_group_check");
    }

    #[test]
    fn analyze_no_connections_no_diagnostics() {
        // System with no connections at all.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert!(diags.is_empty());
    }

    #[test]
    fn analyze_feature_name_matches_but_wrong_kind_warns() {
        // Feature named "sensors" exists but is a DataPort, not a FeatureGroup.
        // This kills the `&&` to `||` mutant in the `any()` closure: with `||`
        // the name match alone would make `is_fg = true`, suppressing the warning.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // "a" has a feature named "sensors" but it's a DataPort, not a FeatureGroup
        b.add_feature("sensors", FeatureKind::DataPort, Some(Direction::Out), a);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(
            diags.len(),
            1,
            "expected warning for wrong-kind feature: {diags:?}"
        );
        assert!(diags[0].message.contains("source feature"));
        assert!(diags[0].message.contains("sensors"));
        assert!(diags[0].message.contains("not a feature group"));
    }

    #[test]
    fn analyze_dst_feature_name_matches_but_wrong_kind_warns() {
        // Same as above but for the destination side.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, a);
        // "b" has "sensors" as EventDataPort, not FeatureGroup
        b.add_feature(
            "sensors",
            FeatureKind::EventDataPort,
            Some(Direction::In),
            bb,
        );
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        assert_eq!(
            diags.len(),
            1,
            "expected warning for wrong-kind dst feature: {diags:?}"
        );
        assert!(diags[0].message.contains("destination feature"));
    }

    #[test]
    fn inverse_of_without_features_is_valid() {
        // FGT with inverse_of but empty features list — should be valid.
        // Kills `&&` to `||` mutant on line 181: with `||`, this would fire
        // because `is_some()` is true even though features is empty.
        let mut tree = ItemTree::default();

        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorOutput"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: Vec::new(),
            prototypes: Vec::new(),
        });

        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorInput"),
            is_public: true,
            extends: None,
            inverse_of: Some(ClassifierRef::type_only(Name::new("SensorOutput"))),
            features: Vec::new(), // No explicit features — valid for inverse_of
            prototypes: Vec::new(),
        });

        let diags = check_inverse_of_rules(&tree);
        assert!(
            diags.is_empty(),
            "inverse_of with empty features should be valid: {diags:?}"
        );
    }

    #[test]
    fn no_inverse_of_with_features_is_valid() {
        // FGT without inverse_of but with features — should be valid.
        // Kills `&&` to `||` mutant: with `||`, `!features.is_empty()` alone
        // would trigger the error even without `inverse_of`.
        let mut tree = ItemTree::default();

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
            inverse_of: None,  // No inverse_of
            features: vec![f], // Has features — valid without inverse_of
            prototypes: Vec::new(),
        });

        let diags = check_inverse_of_rules(&tree);
        assert!(
            diags.is_empty(),
            "features without inverse_of should be valid: {diags:?}"
        );
    }

    #[test]
    fn analyze_feature_name_not_found_but_other_features_exist() {
        // Component has features, but not the one referenced by the connection.
        // The feature check iterates features looking for a name+kind match.
        // If no match is found and features exist, that's a warning.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::System, Some(root));
        let bb = b.add_component("b", ComponentCategory::System, Some(root));
        // "a" has a feature but with a different name than the connection references.
        b.add_feature("other_fg", FeatureKind::FeatureGroup, None, a);
        b.add_feature("sensors", FeatureKind::FeatureGroup, None, bb);
        b.add_connection(
            "c1",
            ConnectionKind::FeatureGroup,
            root,
            Some(end(Some("a"), "sensors")),
            Some(end(Some("b"), "sensors")),
        );
        b.set_children(root, vec![a, bb]);
        let inst = b.build(root);

        let diags = FeatureGroupCheckAnalysis.analyze(&inst);
        // "a" has features but none named "sensors" that is a FeatureGroup, so warning.
        assert_eq!(diags.len(), 1, "expected 1 warning: {diags:?}");
        assert!(diags[0].message.contains("source feature"));
        assert!(diags[0].message.contains("sensors"));
    }
}
