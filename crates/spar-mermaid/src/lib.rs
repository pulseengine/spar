//! Mermaid diagram emission for spar AADL instance models.
//!
//! This crate provides:
//! - A `flowchart TD` emitter ([`emit_flowchart`]) that walks a
//!   [`spar_hir_def::instance::SystemInstance`] and produces Mermaid markup.
//! - A `classDiagram` emitter ([`emit_class_diagram`]) that produces one class
//!   per component type (with stereotypes and feature attributes).
//! - A `requirementDiagram` emitter ([`emit_requirement_diagram`]) that parses
//!   a rivet `requirements.yaml` and produces Mermaid requirement blocks and
//!   relationship edges.
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
use std::path::Path;

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

// ── classDiagram emitter ─────────────────────────────────────────────────────

/// Emit a `classDiagram` Mermaid diagram for the given [`SystemInstance`].
///
/// Each component that passes the filters in `opts` becomes one Mermaid `class`
/// block.  The class name is the component's `type_name`.  The AADL category is
/// shown as a stereotype (`<<system>>`, `<<thread>>`, etc.).  Each feature on
/// the component instance is listed as an attribute of the form
/// `+direction kind name` (direction is omitted when `None`).
///
/// Only components whose `type_name` has not yet been emitted are output; if
/// multiple instances share the same type they produce a single class block.
/// Inheritance arrows (`--|>`) are emitted when `impl_name` is `None` (type
/// reference only) and the parent component has a different `type_name`,
/// mirroring an "implements / extends" relationship.
pub fn emit_class_diagram(instance: &SystemInstance, opts: &MermaidOptions) -> String {
    let depths = compute_depths(instance);

    // Determine which components pass the filters.
    let mut seen_types: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut class_lines: Vec<String> = Vec::new();
    let mut relation_lines: Vec<String> = Vec::new();

    // Collect filtered components sorted for deterministic output.
    let mut filtered: Vec<ComponentInstanceIdx> = instance
        .all_components()
        .filter(|(idx, comp)| {
            if !opts.categories.is_empty() && !opts.categories.contains(&comp.category) {
                return false;
            }
            if let Some(max_d) = opts.max_depth {
                let d = depths.get(idx).copied().unwrap_or(0);
                if d > max_d {
                    return false;
                }
            }
            true
        })
        .map(|(idx, _)| idx)
        .collect();

    // Sort by type_name for deterministic output.
    filtered.sort_by_key(|idx| instance.component(*idx).type_name.as_str().to_string());

    for idx in &filtered {
        let comp = instance.component(*idx);
        let type_name = comp.type_name.as_str().to_string();

        if seen_types.contains(&type_name) {
            continue;
        }
        seen_types.insert(type_name.clone());

        let stereotype = category_stereotype(comp.category);
        let mut block = format!("    class {} {{\n", sanitize_id(&type_name));
        block.push_str(&format!("        <<{}>>\n", stereotype));

        // Emit features as attributes.
        for &feat_idx in &comp.features {
            let feat = &instance.features[feat_idx];
            let dir_str = match feat.direction {
                Some(dir) => format!("{} ", dir),
                None => String::new(),
            };
            block.push_str(&format!(
                "        +{}{} {}\n",
                dir_str,
                feat.kind,
                feat.name.as_str()
            ));
        }
        block.push_str("    }");
        class_lines.push(block);

        // Emit inheritance arrow from child type to parent type when the parent
        // has a distinct type.
        if let Some(parent_idx) = comp.parent {
            let parent_comp = instance.component(parent_idx);
            let parent_type = sanitize_id(parent_comp.type_name.as_str());
            let child_type = sanitize_id(&type_name);
            if parent_type != child_type {
                let arrow = format!("    {} --|> {}", child_type, parent_type);
                if !relation_lines.contains(&arrow) {
                    relation_lines.push(arrow);
                }
            }
        }
    }

    let mut out = String::from("classDiagram\n");
    for line in &class_lines {
        out.push_str(line);
        out.push('\n');
    }
    relation_lines.sort();
    relation_lines.dedup();
    for line in &relation_lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Map a [`ComponentCategory`] to the Mermaid stereotype string (without angle brackets).
fn category_stereotype(cat: ComponentCategory) -> &'static str {
    match cat {
        ComponentCategory::System => "system",
        ComponentCategory::Process => "process",
        ComponentCategory::Thread => "thread",
        ComponentCategory::ThreadGroup => "thread group",
        ComponentCategory::Processor => "processor",
        ComponentCategory::VirtualProcessor => "virtual processor",
        ComponentCategory::Memory => "memory",
        ComponentCategory::Bus => "bus",
        ComponentCategory::VirtualBus => "virtual bus",
        ComponentCategory::Device => "device",
        ComponentCategory::Subprogram => "subprogram",
        ComponentCategory::SubprogramGroup => "subprogram group",
        ComponentCategory::Data => "data",
        ComponentCategory::Abstract => "abstract",
    }
}

