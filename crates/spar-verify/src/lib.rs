//! AADL configuration verification attributes.
//!
//! This crate re-exports the `#[aadl_config]` proc macro from
//! `spar-verify-macros`. Apply it to modules containing AADL property
//! constants so that `spar verify` can check them against the model.
//!
//! # Example
//!
//! ```rust
//! #[spar_verify::aadl_config]
//! pub mod ctrl {
//!     pub const COMPONENT: &str = "SensorFusion::Ctrl.Impl";
//!     pub const CATEGORY: &str = "thread";
//!     pub const PERIOD_PS: u64 = 10_000_000_000;
//! }
//!
//! assert_eq!(ctrl::COMPONENT, "SensorFusion::Ctrl.Impl");
//! ```

pub use spar_verify_macros::aadl_config;

#[cfg(test)]
mod tests;
