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

mod eval;
mod lexer;
mod parser;
mod syntax;

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::verify::SeverityFilter;

pub(crate) use eval::{EvalContext, Value};
use parser::ParseResult;

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

// ── Parse cache ─────────────────────────────────────────────────────
//
// TODO: migrate to salsa tracked function when assertions are integrated into LSP.
// For now, a simple thread-local HashMap memoizes `parse(check_expr) -> ParseResult`.

thread_local! {
    static PARSE_CACHE: RefCell<HashMap<String, ParseResult>> = RefCell::new(HashMap::new());
}

fn cached_parse(input: &str) -> ParseResult {
    PARSE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(result) = cache.get(input) {
            return result.clone();
        }
        let result = parser::parse_expr(input);
        cache.insert(input.to_string(), result.clone());
        result
    })
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

    let parse_result = cached_parse(&assertion.check);

    if !parse_result.ok() {
        return AssertionResult {
            id: assertion.id.clone(),
            description: assertion.description.clone(),
            check: assertion.check.clone(),
            severity: sev,
            status: crate::verify::Status::Fail,
            detail: format!(
                "parse error: at position 0: {}",
                parse_result.errors().join("; ")
            ),
        };
    }

    let root = parse_result.syntax_node();

    match eval::eval_node(&root, ctx) {
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
        Ok(Value::Count(n)) => AssertionResult {
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
        },
        Ok(Value::Components(comps)) => AssertionResult {
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
        },
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
    use super::eval::{EvalError, category_matches, direction_matches, feature_kind_matches};
    use super::*;

    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_analysis::{AnalysisDiagnostic, Severity};
    use spar_hir_def::instance::{
        ComponentInstance, ConnectionEnd, ConnectionInstance, FeatureInstance, SystemInstance,
    };
    use spar_hir_def::item_tree::{ComponentCategory, ConnectionKind, Direction, FeatureKind};
    use spar_hir_def::name::Name;
    use spar_hir_def::properties::PropertyMap;

    // ── Helper: parse and evaluate convenience ──────────────────────

    fn parse_check(input: &str) -> ParseResult {
        parser::parse_expr(input)
    }

    fn eval_check(input: &str, ctx: &EvalContext) -> Result<Value, EvalError> {
        let result = parse_check(input);
        assert!(result.ok(), "parse errors: {:?}", result.errors());
        eval::eval_node(&result.syntax_node(), ctx)
    }

    // ── Parser tests ────────────────────────────────────────────────

    #[test]
    fn parse_components_source() {
        let result = parse_check("components");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_analysis_source() {
        let result = parse_check("analysis('scheduling')");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_components_where() {
        let result = parse_check("components.where(category == 'thread')");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_all_predicate() {
        let result = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_and_predicate() {
        let result = parse_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period') and has('Timing_Properties::Compute_Execution_Time'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_or_predicate() {
        let result = parse_check("components.any(category == 'thread' or category == 'process')");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_not_predicate() {
        let result = parse_check("components.none(not has('Timing_Properties::Period'))");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_features_pipeline() {
        let result = parse_check(
            "components.where(category == 'thread').features.where(kind == 'data_port' and direction == 'out').all(connected)",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_analysis_diagnostics() {
        let result = parse_check(
            "analysis('scheduling').diagnostics.none(severity == 'warning' and message.contains('exceeds'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_count() {
        let result = parse_check("components.where(category == 'thread').count()");
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    #[test]
    fn parse_parenthesized_bool() {
        let result = parse_check(
            "components.all((category == 'thread' or category == 'process') and has('Timing_Properties::Period'))",
        );
        assert!(result.ok(), "errors: {:?}", result.errors());
    }

    // ── Parser error tests ──────────────────────────────────────────

    #[test]
    fn parse_error_empty() {
        let result = parse_check("");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected"));
    }

    #[test]
    fn parse_error_bad_source() {
        let result = parse_check("foobar");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected"));
    }

    #[test]
    fn parse_error_unterminated_string() {
        let result = parse_check("components.where(category == 'thread)");
        assert!(!result.ok());
    }

    #[test]
    fn parse_error_missing_paren() {
        let result = parse_check("components.where(category == 'thread'");
        assert!(!result.ok());
    }

    #[test]
    fn parse_error_bad_method() {
        let result = parse_check("components.foobar()");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("expected method name"));
    }

    #[test]
    fn parse_error_trailing_text() {
        let result = parse_check("components foobar");
        assert!(!result.ok());
        assert!(result.errors()[0].contains("unexpected"));
    }

    // ── Test fixtures ───────────────────────────────────────────────

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
        let proc_idx = components.alloc(ComponentInstance {
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
        let t2_out = features.alloc(FeatureInstance {
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
        components[root_idx].children = vec![thread1_idx, thread2_idx, proc_idx];
        components[root_idx].connections = vec![conn_idx];
        components[thread1_idx].features = vec![t1_out];
        components[thread2_idx].features = vec![t2_in, t2_out];

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

    // ── Evaluator tests ─────────────────────────────────────────────

    #[test]
    fn eval_components_returns_all() {
        let inst = make_test_instance();
        let diags = vec![];
        let ctx = EvalContext {
            instance: &inst,
            diagnostics: &diags,
        };
        match eval_check("components", &ctx).unwrap() {
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
        match eval_check("components.where(category == 'thread')", &ctx).unwrap() {
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
        match eval_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
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
        match eval_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
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
        match eval_check(
            "components.where(category == 'thread').none(has('Deployment_Properties::Actual_Processor_Binding'))",
            &ctx,
        ).unwrap() {
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
        match eval_check("components.where(category == 'thread').count()", &ctx).unwrap() {
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
        match eval_check(
            "components.where(category == 'thread').features.where(kind == 'data_port').count()",
            &ctx,
        )
        .unwrap()
        {
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
        match eval_check(
            "components.where(category == 'thread').features.where(kind == 'data_port' and direction == 'out').count()",
            &ctx,
        ).unwrap() {
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
        match eval_check(
            "analysis('scheduling').diagnostics.none(severity == 'warning' and message.contains('exceeds'))",
            &ctx,
        ).unwrap() {
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
        match eval_check(
            "analysis('scheduling').diagnostics.none(severity == 'error' and message.contains('exceeds'))",
            &ctx,
        ).unwrap() {
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
        match eval_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period') and has('Timing_Properties::Compute_Execution_Time'))",
            &ctx,
        ).unwrap() {
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
        match eval_check(
            "components.any(category == 'thread' or category == 'processor')",
            &ctx,
        )
        .unwrap()
        {
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
        match eval_check(
            "components.where(category == 'processor').all(not has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {:?}", other),
        }
    }

    #[test]
    fn eval_empty_model() {
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
        match eval_check(
            "components.where(category == 'thread').all(has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
            Value::Bool(b) => assert!(b, "all() on empty set should be true"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // any() on empty set is false
        match eval_check(
            "components.where(category == 'thread').any(has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
            Value::Bool(b) => assert!(!b, "any() on empty set should be false"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // none() on empty set is vacuously true
        match eval_check(
            "components.where(category == 'thread').none(has('Timing_Properties::Period'))",
            &ctx,
        )
        .unwrap()
        {
            Value::Bool(b) => assert!(b, "none() on empty set should be true"),
            other => panic!("expected Bool, got {:?}", other),
        }

        // count on empty set is 0
        match eval_check("components.where(category == 'thread').count()", &ctx).unwrap() {
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
                check:
                    "components.where(category == 'thread').all(has('Timing_Properties::Period'))"
                        .to_string(),
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
        assert!(category_matches(
            &ComponentCategory::ThreadGroup,
            "thread-group"
        ));
        assert!(category_matches(
            &ComponentCategory::ThreadGroup,
            "thread_group"
        ));
        assert!(category_matches(
            &ComponentCategory::VirtualProcessor,
            "virtual-processor"
        ));
        assert!(category_matches(
            &ComponentCategory::VirtualBus,
            "virtual-bus"
        ));
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
        assert!(feature_kind_matches(
            &FeatureKind::EventDataPort,
            "event_data_port"
        ));
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
