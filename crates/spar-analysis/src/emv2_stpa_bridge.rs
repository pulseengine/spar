//! EMV2-to-STPA bridge: maps fault trees to Rivet STPA hazard artifacts.
//!
//! This module bridges EMV2 fault tree analysis results to STPA-compatible
//! artifact structures. The mapping rules are:
//!
//! | EMV2 Concept                     | Rivet STPA Type       |
//! |----------------------------------|-----------------------|
//! | Composite error state (top event)| `hazard`              |
//! | Error propagation path           | `loss-scenario`       |
//! | Component error state (leaf)     | `sub-hazard`          |
//! | Error type                       | Tag on hazard/scenario|
//! | Fault tree cut set               | `loss-scenario` desc  |
//! | Error behavior transition        | `controller-constraint`|
//!
//! The bridge generates YAML output compatible with rivet's STPA schema.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

use crate::emv2_analysis::{FaultTree, FaultTreeNode};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

// ── STPA artifact types ────────────────────────────────────────────

/// An STPA artifact generated from EMV2 analysis results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StpaArtifact {
    /// Artifact identifier (e.g., "H-EMV2-001").
    pub id: String,
    /// STPA artifact type (e.g., "hazard", "loss-scenario", "sub-hazard").
    pub artifact_type: String,
    /// Human-readable title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Classification tags.
    pub tags: Vec<String>,
    /// Traceability links to other artifacts.
    pub links: Vec<StpaLink>,
}

/// A traceability link between STPA artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StpaLink {
    /// Link type (e.g., "leads-to-loss", "caused-by-uca", "refines").
    pub link_type: String,
    /// Target artifact IDs.
    pub targets: Vec<String>,
}

/// Result of bridging EMV2 fault trees to STPA artifacts.
#[derive(Debug, Clone)]
pub struct BridgeResult {
    /// Generated STPA artifacts.
    pub artifacts: Vec<StpaArtifact>,
}

// ── Bridge logic ───────────────────────────────────────────────────

/// Bridge EMV2 fault tree results to STPA artifact structures.
///
/// Takes a fault tree (produced by [`crate::emv2_analysis`]) and maps it
/// to hazards, sub-hazards, and loss scenarios compatible with rivet's
/// STPA schema.
pub fn bridge_fault_tree_to_stpa(tree: &FaultTree) -> BridgeResult {
    let mut artifacts = Vec::new();
    let mut hazard_counter = 1u32;
    let mut scenario_counter = 1u32;
    let mut sub_hazard_counter = 1u32;

    // Top event → hazard
    let cut_sets = tree.minimal_cut_sets();
    let cut_set_desc = format_cut_sets(&cut_sets);
    let hazard_id = format!("H-EMV2-{:03}", hazard_counter);
    hazard_counter += 1;

    artifacts.push(StpaArtifact {
        id: hazard_id.clone(),
        artifact_type: "hazard".to_string(),
        title: sanitize_title(&tree.top_event),
        description: format!(
            "Generated from EMV2 composite error state: {}.\n\
             Fault tree cut sets: {}",
            tree.top_event, cut_set_desc
        ),
        tags: vec!["emv2-generated".to_string(), "fault-tree".to_string()],
        links: Vec::new(),
    });

    // Walk tree nodes to produce sub-hazards and loss scenarios
    bridge_node(
        &tree.root,
        &hazard_id,
        &mut artifacts,
        &mut hazard_counter,
        &mut scenario_counter,
        &mut sub_hazard_counter,
    );

    BridgeResult { artifacts }
}

