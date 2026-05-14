//! EMV2 error-propagation traversal across the AADL connection graph.
//!
//! This module computes which downstream components inherit each declared
//! error by walking outgoing connections from components that have `out`
//! propagations to components that have matching `in` propagations.
//!
//! # Background
//!
//! EMV2 error propagations (AADL v2, AS5506 §12.3) declare, for each
//! component, which error types flow in or out via its ports/features.
//! When a component has an `out propagation` for error type T on a port P,
//! any component connected (via an AADL connection) to that port inherits T
//! if it declares a matching `in propagation`.
//!
//! # Design
//!
//! The AADL instance model (`SystemInstance`) does not yet integrate EMV2
//! annex data — those live in separate `.emv2` files.  This module therefore
//! accepts an [`Emv2Overlay`] that callers supply, which carries the
//! lightweight propagation/flow annotations for each component instance.
//!
//! The traversal uses `SystemInstance::semantic_connections` to resolve
//! inter-component links.  Cycle detection tracks `(component, error_type)`
//! pairs so that A → B → A terminates cleanly.
//!
//! # Usage
//!
//! ```ignore
//! let overlay = Emv2Overlay::builder()
//!     .add_out_propagation(sensor_idx, "dataout", "BadValue")
//!     .add_in_propagation(controller_idx, "datain", "BadValue")
//!     .build();
//! let report = compute_error_propagation(&instance, &overlay);
//! ```

use rustc_hash::{FxHashMap, FxHashSet};
use spar_hir_def::instance::{ComponentInstanceIdx, ConnectionInstanceIdx, SystemInstance};

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

// ── Public types ─────────────────────────────────────────────────────

/// An error-propagation direction on a component port/feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropagationDirection {
    In,
    Out,
}

/// A single EMV2 propagation declaration on a component port.
///
/// Corresponds to an `in propagation` or `out propagation` entry in the
/// EMV2 `error propagations` block (AS5506 §12.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPropagation {
    /// The component this propagation belongs to.
    pub component: ComponentInstanceIdx,
    /// Port/feature name (e.g., `"dataout"`, `"datain"`).
    pub port: String,
    /// Direction: `In` or `Out`.
    pub direction: PropagationDirection,
    /// Error type names declared on this propagation (e.g., `["BadValue"]`).
    pub error_types: Vec<String>,
}

/// A single EMV2 error flow declaration (`path`, `sink`, or `source`).
///
/// Error flows refine how errors enter, pass through, or leave a component.
/// Unlike propagations, flows do not by themselves cause cross-component
/// propagation — they describe local behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorFlow {
    /// The component this flow belongs to.
    pub component: ComponentInstanceIdx,
    /// Flow name (e.g., `"f0"`).
    pub name: String,
    /// Flow kind: `"source"`, `"sink"`, or `"path"`.
    pub kind: String,
    /// Error type names associated with this flow.
    pub error_types: Vec<String>,
}

/// Severity level for EMV2 diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmvDiagnosticSeverity {
    Info,
    Warning,
}

/// A diagnostic produced during propagation traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmvDiagnostic {
    pub severity: EmvDiagnosticSeverity,
    pub message: String,
}

/// An end-to-end error propagation chain from an origin component to its
/// transitive downstream recipients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPropagationChain {
    /// Component where the error originates (has an `out propagation`).
    pub origin: ComponentInstanceIdx,
    /// Error type name (e.g., `"BadValue"`).
    pub error_type: String,
    /// Downstream components in propagation order (closest first).
    pub downstream: Vec<ComponentInstanceIdx>,
    /// Connection instances traversed, in the same order as `downstream`.
    pub via_connections: Vec<ConnectionInstanceIdx>,
}

