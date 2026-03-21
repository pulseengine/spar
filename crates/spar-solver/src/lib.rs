//! Deployment constraint extraction and solver for AADL instance models.
//!
//! This crate extracts timing, resource, and binding constraints from AADL
//! property associations on component instances. These constraints feed into
//! deployment solvers that assign threads to processors.

pub mod constraints;

#[cfg(test)]
mod tests;
