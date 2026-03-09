//! EMV2 fault tree analysis.
//!
//! Builds fault trees from the component hierarchy and detects
//! single-point-of-failure paths. Also checks that safety-relevant
//! components (process, thread, device, processor) carry error model
//! annotations.
//!
//! Since EMV2 annex data is not yet integrated into the instance model,
//! this pass works structurally: each composite component is an OR gate
//! (any child failure propagates up), and leaf components are basic
//! events. Future work will incorporate actual EMV2 error propagation
//! and behavior state machines.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::{component_path, Analysis, AnalysisDiagnostic, Severity};

// ── Fault tree types ────────────────────────────────────────────────

/// A node in a fault tree.
#[derive(Debug, Clone)]
pub enum FaultTreeNode {
    /// Basic event (leaf): an error occurrence at a specific component.
    BasicEvent {
        /// Component path (e.g., "top.sensor").
        component: String,
        /// Error type name (e.g., "ServiceFailure", "TimingError").
        error_type: String,
        /// Human description.
        description: String,
    },
    /// AND gate: all children must occur for parent to occur.
    And {
        description: String,
        children: Vec<FaultTreeNode>,
    },
    /// OR gate: any child occurring causes parent to occur.
    Or {
        description: String,
        children: Vec<FaultTreeNode>,
    },
}

/// A complete fault tree rooted at a top-level hazard.
#[derive(Debug, Clone)]
pub struct FaultTree {
    pub top_event: String,
    pub root: FaultTreeNode,
}

impl FaultTree {
    /// Compute minimal cut sets (sets of basic events that cause the top event).
    /// Uses a simple recursive algorithm suitable for small trees.
    pub fn minimal_cut_sets(&self) -> Vec<Vec<String>> {
        let raw = Self::cut_sets_recursive(&self.root);
        // Minimize: remove supersets
        let mut minimal: Vec<Vec<String>> = Vec::new();
        let mut sorted: Vec<Vec<String>> = raw;
        sorted.sort_by_key(|s| s.len());
        for set in &sorted {
            let dominated = minimal.iter().any(|m| m.iter().all(|e| set.contains(e)));
            if !dominated {
                minimal.push(set.clone());
            }
        }
        minimal
    }

    fn cut_sets_recursive(node: &FaultTreeNode) -> Vec<Vec<String>> {
        match node {
            FaultTreeNode::BasicEvent {
                component,
                error_type,
                ..
            } => {
                vec![vec![format!("{}.{}", component, error_type)]]
            }
            FaultTreeNode::Or { children, .. } => {
                // Union of all children's cut sets
                children
                    .iter()
                    .flat_map(|c| Self::cut_sets_recursive(c))
                    .collect()
            }
            FaultTreeNode::And { children, .. } => {
                // Cross-product of children's cut sets
                let mut result = vec![vec![]];
                for child in children {
                    let child_sets = Self::cut_sets_recursive(child);
                    let mut new_result = Vec::new();
                    for existing in &result {
                        for cs in &child_sets {
                            let mut combined = existing.clone();
                            for e in cs {
                                if !combined.contains(e) {
                                    combined.push(e.clone());
                                }
                            }
                            new_result.push(combined);
                        }
                    }
                    result = new_result;
                }
                result
            }
        }
    }
}

// ── Analysis pass ───────────────────────────────────────────────────

/// EMV2 fault tree analysis pass.
pub struct Emv2Analysis;

