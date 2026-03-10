//! Unified legality rule engine for AADL models.
//!
//! Aggregates all analysis passes (both [`ItemTree`]-level and
//! [`SystemInstance`]-level) into a single engine that tags every
//! diagnostic with a standard rule identifier.
//!
//! # Rule ID scheme
//!
//! | Prefix        | Source                      |
//! |---------------|-----------------------------|
//! | `N-*`         | Naming rules                |
//! | `C-*`         | Category restriction        |
//! | `D-*`         | Direction rules             |
//! | `B-*`         | Binding checks              |
//! | `F-*`         | Flow checks                 |
//! | `CONN-*`      | Connectivity                |
//! | `CONN-TYPE`   | Connection feature kind     |
//! | `CONN-SELF`   | Connection self-loop        |
//! | `H-*`         | Hierarchy                   |
//! | `COMP-*`      | Completeness                |
//! | `MODE-*`      | Mode rules                  |
//! | `SUB-*`       | Subcomponent rules          |
//! | `L-*`         | Cross-cutting legality      |

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ItemTree;

use crate::binding_check::BindingCheckAnalysis;
use crate::category_check::check_category_rules;
use crate::completeness::CompletenessAnalysis;
use crate::connection_rules::ConnectionRuleAnalysis;
use crate::connectivity::ConnectivityAnalysis;
use crate::direction_rules::DirectionRuleAnalysis;
use crate::flow_check::FlowCheckAnalysis;
use crate::hierarchy::HierarchyAnalysis;
use crate::mode_rules::ModeRuleAnalysis;
use crate::naming_rules::check_naming_rules;
use crate::subcomponent_rules::SubcomponentRuleAnalysis;
use crate::{Analysis, AnalysisDiagnostic, Severity};

// ── Rule descriptor ────────────────────────────────────────────────

/// A single AADL legality rule from the standard.
#[derive(Debug, Clone)]
pub struct LegalityRule {
    /// Short identifier, e.g. `"N-1"`, `"D-2"`.
    pub id: &'static str,
    /// Human-readable description of what the rule checks.
    pub description: &'static str,
    /// AS5506 section reference (or internal tag).
    pub section: &'static str,
}

// ── Diagnostic wrapper ─────────────────────────────────────────────

/// A diagnostic produced by a legality check, tagged with the rule ID.
#[derive(Debug, Clone)]
pub struct LegalityDiagnostic {
    /// The rule that was violated.
    pub rule: LegalityRule,
    /// The underlying analysis diagnostic.
    pub inner: AnalysisDiagnostic,
}

// ── Engine ─────────────────────────────────────────────────────────

/// Aggregates all built-in analysis passes and runs them through a
/// unified rule-tagging layer.
pub struct LegalityEngine {
    instance_analyses: Vec<Box<dyn Analysis>>,
}

impl Default for LegalityEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LegalityEngine {
    /// Create a new engine with all built-in analyses registered.
    pub fn new() -> Self {
        let instance_analyses: Vec<Box<dyn Analysis>> = vec![
            Box::new(DirectionRuleAnalysis),
            Box::new(BindingCheckAnalysis),
            Box::new(FlowCheckAnalysis),
            Box::new(ConnectivityAnalysis),
            Box::new(HierarchyAnalysis),
            Box::new(CompletenessAnalysis),
            Box::new(ConnectionRuleAnalysis),
            Box::new(ModeRuleAnalysis),
            Box::new(SubcomponentRuleAnalysis),
        ];
        Self { instance_analyses }
    }

    /// Run ItemTree-level checks (naming, category, cross-cutting).
    pub fn check_item_tree(&self, tree: &ItemTree) -> Vec<LegalityDiagnostic> {
        let mut out = Vec::new();

        // ── Naming rules ──────────────────────────────────────────
        let naming_diags = check_naming_rules(tree);
        for d in naming_diags {
            out.push(LegalityDiagnostic {
                rule: classify_naming_rule(&d),
                inner: d,
            });
        }

        // ── Category restriction rules ────────────────────────────
        let cat_diags = check_category_rules(tree);
        for d in cat_diags {
            out.push(LegalityDiagnostic {
                rule: classify_category_rule(&d),
                inner: d,
            });
        }

        // ── Cross-cutting legality rules ──────────────────────────
        out.extend(check_impl_type_match(tree));
        out.extend(check_feature_group_nonempty(tree));

        out
    }