// ── requirementDiagram emitter ───────────────────────────────────────────────

/// Error type for [`emit_requirement_diagram`].
#[derive(Debug)]
pub enum MermaidError {
    /// The requirements YAML file could not be read.
    Io(std::io::Error),
}

impl std::fmt::Display for MermaidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MermaidError::Io(e) => write!(f, "IO error reading requirements YAML: {e}"),
        }
    }
}

impl std::error::Error for MermaidError {}

impl From<std::io::Error> for MermaidError {
    fn from(e: std::io::Error) -> Self {
        MermaidError::Io(e)
    }
}

/// Emit a `requirementDiagram` Mermaid diagram from a rivet requirements YAML file.
///
/// Parses `yaml_path` using the same line-oriented parser as
/// `spar_sysml2::generate::parse_rivet_yaml`.  Only artifacts whose `type` is
/// one of `requirement`, `feature`, `functional-requirement`, or
/// `performance-requirement` are emitted as requirement blocks.  All other
/// artifact types (e.g. `design-decision`) are skipped.
///
/// Relationship edges (`satisfies`, `verifies`, `derives`, `refines`, `traces`)
/// found in the `links:` section are emitted as Mermaid requirement relationship
/// lines: `<src> - <rel> -> <dst>`.
///
/// The Mermaid `requirementDiagram` grammar used here:
/// ```text
/// requirementDiagram
///
///     requirement REQ_ID {
///         id: "REQ-ID"
///         text: "Title"
///         risk: undefined
///         verifymethod: analysis
///     }
///
///     REQ_A - satisfies -> REQ_B
/// ```
///
/// Note: requirement `id` node names must be valid Mermaid identifiers (no
/// hyphens).  This function sanitises IDs by replacing `-` with `_`.
pub fn emit_requirement_diagram(yaml_path: &Path) -> Result<String, MermaidError> {
    let content = std::fs::read_to_string(yaml_path)?;
    Ok(emit_requirement_diagram_from_str(&content))
}

/// Inner implementation that works on an already-loaded YAML string.
/// Exposed for unit-test convenience (no file I/O).
pub fn emit_requirement_diagram_from_str(yaml: &str) -> String {
    // ── 1. Parse artifacts ──────────────────────────────────────────────────
    let artifacts = parse_req_yaml(yaml);

    // ── 2. Determine which artifact types produce a requirement block ───────
    let is_req_type = |t: &str| {
        matches!(
            t,
            "requirement"
                | "feature"
                | "functional-requirement"
                | "performance-requirement"
                | "functionalRequirement"
                | "performanceRequirement"
        )
    };

    // Collect the set of emitted IDs (sanitised) for edge validation.
    let emitted_ids: std::collections::HashSet<String> = artifacts
        .iter()
        .filter(|a| is_req_type(&a.artifact_type))
        .map(|a| sanitize_req_id(&a.id))
        .collect();

    let mut out = String::from("requirementDiagram\n\n");

    // ── 3. Emit requirement blocks ──────────────────────────────────────────
    for artifact in &artifacts {
        if !is_req_type(&artifact.artifact_type) {
            continue;
        }
        let req_keyword = match artifact.artifact_type.as_str() {
            "feature" | "functional-requirement" | "functionalRequirement" => {
                "functionalRequirement"
            }
            "performance-requirement" | "performanceRequirement" => "performanceRequirement",
            _ => "requirement",
        };
        let node_id = sanitize_req_id(&artifact.id);
        // Truncate description to first sentence for the `text` field.
        let text = artifact
            .title
            .replace('"', "'")
            .chars()
            .take(120)
            .collect::<String>();
        out.push_str(&format!("    {} {} {{\n", req_keyword, node_id));
        out.push_str(&format!("        id: \"{}\"\n", artifact.id));
        out.push_str(&format!("        text: \"{}\"\n", text));
        out.push_str("        risk: undefined\n");
        out.push_str("        verifymethod: analysis\n");
        out.push_str("    }\n\n");
    }

    // ── 4. Emit relationship edges ──────────────────────────────────────────
    let mut edges: Vec<String> = Vec::new();
    for artifact in &artifacts {
        if !is_req_type(&artifact.artifact_type) {
            continue;
        }
        let src_id = sanitize_req_id(&artifact.id);
        for link in &artifact.links {
            let rel = match link.link_type.as_str() {
                "satisfies" => "satisfies",
                "verifies" => "verifies",
                "derives" => "derives",
                "refines" => "refines",
                "traces" => "traces",
                _ => continue,
            };
            let dst_id = sanitize_req_id(&link.target);
            // Only emit if both endpoints were emitted.
            if emitted_ids.contains(&dst_id) {
                edges.push(format!("    {} - {} -> {}", src_id, rel, dst_id));
            }
        }
    }
    edges.sort();
    edges.dedup();
    for edge in &edges {
        out.push_str(edge);
        out.push('\n');
    }

    out
}