/// Full result of the error-propagation analysis pass.
#[derive(Debug, Clone)]
pub struct ErrorPropagationReport {
    /// End-to-end propagation chains discovered.
    pub chains: Vec<ErrorPropagationChain>,
    /// Local error flows captured per component (path/sink/source — not
    /// followed across connections).
    pub local_flows: Vec<ErrorFlow>,
    /// Informational and warning diagnostics.
    pub diagnostics: Vec<EmvDiagnostic>,
}

// ── Overlay ───────────────────────────────────────────────────────────

/// EMV2 annotation overlay: carries propagation and flow declarations
/// keyed by component instance index.
///
/// Because EMV2 annex data is not yet integrated into `SystemInstance`,
/// callers supply this overlay when constructing analysis inputs.
/// Production code will eventually populate this from the parsed `.emv2`
/// annex subclause; in the interim, tests build it programmatically.
#[derive(Debug, Clone, Default)]
pub struct Emv2Overlay {
    /// Out-propagations per component: component → list of declarations.
    pub out_propagations: FxHashMap<ComponentInstanceIdx, Vec<ErrorPropagation>>,
    /// In-propagations per component.
    pub in_propagations: FxHashMap<ComponentInstanceIdx, Vec<ErrorPropagation>>,
    /// Error flows per component.
    pub flows: FxHashMap<ComponentInstanceIdx, Vec<ErrorFlow>>,
}

impl Emv2Overlay {
    /// Create an empty overlay.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an `out propagation` on a component port.
    pub fn add_out_propagation(
        &mut self,
        component: ComponentInstanceIdx,
        port: &str,
        error_types: &[&str],
    ) {
        self.out_propagations
            .entry(component)
            .or_default()
            .push(ErrorPropagation {
                component,
                port: port.to_string(),
                direction: PropagationDirection::Out,
                error_types: error_types.iter().map(|s| s.to_string()).collect(),
            });
    }

    /// Register an `in propagation` on a component port.
    pub fn add_in_propagation(
        &mut self,
        component: ComponentInstanceIdx,
        port: &str,
        error_types: &[&str],
    ) {
        self.in_propagations
            .entry(component)
            .or_default()
            .push(ErrorPropagation {
                component,
                port: port.to_string(),
                direction: PropagationDirection::In,
                error_types: error_types.iter().map(|s| s.to_string()).collect(),
            });
    }

    /// Register an error flow on a component.
    pub fn add_flow(
        &mut self,
        component: ComponentInstanceIdx,
        name: &str,
        kind: &str,
        error_types: &[&str],
    ) {
        self.flows.entry(component).or_default().push(ErrorFlow {
            component,
            name: name.to_string(),
            kind: kind.to_string(),
            error_types: error_types.iter().map(|s| s.to_string()).collect(),
        });
    }

    /// Check whether a component has any `in propagation` that matches
    /// `error_type` (case-insensitive, per AADL EMV2 v2 §4.3).
    pub fn has_in_propagation_for(
        &self,
        component: ComponentInstanceIdx,
        error_type: &str,
    ) -> bool {
        let et_lower = error_type.to_ascii_lowercase();
        self.in_propagations
            .get(&component)
            .map(|props| {
                props.iter().any(|p| {
                    p.error_types
                        .iter()
                        .any(|t| t.to_ascii_lowercase() == et_lower)
                })
            })
            .unwrap_or(false)
    }

    /// Return all out-propagation error types for a component.
    pub fn out_error_types(&self, component: ComponentInstanceIdx) -> Vec<String> {
        self.out_propagations
            .get(&component)
            .map(|props| {
                let mut types: Vec<String> =
                    props.iter().flat_map(|p| p.error_types.clone()).collect();
                types.sort();
                types.dedup();
                types
            })
            .unwrap_or_default()
    }
}

// ── Core algorithm ────────────────────────────────────────────────────