/// Recursively map fault tree nodes to STPA artifacts.
fn bridge_node(
    node: &FaultTreeNode,
    parent_hazard_id: &str,
    artifacts: &mut Vec<StpaArtifact>,
    _hazard_counter: &mut u32,
    scenario_counter: &mut u32,
    sub_hazard_counter: &mut u32,
) {
    match node {
        FaultTreeNode::BasicEvent {
            component,
            error_type,
            description,
        } => {
            // Leaf component error state → sub-hazard
            let sub_id = format!("SH-EMV2-{:03}", *sub_hazard_counter);
            *sub_hazard_counter += 1;

            artifacts.push(StpaArtifact {
                id: sub_id,
                artifact_type: "sub-hazard".to_string(),
                title: format!("{} {} at {}", error_type, "failure", component),
                description: format!(
                    "Component-level failure: {}. Error type: {}.",
                    description, error_type,
                ),
                tags: vec![
                    "emv2-generated".to_string(),
                    format!("error-type:{}", error_type.to_lowercase()),
                ],
                links: vec![StpaLink {
                    link_type: "refines".to_string(),
                    targets: vec![parent_hazard_id.to_string()],
                }],
            });
        }
        FaultTreeNode::Or {
            description,
            children,
        } => {
            // OR gate: error propagation — any child causes parent failure.
            // Map to a loss scenario describing the propagation path.
            let scenario_id = format!("LS-EMV2-{:03}", *scenario_counter);
            *scenario_counter += 1;

            let child_components = collect_basic_events(children);
            let child_desc = if child_components.is_empty() {
                "no basic events".to_string()
            } else {
                child_components.join(", ")
            };

            artifacts.push(StpaArtifact {
                id: scenario_id,
                artifact_type: "loss-scenario".to_string(),
                title: format!("Error propagation: {}", sanitize_title(description)),
                description: format!(
                    "EMV2 OR gate: any of [{}] failing causes {}. \
                     This represents an error propagation path where \
                     a single component failure propagates upward.",
                    child_desc, description,
                ),
                tags: vec!["emv2-generated".to_string(), "propagation-path".to_string()],
                links: vec![StpaLink {
                    link_type: "leads-to-hazard".to_string(),
                    targets: vec![parent_hazard_id.to_string()],
                }],
            });

            for child in children {
                bridge_node(
                    child,
                    parent_hazard_id,
                    artifacts,
                    _hazard_counter,
                    scenario_counter,
                    sub_hazard_counter,
                );
            }
        }
        FaultTreeNode::And {
            description,
            children,
        } => {
            // AND gate: all children must fail — represents a redundancy barrier.
            // Map to a loss scenario with all children as causal factors.
            let scenario_id = format!("LS-EMV2-{:03}", *scenario_counter);
            *scenario_counter += 1;

            let child_components = collect_basic_events(children);
            let child_desc = if child_components.is_empty() {
                "no basic events".to_string()
            } else {
                child_components.join(" AND ")
            };

            artifacts.push(StpaArtifact {
                id: scenario_id,
                artifact_type: "loss-scenario".to_string(),
                title: format!("Concurrent failures: {}", sanitize_title(description)),
                description: format!(
                    "EMV2 AND gate: all of [{}] must fail for {}. \
                     This represents a redundancy barrier — the parent \
                     only fails when all children fail simultaneously.",
                    child_desc, description,
                ),
                tags: vec![
                    "emv2-generated".to_string(),
                    "concurrent-failure".to_string(),
                ],
                links: vec![StpaLink {
                    link_type: "leads-to-hazard".to_string(),
                    targets: vec![parent_hazard_id.to_string()],
                }],
            });

            for child in children {
                bridge_node(
                    child,
                    parent_hazard_id,
                    artifacts,
                    _hazard_counter,
                    scenario_counter,
                    sub_hazard_counter,
                );
            }
        }
    }
}

// ── YAML generation ────────────────────────────────────────────────

