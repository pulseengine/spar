//! Recursive descent parser for the assertion expression language.
//!
//! Builds a `rowan::GreenNode` using `rowan::GreenNodeBuilder`, producing the
//! same tree structure that the old `enum Expr` / `enum BoolExpr` represented.

use rowan::GreenNodeBuilder;

use super::lexer::lex;
use super::syntax::{ExprSyntaxKind, SyntaxNode};

/// Result of parsing an assertion expression.
#[derive(Clone)]
pub(crate) struct ParseResult {
    green: rowan::GreenNode,
    errors: Vec<String>,
}

impl ParseResult {
    /// Build a typed `SyntaxNode` root from the green tree.
    pub(crate) fn syntax_node(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// Return the list of parse errors.
    pub(crate) fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Returns `true` if there were no parse errors.
    pub(crate) fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Parse an assertion expression string into a rowan green tree.
pub(crate) fn parse_expr(input: &str) -> ParseResult {
    let tokens = lex(input);
    let mut p = ExprParser::new(tokens);
    p.parse_root();
    p.finish()
}

// ── Parser internals ────────────────────────────────────────────────

struct ExprParser {
    tokens: Vec<(ExprSyntaxKind, String)>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<String>,
}

impl ExprParser {
    fn new(tokens: Vec<(ExprSyntaxKind, String)>) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
        }
    }

    fn finish(self) -> ParseResult {
        ParseResult {
            green: self.builder.finish(),
            errors: self.errors,
        }
    }

    // ── Token helpers ───────────────────────────────────────────────

    fn current_kind(&self) -> Option<ExprSyntaxKind> {
        let mut i = self.pos;
        while i < self.tokens.len() {
            if self.tokens[i].0 != ExprSyntaxKind::WHITESPACE {
                return Some(self.tokens[i].0);
            }
            i += 1;
        }
        None
    }

    fn current_text(&self) -> &str {
        let mut i = self.pos;
        while i < self.tokens.len() {
            if self.tokens[i].0 != ExprSyntaxKind::WHITESPACE {
                return &self.tokens[i].1;
            }
            i += 1;
        }
        ""
    }

    fn at_kind(&self, kind: ExprSyntaxKind) -> bool {
        self.current_kind() == Some(kind)
    }

    fn at_ident(&self, text: &str) -> bool {
        self.current_kind() == Some(ExprSyntaxKind::IDENT) && self.current_text() == text
    }

    fn at_eof(&self) -> bool {
        self.current_kind().is_none()
    }

    /// Peek at the nth non-whitespace token kind from current position.
    fn peek_kind(&self, n: usize) -> Option<ExprSyntaxKind> {
        let mut count = 0;
        let mut i = self.pos;
        while i < self.tokens.len() {
            if self.tokens[i].0 != ExprSyntaxKind::WHITESPACE {
                if count == n {
                    return Some(self.tokens[i].0);
                }
                count += 1;
            }
            i += 1;
        }
        None
    }

    // ── Token consumption ───────────────────────────────────────────

    fn eat_ws(&mut self) {
        while self.pos < self.tokens.len()
            && self.tokens[self.pos].0 == ExprSyntaxKind::WHITESPACE
        {
            let (kind, ref text) = self.tokens[self.pos];
            self.builder.token(kind.into(), text);
            self.pos += 1;
        }
    }

    fn bump(&mut self) {
        self.eat_ws();
        if self.pos < self.tokens.len() {
            let (kind, ref text) = self.tokens[self.pos];
            self.builder.token(kind.into(), text);
            self.pos += 1;
        }
    }

    fn expect(&mut self, kind: ExprSyntaxKind) -> bool {
        self.eat_ws();
        if self.pos < self.tokens.len() && self.tokens[self.pos].0 == kind {
            let (k, ref text) = self.tokens[self.pos];
            self.builder.token(k.into(), text);
            self.pos += 1;
            true
        } else {
            let found = if self.pos < self.tokens.len() {
                format!("'{}'", self.tokens[self.pos].1)
            } else {
                "EOF".to_string()
            };
            self.errors
                .push(format!("expected '{:?}', found {}", kind, found));
            false
        }
    }

