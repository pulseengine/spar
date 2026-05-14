//! Mermaid diagram emission for spar AADL instance models.
//!
//! This crate provides a foundation-level `flowchart TD` emitter that walks a
//! [`spar_hir_def::instance::SystemInstance`] and produces Mermaid markup.
//! It intentionally covers only `flowchart` emission; `classDiagram`,
//! `requirementDiagram`, and `block-beta` are follow-on work.
//!
//! # Cyclic-containment assumption
//!
//! [`SystemInstance`] is produced by `spar_hir_def::instance::SystemInstance::instantiate`,
//! which already runs a cycle-detection pass before building the arena.
//! This emitter therefore trusts the `parent`/`children` links are acyclic
//! and does not re-check for cycles.  If a caller constructs a `SystemInstance`
//! manually with a cycle in `children`, the depth filter (`max_depth`) provides
//! a natural termination bound — but only when set to `Some(_)`.  Construction
//! of broken instances is a caller bug.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::Name;
use std::collections::HashMap;

/// Options controlling which parts of a [`SystemInstance`] are emitted.
#[derive(Debug, Clone)]
pub struct MermaidOptions {
    /// Limit emission to components whose category is in this set.
    /// Empty = emit all categories.
    pub categories: Vec<ComponentCategory>,
    /// Limit emission to components within this many parent-hops from the root.
    /// `None` = emit all depths.
    pub max_depth: Option<usize>,
    /// Include connection edges in the diagram (default `true`).
    pub include_connections: bool,
}

impl Default for MermaidOptions {
    fn default() -> Self {
        Self {
            categories: Vec::new(),
            max_depth: None,
            include_connections: true,
        }
    }
}

