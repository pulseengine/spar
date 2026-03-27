//! Deployment solver for AADL models.
//!
//! Provides topology graph extraction, constraint formulation, and
//! bin-packing allocation following the NDS-layered hierarchical approach.

pub mod allocate;
pub mod constraints;
pub mod milp;
pub mod nsga2;
pub mod topology;

#[cfg(test)]
mod tests;
