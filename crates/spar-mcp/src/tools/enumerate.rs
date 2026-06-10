//! `spar.enumerate_moves` — wraps [`spar_cli::moves::enumerate_pipeline`].
//!
//! Input shape:
//!
//! ```json
//! {
//!   "model": "path/to/system.aadl",
//!   "root":  "Pkg::Type.Impl",
//!   "component": "t1",
//!   "target_filter":   "cpu",                 // optional
//!   "objective":       "max-response",        // optional, default
//!   "variant":         "diesel-eu5",          // optional
//!   "variant_context": "/path/to/ctx.json"    // optional, mutually exclusive with variant
//! }
//! ```
//!
//! Output: the `MoveEnumerateReport` shape produced by
//! `spar moves enumerate --format json` (component, candidates, total,
//! valid, plus the variant audit-trail fields).

use serde_json::Value;
use spar_cli::moves::{
    EnumerateArgs, EnumerationObjective, MovesError, enumerate_pipeline, parse_objective,
};

use super::{ToolResult, optional_string, required_string};

/// In-process entry point for the enumerate tool.
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
    let target_filter = match optional_string(arguments, "target_filter") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let objective_str = match optional_string(arguments, "objective") {
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

    let objective = match objective_str.as_deref() {
        None => EnumerationObjective::max_response(),
        Some(s) => match parse_objective(s) {
            Ok(o) => o,
            Err(e) => {
                return ToolResult::Error {
                    code: "BAD_INPUT",
                    message: format!("{e}"),
                };
            }
        },
    };

    let args = EnumerateArgs {
        model_files: vec![model],
        root,
        component,
        target_filter,
        format: "json".to_string(),
        objective,
        variant,
        variant_context,
    };

    match enumerate_pipeline(&args) {
        Ok(report) => match serde_json::to_value(&report) {
            Ok(v) => ToolResult::Ok(v),
            Err(e) => ToolResult::Error {
                code: "INTERNAL",
                message: format!("failed to serialise MoveEnumerateReport: {e}"),
            },
        },
        Err(e) => ToolResult::Error {
            code: classify_error(&e),
            message: format!("{e}"),
        },
    }
}

fn classify_error(e: &MovesError) -> &'static str {
    use MovesError as M;
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
