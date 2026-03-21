//! Deployment solver for AADL models.
//!
//! Provides algorithms for allocating threads to processors
//! based on utilization constraints extracted from AADL models.

pub mod allocate;
pub mod constraints;

#[cfg(test)]
mod tests;
