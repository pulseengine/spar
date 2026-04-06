//! Feature group expansion (AS5506 §8.2).
//!
//! Expands feature group types into their individual features,
//! handling `inverse of` by flipping port directions.

use crate::item_tree::{Direction, FeatureKind};
use crate::name::{ClassifierRef, Name};
use crate::resolver::{GlobalScope, ResolvedClassifier};

/// A single feature produced by expanding a feature group type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedFeature {
    /// The feature name.
    pub name: Name,
    /// The feature kind (DataPort, EventPort, etc.).
    pub kind: FeatureKind,
    /// The feature direction (may be flipped if inverse).
    pub direction: Option<Direction>,
    /// Dotted path prefix from enclosing feature group(s).
    pub group_prefix: Option<Name>,
}

/// Expand a feature group type into its individual features.
///
/// If `is_inverse` is true, all In/Out directions are flipped (InOut stays).
/// Handles nested feature groups recursively with a depth limit.
pub fn expand_feature_group(
    scope: &GlobalScope,
    package: &Name,
    fg_name: &Name,
    is_inverse: bool,
) -> Vec<ExpandedFeature> {
    // Build a ClassifierRef for the feature group type name (unqualified,
    // within the given package).
    let class_ref = ClassifierRef::type_only(fg_name.clone());
    expand_from_ref(scope, package, &class_ref, is_inverse, None, 0)
}

/// Maximum recursion depth for nested feature groups.
const MAX_DEPTH: usize = 10;

/// Core recursive expansion.
fn expand_from_ref(
    scope: &GlobalScope,
    package: &Name,
    class_ref: &ClassifierRef,
    is_inverse: bool,
    prefix: Option<&Name>,
    depth: usize,
) -> Vec<ExpandedFeature> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }

    // Resolve the classifier to find the feature group type.
    let resolved = scope.resolve_classifier(package, class_ref);
    let (pkg_name, loc) = match &resolved {
        ResolvedClassifier::FeatureGroupType { package, loc } => (package.clone(), *loc),
        _ => return Vec::new(),
    };

    let fgt = match scope.get_feature_group_type(loc) {
        Some(fgt) => fgt,
        None => return Vec::new(),
    };

    // If this feature group type is defined as `inverse of AnotherFGT`,
    // delegate to the referenced type with the inverse flag toggled.
    if let Some(inv_ref) = &fgt.inverse_of {
        return expand_from_ref(scope, &pkg_name, inv_ref, !is_inverse, prefix, depth + 1);
    }

    // Handle extends: collect parent features first (grandparent → parent → self).
    let mut result = Vec::new();
    if let Some(ext_ref) = &fgt.extends {
        let parent_features =
            expand_from_ref(scope, &pkg_name, ext_ref, is_inverse, prefix, depth + 1);
        result.extend(parent_features);
    }

    // Collect the feature indices we need before accessing them,
    // since we need the tree for feature lookup.
    let feat_indices: Vec<_> = fgt.features.clone();
    for &feat_idx in &feat_indices {
        let feat = match scope.get_feature(loc.tree, feat_idx) {
            Some(f) => f,
            None => continue,
        };

        if feat.kind == FeatureKind::FeatureGroup {
            // Nested feature group — recurse if there is a classifier reference.
            let nested_prefix = make_prefix(prefix, &feat.name);
            if let Some(nested_ref) = &feat.classifier {
                let nested = expand_from_ref(
                    scope,
                    &pkg_name,
                    nested_ref,
                    is_inverse,
                    Some(&nested_prefix),
                    depth + 1,
                );
                result.extend(nested);
            } else {
                // Feature group without a classifier — emit it as-is.
                result.push(ExpandedFeature {
                    name: feat.name.clone(),
                    kind: feat.kind,
                    direction: maybe_flip(feat.direction, is_inverse),
                    group_prefix: prefix.cloned(),
                });
            }
        } else {
            result.push(ExpandedFeature {
                name: feat.name.clone(),
                kind: feat.kind,
                direction: maybe_flip(feat.direction, is_inverse),
                group_prefix: prefix.cloned(),
            });
        }
    }

    result
}

/// Build a dotted prefix name: `existing_prefix.name` or just `name`.
fn make_prefix(existing: Option<&Name>, name: &Name) -> Name {
    match existing {
        Some(p) => Name::new(&format!("{}.{}", p.as_str(), name.as_str())),
        None => name.clone(),
    }
}

/// Optionally flip a direction when `is_inverse` is true.
fn maybe_flip(dir: Option<Direction>, is_inverse: bool) -> Option<Direction> {
    match (dir, is_inverse) {
        (Some(d), true) => Some(flip_direction(d)),
        (d, _) => d,
    }
}

