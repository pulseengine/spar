//! Salsa incremental computation database for AADL analysis.
//!
//! This crate provides the foundation for incremental AADL analysis:
//!
//! - [`SourceFile`] — salsa input representing a single AADL file's text
//! - [`parse_file`] — tracked function: file text → CST + annex parse results
//! - [`Db`] trait — the salsa database trait that downstream crates depend on
//!
//! # Architecture
//!
//! ```text
//! spar-syntax (CST)  ──┐
//! spar-annex  (annexes) ┤
//!                       ├──▶ spar-base-db (salsa queries)
//!                       │        │
//!                       │        ▼
//!                       │    spar-hir-def (name resolution)
//!                       │        │
//!                       │        ▼
//!                       │    spar-analysis (pluggable analyses)
//! ```
//!
//! Each AADL file is a [`SourceFile`] input. When file text changes,
//! salsa automatically recomputes only the affected downstream queries.

use std::sync::Arc;

/// The salsa database trait for AADL analysis.
///
/// All crates in the spar analysis stack depend on `Db` rather than
/// a concrete database type. This enables:
/// - Testing with lightweight in-memory databases
/// - IDE integration with LSP-aware databases
/// - CLI batch analysis with simple databases
#[salsa::db]
pub trait Db: salsa::Database {}

/// A single AADL source file tracked by the database.
///
/// This is a salsa input: the file text is set externally (by the VFS,
/// CLI, or test harness) and triggers recomputation of downstream queries.
#[salsa::input]
pub struct SourceFile {
    /// File path or name (for diagnostics, not used for identity).
    #[returns(ref)]
    pub name: String,

    /// The full source text of the file.
    #[returns(ref)]
    pub text: String,
}

/// Parsed AADL file: CST + annex parse results.
///
/// This is the primary output of the parse query. It bundles the
/// main AADL parse tree with structured annex results (EMV2, BA, etc.).
///
/// Wraps spar-annex/spar-syntax types in a salsa-friendly form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseResult {
    /// The main AADL green tree (immutable, cheap to clone).
    green: rowan::GreenNode,
    /// Parse errors from the main AADL parser.
    errors: Vec<ParseError>,
    /// Annex parse results (side table).
    annexes: Vec<AnnexResult>,
}

/// A parse error with message and byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub msg: String,
    pub offset: usize,
}

/// A parsed annex within a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnexResult {
    pub name: String,
    pub range: std::ops::Range<u32>,
    pub result: spar_annex::AnnexParseResult,
}

impl ParseResult {
    /// Build a typed syntax node from the green tree.
    pub fn syntax_node(&self) -> spar_syntax::SyntaxNode {
        spar_syntax::SyntaxNode::new_root(self.green.clone())
    }

    /// Return parse errors.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Returns true if parsing produced no errors.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Return annex parse results.
    pub fn annexes(&self) -> &[AnnexResult] {
        &self.annexes
    }
}

// ── Tracked query functions ──────────────────────────────────────

/// Parse an AADL source file into a CST with annex results.
///
/// This is a salsa tracked function: it memoizes results and only
/// recomputes when the input `SourceFile` text changes.
#[salsa::tracked]
pub fn parse_file(db: &dyn Db, file: SourceFile) -> ParseResult {
    let text = file.text(db);
    let parse = spar_syntax::parse(text);

    // Dispatch annex content to registered parsers.
    let registry = default_annex_registry();
    let root = parse.syntax_node();
    let annex_results = registry.parse_all_annexes(&root);

    let errors = parse
        .errors()
        .iter()
        .map(|e| ParseError {
            msg: e.msg.clone(),
            offset: e.offset,
        })
        .collect();

    let annexes = annex_results
        .into_iter()
        .map(|(name, range, result)| AnnexResult {
            name,
            range: u32::from(range.start())..u32::from(range.end()),
            result,
        })
        .collect();

    ParseResult {
        green: root.green().into(),
        errors,
        annexes,
    }
}

/// Build the default annex registry with built-in parsers.
fn default_annex_registry() -> spar_annex::AnnexRegistry {
    let mut registry = spar_annex::AnnexRegistry::new();
    registry.register(Arc::new(spar_annex::emv2::Emv2AnnexParser));
    registry
}

// ── Default database implementation ──────────────────────────────

/// A simple in-memory database for testing and CLI usage.
///
/// Production IDE integration would use a more sophisticated database
/// with VFS integration, but this covers testing and batch analysis.
#[salsa::db]
#[derive(Default)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_file() {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "test.aadl".to_string(),
            "package Test\npublic\nend Test;".to_string(),
        );

        let result = parse_file(&db, file);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_with_emv2_annex() {
        let db = RootDatabase::default();
        let src = r#"
package ErrorLib
public
  annex EMV2 {**
    error types
      ServiceError: type;
    end types;
  **};
end ErrorLib;
"#;
        let file = SourceFile::new(&db, "error_lib.aadl".to_string(), src.to_string());
        let result = parse_file(&db, file);
        assert!(result.ok(), "errors: {:?}", result.errors());
        assert_eq!(result.annexes().len(), 1);
        assert_eq!(result.annexes()[0].name, "EMV2");
    }

    #[test]
    fn incremental_reparse() {
        use salsa::Setter;
        let mut db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "test.aadl".to_string(),
            "package V1\npublic\nend V1;".to_string(),
        );

        // First parse.
        let r1 = parse_file(&db, file);
        assert!(r1.ok());

        // Change file text — salsa should recompute.
        file.set_text(&mut db).to("package V2\npublic\nend V2;".to_string());

        let r2 = parse_file(&db, file);
        assert!(r2.ok());

        // Results should differ (different package name).
        let text1 = r1.syntax_node().text().to_string();
        let text2 = r2.syntax_node().text().to_string();
        assert_ne!(text1, text2);
        assert!(text2.contains("V2"));
    }
}