impl Analysis for Emv2Analysis {
    fn name(&self) -> &str {
        "emv2_fault_tree"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Check for components that could propagate errors without handling
        check_error_handling(instance, instance.root, &mut diags);

        // Build fault tree from component hierarchy
        if let Some(tree) = build_hierarchy_fault_tree(instance, instance.root) {
            let cut_sets = tree.minimal_cut_sets();
            if !cut_sets.is_empty() {
                let root_path = component_path(instance, instance.root);

                // Report single-point failures (cut sets of size 1)
                for cs in &cut_sets {
                    if cs.len() == 1 {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Warning,
                            message: format!(
                                "single point of failure: {} can cause top-level failure of {}",
                                cs[0], tree.top_event
                            ),
                            path: root_path.clone(),
                            analysis: self.name().to_string(),
                        });
                    }
                }

                // Summary diagnostic
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "fault tree for '{}': {} minimal cut sets, {} single-point failures",
                        tree.top_event,
                        cut_sets.len(),
                        cut_sets.iter().filter(|cs| cs.len() == 1).count()
                    ),
                    path: root_path,
                    analysis: self.name().to_string(),
                });
            }
        }

        diags
    }
}

// ── Helper functions ────────────────────────────────────────────────

fn dotted_path(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    component_path(instance, idx).join(".")
}

/// Check that safety-relevant components have error handling.
/// Reports info diagnostics for process/thread/device/processor components
/// without EMV2-related properties.
fn check_error_handling(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let comp = instance.component(idx);
    let path = component_path(instance, idx);

    // Safety-relevant categories that should have error handling
    let needs_error_handling = matches!(
        comp.category,
        ComponentCategory::Process
            | ComponentCategory::Thread
            | ComponentCategory::Device
            | ComponentCategory::Processor
    );

    if needs_error_handling && comp.children.is_empty() {
        // Leaf safety-relevant component — check for error model properties.
        // Since EMV2 annex data isn't in the instance model yet, we check
        // if the component has any property with "error" or "emv2" in the name.
        let props = instance.properties_for(idx);
        let has_error_props = props.iter().any(|(_key, values)| {
            values.iter().any(|pv| {
                let prop_name = pv.name.property_name.as_str().to_lowercase();
                let prop_set = pv
                    .name
                    .property_set
                    .as_ref()
                    .map(|ps| ps.as_str().to_lowercase())
                    .unwrap_or_default();
                prop_name.contains("error")
                    || prop_name.contains("emv2")
                    || prop_set.contains("error")
                    || prop_set.contains("emv2")
            })
        });

        if !has_error_props {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "{} component '{}' has no error model annotations",
                    category_label(comp.category),
                    comp.name
                ),
                path,
                analysis: "emv2_fault_tree".to_string(),
            });
        }
    }

    for &child in &comp.children {
        check_error_handling(instance, child, diags);
    }
}

fn category_label(cat: ComponentCategory) -> &'static str {
    match cat {
        ComponentCategory::Process => "process",
        ComponentCategory::Thread => "thread",
        ComponentCategory::Device => "device",
        ComponentCategory::Processor => "processor",
        _ => "component",
    }
}

/// Build a fault tree from the component hierarchy.
/// The top event is failure of the root component.
/// Each composite component is an OR gate (any child failure propagates up).
/// Leaf components are basic events.
fn build_hierarchy_fault_tree(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
) -> Option<FaultTree> {
    let path = dotted_path(instance, idx);
    let node = build_subtree(instance, idx);

    Some(FaultTree {
        top_event: format!("failure of {}", path),
        root: node,
    })
}

