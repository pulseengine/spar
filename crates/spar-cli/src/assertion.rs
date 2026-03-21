//! Structural assertion engine for `spar verify`.
//!
//! Parses and evaluates a mini expression language that queries the AADL
//! instance model directly. Assertions complement the existing requirement
//! checks (which filter analysis diagnostics) by enabling structural queries
//! like "all threads must have Period" or "no processor above 80% utilization".
//!
//! # Expression Grammar
//!
//! ```text
//! expr        = pipeline | bool_expr
//! pipeline    = source ( '.' method )*
//! source      = 'components' | 'analysis' '(' STRING ')'
//! method      = 'where' '(' bool_expr ')'
//!             | 'all' '(' bool_expr ')'
//!             | 'any' '(' bool_expr ')'
//!             | 'none' '(' bool_expr ')'
//!             | 'count' '(' ')'
//!             | 'features'
//!             | 'diagnostics'
//! bool_expr   = bool_term ( 'or' bool_term )*
//! bool_term   = bool_atom ( 'and' bool_atom )*
//! bool_atom   = 'not' bool_atom
//!             | 'has' '(' STRING ')'
//!             | 'connected'
//!             | field '==' STRING
//!             | field '.contains' '(' STRING ')'
//!             | '(' bool_expr ')'
//! field       = 'category' | 'kind' | 'direction' | 'severity' | 'message'
//! STRING      = '\'' [^']* '\''
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};
use spar_analysis::{AnalysisDiagnostic, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, FeatureInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};

use crate::verify::SeverityFilter;

// ── TOML schema ─────────────────────────────────────────────────────

/// A single assertion entry from the TOML file.
#[derive(Debug, Deserialize)]
pub(crate) struct Assertion {
    /// Unique assertion identifier, e.g. `"ASSERT-TIMING-001"`.
    pub id: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// The check expression to evaluate.
    pub check: String,
    /// Severity for a failed assertion (used by the reporter for output formatting).
    #[serde(default = "default_severity")]
    pub severity: SeverityFilter,
}


fn default_severity() -> SeverityFilter {
    SeverityFilter::Error
}

// ── Report types ────────────────────────────────────────────────────

/// Outcome of evaluating one assertion.
#[derive(Debug, Serialize)]
pub(crate) struct AssertionResult {
    pub id: String,
    pub description: String,
    pub check: String,
    pub severity: String,
    pub status: crate::verify::Status,
    /// Human-readable explanation of the result.
    pub detail: String,
}

// ── AST ─────────────────────────────────────────────────────────────

/// Top-level expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    /// `components`
    Components,
    /// `analysis('name')`
    Analysis(String),
    /// Pipeline: `source.method1().method2()`
    Pipeline(Box<Expr>, Vec<Method>),
}

/// A method call in a pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Method {
    Where(BoolExpr),
    All(BoolExpr),
    Any(BoolExpr),
    None(BoolExpr),
    Count,
    Features,
    Diagnostics,
}

/// A boolean expression for predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BoolExpr {
    And(Vec<BoolExpr>),
    Or(Vec<BoolExpr>),
    Not(Box<BoolExpr>),
    /// `has('Property_Set::Property_Name')`
    Has(String),
    /// `connected` — feature is connected
    Connected,
    /// `category == 'thread'`
    CategoryEq(String),
    /// `kind == 'data_port'`
    KindEq(String),
    /// `direction == 'in'`
    DirectionEq(String),
    /// `severity == 'error'`
    SeverityEq(String),
    /// `message.contains('text')`
    MessageContains(String),
}

// ── Parser ──────────────────────────────────────────────────────────

/// Parse error with position information.
#[derive(Debug, Clone)]
pub(crate) struct ParseError {
    pub pos: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at position {}: {}", self.pos, self.message)
    }
}

struct Parser {
    input: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();

        // Parse source
        let source = if self.peek_word("components") {
            self.advance_word("components");
            Expr::Components
        } else if self.peek_word("analysis") {
            self.advance_word("analysis");
            self.skip_ws();
            self.expect_char('(')?;
            let name = self.parse_string()?;
            self.skip_ws();
            self.expect_char(')')?;
            Expr::Analysis(name)
        } else {
            return Err(self.error("expected 'components' or 'analysis'"));
        };

        // Parse methods
        let mut methods = Vec::new();
        loop {
            self.skip_ws();
            if self.peek_char() != Some('.') {
                break;
            }
            self.pos += 1; // consume '.'
            let method = self.parse_method()?;
            methods.push(method);
        }

        self.skip_ws();
        if self.pos != self.input.len() {
            return Err(self.error(&format!(
                "unexpected character '{}'",
                self.input[self.pos]
            )));
        }