/// Flip a direction: In becomes Out, Out becomes In, InOut stays InOut.
pub fn flip_direction(dir: Direction) -> Direction {
    match dir {
        Direction::In => Direction::Out,
        Direction::Out => Direction::In,
        Direction::InOut => Direction::InOut,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::*;
    use std::sync::Arc;

    // ── flip_direction ─────────────────────────────────────────────

    #[test]
    fn flip_in_to_out() {
        assert_eq!(flip_direction(Direction::In), Direction::Out);
    }

    #[test]
    fn flip_out_to_in() {
        assert_eq!(flip_direction(Direction::Out), Direction::In);
    }

    #[test]
    fn flip_inout_stays() {
        assert_eq!(flip_direction(Direction::InOut), Direction::InOut);
    }

    // ── Helper: build an ItemTree with a feature group type ────────

    /// Build an ItemTree containing one package with a feature group type
    /// that has the given features (and optionally an inverse_of reference).
    fn build_fg_tree(
        pkg_name: &str,
        fg_name: &str,
        features: Vec<(&str, FeatureKind, Option<Direction>, Option<ClassifierRef>)>,
        inverse_of: Option<ClassifierRef>,
    ) -> ItemTree {
        let mut tree = ItemTree::default();

        let mut feat_indices = Vec::new();
        for (name, kind, dir, classifier) in features {
            let idx = tree.features.alloc(Feature {
                name: Name::new(name),
                kind,
                direction: dir,
                access_kind: None,
                classifier,
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

    // ── Basic expansion ────────────────────────────────────────────

    #[test]
    fn expand_basic_feature_group() {
        let tree = build_fg_tree(
            "Sensors",
            "SensorData",
            vec![
                (
                    "temperature",
                    FeatureKind::DataPort,
                    Some(Direction::Out),
                    None,
                ),
                (
                    "pressure",
                    FeatureKind::DataPort,
                    Some(Direction::Out),
                    None,
                ),
                ("status", FeatureKind::EventPort, Some(Direction::Out), None),
            ],
            None,
        );

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let expanded = expand_feature_group(
            &scope,
            &Name::new("Sensors"),
            &Name::new("SensorData"),
            false,
        );

        assert_eq!(expanded.len(), 3);

        assert_eq!(expanded[0].name.as_str(), "temperature");
        assert_eq!(expanded[0].kind, FeatureKind::DataPort);
        assert_eq!(expanded[0].direction, Some(Direction::Out));
        assert!(expanded[0].group_prefix.is_none());

        assert_eq!(expanded[1].name.as_str(), "pressure");
        assert_eq!(expanded[1].kind, FeatureKind::DataPort);
        assert_eq!(expanded[1].direction, Some(Direction::Out));

        assert_eq!(expanded[2].name.as_str(), "status");
        assert_eq!(expanded[2].kind, FeatureKind::EventPort);
        assert_eq!(expanded[2].direction, Some(Direction::Out));
    }

    // ── Inverse expansion ──────────────────────────────────────────

    #[test]
    fn expand_inverse_flips_directions() {
        let tree = build_fg_tree(
            "P",
            "SensorData",
            vec![
                (
                    "temperature",
                    FeatureKind::DataPort,
                    Some(Direction::Out),
                    None,
                ),
                ("pressure", FeatureKind::DataPort, Some(Direction::In), None),
                (
                    "bus_io",
                    FeatureKind::DataPort,
                    Some(Direction::InOut),
                    None,
                ),
            ],
            None,
        );

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let expanded =
            expand_feature_group(&scope, &Name::new("P"), &Name::new("SensorData"), true);

        assert_eq!(expanded.len(), 3);

        // Out -> In
        assert_eq!(expanded[0].name.as_str(), "temperature");
        assert_eq!(expanded[0].direction, Some(Direction::In));

        // In -> Out
        assert_eq!(expanded[1].name.as_str(), "pressure");
        assert_eq!(expanded[1].direction, Some(Direction::Out));

        // InOut stays InOut
        assert_eq!(expanded[2].name.as_str(), "bus_io");
        assert_eq!(expanded[2].direction, Some(Direction::InOut));
    }

    // ── inverse of declaration ─────────────────────────────────────

    #[test]
    fn expand_inverse_of_declaration() {
        // Build a tree with two feature group types:
        //   SensorOutput: temperature out, pressure out
        //   SensorInput:  inverse of SensorOutput
        let mut tree = ItemTree::default();

        // Features for SensorOutput
        let f0 = tree.features.alloc(Feature {
            name: Name::new("temperature"),
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

        let fgt_output = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorOutput"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![f0, f1],
            prototypes: Vec::new(),
        });

        let fgt_input = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("SensorInput"),
            is_public: true,
            extends: None,
            inverse_of: Some(ClassifierRef::type_only(Name::new("SensorOutput"))),
            features: Vec::new(),
            prototypes: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("P"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::FeatureGroupType(fgt_output),
                ItemRef::FeatureGroupType(fgt_input),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // SensorInput is declared as inverse of SensorOutput,
        // so expanding it (not inverse) should flip the directions.
        let expanded =
            expand_feature_group(&scope, &Name::new("P"), &Name::new("SensorInput"), false);

        assert_eq!(expanded.len(), 2);
        // SensorOutput had Out, inverse flips to In
        assert_eq!(expanded[0].name.as_str(), "temperature");
        assert_eq!(expanded[0].direction, Some(Direction::In));
        assert_eq!(expanded[1].name.as_str(), "pressure");
        assert_eq!(expanded[1].direction, Some(Direction::In));
    }

    // ── Nested feature groups ──────────────────────────────────────

    #[test]
    fn expand_nested_feature_group() {
        // Build a tree with:
        //   InnerFG: temp (out data port), pressure (out data port)
        //   OuterFG: status (out event port), readings (feature group InnerFG)
        let mut tree = ItemTree::default();

        // Inner features
        let inner_f0 = tree.features.alloc(Feature {
            name: Name::new("temp"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let inner_f1 = tree.features.alloc(Feature {
            name: Name::new("pressure"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let inner_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("InnerFG"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![inner_f0, inner_f1],
            prototypes: Vec::new(),
        });

        // Outer features
        let outer_f0 = tree.features.alloc(Feature {
            name: Name::new("status"),
            kind: FeatureKind::EventPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let outer_f1 = tree.features.alloc(Feature {
            name: Name::new("readings"),
            kind: FeatureKind::FeatureGroup,
            direction: None,
            access_kind: None,
            classifier: Some(ClassifierRef::type_only(Name::new("InnerFG"))),
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let outer_fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("OuterFG"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: vec![outer_f0, outer_f1],
            prototypes: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("P"),
            with_clauses: Vec::new(),
            public_items: vec![
                ItemRef::FeatureGroupType(inner_fgt),
                ItemRef::FeatureGroupType(outer_fgt),
            ],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let expanded = expand_feature_group(&scope, &Name::new("P"), &Name::new("OuterFG"), false);

        // status (direct) + temp, pressure (from InnerFG via readings)
        assert_eq!(expanded.len(), 3);

        assert_eq!(expanded[0].name.as_str(), "status");
        assert_eq!(expanded[0].kind, FeatureKind::EventPort);
        assert!(expanded[0].group_prefix.is_none());

        assert_eq!(expanded[1].name.as_str(), "temp");
        assert_eq!(expanded[1].kind, FeatureKind::DataPort);
        assert_eq!(
            expanded[1].group_prefix.as_ref().unwrap().as_str(),
            "readings"
        );

        assert_eq!(expanded[2].name.as_str(), "pressure");
        assert_eq!(expanded[2].kind, FeatureKind::DataPort);
        assert_eq!(
            expanded[2].group_prefix.as_ref().unwrap().as_str(),
            "readings"
        );
    }

    // ── Unresolved feature group returns empty ──────────────────────

    #[test]
    fn expand_unresolved_returns_empty() {
        let tree = ItemTree::default();
        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        let expanded = expand_feature_group(
            &scope,
            &Name::new("NonExistent"),
            &Name::new("Phantom"),
            false,
        );
        assert!(expanded.is_empty());
    }

    // ── Feature with no direction ──────────────────────────────────

    #[test]
    fn expand_feature_without_direction() {
        let tree = build_fg_tree(
            "P",
            "FG",
            vec![("f1", FeatureKind::AbstractFeature, None, None)],
            None,
        );

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let expanded = expand_feature_group(&scope, &Name::new("P"), &Name::new("FG"), false);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].name.as_str(), "f1");
        assert_eq!(expanded[0].direction, None);
    }

    #[test]
    fn expand_feature_without_direction_inverse() {
        let tree = build_fg_tree(
            "P",
            "FG",
            vec![("f1", FeatureKind::AbstractFeature, None, None)],
            None,
        );

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let expanded = expand_feature_group(&scope, &Name::new("P"), &Name::new("FG"), true);

        assert_eq!(expanded.len(), 1);
        // No direction to flip — stays None
        assert_eq!(expanded[0].direction, None);
    }

    // ── Cross-package feature group ────────────────────────────────

    #[test]
    fn expand_cross_package_feature_group() {
        // Package "Types" has SensorData FGT
        let types_tree = build_fg_tree(
            "Types",
            "SensorData",
            vec![("reading", FeatureKind::DataPort, Some(Direction::Out), None)],
            None,
        );

        // Package "User" imports Types and references Types::SensorData
        let mut user_tree = ItemTree::default();
        user_tree.packages.alloc(Package {
            name: Name::new("User"),
            with_clauses: vec![Name::new("Types")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(types_tree), Arc::new(user_tree)]);

        // Expand Types::SensorData from the User package context
        let class_ref = ClassifierRef::qualified(Name::new("Types"), Name::new("SensorData"));
        let expanded = expand_from_ref(&scope, &Name::new("User"), &class_ref, false, None, 0);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].name.as_str(), "reading");
        assert_eq!(expanded[0].direction, Some(Direction::Out));
    }
}
