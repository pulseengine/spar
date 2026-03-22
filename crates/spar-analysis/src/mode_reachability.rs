//! Mode reachability analysis and NuSMV export (AS5506 §12).
//!
//! Provides:
//! - **Reachability matrix**: which modes are reachable from which via transitions
//! - **Unreachable mode detection**: modes with no incoming transitions except initial
//! - **Dead transition detection**: transitions whose trigger ports have no incoming connections
//! - **NuSMV (.smv) export**: formal model checking export for mode state machines
//!
//! References: AS5506 §12 (modes), §14.5 (SOMs), OSATE2 `org.osate.analysis.modes`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as FmtWrite;

use spar_hir_def::instance::{ComponentInstanceIdx, ModeTransitionInstance, SystemInstance};

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// Reachability matrix for a single modal component.
#[derive(Debug, Clone)]
pub struct ReachabilityMatrix {
    /// Component path (for display).
    pub component_path: Vec<String>,
    /// Component instance index.
    pub component_idx: ComponentInstanceIdx,
    /// Ordered list of mode names.
    pub modes: Vec<String>,
    /// Initial mode name.
    pub initial_mode: String,
    /// matrix[i][j] = true if mode j is reachable from mode i via transitions.
    pub matrix: Vec<Vec<bool>>,
    /// Modes not reachable from the initial mode.
    pub unreachable: Vec<String>,
    /// Transitions whose trigger ports have no incoming connections.
    pub dead_transitions: Vec<DeadTransition>,
}

/// A transition whose trigger port is never connected.
#[derive(Debug, Clone)]
pub struct DeadTransition {
    pub name: String,
    pub source: String,
    pub destination: String,
    pub trigger: String,
}

/// Mode reachability analysis pass.
///
/// For each modal component, computes a reachability matrix showing which
/// modes can reach which others via transition paths. Flags unreachable
/// modes and dead transitions.
pub struct ModeReachabilityAnalysis;

impl Analysis for ModeReachabilityAnalysis {
    fn name(&self) -> &str {
        "mode_reachability"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — unreachable mode, dead transition (trigger port not connected)
        //   Info    — reachability matrix summary for modal components
        let mut diags = Vec::new();

        for matrix in compute_reachability_matrices(instance) {
            let path = &matrix.component_path;

            // Report unreachable modes
            for mode in &matrix.unreachable {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "mode '{}' is not reachable from initial mode '{}' \
                         via any transition path",
                        mode, matrix.initial_mode
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Report dead transitions
            for dt in &matrix.dead_transitions {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "transition '{}' ({} -> {}) has trigger port '{}' \
                         with no incoming connections — transition can never fire",
                        dt.name, dt.source, dt.destination, dt.trigger
                    ),
                    path: path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Info: reachability summary
            let reachable_count = matrix.modes.len().saturating_sub(matrix.unreachable.len());
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "mode reachability: {}/{} modes reachable from initial mode '{}'",
                    reachable_count,
                    matrix.modes.len(),
                    matrix.initial_mode
                ),
                path: path.clone(),
                analysis: self.name().to_string(),
            });
        }

        diags
    }
}