    /// Run instance-level checks (direction, binding, flow, connectivity,
    /// hierarchy, completeness).
    pub fn check_instance(&self, instance: &SystemInstance) -> Vec<LegalityDiagnostic> {
        let mut out = Vec::new();

        for analysis in &self.instance_analyses {
            let diags = analysis.analyze(instance);
            let name = analysis.name();
            for d in diags {
                out.push(LegalityDiagnostic {
                    rule: classify_instance_rule(name, &d),
                    inner: d,
                });
            }
        }

        out
    }

    /// Run all checks (both ItemTree-level and instance-level).
    pub fn check_all(
        &self,
        tree: &ItemTree,
        instance: &SystemInstance,
    ) -> Vec<LegalityDiagnostic> {
        let mut out = self.check_item_tree(tree);
        out.extend(self.check_instance(instance));
        out
    }
}

// ── Rule classification helpers ────────────────────────────────────

/// Map a naming-rule diagnostic to a specific rule ID.
fn classify_naming_rule(d: &AnalysisDiagnostic) -> LegalityRule {
    let msg = &d.message;
    if msg.contains("empty name") || msg.contains("empty type name") || msg.contains("empty impl name") {
        LegalityRule {
            id: "N-1",
            description: "Identifiers must be non-empty",
            section: "AS5506 \u{00a7}4",
        }
    } else if msg.contains("duplicate") && msg.contains("with clause") {
        LegalityRule {
            id: "N-3",
            description: "No duplicate with-clause entries",
            section: "AS5506 \u{00a7}4.2",
        }
    } else if msg.contains("imports itself") {
        LegalityRule {
            id: "N-3",
            description: "No self-referencing with clauses",
            section: "AS5506 \u{00a7}4.2",
        }
    } else if msg.contains("duplicate property definition") || msg.contains("duplicate property type") {
        LegalityRule {
            id: "N-4",
            description: "No duplicate property set member names",
            section: "AS5506 \u{00a7}11",
        }
    } else {
        // Duplicate feature / subcomponent / connection / flow / mode names
        LegalityRule {
            id: "N-2",
            description: "No duplicate names within a scope",
            section: "AS5506 \u{00a7}4.3",
        }
    }
}

/// Map a category-check diagnostic to a specific rule ID.
fn classify_category_rule(d: &AnalysisDiagnostic) -> LegalityRule {
    if d.message.contains("feature") {
        LegalityRule {
            id: "C-1",
            description: "Features must be allowed for the component category",
            section: "AS5506 \u{00a7}5-6",
        }
    } else {
        LegalityRule {
            id: "C-2",
            description: "Subcomponent categories must be allowed for the parent",
            section: "AS5506 \u{00a7}5-6",
        }
    }
}

/// Map an instance-level analysis diagnostic to a rule ID.
fn classify_instance_rule(analysis_name: &str, d: &AnalysisDiagnostic) -> LegalityRule {
    match analysis_name {
        "direction_rules" => {
            let msg = &d.message;
            if msg.contains("bidirectional") {
                LegalityRule {
                    id: "D-4",
                    description: "Bidirectional connections require in out on both ends",
                    section: "AS5506 \u{00a7}9.4",
                }
            } else if msg.contains("across") {
                LegalityRule {
                    id: "D-1",
                    description: "Across connections: out -> in",
                    section: "AS5506 \u{00a7}9.3",
                }
            } else if msg.contains("up") {
                LegalityRule {
                    id: "D-2",
                    description: "Up connections: subcomponent out -> enclosing out",
                    section: "AS5506 \u{00a7}9.3",
                }
            } else if msg.contains("down") {
                LegalityRule {
                    id: "D-3",
                    description: "Down connections: enclosing in -> subcomponent in",
                    section: "AS5506 \u{00a7}9.3",
                }
            } else {
                LegalityRule {
                    id: "D-1",
                    description: "Port direction compatibility",
                    section: "AS5506 \u{00a7}9.3",
                }
            }
        }
        "binding_check" => {
            if d.message.contains("references") && d.severity == Severity::Error {
                LegalityRule {
                    id: "B-2",
                    description: "Binding target must be an appropriate category",
                    section: "AS5506 \u{00a7}10.6",
                }
            } else {
                LegalityRule {
                    id: "B-1",
                    description: "Required deployment bindings should be present",
                    section: "AS5506 \u{00a7}10.6",
                }
            }
        }
        "flow_check" => {
            if d.message.contains("end-to-end") || d.message.contains("segment") {
                LegalityRule {
                    id: "F-2",
                    description: "End-to-end flow structural validity",
                    section: "AS5506 \u{00a7}10.5",
                }
            } else {
                LegalityRule {
                    id: "F-1",
                    description: "Flow spec port consistency",
                    section: "AS5506 \u{00a7}10.3",
                }
            }
        }
        "connectivity" => LegalityRule {
            id: "CONN-1",
            description: "Port connection completeness",
            section: "AS5506 \u{00a7}9.2",
        },
        "hierarchy" => LegalityRule {
            id: "H-1",
            description: "Component containment rules",
            section: "AS5506 \u{00a7}4.5",
        },
        "completeness" => LegalityRule {
            id: "COMP-1",
            description: "Model completeness (types, features, classifiers)",
            section: "AS5506 \u{00a7}4",
        },
        "connection_rules" => {
            let msg = &d.message;
            if msg.contains("self-loop") {
                LegalityRule {
                    id: "CONN-SELF",
                    description: "Connection must not loop back to same endpoint",
                    section: "AS5506 \u{00a7}9",
                }
            } else {
                LegalityRule {
                    id: "CONN-TYPE",
                    description: "Connected feature kinds must be compatible",
                    section: "AS5506 \u{00a7}9",
                }
            }
        }
        "mode_rules" => {
            let msg = &d.message;
            if msg.contains("duplicate mode name") {
                LegalityRule {
                    id: "MODE-UNIQUE",
                    description: "Mode names must be unique within a component",
                    section: "AS5506 \u{00a7}12",
                }
            } else {
                LegalityRule {
                    id: "MODE-TRIGGER",
                    description: "Mode transition triggers should be event ports",
                    section: "AS5506 \u{00a7}12",
                }
            }
        }
        "subcomponent_rules" => {
            let msg = &d.message;
            if msg.contains("duplicate subcomponent name") {
                LegalityRule {
                    id: "SUB-UNIQUE",
                    description: "Subcomponent names must be unique within a component",
                    section: "AS5506 \u{00a7}4.4",
                }
            } else {
                LegalityRule {
                    id: "SUB-CAT",
                    description: "Subcomponent category must be valid for containing component",
                    section: "AS5506 \u{00a7}4.5",
                }
            }
        }
        _ => LegalityRule {
            id: "UNKNOWN",
            description: "Unknown analysis rule",
            section: "",
        },
    }
}

