//! Deployment solver for AADL models.
//!
//! Provides topology graph extraction, constraint formulation, and
//! bin-packing allocation following the NDS-layered hierarchical approach.
//!
//! # Layers
//!
//! - Layer 0 (Component): Thread scheduling within a processor — uses spar-analysis RTA
//! - Layer 1 (Cluster): Process-to-processor allocation — FFD/BFD bin packing
//! - Layer 2 (System): Topology graph + bus binding optimization
//! - Layer 3 (Global): Cross-zone E2E + safety decomposition (future)

pub mod topology;
pub mod allocate;
pub mod constraints;

#[cfg(test)]
mod tests;