fn build_subtree(instance: &SystemInstance, idx: ComponentInstanceIdx) -> FaultTreeNode {
    let comp = instance.component(idx);
    let path = dotted_path(instance, idx);

    if comp.children.is_empty() {
        // Leaf: basic event
        FaultTreeNode::BasicEvent {
            component: path,
            error_type: "ServiceFailure".to_string(),
            description: format!("{} {} fails", category_label(comp.category), comp.name),
        }
    } else {
        // Composite: OR gate (any child failure causes parent failure)
        let children: Vec<FaultTreeNode> = comp
            .children
            .iter()
            .map(|&child_idx| build_subtree(instance, child_idx))
            .collect();

        FaultTreeNode::Or {
            description: format!("failure of {}", comp.name),
            children,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
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
            })
        }

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        fn set_property(
            &mut self,
            comp: ComponentInstanceIdx,
            set: &str,
            name: &str,
            value: &str,
        ) {
            let map = self
                .property_maps
                .entry(comp)
                .or_insert_with(PropertyMap::new);
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

    // ── FaultTree::minimal_cut_sets tests ───────────────────────────

    #[test]
    fn cut_sets_or_gate_independent() {
        // OR gate with 3 basic events -> 3 cut sets of size 1
        let tree = FaultTree {
            top_event: "top failure".to_string(),
            root: FaultTreeNode::Or {
                description: "any fails".to_string(),
                children: vec![
                    FaultTreeNode::BasicEvent {
                        component: "a".to_string(),
                        error_type: "Err".to_string(),
                        description: "a fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "b".to_string(),
                        error_type: "Err".to_string(),
                        description: "b fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "c".to_string(),
                        error_type: "Err".to_string(),
                        description: "c fails".to_string(),
                    },
                ],
            },
        };

        let cs = tree.minimal_cut_sets();
        assert_eq!(cs.len(), 3, "OR gate: each child is independent: {:?}", cs);
        for set in &cs {
            assert_eq!(set.len(), 1, "each cut set should have 1 element: {:?}", set);
        }
    }

    #[test]
    fn cut_sets_and_gate_all_required() {
        // AND gate with 3 basic events -> 1 cut set of size 3
        let tree = FaultTree {
            top_event: "top failure".to_string(),
            root: FaultTreeNode::And {
                description: "all must fail".to_string(),
                children: vec![
                    FaultTreeNode::BasicEvent {
                        component: "a".to_string(),
                        error_type: "Err".to_string(),
                        description: "a fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "b".to_string(),
                        error_type: "Err".to_string(),
                        description: "b fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "c".to_string(),
                        error_type: "Err".to_string(),
                        description: "c fails".to_string(),
                    },
                ],
            },
        };

        let cs = tree.minimal_cut_sets();
        assert_eq!(cs.len(), 1, "AND gate: one combined cut set: {:?}", cs);
        assert_eq!(cs[0].len(), 3, "cut set should contain all 3 events: {:?}", cs[0]);
    }

    #[test]
    fn cut_sets_nested_and_or_minimized() {
        // OR( AND(a, b), a ) -> minimal cut sets: { {a} }
        // Because {a} is a subset of {a, b}, so {a, b} is removed.
        let tree = FaultTree {
            top_event: "top".to_string(),
            root: FaultTreeNode::Or {
                description: "top".to_string(),
                children: vec![
                    FaultTreeNode::And {
                        description: "both".to_string(),
                        children: vec![
                            FaultTreeNode::BasicEvent {
                                component: "a".to_string(),
                                error_type: "Err".to_string(),
                                description: "a".to_string(),
                            },
                            FaultTreeNode::BasicEvent {
                                component: "b".to_string(),
                                error_type: "Err".to_string(),
                                description: "b".to_string(),
                            },
                        ],
                    },
                    FaultTreeNode::BasicEvent {
                        component: "a".to_string(),
                        error_type: "Err".to_string(),
                        description: "a".to_string(),
                    },
                ],
            },
        };

        let cs = tree.minimal_cut_sets();
        assert_eq!(cs.len(), 1, "superset {{a,b}} should be removed: {:?}", cs);
        assert_eq!(cs[0], vec!["a.Err"], "only {{a.Err}} should remain: {:?}", cs);
    }

    #[test]
    fn cut_sets_and_of_or_gates() {
        // AND( OR(a, b), OR(c, d) ) -> cut sets: {a,c}, {a,d}, {b,c}, {b,d}
        let tree = FaultTree {
            top_event: "top".to_string(),
            root: FaultTreeNode::And {
                description: "both sides".to_string(),
                children: vec![
                    FaultTreeNode::Or {
                        description: "left".to_string(),
                        children: vec![
                            FaultTreeNode::BasicEvent {
                                component: "a".to_string(),
                                error_type: "E".to_string(),
                                description: "a".to_string(),
                            },
                            FaultTreeNode::BasicEvent {
                                component: "b".to_string(),
                                error_type: "E".to_string(),
                                description: "b".to_string(),
                            },
                        ],
                    },
                    FaultTreeNode::Or {
                        description: "right".to_string(),
                        children: vec![
                            FaultTreeNode::BasicEvent {
                                component: "c".to_string(),
                                error_type: "E".to_string(),
                                description: "c".to_string(),
                            },
                            FaultTreeNode::BasicEvent {
                                component: "d".to_string(),
                                error_type: "E".to_string(),
                                description: "d".to_string(),
                            },
                        ],
                    },
                ],
            },
        };

        let cs = tree.minimal_cut_sets();
        assert_eq!(cs.len(), 4, "AND of two OR(2) = 4 cut sets: {:?}", cs);
        for set in &cs {
            assert_eq!(set.len(), 2, "each cut set has 2 events: {:?}", set);
        }
    }

    // ── Emv2Analysis::analyze tests ─────────────────────────────────

    #[test]
    fn analyze_reports_missing_error_annotations() {
        // System with leaf thread and device — both lack error model props
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        let dev = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![proc, dev]);
        b.set_children(proc, vec![t1]);

        let inst = b.build(root);
        let diags = Emv2Analysis.analyze(&inst);

        let no_annot: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("no error model annotations"))
            .collect();
        // t1 (thread, leaf), sensor (device, leaf), proc (process, leaf? no — has child t1)
        // proc has children so it's not checked. t1 and sensor are leaves.
        assert_eq!(
            no_annot.len(),
            2,
            "should report t1 and sensor: {:?}",
            no_annot
        );
    }

    #[test]
    fn analyze_suppresses_warning_when_error_props_present() {
        // Thread with an EMV2-related property should not be flagged
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
        let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
        b.set_children(root, vec![proc]);
        b.set_children(proc, vec![t1]);

        // Add an error-related property
        b.set_property(t1, "EMV2", "OccurrenceDistribution", "fixed 1.0e-6");

        let inst = b.build(root);
        let diags = Emv2Analysis.analyze(&inst);

        let no_annot: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("no error model annotations"))
            .collect();
        assert!(
            no_annot.is_empty(),
            "thread with EMV2 property should not be flagged: {:?}",
            no_annot
        );
    }

    #[test]
    fn analyze_single_point_of_failure() {
        // System with two leaf children (OR gate) -> each is a single point of failure
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let dev1 = b.add_component("sensor1", ComponentCategory::Device, Some(root));
        let dev2 = b.add_component("sensor2", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![dev1, dev2]);

        let inst = b.build(root);
        let diags = Emv2Analysis.analyze(&inst);

        let spof: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("single point of failure"))
            .collect();
        assert_eq!(
            spof.len(),
            2,
            "each leaf under OR gate is a single-point failure: {:?}",
            spof
        );
    }

    #[test]
    fn analyze_fault_tree_summary() {
        // Verify the summary diagnostic is emitted
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let child = b.add_component("sub", ComponentCategory::System, Some(root));
        b.set_children(root, vec![child]);

        let inst = b.build(root);
        let diags = Emv2Analysis.analyze(&inst);

        let summary: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("fault tree for"))
            .collect();
        assert_eq!(summary.len(), 1, "should have one summary: {:?}", diags);
        assert!(
            summary[0].message.contains("minimal cut sets"),
            "summary should mention cut sets: {}",
            summary[0].message
        );
    }

    #[test]
    fn analyze_leaf_only_system() {
        // Root with no children: just a basic event, single point of failure = itself
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);

        let inst = b.build(root);
        let diags = Emv2Analysis.analyze(&inst);

        let spof: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("single point of failure"))
            .collect();
        assert_eq!(
            spof.len(),
            1,
            "root alone is a single-point failure: {:?}",
            spof
        );
    }
}
