//! Types for annex parse results.
//!
//! These mirror the WIT interface (`spar:annex/types`) so that
//! native Rust parsers and WASM component parsers produce the
//! same output format.

/// A byte range in the annex source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

/// A diagnostic message from annex parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnexDiagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

/// A node in the annex parse tree.
///
/// Annex parse trees use a flat representation (pre-order traversal)
/// for efficient serialization across the WASM boundary. Each node
/// references its parent by index (-1 for root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnexNode {
    /// Annex-specific node kind (e.g., "error-type-def", "state-machine").
    pub kind: String,
    /// Byte span in the annex source text.
    pub span: Span,
    /// Parent node index, or -1 for the root.
    pub parent: i32,
    /// Leaf text content (empty for branch nodes).
    pub text: String,
}

/// Result of parsing an annex.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnexParseResult {
    /// Flat list of nodes in pre-order traversal.
    pub nodes: Vec<AnnexNode>,
    /// Diagnostics from parsing.
    pub diagnostics: Vec<AnnexDiagnostic>,
}

impl AnnexParseResult {
    /// Create an empty result with no nodes or diagnostics.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if parsing produced any errors.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Get all error diagnostics.
    pub fn errors(&self) -> impl Iterator<Item = &AnnexDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }
}
