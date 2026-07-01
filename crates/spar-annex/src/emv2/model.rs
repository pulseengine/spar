//! Semantic EMV2 model — the typed projection of an `error propagations`
//! subclause (REQ-EMV2-PROPAGATION-002, layer 1 of 4, GitHub #294).
//!
//! The EMV2 parser ([`super::parse`]) produces a lossless Rowan CST. Downstream
//! analysis (fault-propagation traversal, the `Emv2Overlay`) needs a *semantic*
//! view: for each component, which features carry `in`/`out` error
//! propagations, and which `error source`/`sink`/`path` flows are declared.
//! This module lifts the relevant CST nodes into small typed structs. It is
//! deliberately confined to the `error propagations` section — error *types*,
//! behavior state machines, and composite behavior are out of scope here (they
//! are separate concerns handled elsewhere / later).
//!
//! # Grammar shapes lifted
//!
//! - `ERROR_PROPAGATION` — `(feature | kind) : not? (in|out) propagation
//!   {TypeSet};`. The feature reference is wrapped in a `FEATURE_OR_PP_REF`
//!   node, so its dotted path is recovered from that node's text.
//! - `ERROR_SOURCE` / `ERROR_SINK` / `ERROR_PATH` — the flows subsection. Their
//!   propagation *points* are NOT wrapped in a node (the grammar bumps the
//!   `IDENT`/`.`/`all` tokens straight into the flow node), so the point is
//!   recovered by a landmark-driven walk: the flow keyword starts the incoming
//!   point, `->` starts the outgoing point (paths only), and a `{`, `when`,
//!   `if`, or `;` ends it.

use super::syntax_kind::{Emv2Kind, Emv2SyntaxNode};
use rowan::NodeOrToken;

/// Direction of an error propagation relative to the owning component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationDirection {
    /// `in propagation` — an error the component can receive on a feature.
    In,
    /// `out propagation` — an error the component can emit on a feature.
    Out,
}

/// A single `(feature | kind) : not? (in|out) propagation {TypeSet};`
/// declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPropagation {
    /// The feature/port path (dotted, whitespace-stripped) the propagation is
    /// declared on, or the propagation-kind keyword when [`Self::is_kind`].
    pub feature: String,
    /// True when [`Self::feature`] names a propagation *kind* (e.g. `access`,
    /// `binding`) rather than a component feature.
    pub is_kind: bool,
    /// The `not` negation modifier (a *masked* propagation).
    pub negated: bool,
    /// `in` or `out`.
    pub direction: PropagationDirection,
    /// Error type names (or type expressions) in the declared `{TypeSet}`.
    pub error_types: Vec<String>,
}

/// Which kind of error flow a [`ErrorFlow`] declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorFlowKind {
    /// `error source` — the component originates an error at a point.
    Source,
    /// `error sink` — the component absorbs an error arriving at a point.
    Sink,
    /// `error path` — the component forwards an error from one point to another.
    Path,
}

/// A single `name : error (source|sink|path) …;` flow declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorFlow {
    /// The flow's declared identifier.
    pub name: String,
    /// Source, sink, or path.
    pub kind: ErrorFlowKind,
    /// The propagation point: a feature path, a propagation kind, or `all`.
    /// For a source this is the origin (out) point; for a sink the (in) point;
    /// for a path the *incoming* point.
    pub point: String,
    /// A path's *outgoing* point. `None` for sources and sinks.
    pub target_point: Option<String>,
    /// Error type names constraining the flow (the first `{TypeSet}`), if any.
    pub error_types: Vec<String>,
}

/// The semantic model of one EMV2 subclause's `error propagations` section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Emv2Model {
    /// Feature-level `in`/`out` propagation declarations, in source order.
    pub propagations: Vec<ErrorPropagation>,
    /// `source`/`sink`/`path` flows, in source order.
    pub flows: Vec<ErrorFlow>,
}

impl Emv2Model {
    /// Lift the semantic model from a parsed EMV2 subclause syntax tree (the
    /// root returned by [`super::Emv2Parse::syntax_node`]).
    #[must_use]
    pub fn from_syntax(root: &Emv2SyntaxNode) -> Self {
        let mut model = Emv2Model::default();
        for node in root.descendants() {
            match node.kind() {
                Emv2Kind::ERROR_PROPAGATION => {
                    if let Some(p) = extract_propagation(&node) {
                        model.propagations.push(p);
                    }
                }
                Emv2Kind::ERROR_SOURCE | Emv2Kind::ERROR_SINK | Emv2Kind::ERROR_PATH => {
                    if let Some(f) = extract_flow(&node) {
                        model.flows.push(f);
                    }
                }
                _ => {}
            }
        }
        model
    }
}

