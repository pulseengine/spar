//! Network graph types — the typed view of a network topology
//! extracted from an AADL ItemTree, ready for WCTT analysis.
//!
//! These types are the bridge between the AADL `SystemInstance` (in
//! `spar-hir-def`) and the Network Calculus analyses that follow in
//! later Track D commits. They are intentionally lightweight: a node
//! identifies a forwarding device or end station, a link captures the
//! per-hop properties consumed by the WCTT pass, and the [`NetworkGraph`]
//! collects both. No Network Calculus primitives live here — those land
//! in commit 3.

use spar_hir_def::instance::ComponentInstanceIdx;

/// One node in the network graph: either a switch (forwarding device)
/// or an end station (origin/destination of frames).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkNode {
    /// Index of the underlying AADL `ComponentInstance`.
    pub idx: ComponentInstanceIdx,
    /// What role this node plays in the network.
    pub kind: NodeKind,
    /// Display name copied from the underlying `ComponentInstance` for
    /// diagnostics.
    pub name: String,
}

/// Role of a [`NetworkNode`] in the network topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A forwarding bus that carries `Spar_Network::Switch_Type`.
    Switch { switch_type: SwitchType },
    /// A device or processor connected to one or more switches.
    EndStation,
}

/// Forwarding discipline of a switch, sourced from
/// `Spar_Network::Switch_Type`.
///
/// Phase 1 (v0.8.0) covers `Fifo` and `Priority`. `Tsn` is accepted and
/// classified, but Phase 1 analyses treat it as opaque — full
/// TSN-shaped service curves land in Phase 2 alongside the `Spar_TSN`
/// property set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchType {
    Fifo,
    Priority,
    /// Reserved for Phase 2 (TSN-shaped service curves). Phase 1
    /// analysis treats TSN-typed switches as opaque.
    Tsn,
}

impl SwitchType {
    /// Parse the AADL enumeration literal for `Switch_Type`.
    ///
    /// Comparison is case-insensitive, matching AADL spec semantics.
    /// Returns `None` for unrecognised values.
    pub fn from_aadl_enum(s: &str) -> Option<Self> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "fifo" => Some(Self::Fifo),
            "priority" => Some(Self::Priority),
            "tsn" => Some(Self::Tsn),
            _ => None,
        }
    }
}

/// One link (edge) in the network graph: a connection between two
/// [`NetworkNode`]s that traverses a specific switch (bus).
///
/// All numeric fields are optional because the underlying AADL model
/// may omit `Spar_Network::*` annotations; the WCTT pass that consumes
/// the graph is responsible for diagnosing missing values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkLink {
    /// Source node (typically an end station or another switch).
    pub from: ComponentInstanceIdx,
    /// Destination node.
    pub to: ComponentInstanceIdx,
    /// The switch (bus) that carries this link.
    pub bus_idx: ComponentInstanceIdx,
    /// Egress bandwidth in bits per second, from
    /// `Spar_Network::Output_Rate` on the bus.
    pub bandwidth_bps: Option<u64>,
    /// Per-hop store-and-forward latency in picoseconds as
    /// `(BCET, WCET)`, from `Spar_Network::Forwarding_Latency`.
    pub forwarding_latency_ps: Option<(u64, u64)>,
    /// Per-port queue capacity in frames, from
    /// `Spar_Network::Queue_Depth`.
    pub queue_depth: Option<u64>,
}

/// The full network graph extracted from a [`SystemInstance`].
///
/// [`SystemInstance`]: spar_hir_def::instance::SystemInstance
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkGraph {
    pub nodes: Vec<NetworkNode>,
    pub links: Vec<NetworkLink>,
}

impl NetworkGraph {
    /// All nodes in the graph (both switches and end stations).
    pub fn nodes(&self) -> &[NetworkNode] {
        &self.nodes
    }

    /// All links in the graph.
    pub fn links(&self) -> &[NetworkLink] {
        &self.links
    }

    /// Iterate over only the switch nodes.
    pub fn switches(&self) -> impl Iterator<Item = &NetworkNode> {
        self.nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Switch { .. }))
    }

    /// Iterate over only the end-station nodes.
    pub fn end_stations(&self) -> impl Iterator<Item = &NetworkNode> {
        self.nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::EndStation))
    }

    /// Look up a node by its underlying [`ComponentInstanceIdx`].
    pub fn node(&self, idx: ComponentInstanceIdx) -> Option<&NetworkNode> {
        self.nodes.iter().find(|n| n.idx == idx)
    }

    /// Returns the set of nodes reachable from `start` via `NetworkLink`s.
    ///
    /// Links are treated as undirected for reachability — a network
    /// link between two stations on the same switch makes them mutually
    /// reachable. The starting node itself is included in the result.
    /// Returns an empty vec if `start` is not a node in the graph.
    pub fn reachable_from(&self, start: ComponentInstanceIdx) -> Vec<ComponentInstanceIdx> {
        if self.node(start).is_none() {
            return Vec::new();
        }

        let mut visited: Vec<ComponentInstanceIdx> = Vec::new();
        let mut stack: Vec<ComponentInstanceIdx> = vec![start];

        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);

            for link in &self.links {
                let next = if link.from == current {
                    Some(link.to)
                } else if link.to == current {
                    Some(link.from)
                } else {
                    None
                };
                if let Some(next) = next
                    && !visited.contains(&next)
                {
                    stack.push(next);
                }
            }
        }

        visited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_type_from_aadl_enum_recognises_canonical_values() {
        assert_eq!(SwitchType::from_aadl_enum("FIFO"), Some(SwitchType::Fifo));
        assert_eq!(
            SwitchType::from_aadl_enum("Priority"),
            Some(SwitchType::Priority)
        );
        assert_eq!(SwitchType::from_aadl_enum("TSN"), Some(SwitchType::Tsn));
    }

    #[test]
    fn switch_type_from_aadl_enum_is_case_insensitive() {
        assert_eq!(SwitchType::from_aadl_enum("fifo"), Some(SwitchType::Fifo));
        assert_eq!(
            SwitchType::from_aadl_enum("priority"),
            Some(SwitchType::Priority)
        );
        assert_eq!(SwitchType::from_aadl_enum("tsn"), Some(SwitchType::Tsn));
        assert_eq!(SwitchType::from_aadl_enum("TsN"), Some(SwitchType::Tsn));
    }

    #[test]
    fn switch_type_from_aadl_enum_rejects_unknown() {
        assert_eq!(SwitchType::from_aadl_enum("Random"), None);
        assert_eq!(SwitchType::from_aadl_enum(""), None);
    }

    #[test]
    fn empty_graph_has_no_nodes_or_links() {
        let g = NetworkGraph::default();
        assert!(g.nodes().is_empty());
        assert!(g.links().is_empty());
        assert_eq!(g.switches().count(), 0);
        assert_eq!(g.end_stations().count(), 0);
    }
}
