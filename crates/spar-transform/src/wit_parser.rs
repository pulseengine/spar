//! A simple hand-written parser for WIT (WebAssembly Interface Type) syntax.
//!
//! This parser handles the subset of WIT needed for AADL interop:
//! packages, interfaces with functions and type definitions, and worlds
//! with imports and exports.

// ── AST types ──────────────────────────────────────────────────────

/// A parsed WIT document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitDocument {
    pub package: Option<WitPackage>,
    pub worlds: Vec<WitWorld>,
    pub interfaces: Vec<WitInterface>,
}

/// A WIT package declaration: `package ns:name@version;`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitPackage {
    pub namespace: String,
    pub name: String,
    pub version: Option<String>,
}

/// A WIT world declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitWorld {
    pub name: String,
    pub imports: Vec<WitWorldItem>,
    pub exports: Vec<WitWorldItem>,
}

/// An item inside a world block (import or export).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitWorldItem {
    /// A named interface reference (possibly qualified).
    Interface(String),
    /// An inline function definition.
    Function(WitFunction),
    /// A type import/export.
    Type(String),
}

/// A WIT interface declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitInterface {
    pub name: String,
    pub functions: Vec<WitFunction>,
    pub types: Vec<WitTypeDef>,
}

/// A WIT function signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitFunction {
    pub name: String,
    pub params: Vec<(String, WitType)>,
    pub result: Option<WitType>,
    pub is_async: bool,
}

/// A WIT type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String_,
    List(Box<WitType>),
    Option_(Box<WitType>),
    Result {
        ok: Option<Box<WitType>>,
        err: Option<Box<WitType>>,
    },
    Tuple(Vec<WitType>),
    Stream(Box<WitType>),
    Future(Box<WitType>),
    Named(String),
}

/// A WIT type definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitTypeDef {
    Record {
        name: String,
        fields: Vec<(String, WitType)>,
    },
    Enum {
        name: String,
        cases: Vec<String>,
    },
    Variant {
        name: String,
        cases: Vec<(String, Option<WitType>)>,
    },
    Flags {
        name: String,
        flags: Vec<String>,
    },
    TypeAlias {
        name: String,
        target: WitType,
    },
    Resource {
        name: String,
    },
}

// ── Parser ─────────────────────────────────────────────────────────

