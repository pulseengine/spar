//! Behavior Annex (BA) parser.
//!
//! Specification: SAE AS5506/2 Annex D -- Behavior Annex
//!   Full title: "Architecture Analysis & Design Language (AADL)
//!                AS5506/2 Annex D: Behavior Model Annex"
//!   Published by: SAE International
//!
//! Reference implementation: OSATE2 org.osate.ba
//!   Source:  https://github.com/osate/osate2/tree/master/ba
//!
//! The Behavior Annex defines behavior state machines for AADL
//! components, including states, transitions with dispatch/execute
//! conditions, actions (assignment, communication, computation),
//! and control flow (if/elsif/else, for, forall, while, do-until).

pub mod syntax_kind;
mod lexer;
mod parser;
mod grammar;

use std::mem;

pub use syntax_kind::{BaKind, BaLanguage, BaSyntaxNode, BaSyntaxToken};

use crate::{AnnexParser, AnnexParseResult, AnnexNode, AnnexDiagnostic, Span, Severity};

/// Result of parsing BA annex content.
pub struct BaParse {
    green: rowan::GreenNode,
    errors: Vec<BaError>,
}

/// An error from BA parsing.
#[derive(Debug, Clone)]
pub struct BaError {
    pub msg: String,
    pub offset: usize,
}

impl BaParse {
    /// Build a typed syntax node root.
    pub fn syntax_node(&self) -> BaSyntaxNode {
        BaSyntaxNode::new_root(self.green.clone())
    }

    /// Return parse errors.
    pub fn errors(&self) -> &[BaError] {
        &self.errors
    }

    /// Returns true if parsing succeeded without errors.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Convert to the flat AnnexParseResult representation.
    pub fn to_annex_result(&self) -> AnnexParseResult {
        let root = self.syntax_node();
        let mut nodes = Vec::new();
        flatten_node(&root, -1, &mut nodes);

        let diagnostics = self
            .errors
            .iter()
            .map(|e| AnnexDiagnostic {
                span: Span::new(e.offset as u32, e.offset as u32),
                message: e.msg.clone(),
                severity: Severity::Error,
            })
            .collect();

        AnnexParseResult { nodes, diagnostics }
    }
}

/// Flatten a Rowan tree into AnnexNode pre-order list.
fn flatten_node(node: &BaSyntaxNode, parent: i32, out: &mut Vec<AnnexNode>) {
    let idx = out.len() as i32;
    out.push(AnnexNode {
        kind: format!("{:?}", node.kind()),
        span: Span::new(
            u32::from(node.text_range().start()),
            u32::from(node.text_range().end()),
        ),
        parent,
        text: String::new(),
    });

    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => {
                flatten_node(&n, idx, out);
            }
            rowan::NodeOrToken::Token(t) => {
                if !t.kind().is_trivia() {
                    out.push(AnnexNode {
                        kind: format!("{:?}", t.kind()),
                        span: Span::new(
                            u32::from(t.text_range().start()),
                            u32::from(t.text_range().end()),
                        ),
                        parent: idx,
                        text: t.text().to_string(),
                    });
                }
            }
        }
    }
}

/// Parse BA source text (the content between `{**` and `**}`).
pub fn parse(source: &str) -> BaParse {
    let tokens = lexer::tokenize(source);
    let mut p = parser::Parser::new(&tokens, source);
    grammar::root(&mut p);
    let events = p.finish();
    build_tree(source, &tokens, events)
}

// -- Tree builder (adapted from spar-syntax/EMV2) --

enum ResolvedEvent {
    StartNode(BaKind),
    Token { kind: BaKind, n_raw_tokens: u8 },
    FinishNode,
    Error(String),
}

fn resolve_events(mut events: Vec<parser::Event>) -> Vec<ResolvedEvent> {
    let mut resolved = Vec::with_capacity(events.len());
    let mut forward_parents = Vec::new();

    for i in 0..events.len() {
        match mem::replace(&mut events[i], parser::Event::Tombstone) {
            parser::Event::Start {
                kind,
                forward_parent,
            } => {
                forward_parents.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(fwd) = fp {
                    idx += fwd as usize;
                    fp = match mem::replace(&mut events[idx], parser::Event::Tombstone) {
                        parser::Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            forward_parents.push(kind);
                            forward_parent
                        }
                        _ => unreachable!(),
                    };
                }
                for kind in forward_parents.drain(..).rev() {
                    if kind != BaKind::TOMBSTONE {
                        resolved.push(ResolvedEvent::StartNode(kind));
                    }
                }
            }
            parser::Event::Finish => resolved.push(ResolvedEvent::FinishNode),
            parser::Event::Token { kind, n_raw_tokens } => {
                resolved.push(ResolvedEvent::Token { kind, n_raw_tokens });
            }
            parser::Event::Error { msg } => resolved.push(ResolvedEvent::Error(msg)),
            parser::Event::Tombstone => {}
        }
    }

    resolved
}

