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
}