/// Parse a WIT source string into a `WitDocument`.
///
/// The parser is lenient: it skips constructs it doesn't understand
/// rather than failing hard.
pub fn parse_wit(source: &str) -> Result<WitDocument, Vec<String>> {
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
            // Skip whitespace
            let before = self.pos;
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

    fn peek_char(&mut self) -> Option<char> {
        self.skip_ws_and_comments();
        self.remaining().chars().next()
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
        if let Some(after) = rem.strip_prefix(kw)
            && (after.is_empty() || !is_ident_continue(after.as_bytes()[0]))
        {
            // Whole word match (next char is not alphanumeric or hyphen)
            self.pos += kw.len();
            return true;
        }
        false
    }

    /// Parse a WIT identifier (kebab-case: [a-z][a-z0-9-]*)
    /// Also accepts uppercase for flexibility.
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

    /// Parse a possibly-qualified WIT name: `ns:pkg/name@version` or just `name`.
    fn parse_use_path(&mut self) -> Option<String> {
        self.skip_ws_and_comments();
        let start = self.pos;
        // Consume characters that form a use-path: [a-zA-Z0-9_:/.@-]
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

    /// Skip a balanced brace block `{ ... }`, including the braces.
    fn skip_braces(&mut self) {
        if !self.eat_char('{') {
            return;
        }
        let mut depth = 1u32;
        while self.pos < self.source.len() && depth > 0 {
            match self.source.as_bytes()[self.pos] {
                b'{' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    depth -= 1;
                    self.pos += 1;
                }
                b'/' if self.pos + 1 < self.source.len()
                    && self.source.as_bytes()[self.pos + 1] == b'/' =>
                {
                    // Skip line comment
                    while self.pos < self.source.len() && self.source.as_bytes()[self.pos] != b'\n'
                    {
                        self.pos += 1;
                    }
                }
                _ => {
                    self.pos += 1;
                }
            }
        }
    }

    // ── Top-level parsing ──────────────────────────────────────────

    fn parse_document(&mut self) -> Result<WitDocument, Vec<String>> {
        let mut doc = WitDocument {
            package: None,
            worlds: Vec::new(),
            interfaces: Vec::new(),
        };

        loop {
            self.skip_ws_and_comments();
            if self.is_eof() {
                break;
            }

            if self.eat_keyword("package") {
                doc.package = self.parse_package_decl();
            } else if self.eat_keyword("world") {
                if let Some(w) = self.parse_world() {
                    doc.worlds.push(w);
                }
            } else if self.eat_keyword("interface") {
                if let Some(iface) = self.parse_interface() {
                    doc.interfaces.push(iface);
                }
            } else if self.eat_keyword("use") {
                // Top-level use statement — skip it
                self.skip_past(';');
            } else {
                // Skip unknown token
                self.pos += 1;
            }
        }

        if self.errors.is_empty() {
            Ok(doc)
        } else {
            // Return document even with errors (lenient)
            Ok(doc)
        }
    }

    fn parse_package_decl(&mut self) -> Option<WitPackage> {
        // package ns:name@version;
        // or: package ns:name;
        let path = self.parse_use_path()?;
        self.eat_char(';');

        // Split on ':'
        let (ns_part, rest) = if let Some(idx) = path.find(':') {
            (&path[..idx], &path[idx + 1..])
        } else {
            // No namespace — use entire thing as name
            return Some(WitPackage {
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

        Some(WitPackage {
            namespace: ns_part.to_string(),
            name: name_part.to_string(),
            version,
        })
    }

    fn parse_world(&mut self) -> Option<WitWorld> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }

        let mut imports = Vec::new();
        let mut exports = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                self.errors.push("unexpected EOF in world block".into());
                break;
            }

            if self.eat_keyword("import") {
                if let Some(item) = self.parse_world_item() {
                    imports.push(item);
                }
            } else if self.eat_keyword("export") {
                if let Some(item) = self.parse_world_item() {
                    exports.push(item);
                }
            } else if self.eat_keyword("use") {
                self.skip_past(';');
            } else if self.eat_keyword("include") {
                // include SomeWorld;
                self.skip_past(';');
            } else {
                // Try to parse as inline type definition (skip)
                self.pos += 1;
            }
        }

        Some(WitWorld {
            name,
            imports,
            exports,
        })
    }

    fn parse_world_item(&mut self) -> Option<WitWorldItem> {
        // Could be:
        //   import iface-name;
        //   import ns:pkg/iface@ver;
        //   import func-name: func(...) -> ...;
        //   import type-name: type;
        self.skip_ws_and_comments();

        let saved_pos = self.pos;

        // First, try parsing a simple identifier (stops at `:`)
        // to detect inline definitions like `run: func(...)`.
        if let Some(ident) = self.parse_ident() {
            self.skip_ws_and_comments();

            // Check if next char is `:` — could be inline def OR qualified path
            if self.remaining().starts_with(':') {
                // Peek ahead: is this `ident: func` or `ident: interface`?
                // Or is it `ns:pkg/...` (qualified path)?
                self.pos += 1; // skip ':'
                self.skip_ws_and_comments();

                let is_async = self.eat_keyword("async");
                if self.eat_keyword("func") {
                    let mut func = self.parse_func_signature(&ident)?;
                    func.is_async = is_async;
                    self.eat_char(';');
                    return Some(WitWorldItem::Function(func));
                }
                if self.eat_keyword("interface") {
                    self.skip_braces();
                    return Some(WitWorldItem::Interface(ident));
                }

                // Not func/interface — this is a qualified use-path.
                // Restore to before the ident and parse as use-path.
                self.pos = saved_pos;
            } else if self.eat_char(';') {
                // Simple unqualified interface reference: `import foo;`
                return Some(WitWorldItem::Interface(ident));
            } else {
                // Something unexpected — restore
                self.pos = saved_pos;
            }
        } else {
            self.pos = saved_pos;
        }

        // Try as a (possibly qualified) use-path: `ns:pkg/iface@ver`
        if let Some(path) = self.parse_use_path() {
            self.skip_ws_and_comments();
            if self.eat_char(';') {
                return Some(WitWorldItem::Interface(path));
            }
            // No semicolon — skip to next one
            self.skip_past(';');
            return Some(WitWorldItem::Interface(path));
        }

        // Fallback: restore and skip
        self.pos = saved_pos;
        self.skip_past(';');
        None
    }

    fn parse_interface(&mut self) -> Option<WitInterface> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }

        let mut functions = Vec::new();
        let mut types = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                self.errors.push("unexpected EOF in interface block".into());
                break;
            }

            if self.eat_keyword("record") {
                if let Some(td) = self.parse_record() {
                    types.push(td);
                }
            } else if self.eat_keyword("enum") {
                if let Some(td) = self.parse_enum() {
                    types.push(td);
                }
            } else if self.eat_keyword("variant") {
                if let Some(td) = self.parse_variant() {
                    types.push(td);
                }
            } else if self.eat_keyword("flags") {
                if let Some(td) = self.parse_flags() {
                    types.push(td);
                }
            } else if self.eat_keyword("type") {
                if let Some(td) = self.parse_type_alias() {
                    types.push(td);
                }
            } else if self.eat_keyword("resource") {
                if let Some(td) = self.parse_resource() {
                    types.push(td);
                }
            } else if self.eat_keyword("use") {
                // use some-interface.{type1, type2};
                self.skip_past(';');
            } else {
                // Try to parse as function: name: func(...)
                if let Some(func) = self.try_parse_function() {
                    functions.push(func);
                } else {
                    // Skip unknown
                    self.pos += 1;
                }
            }
        }

        Some(WitInterface {
            name,
            functions,
            types,
        })
    }

    fn try_parse_function(&mut self) -> Option<WitFunction> {
        let saved = self.pos;
        let name = self.parse_ident()?;
        if !self.eat_char(':') {
            self.pos = saved;
            return None;
        }
        self.skip_ws_and_comments();
        let is_async = self.eat_keyword("async");
        if !self.eat_keyword("func") {
            // Not a function — could be something else. Skip to semicolon.
            self.pos = saved;
            return None;
        }
        let mut func = self.parse_func_signature(&name)?;
        func.is_async = is_async;
        self.eat_char(';');
        Some(func)
    }

    fn parse_func_signature(&mut self, name: &str) -> Option<WitFunction> {
        // func(params) -> result
        if !self.expect_char('(') {
            return None;
        }

        let params = self.parse_param_list();

        if !self.expect_char(')') {
            return None;
        }

        let result = if self.eat_char('-') {
            // consume the '>'
            self.eat_char('>');
            self.skip_ws_and_comments();
            Some(self.parse_type())
        } else {
            None
        };

        Some(WitFunction {
            name: name.to_string(),
            params,
            result,
            is_async: false,
        })
    }

    fn parse_param_list(&mut self) -> Vec<(String, WitType)> {
        let mut params = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.remaining().starts_with(')') {
                break;
            }
            if let Some(name) = self.parse_ident() {
                if self.eat_char(':') {
                    let ty = self.parse_type();
                    params.push((name, ty));
                }
            } else {
                break;
            }
            self.eat_char(',');
        }
        params
    }

    // ── Type parsing ───────────────────────────────────────────────

    fn parse_type(&mut self) -> WitType {
        self.skip_ws_and_comments();

        // Check for parameterized types
        if self.eat_keyword("list") {
            return self.parse_generic_one(|inner| WitType::List(Box::new(inner)));
        }
        if self.eat_keyword("option") {
            return self.parse_generic_one(|inner| WitType::Option_(Box::new(inner)));
        }
        if self.eat_keyword("result") {
            return self.parse_result_type();
        }
        if self.eat_keyword("tuple") {
            return self.parse_tuple_type();
        }
        if self.eat_keyword("stream") {
            return self.parse_generic_one(|inner| WitType::Stream(Box::new(inner)));
        }
        if self.eat_keyword("future") {
            return self.parse_generic_one(|inner| WitType::Future(Box::new(inner)));
        }

        // Primitive types
        if self.eat_keyword("bool") {
            return WitType::Bool;
        }
        if self.eat_keyword("u8") {
            return WitType::U8;
        }
        if self.eat_keyword("u16") {
            return WitType::U16;
        }
        if self.eat_keyword("u32") {
            return WitType::U32;
        }
        if self.eat_keyword("u64") {
            return WitType::U64;
        }
        if self.eat_keyword("s8") {
            return WitType::S8;
        }
        if self.eat_keyword("s16") {
            return WitType::S16;
        }
        if self.eat_keyword("s32") {
            return WitType::S32;
        }
        if self.eat_keyword("s64") {
            return WitType::S64;
        }
        if self.eat_keyword("f32") {
            return WitType::F32;
        }
        if self.eat_keyword("f64") {
            return WitType::F64;
        }
        if self.eat_keyword("char") {
            return WitType::Char;
        }
        if self.eat_keyword("string") {
            return WitType::String_;
        }

        // Named type reference
        if let Some(name) = self.parse_ident() {
            return WitType::Named(name);
        }

        // Wildcard `_`
        if self.eat_char('_') {
            return WitType::Named("_".into());
        }

        self.errors
            .push(format!("expected type at position {}", self.pos));
        WitType::Named("unknown".into())
    }

    fn parse_generic_one<F>(&mut self, ctor: F) -> WitType
    where
        F: FnOnce(WitType) -> WitType,
    {
        if self.eat_char('<') {
            let inner = self.parse_type();
            self.expect_char('>');
            ctor(inner)
        } else {
            // Bare list/option without angle brackets — treat as named
            WitType::Named("unknown".into())
        }
    }

    fn parse_result_type(&mut self) -> WitType {
        if !self.eat_char('<') {
            // `result` without type params — result<_, _>
            return WitType::Result {
                ok: None,
                err: None,
            };
        }

        // Parse ok type (could be `_`)
        self.skip_ws_and_comments();
        let ok = if self.remaining().starts_with('_') {
            self.pos += 1;
            None
        } else if self.remaining().starts_with(',') || self.remaining().starts_with('>') {
            None
        } else {
            Some(Box::new(self.parse_type()))
        };

        let err = if self.eat_char(',') {
            self.skip_ws_and_comments();
            if self.remaining().starts_with('_') {
                self.pos += 1;
                None
            } else if self.remaining().starts_with('>') {
                None
            } else {
                Some(Box::new(self.parse_type()))
            }
        } else {
            None
        };

        self.expect_char('>');
        WitType::Result { ok, err }
    }

    fn parse_tuple_type(&mut self) -> WitType {
        if !self.eat_char('<') {
            return WitType::Tuple(Vec::new());
        }
        let mut types = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.remaining().starts_with('>') {
                break;
            }
            types.push(self.parse_type());
            if !self.eat_char(',') {
                break;
            }
        }
        self.expect_char('>');
        WitType::Tuple(types)
    }

    // ── Type definition parsing ────────────────────────────────────

    fn parse_record(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }
        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                break;
            }
            if let Some(fname) = self.parse_ident()
                && self.eat_char(':')
            {
                let ty = self.parse_type();
                fields.push((fname, ty));
            }
            self.eat_char(',');
        }
        Some(WitTypeDef::Record { name, fields })
    }

    fn parse_enum(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }
        let mut cases = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                break;
            }
            if let Some(case_name) = self.parse_ident() {
                cases.push(case_name);
            }
            self.eat_char(',');
        }
        Some(WitTypeDef::Enum { name, cases })
    }

    fn parse_variant(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }
        let mut cases = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                break;
            }
            if let Some(case_name) = self.parse_ident() {
                let payload = if self.eat_char('(') {
                    let ty = self.parse_type();
                    self.expect_char(')');
                    Some(ty)
                } else {
                    None
                };
                cases.push((case_name, payload));
            }
            self.eat_char(',');
        }
        Some(WitTypeDef::Variant { name, cases })
    }

    fn parse_flags(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        if !self.expect_char('{') {
            return None;
        }
        let mut flags = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.eat_char('}') {
                break;
            }
            if self.is_eof() {
                break;
            }
            if let Some(flag_name) = self.parse_ident() {
                flags.push(flag_name);
            }
            self.eat_char(',');
        }
        Some(WitTypeDef::Flags { name, flags })
    }

    fn parse_type_alias(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        if !self.eat_char('=') {
            self.skip_past(';');
            return None;
        }
        let target = self.parse_type();
        self.eat_char(';');
        Some(WitTypeDef::TypeAlias { name, target })
    }

    fn parse_resource(&mut self) -> Option<WitTypeDef> {
        let name = self.parse_ident()?;
        // Resource may have a body or just a semicolon
        self.skip_ws_and_comments();
        if self.peek_char() == Some('{') {
            self.skip_braces();
        } else {
            self.eat_char(';');
        }
        Some(WitTypeDef::Resource { name })
    }
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