        if methods.is_empty() {
            Ok(source)
        } else {
            Ok(Expr::Pipeline(Box::new(source), methods))
        }
    }

    fn parse_method(&mut self) -> Result<Method, ParseError> {
        self.skip_ws();

        if self.peek_word("where") {
            self.advance_word("where");
            self.skip_ws();
            self.expect_char('(')?;
            let pred = self.parse_bool_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            Ok(Method::Where(pred))
        } else if self.peek_word("all") {
            self.advance_word("all");
            self.skip_ws();
            self.expect_char('(')?;
            let pred = self.parse_bool_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            Ok(Method::All(pred))
        } else if self.peek_word("any") {
            self.advance_word("any");
            self.skip_ws();
            self.expect_char('(')?;
            let pred = self.parse_bool_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            Ok(Method::Any(pred))
        } else if self.peek_word("none") {
            self.advance_word("none");
            self.skip_ws();
            self.expect_char('(')?;
            let pred = self.parse_bool_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            Ok(Method::None(pred))
        } else if self.peek_word("count") {
            self.advance_word("count");
            self.skip_ws();
            self.expect_char('(')?;
            self.skip_ws();
            self.expect_char(')')?;
            Ok(Method::Count)
        } else if self.peek_word("features") {
            self.advance_word("features");
            Ok(Method::Features)
        } else if self.peek_word("diagnostics") {
            self.advance_word("diagnostics");
            Ok(Method::Diagnostics)
        } else {
            Err(self.error("expected method name (where, all, any, none, count, features, diagnostics)"))
        }
    }

    fn parse_bool_expr(&mut self) -> Result<BoolExpr, ParseError> {
        let first = self.parse_bool_term()?;
        let mut terms = vec![first];

        loop {
            self.skip_ws();
            if self.peek_word("or") {
                self.advance_word("or");
                terms.push(self.parse_bool_term()?);
            } else {
                break;
            }
        }

        if terms.len() == 1 {
            Ok(terms.pop().unwrap())
        } else {
            Ok(BoolExpr::Or(terms))
        }
    }

    fn parse_bool_term(&mut self) -> Result<BoolExpr, ParseError> {
        let first = self.parse_bool_atom()?;
        let mut factors = vec![first];

        loop {
            self.skip_ws();
            if self.peek_word("and") {
                self.advance_word("and");
                factors.push(self.parse_bool_atom()?);
            } else {
                break;
            }
        }

        if factors.len() == 1 {
            Ok(factors.pop().unwrap())
        } else {
            Ok(BoolExpr::And(factors))
        }
    }

    fn parse_bool_atom(&mut self) -> Result<BoolExpr, ParseError> {
        self.skip_ws();

        // not
        if self.peek_word("not") {
            self.advance_word("not");
            let inner = self.parse_bool_atom()?;
            return Ok(BoolExpr::Not(Box::new(inner)));
        }

        // parenthesized expression
        if self.peek_char() == Some('(') {
            self.pos += 1;
            let inner = self.parse_bool_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            return Ok(inner);
        }

        // has('...')
        if self.peek_word("has") {
            self.advance_word("has");
            self.skip_ws();
            self.expect_char('(')?;
            let prop = self.parse_string()?;
            self.skip_ws();
            self.expect_char(')')?;
            return Ok(BoolExpr::Has(prop));
        }

        // connected
        if self.peek_word("connected") {
            self.advance_word("connected");
            return Ok(BoolExpr::Connected);
        }

        // field == 'value' or field.contains('value')
        if self.peek_word("category") {
            self.advance_word("category");
            self.skip_ws();
            self.expect_str("==")?;
            self.skip_ws();
            let val = self.parse_string()?;
            return Ok(BoolExpr::CategoryEq(val));
        }

        if self.peek_word("kind") {
            self.advance_word("kind");
            self.skip_ws();
            self.expect_str("==")?;
            self.skip_ws();
            let val = self.parse_string()?;
            return Ok(BoolExpr::KindEq(val));
        }

        if self.peek_word("direction") {
            self.advance_word("direction");
            self.skip_ws();
            self.expect_str("==")?;
            self.skip_ws();
            let val = self.parse_string()?;
            return Ok(BoolExpr::DirectionEq(val));
        }

        if self.peek_word("severity") {
            self.advance_word("severity");
            self.skip_ws();
            self.expect_str("==")?;
            self.skip_ws();
            let val = self.parse_string()?;
            return Ok(BoolExpr::SeverityEq(val));
        }

        if self.peek_word("message") {
            self.advance_word("message");
            self.skip_ws();
            self.expect_char('.')?;
            if !self.peek_word("contains") {
                return Err(self.error("expected 'contains' after 'message.'"));
            }
            self.advance_word("contains");
            self.skip_ws();
            self.expect_char('(')?;
            let val = self.parse_string()?;
            self.skip_ws();
            self.expect_char(')')?;
            return Ok(BoolExpr::MessageContains(val));
        }

        Err(self.error(
            "expected 'not', 'has', 'connected', 'category', 'kind', 'direction', 'severity', 'message', or '('",
        ))
    }

    // ── Helpers ─────────────────────────────────────────────────────

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_word(&self, word: &str) -> bool {
        let chars: Vec<char> = word.chars().collect();
        if self.pos + chars.len() > self.input.len() {
            return false;
        }
        for (i, &ch) in chars.iter().enumerate() {
            if self.input[self.pos + i] != ch {
                return false;
            }
        }
        // Must not be followed by an alphanumeric or underscore (word boundary)
        let after = self.pos + chars.len();
        if after < self.input.len() {
            let next = self.input[after];
            if next.is_alphanumeric() || next == '_' {
                return false;
            }
        }
        true
    }

    fn advance_word(&mut self, word: &str) {
        self.pos += word.len();
    }

    fn expect_char(&mut self, ch: char) -> Result<(), ParseError> {
        self.skip_ws();
        if self.peek_char() == Some(ch) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!(
                "expected '{}', found '{}'",
                ch,
                self.peek_char()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "EOF".to_string())
            )))
        }
    }

    fn expect_str(&mut self, s: &str) -> Result<(), ParseError> {
        self.skip_ws();
        let chars: Vec<char> = s.chars().collect();
        for (i, &ch) in chars.iter().enumerate() {
            if self.pos + i >= self.input.len() || self.input[self.pos + i] != ch {
                return Err(self.error(&format!("expected '{}'", s)));
            }
        }
        self.pos += chars.len();
        Ok(())
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.skip_ws();
        if self.peek_char() != Some('\'') {
            return Err(self.error("expected string literal starting with '"));
        }
        self.pos += 1; // consume opening quote
        let start = self.pos;
        while self.pos < self.input.len() && self.input[self.pos] != '\'' {
            self.pos += 1;
        }
        if self.pos >= self.input.len() {
            return Err(self.error("unterminated string literal"));
        }
        let s: String = self.input[start..self.pos].iter().collect();
        self.pos += 1; // consume closing quote
        Ok(s)
    }

    fn error(&self, msg: &str) -> ParseError {
        ParseError {
            pos: self.pos,
            message: msg.to_string(),
        }
    }
}