/// Sanitise a rivet artifact ID for use as a Mermaid requirement node name.
///
/// Replaces `-` with `_` so that `REQ-PARSE-001` becomes `REQ_PARSE_001`.
fn sanitize_req_id(id: &str) -> String {
    id.replace('-', "_")
}

// ── Minimal YAML parser (requirement artifacts only) ──────────────────────────

/// A minimal parsed artifact (id, type, title, links).
struct ReqArtifact {
    id: String,
    artifact_type: String,
    title: String,
    links: Vec<ReqLink>,
}

struct ReqLink {
    link_type: String,
    target: String,
}

/// Parse rivet YAML for requirement-diagram purposes.
///
/// This is a line-oriented parser that mirrors the logic in
/// `spar_sysml2::generate::parse_rivet_yaml` but is local to this crate to
/// avoid a cross-crate dependency.
fn parse_req_yaml(yaml: &str) -> Vec<ReqArtifact> {
    let mut artifacts: Vec<ReqArtifact> = Vec::new();
    let mut id: Option<String> = None;
    let mut artifact_type = String::new();
    let mut title = String::new();
    let mut links: Vec<ReqLink> = Vec::new();
    let mut in_links = false;
    let mut pending_link_type: Option<String> = None;

    let flush = |id: &mut Option<String>,
                 artifact_type: &mut String,
                 title: &mut String,
                 links: &mut Vec<ReqLink>,
                 in_links: &mut bool,
                 pending_link_type: &mut Option<String>,
                 artifacts: &mut Vec<ReqArtifact>| {
        if let Some(pending_lt) = pending_link_type.take() {
            links.push(ReqLink {
                link_type: pending_lt,
                target: String::new(),
            });
        }
        if let Some(artifact_id) = id.take() {
            artifacts.push(ReqArtifact {
                id: artifact_id,
                artifact_type: std::mem::take(artifact_type),
                title: std::mem::take(title),
                links: std::mem::take(links),
            });
        }
        *in_links = false;
    };

    for line in yaml.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("- id:") {
            flush(
                &mut id,
                &mut artifact_type,
                &mut title,
                &mut links,
                &mut in_links,
                &mut pending_link_type,
                &mut artifacts,
            );
            id = Some(trimmed.trim_start_matches("- id:").trim().to_string());
            continue;
        }

        if id.is_none() {
            continue;
        }

        if trimmed.starts_with("type:") && !in_links {
            artifact_type = trimmed.trim_start_matches("type:").trim().to_string();
        } else if trimmed.starts_with("title:") {
            let val = trimmed.trim_start_matches("title:").trim();
            title = val.trim_matches('"').to_string();
        } else if trimmed == "links:" {
            in_links = true;
        } else if in_links && trimmed.starts_with("- type:") {
            // Flush pending link.
            if let Some(plt) = pending_link_type.take() {
                links.push(ReqLink {
                    link_type: plt,
                    target: String::new(),
                });
            }
            pending_link_type = Some(trimmed.trim_start_matches("- type:").trim().to_string());
        } else if in_links && trimmed.starts_with("target:") {
            let tgt = trimmed.trim_start_matches("target:").trim().to_string();
            if let Some(plt) = pending_link_type.take() {
                links.push(ReqLink {
                    link_type: plt,
                    target: tgt,
                });
            }
        }
    }

    // Flush last artifact.
    flush(
        &mut id,
        &mut artifact_type,
        &mut title,
        &mut links,
        &mut in_links,
        &mut pending_link_type,
        &mut artifacts,
    );

    artifacts
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

    // ── classDiagram tests ───────────────────────────────────────────────────

    /// Build an instance with distinct type_name per component for classDiagram tests.
    fn class_instance() -> SystemInstance {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let mut comp_idxs: Vec<ComponentInstanceIdx> = Vec::new();

        let specs: &[(&str, &str, ComponentCategory)] = &[
            ("root", "RootSys", ComponentCategory::System),
            ("worker", "WorkerThread", ComponentCategory::Thread),
            ("cpu", "MainCpu", ComponentCategory::Processor),
        ];

        for (name, type_name, cat) in specs {
            let idx = components.alloc(ComponentInstance {
                name: Name::new(name),
                category: *cat,
                type_name: Name::new(type_name),
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

        // Wire root → worker, root → cpu
        for i in 1..3 {
            components[comp_idxs[i]].parent = Some(comp_idxs[0]);
            components[comp_idxs[0]].children.push(comp_idxs[i]);
        }

        let root = comp_idxs[0];
        SystemInstance {
            root,
            components,
            features: Arena::default(),
            connections: Arena::default(),
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

    #[test]
    fn test_class_diagram_one_class_per_component() {
        let instance = class_instance();
        let opts = MermaidOptions::default();
        let out = emit_class_diagram(&instance, &opts);

        assert!(
            out.starts_with("classDiagram\n"),
            "expected 'classDiagram' header; got:\n{out}"
        );
        // Each distinct type name should appear once as a class block.
        assert!(out.contains("class RootSys"), "expected class RootSys");
        assert!(
            out.contains("class WorkerThread"),
            "expected class WorkerThread"
        );
        assert!(out.contains("class MainCpu"), "expected class MainCpu");
    }

    #[test]
    fn test_class_diagram_stereotype_shows_category() {
        let instance = class_instance();
        let opts = MermaidOptions::default();
        let out = emit_class_diagram(&instance, &opts);

        assert!(out.contains("<<system>>"), "expected <<system>> stereotype");
        assert!(out.contains("<<thread>>"), "expected <<thread>> stereotype");
        assert!(
            out.contains("<<processor>>"),
            "expected <<processor>> stereotype"
        );
    }

    #[test]
    fn test_class_diagram_category_filter_excludes_processor() {
        let instance = class_instance();
        let opts = MermaidOptions {
            categories: vec![ComponentCategory::System, ComponentCategory::Thread],
            ..Default::default()
        };
        let out = emit_class_diagram(&instance, &opts);

        assert!(
            !out.contains("MainCpu"),
            "processor class MainCpu should be absent when filtered"
        );
        assert!(
            out.contains("WorkerThread"),
            "thread class should be present"
        );
    }

    // ── requirementDiagram tests ─────────────────────────────────────────────

    const REQ_YAML: &str = r#"
artifacts:

  - id: REQ-ALPHA-001
    type: requirement
    title: Alpha requirement
    description: First requirement.
    status: implemented
    tags: [alpha]
    links:
      - type: satisfies
        target: REQ-BETA-001

  - id: REQ-BETA-001
    type: requirement
    title: Beta requirement
    description: Second requirement.
    status: implemented
    tags: [beta]

  - id: DEC-IGNORE-001
    type: design-decision
    title: Ignored decision
    description: Should not appear in diagram.
    status: implemented
    tags: [design]
"#;

    #[test]
    fn test_req_diagram_header_and_block() {
        let out = emit_requirement_diagram_from_str(REQ_YAML);

        assert!(
            out.starts_with("requirementDiagram\n"),
            "expected 'requirementDiagram' header; got:\n{out}"
        );
        // At least one requirement block should be present.
        assert!(
            out.contains("requirement REQ_ALPHA_001"),
            "expected REQ_ALPHA_001 block; got:\n{out}"
        );
        assert!(
            out.contains("requirement REQ_BETA_001"),
            "expected REQ_BETA_001 block; got:\n{out}"
        );
    }

    #[test]
    fn test_req_diagram_satisfies_edge() {
        let out = emit_requirement_diagram_from_str(REQ_YAML);

        assert!(
            out.contains("REQ_ALPHA_001 - satisfies -> REQ_BETA_001"),
            "expected satisfies edge; got:\n{out}"
        );
    }

    #[test]
    fn test_req_diagram_skips_design_decisions() {
        let out = emit_requirement_diagram_from_str(REQ_YAML);

        assert!(
            !out.contains("DEC_IGNORE_001"),
            "design-decision artifact should be excluded; got:\n{out}"
        );
    }
}
