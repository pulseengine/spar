//! SysML v2 parser — requirements, constraints, satisfy/verify/refine.
//!
//! This crate implements a recursive-descent parser for SysML v2, following
//! the same marker-based architecture as `spar-parser`. It produces a flat
//! stream of [`Event`]s that can be consumed by a tree builder.
//!
//! # Architecture
//!
//! * [`syntax_kind`] — every token and node kind for SysML v2.
//! * [`event`] — the event types produced by the parser.
//! * [`token_set`] — efficient bitset for recovery sets.
//! * [`marker`] — marker/completed-marker system for building the event stream.
//! * [`parser`] — the `Parser` struct that grammar functions call into.
//! * [`lexer`] — SysML v2 lexer (C-style comments, keywords).
//! * [`grammar`] — grammar rules for requirements, constraints, relationships.

pub mod event;
pub mod grammar;
pub mod lexer;
pub mod marker;
pub mod parser;
pub mod syntax_kind;
pub mod token_set;

pub use syntax_kind::SyntaxKind;

/// Parse SysML v2 source text and return the event stream.
///
/// This is the main entry point. It tokenizes the source and runs the
/// recursive-descent parser, returning events that describe the CST.
pub fn parse(source: &str) -> Vec<event::Event> {
    let tokens = lexer::tokenize(source);
    let mut p = parser::Parser::new(&tokens, source);
    grammar::source_file(&mut p);
    p.finish()
}

/// Convenience: parse and collect all error messages.
pub fn parse_errors(source: &str) -> Vec<String> {
    parse(source)
        .into_iter()
        .filter_map(|e| match e {
            event::Event::Error { msg } => Some(msg),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Helper: check that parsing produces no errors
    // ------------------------------------------------------------------
    fn assert_no_errors(source: &str) {
        let errors = parse_errors(source);
        assert!(
            errors.is_empty(),
            "unexpected parse errors for:\n{}\nerrors: {:?}",
            source,
            errors
        );
    }

    /// Check that a specific node kind appears in the event stream.
    fn has_node(source: &str, kind: SyntaxKind) -> bool {
        parse(source).iter().any(|e| match e {
            event::Event::Start { kind: k, .. } => *k == kind,
            _ => false,
        })
    }

    // ------------------------------------------------------------------
    // Test 1: parse_requirement_def
    // ------------------------------------------------------------------
    #[test]
    fn parse_requirement_def() {
        let source = r#"requirement def LatencyReq {
    attribute maxLatency : Real = 20.0;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REQUIREMENT_DEF));
        assert!(has_node(source, SyntaxKind::ATTRIBUTE_USAGE));
        assert!(has_node(source, SyntaxKind::BODY_BLOCK));
    }

    // ------------------------------------------------------------------
    // Test 2: parse_requirement_usage
    // ------------------------------------------------------------------
    #[test]
    fn parse_requirement_usage() {
        let source = r#"requirement sensorLatency : LatencyReq {
    subject sensor : SensorSubsystem;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REQUIREMENT_USAGE));
        assert!(has_node(source, SyntaxKind::SUBJECT_MEMBER));
        assert!(has_node(source, SyntaxKind::TYPE_REF));
    }

    // ------------------------------------------------------------------
    // Test 3: parse_satisfy
    // ------------------------------------------------------------------
    #[test]
    fn parse_satisfy() {
        let source = "satisfy sensorLatency by ecu.controller;";
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::SATISFY_REQ));
        assert!(has_node(source, SyntaxKind::NAME_REF));
    }

    // ------------------------------------------------------------------
    // Test 4: parse_verify
    // ------------------------------------------------------------------
    #[test]
    fn parse_verify() {
        let source = "verify sensorLatency by latencyTest;";
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::VERIFY_REQ));
    }

    // ------------------------------------------------------------------
    // Test 5: parse_constraint_def
    // ------------------------------------------------------------------
    #[test]
    fn parse_constraint_def() {
        let source = r#"constraint def TimingBudget {
    attribute totalLatency : Real;
    attribute bound : Real;
    totalLatency <= bound;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::CONSTRAINT_DEF));
        assert!(has_node(source, SyntaxKind::ATTRIBUTE_USAGE));
        assert!(has_node(source, SyntaxKind::BINARY_EXPR));
    }

    // ------------------------------------------------------------------
    // Test 6: parse_requirement_with_doc
    // ------------------------------------------------------------------
    #[test]
    fn parse_requirement_with_doc() {
        let source = r#"requirement def LatencyReq {
    doc /* Sensor-to-actuator latency < 20ms */
    attribute maxLatency : Real = 20.0;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REQUIREMENT_DEF));
        assert!(has_node(source, SyntaxKind::DOC_MEMBER));
        assert!(has_node(source, SyntaxKind::ATTRIBUTE_USAGE));
    }

    // ------------------------------------------------------------------
    // Additional tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_refine() {
        let source = "refine highLevelReq by detailedReq;";
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REFINE_REQ));
    }

    #[test]
    fn parse_constraint_usage() {
        let source = r#"constraint timingCheck : TimingBudget {
    totalLatency = sensorDelay + processingTime;
    bound = 20.0;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::CONSTRAINT_USAGE));
        assert!(has_node(source, SyntaxKind::BINARY_EXPR));
    }

    #[test]
    fn parse_satisfy_dotted_path() {
        let source = "satisfy sensorLatency by ecu.controller.subsystem;";
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::SATISFY_REQ));
    }

    #[test]
    fn parse_multiple_items() {
        let source = r#"requirement def SafetyReq {
    attribute criticality : Integer = 1;
}

requirement systemSafety : SafetyReq {
    subject controller : FlightController;
}

satisfy systemSafety by flightController;
verify systemSafety by safetyTest;
"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REQUIREMENT_DEF));
        assert!(has_node(source, SyntaxKind::REQUIREMENT_USAGE));
        assert!(has_node(source, SyntaxKind::SATISFY_REQ));
        assert!(has_node(source, SyntaxKind::VERIFY_REQ));
    }

    #[test]
    fn parse_requirement_without_body() {
        let source = "requirement sensorLatency : LatencyReq;";
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::REQUIREMENT_USAGE));
    }

    #[test]
    fn parse_constraint_with_doc() {
        let source = r#"constraint def TimingBudget {
    doc /* Total timing budget for the system */
    attribute totalLatency : Real;
}"#;
        assert_no_errors(source);
        assert!(has_node(source, SyntaxKind::CONSTRAINT_DEF));
        assert!(has_node(source, SyntaxKind::DOC_MEMBER));
    }
}