    fn expect_ident(&mut self, text: &str) -> bool {
        self.eat_ws();
        if self.pos < self.tokens.len()
            && self.tokens[self.pos].0 == ExprSyntaxKind::IDENT
            && self.tokens[self.pos].1 == text
        {
            let (k, ref t) = self.tokens[self.pos];
            self.builder.token(k.into(), t);
            self.pos += 1;
            true
        } else {
            let found = if self.pos < self.tokens.len() {
                format!("'{}'", self.tokens[self.pos].1)
            } else {
                "EOF".to_string()
            };
            self.errors
                .push(format!("expected '{}', found {}", text, found));
            false
        }
    }

    // ── Grammar rules ───────────────────────────────────────────────

    fn parse_root(&mut self) {
        self.builder.start_node(ExprSyntaxKind::ROOT.into());
        self.parse_pipeline();
        self.eat_ws();
        if !self.at_eof() {
            let text = self.current_text().to_string();
            self.errors
                .push(format!("unexpected '{}'", text));
            // Consume remaining tokens as error
            while self.pos < self.tokens.len() {
                let (kind, ref text) = self.tokens[self.pos];
                self.builder.token(kind.into(), text);
                self.pos += 1;
            }
        }
        self.builder.finish_node();
    }

    /// pipeline = source ( '.' method )*
    fn parse_pipeline(&mut self) {
        self.builder
            .start_node(ExprSyntaxKind::PIPELINE_EXPR.into());

        // Parse source
        if self.at_ident("components") {
            self.builder
                .start_node(ExprSyntaxKind::IDENT_EXPR.into());
            self.bump();
            self.builder.finish_node();
        } else if self.at_ident("analysis") {
            self.builder
                .start_node(ExprSyntaxKind::CALL_EXPR.into());
            self.bump(); // 'analysis'
            self.expect(ExprSyntaxKind::L_PAREN);
            self.builder.start_node(ExprSyntaxKind::LITERAL.into());
            self.expect(ExprSyntaxKind::STRING_LIT);
            self.builder.finish_node();
            self.expect(ExprSyntaxKind::R_PAREN);
            self.builder.finish_node();
        } else {
            let text = self.current_text().to_string();
            self.errors.push(format!(
                "expected 'components' or 'analysis', found '{}'",
                if text.is_empty() { "EOF" } else { &text }
            ));
        }

        // Method chain
        loop {
            self.eat_ws();
            if !self.at_kind(ExprSyntaxKind::DOT) {
                break;
            }
            self.parse_dot_call();
        }

        self.builder.finish_node();
    }

    /// dot_call = '.' ident [ '(' args ')' ]
    fn parse_dot_call(&mut self) {
        self.builder
            .start_node(ExprSyntaxKind::DOT_CALL.into());
        self.expect(ExprSyntaxKind::DOT);

        let method = self.current_text().to_string();
        match method.as_str() {
            "where" | "all" | "any" | "none" => {
                self.bump();
                self.builder
                    .start_node(ExprSyntaxKind::CALL_ARGS.into());
                self.expect(ExprSyntaxKind::L_PAREN);
                self.parse_or_expr();
                self.expect(ExprSyntaxKind::R_PAREN);
                self.builder.finish_node();
            }
            "count" => {
                self.bump();
                self.builder
                    .start_node(ExprSyntaxKind::CALL_ARGS.into());
                self.expect(ExprSyntaxKind::L_PAREN);
                self.expect(ExprSyntaxKind::R_PAREN);
                self.builder.finish_node();
            }
            "features" | "diagnostics" => {
                self.bump();
            }
            _ => {
                self.errors.push(format!(
                    "expected method name (where, all, any, none, count, features, diagnostics), found '{}'",
                    method
                ));
                if !self.at_eof() {
                    self.bump();
                }
            }
        }

        self.builder.finish_node();
    }

