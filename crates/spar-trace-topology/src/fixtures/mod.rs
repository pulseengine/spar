//! Fixture-generation helpers for the `gen-fixtures` binary.
//!
//! The sub-modules are split so the pure transformer logic (`transform`)
//! is unit-testable without any network namespace involvement, and the
//! RAII netns wrappers (`netns`) are kept isolated behind a thin
//! orchestration layer.
//!
//! # Module layout
//!
//! ```text
//! fixtures/
//!   mod.rs        — re-exports + shared error type
//!   netns.rs      — NetnsGuard (RAII ip-netns-add/del) + Command helpers
//!   transform.rs  — pure tc→Qcc-YANG + pmc-text→gPTP-JSON transformers
//! ```

pub mod netns;
pub mod transform;

use std::{io, path::PathBuf};

/// Top-level error type for the fixture generator.
#[derive(Debug)]
pub enum FixtureError {
    /// I/O error (file write, directory creation, …).
    Io(io::Error),
    /// A subprocess exited with non-zero status or could not be spawned.
    Command { program: String, detail: String },
    /// JSON serialise/deserialise error.
    Json(serde_json::Error),
    /// A required capability (netns, taprio, …) is not available on
    /// this host.
    CapabilityMissing(String),
    /// Input data could not be parsed / transformed.
    Transform(String),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Command { program, detail } => {
                write!(f, "command `{program}` failed: {detail}")
            }
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::CapabilityMissing(msg) => write!(f, "capability missing: {msg}"),
            Self::Transform(msg) => write!(f, "transform error: {msg}"),
        }
    }
}

impl std::error::Error for FixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FixtureError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for FixtureError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Resolved output paths for a single gen-fixtures run.
#[derive(Debug, Clone)]
pub struct OutputPaths {
    /// Directory that receives all fixture files.
    pub dir: PathBuf,
    /// `<dir>/capture.pcapng`
    pub pcapng: PathBuf,
    /// `<dir>/lldp.json`
    pub lldp_json: PathBuf,
    /// `<dir>/qcc-yang.json`
    pub qcc_json: PathBuf,
    /// `<dir>/gptp.json`
    pub gptp_json: PathBuf,
}

impl OutputPaths {
    /// Build from a base directory.
    pub fn new(dir: PathBuf) -> Self {
        let pcapng = dir.join("capture.pcapng");
        let lldp_json = dir.join("lldp.json");
        let qcc_json = dir.join("qcc-yang.json");
        let gptp_json = dir.join("gptp.json");
        Self {
            dir,
            pcapng,
            lldp_json,
            qcc_json,
            gptp_json,
        }
    }
}