/// Compute reachability matrices for all modal components in the instance.
pub fn compute_reachability_matrices(instance: &SystemInstance) -> Vec<ReachabilityMatrix> {
    let mut results = Vec::new();

    for (comp_idx, _comp) in instance.all_components() {
        let modes = instance.modes_for(comp_idx);
        let transitions = instance.mode_transitions_for(comp_idx);

        if modes.len() < 2 || transitions.is_empty() {
            continue;
        }

        let initial = modes.iter().find(|m| m.is_initial);
        let initial_name = match initial {
            Some(m) => m.name.as_str().to_string(),
            None => continue,
        };

        let path = component_path(instance, comp_idx);

        // Build ordered mode name list
        let mode_names: Vec<String> = modes.iter().map(|m| m.name.as_str().to_string()).collect();
        let mode_index: BTreeMap<String, usize> = mode_names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_ascii_lowercase(), i))
            .collect();
        let n = mode_names.len();

        // Direct adjacency: adj[i] contains set of directly reachable mode indices from i
        let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
        for mt in &transitions {
            let src_lower = mt.source.as_str().to_ascii_lowercase();
            let dst_lower = mt.destination.as_str().to_ascii_lowercase();
            if let (Some(&si), Some(&di)) = (mode_index.get(&src_lower), mode_index.get(&dst_lower))
            {
                adj[si].insert(di);
            }
        }

        // Compute transitive closure via BFS from each mode
        let mut matrix = vec![vec![false; n]; n];
        for start in 0..n {
            let mut visited = vec![false; n];
            visited[start] = true;
            let mut queue = Vec::new();
            for &next in &adj[start] {
                if !visited[next] {
                    visited[next] = true;
                    queue.push(next);
                }
            }
            while let Some(current) = queue.pop() {
                for &next in &adj[current] {
                    if !visited[next] {
                        visited[next] = true;
                        queue.push(next);
                    }
                }
            }
            matrix[start] = visited;
        }

        // Determine unreachable modes from initial
        let initial_lower = initial_name.to_ascii_lowercase();
        let initial_idx = mode_index.get(&initial_lower).copied().unwrap_or(0);
        let unreachable: Vec<String> = mode_names
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != initial_idx && !matrix[initial_idx][*i])
            .map(|(_, name)| name.clone())
            .collect();

        // Detect dead transitions (trigger port with no incoming connections)
        let dead_transitions = find_dead_transitions(instance, comp_idx, &transitions);

        results.push(ReachabilityMatrix {
            component_path: path,
            component_idx: comp_idx,
            modes: mode_names,
            initial_mode: initial_name,
            matrix,
            unreachable,
            dead_transitions,
        });
    }

    results
}

/// Find transitions whose trigger ports have no incoming connections.
fn find_dead_transitions(
    instance: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    transitions: &[&ModeTransitionInstance],
) -> Vec<DeadTransition> {
    let comp = instance.component(comp_idx);
    let mut dead = Vec::new();

    // Collect feature names that have incoming connections
    // A feature has an incoming connection if it appears as a destination
    // in any connection on this component or its parent
    let mut connected_features: BTreeSet<String> = BTreeSet::new();

    // Check connections owned by this component
    for &conn_idx in &comp.connections {
        let conn = &instance.connections[conn_idx];
        if let Some(ref dst) = conn.dst
            && dst.subcomponent.is_none()
        {
            connected_features.insert(dst.feature.as_str().to_ascii_lowercase());
        }
    }

    // Check semantic connections targeting features on this component
    for sc in &instance.semantic_connections {
        if sc.ultimate_destination.0 == comp_idx {
            connected_features.insert(sc.ultimate_destination.1.as_str().to_ascii_lowercase());
        }
    }

    // Check connections from parent that target this component's features
    if let Some(parent_idx) = comp.parent {
        let parent = instance.component(parent_idx);
        for &conn_idx in &parent.connections {
            let conn = &instance.connections[conn_idx];
            if let Some(ref dst) = conn.dst
                && dst
                    .subcomponent
                    .as_ref()
                    .is_some_and(|s| s.as_str().eq_ignore_ascii_case(comp.name.as_str()))
            {
                connected_features.insert(dst.feature.as_str().to_ascii_lowercase());
            }
        }
    }

    for mt in transitions {
        for trigger in &mt.triggers {
            let trigger_lower = trigger.as_str().to_ascii_lowercase();
            if !connected_features.contains(&trigger_lower) {
                let mt_label = mt
                    .name
                    .as_ref()
                    .map(|n| n.as_str().to_string())
                    .unwrap_or_else(|| {
                        format!("{}->{}", mt.source.as_str(), mt.destination.as_str())
                    });
                dead.push(DeadTransition {
                    name: mt_label,
                    source: mt.source.as_str().to_string(),
                    destination: mt.destination.as_str().to_string(),
                    trigger: trigger.as_str().to_string(),
                });
            }
        }
    }

    dead
}

// ── NuSMV Export ─────────────────────────────────────────────────────

