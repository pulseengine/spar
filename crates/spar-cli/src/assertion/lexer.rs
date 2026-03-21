//! Lexer for the assertion expression language.
//!
//! Produces `(ExprSyntaxKind, &str)` token pairs from an input string.

use super::syntax::ExprSyntaxKind;

/// Tokenize the input string into `(kind, text)` pairs.
pub(crate) fn lex(input: &str) -> Vec<(ExprSyntaxKind, String)> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut pos = 0;

    while pos < bytes.len() {
        let start = pos;

        match bytes[pos] {
            // Whitespace
            b' ' | b'\t' | b'\n' | b'\r' => {
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                tokens.push((ExprSyntaxKind::WHITESPACE, input[start..pos].to_string()));
            }

            // Single-quoted string literal
            b'\'' => {
                pos += 1; // consume opening quote
                while pos < bytes.len() && bytes[pos] != b'\'' {
                    pos += 1;
                }
                if pos < bytes.len() {
                    pos += 1; // consume closing quote
                }
                // Includes quotes in the token text
                tokens.push((ExprSyntaxKind::STRING_LIT, input[start..pos].to_string()));
            }

            // Symbols
            b'.' => {
                pos += 1;
                tokens.push((ExprSyntaxKind::DOT, ".".to_string()));
            }
            b'(' => {
                pos += 1;
                tokens.push((ExprSyntaxKind::L_PAREN, "(".to_string()));
            }
            b')' => {
                pos += 1;
                tokens.push((ExprSyntaxKind::R_PAREN, ")".to_string()));
            }
            b',' => {
                pos += 1;
                tokens.push((ExprSyntaxKind::COMMA, ",".to_string()));
            }
            b'=' if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' => {
                pos += 2;
                tokens.push((ExprSyntaxKind::EQ_EQ, "==".to_string()));
            }

            // Identifiers and keywords
            b if is_ident_start(b) => {
                while pos < bytes.len() && is_ident_continue(bytes[pos]) {
                    pos += 1;
                }
                let text = &input[start..pos];
                let kind = match text {
                    "and" => ExprSyntaxKind::AND_KW,
                    "or" => ExprSyntaxKind::OR_KW,
                    "not" => ExprSyntaxKind::NOT_KW,
                    _ => ExprSyntaxKind::IDENT,
                };
                tokens.push((kind, text.to_string()));
            }

            // Anything else is an error token — advance a full UTF-8 character
            _ => {
                // Advance past the full UTF-8 character, not just one byte.
                let ch = input[start..].chars().next().unwrap();
                pos += ch.len_utf8();
                tokens.push((ExprSyntaxKind::ERROR, input[start..pos].to_string()));
            }
        }
    }

    tokens
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn lex_simple_pipeline() {
        let tokens = lex("components.where(category == 'thread')");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ExprSyntaxKind::IDENT,      // components
                ExprSyntaxKind::DOT,        // .
                ExprSyntaxKind::IDENT,      // where
                ExprSyntaxKind::L_PAREN,    // (
                ExprSyntaxKind::IDENT,      // category
                ExprSyntaxKind::WHITESPACE, // ' '
                ExprSyntaxKind::EQ_EQ,      // ==
                ExprSyntaxKind::WHITESPACE, // ' '
                ExprSyntaxKind::STRING_LIT, // 'thread'
                ExprSyntaxKind::R_PAREN,    // )
            ]
        );
    }

    #[test]
    fn lex_keywords() {
        let tokens = lex("not a and b or c");
        let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ExprSyntaxKind::NOT_KW,
                ExprSyntaxKind::WHITESPACE,
                ExprSyntaxKind::IDENT,
                ExprSyntaxKind::WHITESPACE,
                ExprSyntaxKind::AND_KW,
                ExprSyntaxKind::WHITESPACE,
                ExprSyntaxKind::IDENT,
                ExprSyntaxKind::WHITESPACE,
                ExprSyntaxKind::OR_KW,
                ExprSyntaxKind::WHITESPACE,
                ExprSyntaxKind::IDENT,
            ]
        );
    }

    #[test]
    fn lex_string_literal() {
        let tokens = lex("'Timing_Properties::Period'");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, ExprSyntaxKind::STRING_LIT);
        assert_eq!(tokens[0].1, "'Timing_Properties::Period'");
    }

    #[test]
    fn lex_error_token() {
        let tokens = lex("@");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, ExprSyntaxKind::ERROR);
    }

    // ── Property-based tests ────────────────────────────────────────

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(
            std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100)
        ))]

        /// The lexer must never panic, regardless of input.
        #[test]
        fn lexer_never_panics(input in "\\PC{0,300}") {
            let tokens = lex(&input);
            // Every byte of input must be accounted for in tokens.
            let total_len: usize = tokens.iter().map(|(_, text)| text.len()).sum();
            prop_assert_eq!(total_len, input.len(), "token lengths must sum to input length");
        }

        /// The lexer must never panic on arbitrary unicode.
        #[test]
        fn lexer_never_panics_unicode(input in ".{0,200}") {
            let tokens = lex(&input);
            let total_len: usize = tokens.iter().map(|(_, text)| text.len()).sum();
            prop_assert_eq!(total_len, input.len());
        }

        /// Concatenating all token texts must reconstruct the original input.
        #[test]
        fn lexer_roundtrip(input in "\\PC{0,300}") {
            let tokens = lex(&input);
            let reconstructed: String = tokens.iter().map(|(_, text)| text.as_str()).collect();
            prop_assert_eq!(reconstructed, input, "token texts must reconstruct input");
        }
    }
}