/// Emit a `flowchart TD` Mermaid diagram for the given [`SystemInstance`].
///
/// Each component that passes the filters in `opts` becomes one node.
/// Each [`spar_hir_def::instance::ConnectionInstance`] whose source and
/// destination both resolve to emitted nodes becomes one directed edge.
///
/// # Node identifiers
///
/// Mermaid node IDs must not contain spaces or special characters.
/// This function sanitises component names by replacing every non-alphanumeric
/// ASCII byte with `_`.  Array indices are preserved via the index suffix.
///
/// # Edge resolution
///
/// [`spar_hir_def::instance::ConnectionEnd`] identifies an endpoint by an
/// optional subcomponent [`Name`] plus a feature [`Name`].  When the
/// subcomponent name is `None` the endpoint refers to the owning component
/// itself.  Resolution looks up the subcomponent name among the owner's
/// direct children using case-insensitive comparison (AADL §3.1).
pub fn emit_flowchart(instance: &SystemInstance, opts: &MermaidOptions) -> String {
    // ── 1. Collect emitted node set ─────────────────────────────────────────

    // Build a map from ComponentInstanceIdx → depth in the tree.
    let depths = compute_depths(instance);

    // Determine which components pass the filters.
    let emitted: HashMap<ComponentInstanceIdx, String> = instance
        .all_components()
        .filter(|(idx, comp)| {
            // Category filter.
            if !opts.categories.is_empty() && !opts.categories.contains(&comp.category) {
                return false;
            }
            // Depth filter.
            if let Some(max_d) = opts.max_depth {
                let d = depths.get(idx).copied().unwrap_or(0);
                if d > max_d {
                    return false;
                }
            }
            true
        })
        .map(|(idx, comp)| {
            let node_id = sanitize_id(comp.name.as_str());
            (idx, node_id)
        })
        .collect();

    // ── 2. Build output ─────────────────────────────────────────────────────

    let mut out = String::from("flowchart TD\n");

    // Sort by node id for deterministic output.
    let mut node_entries: Vec<(ComponentInstanceIdx, &String)> =
        emitted.iter().map(|(idx, id)| (*idx, id)).collect();
    node_entries.sort_by_key(|(_, id)| id.as_str());

    for (idx, node_id) in &node_entries {
        let comp = instance.component(*idx);
        let label = format!("{}: {}", comp.category, comp.name);
        out.push_str(&format!("    {}[\"{}\"]\n", node_id, label));
    }

    // ── 3. Edges ────────────────────────────────────────────────────────────

    if opts.include_connections {
        // Build a lookup: owner idx → Vec<(child_name_lc, child_idx)>
        // so we can resolve ConnectionEnd.subcomponent by name.
        let children_by_name: HashMap<ComponentInstanceIdx, Vec<(String, ComponentInstanceIdx)>> =
            instance
                .all_components()
                .map(|(owner_idx, comp)| {
                    let children: Vec<(String, ComponentInstanceIdx)> = comp
                        .children
                        .iter()
                        .map(|&child_idx| {
                            let child = instance.component(child_idx);
                            (child.name.as_str().to_ascii_lowercase(), child_idx)
                        })
                        .collect();
                    (owner_idx, children)
                })
                .collect();

        let mut edges: Vec<(String, String)> = Vec::new();

        for (_, conn) in instance.connections.iter() {
            let (Some(src_end), Some(dst_end)) = (&conn.src, &conn.dst) else {
                continue;
            };

            let src_idx =
                resolve_endpoint(conn.owner, src_end.subcomponent.as_ref(), &children_by_name);
            let dst_idx =
                resolve_endpoint(conn.owner, dst_end.subcomponent.as_ref(), &children_by_name);

            let (Some(src_idx), Some(dst_idx)) = (src_idx, dst_idx) else {
                continue;
            };

            let (Some(src_id), Some(dst_id)) = (emitted.get(&src_idx), emitted.get(&dst_idx))
            else {
                continue;
            };

            edges.push((src_id.clone(), dst_id.clone()));
        }

        // Deduplicate and sort for deterministic output.
        edges.sort();
        edges.dedup();

        for (src, dst) in edges {
            out.push_str(&format!("    {} --> {}\n", src, dst));
        }
    }

    out
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Compute the depth (parent-hop distance from root) for every component.
fn compute_depths(instance: &SystemInstance) -> HashMap<ComponentInstanceIdx, usize> {
    let mut depths = HashMap::new();
    depths.insert(instance.root, 0usize);

    // BFS from root using the children links.
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(instance.root);

    while let Some(idx) = queue.pop_front() {
        let depth = depths[&idx];
        let comp = instance.component(idx);
        for &child in &comp.children {
            depths.entry(child).or_insert_with(|| {
                queue.push_back(child);
                depth + 1
            });
        }
    }

    depths
}

/// Resolve a [`ConnectionEnd`] subcomponent name to a [`ComponentInstanceIdx`].
///
/// If `subcomponent` is `None` the endpoint is the owner component itself.
/// Otherwise we search the owner's direct children case-insensitively.
fn resolve_endpoint(
    owner: ComponentInstanceIdx,
    subcomponent: Option<&Name>,
    children_by_name: &HashMap<ComponentInstanceIdx, Vec<(String, ComponentInstanceIdx)>>,
) -> Option<ComponentInstanceIdx> {
    match subcomponent {
        None => Some(owner),
        Some(sub_name) => {
            let name_lc = sub_name.as_str().to_ascii_lowercase();
            children_by_name
                .get(&owner)?
                .iter()
                .find(|(n, _)| n == &name_lc)
                .map(|(_, idx)| *idx)
        }
    }
}

/// Sanitise an AADL identifier for use as a Mermaid node ID.
///
/// Replaces every character that is not ASCII alphanumeric or `_` with `_`.
fn sanitize_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::{
        ComponentInstance, ConnectionEnd, ConnectionInstance, SystemInstance,
    };
    use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind};
    use spar_hir_def::name::Name;

    /// Build a minimal [`SystemInstance`] with the given components and connections
    /// for use in unit tests.
    ///
    /// Components are inserted in `spec` order; the first element is the root.
    /// `connections` is a list of `(conn_name, owner_idx_in_spec, src_sub, dst_sub)`
    /// where `src_sub` / `dst_sub` are indices into `spec` converted to Names.
    fn build_instance(
        spec: &[(&str, ComponentCategory)],          // (name, category)
        parent_map: &[(usize, usize)],               // (child_idx, parent_idx) in spec
        connections: &[(&str, usize, usize, usize)], // (name, owner, src_child, dst_child)
    ) -> SystemInstance {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut comp_idxs: Vec<ComponentInstanceIdx> = Vec::new();

        // First pass: allocate all components.
        for (name, cat) in spec {
            let idx = components.alloc(ComponentInstance {
                name: Name::new(name),
                category: *cat,
                type_name: Name::new("T"),
                impl_name: None,
                package: Name::new("Pkg"),
                parent: None,
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: None,
                in_modes: Vec::new(),
            });
            comp_idxs.push(idx);
        }

        // Second pass: wire parent/children.
        for &(child_i, parent_i) in parent_map {
            let child_idx = comp_idxs[child_i];
            let parent_idx = comp_idxs[parent_i];
            components[child_idx].parent = Some(parent_idx);
            components[parent_idx].children.push(child_idx);
        }

        // Third pass: allocate connections.
        let mut conns: Arena<ConnectionInstance> = Arena::default();
        for &(conn_name, owner_i, src_i, dst_i) in connections {
            let owner_idx = comp_idxs[owner_i];
            let src_name = spec[src_i].0;
            let dst_name = spec[dst_i].0;
            let conn_idx = conns.alloc(ConnectionInstance {
                name: Name::new(conn_name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner: owner_idx,
                src: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(src_name)),
                    feature: Name::new("out"),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(dst_name)),
                    feature: Name::new("in"),
                }),
                in_modes: Vec::new(),
            });
            components[owner_idx].connections.push(conn_idx);
        }

        let root = comp_idxs[0];

        SystemInstance {
            root,
            components,
            features: Arena::default(),
            connections: conns,
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

    /// Returns a canonical small system: root system, 3 threads, 1 processor, 2 connections.
    ///
    /// Layout:
    ///   root (system)
    ///     ├─ t1 (thread)
    ///     ├─ t2 (thread)
    ///     ├─ t3 (thread)
    ///     └─ cpu (processor)
    ///   conn1: t1 → t2  (owner = root)
    ///   conn2: t2 → t3  (owner = root)
    fn canonical_instance() -> SystemInstance {
        build_instance(
            &[
                ("root", ComponentCategory::System),
                ("t1", ComponentCategory::Thread),
                ("t2", ComponentCategory::Thread),
                ("t3", ComponentCategory::Thread),
                ("cpu", ComponentCategory::Processor),
            ],
            &[(1, 0), (2, 0), (3, 0), (4, 0)],
            &[("conn1", 0, 1, 2), ("conn2", 0, 2, 3)],
        )
    }

    #[test]
    fn test_flowchart_header_and_nodes() {
        let instance = canonical_instance();
        let opts = MermaidOptions::default();
        let out = emit_flowchart(&instance, &opts);

        assert!(
            out.starts_with("flowchart TD\n"),
            "missing flowchart header"
        );
        assert!(out.contains("t1"), "missing t1");
        assert!(out.contains("t2"), "missing t2");
        assert!(out.contains("t3"), "missing t3");
        assert!(out.contains("cpu"), "missing cpu");
        assert!(out.contains("-->"), "missing at least one edge");
    }

    #[test]
    fn test_category_filter_excludes_processor() {
        let instance = canonical_instance();
        let opts = MermaidOptions {
            categories: vec![ComponentCategory::Thread, ComponentCategory::System],
            ..Default::default()
        };
        let out = emit_flowchart(&instance, &opts);

        assert!(!out.contains("cpu"), "cpu (Processor) should be absent");
        assert!(out.contains("t1"), "t1 (Thread) should be present");
    }

    #[test]
    fn test_max_depth_zero_emits_only_root() {
        let instance = canonical_instance();
        let opts = MermaidOptions {
            max_depth: Some(0),
            ..Default::default()
        };
        let out = emit_flowchart(&instance, &opts);

        // Only the root node (depth 0) should appear.
        assert!(out.contains("root"), "root should be present");
        // Depth-1 components must be absent.
        assert!(!out.contains("t1"), "t1 at depth 1 should be absent");
        assert!(!out.contains("cpu"), "cpu at depth 1 should be absent");
    }

    #[test]
    fn test_no_connections_flag() {
        let instance = canonical_instance();
        let opts = MermaidOptions {
            include_connections: false,
            ..Default::default()
        };
        let out = emit_flowchart(&instance, &opts);

        assert!(!out.contains("-->"), "edges should be suppressed");
    }

    #[test]
    fn test_sanitize_id_special_chars() {
        assert_eq!(sanitize_id("foo_bar"), "foo_bar");
        assert_eq!(sanitize_id("foo-bar"), "foo_bar");
        assert_eq!(sanitize_id("foo bar"), "foo_bar");
        assert_eq!(sanitize_id("foo.bar"), "foo_bar");
    }

    #[test]
    fn test_label_format() {
        let instance = canonical_instance();
        let opts = MermaidOptions::default();
        let out = emit_flowchart(&instance, &opts);
        // Labels should be "category: name"
        assert!(out.contains("thread: t1"), "expected 'thread: t1' in label");
        assert!(
            out.contains("processor: cpu"),
            "expected 'processor: cpu' in label"
        );
    }
}