/// Generate NuSMV (.smv) model for all modal components in the instance.
///
/// Each modal component becomes a MODULE with:
/// - A `state` variable enumerating its modes
/// - INIT constraint setting the initial mode
/// - TRANS constraints encoding allowed transitions
///
/// The `main` module composes all modal component modules.
pub fn export_smv(instance: &SystemInstance) -> String {
    let matrices = compute_reachability_matrices(instance);
    if matrices.is_empty() {
        return String::from("-- No modal components found.\n");
    }

    let mut out = String::new();
    writeln!(
        out,
        "-- NuSMV model generated by spar mode_reachability analysis"
    )
    .unwrap();
    writeln!(out, "-- AS5506 §12 mode state machines").unwrap();
    writeln!(out).unwrap();

    // Generate a MODULE for each modal component
    let mut module_names = Vec::new();

    for matrix in &matrices {
        let module_name = sanitize_smv_id(&matrix.component_path.join("_"));
        module_names.push(module_name.clone());

        let transitions = instance.mode_transitions_for(matrix.component_idx);

        writeln!(out, "MODULE {module_name}()").unwrap();
        writeln!(out, "  VAR").unwrap();

        // State variable
        let mode_list: Vec<String> = matrix.modes.iter().map(|m| sanitize_smv_id(m)).collect();
        writeln!(out, "    state : {{{}}};", mode_list.join(", ")).unwrap();
        writeln!(out).unwrap();

        // Initial state
        let initial_id = sanitize_smv_id(&matrix.initial_mode);
        writeln!(out, "  INIT").unwrap();
        writeln!(out, "    state = {initial_id}").unwrap();
        writeln!(out).unwrap();

        // Transitions
        writeln!(out, "  TRANS").unwrap();
        writeln!(out, "    case").unwrap();

        // Group transitions by source mode
        let mut trans_by_source: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for mt in &transitions {
            let src = sanitize_smv_id(mt.source.as_str());
            let dst = sanitize_smv_id(mt.destination.as_str());
            trans_by_source.entry(src).or_default().insert(dst);
        }

        for (src, dsts) in &trans_by_source {
            let dst_list: Vec<&String> = dsts.iter().collect();
            if dst_list.len() == 1 {
                writeln!(out, "      state = {src} : next(state) = {};", dst_list[0]).unwrap();
            } else {
                let dst_str: Vec<&str> = dst_list.iter().map(|s| s.as_str()).collect();
                writeln!(
                    out,
                    "      state = {src} : next(state) in {{{}}};",
                    dst_str.join(", ")
                )
                .unwrap();
            }
        }

        // Default: stay in current state (no transition available)
        writeln!(out, "      TRUE : next(state) = state;").unwrap();
        writeln!(out, "    esac").unwrap();
        writeln!(out).unwrap();

        // Reachability specification
        for mode_name in &matrix.modes {
            let mode_id = sanitize_smv_id(mode_name);
            if mode_id != initial_id {
                if matrix.unreachable.iter().any(|u| u == mode_name) {
                    writeln!(
                        out,
                        "  -- UNREACHABLE: state = {mode_id} can never be reached"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        out,
                        "  SPEC EF state = {mode_id}  -- reachable from {initial_id}"
                    )
                    .unwrap();
                }
            }
        }

        // Dead transition comments
        for dt in &matrix.dead_transitions {
            writeln!(
                out,
                "  -- DEAD TRANSITION: {} ({} -> {}) trigger '{}' never connected",
                dt.name, dt.source, dt.destination, dt.trigger
            )
            .unwrap();
        }

        writeln!(out).unwrap();
    }

    // Main module composing all modal components
    writeln!(out, "MODULE main").unwrap();
    writeln!(out, "  VAR").unwrap();
    for name in &module_names {
        writeln!(out, "    {name} : {name}();").unwrap();
    }
    writeln!(out).unwrap();

    // Global reachability specs
    for (matrix, name) in matrices.iter().zip(module_names.iter()) {
        for mode_name in &matrix.modes {
            let mode_id = sanitize_smv_id(mode_name);
            if !matrix.unreachable.iter().any(|u| u == mode_name)
                && sanitize_smv_id(&matrix.initial_mode) != mode_id
            {
                writeln!(
                    out,
                    "  SPEC EF {name}.state = {mode_id}  -- {name} can reach {mode_id}"
                )
                .unwrap();
            }
        }
    }

    out
}