    // ── Boolean expression grammar ──────────────────────────────────
    //
    // or_expr   = and_expr ( 'or' and_expr )*
    // and_expr  = atom ( 'and' atom )*
    // atom      = 'not' atom
    //           | 'has' '(' STRING ')'
    //           | 'connected'
    //           | field '==' STRING
    //           | 'message' '.' 'contains' '(' STRING ')'
    //           | '(' or_expr ')'
    //
    // If there's a single operand, the atom/term is emitted directly.
    // If there are multiple, we wrap in a BINARY_EXPR.
    // We pre-scan to decide whether to wrap.

    fn parse_or_expr(&mut self) {
        let has_or = self.has_binary_op_at_level(ExprSyntaxKind::OR_KW);

        if has_or {
            self.builder
                .start_node(ExprSyntaxKind::BINARY_EXPR.into());
            self.parse_and_expr();
            while self.at_kind(ExprSyntaxKind::OR_KW) {
                self.bump(); // 'or'
                self.parse_and_expr();
            }
            self.builder.finish_node();
        } else {
            self.parse_and_expr();
        }
    }

    fn parse_and_expr(&mut self) {
        let has_and = self.has_binary_op_at_level(ExprSyntaxKind::AND_KW);

        if has_and {
            self.builder
                .start_node(ExprSyntaxKind::BINARY_EXPR.into());
            self.parse_atom();
            while self.at_kind(ExprSyntaxKind::AND_KW) {
                self.bump(); // 'and'
                self.parse_atom();
            }
            self.builder.finish_node();
        } else {
            self.parse_atom();
        }
    }

    /// Check whether there's a binary operator at the current nesting level.
    /// Tracks paren depth and looks for the given keyword at depth 0.
    fn has_binary_op_at_level(&self, op: ExprSyntaxKind) -> bool {
        let mut depth: i32 = 0;
        let mut i = self.pos;
        let mut found_first_atom = false;
        while i < self.tokens.len() {
            let kind = self.tokens[i].0;
            match kind {
                ExprSyntaxKind::WHITESPACE => {}
                ExprSyntaxKind::L_PAREN => {
                    depth += 1;
                    found_first_atom = true;
                }
                ExprSyntaxKind::R_PAREN => {
                    if depth == 0 {
                        return false; // end of our scope
                    }
                    depth -= 1;
                }
                _ if kind == op && depth == 0 && found_first_atom => {
                    return true;
                }
                _ => {
                    found_first_atom = true;
                }
            }
            i += 1;
        }
        false
    }

