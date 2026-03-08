//! BA lexer.
//!
//! Tokenizes the text between `{**` and `**}` in a Behavior Annex.
//! Produces `(BaKind, byte_length)` pairs for the Rowan tree builder.
//!
//! BA uses AADL-style `--` comments and case-insensitive keywords.
//!
//! Specification: SAE AS5506/2 Annex D

use super::syntax_kind::BaKind;

/// Tokenize BA source text into `(kind, byte_length)` pairs.
pub(crate) fn tokenize(source: &str) -> Vec<(BaKind, usize)> {
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
                BaKind::WHITESPACE
            }

            // Comment: -- to end of line
            b'-' if i + 1 < len && bytes[i + 1] == b'-' => {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                BaKind::COMMENT
            }

            // -[ (transition open)
            b'-' if i + 1 < len && bytes[i + 1] == b'[' => {
                i += 2;
                BaKind::TRANS_OPEN
            }

            // -> (arrow)
            b'-' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                BaKind::ARROW
            }

            // - (minus)
            b'-' => {
                i += 1;
                BaKind::MINUS
            }

            // ]-> (transition close)
            b']' if i + 2 < len && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' => {
                i += 3;
                BaKind::TRANS_CLOSE
            }

            // ] (right bracket)
            b']' => {
                i += 1;
                BaKind::R_BRACK
            }

            // := (assign) or :: (qualify) or : (colon)
            b':' if i + 1 < len && bytes[i + 1] == b'=' => {
                i += 2;
                BaKind::COLON_EQ
            }
            b':' if i + 1 < len && bytes[i + 1] == b':' => {
                i += 2;
                BaKind::COLON_COLON
            }
            b':' => {
                i += 1;
                BaKind::COLON
            }

            // => (fat arrow)
            b'=' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                BaKind::FAT_ARROW
            }

            // = (equals)
            b'=' => {
                i += 1;
                BaKind::EQ
            }

            // ** (power) or * (star)
            b'*' if i + 1 < len && bytes[i + 1] == b'*' => {
                i += 2;
                BaKind::STAR_STAR
            }
            b'*' => {
                i += 1;
                BaKind::STAR
            }

            // .. (range) or . (dot)
            b'.' if i + 1 < len && bytes[i + 1] == b'.' => {
                i += 2;
                BaKind::DOT_DOT
            }
            b'.' => {
                i += 1;
                BaKind::DOT
            }

            // != or !< or !> or ! (bang)
            b'!' if i + 1 < len && bytes[i + 1] == b'=' => {
                i += 2;
                BaKind::BANG_EQ
            }
            b'!' if i + 1 < len && bytes[i + 1] == b'<' => {
                i += 2;
                BaKind::BANG_L_ANGLE
            }
            b'!' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                BaKind::BANG_R_ANGLE
            }
            b'!' => {
                i += 1;
                BaKind::BANG
            }

            // <= or < (angle)
            b'<' if i + 1 < len && bytes[i + 1] == b'=' => {
                i += 2;
                BaKind::L_ANGLE_EQ
            }
            b'<' => {
                i += 1;
                BaKind::L_ANGLE
            }

            // >> or >= or > (angle)
            b'>' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                BaKind::R_ANGLE_R_ANGLE
            }
            b'>' if i + 1 < len && bytes[i + 1] == b'=' => {
                i += 2;
                BaKind::R_ANGLE_EQ
            }
            b'>' => {
                i += 1;
                BaKind::R_ANGLE
            }

            // Simple single-char symbols
            b';' => {
                i += 1;
                BaKind::SEMICOLON
            }
            b',' => {
                i += 1;
                BaKind::COMMA
            }
            b'{' => {
                i += 1;
                BaKind::L_CURLY
            }
            b'}' => {
                i += 1;
                BaKind::R_CURLY
            }
            b'(' => {
                i += 1;
                BaKind::L_PAREN
            }
            b')' => {
                i += 1;
                BaKind::R_PAREN
            }
            b'[' => {
                i += 1;
                BaKind::L_BRACK
            }
            b'+' => {
                i += 1;
                BaKind::PLUS
            }
            b'/' => {
                i += 1;
                BaKind::SLASH
            }
            b'&' => {
                i += 1;
                BaKind::AMP
            }
            b'?' => {
                i += 1;
                BaKind::QUESTION
            }
            b'\'' => {
                i += 1;
                BaKind::TICK
            }
            b'#' => {
                i += 1;
                BaKind::HASH
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
                BaKind::STRING_LIT
            }

            // Number (integer or real)
            b'0'..=b'9' => {
                i += 1;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let mut kind = BaKind::INT_LIT;
                if i < len
                    && bytes[i] == b'.'
                    && i + 1 < len
                    && bytes[i + 1].is_ascii_digit()
                {
                    kind = BaKind::REAL_LIT;
                    i += 1;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                // Optional exponent
                if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
                    kind = BaKind::REAL_LIT;
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
                BaKind::from_keyword(text).unwrap_or(BaKind::IDENT)
            }

            // Unknown character
            _ => {
                i += 1;
                BaKind::ERROR
            }
        };

        tokens.push((kind, i - start));
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<BaKind> {
        tokenize(source)
            .into_iter()
            .filter(|(k, _)| !k.is_trivia())
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn lex_variable_decl() {
        assert_eq!(
            kinds("tmp : number;"),
            vec![BaKind::IDENT, BaKind::COLON, BaKind::IDENT, BaKind::SEMICOLON]
        );
    }

    #[test]
    fn lex_state_decl() {
        assert_eq!(
            kinds("s0 : initial complete final state;"),
            vec![
                BaKind::IDENT,
                BaKind::COLON,
                BaKind::INITIAL_KW,
                BaKind::COMPLETE_KW,
                BaKind::FINAL_KW,
                BaKind::STATE_KW,
                BaKind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn lex_transition() {
        assert_eq!(
            kinds("s0 -[ on dispatch ]-> s1"),
            vec![
                BaKind::IDENT,
                BaKind::TRANS_OPEN,
                BaKind::ON_KW,
                BaKind::DISPATCH_KW,
                BaKind::TRANS_CLOSE,
                BaKind::IDENT,
            ]
        );
    }

    #[test]
    fn lex_assignment() {
        assert_eq!(
            kinds("x := y + 1;"),
            vec![
                BaKind::IDENT,
                BaKind::COLON_EQ,
                BaKind::IDENT,
                BaKind::PLUS,
                BaKind::INT_LIT,
                BaKind::SEMICOLON,
            ]
        );
    }

    #[test]
    fn lex_communication_actions() {
        assert_eq!(
            kinds("port! port!(v) port?(x) port>>"),
            vec![
                BaKind::IDENT,
                BaKind::BANG,
                BaKind::IDENT,
                BaKind::BANG,
                BaKind::L_PAREN,
                BaKind::IDENT,
                BaKind::R_PAREN,
                BaKind::IDENT,
                BaKind::QUESTION,
                BaKind::L_PAREN,
                BaKind::IDENT,
                BaKind::R_PAREN,
                BaKind::IDENT,
                BaKind::R_ANGLE_R_ANGLE,
            ]
        );
    }

    #[test]
    fn lex_port_property() {
        assert_eq!(
            kinds("tick'count"),
            vec![BaKind::IDENT, BaKind::TICK, BaKind::COUNT_KW]
        );
    }

    #[test]
    fn lex_qualified_name() {
        assert_eq!(
            kinds("pkg::type"),
            vec![BaKind::IDENT, BaKind::COLON_COLON, BaKind::IDENT]
        );
    }

    #[test]
    fn lex_relational_ops() {
        assert_eq!(
            kinds("a = b != c < d <= e > f >= g"),
            vec![
                BaKind::IDENT, BaKind::EQ, BaKind::IDENT, BaKind::BANG_EQ,
                BaKind::IDENT, BaKind::L_ANGLE, BaKind::IDENT, BaKind::L_ANGLE_EQ,
                BaKind::IDENT, BaKind::R_ANGLE, BaKind::IDENT, BaKind::R_ANGLE_EQ,
                BaKind::IDENT,
            ]
        );
    }

    #[test]
    fn lex_power_operator() {
        assert_eq!(
            kinds("x ** 2"),
            vec![BaKind::IDENT, BaKind::STAR_STAR, BaKind::INT_LIT]
        );
    }

    #[test]
    fn lex_range() {
        assert_eq!(
            kinds("1 .. 10"),
            vec![BaKind::INT_LIT, BaKind::DOT_DOT, BaKind::INT_LIT]
        );
    }

    #[test]
    fn lex_comment() {
        assert_eq!(
            kinds("variables -- comment\nstates"),
            vec![BaKind::VARIABLES_KW, BaKind::STATES_KW]
        );
    }

    #[test]
    fn lex_real_literal() {
        assert_eq!(
            kinds("3.14 1.5e-3"),
            vec![BaKind::REAL_LIT, BaKind::REAL_LIT]
        );
    }

    #[test]
    fn lex_case_insensitive() {
        assert_eq!(
            kinds("Variables STATES Transitions"),
            vec![BaKind::VARIABLES_KW, BaKind::STATES_KW, BaKind::TRANSITIONS_KW]
        );
    }

    #[test]
    fn byte_lengths() {
        let tokens = tokenize("variables states");
        assert_eq!(
            tokens,
            vec![
                (BaKind::VARIABLES_KW, 9),
                (BaKind::WHITESPACE, 1),
                (BaKind::STATES_KW, 6),
            ]
        );
    }

    #[test]
    fn lex_hash_property_ref() {
        assert_eq!(
            kinds("#PropertySet::Prop"),
            vec![
                BaKind::HASH,
                BaKind::IDENT,
                BaKind::COLON_COLON,
                BaKind::IDENT,
            ]
        );
    }

    #[test]
    fn lex_string_literal() {
        assert_eq!(kinds("\"hello\""), vec![BaKind::STRING_LIT]);
    }

    #[test]
    fn lex_data_access_ops() {
        assert_eq!(
            kinds("p!< p!>"),
            vec![
                BaKind::IDENT, BaKind::BANG_L_ANGLE,
                BaKind::IDENT, BaKind::BANG_R_ANGLE,
            ]
        );
    }
}
