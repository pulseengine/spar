//! `spar.verify_move` — wraps [`spar_cli::moves::verify_pipeline`].
//!
//! Input shape (mirrors the schema in [`crate::schema`]):
//!
//! ```json
//! {
//!   "model": "path/to/system.aadl",
//!   "root":  "Pkg::Type.Impl",
//!   "component": "t1",
//!   "target":    "cpu2",
//!   "variant":         "diesel-eu5",          // optional
//!   "variant_context": "/path/to/ctx.json"    // optional, mutually exclusive with variant
//! }
//! ```
//!
//! Output: the same `MoveVerifyReport` shape produced by
//! `spar moves verify --format json`. The exit-code field is preserved
//! as `cli_exit_code` so an agent can quickly distinguish overlay
//! violations (2) from analysis errors (1) from clean runs (0) without
//! re-deriving from `violations[*].kind`.

use serde_json::{Value, json};
use spar_cli::moves::{VerifyArgs, verify_pipeline};

use super::{ToolResult, optional_string, required_string};

/// In-process entry point for the verify tool. Tests call this
/// directly; the JSON-RPC server delegates here via the dispatcher.
pub fn call(arguments: &Value) -> ToolResult {
    let model = match required_string(arguments, "model") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let root = match required_string(arguments, "root") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let component = match required_string(arguments, "component") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target = match required_string(arguments, "target") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let variant = match optional_string(arguments, "variant") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let variant_context = match optional_string(arguments, "variant_context") {
        Ok(v) => v,
        Err(e) => return e,
    };

    let args = VerifyArgs {
        model_files: vec![model],
        root,
        component,
        target,
        // The MCP transport always wants the structured report; the
        // CLI `--format json/text` lever is irrelevant here, but the
        // pipeline still validates the value, so we hard-code `json`.
        format: "json".to_string(),
        variant,
        variant_context,
    };

    match verify_pipeline(&args) {
        Ok((report, exit_code)) => {
            let mut payload = match serde_json::to_value(&report) {
                Ok(v) => v,
                Err(e) => {
                    return ToolResult::Error {
                        code: "INTERNAL",
                        message: format!("failed to serialise MoveVerifyReport: {e}"),
                    };
                }
            };
            // Inject the exit code so agents can branch without
            // re-walking violations[*]. The base report does not carry
            // this field because the CLI surface threads it through
            // process::exit instead.
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("cli_exit_code".to_string(), json!(exit_code));
            }
            ToolResult::Ok(payload)
        }
        Err(e) => ToolResult::Error {
            code: classify_error(&e),
            message: format!("{e}"),
        },
    }
}

/// Map a [`spar_cli::moves::MovesError`] to a stable MCP error code.
///
/// The mapping is deliberately conservative: the rich `MovesError`
/// surface buckets into a small set of agent-actionable codes
/// (`MODEL_NOT_FOUND`, `BAD_INPUT`, `COMPONENT_NOT_FOUND`,
/// `TARGET_INCOMPATIBLE`, `INTERNAL`) — these match the error-code
/// vocabulary in Track E §6.5 "Auth, idempotency, errors".
fn classify_error(e: &spar_cli::moves::MovesError) -> &'static str {
    use spar_cli::moves::MovesError as M;
    match e {
        M::Io(..) => "MODEL_NOT_FOUND",
        M::Parse(..) => "BAD_INPUT",
        M::UnknownRoot(..) => "MODEL_NOT_FOUND",
        M::UnknownComponent(..) | M::ComponentNotInVariant { .. } => "COMPONENT_NOT_FOUND",
        M::UnknownTarget(..) | M::TargetNotInVariant { .. } => "COMPONENT_NOT_FOUND",
        M::TargetNotProcessor { .. } => "TARGET_INCOMPATIBLE",
        M::UnknownFormat(..) | M::UnknownObjective(..) | M::VariantArgsConflict => "BAD_INPUT",
        M::VariantContextIo(..) | M::VariantContextSchema(..) => "BAD_INPUT",
        M::RivetNotFound | M::RivetFailed { .. } | M::RivetIo(..) => "INTERNAL",
    }
}