// ── Name conversion utilities ──────────────────────────────────────

/// Convert a kebab-case WIT name to AADL-compatible PascalCase.
pub fn kebab_to_pascal(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let mut s = c.to_uppercase().to_string();
                    s.extend(chars);
                    s
                }
                None => String::new(),
            }
        })
        .collect()
}

/// Convert a kebab-case WIT name to AADL-compatible snake_case.
pub fn kebab_to_snake(name: &str) -> String {
    name.replace('-', "_")
}

/// Convert an AADL PascalCase or snake_case name to WIT kebab-case.
pub fn to_kebab_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' {
            result.push('-');
            prev_lower = false;
        } else if ch.is_uppercase() {
            if prev_lower {
                result.push('-');
            }
            result.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_lower = false;
        } else {
            result.push(ch);
            prev_lower = ch.is_lowercase();
        }
    }
    result
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_package() {
        let doc = parse_wit("package example:sensors@1.0.0;").unwrap();
        let pkg = doc.package.unwrap();
        assert_eq!(pkg.namespace, "example");
        assert_eq!(pkg.name, "sensors");
        assert_eq!(pkg.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn parse_package_no_version() {
        let doc = parse_wit("package wasi:io;").unwrap();
        let pkg = doc.package.unwrap();
        assert_eq!(pkg.namespace, "wasi");
        assert_eq!(pkg.name, "io");
        assert_eq!(pkg.version, None);
    }

    #[test]
    fn parse_empty_interface() {
        let doc = parse_wit("interface empty {}").unwrap();
        assert_eq!(doc.interfaces.len(), 1);
        assert_eq!(doc.interfaces[0].name, "empty");
        assert!(doc.interfaces[0].functions.is_empty());
        assert!(doc.interfaces[0].types.is_empty());
    }

    #[test]
    fn parse_interface_with_function() {
        let src = r#"
            interface greet {
                greet: func(name: string) -> string;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.interfaces.len(), 1);
        assert_eq!(doc.interfaces[0].functions.len(), 1);
        let f = &doc.interfaces[0].functions[0];
        assert_eq!(f.name, "greet");
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].0, "name");
        assert_eq!(f.params[0].1, WitType::String_);
        assert_eq!(f.result, Some(WitType::String_));
    }

    #[test]
    fn parse_record() {
        let src = r#"
            interface data {
                record point {
                    x: f64,
                    y: f64,
                    z: f64,
                }
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.interfaces[0].types.len(), 1);
        match &doc.interfaces[0].types[0] {
            WitTypeDef::Record { name, fields } => {
                assert_eq!(name, "point");
                assert_eq!(fields.len(), 3);
                assert_eq!(fields[0].0, "x");
                assert_eq!(fields[0].1, WitType::F64);
            }
            other => panic!("expected Record, got {:?}", other),
        }
    }

    #[test]
    fn parse_enum() {
        let src = r#"
            interface types {
                enum color {
                    red,
                    green,
                    blue,
                }
            }
        "#;
        let doc = parse_wit(src).unwrap();
        match &doc.interfaces[0].types[0] {
            WitTypeDef::Enum { name, cases } => {
                assert_eq!(name, "color");
                assert_eq!(cases, &["red", "green", "blue"]);
            }
            other => panic!("expected Enum, got {:?}", other),
        }
    }

    #[test]
    fn parse_variant() {
        let src = r#"
            interface types {
                variant filter {
                    all,
                    none,
                    some(list<string>),
                }
            }
        "#;
        let doc = parse_wit(src).unwrap();
        match &doc.interfaces[0].types[0] {
            WitTypeDef::Variant { name, cases } => {
                assert_eq!(name, "filter");
                assert_eq!(cases.len(), 3);
                assert_eq!(cases[0], ("all".into(), None));
                assert_eq!(cases[1], ("none".into(), None));
                assert_eq!(
                    cases[2],
                    (
                        "some".into(),
                        Some(WitType::List(Box::new(WitType::String_)))
                    )
                );
            }
            other => panic!("expected Variant, got {:?}", other),
        }
    }

    #[test]
    fn parse_flags() {
        let src = r#"
            interface perms {
                flags permissions {
                    read,
                    write,
                    exec,
                }
            }
        "#;
        let doc = parse_wit(src).unwrap();
        match &doc.interfaces[0].types[0] {
            WitTypeDef::Flags { name, flags } => {
                assert_eq!(name, "permissions");
                assert_eq!(flags, &["read", "write", "exec"]);
            }
            other => panic!("expected Flags, got {:?}", other),
        }
    }

    #[test]
    fn parse_type_alias() {
        let src = r#"
            interface types {
                type byte-list = list<u8>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        match &doc.interfaces[0].types[0] {
            WitTypeDef::TypeAlias { name, target } => {
                assert_eq!(name, "byte-list");
                assert_eq!(*target, WitType::List(Box::new(WitType::U8)));
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    #[test]
    fn parse_world() {
        let src = r#"
            world my-app {
                import wasi:clocks/monotonic-clock@0.2.0;
                export run: func() -> result<_, string>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.worlds.len(), 1);
        let w = &doc.worlds[0];
        assert_eq!(w.name, "my-app");
        assert_eq!(w.imports.len(), 1);
        assert_eq!(w.exports.len(), 1);
        match &w.imports[0] {
            WitWorldItem::Interface(name) => {
                assert_eq!(name, "wasi:clocks/monotonic-clock@0.2.0");
            }
            other => panic!("expected Interface import, got {:?}", other),
        }
        match &w.exports[0] {
            WitWorldItem::Function(f) => {
                assert_eq!(f.name, "run");
                assert!(f.params.is_empty());
                assert!(matches!(f.result, Some(WitType::Result { .. })));
            }
            other => panic!("expected Function export, got {:?}", other),
        }
    }

    #[test]
    fn parse_complex_document() {
        let src = r#"
            package example:sensors@1.0.0;

            interface readings {
                record sensor-data {
                    temperature: f64,
                    pressure: f64,
                    timestamp: u64,
                }

                get-reading: func() -> sensor-data;
                calibrate: func(offset: f64) -> result<_, string>;
            }

            world sensor {
                export readings;
                import wasi:clocks/monotonic-clock@0.2.0;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert!(doc.package.is_some());
        assert_eq!(doc.interfaces.len(), 1);
        assert_eq!(doc.worlds.len(), 1);

        let iface = &doc.interfaces[0];
        assert_eq!(iface.name, "readings");
        assert_eq!(iface.types.len(), 1);
        assert_eq!(iface.functions.len(), 2);

        let world = &doc.worlds[0];
        assert_eq!(world.name, "sensor");
        assert_eq!(world.exports.len(), 1);
        assert_eq!(world.imports.len(), 1);
    }

    #[test]
    fn parse_result_types() {
        let src = r#"
            interface results {
                ok-only: func() -> result<u32>;
                err-only: func() -> result<_, string>;
                both: func() -> result<u32, string>;
                neither: func() -> result;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let funcs = &doc.interfaces[0].functions;

        assert!(matches!(
            funcs[0].result,
            Some(WitType::Result {
                ok: Some(_),
                err: None
            })
        ));
        assert!(matches!(
            funcs[1].result,
            Some(WitType::Result {
                ok: None,
                err: Some(_)
            })
        ));
        assert!(matches!(
            funcs[2].result,
            Some(WitType::Result {
                ok: Some(_),
                err: Some(_)
            })
        ));
        assert!(matches!(
            funcs[3].result,
            Some(WitType::Result {
                ok: None,
                err: None
            })
        ));
    }

    #[test]
    fn parse_comments() {
        let src = r#"
            // This is a comment
            interface foo {
                // Another comment
                bar: func();
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.interfaces.len(), 1);
        assert_eq!(doc.interfaces[0].functions.len(), 1);
    }

    #[test]
    fn parse_option_type() {
        let src = r#"
            interface types {
                get: func() -> option<u32>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(
            doc.interfaces[0].functions[0].result,
            Some(WitType::Option_(Box::new(WitType::U32)))
        );
    }

    #[test]
    fn parse_tuple_type() {
        let src = r#"
            interface types {
                get: func() -> tuple<u32, string, bool>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(
            doc.interfaces[0].functions[0].result,
            Some(WitType::Tuple(vec![
                WitType::U32,
                WitType::String_,
                WitType::Bool
            ]))
        );
    }

    #[test]
    fn parse_empty_document() {
        let doc = parse_wit("").unwrap();
        assert!(doc.package.is_none());
        assert!(doc.worlds.is_empty());
        assert!(doc.interfaces.is_empty());
    }

    #[test]
    fn parse_multiple_params() {
        let src = r#"
            interface math {
                add: func(a: s32, b: s32) -> s32;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let f = &doc.interfaces[0].functions[0];
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0], ("a".into(), WitType::S32));
        assert_eq!(f.params[1], ("b".into(), WitType::S32));
        assert_eq!(f.result, Some(WitType::S32));
    }

    #[test]
    fn parse_no_return() {
        let src = r#"
            interface io {
                print: func(msg: string);
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.interfaces[0].functions[0].result, None);
    }

    #[test]
    fn kebab_case_conversion() {
        assert_eq!(kebab_to_pascal("sensor-data"), "SensorData");
        assert_eq!(kebab_to_pascal("get-reading"), "GetReading");
        assert_eq!(kebab_to_pascal("simple"), "Simple");
        assert_eq!(kebab_to_snake("sensor-data"), "sensor_data");
        assert_eq!(kebab_to_snake("get-reading"), "get_reading");
    }

    #[test]
    fn to_kebab_case_conversion() {
        assert_eq!(to_kebab_case("SensorData"), "sensor-data");
        assert_eq!(to_kebab_case("get_reading"), "get-reading");
        assert_eq!(to_kebab_case("simple"), "simple");
    }

    #[test]
    fn parse_world_with_interface_export() {
        let src = r#"
            world http-handler {
                import wasi:http/types@0.2.0;
                export wasi:http/handler@0.2.0;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let w = &doc.worlds[0];
        assert_eq!(w.imports.len(), 1);
        assert_eq!(w.exports.len(), 1);
    }

    #[test]
    fn parse_nested_generic_types() {
        let src = r#"
            interface types {
                get: func() -> option<list<u8>>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(
            doc.interfaces[0].functions[0].result,
            Some(WitType::Option_(Box::new(WitType::List(Box::new(
                WitType::U8
            )))))
        );
    }

    #[test]
    fn parse_resource_definition() {
        let src = r#"
            interface streams {
                resource input-stream {
                    read: func(len: u64) -> list<u8>;
                }
            }
        "#;
        let doc = parse_wit(src).unwrap();
        assert_eq!(doc.interfaces[0].types.len(), 1);
        match &doc.interfaces[0].types[0] {
            WitTypeDef::Resource { name } => assert_eq!(name, "input-stream"),
            other => panic!("expected Resource, got {:?}", other),
        }
    }

    #[test]
    fn parse_async_function() {
        let src = r#"
            interface pipeline {
                process: async func(input: stream<u32>) -> stream<f64>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let f = &doc.interfaces[0].functions[0];
        assert_eq!(f.name, "process");
        assert!(f.is_async);
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].1, WitType::Stream(Box::new(WitType::U32)));
        assert_eq!(f.result, Some(WitType::Stream(Box::new(WitType::F64))));
    }

    #[test]
    fn parse_future_type() {
        let src = r#"
            interface types {
                classify: async func(sample: future<f64>) -> string;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let f = &doc.interfaces[0].functions[0];
        assert!(f.is_async);
        assert_eq!(f.params[0].1, WitType::Future(Box::new(WitType::F64)));
    }

    #[test]
    fn parse_world_with_async_export() {
        let src = r#"
            world processor {
                export process: async func(input: stream<u32>) -> stream<f64>;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let w = &doc.worlds[0];
        assert_eq!(w.exports.len(), 1);
        match &w.exports[0] {
            WitWorldItem::Function(f) => {
                assert!(f.is_async);
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn sync_function_not_async() {
        let src = r#"
            interface api {
                greet: func(name: string) -> string;
            }
        "#;
        let doc = parse_wit(src).unwrap();
        let f = &doc.interfaces[0].functions[0];
        assert!(!f.is_async);
    }
}
