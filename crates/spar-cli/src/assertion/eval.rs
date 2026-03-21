//! Evaluator for the assertion expression language.
//!
//! Walks rowan typed CST nodes (`SyntaxNode`) instead of the old enum-based AST.

use std::fmt;

use spar_analysis::{AnalysisDiagnostic, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, FeatureInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};

use super::syntax::{ExprSyntaxKind, SyntaxNode};

// ── Value type ──────────────────────────────────────────────────────

/// Intermediate value during evaluation.
#[derive(Debug)]
pub(crate) enum Value {
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

// ── Top-level evaluator ─────────────────────────────────────────────

/// Evaluate a parsed expression tree against a context.
pub(crate) fn eval_node(node: &SyntaxNode, ctx: &EvalContext) -> Result<Value, EvalError> {
    match node.kind() {
        ExprSyntaxKind::ROOT => {
            // Evaluate the single child expression
            for child in node.children() {
                return eval_node(&child, ctx);
            }
            Err(EvalError {
                message: "empty expression".to_string(),
            })
        }
        ExprSyntaxKind::PIPELINE_EXPR => eval_pipeline(node, ctx),
        _ => Err(EvalError {
            message: format!("unexpected top-level node: {:?}", node.kind()),
        }),
    }
}

/// Evaluate a pipeline expression: source followed by method calls.
fn eval_pipeline(node: &SyntaxNode, ctx: &EvalContext) -> Result<Value, EvalError> {
    let mut children = node.children();

    // First child is the source
    let source_node = children.next().ok_or_else(|| EvalError {
        message: "empty pipeline".to_string(),
    })?;

    let mut value = eval_source(&source_node, ctx)?;

    // Remaining children are DOT_CALL nodes
    for child in children {
        if child.kind() == ExprSyntaxKind::DOT_CALL {
            value = eval_dot_call(&child, value, ctx)?;
        }
    }

    Ok(value)
}

/// Evaluate a source expression: `components` or `analysis('name')`.
fn eval_source(node: &SyntaxNode, ctx: &EvalContext) -> Result<Value, EvalError> {
    match node.kind() {
        ExprSyntaxKind::IDENT_EXPR => {
            let text = node_text(node);
            match text.as_str() {
                "components" => {
                    let indices: Vec<_> =
                        ctx.instance.all_components().map(|(idx, _)| idx).collect();
                    Ok(Value::Components(indices))
                }
                other => Err(EvalError {
                    message: format!("unknown source: '{}'", other),
                }),
            }
        }
        ExprSyntaxKind::CALL_EXPR => {
            // analysis('name')
            let func_name = first_token_text(node);
            if func_name == "analysis" {
                let arg = find_string_literal(node)?;
                let filtered: Vec<_> = ctx
                    .diagnostics
                    .iter()
                    .filter(|d| d.analysis == arg)
                    .cloned()
                    .collect();
                Ok(Value::Diagnostics(filtered))
            } else {
                Err(EvalError {
                    message: format!("unknown function: '{}'", func_name),
                })
            }
        }
        _ => Err(EvalError {
            message: format!("unexpected source node: {:?}", node.kind()),
        }),
    }
}

/// Evaluate a DOT_CALL: `.method(args)` or `.field`.
fn eval_dot_call(
    node: &SyntaxNode,
    value: Value,
    ctx: &EvalContext,
) -> Result<Value, EvalError> {
    // The DOT_CALL has tokens: DOT, IDENT (method name)
    // and optionally a CALL_ARGS child node.
    let method_name = get_method_name(node);
    let args_node = node
        .children()
        .find(|c| c.kind() == ExprSyntaxKind::CALL_ARGS);

    match method_name.as_str() {
        "where" => {
            let pred = args_node.as_ref().ok_or_else(|| EvalError {
                message: "where() requires arguments".to_string(),
            })?;
            eval_where(pred, value, ctx)
        }
        "all" => {
            let pred = args_node.as_ref().ok_or_else(|| EvalError {
                message: "all() requires arguments".to_string(),
            })?;
            eval_quantifier(pred, value, ctx, Quantifier::All)
        }
        "any" => {
            let pred = args_node.as_ref().ok_or_else(|| EvalError {
                message: "any() requires arguments".to_string(),
            })?;
            eval_quantifier(pred, value, ctx, Quantifier::Any)
        }
        "none" => {
            let pred = args_node.as_ref().ok_or_else(|| EvalError {
                message: "none() requires arguments".to_string(),
            })?;
            eval_quantifier(pred, value, ctx, Quantifier::None)
        }
        "count" => match value {
            Value::Components(comps) => Ok(Value::Count(comps.len())),
            Value::Features(feats) => Ok(Value::Count(feats.len())),
            Value::Diagnostics(diags) => Ok(Value::Count(diags.len())),
            _ => Err(EvalError {
                message: "count() can only be applied to components, features, or diagnostics"
                    .to_string(),
            }),
        },
        "features" => match value {
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
        "diagnostics" => match value {
            Value::Diagnostics(_) => Ok(value),
            _ => Err(EvalError {
                message: "diagnostics can only be accessed on analysis results".to_string(),
            }),
        },
        other => Err(EvalError {
            message: format!("unknown method: '{}'", other),
        }),
    }
}

// ── Where/quantifier evaluation ─────────────────────────────────────

enum Quantifier {
    All,
    Any,
    None,
}

fn eval_where(
    args_node: &SyntaxNode,
    value: Value,
    ctx: &EvalContext,
) -> Result<Value, EvalError> {
    let pred_node = find_predicate_in_args(args_node)?;

    match value {
        Value::Components(comps) => {
            let filtered: Vec<_> = comps
                .into_iter()
                .filter(|&idx| eval_component_predicate(&pred_node, idx, ctx))
                .collect();
            Ok(Value::Components(filtered))
        }
        Value::Features(feats) => {
            let filtered: Vec<_> = feats
                .into_iter()
                .filter(|&(comp_idx, feat_idx)| {
                    eval_feature_predicate(&pred_node, comp_idx, feat_idx, ctx)
                })
                .collect();
            Ok(Value::Features(filtered))
        }
        Value::Diagnostics(diags) => {
            let filtered: Vec<_> = diags
                .into_iter()
                .filter(|d| eval_diagnostic_predicate(&pred_node, d))
                .collect();
            Ok(Value::Diagnostics(filtered))
        }
        _ => Err(EvalError {
            message: "where() can only be applied to components, features, or diagnostics"
                .to_string(),
        }),
    }
}

fn eval_quantifier(
    args_node: &SyntaxNode,
    value: Value,
    ctx: &EvalContext,
    quantifier: Quantifier,
) -> Result<Value, EvalError> {
    let pred_node = find_predicate_in_args(args_node)?;

    let type_name = match &value {
        Value::Components(_) => "components",
        Value::Features(_) => "features",
        Value::Diagnostics(_) => "diagnostics",
        _ => {
            return Err(EvalError {
                message: "quantifier can only be applied to components, features, or diagnostics"
                    .to_string(),
            })
        }
    };

    let result = match (&quantifier, value) {
        (_, Value::Components(comps)) => {
            let iter = comps
                .iter()
                .map(|&idx| eval_component_predicate(&pred_node, idx, ctx));
            match quantifier {
                Quantifier::All => iter.fold(true, |acc, v| acc && v),
                Quantifier::Any => iter.fold(false, |acc, v| acc || v),
                Quantifier::None => !iter.fold(false, |acc, v| acc || v),
            }
        }
        (_, Value::Features(feats)) => {
            let iter = feats
                .iter()
                .map(|&(c, f)| eval_feature_predicate(&pred_node, c, f, ctx));
            match quantifier {
                Quantifier::All => iter.fold(true, |acc, v| acc && v),
                Quantifier::Any => iter.fold(false, |acc, v| acc || v),
                Quantifier::None => !iter.fold(false, |acc, v| acc || v),
            }
        }
        (_, Value::Diagnostics(diags)) => {
            let iter = diags
                .iter()
                .map(|d| eval_diagnostic_predicate(&pred_node, d));
            match quantifier {
                Quantifier::All => iter.fold(true, |acc, v| acc && v),
                Quantifier::Any => iter.fold(false, |acc, v| acc || v),
                Quantifier::None => !iter.fold(false, |acc, v| acc || v),
            }
        }
        _ => {
            return Err(EvalError {
                message: format!(
                    "quantifier can only be applied to {}", type_name
                ),
            })
        }
    };

    Ok(Value::Bool(result))
}

// ── Predicate evaluation ────────────────────────────────────────────

fn eval_component_predicate(
    node: &SyntaxNode,
    idx: ComponentInstanceIdx,
    ctx: &EvalContext,
) -> bool {
    match node.kind() {
        ExprSyntaxKind::BINARY_EXPR => {
            let (op, children) = parse_binary_expr(node);
            match op {
                BinaryOp::And => children
                    .iter()
                    .all(|c| eval_component_predicate(c, idx, ctx)),
                BinaryOp::Or => children
                    .iter()
                    .any(|c| eval_component_predicate(c, idx, ctx)),
            }
        }
        ExprSyntaxKind::UNARY_EXPR => {
            let inner = node.children().next().unwrap();
            !eval_component_predicate(&inner, idx, ctx)
        }
        ExprSyntaxKind::PAREN_EXPR => {
            let inner = find_inner_expr(node);
            eval_component_predicate(&inner, idx, ctx)
        }
        ExprSyntaxKind::COMPARE_EXPR => {
            let (field, value) = parse_compare(node);
            match field.as_str() {
                "category" => {
                    let comp = ctx.instance.component(idx);
                    category_matches(&comp.category, &value)
                }
                // Feature-level predicates don't apply to components
                "kind" | "direction" => false,
                // Diagnostic-level predicates don't apply to components
                "severity" | "message" => false,
                _ => false,
            }
        }
        ExprSyntaxKind::CALL_EXPR => {
            let func_name = first_token_text(node);
            if func_name == "has" {
                let prop_name = find_string_literal(node).unwrap_or_default();
                let props = ctx.instance.properties_for(idx);
                if let Some((set, name)) = prop_name.split_once("::") {
                    props.get(set, name).is_some()
                } else {
                    props.get("", &prop_name).is_some()
                }
            } else {
                false
            }
        }
        ExprSyntaxKind::IDENT_EXPR => {
            let text = node_text(node);
            match text.as_str() {
                "connected" => {
                    let comp = ctx.instance.component(idx);
                    !comp.connections.is_empty()
                        || comp
                            .parent
                            .map(|p| !ctx.instance.component(p).connections.is_empty())
                            .unwrap_or(false)
                }
                _ => false,
            }
        }
        ExprSyntaxKind::CONTAINS_EXPR => {
            // message.contains(...) doesn't apply to components
            false
        }
        _ => false,
    }
}

fn eval_feature_predicate(
    node: &SyntaxNode,
    comp_idx: ComponentInstanceIdx,
    feat_idx: FeatureInstanceIdx,
    ctx: &EvalContext,
) -> bool {
    match node.kind() {
        ExprSyntaxKind::BINARY_EXPR => {
            let (op, children) = parse_binary_expr(node);
            match op {
                BinaryOp::And => children
                    .iter()
                    .all(|c| eval_feature_predicate(c, comp_idx, feat_idx, ctx)),
                BinaryOp::Or => children
                    .iter()
                    .any(|c| eval_feature_predicate(c, comp_idx, feat_idx, ctx)),
            }
        }
        ExprSyntaxKind::UNARY_EXPR => {
            let inner = node.children().next().unwrap();
            !eval_feature_predicate(&inner, comp_idx, feat_idx, ctx)
        }
        ExprSyntaxKind::PAREN_EXPR => {
            let inner = find_inner_expr(node);
            eval_feature_predicate(&inner, comp_idx, feat_idx, ctx)
        }
        ExprSyntaxKind::COMPARE_EXPR => {
            let (field, value) = parse_compare(node);
            match field.as_str() {
                "kind" => {
                    let feat = &ctx.instance.features[feat_idx];
                    feature_kind_matches(&feat.kind, &value)
                }
                "direction" => {
                    let feat = &ctx.instance.features[feat_idx];
                    direction_matches(feat.direction.as_ref(), &value)
                }
                "category" => {
                    let comp = ctx.instance.component(comp_idx);
                    category_matches(&comp.category, &value)
                }
                _ => false,
            }
        }
        ExprSyntaxKind::CALL_EXPR => {
            let func_name = first_token_text(node);
            if func_name == "has" {
                let prop_name = find_string_literal(node).unwrap_or_default();
                let props = ctx.instance.properties_for(comp_idx);
                if let Some((set, name)) = prop_name.split_once("::") {
                    props.get(set, name).is_some()
                } else {
                    props.get("", &prop_name).is_some()
                }
            } else {
                false
            }
        }
        ExprSyntaxKind::IDENT_EXPR => {
            let text = node_text(node);
            match text.as_str() {
                "connected" => {
                    let comp = ctx.instance.component(comp_idx);
                    let owner_has_conns = !comp.connections.is_empty();
                    let parent_has_conns = comp
                        .parent
                        .map(|p| !ctx.instance.component(p).connections.is_empty())
                        .unwrap_or(false);

                    let feat = &ctx.instance.features[feat_idx];
                    let feat_name = feat.name.as_str();

                    let specifically_connected =
                        ctx.instance.connections.iter().any(|(_, conn)| {
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
                _ => false,
            }
        }
        ExprSyntaxKind::CONTAINS_EXPR => {
            // message.contains(...) doesn't apply to features
            false
        }
        _ => false,
    }
}

fn eval_diagnostic_predicate(node: &SyntaxNode, diag: &AnalysisDiagnostic) -> bool {
    match node.kind() {
        ExprSyntaxKind::BINARY_EXPR => {
            let (op, children) = parse_binary_expr(node);
            match op {
                BinaryOp::And => children
                    .iter()
                    .all(|c| eval_diagnostic_predicate(c, diag)),
                BinaryOp::Or => children
                    .iter()
                    .any(|c| eval_diagnostic_predicate(c, diag)),
            }
        }
        ExprSyntaxKind::UNARY_EXPR => {
            let inner = node.children().next().unwrap();
            !eval_diagnostic_predicate(&inner, diag)
        }
        ExprSyntaxKind::PAREN_EXPR => {
            let inner = find_inner_expr(node);
            eval_diagnostic_predicate(&inner, diag)
        }
        ExprSyntaxKind::COMPARE_EXPR => {
            let (field, value) = parse_compare(node);
            match field.as_str() {
                "severity" => {
                    let sev_str = match diag.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                        Severity::Info => "info",
                    };
                    sev_str == value.as_str()
                }
                // Component/feature predicates don't apply to diagnostics
                "category" | "kind" | "direction" => false,
                _ => false,
            }
        }
        ExprSyntaxKind::CONTAINS_EXPR => {
            let text = find_contains_text(node);
            diag.message.contains(text.as_str())
        }
        ExprSyntaxKind::IDENT_EXPR => {
            let text = node_text(node);
            match text.as_str() {
                "connected" | _ => false,
            }
        }
        ExprSyntaxKind::CALL_EXPR => {
            // has(...) doesn't apply to diagnostics
            false
        }
        _ => false,
    }
}

// ── Matching helpers ────────────────────────────────────────────────

pub(crate) fn category_matches(cat: &ComponentCategory, val: &str) -> bool {
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

pub(crate) fn feature_kind_matches(kind: &FeatureKind, val: &str) -> bool {
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

pub(crate) fn direction_matches(dir: Option<&Direction>, val: &str) -> bool {
    let normalized = val.to_lowercase().replace('-', "_");
    match (dir, normalized.as_str()) {
        (Some(Direction::In), "in") => true,
        (Some(Direction::Out), "out") => true,
        (Some(Direction::InOut), "in_out" | "inout") => true,
        (None, "none") => true,
        _ => false,
    }
}

// ── Node inspection helpers ─────────────────────────────────────────

/// Get the trimmed text content of a node (all descendant tokens concatenated).
fn node_text(node: &SyntaxNode) -> String {
    // Collect all tokens, trimming whitespace
    let mut s = String::new();
    for token in node.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(t) = token {
            if t.kind() != ExprSyntaxKind::WHITESPACE {
                s.push_str(t.text());
            }
        }
    }
    s
}

/// Get the text of the first non-whitespace token in a node.
fn first_token_text(node: &SyntaxNode) -> String {
    for token in node.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(t) = token {
            if t.kind() != ExprSyntaxKind::WHITESPACE {
                return t.text().to_string();
            }
        }
    }
    String::new()
}

/// Find the string literal value inside a node (strips quotes).
fn find_string_literal(node: &SyntaxNode) -> Result<String, EvalError> {
    for child in node.children() {
        if child.kind() == ExprSyntaxKind::LITERAL {
            for token in child.descendants_with_tokens() {
                if let rowan::NodeOrToken::Token(t) = token {
                    if t.kind() == ExprSyntaxKind::STRING_LIT {
                        let text = t.text();
                        // Strip surrounding quotes
                        let inner = text
                            .strip_prefix('\'')
                            .and_then(|s| s.strip_suffix('\''))
                            .unwrap_or(text);
                        return Ok(inner.to_string());
                    }
                }
            }
        }
    }
    Err(EvalError {
        message: "expected string literal".to_string(),
    })
}

/// Get the method name from a DOT_CALL node (the IDENT token after the DOT).
fn get_method_name(node: &SyntaxNode) -> String {
    let mut found_dot = false;
    for token in node.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(t) = token {
            match t.kind() {
                ExprSyntaxKind::DOT => found_dot = true,
                ExprSyntaxKind::IDENT if found_dot => return t.text().to_string(),
                ExprSyntaxKind::WHITESPACE => {}
                _ => {}
            }
        }
    }
    String::new()
}

/// Find the predicate expression node inside a CALL_ARGS node.
/// The predicate is the first child node (between L_PAREN and R_PAREN).
fn find_predicate_in_args(args_node: &SyntaxNode) -> Result<SyntaxNode, EvalError> {
    for child in args_node.children() {
        // Return the first child that's a meaningful expression node
        match child.kind() {
            ExprSyntaxKind::BINARY_EXPR
            | ExprSyntaxKind::UNARY_EXPR
            | ExprSyntaxKind::COMPARE_EXPR
            | ExprSyntaxKind::CALL_EXPR
            | ExprSyntaxKind::PAREN_EXPR
            | ExprSyntaxKind::IDENT_EXPR
            | ExprSyntaxKind::CONTAINS_EXPR => return Ok(child),
            _ => {}
        }
    }
    Err(EvalError {
        message: "no predicate found in arguments".to_string(),
    })
}

enum BinaryOp {
    And,
    Or,
}

/// Parse a BINARY_EXPR: determine operator and collect operand child nodes.
fn parse_binary_expr(node: &SyntaxNode) -> (BinaryOp, Vec<SyntaxNode>) {
    let mut op = BinaryOp::And; // default
    let mut children = Vec::new();

    for elem in node.children_with_tokens() {
        match elem {
            rowan::NodeOrToken::Token(t) => {
                if t.kind() == ExprSyntaxKind::AND_KW {
                    op = BinaryOp::And;
                } else if t.kind() == ExprSyntaxKind::OR_KW {
                    op = BinaryOp::Or;
                }
            }
            rowan::NodeOrToken::Node(n) => {
                children.push(n);
            }
        }
    }

    (op, children)
}

/// Find the inner expression in a PAREN_EXPR (the child between parens).
fn find_inner_expr(node: &SyntaxNode) -> SyntaxNode {
    for child in node.children() {
        match child.kind() {
            ExprSyntaxKind::BINARY_EXPR
            | ExprSyntaxKind::UNARY_EXPR
            | ExprSyntaxKind::COMPARE_EXPR
            | ExprSyntaxKind::CALL_EXPR
            | ExprSyntaxKind::PAREN_EXPR
            | ExprSyntaxKind::IDENT_EXPR
            | ExprSyntaxKind::CONTAINS_EXPR => return child,
            _ => {}
        }
    }
    // Fallback: return first child
    node.children().next().unwrap()
}

/// Extract the field name and value from a COMPARE_EXPR (field == 'value').
fn parse_compare(node: &SyntaxNode) -> (String, String) {
    let mut field = String::new();
    let mut value = String::new();

    for elem in node.children_with_tokens() {
        match elem {
            rowan::NodeOrToken::Token(t) => {
                if t.kind() == ExprSyntaxKind::IDENT && field.is_empty() {
                    field = t.text().to_string();
                }
            }
            rowan::NodeOrToken::Node(n) => {
                if n.kind() == ExprSyntaxKind::LITERAL {
                    for token in n.descendants_with_tokens() {
                        if let rowan::NodeOrToken::Token(t) = token {
                            if t.kind() == ExprSyntaxKind::STRING_LIT {
                                let text = t.text();
                                let inner = text
                                    .strip_prefix('\'')
                                    .and_then(|s| s.strip_suffix('\''))
                                    .unwrap_or(text);
                                value = inner.to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    (field, value)
}

/// Extract the search text from a CONTAINS_EXPR (message.contains('text')).
fn find_contains_text(node: &SyntaxNode) -> String {
    for child in node.children() {
        if child.kind() == ExprSyntaxKind::LITERAL {
            for token in child.descendants_with_tokens() {
                if let rowan::NodeOrToken::Token(t) = token {
                    if t.kind() == ExprSyntaxKind::STRING_LIT {
                        let text = t.text();
                        let inner = text
                            .strip_prefix('\'')
                            .and_then(|s| s.strip_suffix('\''))
                            .unwrap_or(text);
                        return inner.to_string();
                    }
                }
            }
        }
    }
    String::new()
}