/// Parse a check expression string into an AST.
fn parse_check(input: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(input);
    parser.parse_expr()
}

// ── Evaluator ───────────────────────────────────────────────────────

/// The intermediate value during evaluation.
#[derive(Debug)]
enum Value {
    /// A set of component instances.
    Components(Vec<ComponentInstanceIdx>),
    /// A set of feature instances (with their owning component for context).
    Features(Vec<(ComponentInstanceIdx, FeatureInstanceIdx)>),
    /// A boolean result.
    Bool(bool),
    /// A count.
    Count(usize),
    /// A set of diagnostics.
    Diagnostics(Vec<AnalysisDiagnostic>),
}

/// Context for evaluation.
pub(crate) struct EvalContext<'a> {
    pub instance: &'a SystemInstance,
    pub diagnostics: &'a [AnalysisDiagnostic],
}

/// Evaluation error.
#[derive(Debug, Clone)]
pub(crate) struct EvalError {
    pub message: String,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Evaluate a parsed expression against a context.
fn eval_expr(expr: &Expr, ctx: &EvalContext) -> Result<Value, EvalError> {
    match expr {
        Expr::Components => {
            let indices: Vec<_> = ctx.instance.all_components().map(|(idx, _)| idx).collect();
            Ok(Value::Components(indices))
        }
        Expr::Analysis(name) => {
            let filtered: Vec<_> = ctx
                .diagnostics
                .iter()
                .filter(|d| d.analysis == *name)
                .cloned()
                .collect();
            Ok(Value::Diagnostics(filtered))
        }
        Expr::Pipeline(source, methods) => {
            let mut value = eval_expr(source, ctx)?;
            for method in methods {
                value = eval_method(method, value, ctx)?;
            }
            Ok(value)
        }
    }
}

/// Evaluate a method call on a value.
fn eval_method(method: &Method, value: Value, ctx: &EvalContext) -> Result<Value, EvalError> {
    match method {
        Method::Where(pred) => match value {
            Value::Components(comps) => {
                let filtered: Vec<_> = comps
                    .into_iter()
                    .filter(|&idx| eval_component_predicate(pred, idx, ctx))
                    .collect();
                Ok(Value::Components(filtered))
            }
            Value::Features(feats) => {
                let filtered: Vec<_> = feats
                    .into_iter()
                    .filter(|&(comp_idx, feat_idx)| {
                        eval_feature_predicate(pred, comp_idx, feat_idx, ctx)
                    })
                    .collect();
                Ok(Value::Features(filtered))
            }
            Value::Diagnostics(diags) => {
                let filtered: Vec<_> = diags
                    .into_iter()
                    .filter(|d| eval_diagnostic_predicate(pred, d))
                    .collect();
                Ok(Value::Diagnostics(filtered))
            }
            _ => Err(EvalError {
                message: "where() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        Method::All(pred) => match value {
            Value::Components(comps) => {
                let result = comps
                    .iter()
                    .all(|&idx| eval_component_predicate(pred, idx, ctx));
                Ok(Value::Bool(result))
            }
            Value::Features(feats) => {
                let result = feats
                    .iter()
                    .all(|&(comp_idx, feat_idx)| {
                        eval_feature_predicate(pred, comp_idx, feat_idx, ctx)
                    });
                Ok(Value::Bool(result))
            }
            Value::Diagnostics(diags) => {
                let result = diags.iter().all(|d| eval_diagnostic_predicate(pred, d));
                Ok(Value::Bool(result))
            }
            _ => Err(EvalError {
                message: "all() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        Method::Any(pred) => match value {
            Value::Components(comps) => {
                let result = comps
                    .iter()
                    .any(|&idx| eval_component_predicate(pred, idx, ctx));
                Ok(Value::Bool(result))
            }
            Value::Features(feats) => {
                let result = feats
                    .iter()
                    .any(|&(comp_idx, feat_idx)| {
                        eval_feature_predicate(pred, comp_idx, feat_idx, ctx)
                    });
                Ok(Value::Bool(result))
            }
            Value::Diagnostics(diags) => {
                let result = diags.iter().any(|d| eval_diagnostic_predicate(pred, d));
                Ok(Value::Bool(result))
            }
            _ => Err(EvalError {
                message: "any() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        Method::None(pred) => match value {
            Value::Components(comps) => {
                let result = !comps
                    .iter()
                    .any(|&idx| eval_component_predicate(pred, idx, ctx));
                Ok(Value::Bool(result))
            }
            Value::Features(feats) => {
                let result = !feats
                    .iter()
                    .any(|&(comp_idx, feat_idx)| {
                        eval_feature_predicate(pred, comp_idx, feat_idx, ctx)
                    });
                Ok(Value::Bool(result))
            }
            Value::Diagnostics(diags) => {
                let result = !diags.iter().any(|d| eval_diagnostic_predicate(pred, d));
                Ok(Value::Bool(result))
            }
            _ => Err(EvalError {
                message: "none() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        Method::Count => match value {
            Value::Components(comps) => Ok(Value::Count(comps.len())),
            Value::Features(feats) => Ok(Value::Count(feats.len())),
            Value::Diagnostics(diags) => Ok(Value::Count(diags.len())),
            _ => Err(EvalError {
                message: "count() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        Method::Features => match value {
            Value::Components(comps) => {
                let mut feats = Vec::new();
                for &comp_idx in &comps {
                    let comp = ctx.instance.component(comp_idx);
                    for &feat_idx in &comp.features {
                        feats.push((comp_idx, feat_idx));
                    }
                }
                Ok(Value::Features(feats))
            }
            _ => Err(EvalError {
                message: "features can only be accessed on components".to_string(),
            }),
        },
        Method::Diagnostics => match value {
            Value::Diagnostics(_) => Ok(value),
            _ => Err(EvalError {
                message: "diagnostics can only be accessed on analysis results".to_string(),
            }),
        },
    }
}

// ── Predicate evaluation ────────────────────────────────────────────

fn eval_component_predicate(
    pred: &BoolExpr,
    idx: ComponentInstanceIdx,
    ctx: &EvalContext,
) -> bool {
    match pred {
        BoolExpr::And(terms) => terms.iter().all(|t| eval_component_predicate(t, idx, ctx)),
        BoolExpr::Or(terms) => terms.iter().any(|t| eval_component_predicate(t, idx, ctx)),
        BoolExpr::Not(inner) => !eval_component_predicate(inner, idx, ctx),
        BoolExpr::CategoryEq(val) => {
            let comp = ctx.instance.component(idx);
            category_matches(&comp.category, val)
        }
        BoolExpr::Has(prop_name) => {
            let props = ctx.instance.properties_for(idx);
            // Parse "Property_Set::Property_Name" or "Property_Name"
            if let Some((set, name)) = prop_name.split_once("::") {
                props.get(set, name).is_some()
            } else {
                props.get("", prop_name).is_some()
            }
        }
        BoolExpr::Connected => {
            // A component is "connected" if it has connections or its parent does
            let comp = ctx.instance.component(idx);
            !comp.connections.is_empty()
                || comp
                    .parent
                    .map(|p| !ctx.instance.component(p).connections.is_empty())
                    .unwrap_or(false)
        }
        // Feature-level predicates don't apply to components
        BoolExpr::KindEq(_) | BoolExpr::DirectionEq(_) => false,
        // Diagnostic-level predicates don't apply to components
        BoolExpr::SeverityEq(_) | BoolExpr::MessageContains(_) => false,
    }
}

fn eval_feature_predicate(
    pred: &BoolExpr,
    comp_idx: ComponentInstanceIdx,
    feat_idx: FeatureInstanceIdx,
    ctx: &EvalContext,
) -> bool {
    match pred {
        BoolExpr::And(terms) => terms
            .iter()
            .all(|t| eval_feature_predicate(t, comp_idx, feat_idx, ctx)),
        BoolExpr::Or(terms) => terms
            .iter()
            .any(|t| eval_feature_predicate(t, comp_idx, feat_idx, ctx)),
        BoolExpr::Not(inner) => !eval_feature_predicate(inner, comp_idx, feat_idx, ctx),
        BoolExpr::KindEq(val) => {
            let feat = &ctx.instance.features[feat_idx];
            feature_kind_matches(&feat.kind, val)
        }
        BoolExpr::DirectionEq(val) => {
            let feat = &ctx.instance.features[feat_idx];
            direction_matches(feat.direction.as_ref(), val)
        }
        BoolExpr::Connected => {
            // A feature is considered "connected" if the owning component or
            // its parent has connections. This mirrors the heuristic used
            // by ConnectivityAnalysis.
            let comp = ctx.instance.component(comp_idx);
            let owner_has_conns = !comp.connections.is_empty();
            let parent_has_conns = comp
                .parent
                .map(|p| !ctx.instance.component(p).connections.is_empty())
                .unwrap_or(false);

            // For a more precise check, we examine connection endpoints
            // to see if this specific feature is referenced.
            let feat = &ctx.instance.features[feat_idx];
            let feat_name = feat.name.as_str();

            let specifically_connected = ctx.instance.connections.iter().any(|(_, conn)| {
                let src_match = conn
                    .src
                    .as_ref()
                    .map(|e| e.feature.as_str() == feat_name)
                    .unwrap_or(false);
                let dst_match = conn
                    .dst
                    .as_ref()
                    .map(|e| e.feature.as_str() == feat_name)
                    .unwrap_or(false);
                (src_match || dst_match)
                    && (conn.owner == comp_idx
                        || comp.parent.map(|p| p == conn.owner).unwrap_or(false))
            });

            specifically_connected || (owner_has_conns || parent_has_conns)
        }
        BoolExpr::CategoryEq(val) => {
            // Allow checking the owning component's category from a feature context
            let comp = ctx.instance.component(comp_idx);
            category_matches(&comp.category, val)
        }
        BoolExpr::Has(prop_name) => {
            // Check property on the owning component
            let props = ctx.instance.properties_for(comp_idx);
            if let Some((set, name)) = prop_name.split_once("::") {
                props.get(set, name).is_some()
            } else {
                props.get("", prop_name).is_some()
            }
        }
        // Diagnostic-level predicates don't apply to features
        BoolExpr::SeverityEq(_) | BoolExpr::MessageContains(_) => false,
    }
}

fn eval_diagnostic_predicate(pred: &BoolExpr, diag: &AnalysisDiagnostic) -> bool {
    match pred {
        BoolExpr::And(terms) => terms.iter().all(|t| eval_diagnostic_predicate(t, diag)),
        BoolExpr::Or(terms) => terms.iter().any(|t| eval_diagnostic_predicate(t, diag)),
        BoolExpr::Not(inner) => !eval_diagnostic_predicate(inner, diag),
        BoolExpr::SeverityEq(val) => {
            let sev_str = match diag.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            sev_str == val.as_str()
        }
        BoolExpr::MessageContains(text) => diag.message.contains(text.as_str()),
        // Component/feature predicates don't apply to diagnostics
        BoolExpr::CategoryEq(_)
        | BoolExpr::KindEq(_)
        | BoolExpr::DirectionEq(_)
        | BoolExpr::Connected
        | BoolExpr::Has(_) => false,
    }
}

// ── Matching helpers ────────────────────────────────────────────────

fn category_matches(cat: &ComponentCategory, val: &str) -> bool {
    let normalized = val.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "system" => *cat == ComponentCategory::System,
        "process" => *cat == ComponentCategory::Process,
        "thread" => *cat == ComponentCategory::Thread,
        "thread_group" | "threadgroup" => *cat == ComponentCategory::ThreadGroup,
        "processor" => *cat == ComponentCategory::Processor,
        "virtual_processor" | "virtualprocessor" => *cat == ComponentCategory::VirtualProcessor,
        "memory" => *cat == ComponentCategory::Memory,
        "bus" => *cat == ComponentCategory::Bus,
        "virtual_bus" | "virtualbus" => *cat == ComponentCategory::VirtualBus,
        "device" => *cat == ComponentCategory::Device,
        "subprogram" => *cat == ComponentCategory::Subprogram,
        "subprogram_group" | "subprogramgroup" => *cat == ComponentCategory::SubprogramGroup,
        "data" => *cat == ComponentCategory::Data,
        "abstract" => *cat == ComponentCategory::Abstract,
        _ => false,
    }
}

fn feature_kind_matches(kind: &FeatureKind, val: &str) -> bool {
    let normalized = val.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "data_port" | "dataport" => *kind == FeatureKind::DataPort,
        "event_port" | "eventport" => *kind == FeatureKind::EventPort,
        "event_data_port" | "eventdataport" => *kind == FeatureKind::EventDataPort,
        "parameter" => *kind == FeatureKind::Parameter,
        "data_access" | "dataaccess" => *kind == FeatureKind::DataAccess,
        "bus_access" | "busaccess" => *kind == FeatureKind::BusAccess,
        "subprogram_access" | "subprogramaccess" => *kind == FeatureKind::SubprogramAccess,
        "subprogram_group_access" | "subprogramgroupaccess" => {
            *kind == FeatureKind::SubprogramGroupAccess
        }
        "feature_group" | "featuregroup" => *kind == FeatureKind::FeatureGroup,
        "abstract_feature" | "abstractfeature" => *kind == FeatureKind::AbstractFeature,
        _ => false,
    }
}

fn direction_matches(dir: Option<&Direction>, val: &str) -> bool {
    let normalized = val.to_lowercase().replace('-', "_");
    match (dir, normalized.as_str()) {
        (Some(Direction::In), "in") => true,
        (Some(Direction::Out), "out") => true,
        (Some(Direction::InOut), "in_out" | "inout") => true,
        (None, "none") => true,
        _ => false,
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Evaluate a list of assertions against an instance model and diagnostics.
pub(crate) fn evaluate_assertions(
    assertions: &[Assertion],
    ctx: &EvalContext,
) -> Vec<AssertionResult> {
    assertions.iter().map(|a| evaluate_one(a, ctx)).collect()
}

fn evaluate_one(assertion: &Assertion, ctx: &EvalContext) -> AssertionResult {
    let sev = assertion.severity.to_string();

    let expr = match parse_check(&assertion.check) {
        Ok(e) => e,
        Err(err) => {
            return AssertionResult {
                id: assertion.id.clone(),
                description: assertion.description.clone(),
                check: assertion.check.clone(),
                severity: sev,
                status: crate::verify::Status::Fail,
                detail: format!("parse error: {}", err),
            };
        }
    };

    match eval_expr(&expr, ctx) {
        Ok(Value::Bool(true)) => AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: crate::verify::Status::Pass,
            detail: "assertion passed".to_string(),
        },
        Ok(Value::Bool(false)) => AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: crate::verify::Status::Fail,
            detail: "assertion failed".to_string(),
        },
        Ok(Value::Count(n)) => {
            // For count, we report the value; pass if > 0
            AssertionResult {
                id: assertion.id.clone(),
                description: assertion.description.clone(),
                check: assertion.check.clone(),
                severity: sev,
                status: if n > 0 {
                    crate::verify::Status::Pass
                } else {
                    crate::verify::Status::Fail
                },
                detail: format!("count = {}", n),
            }
        }
        Ok(Value::Components(comps)) => {
            // A pipeline that ends at components — report count, pass if non-empty
            AssertionResult {
                id: assertion.id.clone(),
                description: assertion.description.clone(),
                check: assertion.check.clone(),
                severity: sev,
                status: if !comps.is_empty() {
                    crate::verify::Status::Pass
                } else {
                    crate::verify::Status::Fail
                },
                detail: format!("matched {} components", comps.len()),
            }
        }
        Ok(Value::Features(feats)) => AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: if !feats.is_empty() {
                crate::verify::Status::Pass
            } else {
                crate::verify::Status::Fail
            },
            detail: format!("matched {} features", feats.len()),
        },
        Ok(Value::Diagnostics(diags)) => AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: if !diags.is_empty() {
                crate::verify::Status::Pass
            } else {
                crate::verify::Status::Fail
            },
            detail: format!("matched {} diagnostics", diags.len()),
        },
        Err(err) => AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: crate::verify::Status::Fail,
            detail: format!("evaluation error: {}", err),
        },
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parser tests ────────────────────────────────────────────────

    #[test]
    fn parse_components_source() {
        let expr = parse_check("components").unwrap();
        assert_eq!(expr, Expr::Components);
    }

    #[test]
    fn parse_analysis_source() {
        let expr = parse_check("analysis('scheduling')").unwrap();
        assert_eq!(expr, Expr::Analysis("scheduling".to_string()));
    }

    #[test]
    fn parse_components_where() {
        let expr =
            parse_check("components.where(category == 'thread')").unwrap();
        match expr {
            Expr::Pipeline(source, methods) => {
                assert_eq!(*source, Expr::Components);
                assert_eq!(methods.len(), 1);
                match &methods[0] {
                    Method::Where(BoolExpr::CategoryEq(val)) => {
                        assert_eq!(val, "thread");
                    }
                    other => panic!("expected Where(CategoryEq), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_all_predicate() {
        let expr = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                assert_eq!(methods.len(), 2);
                assert!(matches!(&methods[0], Method::Where(_)));
                match &methods[1] {
                    Method::All(BoolExpr::Has(prop)) => {
                        assert_eq!(prop, "Timing_Properties::Period");
                    }
                    other => panic!("expected All(Has), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_and_predicate() {
        let expr = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period') and has('Timing_Properties::Compute_Execution_Time'))",
        ).unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                match &methods[1] {
                    Method::All(BoolExpr::And(terms)) => {
                        assert_eq!(terms.len(), 2);
                        assert!(matches!(&terms[0], BoolExpr::Has(_)));
                        assert!(matches!(&terms[1], BoolExpr::Has(_)));
                    }
                    other => panic!("expected All(And), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_or_predicate() {
        let expr = parse_check(
            "components.any(category == 'thread' or category == 'process')",
        )
        .unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                match &methods[0] {
                    Method::Any(BoolExpr::Or(terms)) => {
                        assert_eq!(terms.len(), 2);
                    }
                    other => panic!("expected Any(Or), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_not_predicate() {
        let expr = parse_check("components.none(not has('Timing_Properties::Period'))").unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                match &methods[0] {
                    Method::None(BoolExpr::Not(inner)) => {
                        assert!(matches!(**inner, BoolExpr::Has(_)));
                    }
                    other => panic!("expected None(Not), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_features_pipeline() {
        let expr = parse_check(
            "components.where(category == 'thread').features.where(kind == 'data_port' and direction == 'out').all(connected)",
        ).unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                assert_eq!(methods.len(), 4);
                assert!(matches!(&methods[0], Method::Where(_)));
                assert!(matches!(&methods[1], Method::Features));
                assert!(matches!(&methods[2], Method::Where(_)));
                assert!(matches!(&methods[3], Method::All(BoolExpr::Connected)));
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_analysis_diagnostics() {
        let expr = parse_check(
            "analysis('scheduling').diagnostics.none(severity == 'warning' and message.contains('exceeds'))",
        ).unwrap();
        match expr {
            Expr::Pipeline(source, methods) => {
                assert_eq!(*source, Expr::Analysis("scheduling".to_string()));
                assert_eq!(methods.len(), 2);
                assert!(matches!(&methods[0], Method::Diagnostics));
                match &methods[1] {
                    Method::None(BoolExpr::And(terms)) => {
                        assert_eq!(terms.len(), 2);
                        assert!(matches!(&terms[0], BoolExpr::SeverityEq(_)));
                        assert!(matches!(&terms[1], BoolExpr::MessageContains(_)));
                    }
                    other => panic!("expected None(And), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_count() {
        let expr =
            parse_check("components.where(category == 'thread').count()").unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                assert_eq!(methods.len(), 2);
                assert!(matches!(&methods[1], Method::Count));
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    #[test]
    fn parse_parenthesized_bool() {
        let expr = parse_check(
            "components.all((category == 'thread' or category == 'process') and has('Timing_Properties::Period'))",
        ).unwrap();
        match expr {
            Expr::Pipeline(_, methods) => {
                match &methods[0] {
                    Method::All(BoolExpr::And(terms)) => {
                        assert_eq!(terms.len(), 2);
                        assert!(matches!(&terms[0], BoolExpr::Or(_)));
                        assert!(matches!(&terms[1], BoolExpr::Has(_)));
                    }
                    other => panic!("expected All(And(Or, Has)), got {:?}", other),
                }
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
    }

    // ── Parser error tests ──────────────────────────────────────────

    #[test]
    fn parse_error_empty() {
        let err = parse_check("").unwrap_err();
        assert!(err.message.contains("expected"));
    }

    #[test]
    fn parse_error_bad_source() {
        let err = parse_check("foobar").unwrap_err();
        assert!(err.message.contains("expected"));
    }

    #[test]
    fn parse_error_unterminated_string() {
        let err = parse_check("components.where(category == 'thread)").unwrap_err();
        assert!(err.message.contains("unterminated string"));
    }

    #[test]
    fn parse_error_missing_paren() {
        let err = parse_check("components.where(category == 'thread'").unwrap_err();
        assert!(err.message.contains("expected ')'"));
    }

    #[test]
    fn parse_error_bad_method() {
        let err = parse_check("components.foobar()").unwrap_err();
        assert!(err.message.contains("expected method name"));
    }

    #[test]
    fn parse_error_trailing_text() {
        let err = parse_check("components foobar").unwrap_err();
        assert!(err.message.contains("unexpected"));
    }

    // ── Evaluator tests ─────────────────────────────────────────────

    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::{
        ComponentInstance, ConnectionEnd, ConnectionInstance, FeatureInstance,
    };
    use spar_hir_def::item_tree::ConnectionKind;
    use spar_hir_def::name::Name;
    use spar_hir_def::properties::PropertyMap;

    /// Build a minimal SystemInstance for testing.
    fn make_test_instance() -> SystemInstance {
        let mut components = Arena::<ComponentInstance>::default();
        let mut features = Arena::<FeatureInstance>::default();
        let mut connections = Arena::<ConnectionInstance>::default();
        let mut property_maps = FxHashMap::default();

        // Root system component
        let root_idx = components.alloc(ComponentInstance {
            name: Name::new("root"),
            category: ComponentCategory::System,
            type_name: Name::new("TopLevel"),
            impl_name: Some(Name::new("Impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Thread with timing properties
        let thread1_idx = components.alloc(ComponentInstance {
            name: Name::new("thread1"),
            category: ComponentCategory::Thread,
            type_name: Name::new("SensorThread"),
            impl_name: Some(Name::new("Impl")),
            package: Name::new("Pkg"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Set timing properties on thread1
        let mut t1_props = PropertyMap::new();
        use spar_hir_def::name::PropertyRef;
        use spar_hir_def::properties::PropertyValue;
        t1_props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Timing_Properties")),
                property_name: Name::new("Period"),
            },
            value: "10 ms".to_string(),
            is_append: false,
        });
        t1_props.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new("Timing_Properties")),
                property_name: Name::new("Compute_Execution_Time"),
            },
            value: "1 ms .. 5 ms".to_string(),
            is_append: false,
        });
        property_maps.insert(thread1_idx, t1_props);

        // Thread without timing properties
        let thread2_idx = components.alloc(ComponentInstance {
            name: Name::new("thread2"),
            category: ComponentCategory::Thread,
            type_name: Name::new("ActuatorThread"),
            impl_name: Some(Name::new("Impl")),
            package: Name::new("Pkg"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Processor
        let _proc_idx = components.alloc(ComponentInstance {
            name: Name::new("cpu"),
            category: ComponentCategory::Processor,
            type_name: Name::new("ARM"),
            impl_name: Some(Name::new("Impl")),
            package: Name::new("Pkg"),
            parent: Some(root_idx),
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        // Add features to thread1: an out data port
        let t1_out = features.alloc(FeatureInstance {
            name: Name::new("sensor_out"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::Out),
            owner: thread1_idx,
            classifier: None,
            access_kind: None,
            array_index: None,
        });

        // Add features to thread2: an in data port
        let t2_in = features.alloc(FeatureInstance {
            name: Name::new("cmd_in"),
            kind: FeatureKind::DataPort,
            direction: Some(Direction::In),
            owner: thread2_idx,
            classifier: None,
            access_kind: None,
            array_index: None,
        });

        // Unconnected out port on thread2
        let _t2_out = features.alloc(FeatureInstance {
            name: Name::new("status_out"),
            kind: FeatureKind::EventPort,
            direction: Some(Direction::Out),
            owner: thread2_idx,
            classifier: None,
            access_kind: None,
            array_index: None,
        });

        // Connect thread1.sensor_out -> thread2.cmd_in
        let conn_idx = connections.alloc(ConnectionInstance {
            name: Name::new("c1"),
            kind: ConnectionKind::Port,
            is_bidirectional: false,
            owner: root_idx,
            src: Some(ConnectionEnd {
                subcomponent: Some(Name::new("thread1")),
                feature: Name::new("sensor_out"),
            }),
            dst: Some(ConnectionEnd {
                subcomponent: Some(Name::new("thread2")),
                feature: Name::new("cmd_in"),
            }),
            in_modes: Vec::new(),
        });

        // Update parent/child/feature/connection references
        components[root_idx].children = vec![thread1_idx, thread2_idx, _proc_idx];
        components[root_idx].connections = vec![conn_idx];
        components[thread1_idx].features = vec![t1_out];
        components[thread2_idx].features = vec![t2_in, _t2_out];

        SystemInstance {
            root: root_idx,
            components,
            features,
            connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps,
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }

    fn make_test_diagnostics() -> Vec<AnalysisDiagnostic> {
        vec![
            AnalysisDiagnostic {
                severity: Severity::Warning,
                message: "processor utilization exceeds 80%".to_string(),
                path: vec!["root".to_string(), "cpu".to_string()],
                analysis: "scheduling".to_string(),
            },
            AnalysisDiagnostic {
                severity: Severity::Error,
                message: "missing binding".to_string(),
                path: vec!["root".to_string(), "thread1".to_string()],
                analysis: "binding_check".to_string(),
            },
            AnalysisDiagnostic {
                severity: Severity::Info,
                message: "all ports connected".to_string(),
                path: vec!["root".to_string()],
                analysis: "connectivity".to_string(),
            },
        ]
    }

    #[test]
    fn eval_components_returns_all() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check("components").unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Components(comps) => assert_eq!(comps.len(), 4), // root + 2 threads + cpu
            other => panic!("expected Components, got {:?}", other),
        }
    }

    #[test]
    fn eval_components_where_category() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check("components.where(category == 'thread')").unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Components(comps) => assert_eq!(comps.len(), 2),
            other => panic!("expected Components, got {:?}", other),
        }
    }

    #[test]
    fn eval_all_has_property_fails() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // Only thread1 has Period, thread2 does not
        let expr = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(!b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_any_has_property_passes() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_none_has_property() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // None of the threads have Deployment_Properties::Actual_Processor_Binding
        let expr = parse_check(
            "components.where(category == 'thread').none(has('Deployment_Properties::Actual_Processor_Binding'))",
        ).unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_count() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check("components.where(category == 'thread').count()").unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Count(n) => assert_eq!(n, 2),
            other => panic!("expected Count, got {:?}", other),
        }
    }

    #[test]
    fn eval_features_where_kind() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check(
            "components.where(category == 'thread').features.where(kind == 'data_port').count()",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Count(n) => assert_eq!(n, 2), // sensor_out + cmd_in
            other => panic!("expected Count, got {:?}", other),
        }
    }

    #[test]
    fn eval_features_where_direction() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check(
            "components.where(category == 'thread').features.where(kind == 'data_port' and direction == 'out').count()",
        ).unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Count(n) => assert_eq!(n, 1), // only sensor_out
            other => panic!("expected Count, got {:?}", other),
        }
    }

    #[test]
    fn eval_analysis_diagnostics_none() {
        let inst = make_test_instance();
        let diags = make_test_diagnostics();
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // Should fail because there IS a scheduling warning containing "exceeds"
        let expr = parse_check(
            "analysis('scheduling').diagnostics.none(severity == 'warning' and message.contains('exceeds'))",
        ).unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(!b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_analysis_diagnostics_no_match() {
        let inst = make_test_instance();
        let diags = make_test_diagnostics();
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // Should pass because no scheduling ERROR containing "exceeds"
        let expr = parse_check(
            "analysis('scheduling').diagnostics.none(severity == 'error' and message.contains('exceeds'))",
        ).unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_and_predicate() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // thread1 has both properties, thread2 has neither
        let expr = parse_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period') and has('Timing_Properties::Compute_Execution_Time'))",
        ).unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_or_predicate() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        let expr = parse_check(
            "components.any(category == 'thread' or category == 'processor')",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_not_predicate() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        // All processors: none of them have Timing_Properties::Period
        let expr = parse_check(
            "components.where(category == 'processor').all(not has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_empty_model() {
        // An empty model with just a root
        let mut components = Arena::<ComponentInstance>::default();
        let root_idx = components.alloc(ComponentInstance {
            name: Name::new("root"),
            category: ComponentCategory::System,
            type_name: Name::new("Empty"),
            impl_name: Some(Name::new("Impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });

        let inst = SystemInstance {
            root: root_idx,
            components,
            features: Arena::default(),
            connections: Arena::default(),
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };

        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &[],
        };

        // all() on empty set is vacuously true
        let expr = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b, "all() on empty set should be true"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // any() on empty set is false
        let expr = parse_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(!b, "any() on empty set should be false"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // none() on empty set is vacuously true
        let expr = parse_check(
            "components.where(category == 'thread').none(has('Timing_Properties::Period'))",
        )
        .unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Bool(b) => assert!(b, "none() on empty set should be true"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // count on empty set is 0
        let expr =
            parse_check("components.where(category == 'thread').count()").unwrap();
        match eval_expr(&expr, &ctx).unwrap() {
            Value::Count(n) => assert_eq!(n, 0),
            other => panic!("expected Count, got {:?}", other),
        }
    }

    // ── evaluate_assertions integration test ────────────────────────

    #[test]
    fn evaluate_assertions_pass_and_fail() {
        let inst = make_test_instance();
        let diags = make_test_diagnostics();
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };

        let assertions = vec![
            Assertion {
                id: "ASSERT-001".to_string(),
                description: "At least one thread exists".to_string(),
                check: "components.where(category == 'thread').any(category == 'thread')"
                    .to_string(),
                severity: SeverityFilter::Error,
            },
            Assertion {
                id: "ASSERT-002".to_string(),
                description: "All threads have Period".to_string(),
                check: "components.where(category == 'thread').all(has('Timing_Properties::Period'))".to_string(),
                severity: SeverityFilter::Error,
            },
        ];

        let results = evaluate_assertions(&assertions, &ctx);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, crate::verify::Status::Pass);
        assert_eq!(results[1].status, crate::verify::Status::Fail);
    }

    #[test]
    fn evaluate_assertions_parse_error() {
        let inst = make_test_instance();
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &[],
        };

        let assertions = vec![Assertion {
            id: "ASSERT-BAD".to_string(),
            description: "Invalid expression".to_string(),
            check: "foobar.baz()".to_string(),
            severity: SeverityFilter::Error,
        }];

        let results = evaluate_assertions(&assertions, &ctx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, crate::verify::Status::Fail);
        assert!(results[0].detail.contains("parse error"));
    }

    // ── Category matching tests ─────────────────────────────────────

    #[test]
    fn category_matching_kebab_case() {
        assert!(category_matches(&ComponentCategory::ThreadGroup, "thread-group"));
        assert!(category_matches(&ComponentCategory::ThreadGroup, "thread_group"));
        assert!(category_matches(&ComponentCategory::VirtualProcessor, "virtual-processor"));
        assert!(category_matches(&ComponentCategory::VirtualBus, "virtual-bus"));
    }

    #[test]
    fn category_matching_case_insensitive() {
        assert!(category_matches(&ComponentCategory::Thread, "Thread"));
        assert!(category_matches(&ComponentCategory::Thread, "THREAD"));
        assert!(category_matches(&ComponentCategory::System, "System"));
    }

    // ── Feature kind matching tests ─────────────────────────────────

    #[test]
    fn feature_kind_matching() {
        assert!(feature_kind_matches(&FeatureKind::DataPort, "data_port"));
        assert!(feature_kind_matches(&FeatureKind::DataPort, "dataport"));
        assert!(feature_kind_matches(&FeatureKind::EventPort, "event_port"));
        assert!(feature_kind_matches(&FeatureKind::EventDataPort, "event_data_port"));
        assert!(feature_kind_matches(
            &FeatureKind::SubprogramAccess,
            "subprogram_access"
        ));
    }

    // ── Direction matching tests ────────────────────────────────────

    #[test]
    fn direction_matching() {
        assert!(direction_matches(Some(&Direction::In), "in"));
        assert!(direction_matches(Some(&Direction::Out), "out"));
        assert!(direction_matches(Some(&Direction::InOut), "in_out"));
        assert!(direction_matches(Some(&Direction::InOut), "inout"));
        assert!(direction_matches(None, "none"));
        assert!(!direction_matches(Some(&Direction::In), "out"));
    }

    // ── Assertion result for TOML parsing ───────────────────────────

    #[test]
    fn parse_assertion_toml() {
        let toml_str = r#"
[[assertion]]
id = "ASSERT-TIMING-001"
description = "All threads must have Period"
check = "components.where(category == 'thread').all(has('Timing_Properties::Period'))"
severity = "error"

[[assertion]]
id = "ASSERT-CONN-001"
description = "All data ports must be connected"
check = "components.where(category == 'thread').features.where(kind == 'data_port' and direction == 'out').all(connected)"
severity = "warning"
"#;
        // Parse just the assertions
        #[derive(Debug, Deserialize)]
        struct TestFile {
            #[serde(default)]
            assertion: Vec<Assertion>,
        }
        let file: TestFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.assertion.len(), 2);
        assert_eq!(file.assertion[0].id, "ASSERT-TIMING-001");
        assert_eq!(file.assertion[0].severity, SeverityFilter::Error);
        assert_eq!(file.assertion[1].id, "ASSERT-CONN-001");
        assert_eq!(file.assertion[1].severity, SeverityFilter::Warning);
    }

    #[test]
    fn parse_assertion_toml_defaults() {
        let toml_str = r#"
[[assertion]]
id = "ASSERT-001"
check = "components.count()"
"#;
        #[derive(Debug, Deserialize)]
        struct TestFile {
            #[serde(default)]
            assertion: Vec<Assertion>,
        }
        let file: TestFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.assertion.len(), 1);
        assert!(file.assertion[0].description.is_empty());
        assert_eq!(file.assertion[0].severity, SeverityFilter::Error);
    }
}
