//! Pluggable annex parser framework for AADL.
//!
//! AADL annexes (`annex Name {** ... **};`) contain domain-specific
//! sublanguages. This crate provides:
//!
//! - [`AnnexParser`] trait — implement to add a new annex parser
//! - [`AnnexRegistry`] — maps annex names to parsers
//! - [`AnnexParseResult`] — structured output from annex parsing
//!
//! # Architecture
//!
//! The core AADL parser captures annex content as opaque text. After
//! the main parse, annex-specific parsers are invoked to parse the
//! content into structured subtrees.
//!
//! Annex parsers can be:
//! - **Built-in** — compiled into spar (EMV2, Behavior Annex)
//! - **WASM components** — loaded at runtime via the WIT interface
//!   defined in `wit/annex-parser.wit`
//!
//! # Known annexes
//!
//! Only EMV2 and behavior_specification require sublanguage parsers.
//! Annexes A (Code Generation), B (Data Modeling), and ARINC653 are
//! property sets — they use the standard AADL property mechanism.
//!
//! | Name | Standard | Annex Letter | Status |
//! |------|----------|-------------|--------|
//! | `EMV2` | SAE AS-5506/1, AS-5506/3 | C | **Implemented** |
//! | `behavior_specification` | SAE AS-5506/2 | D | **Implemented** |
//! | `agree` | Non-standard (Loonwerks/Collins) | — | Opaque only |
//! | `Resolute` | Non-standard (Loonwerks/Collins) | — | Opaque only |
//! | `BLESS` | Non-standard (Kansas State Univ) | — | Opaque only |
//! | `security` | Experimental (DARPA CASE) | — | Opaque only |

mod registry;
mod types;
pub mod emv2;
pub mod ba;

pub use registry::AnnexRegistry;
pub use types::*;

/// Trait for annex parsers.
///
/// Implement this trait to add parsing support for an AADL annex
/// sublanguage. The parser receives the raw text between `{**` and
/// `**}` and returns a structured parse result.
///
/// # Example
///
/// ```rust,ignore
/// struct Emv2Parser;
///
/// impl AnnexParser for Emv2Parser {
///     fn names(&self) -> &[&str] {
///         &["EMV2"]
///     }
///
///     fn parse(&self, name: &str, source: &str) -> AnnexParseResult {
///         // Parse EMV2 content...
///         AnnexParseResult::default()
///     }
/// }
/// ```
pub trait AnnexParser: Send + Sync {
    /// The annex name(s) this parser handles.
    ///
    /// Names are matched case-insensitively against the annex name
    /// in the AADL source (e.g., `annex EMV2 {** ... **};`).
    fn names(&self) -> &[&str];

    /// Parse annex content.
    ///
    /// `name` is the annex name as written in the source.
    /// `source` is the text between `{**` and `**}`.
    fn parse(&self, name: &str, source: &str) -> AnnexParseResult;
}

/// A parsed AADL file with its annex parse results.
///
/// The main CST stores annex content as opaque `ANNEX_TEXT` tokens.
/// This struct pairs the CST with structured annex parse results,
/// stored as a side table keyed by the CST node's text range.
///
/// This follows rust-analyzer's pattern for macro expansions:
/// the main tree contains a placeholder, and the expansion lives
/// as a separate tree linked via source maps.
pub struct ParsedFile {
    /// The main AADL parse (CST).
    pub parse: spar_syntax::Parse,
    /// Annex parse results, keyed by the annex node's text range.
    pub annexes: Vec<ParsedAnnex>,
}

/// A single parsed annex within a file.
pub struct ParsedAnnex {
    /// Annex name as written in the source (e.g., "EMV2").
    pub name: String,
    /// Text range of the ANNEX_SUBCLAUSE/ANNEX_LIBRARY node in the main CST.
    pub range: rowan::TextRange,
    /// Structured parse result from the annex parser.
    pub result: AnnexParseResult,
}

impl ParsedFile {
    /// Parse an AADL source file and dispatch annex content to registered parsers.
    pub fn parse(source: &str, registry: &AnnexRegistry) -> Self {
        let parse = spar_syntax::parse(source);
        let root = parse.syntax_node();
        let annex_results = registry.parse_all_annexes(&root);
        let annexes = annex_results
            .into_iter()
            .map(|(name, range, result)| ParsedAnnex {
                name,
                range,
                result,
            })
            .collect();
        Self { parse, annexes }
    }

    /// Get all annex results for a given annex name.
    pub fn annexes_by_name(&self, name: &str) -> impl Iterator<Item = &ParsedAnnex> {
        let name_lower = name.to_ascii_lowercase();
        self.annexes
            .iter()
            .filter(move |a| a.name.to_ascii_lowercase() == name_lower)
    }
}

/// Extract annex text from an ANNEX_SUBCLAUSE or ANNEX_LIBRARY node.
///
/// Returns `(annex_name, annex_text)` if the node has the expected structure.
pub fn extract_annex_content(node: &spar_syntax::SyntaxNode) -> Option<(String, String)> {
    use spar_syntax::SyntaxKind;

    let kind = node.kind();
    if kind != SyntaxKind::ANNEX_SUBCLAUSE && kind != SyntaxKind::ANNEX_LIBRARY {
        return None;
    }

    let mut name = None;
    let mut text_parts = Vec::new();
    let mut in_annex_body = false;

    for child in node.children_with_tokens() {
        match child.kind() {
            SyntaxKind::IDENT if name.is_none() => {
                name = Some(child.as_token()?.text().to_string());
            }
            SyntaxKind::ANNEX_OPEN => {
                in_annex_body = true;
            }
            SyntaxKind::ANNEX_CLOSE => {
                in_annex_body = false;
            }
            _ if in_annex_body => {
                text_parts.push(child.as_token()?.text().to_string());
            }
            _ => {}
        }
    }

    let name = name?;
    let text = text_parts.join("");
    Some((name, text))
}