// ── Cross-cutting ItemTree rules ───────────────────────────────────

/// **L-impl-type**: A component implementation's `type_name` must
/// match a declared component type in the same package.
fn check_impl_type_match(tree: &ItemTree) -> Vec<LegalityDiagnostic> {
    let mut out = Vec::new();

    for (_idx, pkg) in tree.packages.iter() {
        // Collect all type names declared in this package (case-insensitive).
        let mut declared_types: rustc_hash::FxHashSet<String> = rustc_hash::FxHashSet::default();
        for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
            if let spar_hir_def::item_tree::ItemRef::ComponentType(ct_idx) = item_ref {
                declared_types.insert(tree.component_types[*ct_idx].name.as_str().to_ascii_lowercase());
            }
        }

        // Check each implementation's type_name.
        for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
            if let spar_hir_def::item_tree::ItemRef::ComponentImpl(ci_idx) = item_ref {
                let ci = &tree.component_impls[*ci_idx];
                let tn_lower = ci.type_name.as_str().to_ascii_lowercase();
                if !tn_lower.is_empty() && !declared_types.contains(&tn_lower) {
                    out.push(LegalityDiagnostic {
                        rule: LegalityRule {
                            id: "L-impl-type",
                            description: "Implementation type must match a declared type in the same package",
                            section: "AS5506 \u{00a7}4.4",
                        },
                        inner: AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "implementation '{}.{}' references type '{}' which is not declared in package '{}'",
                                ci.type_name, ci.impl_name, ci.type_name, pkg.name
                            ),
                            path: vec![pkg.name.as_str().to_string(), format!("{}.{}", ci.type_name, ci.impl_name)],
                            analysis: "legality".to_string(),
                        },
                    });
                }
            }
        }
    }

    out
}

