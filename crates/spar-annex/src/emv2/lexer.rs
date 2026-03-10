//! EMV2 lexer.
//!
//! Tokenizes the text between `{**` and `**}` in an EMV2 annex.
//! Produces `(Emv2Kind, byte_length)` pairs for the Rowan tree builder.
//!
//! EMV2 uses AADL-style `--` comments and case-insensitive keywords.
//!
//! Specification: SAE AS5506/1 Annex E

use super::syntax_kind::Emv2Kind;

/// Tokenize EMV2 source text into `(kind, byte_length)` pairs.
pub(crate) fn tokenize(source: &str) -> Vec<(Emv2Kind, usize)> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let start = i;
        let b = bytes[i];

        let kind = match b {
            // Whitespace
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
                while i < len && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                Emv2Kind::WHITESPACE
            }

            // Comment: -- to end of line
            b'-' if i + 1 < len && bytes[i + 1] == b'-' => {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                Emv2Kind::COMMENT
            }

            // -[ (transition open)
            b'-' if i + 1 < len && bytes[i + 1] == b'[' => {
                i += 2;
                Emv2Kind::TRANS_OPEN
            }

            // -> (arrow)
            b'-' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                Emv2Kind::ARROW
            }

            // - (minus)
            b'-' => {
                i += 1;
                Emv2Kind::MINUS
            }

            // ]-> (transition close)
            b']' if i + 2 < len && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' => {
                i += 3;
                Emv2Kind::TRANS_CLOSE
            }

            // ] (right bracket)
            b']' => {
                i += 1;
                Emv2Kind::R_BRACK
            }

            // :: or :
            b':' if i + 1 < len && bytes[i + 1] == b':' => {
                i += 2;
                Emv2Kind::COLON_COLON
            }
            b':' => {
                i += 1;
                Emv2Kind::COLON
            }

            // => (fat arrow)
            b'=' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                Emv2Kind::FAT_ARROW
            }

            // Simple single-char symbols
            b';' => {
                i += 1;
                Emv2Kind::SEMICOLON
            }
            b',' => {
                i += 1;
                Emv2Kind::COMMA
            }
            b'.' => {
                i += 1;
                Emv2Kind::DOT
            }
            b'*' => {
                i += 1;
                Emv2Kind::STAR
            }
            b'{' => {
                i += 1;
                Emv2Kind::L_CURLY
            }
            b'}' => {
                i += 1;
                Emv2Kind::R_CURLY
            }
            b'(' => {
                i += 1;
                Emv2Kind::L_PAREN
            }
            b')' => {
                i += 1;
                Emv2Kind::R_PAREN
            }
            b'[' => {
                i += 1;
                Emv2Kind::L_BRACK
            }
            b'!' => {
                i += 1;
                Emv2Kind::BANG
            }
            b'^' => {
                i += 1;
                Emv2Kind::CARET
            }
            b'@' => {
                i += 1;
                Emv2Kind::AT
            }

            // String literal
            b'"' => {
                i += 1;
                while i < len && bytes[i] != b'"' {
                    i += 1;
                }
                if i < len {
                    i += 1; // closing quote
                }
                Emv2Kind::STRING_LIT
            }

            // Number (integer or real)
            b'0'..=b'9' => {
                i += 1;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let mut kind = Emv2Kind::INT_LIT;
                if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                    kind = Emv2Kind::REAL_LIT;
                    i += 1;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                // Optional exponent
                if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
                    kind = Emv2Kind::REAL_LIT;
                    i += 1;
                    if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                kind
            }

            // Identifier or keyword
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let text = &source[start..i];
                Emv2Kind::from_keyword(text).unwrap_or(Emv2Kind::IDENT)
            }

            // Unknown character
            _ => {
                i += 1;
                Emv2Kind::ERROR
            }
        };

        tokens.push((kind, i - start));
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Emv2Kind> {
        tokenize(source)
            .into_iter()
            .filter(|(k, _)| !k.is_trivia())
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn lex_type_definition() {
        assert_eq!(
            kinds("ServiceError: type;"),
            vec![
                Emv2Kind::IDENT,
                Emv2Kind::COLON,
                Emv2Kind::TYPE_KW,
                Emv2Kind::SEMICOLON
            ]
        );
    }

    #[test]
    fn lex_transition() {
        assert_eq!(
            kinds("Operational -[ Failure ]-> FailStop ;"),
            vec![
                Emv2Kind::IDENT,
                Emv2Kind::TRANS_OPEN,
                Emv2Kind::IDENT,
                Emv2Kind::TRANS_CLOSE,
                Emv2Kind::IDENT,
                Emv2Kind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn lex_composite_state() {
        assert_eq!(
            kinds("[a0.failstop and a1.failstop]-> failstop;"),
            vec![
                Emv2Kind::L_BRACK,
                Emv2Kind::IDENT,
                Emv2Kind::DOT,
                Emv2Kind::IDENT,
                Emv2Kind::AND_KW,
                Emv2Kind::IDENT,
                Emv2Kind::DOT,
                Emv2Kind::IDENT,
                Emv2Kind::TRANS_CLOSE,
                Emv2Kind::IDENT,
                Emv2Kind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn lex_qualified_ref() {
        assert_eq!(
            kinds("ErrorLibrary::FailStop"),
            vec![Emv2Kind::IDENT, Emv2Kind::COLON_COLON, Emv2Kind::IDENT]
        );
    }

    #[test]
    fn lex_comment() {
        assert_eq!(
            kinds("error -- comment\ntypes"),
            vec![Emv2Kind::ERROR_KW, Emv2Kind::TYPES_KW]
        );
    }

    #[test]
    fn lex_branch_value() {
        assert_eq!(
            kinds("(Failed with 0.5, FailStop with others)"),
            vec![
                Emv2Kind::L_PAREN,
                Emv2Kind::IDENT,
                Emv2Kind::WITH_KW,
                Emv2Kind::REAL_LIT,
                Emv2Kind::COMMA,
                Emv2Kind::IDENT,
                Emv2Kind::WITH_KW,
                Emv2Kind::OTHERS_KW,
                Emv2Kind::R_PAREN,
            ]
        );
    }

    #[test]
    fn lex_case_insensitive() {
        assert_eq!(
            kinds("Error TYPES End"),
            vec![Emv2Kind::ERROR_KW, Emv2Kind::TYPES_KW, Emv2Kind::END_KW]
        );
    }

    #[test]
    fn byte_lengths() {
        let tokens = tokenize("error types");
        assert_eq!(
            tokens,
            vec![
                (Emv2Kind::ERROR_KW, 5),
                (Emv2Kind::WHITESPACE, 1),
                (Emv2Kind::TYPES_KW, 5),
            ]
        );
    }
}