    fn parse_atom(&mut self) {
        self.eat_ws();

        // not
        if self.at_kind(ExprSyntaxKind::NOT_KW) {
            self.builder
                .start_node(ExprSyntaxKind::UNARY_EXPR.into());
            self.bump(); // 'not'
            self.parse_atom();
            self.builder.finish_node();
            return;
        }

        // parenthesized expression
        if self.at_kind(ExprSyntaxKind::L_PAREN) {
            self.builder
                .start_node(ExprSyntaxKind::PAREN_EXPR.into());
            self.bump(); // '('
            self.parse_or_expr();
            self.expect(ExprSyntaxKind::R_PAREN);
            self.builder.finish_node();
            return;
        }

        // has('...')
        if self.at_ident("has") {
            self.builder
                .start_node(ExprSyntaxKind::CALL_EXPR.into());
            self.bump(); // 'has'
            self.expect(ExprSyntaxKind::L_PAREN);
            self.builder.start_node(ExprSyntaxKind::LITERAL.into());
            self.expect(ExprSyntaxKind::STRING_LIT);
            self.builder.finish_node();
            self.expect(ExprSyntaxKind::R_PAREN);
            self.builder.finish_node();
            return;
        }

        // connected
        if self.at_ident("connected") {
            self.builder
                .start_node(ExprSyntaxKind::IDENT_EXPR.into());
            self.bump();
            self.builder.finish_node();
            return;
        }

        // message.contains('text')
        if self.at_ident("message") && self.peek_kind(1) == Some(ExprSyntaxKind::DOT) {
            self.builder
                .start_node(ExprSyntaxKind::CONTAINS_EXPR.into());
            self.bump(); // 'message'
            self.expect(ExprSyntaxKind::DOT);
            self.expect_ident("contains");
            self.expect(ExprSyntaxKind::L_PAREN);
            self.builder.start_node(ExprSyntaxKind::LITERAL.into());
            self.expect(ExprSyntaxKind::STRING_LIT);
            self.builder.finish_node();
            self.expect(ExprSyntaxKind::R_PAREN);
            self.builder.finish_node();
            return;
        }

        // field == 'value'   (category, kind, direction, severity, message)
        if self.at_ident("category")
            || self.at_ident("kind")
            || self.at_ident("direction")
            || self.at_ident("severity")
            || self.at_ident("message")
        {
            self.builder
                .start_node(ExprSyntaxKind::COMPARE_EXPR.into());
            self.bump(); // field name
            self.expect(ExprSyntaxKind::EQ_EQ);
            self.builder.start_node(ExprSyntaxKind::LITERAL.into());
            self.expect(ExprSyntaxKind::STRING_LIT);
            self.builder.finish_node();
            self.builder.finish_node();
            return;
        }

        // Error
        let text = self.current_text().to_string();
        self.errors.push(format!(
            "expected 'not', 'has', 'connected', 'category', 'kind', 'direction', 'severity', 'message', or '(', found '{}'",
            if text.is_empty() { "EOF" } else { &text }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertion::syntax::ExprSyntaxKind;

    #[test]
    fn parse_components() {
        let result = parse_expr("components");
        assert!(result.ok(), "errors: {:?}", result.errors());
        let root = result.syntax_node();
        assert_eq!(root.kind(), ExprSyntaxKind::ROOT);
        let pipeline = root.children().next().unwrap();
        assert_eq!(pipeline.kind(), ExprSyntaxKind::PIPELINE_EXPR);
        let source = pipeline.children().next().unwrap();
        assert_eq!(source.kind(), ExprSyntaxKind::IDENT_EXPR);
        assert_eq!(source.text().to_string(), "components");
    }

    #[test]
    fn parse_analysis() {
        let result = parse_expr("analysis('scheduling')");
        assert!(result.ok(), "errors: {:?}", result.errors());
        let root = result.syntax_node();
        let pipeline = root.children().next().unwrap();
        let source = pipeline.children().next().unwrap();
        assert_eq!(source.kind(), ExprSyntaxKind::CALL_EXPR);
    }

    #[test]
    fn parse_where_clause() {
        let result = parse_expr("components.where(category == 'thread')");
        assert!(result.ok(), "errors: {:?}", result.errors());
        let root = result.syntax_node();
        let pipeline = root.children().next().unwrap();
        let children: Vec<_> = pipeline.children().collect();
        assert_eq!(children.len(), 2); // IDENT_EXPR + DOT_CALL
        assert_eq!(children[1].kind(), ExprSyntaxKind::DOT_CALL);
    }

    #[test]
    fn parse_and_expr_test() {
        let result = parse_expr("components.all(has('A') and has('B'))");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_or_expr_test() {
        let result = parse_expr(
            "components.any(category == 'thread' or category == 'process')",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_not_expr_test() {
        let result = parse_expr(
            "components.none(not has('Timing_Properties::Period'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_message_contains() {
        let result = parse_expr(
            "analysis('scheduling').diagnostics.none(message.contains('exceeds'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_parenthesized() {
        let result = parse_expr(
            "components.all((category == 'thread' or category == 'process') and has('P'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_error_empty() {
        let result = parse_expr("");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected"));
    }

    #[test]
    fn parse_error_bad_source() {
        let result = parse_expr("foobar");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected"));
    }

    #[test]
    fn parse_error_unterminated_string() {
        let result = parse_expr("components.where(category == 'thread)");
        assert!(!result.ok());
    }

    #[test]
    fn parse_error_trailing_text() {
        let result = parse_expr("components foobar");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("unexpected"));
    }

    #[test]
    fn parse_error_bad_method() {
        let result = parse_expr("components.foobar()");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected method name"));
    }
}