/// Generate YAML output compatible with rivet's STPA schema.
///
/// The output format matches the structure used in `safety/stpa/analysis.yaml`:
/// a top-level `artifacts:` key containing a list of artifact maps.
pub fn generate_stpa_yaml(result: &BridgeResult) -> String {
    let mut out = String::from("# Generated by spar EMV2-STPA bridge\n");
    out.push_str("# Maps EMV2 fault tree analysis to STPA hazard artifacts\n\n");
    out.push_str("artifacts:\n");

    for artifact in &result.artifacts {
        out.push_str(&format!("\n  - id: {}\n", artifact.id));
        out.push_str(&format!("    type: {}\n", artifact.artifact_type));
        out.push_str(&format!(
            "    title: \"{}\"\n",
            yaml_escape(&artifact.title)
        ));
        out.push_str("    description: >\n");
        for line in artifact.description.lines() {
            out.push_str(&format!("      {}\n", line));
        }
        if !artifact.tags.is_empty() {
            out.push_str(&format!(
                "    tags: [{}]\n",
                artifact
                    .tags
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !artifact.links.is_empty() {
            out.push_str("    links:\n");
            for link in &artifact.links {
                out.push_str(&format!(
                    "      - type: {}\n        target: {}\n",
                    link.link_type,
                    link.targets.join(", ")
                ));
            }
        }
    }

    out
}

// ── Helpers ────────────────────────────────────────────────────────

/// Collect basic event labels from a list of fault tree nodes.
fn collect_basic_events(nodes: &[FaultTreeNode]) -> Vec<String> {
    let mut events = Vec::new();
    for node in nodes {
        collect_basic_events_recursive(node, &mut events);
    }
    events
}

fn collect_basic_events_recursive(node: &FaultTreeNode, events: &mut Vec<String>) {
    match node {
        FaultTreeNode::BasicEvent {
            component,
            error_type,
            ..
        } => {
            events.push(format!("{}.{}", component, error_type));
        }
        FaultTreeNode::Or { children, .. } | FaultTreeNode::And { children, .. } => {
            for child in children {
                collect_basic_events_recursive(child, events);
            }
        }
    }
}

/// Format cut sets for inclusion in a description.
fn format_cut_sets(cut_sets: &[Vec<String>]) -> String {
    if cut_sets.is_empty() {
        return "none".to_string();
    }
    cut_sets
        .iter()
        .map(|cs| format!("{{{}}}", cs.join(", ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Sanitize a title string: capitalize first letter, truncate if needed.
fn sanitize_title(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return "Unnamed".to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(c) => {
            let title: String = c.to_uppercase().chain(chars).collect();
            if title.len() > 120 {
                format!("{}...", &title[..117])
            } else {
                title
            }
        }
        None => "Unnamed".to_string(),
    }
}

/// Escape a string for YAML double-quoted scalar.
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build a dotted path for a component instance.
fn dotted_path(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    component_path(instance, idx).join(".")
}

// ── Analysis pass ──────────────────────────────────────────────────

/// EMV2-to-STPA bridge analysis pass.
///
/// This pass builds fault trees from the component hierarchy (reusing
/// the same structural approach as [`crate::emv2_analysis::Emv2Analysis`]),
/// then maps results to STPA artifact structures and emits diagnostics
/// summarizing the generated artifacts.
pub struct Emv2StpaBridgeAnalysis;

impl Analysis for Emv2StpaBridgeAnalysis {
    fn name(&self) -> &str {
        "emv2_stpa_bridge"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();
        let root_path = component_path(instance, instance.root);

        // Build fault tree (same structural approach as emv2_analysis)
        let tree = build_hierarchy_fault_tree(instance, instance.root);

        let result = bridge_fault_tree_to_stpa(&tree);

        let hazard_count = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "hazard")
            .count();
        let scenario_count = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "loss-scenario")
            .count();
        let sub_hazard_count = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "sub-hazard")
            .count();

        diags.push(AnalysisDiagnostic {
            severity: Severity::Info,
            message: format!(
                "EMV2-STPA bridge: generated {} hazard(s), {} loss scenario(s), \
                 {} sub-hazard(s) from fault tree of '{}'",
                hazard_count,
                scenario_count,
                sub_hazard_count,
                dotted_path(instance, instance.root),
            ),
            path: root_path,
            analysis: self.name().to_string(),
        });

        diags
    }
}

/// Build a fault tree from the component hierarchy.
/// Reuses the structural approach from `emv2_analysis`:
/// composite components are OR gates, leaves are basic events.
fn build_hierarchy_fault_tree(instance: &SystemInstance, idx: ComponentInstanceIdx) -> FaultTree {
    let path = dotted_path(instance, idx);
    let node = build_subtree(instance, idx);

    FaultTree {
        top_event: format!("failure of {}", path),
        root: node,
    }
}

fn build_subtree(instance: &SystemInstance, idx: ComponentInstanceIdx) -> FaultTreeNode {
    let comp = instance.component(idx);
    let path = dotted_path(instance, idx);

    if comp.children.is_empty() {
        FaultTreeNode::BasicEvent {
            component: path,
            error_type: "ServiceFailure".to_string(),
            description: format!("{} {} fails", category_label(comp.category), comp.name),
        }
    } else {
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

fn category_label(cat: spar_hir_def::item_tree::ComponentCategory) -> &'static str {
    match cat {
        spar_hir_def::item_tree::ComponentCategory::Process => "process",
        spar_hir_def::item_tree::ComponentCategory::Thread => "thread",
        spar_hir_def::item_tree::ComponentCategory::Device => "device",
        spar_hir_def::item_tree::ComponentCategory::Processor => "processor",
        _ => "component",
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emv2_analysis::{FaultTree, FaultTreeNode};
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::Name;
    use spar_hir_def::properties::PropertyMap;

    // ── Test builder ───────────────────────────────────────────────

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

    // ── bridge_fault_tree_to_stpa tests ────────────────────────────

    #[test]
    fn bridge_single_leaf_produces_hazard_and_sub_hazard() {
        let tree = FaultTree {
            top_event: "failure of root".to_string(),
            root: FaultTreeNode::BasicEvent {
                component: "root.sensor".to_string(),
                error_type: "ServiceFailure".to_string(),
                description: "device sensor fails".to_string(),
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);

        // Should produce: 1 hazard (top event) + 1 sub-hazard (basic event)
        assert_eq!(
            result.artifacts.len(),
            2,
            "artifacts: {:?}",
            result.artifacts
        );

        let hazard = &result.artifacts[0];
        assert_eq!(hazard.artifact_type, "hazard");
        assert_eq!(hazard.id, "H-EMV2-001");
        assert!(hazard.tags.contains(&"emv2-generated".to_string()));
        assert!(hazard.tags.contains(&"fault-tree".to_string()));

        let sub = &result.artifacts[1];
        assert_eq!(sub.artifact_type, "sub-hazard");
        assert_eq!(sub.id, "SH-EMV2-001");
        assert!(sub.description.contains("ServiceFailure"));
    }

    #[test]
    fn bridge_or_gate_produces_loss_scenario() {
        let tree = FaultTree {
            top_event: "failure of root".to_string(),
            root: FaultTreeNode::Or {
                description: "failure of root".to_string(),
                children: vec![
                    FaultTreeNode::BasicEvent {
                        component: "root.a".to_string(),
                        error_type: "FailStop".to_string(),
                        description: "a fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "root.b".to_string(),
                        error_type: "FailStop".to_string(),
                        description: "b fails".to_string(),
                    },
                ],
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);

        // hazard + loss-scenario (OR gate) + 2 sub-hazards
        let scenarios: Vec<_> = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "loss-scenario")
            .collect();
        assert_eq!(
            scenarios.len(),
            1,
            "should have 1 loss scenario: {:?}",
            scenarios
        );
        assert!(
            scenarios[0].tags.contains(&"propagation-path".to_string()),
            "OR gate scenario should be tagged as propagation-path"
        );
        assert!(
            scenarios[0].description.contains("root.a.FailStop"),
            "should mention child events: {}",
            scenarios[0].description
        );
    }

    #[test]
    fn bridge_and_gate_produces_concurrent_failure_scenario() {
        let tree = FaultTree {
            top_event: "failure of sensors".to_string(),
            root: FaultTreeNode::And {
                description: "all sensors fail".to_string(),
                children: vec![
                    FaultTreeNode::BasicEvent {
                        component: "sensors.a".to_string(),
                        error_type: "FailStop".to_string(),
                        description: "sensor A fails".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "sensors.b".to_string(),
                        error_type: "FailStop".to_string(),
                        description: "sensor B fails".to_string(),
                    },
                ],
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);

        let scenarios: Vec<_> = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "loss-scenario")
            .collect();
        assert_eq!(
            scenarios.len(),
            1,
            "should have 1 loss scenario: {:?}",
            scenarios
        );
        assert!(
            scenarios[0]
                .tags
                .contains(&"concurrent-failure".to_string()),
            "AND gate scenario should be tagged as concurrent-failure"
        );
        assert!(
            scenarios[0].description.contains("AND"),
            "should mention AND gate: {}",
            scenarios[0].description
        );
    }

    #[test]
    fn bridge_nested_tree_produces_correct_artifact_count() {
        // OR( AND(a, b), c ) -> hazard + 2 loss scenarios + 3 sub-hazards
        let tree = FaultTree {
            top_event: "failure of system".to_string(),
            root: FaultTreeNode::Or {
                description: "failure of system".to_string(),
                children: vec![
                    FaultTreeNode::And {
                        description: "redundant pair fails".to_string(),
                        children: vec![
                            FaultTreeNode::BasicEvent {
                                component: "sys.a".to_string(),
                                error_type: "Err".to_string(),
                                description: "a fails".to_string(),
                            },
                            FaultTreeNode::BasicEvent {
                                component: "sys.b".to_string(),
                                error_type: "Err".to_string(),
                                description: "b fails".to_string(),
                            },
                        ],
                    },
                    FaultTreeNode::BasicEvent {
                        component: "sys.c".to_string(),
                        error_type: "Err".to_string(),
                        description: "c fails".to_string(),
                    },
                ],
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);

        let hazards = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "hazard")
            .count();
        let scenarios = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "loss-scenario")
            .count();
        let sub_hazards = result
            .artifacts
            .iter()
            .filter(|a| a.artifact_type == "sub-hazard")
            .count();

        assert_eq!(hazards, 1, "one top-level hazard");
        assert_eq!(scenarios, 2, "OR gate + AND gate = 2 loss scenarios");
        assert_eq!(sub_hazards, 3, "3 basic events = 3 sub-hazards");
    }

    #[test]
    fn bridge_links_point_to_parent_hazard() {
        let tree = FaultTree {
            top_event: "failure of root".to_string(),
            root: FaultTreeNode::BasicEvent {
                component: "root.x".to_string(),
                error_type: "ValueError".to_string(),
                description: "x fails".to_string(),
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);
        let sub = result
            .artifacts
            .iter()
            .find(|a| a.artifact_type == "sub-hazard")
            .expect("should have sub-hazard");

        assert_eq!(sub.links.len(), 1);
        assert_eq!(sub.links[0].link_type, "refines");
        assert_eq!(sub.links[0].targets, vec!["H-EMV2-001"]);
    }

    // ── generate_stpa_yaml tests ───────────────────────────────────

    #[test]
    fn yaml_output_contains_required_fields() {
        let tree = FaultTree {
            top_event: "failure of root".to_string(),
            root: FaultTreeNode::BasicEvent {
                component: "root.sensor".to_string(),
                error_type: "ServiceFailure".to_string(),
                description: "sensor fails".to_string(),
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);
        let yaml = generate_stpa_yaml(&result);

        assert!(yaml.contains("artifacts:"), "must have artifacts key");
        assert!(yaml.contains("id: H-EMV2-001"), "must have hazard ID");
        assert!(yaml.contains("type: hazard"), "must have hazard type");
        assert!(
            yaml.contains("type: sub-hazard"),
            "must have sub-hazard type"
        );
        assert!(
            yaml.contains("emv2-generated"),
            "must have emv2-generated tag"
        );
        assert!(yaml.contains("title:"), "must have title field");
        assert!(yaml.contains("description:"), "must have description field");
    }

    #[test]
    fn yaml_output_contains_links() {
        let tree = FaultTree {
            top_event: "failure of root".to_string(),
            root: FaultTreeNode::Or {
                description: "root fails".to_string(),
                children: vec![FaultTreeNode::BasicEvent {
                    component: "root.a".to_string(),
                    error_type: "Err".to_string(),
                    description: "a fails".to_string(),
                }],
            },
        };

        let result = bridge_fault_tree_to_stpa(&tree);
        let yaml = generate_stpa_yaml(&result);

        assert!(yaml.contains("links:"), "must have links section");
        assert!(
            yaml.contains("type: leads-to-hazard"),
            "loss scenario must link to hazard"
        );
        assert!(
            yaml.contains("type: refines"),
            "sub-hazard must refine parent"
        );
    }

    #[test]
    fn yaml_escape_handles_special_chars() {
        assert_eq!(yaml_escape("plain"), "plain");
        assert_eq!(yaml_escape("has \"quotes\""), "has \\\"quotes\\\"");
        assert_eq!(yaml_escape("has \\backslash"), "has \\\\backslash");
    }

    // ── format_cut_sets tests ──────────────────────────────────────

    #[test]
    fn format_cut_sets_empty() {
        assert_eq!(format_cut_sets(&[]), "none");
    }

    #[test]
    fn format_cut_sets_single() {
        let sets = vec![vec!["a.Err".to_string()]];
        assert_eq!(format_cut_sets(&sets), "{a.Err}");
    }

    #[test]
    fn format_cut_sets_multiple() {
        let sets = vec![
            vec!["a.Err".to_string()],
            vec!["b.Err".to_string(), "c.Err".to_string()],
        ];
        assert_eq!(format_cut_sets(&sets), "{a.Err}, {b.Err, c.Err}");
    }

    // ── sanitize_title tests ───────────────────────────────────────

    #[test]
    fn sanitize_title_capitalizes() {
        assert_eq!(sanitize_title("failure of root"), "Failure of root");
    }

    #[test]
    fn sanitize_title_empty() {
        assert_eq!(sanitize_title(""), "Unnamed");
        assert_eq!(sanitize_title("  "), "Unnamed");
    }

    #[test]
    fn sanitize_title_truncates_long() {
        let long = "a".repeat(200);
        let result = sanitize_title(&long);
        assert!(result.len() <= 120, "should truncate: len={}", result.len());
        assert!(result.ends_with("..."));
    }

    // ── Analysis pass tests ────────────────────────────────────────

    #[test]
    fn analysis_pass_name() {
        let pass = Emv2StpaBridgeAnalysis;
        assert_eq!(pass.name(), "emv2_stpa_bridge");
    }

    #[test]
    fn analysis_pass_emits_summary_diagnostic() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let dev = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![dev]);

        let inst = b.build(root);
        let diags = Emv2StpaBridgeAnalysis.analyze(&inst);

        assert_eq!(
            diags.len(),
            1,
            "should have one summary diagnostic: {:?}",
            diags
        );
        assert_eq!(diags[0].severity, Severity::Info);
        assert!(
            diags[0].message.contains("EMV2-STPA bridge"),
            "should mention bridge: {}",
            diags[0].message
        );
        assert!(
            diags[0].message.contains("hazard"),
            "should mention hazard count: {}",
            diags[0].message
        );
        assert_eq!(
            diags[0].analysis, "emv2_stpa_bridge",
            "analysis field must match pass name"
        );
    }

    #[test]
    fn analysis_pass_reports_correct_counts() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Device, Some(root));
        let b_comp = b.add_component("b", ComponentCategory::Device, Some(root));
        let c = b.add_component("c", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![a, b_comp, c]);

        let inst = b.build(root);
        let diags = Emv2StpaBridgeAnalysis.analyze(&inst);

        // 3 children under OR gate -> 1 hazard, 1 loss-scenario, 3 sub-hazards
        assert!(
            diags[0].message.contains("1 hazard"),
            "should have 1 hazard: {}",
            diags[0].message
        );
        assert!(
            diags[0].message.contains("1 loss scenario"),
            "should have 1 loss scenario: {}",
            diags[0].message
        );
        assert!(
            diags[0].message.contains("3 sub-hazard"),
            "should have 3 sub-hazards: {}",
            diags[0].message
        );
    }

    #[test]
    fn analysis_pass_leaf_only_system() {
        // Root with no children: basic event only
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);

        let inst = b.build(root);
        let diags = Emv2StpaBridgeAnalysis.analyze(&inst);

        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("1 hazard"));
        assert!(diags[0].message.contains("0 loss scenario"));
        assert!(diags[0].message.contains("1 sub-hazard"));
    }

    // ── collect_basic_events tests ─────────────────────────────────

    #[test]
    fn collect_basic_events_from_nested_tree() {
        let nodes = vec![
            FaultTreeNode::Or {
                description: "or".to_string(),
                children: vec![
                    FaultTreeNode::BasicEvent {
                        component: "a".to_string(),
                        error_type: "E1".to_string(),
                        description: "a".to_string(),
                    },
                    FaultTreeNode::BasicEvent {
                        component: "b".to_string(),
                        error_type: "E2".to_string(),
                        description: "b".to_string(),
                    },
                ],
            },
            FaultTreeNode::BasicEvent {
                component: "c".to_string(),
                error_type: "E3".to_string(),
                description: "c".to_string(),
            },
        ];

        let events = collect_basic_events(&nodes);
        assert_eq!(events, vec!["a.E1", "b.E2", "c.E3"]);
    }
}
