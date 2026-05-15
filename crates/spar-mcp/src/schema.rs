//! Tool catalog: name, description, JSON Schema, and MCP annotations.
//!
//! Each tool is declared with the MCP 2025-11-25 metadata shape:
//!
//! ```text
//! { name, description, inputSchema, annotations: { readOnlyHint, idempotentHint } }
//! ```
//!
//! `readOnlyHint = true` and `idempotentHint = true` are *load-bearing*
//! per the v0.9.0 design (Track E §6.5): the MCP transport must not be
//! used to mutate the model, and the same input must always produce the
//! same output. Both invariants follow naturally because the underlying
//! [`spar_cli::moves::verify_pipeline`] /
//! [`spar_cli::moves::enumerate_pipeline`] / latency-analysis surfaces
//! are pure functions of (model files, root, parameters).

use serde::Serialize;
use serde_json::{Value, json};

/// MCP tool descriptor as returned by `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    /// Stable tool identifier (e.g., `"spar.verify_move"`).
    pub name: &'static str,
    /// Human-readable description shown in agent UIs.
    pub description: &'static str,
    /// JSON Schema for the tool's input arguments.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// MCP behaviour hints. Both flags are always `true` for spar's
    /// oracle surface — the apply path is CLI-only.
    pub annotations: Annotations,
}

/// MCP `annotations` block — read-only / idempotent hints.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Annotations {
    #[serde(rename = "readOnlyHint")]
    pub read_only_hint: bool,
    #[serde(rename = "idempotentHint")]
    pub idempotent_hint: bool,
    #[serde(rename = "destructiveHint")]
    pub destructive_hint: bool,
}

impl Annotations {
    /// The standard "this is an oracle, not a mutator" annotation set.
    pub const fn read_only_idempotent() -> Self {
        Self {
            read_only_hint: true,
            idempotent_hint: true,
            destructive_hint: false,
        }
    }
}

/// Stable name of the verify-move tool.
pub const VERIFY_MOVE: &str = "spar.verify_move";
/// Stable name of the enumerate-moves tool.
pub const ENUMERATE_MOVES: &str = "spar.enumerate_moves";
/// Stable name of the check-chain tool.
pub const CHECK_CHAIN: &str = "spar.check_chain";

/// Build the descriptor for `spar.verify_move`.
pub fn verify_move_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: VERIFY_MOVE,
        description: "Verify a hypothetical component-to-processor binding without committing changes. \
             Returns per-pass diagnostics and structured violations. Read-only; idempotent.",
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "model": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Path to the AADL model file to load."
                },
                "root": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Root system implementation in `Pkg::Type.Impl` form."
                },
                "component": {
                    "type": "string",
                    "minLength": 1,
                    "description": "FQN, dotted path, or bare name of the component to (hypothetically) move."
                },
                "target": {
                    "type": "string",
                    "minLength": 1,
                    "description": "FQN of the target processor."
                },
                "variant": {
                    "type": "string",
                    "description": "Optional variant name (implicit form; spar shells out to rivet resolve). Mutually exclusive with variant_context."
                },
                "variant_context": {
                    "type": "string",
                    "description": "Optional explicit-form variant-context path or '-' for stdin. Mutually exclusive with variant."
                }
            },
            "required": ["model", "root", "component", "target"],
            "not": {
                "type": "object",
                "required": ["variant", "variant_context"]
            }
        }),
        annotations: Annotations::read_only_idempotent(),
    }
}

/// Build the descriptor for `spar.enumerate_moves`.
pub fn enumerate_moves_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: ENUMERATE_MOVES,
        description: "List every legal hypothetical-rebinding target for a component, ranked by an \
             objective. Returns per-candidate verification status and a multi-objective score. \
             Read-only; idempotent.",
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "model": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Path to the AADL model file to load."
                },
                "root": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Root system implementation in `Pkg::Type.Impl` form."
                },
                "component": {
                    "type": "string",
                    "minLength": 1,
                    "description": "FQN of the component to enumerate."
                },
                "target_filter": {
                    "type": "string",
                    "description": "Optional case-insensitive substring filter on candidate FQNs."
                },
                "objective": {
                    "type": "string",
                    "enum": ["max-response", "total-load", "total-power", "total-weight", "balanced"],
                    "default": "max-response",
                    "description": "Multi-objective ranking mode (commit 5 of Track E)."
                },
                "variant": {
                    "type": "string",
                    "description": "Optional variant name (implicit form). Mutually exclusive with variant_context."
                },
                "variant_context": {
                    "type": "string",
                    "description": "Optional explicit-form variant-context path or '-' for stdin. Mutually exclusive with variant."
                }
            },
            "required": ["model", "root", "component"],
            "not": {
                "type": "object",
                "required": ["variant", "variant_context"]
            }
        }),
        annotations: Annotations::read_only_idempotent(),
    }
}

/// Build the descriptor for `spar.check_chain`.
pub fn check_chain_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: CHECK_CHAIN,
        description: "Compute end-to-end latency bounds for a thread chain (source → sink), surfacing \
             alternating compute (WCET) and network (WCTT) hops. Read-only; idempotent.",
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "model": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Path to the AADL model file to load."
                },
                "root": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Root system implementation in `Pkg::Type.Impl` form."
                },
                "source_thread": {
                    "type": "string",
                    "minLength": 1,
                    "description": "FQN of the source thread of the chain."
                },
                "sink_thread": {
                    "type": "string",
                    "minLength": 1,
                    "description": "FQN of the sink thread of the chain."
                },
                "variant": {
                    "type": "string",
                    "description": "Not yet supported for check_chain (tracked as v0.10 enhancement); supplying this returns BAD_INPUT. Use spar.verify_move or spar.enumerate_moves for variant-scoped queries. Mutually exclusive with variant_context."
                },
                "variant_context": {
                    "type": "string",
                    "description": "Not yet supported for check_chain (tracked as v0.10 enhancement); supplying this returns BAD_INPUT. Use spar.verify_move or spar.enumerate_moves for variant-scoped queries. Mutually exclusive with variant."
                }
            },
            "required": ["model", "root", "source_thread", "sink_thread"],
            "not": {
                "type": "object",
                "required": ["variant", "variant_context"]
            }
        }),
        annotations: Annotations::read_only_idempotent(),
    }
}

/// All three tool descriptors in stable order.
pub fn all_descriptors() -> Vec<ToolDescriptor> {
    vec![
        verify_move_descriptor(),
        enumerate_moves_descriptor(),
        check_chain_descriptor(),
    ]
}