fn build_tree(
    input: &str,
    tokens: &[(BaKind, usize)],
    events: Vec<parser::Event>,
) -> BaParse {
    let mut builder = rowan::GreenNodeBuilder::new();
    let mut errors = Vec::new();

    let token_starts: Vec<usize> = {
        let mut starts = Vec::with_capacity(tokens.len());
        let mut offset = 0usize;
        for &(_, len) in tokens {
            starts.push(offset);
            offset += len;
        }
        starts
    };

    let non_trivia: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, (kind, _))| !kind.is_trivia())
        .map(|(i, _)| i)
        .collect();

    let resolved = resolve_events(events);
    let mut nt_pos: usize = 0;
    let mut raw_pos: usize = 0;
    let mut depth: usize = 0;

    for event in &resolved {
        match event {
            ResolvedEvent::StartNode(kind) => {
                if depth > 0 {
                    eat_trivia(
                        &mut builder,
                        tokens,
                        input,
                        &token_starts,
                        &mut raw_pos,
                        nt_pos,
                        &non_trivia,
                    );
                }
                depth += 1;
                builder.start_node((*kind).into());
                if depth == 1 {
                    eat_trivia(
                        &mut builder,
                        tokens,
                        input,
                        &token_starts,
                        &mut raw_pos,
                        nt_pos,
                        &non_trivia,
                    );
                }
            }
            ResolvedEvent::Token { kind, n_raw_tokens } => {
                for _ in 0..*n_raw_tokens {
                    if nt_pos < non_trivia.len() {
                        let target = non_trivia[nt_pos];
                        while raw_pos < target {
                            let (tk, len) = tokens[raw_pos];
                            let start = token_starts[raw_pos];
                            builder.token(tk.into(), &input[start..start + len]);
                            raw_pos += 1;
                        }
                        let (_, len) = tokens[target];
                        let start = token_starts[target];
                        builder.token((*kind).into(), &input[start..start + len]);
                        raw_pos = target + 1;
                        nt_pos += 1;
                    }
                }
            }
            ResolvedEvent::FinishNode => {
                depth -= 1;
                if depth == 0 {
                    while raw_pos < tokens.len() {
                        let (tk, len) = tokens[raw_pos];
                        let start = token_starts[raw_pos];
                        builder.token(tk.into(), &input[start..start + len]);
                        raw_pos += 1;
                    }
                }
                builder.finish_node();
            }
            ResolvedEvent::Error(msg) => {
                let offset = if nt_pos < non_trivia.len() {
                    token_starts[non_trivia[nt_pos]]
                } else if !tokens.is_empty() {
                    let last = tokens.len() - 1;
                    token_starts[last] + tokens[last].1
                } else {
                    0
                };
                errors.push(BaError {
                    msg: msg.clone(),
                    offset,
                });
            }
        }
    }

    BaParse {
        green: builder.finish(),
        errors,
    }
}

fn eat_trivia(
    builder: &mut rowan::GreenNodeBuilder,
    tokens: &[(BaKind, usize)],
    input: &str,
    token_starts: &[usize],
    raw_pos: &mut usize,
    nt_pos: usize,
    non_trivia: &[usize],
) {
    let target = if nt_pos < non_trivia.len() {
        non_trivia[nt_pos]
    } else {
        return;
    };
    while *raw_pos < target {
        let (tk, len) = tokens[*raw_pos];
        let start = token_starts[*raw_pos];
        builder.token(tk.into(), &input[start..start + len]);
        *raw_pos += 1;
    }
}

// -- AnnexParser trait implementation --

/// Built-in parser for the Behavior Annex.
pub struct BaAnnexParser;

impl AnnexParser for BaAnnexParser {
    fn names(&self) -> &[&str] {
        &["behavior_specification"]
    }

