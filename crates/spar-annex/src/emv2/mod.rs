//! EMV2 (Error Model V2) annex parser.
//!
//! Specification: SAE AS5506/1 Annex E — Error Model Annex
//!   Full title: "Architecture Analysis & Design Language (AADL)
//!                AS5506/1 Annex E: Error Model Annex"
//!   Published by: SAE International
//!
//! Reference implementation: OSATE2 org.osate.xtext.aadl2.errormodel
//!   Grammar: ErrorModel.xtext (703 lines)
//!   Source:  https://github.com/osate/osate2/tree/master/emv2
//!
//! The EMV2 annex defines error types, error behavior state machines,
//! error propagations through component features, composite error
//! behavior for fault tree analysis, and type transformations/mappings.

mod grammar;
mod lexer;
mod parser;
pub mod syntax_kind;

use std::mem;

pub use syntax_kind::{Emv2Kind, Emv2Language, Emv2SyntaxNode, Emv2SyntaxToken};

use crate::{AnnexDiagnostic, AnnexNode, AnnexParseResult, AnnexParser, Severity, Span};

/// Result of parsing EMV2 annex content.
pub struct Emv2Parse {
    green: rowan::GreenNode,
    errors: Vec<Emv2Error>,
}

/// An error from EMV2 parsing.
#[derive(Debug, Clone)]
pub struct Emv2Error {
    pub msg: String,
    pub offset: usize,
}

impl Emv2Parse {
    /// Build a typed syntax node root.
    pub fn syntax_node(&self) -> Emv2SyntaxNode {
        Emv2SyntaxNode::new_root(self.green.clone())
    }

    /// Return parse errors.
    pub fn errors(&self) -> &[Emv2Error] {
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
fn flatten_node(node: &Emv2SyntaxNode, parent: i32, out: &mut Vec<AnnexNode>) {
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

/// Parse EMV2 source text (the content between `{**` and `**}`).
pub fn parse(source: &str) -> Emv2Parse {
    let tokens = lexer::tokenize(source);
    let mut p = parser::Parser::new(&tokens, source);
    grammar::root(&mut p);
    let events = p.finish();
    build_tree(source, &tokens, events)
}

// -- Tree builder (adapted from spar-syntax) --

enum ResolvedEvent {
    StartNode(Emv2Kind),
    Token { kind: Emv2Kind, n_raw_tokens: u8 },
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
                    if kind != Emv2Kind::TOMBSTONE {
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

fn build_tree(input: &str, tokens: &[(Emv2Kind, usize)], events: Vec<parser::Event>) -> Emv2Parse {
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
                errors.push(Emv2Error {
                    msg: msg.clone(),
                    offset,
                });
            }
        }
    }

    Emv2Parse {
        green: builder.finish(),
        errors,
    }
}

fn eat_trivia(
    builder: &mut rowan::GreenNodeBuilder,
    tokens: &[(Emv2Kind, usize)],
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

/// Built-in parser for the EMV2 annex.
pub struct Emv2AnnexParser;

impl AnnexParser for Emv2AnnexParser {
    fn names(&self) -> &[&str] {
        &["EMV2"]
    }

    fn parse(&self, _name: &str, source: &str) -> AnnexParseResult {
        let result = parse(source);
        result.to_annex_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_type_definitions() {
        let src = r#"
            error types
                ServiceError: type;
                ItemOmission: type extends ServiceError;
                TimingError renames type ItemOmission;
            end types;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
        let root = result.syntax_node();
        assert_eq!(root.kind(), Emv2Kind::EMV2_ROOT);
    }

    #[test]
    fn parse_type_set() {
        let src = r#"
            error types
                CommonErrors: type set {ServiceError, TimingError};
            end types;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_error_behavior_sm() {
        let src = r#"
            error behavior FailStop
            events
                Failure : error event;
            states
                Operational : initial state;
                FailStop : state;
            transitions
                FailureTransition : Operational -[ Failure ]-> FailStop;
            end behavior;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_branching_transition() {
        let src = r#"
            error behavior PermanentTransient
            events
                Failure: error event;
                Recovery: recover event;
            states
                Operational: initial state;
                FailedTransient: state;
                FailedPermanent: state;
            transitions
                t1: Operational -[ Failure ]-> (FailedTransient with 0.5, FailedPermanent with others);
                t2: FailedTransient -[ Recovery ]-> Operational;
            end behavior;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_subclause_use_and_propagations() {
        let src = r#"
            use types ErrorLibrary;
            use behavior ErrorLibrary::FailStop;

            error propagations
                valuein: in propagation {LateDelivery};
                valueout: out propagation {ServiceError};
                flows
                    ef0: error path valuein {LateDelivery} -> valueout {ServiceError};
            end propagations;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_component_error_behavior() {
        let src = r#"
            use types ErrorLibrary;
            use behavior ErrorLibrary::FailStop;

            component error behavior
                transitions
                    t0: Operational -[ valuein {ServiceError} ]-> FailStop;
            end component;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_composite_error_behavior() {
        let src = r#"
            use types ErrorLibrary;
            use behavior ErrorLibrary::FailStop;

            composite error behavior
                states
                    [a0.FailStop and a1.FailStop]-> FailStop;
            end composite;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_composite_with_or() {
        let src = r#"
            use types ErrorLibrary;
            use behavior ErrorLibrary::Simple;

            composite error behavior
                states
                    [(s1.Failed or sens1.Failed) and (s2.Failed or sens2.Failed)]-> Failed;
            end composite;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_propagation_paths() {
        let src = r#"
            propagation paths
                externalEffect: propagation point;
                s1.externalEffect -> externalEffect;
            end paths;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_error_source_sink() {
        let src = r#"
            use types ErrorLibrary;

            error propagations
                dataout: out propagation {BadValue};
                flows
                    f0: error source dataout {BadValue};
            end propagations;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_full_library() {
        // Real-world test: subset of ErrorLibrary.aadl
        let src = r#"
error types
ServiceError: type;
ItemOmission: type extends ServiceError;
ServiceOmission: type extends ServiceError;
TimingError renames type ServiceError;
CommonErrors: type set { ServiceError };
end types;

error behavior FailStop
events
    Failure : error event ;
states
    Operational : initial state ;
    FailStop : state ;
transitions
    FailureTransition : Operational -[ Failure ]-> FailStop ;
end behavior ;

error behavior FailAndRecover
events
    Failure: error event ;
    Recovery: recover event;
states
    Operational: initial state;
    Failed: state;
transitions
    FailureTransition : Operational-[Failure]->Failed;
    RecoveryTransition : Failed-[Recovery]->Operational;
end behavior;
        "#;
        let result = parse(src);
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn lossless_round_trip() {
        let src = "error types\n  Err1: type;\nend types;";
        let result = parse(src);
        let root = result.syntax_node();
        assert_eq!(root.text().to_string(), src);
    }

    #[test]
    fn annex_parser_trait() {
        let parser = Emv2AnnexParser;
        assert_eq!(parser.names(), &["EMV2"]);
        let result = parser.parse("EMV2", "error types\nE1: type;\nend types;");
        assert!(!result.has_errors());
        assert!(!result.nodes.is_empty());
    }
}