/// Compute error-propagation chains across the AADL connection graph.
///
/// For each component in the instance model that declares an `out propagation`
/// in the supplied `overlay`, the algorithm:
///
/// 1. Collects all semantic connections from that component to downstream peers.
/// 2. For each downstream peer that declares a matching `in propagation`, records
///    the hop and recurses (following transitive propagations).
/// 3. Terminates when a `(component, error_type)` pair is re-encountered
///    (cycle detection), or when no more matching downstream components exist.
///
/// Error flows (`path`, `sink`, `source`) are collected per component and
/// stored in [`ErrorPropagationReport::local_flows`] but do **not** trigger
/// cross-component propagation by themselves.
pub fn compute_error_propagation(
    instance: &SystemInstance,
    overlay: &Emv2Overlay,
) -> ErrorPropagationReport {
    let mut chains: Vec<ErrorPropagationChain> = Vec::new();
    let mut diagnostics: Vec<EmvDiagnostic> = Vec::new();

    // Collect all local flows (no traversal needed — just read from overlay).
    let local_flows: Vec<ErrorFlow> = overlay.flows.values().flat_map(|v| v.clone()).collect();

    // Build reverse map: source component → (dest component, connection_idx).
    // We use semantic_connections which already resolves full end-to-end paths.
    let mut outgoing: FxHashMap<
        ComponentInstanceIdx,
        Vec<(ComponentInstanceIdx, ConnectionInstanceIdx)>,
    > = FxHashMap::default();

    for sc in &instance.semantic_connections {
        let (src_comp, _src_feat) = &sc.ultimate_source;
        let (dst_comp, _dst_feat) = &sc.ultimate_destination;
        // Pick the last connection in the path (or the only one) as the
        // representative connection index for diagnostic tracing.
        if let Some(&conn_idx) = sc.connection_path.last() {
            outgoing
                .entry(*src_comp)
                .or_default()
                .push((*dst_comp, conn_idx));
        }
    }

    // For each component with out-propagations, start a traversal.
    let components_with_out_props: Vec<ComponentInstanceIdx> =
        overlay.out_propagations.keys().copied().collect();

    for origin in components_with_out_props {
        let error_types = overlay.out_error_types(origin);
        for error_type in error_types {
            // Visited set for cycle detection: (component, error_type).
            let mut visited: FxHashSet<(ComponentInstanceIdx, String)> = FxHashSet::default();
            visited.insert((origin, error_type.clone()));

            let mut downstream: Vec<ComponentInstanceIdx> = Vec::new();
            let mut via_connections: Vec<ConnectionInstanceIdx> = Vec::new();

            traverse(
                origin,
                &error_type,
                &outgoing,
                overlay,
                &mut visited,
                &mut downstream,
                &mut via_connections,
            );

            if !downstream.is_empty() {
                chains.push(ErrorPropagationChain {
                    origin,
                    error_type: error_type.clone(),
                    downstream: downstream.clone(),
                    via_connections: via_connections.clone(),
                });

                diagnostics.push(EmvDiagnostic {
                    severity: EmvDiagnosticSeverity::Info,
                    message: format!(
                        "error propagation chain: {} components downstream of '{}' for error type '{}'",
                        downstream.len(),
                        instance.component(origin).name.as_str(),
                        error_type
                    ),
                });
            }
        }
    }

    ErrorPropagationReport {
        chains,
        local_flows,
        diagnostics,
    }
}

