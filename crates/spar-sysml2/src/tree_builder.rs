use std::mem;

use rowan::GreenNodeBuilder;

use crate::event::Event;
use crate::syntax_kind::SyntaxKind;

/// Result of parsing a SysML v2 source file.
pub struct Parse {
    green: rowan::GreenNode,
    errors: Vec<ParseError>,
}

/// A parse error with a message and byte offset.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub msg: String,
    pub offset: usize,
}

impl Parse {
    /// Return the green tree.
    pub fn green(&self) -> &rowan::GreenNode {
        &self.green
    }

    /// Build a typed [`SyntaxNode`] root from the green tree.
    pub fn syntax_node(&self) -> crate::SyntaxNode {
        crate::SyntaxNode::new_root(self.green.clone())
    }

    /// Return the list of parse errors.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Returns `true` if there were no parse errors.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Preprocessed event -- forward_parent chains resolved
// ---------------------------------------------------------------------------

enum ResolvedEvent {
    StartNode(SyntaxKind),
    Token { kind: SyntaxKind, n_raw_tokens: u8 },
    FinishNode,
    Error(String),
}

/// Resolve forward_parent chains and filter out tombstones.
fn resolve_events(mut events: Vec<Event>) -> Vec<ResolvedEvent> {
    let mut resolved = Vec::with_capacity(events.len());
    let mut forward_parents = Vec::new();

    for i in 0..events.len() {
        match mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                forward_parents.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(fwd) = fp {
                    idx += fwd as usize;
                    fp = match mem::replace(&mut events[idx], Event::Tombstone) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            forward_parents.push(kind);
                            forward_parent
                        }
                        _ => unreachable!("forward_parent must point to a Start event"),
                    };
                }

                for kind in forward_parents.drain(..).rev() {
                    if kind != SyntaxKind::TOMBSTONE {
                        resolved.push(ResolvedEvent::StartNode(kind));
                    }
                }
            }

            Event::Finish => {
                resolved.push(ResolvedEvent::FinishNode);
            }

            Event::Token { kind, n_raw_tokens } => {
                resolved.push(ResolvedEvent::Token { kind, n_raw_tokens });
            }

            Event::Error { msg } => {
                resolved.push(ResolvedEvent::Error(msg));
            }

            Event::Tombstone => {}
        }
    }

    resolved
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------

/// Build a rowan green tree from parser events and the original token list.
pub(crate) fn build_tree(input: &str, tokens: &[(SyntaxKind, usize)], events: Vec<Event>) -> Parse {
    let mut builder = GreenNodeBuilder::new();
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

    let non_trivia_indices: Vec<usize> = tokens
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
                        &non_trivia_indices,
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
                        &non_trivia_indices,
                    );
                }
            }

            ResolvedEvent::Token { kind, n_raw_tokens } => {
                let n = *n_raw_tokens as usize;
                for _ in 0..n {
                    if nt_pos < non_trivia_indices.len() {
                        let target_raw = non_trivia_indices[nt_pos];

                        while raw_pos < target_raw {
                            let (tk, len) = tokens[raw_pos];
                            let start = token_starts[raw_pos];
                            builder.token(tk.into(), &input[start..start + len]);
                            raw_pos += 1;
                        }

                        let (_, len) = tokens[target_raw];
                        let start = token_starts[target_raw];
                        builder.token((*kind).into(), &input[start..start + len]);
                        raw_pos = target_raw + 1;
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
                let offset = if nt_pos < non_trivia_indices.len() {
                    token_starts[non_trivia_indices[nt_pos]]
                } else if !tokens.is_empty() {
                    let last = tokens.len() - 1;
                    token_starts[last] + tokens[last].1
                } else {
                    0
                };
                errors.push(ParseError {
                    msg: msg.clone(),
                    offset,
                });
            }
        }
    }

    Parse {
        green: builder.finish(),
        errors,
    }
}

/// Emit trivia tokens from `raw_pos` up to the raw token index of the
/// non-trivia token at position `nt_pos`.
fn eat_trivia(
    builder: &mut GreenNodeBuilder,
    tokens: &[(SyntaxKind, usize)],
    input: &str,
    token_starts: &[usize],
    raw_pos: &mut usize,
    nt_pos: usize,
    non_trivia_indices: &[usize],
) {
    let target_raw = if nt_pos < non_trivia_indices.len() {
        non_trivia_indices[nt_pos]
    } else {
        return;
    };

    while *raw_pos < target_raw {
        let (tk, len) = tokens[*raw_pos];
        let start = token_starts[*raw_pos];
        builder.token(tk.into(), &input[start..start + len]);
        *raw_pos += 1;
    }
}
