//! Parser for WAC (WebAssembly Compositions) syntax.
//!
//! This parser handles the subset of WAC needed for AADL interop:
//! package declarations, `let` instantiation bindings, imports, and exports.

// ── AST types ──────────────────────────────────────────────────────

/// A parsed WAC document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WacDocument {
    pub package: Option<WacPackage>,
    pub statements: Vec<WacStatement>,
}

/// A WAC package declaration: `package ns:name;` or `package ns:name@version;`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WacPackage {
    pub namespace: String,
    pub name: String,
    pub version: Option<String>,
}

/// A top-level WAC statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WacStatement {
    /// `import name: iface-path;`
    Import {
        name: String,
        interface_path: String,
    },
    /// `let name = new pkg:comp { args };`
    Let {
        name: String,
        component_path: String,
        args: Vec<WacArg>,
    },
    /// `export expr;`
    Export { expr: WacExpr },
}

/// An argument inside a `let` instantiation block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WacArg {
    /// `name: value.field`
    Named { key: String, value: WacExpr },
    /// `...` (implicit pass-through)
    Spread,
    /// `...name` (spread from named component)
    SpreadFrom(String),
}

/// A WAC expression (name reference or field access).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WacExpr {
    /// Simple name reference.
    Name(String),
    /// Field access: `base.field`
    Access { base: String, field: String },
}

// ── Parser ─────────────────────────────────────────────────────────