/// Recursively traverse outgoing connections from `current` for `error_type`,
/// appending to `downstream` and `via_connections`.
///
/// Cycle detection: stops when `(current, error_type)` has already been
/// visited (inserted into `visited` by the caller before this call).
fn traverse(
    current: ComponentInstanceIdx,
    error_type: &str,
    outgoing: &FxHashMap<ComponentInstanceIdx, Vec<(ComponentInstanceIdx, ConnectionInstanceIdx)>>,
    overlay: &Emv2Overlay,
    visited: &mut FxHashSet<(ComponentInstanceIdx, String)>,
    downstream: &mut Vec<ComponentInstanceIdx>,
    via_connections: &mut Vec<ConnectionInstanceIdx>,
) {
    let neighbors = match outgoing.get(&current) {
        Some(v) => v.clone(),
        None => return,
    };

    for (neighbor, conn_idx) in neighbors {
        let key = (neighbor, error_type.to_string());
        if visited.contains(&key) {
            // Cycle detected — stop recursion here.
            continue;
        }

        if overlay.has_in_propagation_for(neighbor, error_type) {
            visited.insert(key);
            downstream.push(neighbor);
            via_connections.push(conn_idx);

            // Recurse: the neighbor may also have out-propagations for this type.
            traverse(
                neighbor,
                error_type,
                outgoing,
                overlay,
                visited,
                downstream,
                via_connections,
            );
        }
    }
}

// ── Analysis pass ─────────────────────────────────────────────────────

/// EMV2 error-propagation analysis pass.
///
/// When registered via `AnalysisRunner::register_all`, this pass runs
/// `compute_error_propagation` with an **empty** overlay (no user-supplied
/// annotations).  Callers that have loaded `.emv2` data should construct
/// an [`Emv2Overlay`] and call [`compute_error_propagation`] directly.
///
/// The pass emits one `Info` diagnostic summarising the number of chains
/// found, regardless of whether the overlay is empty.
pub struct Emv2PropagationAnalysis;

impl Analysis for Emv2PropagationAnalysis {
    fn name(&self) -> &str {
        "emv2_propagation"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let overlay = Emv2Overlay::new();
        let report = compute_error_propagation(instance, &overlay);
        let root_path = component_path(instance, instance.root);

        let mut diags = Vec::new();

        // Surface per-chain info diagnostics.
        for emv_diag in &report.diagnostics {
            diags.push(AnalysisDiagnostic {
                severity: match emv_diag.severity {
                    EmvDiagnosticSeverity::Info => Severity::Info,
                    EmvDiagnosticSeverity::Warning => Severity::Warning,
                },
                message: emv_diag.message.clone(),
                path: root_path.clone(),
                analysis: self.name().to_string(),
            });
        }

        // Summary.
        diags.push(AnalysisDiagnostic {
            severity: Severity::Info,
            message: format!(
                "EMV2 propagation: {} chain(s), {} local flow(s) analysed",
                report.chains.len(),
                report.local_flows.len(),
            ),
            path: root_path,
            analysis: self.name().to_string(),
        });

        diags
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind};
    use spar_hir_def::name::Name;
    use spar_hir_def::properties::PropertyMap;