// ── DOT Export ──────────────────────────────────────────────────────

/// Generate a DOT (Graphviz) graph for all modal components in the instance.
///
/// Each modal component becomes a `subgraph cluster_*` with:
/// - Nodes for each mode (initial mode gets `peripheries=2` for a double circle)
/// - Edges for each transition, labelled with trigger port names
/// - Unreachable modes rendered in red (`color=red, fontcolor=red`)
///
/// The output is suitable for rendering with `dot -Tsvg` or `dot -Tpng`.
pub fn export_dot(instance: &SystemInstance) -> String {
    let matrices = compute_reachability_matrices(instance);
    if matrices.is_empty() {
        return String::from("// No modal components found.\n");
    }

    let mut out = String::new();
    writeln!(out, "digraph modes {{").unwrap();
    writeln!(out, "  rankdir=LR;").unwrap();
    writeln!(
        out,
        "  // DOT graph generated by spar mode_reachability analysis"
    )
    .unwrap();
    writeln!(out, "  // AS5506 §12 mode state machines").unwrap();
    writeln!(out).unwrap();

    for (ci, matrix) in matrices.iter().enumerate() {
        let cluster_label = matrix.component_path.join("/");
        let prefix = sanitize_dot_id(&matrix.component_path.join("_"));

        writeln!(out, "  subgraph cluster_{ci} {{").unwrap();
        writeln!(out, "    label=\"{cluster_label}\";").unwrap();
        writeln!(out, "    style=rounded;").unwrap();
        writeln!(out).unwrap();

        let unreachable_set: BTreeSet<&str> =
            matrix.unreachable.iter().map(|s| s.as_str()).collect();

        // Emit mode nodes
        for mode_name in &matrix.modes {
            let node_id = format!("{prefix}__{}", sanitize_dot_id(mode_name));
            let is_initial = *mode_name == matrix.initial_mode;
            let is_unreachable = unreachable_set.contains(mode_name.as_str());

            let mut attrs = vec![format!("label=\"{mode_name}\"")];
            if is_initial {
                attrs.push("peripheries=2".to_string());
            }
            if is_unreachable {
                attrs.push("color=red".to_string());
                attrs.push("fontcolor=red".to_string());
            }
            writeln!(out, "    {node_id} [{attrs}];", attrs = attrs.join(", ")).unwrap();
        }

        writeln!(out).unwrap();

        // Emit transition edges
        let transitions = instance.mode_transitions_for(matrix.component_idx);
        for mt in &transitions {
            let src_id = format!("{prefix}__{}", sanitize_dot_id(mt.source.as_str()));
            let dst_id = format!("{prefix}__{}", sanitize_dot_id(mt.destination.as_str()));
            let label = if mt.triggers.is_empty() {
                String::new()
            } else {
                mt.triggers
                    .iter()
                    .map(|t| t.as_str().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if label.is_empty() {
                writeln!(out, "    {src_id} -> {dst_id};").unwrap();
            } else {
                writeln!(out, "    {src_id} -> {dst_id} [label=\"{label}\"];").unwrap();
            }
        }

        writeln!(out, "  }}").unwrap();
        writeln!(out).unwrap();
    }

    writeln!(out, "}}").unwrap();
    out
}

/// Sanitize a name for use as a DOT identifier.
/// DOT identifiers: [a-zA-Z_][a-zA-Z0-9_]*
fn sanitize_dot_id(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if i == 0 {
            if ch.is_ascii_alphabetic() || ch == '_' {
                result.push(ch);
            } else {
                result.push('_');
                if ch.is_ascii_alphanumeric() {
                    result.push(ch);
                }
            }
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        result.push_str("_unnamed");
    }
    result
}

/// Sanitize a name for use as a NuSMV identifier.
/// NuSMV identifiers: [A-Za-z_][A-Za-z0-9_.$#-]*
fn sanitize_smv_id(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if i == 0 {
            if ch.is_ascii_alphabetic() || ch == '_' {
                result.push(ch);
            } else {
                result.push('_');
                if ch.is_ascii_alphanumeric() {
                    result.push(ch);
                }
            }
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '$' || ch == '#' {
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        result.push_str("_unnamed");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::Name;

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        mode_instances: Arena<ModeInstance>,
        mode_transition_instances: Arena<ModeTransitionInstance>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                mode_instances: Arena::default(),
                mode_transition_instances: Arena::default(),
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
            dir: Direction,
            owner: ComponentInstanceIdx,
        ) -> FeatureInstanceIdx {
            let idx = self.features.alloc(FeatureInstance {
                name: Name::new(name),
                kind,
                direction: Some(dir),
                owner,
                classifier: None,
                access_kind: None,
                array_index: None,
            });
            self.components[owner].features.push(idx);
            idx
        }

        fn add_mode(&mut self, name: &str, is_initial: bool, owner: ComponentInstanceIdx) {
            let idx = self.mode_instances.alloc(ModeInstance {
                name: Name::new(name),
                is_initial,
                owner,
            });
            self.components[owner].modes.push(idx);
        }

        fn add_mode_transition(
            &mut self,
            name: Option<&str>,
            source: &str,
            destination: &str,
            triggers: Vec<&str>,
            owner: ComponentInstanceIdx,
        ) {
            let idx = self
                .mode_transition_instances
                .alloc(ModeTransitionInstance {
                    name: name.map(Name::new),
                    source: Name::new(source),
                    destination: Name::new(destination),
                    triggers: triggers.into_iter().map(Name::new).collect(),
                    owner,
                });
            self.components[owner].mode_transitions.push(idx);
        }

        fn add_connection(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: Option<&str>,
            src_feat: &str,
            dst_sub: Option<&str>,
            dst_feat: &str,
        ) -> ConnectionInstanceIdx {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: Some(ConnectionEnd {
                    subcomponent: src_sub.map(Name::new),
                    feature: Name::new(src_feat),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: dst_sub.map(Name::new),
                    feature: Name::new(dst_feat),
                }),
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
                mode_instances: self.mode_instances,
                mode_transition_instances: self.mode_transition_instances,
                diagnostics: Vec::new(),
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    // ── Reachability matrix tests ────────────────────────────────────

    #[test]
    fn linear_chain_reachability() {
        // idle -> active -> shutdown: idle can reach all, active can reach shutdown
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("shutdown", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.add_mode_transition(Some("t2"), "active", "shutdown", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);

        let m = &matrices[0];
        assert_eq!(m.modes, vec!["idle", "active", "shutdown"]);
        assert_eq!(m.initial_mode, "idle");
        assert!(m.unreachable.is_empty(), "all modes reachable from idle");

        // idle (0) can reach active (1) and shutdown (2)
        assert!(m.matrix[0][1], "idle -> active");
        assert!(m.matrix[0][2], "idle -> shutdown (transitive)");
        // active (1) can reach shutdown (2) but not idle (0)
        assert!(m.matrix[1][2], "active -> shutdown");
        assert!(!m.matrix[1][0], "active cannot reach idle");
        // shutdown (2) cannot reach anything
        assert!(!m.matrix[2][0], "shutdown cannot reach idle");
        assert!(!m.matrix[2][1], "shutdown cannot reach active");
    }

    #[test]
    fn cyclic_reachability() {
        // idle <-> active -> error -> idle: fully connected cycle
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("error", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.add_mode_transition(Some("t2"), "active", "idle", vec![], ctrl);
        b.add_mode_transition(Some("t3"), "active", "error", vec![], ctrl);
        b.add_mode_transition(Some("t4"), "error", "idle", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);

        let m = &matrices[0];
        assert!(m.unreachable.is_empty());
        // Every mode can reach every other mode
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    assert!(m.matrix[i][j], "mode {} should reach mode {}", i, j);
                }
            }
        }
    }

    #[test]
    fn unreachable_mode_detected() {
        // idle -> active, but "orphan" has no incoming transitions
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("orphan", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);

        let m = &matrices[0];
        assert_eq!(m.unreachable, vec!["orphan"]);
    }

    #[test]
    fn multi_modal_components() {
        // Two child components each with their own modes
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let nav = b.add_component("nav", ComponentCategory::System, Some(root));
        let comm = b.add_component("comm", ComponentCategory::System, Some(root));

        b.add_mode("off", true, nav);
        b.add_mode("on", false, nav);
        b.add_mode_transition(Some("power_on"), "off", "on", vec![], nav);

        b.add_mode("standby", true, comm);
        b.add_mode("active", false, comm);
        b.add_mode("emergency", false, comm);
        b.add_mode_transition(Some("activate"), "standby", "active", vec![], comm);
        b.add_mode_transition(Some("escalate"), "active", "emergency", vec![], comm);
        b.add_mode_transition(Some("reset"), "emergency", "standby", vec![], comm);

        b.set_children(root, vec![nav, comm]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 2);

        // nav: off -> on, both reachable from off
        assert!(matrices[0].unreachable.is_empty());
        // comm: standby -> active -> emergency -> standby, fully cyclic
        assert!(matrices[1].unreachable.is_empty());
    }

    // ── Dead transition tests ────────────────────────────────────────

    #[test]
    fn dead_transition_trigger_not_connected() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));

        // ctrl has an event port "start_cmd" but no connection targets it
        b.add_feature("start_cmd", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["start_cmd"], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);
        assert_eq!(matrices[0].dead_transitions.len(), 1);
        assert_eq!(matrices[0].dead_transitions[0].trigger, "start_cmd");
    }

    #[test]
    fn connected_trigger_not_dead() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));

        b.add_feature("start_cmd", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_feature(
            "trigger_out",
            FeatureKind::EventPort,
            Direction::Out,
            sensor,
        );
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["start_cmd"], ctrl);

        // Parent connects sensor.trigger_out -> ctrl.start_cmd
        b.add_connection(
            "c1",
            root,
            Some("sensor"),
            "trigger_out",
            Some("ctrl"),
            "start_cmd",
        );

        b.set_children(root, vec![ctrl, sensor]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);
        assert!(
            matrices[0].dead_transitions.is_empty(),
            "connected trigger should not be dead"
        );
    }

    #[test]
    fn no_triggers_no_dead_transitions() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        // Transition without triggers (spontaneous)
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);
        assert!(matrices[0].dead_transitions.is_empty());
    }

    // ── Analysis trait integration tests ─────────────────────────────

    #[test]
    fn analysis_reports_unreachable_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("orphan", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let diags = ModeReachabilityAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("orphan"))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("not reachable"));
    }

    #[test]
    fn analysis_reports_dead_transition_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("go", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["go"], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let diags = ModeReachabilityAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("never fire"))
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("go"));
    }

    #[test]
    fn analysis_reports_info_summary() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let diags = ModeReachabilityAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("2/2 modes reachable"))
            .collect();
        assert_eq!(infos.len(), 1);
    }

    // ── SMV export tests ─────────────────────────────────────────────

    #[test]
    fn smv_export_basic() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.add_mode_transition(Some("t2"), "active", "idle", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let smv = export_smv(&inst);

        assert!(smv.contains("MODULE"), "should have MODULE declaration");
        assert!(smv.contains("state : {idle, active}"), "should list modes");
        assert!(smv.contains("state = idle"), "should set initial state");
        assert!(smv.contains("TRANS"), "should have transitions");
        assert!(smv.contains("MODULE main"), "should have main module");
        assert!(
            smv.contains("SPEC EF"),
            "should have reachability specs: {}",
            smv
        );
    }

    #[test]
    fn smv_export_unreachable_mode_commented() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("orphan", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let smv = export_smv(&inst);

        assert!(
            smv.contains("UNREACHABLE: state = orphan"),
            "should comment unreachable mode: {}",
            smv
        );
    }

    #[test]
    fn smv_export_no_modal_components() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let inst = b.build(root);

        let smv = export_smv(&inst);
        assert!(smv.contains("No modal components"), "should say no modal");
    }

    #[test]
    fn smv_export_multi_component() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let nav = b.add_component("nav", ComponentCategory::System, Some(root));
        let comm = b.add_component("comm", ComponentCategory::System, Some(root));

        b.add_mode("off", true, nav);
        b.add_mode("on", false, nav);
        b.add_mode_transition(Some("t1"), "off", "on", vec![], nav);

        b.add_mode("standby", true, comm);
        b.add_mode("active", false, comm);
        b.add_mode_transition(Some("t1"), "standby", "active", vec![], comm);

        b.set_children(root, vec![nav, comm]);

        let inst = b.build(root);
        let smv = export_smv(&inst);

        // Should have two MODULE declarations plus main
        let module_count = smv.matches("MODULE ").count();
        assert_eq!(module_count, 3, "2 component modules + main: {}", smv);
    }

    // ── sanitize_smv_id tests ────────────────────────────────────────

    #[test]
    fn sanitize_smv_id_basic() {
        assert_eq!(sanitize_smv_id("idle"), "idle");
        assert_eq!(sanitize_smv_id("active_mode"), "active_mode");
        assert_eq!(sanitize_smv_id("mode.sub"), "mode.sub");
    }

    #[test]
    fn sanitize_smv_id_special_chars() {
        assert_eq!(sanitize_smv_id("root/ctrl"), "root_ctrl");
        assert_eq!(sanitize_smv_id("123start"), "_123start");
        assert_eq!(sanitize_smv_id(""), "_unnamed");
    }

    // ── DOT export tests ──────────────────────────────────────────────

    #[test]
    fn dot_export_basic() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.add_mode_transition(Some("t2"), "active", "idle", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let dot = export_dot(&inst);

        assert!(dot.starts_with("digraph modes {"), "should be a digraph");
        assert!(
            dot.contains("subgraph cluster_"),
            "should have subgraph cluster"
        );
        assert!(
            dot.contains("label=\"root/ctrl\""),
            "should label with component path: {}",
            dot
        );
        // Initial mode should have double circle
        assert!(
            dot.contains("peripheries=2"),
            "initial mode should have double circle: {}",
            dot
        );
        // Both modes should appear as nodes
        assert!(
            dot.contains("label=\"idle\""),
            "should have idle node: {}",
            dot
        );
        assert!(
            dot.contains("label=\"active\""),
            "should have active node: {}",
            dot
        );
        // Edges should exist
        assert!(dot.contains("->"), "should have transition edges: {}", dot);
    }

    #[test]
    fn dot_export_unreachable_mode_red() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("orphan", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let dot = export_dot(&inst);

        // orphan should be in red
        assert!(
            dot.contains("color=red"),
            "unreachable mode should be red: {}",
            dot
        );
        assert!(
            dot.contains("fontcolor=red"),
            "unreachable mode should have red font: {}",
            dot
        );
    }

    #[test]
    fn dot_export_trigger_labels() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("go", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["go"], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let dot = export_dot(&inst);

        // Edge should have trigger label
        assert!(
            dot.contains("label=\"go\""),
            "transition should show trigger label: {}",
            dot
        );
    }

    #[test]
    fn dot_export_multi_component() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let nav = b.add_component("nav", ComponentCategory::System, Some(root));
        let comm = b.add_component("comm", ComponentCategory::System, Some(root));

        b.add_mode("off", true, nav);
        b.add_mode("on", false, nav);
        b.add_mode_transition(Some("t1"), "off", "on", vec![], nav);

        b.add_mode("standby", true, comm);
        b.add_mode("active", false, comm);
        b.add_mode_transition(Some("t1"), "standby", "active", vec![], comm);

        b.set_children(root, vec![nav, comm]);

        let inst = b.build(root);
        let dot = export_dot(&inst);

        // Should have two subgraph clusters
        let cluster_count = dot.matches("subgraph cluster_").count();
        assert_eq!(
            cluster_count, 2,
            "should have 2 clusters for 2 modal components: {}",
            dot
        );

        assert!(
            dot.contains("label=\"root/nav\""),
            "should label nav cluster: {}",
            dot
        );
        assert!(
            dot.contains("label=\"root/comm\""),
            "should label comm cluster: {}",
            dot
        );
    }

    #[test]
    fn dot_export_no_modal_components() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let inst = b.build(root);

        let dot = export_dot(&inst);
        assert!(
            dot.contains("No modal components"),
            "should say no modal: {}",
            dot
        );
    }

    #[test]
    fn dot_export_multiple_triggers() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("go", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_feature("start", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(
            Some("activate"),
            "idle",
            "active",
            vec!["go", "start"],
            ctrl,
        );
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let dot = export_dot(&inst);

        // Edge should have both trigger labels
        assert!(
            dot.contains("label=\"go, start\""),
            "transition should show both trigger labels: {}",
            dot
        );
    }

    // ── Additional mutation-killing tests ────────────────────────────

    #[test]
    fn single_mode_component_skipped() {
        // Component with only 1 mode and no transitions should not produce a matrix
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("only", true, ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert!(
            matrices.is_empty(),
            "single mode = no matrix: {:?}",
            matrices
        );
    }

    #[test]
    fn two_modes_no_transitions_skipped() {
        // 2 modes but no transitions → skipped
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert!(matrices.is_empty(), "no transitions = no matrix");
    }

    #[test]
    fn no_initial_mode_skipped() {
        // 2 modes with transitions but no initial → skipped
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", false, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert!(matrices.is_empty(), "no initial mode = no matrix");
    }

    #[test]
    fn self_reachability_in_matrix() {
        // Each mode should be reachable from itself (matrix[i][i] = true)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);

        let m = &matrices[0];
        for i in 0..m.modes.len() {
            assert!(
                m.matrix[i][i],
                "mode {} should be reachable from itself",
                m.modes[i]
            );
        }
    }

    #[test]
    fn reachability_summary_count() {
        // 3 modes, 1 unreachable: summary should say 2/3
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode("orphan", false, ctrl);
        b.add_mode_transition(Some("t1"), "idle", "active", vec![], ctrl);
        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let diags = ModeReachabilityAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("2/3"))
            .collect();
        assert_eq!(infos.len(), 1, "should report 2/3 reachable: {:?}", diags);
    }

    #[test]
    fn dead_transition_with_connection_on_self() {
        // Connection on the component itself (not from parent) → trigger is connected
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let ctrl = b.add_component("ctrl", ComponentCategory::System, Some(root));
        b.add_feature("go", FeatureKind::EventPort, Direction::In, ctrl);
        b.add_mode("idle", true, ctrl);
        b.add_mode("active", false, ctrl);
        b.add_mode_transition(Some("activate"), "idle", "active", vec!["go"], ctrl);

        // Connection on ctrl itself with dst feature "go" and no subcomponent
        let conn_idx = b.connections.alloc(ConnectionInstance {
            name: Name::new("c_internal"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: ctrl,
            src: Some(ConnectionEnd {
                subcomponent: None,
                feature: Name::new("something"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: None,
                feature: Name::new("go"),
            }),
            in_modes: Vec::new(),
        });
        b.components[ctrl].connections.push(conn_idx);

        b.set_children(root, vec![ctrl]);

        let inst = b.build(root);
        let matrices = compute_reachability_matrices(&inst);
        assert_eq!(matrices.len(), 1);
        assert!(
            matrices[0].dead_transitions.is_empty(),
            "connected via self connection: should NOT be dead"
        );
    }

    // ── sanitize_dot_id tests ─────────────────────────────────────────

    #[test]
    fn sanitize_dot_id_basic() {
        assert_eq!(sanitize_dot_id("idle"), "idle");
        assert_eq!(sanitize_dot_id("active_mode"), "active_mode");
    }

    #[test]
    fn sanitize_dot_id_special_chars() {
        assert_eq!(sanitize_dot_id("root/ctrl"), "root_ctrl");
        assert_eq!(sanitize_dot_id("123start"), "_123start");
        assert_eq!(sanitize_dot_id(""), "_unnamed");
        // DOT ids don't allow dots (unlike SMV)
        assert_eq!(sanitize_dot_id("a.b"), "a_b");
    }
}