/// Collapse a node's source text to a whitespace-free string (a dotted feature
/// path like `feat . sub` becomes `feat.sub`).
fn compact_text(node: &Emv2SyntaxNode) -> String {
    node.text().to_string().split_whitespace().collect()
}

/// Extract the error type names from a `TYPE_SET_CONSTRUCTOR` node — one entry
/// per `TYPE_SET_ELEMENT`, its text trimmed.
fn error_types_from_type_set(type_set: &Emv2SyntaxNode) -> Vec<String> {
    type_set
        .children()
        .filter(|n| n.kind() == Emv2Kind::TYPE_SET_ELEMENT)
        .map(|el| el.text().to_string().split_whitespace().collect::<String>())
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_propagation(node: &Emv2SyntaxNode) -> Option<ErrorPropagation> {
    // Feature reference (wrapped) or a bare propagation-kind token.
    let (feature, is_kind) = if let Some(fref) = node
        .children()
        .find(|n| n.kind() == Emv2Kind::FEATURE_OR_PP_REF)
    {
        (compact_text(&fref), false)
    } else {
        let kind_tok = node
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find(|t| t.kind().is_propagation_kind())?;
        (kind_tok.text().to_string(), true)
    };

    let tokens = || {
        node.children_with_tokens()
            .filter_map(NodeOrToken::into_token)
    };
    let negated = tokens().any(|t| t.kind() == Emv2Kind::NOT_KW);
    let direction = tokens().find_map(|t| match t.kind() {
        Emv2Kind::IN_KW => Some(PropagationDirection::In),
        Emv2Kind::OUT_KW => Some(PropagationDirection::Out),
        _ => None,
    })?;

    let error_types = node
        .children()
        .find(|n| n.kind() == Emv2Kind::TYPE_SET_CONSTRUCTOR)
        .map(|ts| error_types_from_type_set(&ts))
        .unwrap_or_default();

    Some(ErrorPropagation {
        feature,
        is_kind,
        negated,
        direction,
        error_types,
    })
}

/// A landmark region while walking a flow node's ordered children.
enum FlowRegion {
    /// Before the `:` — capturing the flow name.
    Name,
    /// Between the flow keyword and the first delimiter — the incoming point.
    Incoming,
    /// After `->` — the outgoing point (paths only).
    Outgoing,
    /// After a `when`/`if` guard — stop capturing points.
    Done,
}

/// True for a token that is part of a propagation point (a feature path
/// segment, `all`, a dot, or a propagation kind).
fn is_point_token(kind: Emv2Kind) -> bool {
    matches!(kind, Emv2Kind::IDENT | Emv2Kind::DOT | Emv2Kind::ALL_KW) || kind.is_propagation_kind()
}

fn extract_flow(node: &Emv2SyntaxNode) -> Option<ErrorFlow> {
    let kind = match node.kind() {
        Emv2Kind::ERROR_SOURCE => ErrorFlowKind::Source,
        Emv2Kind::ERROR_SINK => ErrorFlowKind::Sink,
        Emv2Kind::ERROR_PATH => ErrorFlowKind::Path,
        _ => return None,
    };

    let mut name = String::new();
    let mut point = String::new();
    let mut target = String::new();
    let mut error_types: Vec<String> = Vec::new();
    let mut region = FlowRegion::Name;

    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(n) => {
                // The first {TypeSet} constrains the (incoming) point; capture
                // it once, and treat it as the end of the incoming point.
                if n.kind() == Emv2Kind::TYPE_SET_CONSTRUCTOR {
                    if matches!(region, FlowRegion::Incoming) && error_types.is_empty() {
                        error_types = error_types_from_type_set(&n);
                    }
                    if matches!(region, FlowRegion::Incoming) {
                        region = FlowRegion::Done;
                    }
                }
            }
            NodeOrToken::Token(t) => {
                let k = t.kind();
                if k.is_trivia() {
                    continue;
                }
                match k {
                    Emv2Kind::SOURCE_KW | Emv2Kind::SINK_KW | Emv2Kind::PATH_KW => {
                        region = FlowRegion::Incoming;
                    }
                    Emv2Kind::ARROW => {
                        region = FlowRegion::Outgoing;
                    }
                    Emv2Kind::WHEN_KW | Emv2Kind::IF_KW => {
                        region = FlowRegion::Done;
                    }
                    _ => match region {
                        FlowRegion::Name if k == Emv2Kind::IDENT && name.is_empty() => {
                            name = t.text().to_string();
                        }
                        FlowRegion::Incoming if is_point_token(k) => {
                            point.push_str(t.text());
                        }
                        FlowRegion::Outgoing if is_point_token(k) => {
                            target.push_str(t.text());
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    Some(ErrorFlow {
        name,
        kind,
        point,
        target_point: if target.is_empty() {
            None
        } else {
            Some(target)
        },
        error_types,
    })
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;

    fn model(src: &str) -> Emv2Model {
        let parsed = parse(src);
        assert!(parsed.ok(), "parse errors: {:?}", parsed.errors());
        Emv2Model::from_syntax(&parsed.syntax_node())
    }

    #[test]
    fn extracts_in_and_out_propagations_with_types() {
        let m = model(
            r#"
            error propagations
                valuein:  in  propagation {LateDelivery};
                valueout: out propagation {ServiceError};
            end propagations;
        "#,
        );
        assert_eq!(m.propagations.len(), 2);
        let vin = &m.propagations[0];
        assert_eq!(vin.feature, "valuein");
        assert!(!vin.is_kind);
        assert!(!vin.negated);
        assert_eq!(vin.direction, PropagationDirection::In);
        assert_eq!(vin.error_types, vec!["LateDelivery"]);
        let vout = &m.propagations[1];
        assert_eq!(vout.feature, "valueout");
        assert_eq!(vout.direction, PropagationDirection::Out);
        assert_eq!(vout.error_types, vec!["ServiceError"]);
        assert!(m.flows.is_empty());
    }

    #[test]
    fn extracts_source_flow_with_point_and_types() {
        // The canonical repro shape: an out-propagation feeds an error source.
        let m = model(
            r#"
            error propagations
                dataout: out propagation {BadValue};
                flows
                    f0: error source dataout {BadValue};
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        let f = &m.flows[0];
        assert_eq!(f.name, "f0");
        assert_eq!(f.kind, ErrorFlowKind::Source);
        assert_eq!(f.point, "dataout");
        assert_eq!(f.target_point, None);
        assert_eq!(f.error_types, vec!["BadValue"]);
    }

    #[test]
    fn extracts_sink_flow() {
        let m = model(
            r#"
            error propagations
                datain: in propagation {BadValue};
                flows
                    s0: error sink datain {BadValue};
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        let f = &m.flows[0];
        assert_eq!(f.name, "s0");
        assert_eq!(f.kind, ErrorFlowKind::Sink);
        assert_eq!(f.point, "datain");
        assert_eq!(f.target_point, None);
    }

    #[test]
    fn extracts_path_flow_with_incoming_and_outgoing_points() {
        // A path carries an error from an incoming point to an outgoing one —
        // this is exactly the edge a propagation-chain traversal follows.
        let m = model(
            r#"
            error propagations
                valuein:  in  propagation {LateDelivery};
                valueout: out propagation {ServiceError};
                flows
                    ef0: error path valuein {LateDelivery} -> valueout {ServiceError};
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        let f = &m.flows[0];
        assert_eq!(f.name, "ef0");
        assert_eq!(f.kind, ErrorFlowKind::Path);
        assert_eq!(f.point, "valuein");
        assert_eq!(f.target_point.as_deref(), Some("valueout"));
        // The first {TypeSet} (incoming) is the captured constraint.
        assert_eq!(f.error_types, vec!["LateDelivery"]);
    }

    #[test]
    fn recovers_dotted_feature_path() {
        let m = model(
            r#"
            error propagations
                bus.access: out propagation {BadData};
            end propagations;
        "#,
        );
        assert_eq!(m.propagations.len(), 1);
        assert_eq!(m.propagations[0].feature, "bus.access");
        assert!(!m.propagations[0].is_kind);
    }

    #[test]
    fn captures_not_negation() {
        let m = model(
            r#"
            error propagations
                valueout: not out propagation {ServiceError};
            end propagations;
        "#,
        );
        assert_eq!(m.propagations.len(), 1);
        assert!(m.propagations[0].negated);
        assert_eq!(m.propagations[0].direction, PropagationDirection::Out);
    }

    #[test]
    fn bare_source_point_excludes_trailing_semicolon() {
        // A source with NO type-set constraint: the incoming point must stop at
        // the `;`, not swallow it (guards the point-token filter in the walk).
        let m = model(
            r#"
            error propagations
                dataout: out propagation {BadValue};
                flows
                    f0: error source dataout;
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        assert_eq!(m.flows[0].point, "dataout");
        assert!(m.flows[0].error_types.is_empty());
    }

    #[test]
    fn source_point_stops_at_when_guard() {
        // A `when` guard (no preceding type set) must END the incoming point —
        // the state reference after `when` is not part of the point.
        let m = model(
            r#"
            error propagations
                dataout: out propagation {BadValue};
                flows
                    f0: error source dataout when Operational;
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        assert_eq!(
            m.flows[0].point, "dataout",
            "`when` clause must not join the point"
        );
    }

    #[test]
    fn path_outgoing_type_set_does_not_leak_into_error_types() {
        // A path whose ONLY type set is on the OUTGOING side: error_types is the
        // INCOMING constraint, so here it must stay EMPTY (the outgoing
        // {LostMessage} must not be captured as the flow's types).
        let m = model(
            r#"
            error propagations
                connection: in  propagation {ValueCorruption};
                bindings:   out propagation {LostMessage};
                flows
                    p0: error path connection -> bindings {LostMessage};
            end propagations;
        "#,
        );
        assert_eq!(m.flows.len(), 1);
        assert_eq!(m.flows[0].point, "connection");
        assert_eq!(m.flows[0].target_point.as_deref(), Some("bindings"));
        assert!(
            m.flows[0].error_types.is_empty(),
            "outgoing type set must not leak into the flow's (incoming) error_types"
        );
    }

    // NOTE — accepted equivalent mutant: in `extract_flow` the guard
    // `k == IDENT && name.is_empty()` (Name region). A flow's first non-trivia
    // token is ALWAYS its name IDENT (grammar bumps IDENT first), so the
    // conjunction and its `||` mutant never diverge on reachable input: at the
    // first token both operands are true; no later Name-region token is an
    // IDENT nor arrives while `name` is empty. Not killable without malformed
    // input.

    #[test]
    fn multi_element_type_set() {
        let m = model(
            r#"
            error propagations
                p: out propagation {ServiceError, LateDelivery, BadValue};
            end propagations;
        "#,
        );
        assert_eq!(
            m.propagations[0].error_types,
            vec!["ServiceError", "LateDelivery", "BadValue"]
        );
    }

    #[test]
    fn real_osate_corpus_block_source_sink_and_path() {
        // Verbatim from the bundled OSATE corpus
        // (test-data/interop/osate-examples/LargeExamplesforEMV2TR/
        //  network_protocol_pkg.aadl) — NOT a toy snippet. Exercises
        // propagation-KIND points (`bindings`, `connection` are kinds, not
        // features) and a source + sink + path together.
        let m = model(
            r#"
            error propagations
              bindings: out propagation {LateDelivery, ValueCorruption};
              connection: in propagation {LostMessage, ValueCorruption};
            flows
              RetryTiming: error source bindings {LateDelivery};
              MaskLoss: error sink connection {LostMessage};
              PassCorruption: error path connection {ValueCorruption} -> bindings;
            end propagations;
        "#,
        );
        assert_eq!(m.propagations.len(), 2);
        assert_eq!(m.propagations[0].feature, "bindings");
        assert!(
            m.propagations[0].is_kind,
            "`bindings` is a propagation kind"
        );
        assert_eq!(m.propagations[0].direction, PropagationDirection::Out);
        assert_eq!(
            m.propagations[0].error_types,
            vec!["LateDelivery", "ValueCorruption"]
        );
        assert_eq!(m.propagations[1].feature, "connection");
        assert!(m.propagations[1].is_kind);
        assert_eq!(m.propagations[1].direction, PropagationDirection::In);

        assert_eq!(m.flows.len(), 3);
        let names: Vec<&str> = m.flows.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["RetryTiming", "MaskLoss", "PassCorruption"]);
        assert_eq!(m.flows[0].kind, ErrorFlowKind::Source);
        assert_eq!(m.flows[0].point, "bindings");
        assert_eq!(m.flows[0].error_types, vec!["LateDelivery"]);
        assert_eq!(m.flows[1].kind, ErrorFlowKind::Sink);
        assert_eq!(m.flows[1].point, "connection");
        let path = &m.flows[2];
        assert_eq!(path.kind, ErrorFlowKind::Path);
        assert_eq!(path.point, "connection");
        assert_eq!(path.target_point.as_deref(), Some("bindings"));
        assert_eq!(path.error_types, vec!["ValueCorruption"]);
    }

    #[test]
    fn empty_when_no_error_propagations_section() {
        // A subclause with only error *types* yields no propagations/flows.
        let m = model(
            r#"
            error types
                ServiceError: type;
            end types;
        "#,
        );
        assert!(m.propagations.is_empty());
        assert!(m.flows.is_empty());
    }
}
