//! Annex parser registry.
//!
//! Maps annex names to parser implementations. Names are matched
//! case-insensitively, following AADL's case-insensitive identifiers.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{AnnexParseResult, AnnexParser};

/// Registry of annex parsers.
///
/// Maintains a mapping from annex names (lowercased) to parser
/// implementations. Use [`AnnexRegistry::register`] to add parsers
/// and [`AnnexRegistry::parse`] to dispatch to the right parser.
///
/// # Example
///
/// ```rust,ignore
/// let mut registry = AnnexRegistry::new();
/// registry.register(Arc::new(Emv2Parser));
///
/// // Parse an EMV2 annex
/// let result = registry.parse("EMV2", "use types ErrorLibrary;");
/// ```
pub struct AnnexRegistry {
    parsers: HashMap<String, Arc<dyn AnnexParser>>,
}

impl AnnexRegistry {
    /// Create an empty registry with no parsers.
    pub fn new() -> Self {
        Self {
            parsers: HashMap::new(),
        }
    }

    /// Register an annex parser.
    ///
    /// The parser's `names()` method determines which annex names it
    /// handles. All names are stored lowercased for case-insensitive
    /// matching.
    pub fn register(&mut self, parser: Arc<dyn AnnexParser>) {
        for name in parser.names() {
            self.parsers
                .insert(name.to_ascii_lowercase(), Arc::clone(&parser));
        }
    }

    /// Parse annex content by dispatching to the registered parser.
    ///
    /// Returns `None` if no parser is registered for the given annex name.
    /// Returns `Some(result)` with the parse result if a parser was found.
    pub fn parse(&self, name: &str, source: &str) -> Option<AnnexParseResult> {
        let key = name.to_ascii_lowercase();
        let parser = self.parsers.get(&key)?;
        Some(parser.parse(name, source))
    }

    /// Check if a parser is registered for the given annex name.
    pub fn has_parser(&self, name: &str) -> bool {
        self.parsers.contains_key(&name.to_ascii_lowercase())
    }

    /// List all registered annex names.
    pub fn registered_names(&self) -> Vec<&str> {
        self.parsers.keys().map(|s| s.as_str()).collect()
    }

    /// Parse all annexes in a syntax tree.
    ///
    /// Walks the CST, finds all ANNEX_SUBCLAUSE and ANNEX_LIBRARY nodes,
    /// extracts their content, and dispatches to registered parsers.
    ///
    /// Returns a list of `(annex_name, node_text_range, parse_result)` for
    /// each annex that had a registered parser.
    pub fn parse_all_annexes(
        &self,
        root: &spar_syntax::SyntaxNode,
    ) -> Vec<(String, rowan::TextRange, AnnexParseResult)> {
        let mut results = Vec::new();
        self.walk_for_annexes(root, &mut results);
        // Also check direct children that might not be recursed into
        let _ = &results; // suppress unused warning
        results
    }

    fn walk_for_annexes(
        &self,
        node: &spar_syntax::SyntaxNode,
        results: &mut Vec<(String, rowan::TextRange, AnnexParseResult)>,
    ) {
        use spar_syntax::SyntaxKind;

        let kind = node.kind();
        if kind == SyntaxKind::ANNEX_SUBCLAUSE || kind == SyntaxKind::ANNEX_LIBRARY {
            if let Some((name, text)) = crate::extract_annex_content(node) {
                if let Some(result) = self.parse(&name, &text) {
                    results.push((name, node.text_range(), result));
                }
            }
        }

        for child in node.children() {
            self.walk_for_annexes(&child, results);
        }
    }
}

impl Default for AnnexRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestParser;

    impl AnnexParser for TestParser {
        fn names(&self) -> &[&str] {
            &["TestAnnex", "test_annex"]
        }

        fn parse(&self, _name: &str, source: &str) -> AnnexParseResult {
            // Simple test: just count tokens
            let mut result = AnnexParseResult::empty();
            if source.trim().is_empty() {
                result.diagnostics.push(crate::AnnexDiagnostic {
                    span: crate::Span::new(0, 0),
                    message: "empty annex content".to_string(),
                    severity: crate::Severity::Warning,
                });
            }
            result
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = AnnexRegistry::new();
        reg.register(Arc::new(TestParser));

        assert!(reg.has_parser("TestAnnex"));
        assert!(reg.has_parser("testannex")); // case-insensitive
        assert!(reg.has_parser("TESTANNEX"));
        assert!(!reg.has_parser("EMV2"));
    }

    #[test]
    fn parse_dispatches() {
        let mut reg = AnnexRegistry::new();
        reg.register(Arc::new(TestParser));

        let result = reg.parse("TestAnnex", "some content");
        assert!(result.is_some());
        assert!(!result.unwrap().has_errors());

        let result = reg.parse("TestAnnex", "  ");
        assert!(result.is_some());
        assert_eq!(result.unwrap().diagnostics.len(), 1);
    }

    #[test]
    fn unknown_annex_returns_none() {
        let reg = AnnexRegistry::new();
        assert!(reg.parse("EMV2", "anything").is_none());
    }

    #[test]
    fn parse_all_annexes_integration() {
        let mut reg = AnnexRegistry::new();

        struct Emv2Stub;
        impl AnnexParser for Emv2Stub {
            fn names(&self) -> &[&str] {
                &["EMV2"]
            }
            fn parse(&self, _name: &str, _source: &str) -> AnnexParseResult {
                AnnexParseResult::empty()
            }
        }

        reg.register(Arc::new(Emv2Stub));

        let input = r#"package P
public
  system S
    annex EMV2 {** use types ErrorLibrary; **};
  end S;
end P;
"#;
        let parse = spar_syntax::parse(input);
        let root = parse.syntax_node();
        let results = reg.parse_all_annexes(&root);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "EMV2");
    }

    #[test]
    fn parsed_file_full_pipeline() {
        let mut reg = AnnexRegistry::new();

        struct Emv2Counter;
        impl AnnexParser for Emv2Counter {
            fn names(&self) -> &[&str] {
                &["EMV2"]
            }
            fn parse(&self, _name: &str, source: &str) -> AnnexParseResult {
                let word_count = source.split_whitespace().count();
                let mut result = AnnexParseResult::empty();
                result.nodes.push(crate::AnnexNode {
                    kind: "emv2-library".to_string(),
                    span: crate::Span::new(0, source.len() as u32),
                    parent: -1,
                    text: String::new(),
                });
                if word_count == 0 {
                    result.diagnostics.push(crate::AnnexDiagnostic {
                        span: crate::Span::new(0, 0),
                        message: "empty EMV2 content".to_string(),
                        severity: crate::Severity::Error,
                    });
                }
                result
            }
        }

        reg.register(Arc::new(Emv2Counter));

        let input = r#"package P
public
  system S
    annex EMV2 {** use types ErrorLibrary; **};
  end S;
  system implementation S.i
    annex EMV2 {** use behavior ErrorBehavior; **};
  end S.i;
end P;
"#;
        let parsed = crate::ParsedFile::parse(input, &reg);

        // No core parse errors
        assert!(parsed.parse.errors().is_empty());
        // Two EMV2 annexes found and parsed
        assert_eq!(parsed.annexes.len(), 2);
        let emv2s: Vec<_> = parsed.annexes_by_name("EMV2").collect();
        assert_eq!(emv2s.len(), 2);
        // Each has one root node
        assert_eq!(emv2s[0].result.nodes.len(), 1);
        assert_eq!(emv2s[0].result.nodes[0].kind, "emv2-library");
        // No errors
        assert!(!emv2s[0].result.has_errors());
    }
}