    // ── Test helpers ──────────────────────────────────────────────────

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
        semantic_connections: Vec<SemanticConnection>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
            }
        }

        fn add_component(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
        ) -> ComponentInstanceIdx {
            let idx = self.components.alloc(ComponentInstance {
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
            });
            idx
        }

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        /// Add a semantic connection from `src` to `dst`, returning the
        /// representative `ConnectionInstanceIdx` stored in the path.
        fn add_semantic_connection(
            &mut self,
            src: ComponentInstanceIdx,
            dst: ComponentInstanceIdx,
        ) -> ConnectionInstanceIdx {
            let conn_idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(&format!(
                    "c_{}_{}",
                    src.into_raw().into_u32(),
                    dst.into_raw().into_u32()
                )),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner: src,
                src: None,
                dst: None,
                in_modes: Vec::new(),
            });
            self.semantic_connections.push(SemanticConnection {
                name: Name::new("conn"),
                kind: ConnectionKind::Port,
                ultimate_source: (src, Name::new("out")),
                ultimate_destination: (dst, Name::new("in")),
                connection_path: vec![conn_idx],
            });
            conn_idx
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
                semantic_connections: self.semantic_connections,
                system_operation_modes: Vec::new(),
            }
        }
    }

    // ── (a) Single-hop propagation: A → B ─────────────────────────────

    /// Test: component A has `out propagation {BadValue}`, component B has
    /// `in propagation {BadValue}`, and they are connected.  The report must
    /// contain exactly one chain: origin=A, downstream=[B].
    #[test]
    fn emv2_propagation_single_hop() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let bcomp = b.add_component("controller", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![a, bcomp]);
        let conn = b.add_semantic_connection(a, bcomp);

        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        overlay.add_out_propagation(a, "dataout", &["BadValue"]);
        overlay.add_in_propagation(bcomp, "datain", &["BadValue"]);

        let report = compute_error_propagation(&instance, &overlay);

        assert_eq!(
            report.chains.len(),
            1,
            "should find one chain: {:?}",
            report.chains
        );
        let chain = &report.chains[0];
        assert_eq!(chain.origin, a, "origin should be sensor");
        assert_eq!(chain.error_type, "BadValue");
        assert_eq!(
            chain.downstream,
            vec![bcomp],
            "downstream should be [controller]"
        );
        assert_eq!(chain.via_connections, vec![conn]);
    }

    // ── (b) 3-hop chain: A → B → C → D ───────────────────────────────

    /// Test: A→B→C→D chain where each hop has matching in/out propagations.
    /// The report must contain one chain for origin=A with downstream=[B, C, D].
    #[test]
    fn emv2_propagation_three_hop_chain() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Device, Some(root));
        let bcomp = b.add_component("b", ComponentCategory::Process, Some(root));
        let c = b.add_component("c", ComponentCategory::Process, Some(root));
        let d = b.add_component("d", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![a, bcomp, c, d]);

        let conn_ab = b.add_semantic_connection(a, bcomp);
        let conn_bc = b.add_semantic_connection(bcomp, c);
        let conn_cd = b.add_semantic_connection(c, d);

        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        // A: out only
        overlay.add_out_propagation(a, "out", &["ServiceError"]);
        // B: in + out (relay)
        overlay.add_in_propagation(bcomp, "in", &["ServiceError"]);
        overlay.add_out_propagation(bcomp, "out", &["ServiceError"]);
        // C: in + out (relay)
        overlay.add_in_propagation(c, "in", &["ServiceError"]);
        overlay.add_out_propagation(c, "out", &["ServiceError"]);
        // D: in only (terminal)
        overlay.add_in_propagation(d, "in", &["ServiceError"]);

        let report = compute_error_propagation(&instance, &overlay);

        // Chains: one for A (3 hops), one for B (2 hops, B is also an origin),
        // one for C (1 hop).  We care specifically about the chain from A.
        let chain_from_a = report
            .chains
            .iter()
            .find(|ch| ch.origin == a)
            .expect("should find chain from A");

        assert_eq!(
            chain_from_a.downstream.len(),
            3,
            "A→B→C→D: 3 downstream: {:?}",
            chain_from_a.downstream
        );
        assert_eq!(chain_from_a.downstream[0], bcomp, "first hop = B");
        assert_eq!(chain_from_a.downstream[1], c, "second hop = C");
        assert_eq!(chain_from_a.downstream[2], d, "third hop = D");
        assert_eq!(
            chain_from_a.via_connections,
            vec![conn_ab, conn_bc, conn_cd]
        );
    }

    // ── (c) Cycle detection: A → B → A halts cleanly ─────────────────

    /// Test: A and B are mutually connected with matching in/out propagations
    /// for the same error type.  The traversal must terminate without panic
    /// or infinite loop, and each origin's chain must not contain itself.
    #[test]
    fn emv2_propagation_cycle_detection() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Process, Some(root));
        let bcomp = b.add_component("b", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![a, bcomp]);

        // A → B and B → A form a cycle.
        b.add_semantic_connection(a, bcomp);
        b.add_semantic_connection(bcomp, a);

        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        overlay.add_out_propagation(a, "out", &["TimingError"]);
        overlay.add_in_propagation(a, "in", &["TimingError"]);
        overlay.add_out_propagation(bcomp, "out", &["TimingError"]);
        overlay.add_in_propagation(bcomp, "in", &["TimingError"]);

        // Must not hang or panic.
        let report = compute_error_propagation(&instance, &overlay);

        // Each chain's downstream must NOT contain the origin itself.
        for chain in &report.chains {
            assert!(
                !chain.downstream.contains(&chain.origin),
                "cycle: origin {:?} must not appear in its own downstream {:?}",
                chain.origin,
                chain.downstream
            );
        }

        // There should be exactly 2 chains (one per origin), each with 1 downstream.
        assert_eq!(
            report.chains.len(),
            2,
            "A→B and B→A each yield one chain: {:?}",
            report.chains
        );
    }

    // ── (d) Flow extraction: path flows captured, not propagated ──────

    /// Test: component A declares a `path` error flow.  The report must
    /// contain the flow in `local_flows` but must NOT produce a propagation
    /// chain downstream.
    #[test]
    fn emv2_propagation_path_flow_not_propagated() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("filter", ComponentCategory::Process, Some(root));
        let bcomp = b.add_component("actuator", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![a, bcomp]);
        b.add_semantic_connection(a, bcomp);

        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        // A has a path flow but NO out propagation.
        overlay.add_flow(a, "f_path", "path", &["ValueError"]);
        // B has an in propagation (but nobody sends to it).
        overlay.add_in_propagation(bcomp, "datain", &["ValueError"]);

        let report = compute_error_propagation(&instance, &overlay);

        // Flow should appear in local_flows.
        assert_eq!(
            report.local_flows.len(),
            1,
            "path flow should appear in local_flows: {:?}",
            report.local_flows
        );
        assert_eq!(report.local_flows[0].kind, "path");
        assert_eq!(report.local_flows[0].name, "f_path");

        // No chains — a path flow alone does not propagate.
        assert!(
            report.chains.is_empty(),
            "path flow must not produce a propagation chain: {:?}",
            report.chains
        );
    }

    // ── Case-insensitive type matching ────────────────────────────────

    #[test]
    fn emv2_propagation_case_insensitive_match() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Device, Some(root));
        let bcomp = b.add_component("b", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![a, bcomp]);
        b.add_semantic_connection(a, bcomp);
        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        overlay.add_out_propagation(a, "out", &["BadValue"]);
        // B uses different case for the same error type.
        overlay.add_in_propagation(bcomp, "in", &["badvalue"]);

        let report = compute_error_propagation(&instance, &overlay);

        assert_eq!(
            report.chains.len(),
            1,
            "case-insensitive match should yield one chain: {:?}",
            report.chains
        );
    }

    // ── No connection → no chain ──────────────────────────────────────

    #[test]
    fn emv2_propagation_no_connection_no_chain() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("a", ComponentCategory::Device, Some(root));
        let bcomp = b.add_component("b", ComponentCategory::Process, Some(root));
        b.set_children(root, vec![a, bcomp]);
        // No connection between A and B.
        let instance = b.build(root);

        let mut overlay = Emv2Overlay::new();
        overlay.add_out_propagation(a, "out", &["BadValue"]);
        overlay.add_in_propagation(bcomp, "in", &["BadValue"]);

        let report = compute_error_propagation(&instance, &overlay);
        assert!(
            report.chains.is_empty(),
            "no connection → no chain: {:?}",
            report.chains
        );
    }

    // ── Analysis pass emits summary diagnostic ────────────────────────

    #[test]
    fn emv2_propagation_analysis_pass_summary() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let instance = b.build(root);

        let diags = Emv2PropagationAnalysis.analyze(&instance);

        let summary: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("EMV2 propagation"))
            .collect();
        assert_eq!(summary.len(), 1, "should have summary: {:?}", diags);
        assert_eq!(summary[0].severity, Severity::Info);
        assert_eq!(summary[0].analysis, "emv2_propagation");
    }
}