    fn parse(&self, _name: &str, source: &str) -> AnnexParseResult {
        let result = parse(source);
        result.to_annex_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Variable declarations ────────────────────────────────────

    #[test]
    fn parse_simple_variable() {
        let src = "variables tmp : number;";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
        let root = result.syntax_node();
        assert_eq!(root.kind(), BaKind::BA_ROOT);
    }

    #[test]
    fn parse_multiple_variables() {
        let src = "variables x, y : data_type;";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_variable_with_classifier() {
        let src = "variables v : classifier(pkg::data_type.impl);";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_variable_with_qualified_type() {
        let src = "variables v : Base_Types::Integer;";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── State declarations ───────────────────────────────────────

    #[test]
    fn parse_single_state() {
        let src = "states s0 : initial state;";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_combined_state_qualifiers() {
        let src = "states s0 : initial complete final state;";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_multiple_states() {
        let src = r#"
            states
                s0 : initial complete state;
                s1, s2 : state;
                sf : complete final state;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Simple transitions ───────────────────────────────────────

    #[test]
    fn parse_unconditional_transition() {
        let src = r#"
            states
                s0 : initial state;
                s1 : final state;
            transitions
                s0 -[ ]-> s1;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_named_transition() {
        let src = r#"
            states
                s0 : initial state;
                s1 : final state;
            transitions
                t1 : s0 -[ ]-> s1;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_dispatch_transition() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_dispatch_port_trigger() {
        let src = r#"
            states
                s0 : initial complete state;
                s1 : state;
            transitions
                s0 -[ on dispatch p1 ]-> s1;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_dispatch_timeout() {
        let src = r#"
            states
                st : initial complete state;
            transitions
                st -[ on dispatch timeout ]-> st;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_execute_condition() {
        let src = r#"
            states
                s0 : initial state;
                s1 : state;
                sf : final state;
            transitions
                s0 -[ x = 1 ]-> sf;
                s0 -[ x = 0 ]-> s1;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Transitions with action blocks ───────────────────────────

    #[test]
    fn parse_assignment_action() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0 { sp := tick'count };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_communication_send_event() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ ]-> s0 { overflow! };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_communication_send_data() {
        let src = r#"
            states
                st : initial complete state;
            transitions
                st -[ on dispatch timeout ]-> st { d!(1) };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_subprogram_call() {
        let src = r#"
            variables tmp : number;
            states
                s : initial final state;
            transitions
                t : s -[ ]-> s { mul!(x,x,tmp); mul!(tmp,x,y) };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_sequential_actions() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ ]-> s0 { x := 1; y := 2; z := x + y };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_dotted_reference_in_action() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ this.sp <= 100 ]-> s0 { this.elems[this.sp] := v; this.sp := this.sp + 1 };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── If/else control flow ─────────────────────────────────────

    #[test]
    fn parse_if_else() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0 {
                    if (x > 0)
                        y := 1
                    else
                        y := 0
                    end if
                };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_if_elsif_else() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0 {
                    if (x > 10)
                        y := 2
                    elsif (x > 0)
                        y := 1
                    else
                        y := 0
                    end if
                };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── For loops ────────────────────────────────────────────────

    #[test]
    fn parse_for_loop() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0 {
                    for (i : Base_Types::Integer in 0 .. 10) {
                        arr[i] := 0
                    }
                };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── While loops ──────────────────────────────────────────────

    #[test]
    fn parse_while_loop() {
        let src = r#"
            states
                s0 : initial complete final state;
            transitions
                s0 -[ on dispatch ]-> s0 {
                    while (x > 0) {
                        x := x - 1
                    }
                };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Expression precedence ────────────────────────────────────

    #[test]
    fn parse_arithmetic_expression() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ ]-> s0 { r := x + y };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_complex_expression() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ ]-> s0 { r := (a + b) * c - d / e };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_boolean_expression() {
        let src = r#"
            states
                s0 : initial state;
                s1 : final state;
            transitions
                s0 -[ a > 0 and b < 10 ]-> s1;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_unary_expression() {
        let src = r#"
            states
                s0 : initial final state;
            transitions
                s0 -[ not flag ]-> s0;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Computation action ───────────────────────────────────────

    #[test]
    fn parse_computation_action() {
        let src = r#"
            states
                s : initial complete final state;
            transitions
                s -[ on dispatch ]-> s { computation(60ms) };
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Lossless round-trip ──────────────────────────────────────

    #[test]
    fn lossless_round_trip() {
        let src = "states\n  s0: initial state;\ntransitions\n  s0 -[]-> s0;";
        let result = parse(src);
        let root = result.syntax_node();
        assert_eq!(root.text().to_string(), src);
    }

    #[test]
    fn lossless_round_trip_with_actions() {
        let src = "states\n  s : initial final state;\ntransitions\n  s -[]-> s { x := 1 };";
        let result = parse(src);
        let root = result.syntax_node();
        assert_eq!(root.text().to_string(), src);
    }

    // ── AnnexParser trait ────────────────────────────────────────

    #[test]
    fn annex_parser_trait() {
        let parser = BaAnnexParser;
        assert_eq!(parser.names(), &["behavior_specification"]);
        let result = parser.parse(
            "behavior_specification",
            "states\n  s: initial state;\ntransitions\n  s -[]-> s;",
        );
        assert!(!result.has_errors());
        assert!(!result.nodes.is_empty());
    }

    // ── OSATE2 standard examples ─────────────────────────────────

    #[test]
    fn parse_ba_example1_cube() {
        // From ba_example_001.aadl: cube subprogram with variable and subprogram calls
        let src = r#"
  variables tmp : number;
  states s : initial final state;
  transitions t : s -[]-> s { mul!(x,x,tmp); mul!(tmp,x,y) };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example2_speed() {
        // From ba_example_002.aadl: speed counter with port count
        let src = r#"
    states
      s0: initial complete final state;
    transitions
      s0 -[ on dispatch ]-> s0 { sp := tick'count };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example3_sender() {
        // From ba_example_003.aadl: sender with dispatch conditions
        let src = r#"
  states
    st: initial complete state;
    sf: complete final state;
    s1, s2: state;
  transitions
    st -[on dispatch timeout]-> st { v := 1; d!(v) };
    st -[on dispatch a ]-> s1;
    s1 -[a=1]-> sf;
    s1 -[a=0]-> st;
    sf -[on dispatch timeout]-> sf { v := 0; d!(v) };
    sf -[on dispatch a ]-> s2;
    s2 -[a=0]-> st;
    s2 -[a=1]-> sf;
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example4_addition() {
        // From ba_example_004.aadl: addition with boolean flags
        let src = r#"
  states
    s0 : initial state;
    s1 : final state;
  transitions
    regular: s0 -[ ]-> s1 { r := x + y ; ovf := false };
    overflow: s0 -[ ]-> s1 { r := 0; ovf := true };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example5_stack_push() {
        // From ba_example_005.aadl: stack push with array indexing
        let src = r#"
    states
      s0 : initial final state;
    transitions
      s0 -[ this.sp <= 100 ]-> s0 { this.elems[this.sp] := v; this.sp := this.sp+1 };
      s0 -[ this.sp > 100 ]-> s0 { overflow! };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example5_stack_pop() {
        // From ba_example_005.aadl: stack pop
        let src = r#"
    states
      s0 : initial final state;
    transitions
      s0 -[ this.sp > 0 ]-> s0 { this.sp := this.sp - 1 ; v := this.elems[this.sp] };
      s0 -[ this.sp = 0 ]-> s0 { underflow! };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example6_merger() {
        // From ba_example_006.aadl: merger with port comparisons
        let src = r#"
  states
    s0 : initial complete state;
    comp : state;
    next1, next2 : complete final state;
  transitions
    s0 -[ on dispatch p1 ]-> next2 { x1 := p1 };
    s0 -[ on dispatch p2 ]-> next1 { x2 := p2 };
    next1 -[ on dispatch p1 ]-> comp { x1 := p1 };
    next2 -[ on dispatch p2 ]-> comp { x2 := p2 };
    comp -[ x1 < x2 ]-> next1 { m!(x1) };
    comp -[ x2 <= x1 ]-> next2 { m!(x2) };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example7_client() {
        // From ba_example_007.aadl: client with computation action
        let src = r#"
  variables
    x, y : data_type;
  states
    s : initial complete final state;
  transitions
    s -[ on dispatch ]-> s {
      pre!(x,y);
      computation(60ms);
      post!(y,x) };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_ba_example7_server() {
        // From ba_example_007.aadl: server with timeout
        let src = r#"
  variables
    v : data_type.i;
  states
    s0 : initial complete final state;
    s1 : state;
    s2 : complete state;
  transitions
    s0 -[ on dispatch ]-> s1;
    s1 -[ ]-> s2 { long_computation!(v, local_result); local_result.status := 1 };
    s1 -[ timeout ]-> s2 { local_result.status := 0 };
    s2 -[ on dispatch ]-> s0 { send_result!(local_result, v) };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_empty_annex() {
        let src = "";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_only_whitespace() {
        let src = "   \n  \n  ";
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_with_comments() {
        let src = r#"
  -- This is a comment
  states
    s0 : initial state; -- inline comment
  transitions
    s0 -[]-> s0;
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_send_data_with_expression() {
        // m!(x2) -- send data with expression
        let src = r#"
  states
    s : initial final state;
  transitions
    s -[]-> s { m!(x + 1) };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_property_ref_in_expression() {
        let src = r#"
  states
    s : initial complete final state;
  transitions
    s -[ on dispatch ]-> s { x := tick'count + tick'fresh };
"#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }
}