/// Parse a WAC source string into a `WacDocument`.
///
/// The parser is lenient: it skips constructs it doesn't understand
/// rather than failing hard.
pub fn parse_wac(source: &str) -> Result<WacDocument, Vec<String>> {
    let mut parser = Parser::new(source);
    parser.parse_document()
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
    errors: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            errors: Vec::new(),
        }
    }

    // ── Helpers ────────────────────────────────────────────────────

    fn remaining(&self) -> &'a str {
        &self.source[self.pos..]
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            let before = self.pos;
            // Skip whitespace
            while self.pos < self.source.len() {
                let b = self.source.as_bytes()[self.pos];
                if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            // Skip line comments
            if self.remaining().starts_with("//") {
                while self.pos < self.source.len() && self.source.as_bytes()[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            // Skip block comments
            if self.remaining().starts_with("/*") {
                self.pos += 2;
                let mut depth = 1;
                while self.pos + 1 < self.source.len() && depth > 0 {
                    if &self.source[self.pos..self.pos + 2] == "/*" {
                        depth += 1;
                        self.pos += 2;
                    } else if &self.source[self.pos..self.pos + 2] == "*/" {
                        depth -= 1;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                continue;
            }
            if self.pos == before {
                break;
            }
        }
    }

    fn expect_char(&mut self, ch: char) -> bool {
        self.skip_ws_and_comments();
        if self.remaining().starts_with(ch) {
            self.pos += ch.len_utf8();
            true
        } else {
            self.errors
                .push(format!("expected '{}' at position {}", ch, self.pos));
            false
        }
    }

    fn eat_char(&mut self, ch: char) -> bool {
        self.skip_ws_and_comments();
        if self.remaining().starts_with(ch) {
            self.pos += ch.len_utf8();
            true
        } else {
            false
        }
    }

    /// Try to consume a keyword. Returns true if consumed.
    fn eat_keyword(&mut self, kw: &str) -> bool {
        self.skip_ws_and_comments();
        let rem = self.remaining();
        if rem.starts_with(kw) {
            let after = &rem[kw.len()..];
            if after.is_empty() || !is_ident_continue(after.as_bytes()[0]) {
                self.pos += kw.len();
                return true;
            }
        }
        false
    }

    /// Parse an identifier (kebab-case: [a-z][a-z0-9-_]*)
    fn parse_ident(&mut self) -> Option<String> {
        self.skip_ws_and_comments();
        let start = self.pos;
        while self.pos < self.source.len() && is_ident_continue(self.source.as_bytes()[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(self.source[start..self.pos].to_string())
        }
    }

    /// Parse a possibly-qualified path: `ns:pkg/name@version` or `ns:name`.
    fn parse_use_path(&mut self) -> Option<String> {
        self.skip_ws_and_comments();
        let start = self.pos;
        while self.pos < self.source.len() {
            let b = self.source.as_bytes()[self.pos];
            if b.is_ascii_alphanumeric()
                || b == b'-'
                || b == b'_'
                || b == b':'
                || b == b'/'
                || b == b'.'
                || b == b'@'
            {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            Some(self.source[start..self.pos].to_string())
        }
    }

    /// Skip to after the next occurrence of `ch`.
    fn skip_past(&mut self, ch: char) {
        while self.pos < self.source.len() {
            if self.source.as_bytes()[self.pos] == ch as u8 {
                self.pos += 1;
                return;
            }
            self.pos += 1;
        }
    }

    // ── Top-level parsing ──────────────────────────────────────────

    fn parse_document(&mut self) -> Result<WacDocument, Vec<String>> {
        let mut doc = WacDocument {
            package: None,
            statements: Vec::new(),
        };

        loop {
            self.skip_ws_and_comments();
            if self.is_eof() {
                break;
            }

            if self.eat_keyword("package") {
                doc.package = self.parse_package_decl();
            } else if self.eat_keyword("let") {
                if let Some(stmt) = self.parse_let() {
                    doc.statements.push(stmt);
                }
            } else if self.eat_keyword("import") {
                if let Some(stmt) = self.parse_import() {
                    doc.statements.push(stmt);
                }
            } else if self.eat_keyword("export") {
                if let Some(stmt) = self.parse_export() {
                    doc.statements.push(stmt);
                }
            } else {
                // Skip unknown token
                self.pos += 1;
            }
        }

        if !self.errors.is_empty() {
            // Lenient: still return the document
        }
        Ok(doc)
    }

    fn parse_package_decl(&mut self) -> Option<WacPackage> {
        let path = self.parse_use_path()?;
        self.eat_char(';');

        // Split on ':'
        let (ns_part, rest) = if let Some(idx) = path.find(':') {
            (&path[..idx], &path[idx + 1..])
        } else {
            return Some(WacPackage {
                namespace: String::new(),
                name: path,
                version: None,
            });
        };

        // Split rest on '@' for version
        let (name_part, version) = if let Some(idx) = rest.find('@') {
            (&rest[..idx], Some(rest[idx + 1..].to_string()))
        } else {
            (rest, None)
        };

        Some(WacPackage {
            namespace: ns_part.to_string(),
            name: name_part.to_string(),
            version,
        })
    }

    fn parse_let(&mut self) -> Option<WacStatement> {
        // let name = new pkg:comp { args };
        // let name = new pkg:comp;
        let name = self.parse_ident()?;

        if !self.expect_char('=') {
            self.skip_past(';');
            return None;
        }

        if !self.eat_keyword("new") {
            self.skip_past(';');
            return None;
        }

        let component_path = self.parse_use_path()?;
        let mut args = Vec::new();

        self.skip_ws_and_comments();
        if self.eat_char('{') {
            args = self.parse_args();
            self.expect_char('}');
        }

        self.eat_char(';');

        Some(WacStatement::Let {
            name,
            component_path,
            args,
        })
    }

    fn parse_args(&mut self) -> Vec<WacArg> {
        let mut args = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.remaining().starts_with('}') {
                break;
            }
            if self.is_eof() {
                break;
            }

            // Check for spread: `...` or `...name`
            if self.remaining().starts_with("...") {
                self.pos += 3;
                self.skip_ws_and_comments();
                // Check if followed by an identifier (spread-from) or comma/brace (plain spread)
                let saved = self.pos;
                if let Some(spread_name) = self.parse_ident() {
                    // Make sure it's not a comma or brace (which means plain spread)
                    args.push(WacArg::SpreadFrom(spread_name));
                } else {
                    self.pos = saved;
                    args.push(WacArg::Spread);
                }
                self.eat_char(',');
                continue;
            }

            // Named argument: key: value
            if let Some(key) = self.parse_ident() {
                if self.eat_char(':') {
                    let value = self.parse_expr();
                    args.push(WacArg::Named { key, value });
                } else {
                    // Bare name, treat as named with name reference
                    args.push(WacArg::Named {
                        key: key.clone(),
                        value: WacExpr::Name(key),
                    });
                }
                self.eat_char(',');
            } else {
                // Skip unknown
                self.pos += 1;
            }
        }

        args
    }

    fn parse_expr(&mut self) -> WacExpr {
        self.skip_ws_and_comments();
        if let Some(name) = self.parse_ident() {
            // Check for field access: name.field
            if self.eat_char('.') {
                if let Some(field) = self.parse_ident() {
                    return WacExpr::Access { base: name, field };
                }
                // Dot with no field — treat as plain name
                return WacExpr::Name(name);
            }
            WacExpr::Name(name)
        } else {
            // Fallback: try a use-path as a name
            if let Some(path) = self.parse_use_path() {
                WacExpr::Name(path)
            } else {
                WacExpr::Name("unknown".into())
            }
        }
    }

    fn parse_import(&mut self) -> Option<WacStatement> {
        // import name: pkg:iface/path;
        let name = self.parse_ident()?;

        if !self.expect_char(':') {
            self.skip_past(';');
            return None;
        }

        let interface_path = self.parse_use_path()?;
        self.eat_char(';');

        Some(WacStatement::Import {
            name,
            interface_path,
        })
    }

    fn parse_export(&mut self) -> Option<WacStatement> {
        // export expr;
        let expr = self.parse_expr();
        self.eat_char(';');
        Some(WacStatement::Export { expr })
    }
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_composition() {
        let src = r#"
            package example:my-app;
            let backend = new example:backend;
            export backend.api;
        "#;
        let doc = parse_wac(src).unwrap();
        assert_eq!(doc.package.as_ref().unwrap().namespace, "example");
        assert_eq!(doc.package.as_ref().unwrap().name, "my-app");
        assert_eq!(doc.statements.len(), 2);
    }

    #[test]
    fn parse_let_with_args() {
        let src = r#"
            package example:composed;
            let a = new example:component-a;
            let b = new example:component-b {
                data: a.output,
                ...
            };
            export b.result;
        "#;
        let doc = parse_wac(src).unwrap();
        assert_eq!(doc.statements.len(), 3); // 2 lets + 1 export

        match &doc.statements[1] {
            WacStatement::Let { name, args, .. } => {
                assert_eq!(name, "b");
                assert_eq!(args.len(), 2);
                match &args[0] {
                    WacArg::Named { key, value } => {
                        assert_eq!(key, "data");
                        assert_eq!(
                            *value,
                            WacExpr::Access {
                                base: "a".into(),
                                field: "output".into()
                            }
                        );
                    }
                    _ => panic!("expected named arg"),
                }
                assert_eq!(args[1], WacArg::Spread);
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn parse_import() {
        let src = r#"
            package test:app;
            import config: test:config/settings;
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[0] {
            WacStatement::Import {
                name,
                interface_path,
            } => {
                assert_eq!(name, "config");
                assert_eq!(interface_path, "test:config/settings");
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn parse_access_expression() {
        let src = r#"
            package test:app;
            let x = new test:comp;
            export x.api;
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[1] {
            WacStatement::Export {
                expr: WacExpr::Access { base, field },
            } => {
                assert_eq!(base, "x");
                assert_eq!(field, "api");
            }
            _ => panic!("expected access export"),
        }
    }

    #[test]
    fn parse_package_with_version() {
        let src = "package example:my-comp@1.0.0;";
        let doc = parse_wac(src).unwrap();
        let pkg = doc.package.unwrap();
        assert_eq!(pkg.namespace, "example");
        assert_eq!(pkg.name, "my-comp");
        assert_eq!(pkg.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn parse_let_no_args() {
        let src = r#"
            package test:app;
            let x = new test:comp;
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[0] {
            WacStatement::Let {
                name,
                component_path,
                args,
            } => {
                assert_eq!(name, "x");
                assert_eq!(component_path, "test:comp");
                assert!(args.is_empty());
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn parse_spread_from() {
        let src = r#"
            package test:app;
            let a = new test:comp-a;
            let b = new test:comp-b {
                ...a
            };
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[1] {
            WacStatement::Let { args, .. } => {
                assert_eq!(args.len(), 1);
                assert_eq!(args[0], WacArg::SpreadFrom("a".into()));
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn parse_empty_document() {
        let doc = parse_wac("").unwrap();
        assert!(doc.package.is_none());
        assert!(doc.statements.is_empty());
    }

    #[test]
    fn parse_comments() {
        let src = r#"
            // This is a composition
            package test:app;
            // Instantiate backend
            let backend = new test:backend;
            /* Export the API */
            export backend.api;
        "#;
        let doc = parse_wac(src).unwrap();
        assert!(doc.package.is_some());
        assert_eq!(doc.statements.len(), 2);
    }

    #[test]
    fn parse_export_simple_name() {
        let src = r#"
            package test:app;
            let x = new test:comp;
            export x;
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[1] {
            WacStatement::Export {
                expr: WacExpr::Name(n),
            } => {
                assert_eq!(n, "x");
            }
            _ => panic!("expected name export"),
        }
    }

    #[test]
    fn parse_multiple_named_args() {
        let src = r#"
            package test:composed;
            let a = new test:alpha;
            let b = new test:beta;
            let c = new test:gamma {
                input1: a.out,
                input2: b.out,
                ...
            };
        "#;
        let doc = parse_wac(src).unwrap();
        match &doc.statements[2] {
            WacStatement::Let { name, args, .. } => {
                assert_eq!(name, "c");
                assert_eq!(args.len(), 3);
                match &args[0] {
                    WacArg::Named { key, value } => {
                        assert_eq!(key, "input1");
                        assert_eq!(
                            *value,
                            WacExpr::Access {
                                base: "a".into(),
                                field: "out".into()
                            }
                        );
                    }
                    _ => panic!("expected named arg"),
                }
                match &args[1] {
                    WacArg::Named { key, value } => {
                        assert_eq!(key, "input2");
                        assert_eq!(
                            *value,
                            WacExpr::Access {
                                base: "b".into(),
                                field: "out".into()
                            }
                        );
                    }
                    _ => panic!("expected named arg"),
                }
                assert_eq!(args[2], WacArg::Spread);
            }
            _ => panic!("expected let"),
        }
    }
}
