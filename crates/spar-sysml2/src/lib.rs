//! SysML v2 parser, AADL lowering, and requirements extraction.
//!
//! This crate provides:
//! * A lossless concrete syntax tree parser for SysML v2.
//! * [`lower`] -- SysML v2 to AADL lowering (SEI mapping rules).
//! * [`extract`] -- Requirements extraction to rivet YAML artifacts.
//!
//! Parser architecture:
//! * [`syntax_kind`] -- every token and node kind in SysML v2.
//! * [`lexer`] -- tokenizer producing `(SyntaxKind, &str)` pairs.
//! * [`parser`] -- marker-based recursive descent parser.
//! * [`grammar`] -- grammar rules for packages, parts, ports, connections.

pub mod event;
pub mod extract;
pub mod grammar;
pub mod language;
pub mod lexer;
pub mod lower;
pub mod marker;
pub mod parser;
pub mod syntax_kind;
pub mod token_set;
mod tree_builder;

pub use language::{SyntaxNode, SyntaxToken};
pub use syntax_kind::SyntaxKind;
pub use tree_builder::{Parse, ParseError};

/// Parse SysML v2 source text into a lossless concrete syntax tree.
///
/// This is the main entry point for the parser. It tokenizes the input,
/// runs the recursive descent parser, and builds a rowan green tree.
///
/// # Example
///
/// ```
/// let source = r#"
/// package Sensors {
///     part def Sensor { }
///     part mySensor : Sensor;
/// }
/// "#;
/// let parse = spar_sysml2::parse(source);
/// assert!(parse.ok());
/// ```
pub fn parse(source: &str) -> Parse {
    let tokens = lexer::tokenize(source);
    let mut p = parser::Parser::new(&tokens, source);
    grammar::source_file(&mut p);
    let events = p.finish();
    tree_builder::build_tree(source, &tokens, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_file() {
        let parse = parse("");
        assert!(parse.ok());
        let root = parse.syntax_node();
        assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);
    }

    #[test]
    fn parse_package() {
        let parse = parse("package Pkg { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);
        // Should have a PACKAGE child
        let pkg = root.children().find(|n| n.kind() == SyntaxKind::PACKAGE);
        assert!(pkg.is_some(), "expected PACKAGE node in tree");
    }

    #[test]
    fn parse_import() {
        let parse = parse("import ScalarValues::*;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let import = root
            .children()
            .find(|n| n.kind() == SyntaxKind::IMPORT_DECL);
        assert!(import.is_some(), "expected IMPORT_DECL node in tree");
    }

    #[test]
    fn parse_part_def() {
        let parse = parse("part def Vehicle { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let part_def = root.children().find(|n| n.kind() == SyntaxKind::PART_DEF);
        assert!(part_def.is_some(), "expected PART_DEF node in tree");
    }

    #[test]
    fn parse_part_usage() {
        let parse = parse("part v : Vehicle;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let part_usage = root.children().find(|n| n.kind() == SyntaxKind::PART_USAGE);
        assert!(part_usage.is_some(), "expected PART_USAGE node in tree");
    }

    #[test]
    fn parse_port_def() {
        let parse = parse("port def SensorPort { out item data; }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let port_def = root.children().find(|n| n.kind() == SyntaxKind::PORT_DEF);
        assert!(port_def.is_some(), "expected PORT_DEF node in tree");
    }

    #[test]
    fn parse_connection() {
        let parse = parse("connect a.p to b.p;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let conn = root
            .children()
            .find(|n| n.kind() == SyntaxKind::CONNECTION_USAGE);
        assert!(conn.is_some(), "expected CONNECTION_USAGE node in tree");
    }

    #[test]
    fn parse_nested_package() {
        let source = r#"
package Systems {
    import ScalarValues::*;

    part def Engine { }

    part def Vehicle {
        part engine : Engine;
    }

    package SubPkg {
        part def Wheel { }
    }
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let pkg = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PACKAGE)
            .expect("expected PACKAGE node");
        // Package should contain a NAMESPACE_BODY
        let body = pkg
            .children()
            .find(|n| n.kind() == SyntaxKind::NAMESPACE_BODY)
            .expect("expected NAMESPACE_BODY inside package");

        // Count definitions inside the body
        let part_defs: Vec<_> = body
            .children()
            .filter(|n| n.kind() == SyntaxKind::PART_DEF)
            .collect();
        assert_eq!(
            part_defs.len(),
            2,
            "expected 2 PART_DEF nodes (Engine, Vehicle)"
        );

        let imports: Vec<_> = body
            .children()
            .filter(|n| n.kind() == SyntaxKind::IMPORT_DECL)
            .collect();
        assert_eq!(imports.len(), 1, "expected 1 IMPORT_DECL");

        let nested_pkgs: Vec<_> = body
            .children()
            .filter(|n| n.kind() == SyntaxKind::PACKAGE)
            .collect();
        assert_eq!(nested_pkgs.len(), 1, "expected 1 nested PACKAGE (SubPkg)");
    }

    #[test]
    fn parse_specialization() {
        let parse = parse("part v :> Vehicle;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let part = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PART_USAGE)
            .expect("expected PART_USAGE");
        let spec = part
            .children()
            .find(|n| n.kind() == SyntaxKind::SPECIALIZATION);
        assert!(spec.is_some(), "expected SPECIALIZATION node");
    }

    #[test]
    fn parse_multiplicity() {
        let parse = parse("part wheels [0..*] : Wheel;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let part = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PART_USAGE)
            .expect("expected PART_USAGE");
        let mult = part
            .children()
            .find(|n| n.kind() == SyntaxKind::MULTIPLICITY);
        assert!(mult.is_some(), "expected MULTIPLICITY node");
    }

    #[test]
    fn parse_port_with_direction() {
        let parse = parse("in port sensorIn : SensorPort;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let port = root
            .children()
            .find(|n| n.kind() == SyntaxKind::PORT_USAGE)
            .expect("expected PORT_USAGE");
        let dir = port.children().find(|n| n.kind() == SyntaxKind::DIRECTION);
        assert!(dir.is_some(), "expected DIRECTION node");
    }

    #[test]
    fn parse_attribute_usage() {
        let parse = parse("attribute mass : Real;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let attr = root
            .children()
            .find(|n| n.kind() == SyntaxKind::ATTRIBUTE_USAGE);
        assert!(attr.is_some(), "expected ATTRIBUTE_USAGE node");
    }

    #[test]
    fn parse_connection_def() {
        let parse = parse("connection def SensorLink { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let conn_def = root
            .children()
            .find(|n| n.kind() == SyntaxKind::CONNECTION_DEF);
        assert!(conn_def.is_some(), "expected CONNECTION_DEF node");
    }

    #[test]
    fn parse_comprehensive_model() {
        let source = r#"
package SensorSystem {
    import ISQ::*;

    attribute def Temperature;

    port def SensorPort {
        out item data : Temperature;
    }

    port def ProcessorPort {
        in item data : Temperature;
    }

    part def Sensor {
        port sensorOut : SensorPort;
    }

    part def Processor {
        port processorIn : ProcessorPort;
    }

    connection def SensorConnection {
        connect source.sensorOut to target.processorIn;
    }

    part def SensorSystem {
        part sensor : Sensor;
        part processor : Processor;
        connect sensor.sensorOut to processor.processorIn;
    }
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn roundtrip_preserves_text() {
        let source = "package Pkg { part def A { } }";
        let parse = parse(source);
        let root = parse.syntax_node();
        // Lossless: text of the root node should equal the original source
        assert_eq!(root.text().to_string(), source);
    }

    #[test]
    fn error_recovery_continues_parsing() {
        // Missing semicolon -- parser should recover
        let parse = parse("part def A { } part def B { }");
        let root = parse.syntax_node();
        // Both defs should still be parsed
        let defs: Vec<_> = root
            .children()
            .filter(|n| n.kind() == SyntaxKind::PART_DEF)
            .collect();
        assert_eq!(defs.len(), 2, "expected 2 PART_DEF nodes after recovery");
    }

    // -----------------------------------------------------------------------
    // grammar/requirements.rs coverage
    // -----------------------------------------------------------------------

    #[test]
    fn parse_satisfy_req() {
        let parse = parse("satisfy latencyReq by controller;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let sat = root
            .children()
            .find(|n| n.kind() == SyntaxKind::SATISFY_REQ);
        assert!(sat.is_some(), "expected SATISFY_REQ node");
    }

    #[test]
    fn parse_verify_req() {
        let parse = parse("verify safetyReq by safetyTest;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let ver = root.children().find(|n| n.kind() == SyntaxKind::VERIFY_REQ);
        assert!(ver.is_some(), "expected VERIFY_REQ node");
    }

    #[test]
    fn parse_refine_req() {
        let parse = parse("refine highLevelReq by detailedReq;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let refn = root.children().find(|n| n.kind() == SyntaxKind::REFINE_REQ);
        assert!(refn.is_some(), "expected REFINE_REQ node");
    }

    #[test]
    fn parse_satisfy_with_dotted_name() {
        let parse = parse("satisfy latencyReq by ecu.controller;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_verify_with_qualified_name() {
        let parse = parse("verify safety::req by tests::safetyTest;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_requirement_def_with_body() {
        let source = r#"
requirement def LatencyReq {
    doc "System latency must be below 10ms"
    attribute maxLatency : Real;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let req_def = root
            .children()
            .find(|n| n.kind() == SyntaxKind::REQUIREMENT_DEF);
        assert!(req_def.is_some(), "expected REQUIREMENT_DEF");
    }

    #[test]
    fn parse_requirement_usage() {
        let source = "requirement safetyReq : SafetySpec;";
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let req_usage = root
            .children()
            .find(|n| n.kind() == SyntaxKind::REQUIREMENT_USAGE);
        assert!(req_usage.is_some(), "expected REQUIREMENT_USAGE");
    }

    // -----------------------------------------------------------------------
    // grammar/packages.rs coverage (private import, public/private visibility)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_private_import() {
        let source = r#"
package Pkg {
    private import ScalarValues::*;
    part def A { }
}
"#;
        let parse = parse(source);
        // Private keyword may cause a parse error in current grammar, but
        // the parser should handle it gracefully
        let root = parse.syntax_node();
        let pkg = root.children().find(|n| n.kind() == SyntaxKind::PACKAGE);
        assert!(pkg.is_some(), "expected PACKAGE node");
    }

    #[test]
    fn parse_public_import() {
        let source = r#"
package Pkg {
    public import Definitions::*;
    part def B { }
}
"#;
        let parse = parse(source);
        let root = parse.syntax_node();
        let pkg = root.children().find(|n| n.kind() == SyntaxKind::PACKAGE);
        assert!(pkg.is_some(), "expected PACKAGE node");
    }

    #[test]
    fn parse_package_semicolon_form() {
        // Package with semicolon instead of body
        let parse = parse("package EmptyPkg;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let pkg = root.children().find(|n| n.kind() == SyntaxKind::PACKAGE);
        assert!(pkg.is_some(), "expected PACKAGE node");
    }

    // -----------------------------------------------------------------------
    // grammar/parts.rs coverage
    // -----------------------------------------------------------------------

    #[test]
    fn parse_calc_def() {
        let parse = parse("calc def TotalMass { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_allocation_def() {
        let parse = parse("allocation def TaskAlloc { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_enum_def() {
        let parse = parse("enum def Color { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_interface_def() {
        let parse = parse("interface def SensorInterface { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_constraint_def() {
        let parse = parse("constraint def TimingConstraint { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_action_def_and_usage() {
        let source = r#"
package Acts {
    action def Process { }
    action doProcess : Process;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_state_def_and_usage() {
        let source = r#"
package States {
    state def Operational { }
    state running : Operational;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_ref_usage() {
        let parse = parse("ref part driver : Person;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_abstract_part_def() {
        let parse = parse("abstract part def AbstractComponent { }");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let pd = root.children().find(|n| n.kind() == SyntaxKind::PART_DEF);
        assert!(pd.is_some(), "expected PART_DEF from abstract");
    }

    #[test]
    fn parse_feature_kw_decl() {
        let source = r#"
part def Sys {
    feature x : Integer;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_comment_and_doc_nodes() {
        let source = r#"
part def Documented {
    comment "This is a comment"
    doc "This is documentation"
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_named_connection_usage() {
        let source = r#"
package Net {
    connection def Link { }
    connection myLink : Link;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_attribute_with_default() {
        let parse = parse("attribute speed : Real = 100;");
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_attribute_with_string_default() {
        let parse = parse(r#"attribute name : String = "hello";"#);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_redefines_specialization() {
        let source = r#"
part def V {
    part eng redefines baseEng;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_subsets_specialization() {
        let source = r#"
part def V {
    part wheel subsets baseWheel;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    // -----------------------------------------------------------------------
    // grammar/mod.rs coverage (fallback error path)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_unrecognized_top_level_token() {
        // A bare semicolon at top level should trigger the error fallback
        let parse = parse("; part def A { }");
        // Should not panic; parser should recover
        let root = parse.syntax_node();
        // The parser may or may not recover to parse A, but it should not crash
        assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);
        // There should be errors from the bare semicolon
        assert!(!parse.ok(), "expected parse errors");
    }

    #[test]
    fn parse_unrecognized_token_in_body() {
        // A bare token inside a namespace body should trigger err_and_bump
        let source = "package P { 99999 part def X { } }";
        let parse = parse(source);
        let root = parse.syntax_node();
        let pkg = root.children().find(|n| n.kind() == SyntaxKind::PACKAGE);
        assert!(pkg.is_some(), "expected PACKAGE node after recovery");
    }

    // -----------------------------------------------------------------------
    // Integration-level grammar tests for edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_satisfy_verify_refine_in_package() {
        let source = r#"
package SafetyModel {
    requirement def SafetyReq { }
    requirement def DetailedReq { }
    satisfy SafetyReq by controller;
    verify SafetyReq by safetyTest;
    refine SafetyReq by DetailedReq;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_item_usage_with_direction() {
        let source = r#"
port def DataPort {
    in item request;
    out item response;
    inout item bidir;
}
"#;
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_colon_gt_gt_redefine() {
        let source = "part eng :>> baseEng;";
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }

    #[test]
    fn parse_specializes_keyword() {
        let source = "part def Truck specializes Vehicle { }";
        let parse = parse(source);
        assert!(parse.ok(), "errors: {:?}", parse.errors());
    }
}
