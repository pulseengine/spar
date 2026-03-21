//! Deployment solver for AADL models.
//!
//! Extracts hardware topology graphs from AADL system instances and
//! provides constraint-based deployment optimization.

pub mod topology;

#[cfg(test)]
mod tests;