/// **L-fg-features**: A feature group type should have at least one
/// feature (warning, not error).
fn check_feature_group_nonempty(tree: &ItemTree) -> Vec<LegalityDiagnostic> {
    let mut out = Vec::new();

    for (_idx, fgt) in tree.feature_group_types.iter() {
        if fgt.features.is_empty() && fgt.inverse_of.is_none() {
            out.push(LegalityDiagnostic {
                rule: LegalityRule {
                    id: "L-fg-features",
                    description: "Feature group type should have at least one feature",
                    section: "AS5506 \u{00a7}8.2",
                },
                inner: AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "feature group type '{}' has no features and no inverse_of",
                        fgt.name
                    ),
                    path: vec![fgt.name.as_str().to_string()],
                    analysis: "legality".to_string(),
                },
            });
        }
    }

    out
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::Name;

    // ── Helpers ────────────────────────────────────────────────────

    struct TestInstanceBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
    }

    impl TestInstanceBuilder {
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
            type_name: &str,
            impl_name: Option<&str>,
            package: &str,
            parent: Option<ComponentInstanceIdx>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(type_name),
                impl_name: impl_name.map(Name::new),
                package: Name::new(package),
                parent,
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: None,
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
            src: ConnectionEnd,
            dst: ConnectionEnd,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind,
                is_bidirectional: false,
                owner,
                src: Some(src),
                dst: Some(dst),
            });
            self.components[owner].connections.push(idx);
            idx
        }

        fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
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

    /// Build a minimal valid ItemTree with one package, one type, and one impl.
    fn valid_item_tree() -> ItemTree {
        let mut tree = ItemTree::default();

        let f1 = tree.features.alloc(Feature {
            name: Name::new("port_in"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Controller"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![f1],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("Controller"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx), ItemRef::ComponentImpl(ci_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree
    }

    /// Build a minimal valid SystemInstance.
    fn valid_instance() -> (ComponentInstanceIdx, SystemInstance) {
        let mut b = TestInstanceBuilder::new();
        let root = b.add_component(
            "root",
            ComponentCategory::System,
            "Top",
            Some("impl"),
            "Pkg",
            None,
        );
        let child_a = b.add_component(
            "sensor",
            ComponentCategory::System,
            "Sensor",
            Some("basic"),
            "Pkg",
            Some(root),
        );
        let child_b = b.add_component(
            "controller",
            ComponentCategory::System,
            "Controller",
            Some("basic"),
            "Pkg",
            Some(root),
        );

        b.add_feature("reading", FeatureKind::DataPort, Some(Direction::Out), child_a);
        b.add_feature("input", FeatureKind::DataPort, Some(Direction::In), child_b);
        b.add_connection(
            "c1",
            ConnectionKind::Port,
            root,
            end(Some("sensor"), "reading"),
            end(Some("controller"), "input"),
        );
        b.set_children(root, vec![child_a, child_b]);

        let inst = b.build(root);
        (root, inst)
    }

    // ── LegalityEngine::new ───────────────────────────────────────

    #[test]
    fn engine_creates_successfully() {
        let engine = LegalityEngine::new();
        assert_eq!(engine.instance_analyses.len(), 9);
    }

    // ── Valid model produces no errors ─────────────────────────────

    #[test]
    fn valid_tree_and_instance_no_errors() {
        let engine = LegalityEngine::new();
        let tree = valid_item_tree();
        let (_root, instance) = valid_instance();

        let diags = engine.check_all(&tree, &instance);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.inner.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "valid model should have no error-level diagnostics, got: {:#?}",
            errors
        );
    }

    // ── ItemTree checks tag rule IDs correctly ─────────────────────

    #[test]
    fn duplicate_names_produce_tagged_diagnostics() {
        let mut tree = ItemTree::default();

        let f1 = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let f2 = tree.features.alloc(Feature {
            name: Name::new("port_a"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("MyType"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![f1, f2],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        assert!(!diags.is_empty(), "should produce at least one diagnostic");
        let naming = diags.iter().find(|d| d.rule.id.starts_with("N-"));
        assert!(naming.is_some(), "should have an N-* rule tagged diagnostic");
        assert_eq!(naming.unwrap().rule.id, "N-2");
    }

    // ── Instance checks tag rule IDs correctly ─────────────────────

    #[test]
    fn direction_violation_produces_tagged_diagnostics() {
        let mut b = TestInstanceBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
        let a = b.add_component("a", ComponentCategory::System, "A", Some("impl"), "Pkg", Some(root));
        let bb = b.add_component("b", ComponentCategory::System, "B", Some("impl"), "Pkg", Some(root));
        // Wrong direction: in -> out for across connection
        b.add_feature("p1", FeatureKind::DataPort, Some(Direction::In), a);
        b.add_feature("p2", FeatureKind::DataPort, Some(Direction::Out), bb);
        b.add_connection("c1", ConnectionKind::Port, root, end(Some("a"), "p1"), end(Some("b"), "p2"));
        b.set_children(root, vec![a, bb]);

        let instance = b.build(root);
        let engine = LegalityEngine::new();
        let diags = engine.check_instance(&instance);

        let dir_diags: Vec<_> = diags.iter().filter(|d| d.rule.id.starts_with("D-")).collect();
        assert!(
            !dir_diags.is_empty(),
            "should produce D-* tagged diagnostics for direction violations"
        );
        // Across connection violation should be D-1
        let d1_diags: Vec<_> = dir_diags.iter().filter(|d| d.rule.id == "D-1").collect();
        assert!(
            !d1_diags.is_empty(),
            "across direction violations should be tagged D-1, got: {:?}",
            dir_diags.iter().map(|d| d.rule.id).collect::<Vec<_>>()
        );
    }

    // ── Cross-cutting: L-impl-type ─────────────────────────────────

    #[test]
    fn impl_without_matching_type_produces_warning() {
        let mut tree = ItemTree::default();

        // Implementation without a matching type
        let ci_idx = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("MissingType"),
            impl_name: Name::new("impl"),
            category: ComponentCategory::System,
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentImpl(ci_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        let impl_type: Vec<_> = diags.iter().filter(|d| d.rule.id == "L-impl-type").collect();
        assert_eq!(impl_type.len(), 1, "should flag missing type: {:?}", diags.iter().map(|d| (&d.rule.id, &d.inner.message)).collect::<Vec<_>>());
        assert!(impl_type[0].inner.message.contains("MissingType"));
    }

    #[test]
    fn impl_with_matching_type_no_warning() {
        let tree = valid_item_tree();

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        let impl_type: Vec<_> = diags.iter().filter(|d| d.rule.id == "L-impl-type").collect();
        assert!(impl_type.is_empty(), "matching type should produce no L-impl-type: {:?}", impl_type);
    }

    // ── Cross-cutting: L-fg-features ───────────────────────────────

    #[test]
    fn empty_feature_group_type_produces_warning() {
        let mut tree = ItemTree::default();

        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("EmptyGroup"),
            extends: None,
            inverse_of: None,
            features: Vec::new(),
            prototypes: Vec::new(),
            is_public: true,
        });

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        let fg: Vec<_> = diags.iter().filter(|d| d.rule.id == "L-fg-features").collect();
        assert_eq!(fg.len(), 1, "empty feature group should warn");
        assert!(fg[0].inner.message.contains("EmptyGroup"));
    }

    #[test]
    fn feature_group_with_features_no_warning() {
        let mut tree = ItemTree::default();

        let f = tree.features.alloc(Feature {
            name: Name::new("signal"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("BusInterface"),
            extends: None,
            inverse_of: None,
            features: vec![f],
            prototypes: Vec::new(),
            is_public: true,
        });

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        let fg: Vec<_> = diags.iter().filter(|d| d.rule.id == "L-fg-features").collect();
        assert!(fg.is_empty(), "feature group with features should not warn");
    }

    // ── check_all combines both levels ─────────────────────────────

    #[test]
    fn check_all_combines_item_tree_and_instance() {
        let engine = LegalityEngine::new();

        // ItemTree with a duplicate feature
        let mut tree = ItemTree::default();
        let f1 = tree.features.alloc(Feature {
            name: Name::new("dup"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let f2 = tree.features.alloc(Feature {
            name: Name::new("dup"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });
        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("T"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![f1, f2],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });
        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        // Instance with a hierarchy violation (thread in system)
        let mut ib = TestInstanceBuilder::new();
        let root = ib.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
        let thread = ib.add_component("t1", ComponentCategory::Thread, "Worker", None, "Pkg", Some(root));
        ib.set_children(root, vec![thread]);
        let instance = ib.build(root);

        let diags = engine.check_all(&tree, &instance);

        // Should have both N-* (naming) and H-* (hierarchy) diagnostics
        let has_naming = diags.iter().any(|d| d.rule.id.starts_with("N-"));
        let has_hierarchy = diags.iter().any(|d| d.rule.id.starts_with("H-"));
        assert!(has_naming, "should have naming diagnostics from ItemTree check");
        assert!(has_hierarchy, "should have hierarchy diagnostics from instance check");
    }

    // ── Category rule tagging ──────────────────────────────────────

    #[test]
    fn category_feature_violation_tagged_c1() {
        let mut tree = ItemTree::default();

        let f = tree.features.alloc(Feature {
            name: Name::new("bad_param"),
            kind: FeatureKind::Parameter,
            direction: None,
            access_kind: None,
            classifier: None,
            is_refined: false,
            array_dimensions: Vec::new(),
            property_associations: Vec::new(),
        });

        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("BadType"),
            category: ComponentCategory::System,
            extends: None,
            features: vec![f],
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            is_public: true,
        });

        tree.packages.alloc(Package {
            name: Name::new("Pkg"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_idx)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let engine = LegalityEngine::new();
        let diags = engine.check_item_tree(&tree);

        let c1: Vec<_> = diags.iter().filter(|d| d.rule.id == "C-1").collect();
        assert_eq!(c1.len(), 1, "system with parameter should flag C-1");
    }
}
