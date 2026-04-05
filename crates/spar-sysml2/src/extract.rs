//! Requirements and architecture extraction from SysML v2 CST.
//!
//! Walks the parsed SysML v2 CST, finds `REQUIREMENT_DEF`, `REQUIREMENT_USAGE`,
//! `PART_DEF`, `CONNECTION_USAGE`, `ACTION_DEF`, and `STATE_DEF` nodes, and
//! extracts them as rivet YAML artifacts.

use crate::SyntaxNode;
use crate::syntax_kind::SyntaxKind;

/// A single extracted requirement.
#[derive(Debug, Clone)]
pub struct ExtractedRequirement {
    /// Requirement identifier (derived from name).
    pub id: String,
    /// Display title.
    pub title: String,
    /// Description text (from doc comment if available).
    pub description: String,
    /// Tags applied to this requirement.
    pub tags: Vec<String>,
    /// Satisfy relationships: (requirement_name, satisfier_name).
    pub satisfies: Vec<(String, String)>,
    /// Verify relationships: (requirement_name, verifier_name).
    pub verifies: Vec<(String, String)>,
    /// Refine relationships: (source_name, refined_by_name).
    pub refines: Vec<(String, String)>,
    /// Allocate relationships: (source_name, target_name).
    pub allocates: Vec<(String, String)>,
    /// Derive relationships: (source_name, derived_from_name).
    pub derives: Vec<(String, String)>,
}

/// An extracted architecture element (part, action, state).
#[derive(Debug, Clone)]
pub struct ExtractedComponent {
    /// Artifact identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Description text.
    pub description: String,
    /// Component kind: system, action, state.
    pub kind: ComponentKind,
    /// Child part names (subcomponents).
    pub children: Vec<String>,
    /// Port names.
    pub ports: Vec<String>,
}

/// Kind of architecture component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// `part def` — structural component.
    Part,
    /// `action def` — behavioral element.
    Action,
    /// `state def` — mode/state.
    State,
    /// `connection def` — bus/interface.
    Connection,
}

impl ComponentKind {
    fn rivet_type(self) -> &'static str {
        match self {
            Self::Part | Self::Connection => "design-decision",
            Self::Action => "feature",
            Self::State => "design-decision",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Part => "sysml-part",
            Self::Action => "sysml-action",
            Self::State => "sysml-state",
            Self::Connection => "sysml-connection",
        }
    }
}

/// An extracted connection between two components.
#[derive(Debug, Clone)]
pub struct ExtractedConnection {
    /// Source component or port.
    pub source: String,
    /// Target component or port.
    pub target: String,
}

/// Full extraction result: requirements + architecture.
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    /// Extracted requirements.
    pub requirements: Vec<ExtractedRequirement>,
    /// Extracted architecture components (when `include_architecture` is set).
    pub components: Vec<ExtractedComponent>,
    /// Extracted connections (when `include_architecture` is set).
    pub connections: Vec<ExtractedConnection>,
}

/// Extract requirements from a parsed SysML v2 source file.
///
/// Returns a list of extracted requirements found in the CST.
pub fn extract_requirements_list(parse: &crate::Parse) -> Vec<ExtractedRequirement> {
    let root = parse.syntax_node();
    let mut reqs = Vec::new();
    let mut relationships = RelationshipCollector::default();
    collect_requirements(&root, &mut reqs, &mut relationships);

    // Attach relationships to requirements (case-insensitive matching,
    // since SysML names may differ in casing from requirement titles).
    for req in &mut reqs {
        for (req_name, by_name) in &relationships.satisfies {
            if req_name.eq_ignore_ascii_case(&req.title) {
                req.satisfies.push((req_name.clone(), by_name.clone()));
            }
        }
        for (req_name, by_name) in &relationships.verifies {
            if req_name.eq_ignore_ascii_case(&req.title) {
                req.verifies.push((req_name.clone(), by_name.clone()));
            }
        }
        for (req_name, by_name) in &relationships.refines {
            if req_name.eq_ignore_ascii_case(&req.title) {
                req.refines.push((req_name.clone(), by_name.clone()));
            }
        }
        for (source, target) in &relationships.allocates {
            if source.eq_ignore_ascii_case(&req.title) {
                req.allocates.push((source.clone(), target.clone()));
            }
        }
        for (source, from_name) in &relationships.derives {
            if source.eq_ignore_ascii_case(&req.title) {
                req.derives.push((source.clone(), from_name.clone()));
            }
        }
    }

    reqs
}

/// Extract requirements and optionally architecture context from a SysML v2 file.
///
/// When `include_architecture` is true, also extracts `part def`, `action def`,
/// `state def`, `connection def`, and `connection usage` nodes as rivet artifacts.
pub fn extract_all(parse: &crate::Parse, include_architecture: bool) -> ExtractionResult {
    let reqs = extract_requirements_list(parse);

    let mut components = Vec::new();
    let mut connections = Vec::new();

    if include_architecture {
        let root = parse.syntax_node();
        collect_architecture(&root, &mut components, &mut connections);
    }

    ExtractionResult {
        requirements: reqs,
        components,
        connections,
    }
}

/// Extract all artifacts (requirements + architecture) as rivet YAML.
pub fn extract_all_yaml(parse: &crate::Parse, include_architecture: bool) -> String {
    let result = extract_all(parse, include_architecture);

    if result.requirements.is_empty() && result.components.is_empty() {
        return "artifacts: []\n".to_string();
    }

    let mut yaml = String::from("artifacts:\n");

    // Requirements
    for req in &result.requirements {
        write_requirement_yaml(req, &mut yaml);
    }

    // Architecture components
    for comp in &result.components {
        yaml.push_str(&format!("  - id: SYSML-{}\n", comp.id));
        yaml.push_str(&format!("    type: {}\n", comp.kind.rivet_type()));
        yaml.push_str(&format!(
            "    title: \"{}\"\n",
            yaml_escape_str(&comp.title)
        ));
        yaml.push_str(&format!(
            "    description: >\n      {}\n",
            yaml_escape_str(&comp.description)
        ));
        yaml.push_str(&format!(
            "    tags: [sysml2, extracted, {}]\n",
            comp.kind.tag()
        ));

        // Write connections as traces-to links
        let conn_links: Vec<&ExtractedConnection> = result
            .connections
            .iter()
            .filter(|c| c.source.eq_ignore_ascii_case(&comp.title))
            .collect();
        if !conn_links.is_empty() {
            yaml.push_str("    links:\n");
            for conn in conn_links {
                yaml.push_str("      - type: traces-to\n");
                yaml.push_str(&format!(
                    "        target: {}\n",
                    yaml_escape_str(&conn.target)
                ));
            }
        }
    }

    yaml
}

/// Recursively collect architecture elements from the CST.
fn collect_architecture(
    node: &SyntaxNode,
    components: &mut Vec<ExtractedComponent>,
    connections: &mut Vec<ExtractedConnection>,
) {
    match node.kind() {
        SyntaxKind::PART_DEF => {
            if let Some(comp) = extract_component_node(node, ComponentKind::Part) {
                components.push(comp);
            }
        }
        SyntaxKind::ACTION_DEF => {
            if let Some(comp) = extract_component_node(node, ComponentKind::Action) {
                components.push(comp);
            }
        }
        SyntaxKind::STATE_DEF => {
            if let Some(comp) = extract_component_node(node, ComponentKind::State) {
                components.push(comp);
            }
        }
        SyntaxKind::CONNECTION_DEF => {
            if let Some(comp) = extract_component_node(node, ComponentKind::Connection) {
                components.push(comp);
            }
        }
        SyntaxKind::CONNECTION_USAGE => {
            if let Some(conn) = extract_connection_usage(node) {
                connections.push(conn);
            }
        }
        _ => {}
    }

    for child in node.children() {
        collect_architecture(&child, components, connections);
    }
}

/// Extract a component from PART_DEF, ACTION_DEF, STATE_DEF, or CONNECTION_DEF.
fn extract_component_node(node: &SyntaxNode, kind: ComponentKind) -> Option<ExtractedComponent> {
    let name = extract_name(node)?;
    let description = extract_doc_comment(node).unwrap_or_else(|| {
        let label = match kind {
            ComponentKind::Part => "part",
            ComponentKind::Action => "action",
            ComponentKind::State => "state",
            ComponentKind::Connection => "connection",
        };
        format!("SysML v2 {label} definition")
    });

    // Collect child part usages and port usages
    let mut children = Vec::new();
    let mut ports = Vec::new();
    for child in node.descendants() {
        match child.kind() {
            SyntaxKind::PART_USAGE => {
                if let Some(n) = extract_name(&child) {
                    children.push(n);
                }
            }
            SyntaxKind::PORT_USAGE => {
                if let Some(n) = extract_name(&child) {
                    ports.push(n);
                }
            }
            _ => {}
        }
    }

    let prefix = match kind {
        ComponentKind::Part => "PART",
        ComponentKind::Action => "ACTION",
        ComponentKind::State => "STATE",
        ComponentKind::Connection => "CONN",
    };

    Some(ExtractedComponent {
        id: format!("{prefix}-{name}"),
        title: name,
        description,
        kind,
        children,
        ports,
    })
}

/// Extract a connection usage: `connect a.p to b.p;`
fn extract_connection_usage(node: &SyntaxNode) -> Option<ExtractedConnection> {
    let name_refs: Vec<String> = node
        .children()
        .filter(|c| c.kind() == SyntaxKind::NAME_REF)
        .map(|c| c.text().to_string().trim().to_string())
        .collect();

    if name_refs.len() >= 2 {
        Some(ExtractedConnection {
            source: name_refs[0].clone(),
            target: name_refs[1].clone(),
        })
    } else {
        None
    }
}

/// Collected relationships from CST walk.
#[derive(Default)]
struct RelationshipCollector {
    satisfies: Vec<(String, String)>,
    verifies: Vec<(String, String)>,
    refines: Vec<(String, String)>,
    allocates: Vec<(String, String)>,
    derives: Vec<(String, String)>,
}

/// Extract requirements from a parsed SysML v2 source file as rivet YAML.
///
/// Returns a YAML string suitable for use with rivet's artifact system.
/// If no requirements are found, returns a YAML with an empty artifacts list.
pub fn extract_requirements(parse: &crate::Parse) -> String {
    let reqs = extract_requirements_list(parse);

    if reqs.is_empty() {
        return "artifacts: []\n".to_string();
    }

    let mut yaml = String::from("artifacts:\n");
    for req in &reqs {
        write_requirement_yaml(req, &mut yaml);
    }
    yaml
}

/// Escape a string for safe embedding in YAML double-quoted values.
fn yaml_escape_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Write a single requirement as YAML.
fn write_requirement_yaml(req: &ExtractedRequirement, yaml: &mut String) {
    yaml.push_str(&format!("  - id: {}\n", req.id));
    yaml.push_str("    type: requirement\n");
    yaml.push_str(&format!("    title: \"{}\"\n", yaml_escape_str(&req.title)));
    yaml.push_str(&format!(
        "    description: >\n      {}\n",
        yaml_escape_str(&req.description)
    ));
    yaml.push_str("    tags: [sysml2, extracted]\n");

    let has_links = !req.satisfies.is_empty()
        || !req.verifies.is_empty()
        || !req.refines.is_empty()
        || !req.allocates.is_empty()
        || !req.derives.is_empty();

    if has_links {
        yaml.push_str("    links:\n");
        for (_, target) in &req.satisfies {
            yaml.push_str("      - type: satisfies\n");
            yaml.push_str(&format!("        target: {}\n", yaml_escape_str(target)));
        }
        for (_, target) in &req.verifies {
            yaml.push_str("      - type: verifies\n");
            yaml.push_str(&format!("        target: {}\n", yaml_escape_str(target)));
        }
        for (_, target) in &req.refines {
            yaml.push_str("      - type: refines\n");
            yaml.push_str(&format!("        target: {}\n", yaml_escape_str(target)));
        }
        for (_, target) in &req.allocates {
            yaml.push_str("      - type: allocated-to\n");
            yaml.push_str(&format!("        target: {}\n", yaml_escape_str(target)));
        }
        for (_, target) in &req.derives {
            yaml.push_str("      - type: derives-from\n");
            yaml.push_str(&format!("        target: {}\n", yaml_escape_str(target)));
        }
    }
}

/// Recursively collect requirement nodes from the CST.
fn collect_requirements(
    node: &SyntaxNode,
    reqs: &mut Vec<ExtractedRequirement>,
    rels: &mut RelationshipCollector,
) {
    match node.kind() {
        SyntaxKind::REQUIREMENT_DEF | SyntaxKind::REQUIREMENT_USAGE => {
            if let Some(req) = extract_requirement_node(node) {
                reqs.push(req);
            }
        }
        SyntaxKind::SATISFY_REQ => {
            if let Some(pair) = extract_relationship(node) {
                rels.satisfies.push(pair);
            }
        }
        SyntaxKind::VERIFY_REQ => {
            if let Some(pair) = extract_relationship(node) {
                rels.verifies.push(pair);
            }
        }
        SyntaxKind::REFINE_REQ => {
            if let Some(pair) = extract_relationship(node) {
                rels.refines.push(pair);
            }
        }
        SyntaxKind::ALLOCATE_REQ => {
            if let Some(pair) = extract_relationship(node) {
                rels.allocates.push(pair);
            }
        }
        SyntaxKind::DERIVE_REQ => {
            if let Some(pair) = extract_relationship(node) {
                rels.derives.push(pair);
            }
        }
        _ => {}
    }

    for child in node.children() {
        collect_requirements(&child, reqs, rels);
    }
}

/// Extract a single requirement from a REQUIREMENT_DEF or REQUIREMENT_USAGE node.
fn extract_requirement_node(node: &SyntaxNode) -> Option<ExtractedRequirement> {
    let name = extract_name(node)?;

    // Look for doc comment in the body
    let description =
        extract_doc_comment(node).unwrap_or_else(|| "Extracted from SysML v2 model".to_string());

    // Qualify ID with enclosing package name to prevent duplicates
    // across packages (e.g., package A { req R } package B { req R }).
    let pkg_prefix = find_enclosing_package(node)
        .map(|p| format!("{p}-"))
        .unwrap_or_default();
    let id = format!("SYSML-REQ-{pkg_prefix}{name}");

    Some(ExtractedRequirement {
        id,
        title: name,
        description,
        tags: vec!["sysml2".to_string(), "extracted".to_string()],
        satisfies: Vec::new(),
        verifies: Vec::new(),
        refines: Vec::new(),
        allocates: Vec::new(),
        derives: Vec::new(),
    })
}

/// Extract a satisfy/verify relationship: returns (req_name, by_name).
fn extract_relationship(node: &SyntaxNode) -> Option<(String, String)> {
    let name_refs: Vec<String> = node
        .children()
        .filter(|c| c.kind() == SyntaxKind::NAME_REF)
        .map(|c| c.text().to_string().trim().to_string())
        .collect();

    if name_refs.len() >= 2 {
        Some((name_refs[0].clone(), name_refs[1].clone()))
    } else {
        None
    }
}

/// Walk up ancestors to find the enclosing PACKAGE node's name.
fn find_enclosing_package(node: &SyntaxNode) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == SyntaxKind::PACKAGE {
            return extract_name(&n);
        }
        current = n.parent();
    }
    None
}

/// Extract the name from a requirement node.
///
/// For REQUIREMENT_DEF: `requirement def Name { ... }` -> "Name"
/// For REQUIREMENT_USAGE: `requirement name : Type { ... }` -> "name"
fn extract_name(node: &SyntaxNode) -> Option<String> {
    // Look for NAME child first
    for child in node.children() {
        if child.kind() == SyntaxKind::NAME {
            let text = child.text().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    // Look for bare IDENT token
    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.into_token()
            && tok.kind() == SyntaxKind::IDENT
        {
            return Some(tok.text().to_string());
        }
    }
    None
}

/// Extract a doc comment from a node's body.
///
/// Looks for block comments following a `doc` keyword, or for DOC_NODE children.
fn extract_doc_comment(node: &SyntaxNode) -> Option<String> {
    // Check for DOC_NODE child
    for child in node.children() {
        if child.kind() == SyntaxKind::DOC_NODE {
            // Extract string literal or block comment text
            for tok in child.children_with_tokens() {
                if let Some(t) = tok.into_token() {
                    if t.kind() == SyntaxKind::STRING_LIT {
                        let text = t.text().to_string();
                        // Remove quotes
                        return Some(
                            text.trim_start_matches('"')
                                .trim_end_matches('"')
                                .to_string(),
                        );
                    }
                    if t.kind() == SyntaxKind::BLOCK_COMMENT {
                        let text = t.text().to_string();
                        return Some(
                            text.trim_start_matches("/*")
                                .trim_end_matches("*/")
                                .trim()
                                .to_string(),
                        );
                    }
                }
            }
        }
    }

    // Also check NAMESPACE_BODY for DOC_NODE
    if let Some(body) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::NAMESPACE_BODY)
    {
        for child in body.children() {
            if child.kind() == SyntaxKind::DOC_NODE {
                for tok in child.children_with_tokens() {
                    if let Some(t) = tok.into_token()
                        && t.kind() == SyntaxKind::STRING_LIT
                    {
                        let text = t.text().to_string();
                        return Some(
                            text.trim_start_matches('"')
                                .trim_end_matches('"')
                                .to_string(),
                        );
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_requirement() {
        let parse = crate::parse("requirement def LatencyReq { }");
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("SYSML-REQ-LatencyReq"), "yaml: {yaml}");
        assert!(yaml.contains("type: requirement"), "yaml: {yaml}");
        assert!(yaml.contains("title: \"LatencyReq\""), "yaml: {yaml}");
        assert!(yaml.contains("tags: [sysml2, extracted]"), "yaml: {yaml}");
    }

    #[test]
    fn extract_requirement_with_satisfy() {
        let source = r#"
requirement def LatencyReq { }
satisfy LatencyReq by controller;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("SYSML-REQ-LatencyReq"), "yaml: {yaml}");
        assert!(yaml.contains("type: satisfies"), "yaml: {yaml}");
        assert!(yaml.contains("target: controller"), "yaml: {yaml}");
    }

    #[test]
    fn extract_empty_model_no_requirements() {
        let parse = crate::parse("part def Vehicle { }");
        let yaml = extract_requirements(&parse);
        assert_eq!(yaml, "artifacts: []\n");
    }

    #[test]
    fn extract_multiple_requirements() {
        let source = r#"
requirement def SafetyReq { }
requirement def LatencyReq { }
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].id, "SYSML-REQ-SafetyReq");
        assert_eq!(reqs[1].id, "SYSML-REQ-LatencyReq");
    }

    #[test]
    fn extract_requirement_with_verify() {
        let source = r#"
requirement def LatencyReq { }
verify LatencyReq by latencyTest;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("type: verifies"), "yaml: {yaml}");
        assert!(yaml.contains("target: latencyTest"), "yaml: {yaml}");
    }

    #[test]
    fn extract_requirement_inside_package() {
        let source = r#"
package Safety {
    requirement def CriticalReq { }
    satisfy CriticalReq by safetyController;
}
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].id, "SYSML-REQ-CriticalReq");
        assert_eq!(reqs[0].satisfies.len(), 1);
    }

    #[test]
    fn extract_requirement_usage() {
        let source = "requirement latencyReq : LatencySpec { }";
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1, "should extract requirement usage");
        assert_eq!(reqs[0].id, "SYSML-REQ-latencyReq");
        assert_eq!(reqs[0].title, "latencyReq");
    }

    #[test]
    fn extract_requirement_with_doc_string() {
        let source = r#"
requirement def SafetyReq {
    doc "The system shall remain safe under all conditions"
}
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].description,
            "The system shall remain safe under all conditions"
        );
    }

    #[test]
    fn extract_requirement_with_both_satisfy_and_verify() {
        let source = r#"
requirement def SafetyReq { }
satisfy SafetyReq by controller;
verify SafetyReq by safetyTest;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("links:"), "yaml: {yaml}");
        assert!(yaml.contains("type: satisfies"), "yaml: {yaml}");
        assert!(yaml.contains("target: controller"), "yaml: {yaml}");
        assert!(yaml.contains("type: verifies"), "yaml: {yaml}");
        assert!(yaml.contains("target: safetyTest"), "yaml: {yaml}");
    }

    #[test]
    fn extract_multiple_requirements_yaml_format() {
        let source = r#"
requirement def SafetyReq { }
requirement def LatencyReq { }
requirement def ReliabilityReq { }
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.starts_with("artifacts:\n"), "yaml: {yaml}");
        assert!(yaml.contains("SYSML-REQ-SafetyReq"), "yaml: {yaml}");
        assert!(yaml.contains("SYSML-REQ-LatencyReq"), "yaml: {yaml}");
        assert!(yaml.contains("SYSML-REQ-ReliabilityReq"), "yaml: {yaml}");
        // Each should have description fallback
        assert!(
            yaml.contains("Extracted from SysML v2 model"),
            "yaml: {yaml}"
        );
    }

    #[test]
    fn extract_requirement_default_description() {
        let source = "requirement def EmptyReq { }";
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].description, "Extracted from SysML v2 model");
    }

    #[test]
    fn extract_deeply_nested_requirements() {
        let source = r#"
package TopLevel {
    package Safety {
        requirement def NestedReq { }
        satisfy NestedReq by safetyController;
        verify NestedReq by safetyTest;
    }
}
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].id, "SYSML-REQ-NestedReq");
        assert_eq!(reqs[0].satisfies.len(), 1);
        assert_eq!(reqs[0].verifies.len(), 1);
    }

    #[test]
    fn extract_unmatched_satisfy_ignored() {
        // satisfy references a requirement name that doesn't exist
        let source = r#"
requirement def AlphaReq { }
satisfy BetaReq by controller;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].id, "SYSML-REQ-AlphaReq");
        // The satisfy for BetaReq should not attach to AlphaReq
        assert_eq!(reqs[0].satisfies.len(), 0);
    }

    #[test]
    fn extract_refine_relationship() {
        let source = r#"
requirement def HighLevelReq { }
refine HighLevelReq by DetailedReq;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].refines.len(), 1);
        assert_eq!(reqs[0].refines[0].1, "DetailedReq");
    }

    #[test]
    fn extract_refine_in_yaml() {
        let source = r#"
requirement def SafetyReq { }
refine SafetyReq by DetailedSafetyReq;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("type: refines"), "yaml: {yaml}");
        assert!(yaml.contains("target: DetailedSafetyReq"), "yaml: {yaml}");
    }

    #[test]
    fn extract_allocate_relationship() {
        let source = r#"
requirement def ProcessingReq { }
allocate ProcessingReq to ecu;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].allocates.len(), 1);
        assert_eq!(reqs[0].allocates[0].1, "ecu");
    }

    #[test]
    fn extract_allocate_in_yaml() {
        let source = r#"
requirement def ProcessingReq { }
allocate ProcessingReq to ecu;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("type: allocated-to"), "yaml: {yaml}");
        assert!(yaml.contains("target: ecu"), "yaml: {yaml}");
    }

    #[test]
    fn extract_derive_relationship() {
        let source = r#"
requirement def DetailedReq { }
derive DetailedReq from SystemReq;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].derives.len(), 1);
        assert_eq!(reqs[0].derives[0].1, "SystemReq");
    }

    #[test]
    fn extract_derive_in_yaml() {
        let source = r#"
requirement def DetailedReq { }
derive DetailedReq from SystemReq;
"#;
        let parse = crate::parse(source);
        let yaml = extract_requirements(&parse);
        assert!(yaml.contains("type: derives-from"), "yaml: {yaml}");
        assert!(yaml.contains("target: SystemReq"), "yaml: {yaml}");
    }

    #[test]
    fn extract_all_relationship_types() {
        let source = r#"
requirement def MainReq { }
satisfy MainReq by controller;
verify MainReq by testSuite;
refine MainReq by DetailedReq;
allocate MainReq to ecu;
derive MainReq from SystemReq;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].satisfies.len(), 1);
        assert_eq!(reqs[0].verifies.len(), 1);
        assert_eq!(reqs[0].refines.len(), 1);
        assert_eq!(reqs[0].allocates.len(), 1);
        assert_eq!(reqs[0].derives.len(), 1);
    }

    #[test]
    fn extract_relationships_inside_package() {
        let source = r#"
package Safety {
    requirement def CriticalReq { }
    satisfy CriticalReq by safetyController;
    refine CriticalReq by DetailedCriticalReq;
    allocate CriticalReq to safetyCpu;
}
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].satisfies.len(), 1);
        assert_eq!(reqs[0].refines.len(), 1);
        assert_eq!(reqs[0].allocates.len(), 1);
    }

    // ── Architecture extraction tests ──────────────────────────────

    #[test]
    fn extract_part_def_as_component() {
        let source = "part def Vehicle { }";
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].title, "Vehicle");
        assert_eq!(result.components[0].kind, ComponentKind::Part);
        assert_eq!(result.components[0].id, "PART-Vehicle");
    }

    #[test]
    fn extract_action_def_as_component() {
        let source = "action def ProcessData { }";
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].kind, ComponentKind::Action);
        assert_eq!(result.components[0].id, "ACTION-ProcessData");
    }

    #[test]
    fn extract_state_def_as_component() {
        let source = "state def OperatingMode { }";
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].kind, ComponentKind::State);
        assert_eq!(result.components[0].id, "STATE-OperatingMode");
    }

    #[test]
    fn extract_architecture_disabled_by_default() {
        let source = "part def Vehicle { }";
        let parse = crate::parse(source);
        let result = extract_all(&parse, false);
        assert!(
            result.components.is_empty(),
            "architecture should not be extracted when disabled"
        );
    }

    #[test]
    fn extract_part_with_children() {
        let source = r#"
part def Vehicle {
    part engine : Engine;
    part transmission : Transmission;
}
"#;
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].children.len(), 2);
    }

    #[test]
    fn extract_part_with_ports() {
        let source = r#"
part def ECU {
    port sensorIn : SensorPort;
    port actuatorOut : ActuatorPort;
}
"#;
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 1);
        assert_eq!(result.components[0].ports.len(), 2);
    }

    #[test]
    fn extract_mixed_requirements_and_architecture() {
        let source = r#"
requirement def SafetyReq { }
part def Controller { }
action def ProcessSensor { }
satisfy SafetyReq by Controller;
"#;
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.requirements.len(), 1);
        assert_eq!(result.components.len(), 2); // Controller + ProcessSensor
        assert_eq!(result.requirements[0].satisfies.len(), 1);
    }

    #[test]
    fn extract_all_yaml_includes_architecture() {
        let source = r#"
requirement def SafetyReq { }
part def Controller { }
"#;
        let parse = crate::parse(source);
        let yaml = extract_all_yaml(&parse, true);
        assert!(yaml.contains("SYSML-REQ-SafetyReq"), "yaml: {yaml}");
        assert!(yaml.contains("SYSML-PART-Controller"), "yaml: {yaml}");
        assert!(yaml.contains("sysml-part"), "yaml: {yaml}");
    }

    #[test]
    fn extract_architecture_in_package() {
        let source = r#"
package Automotive {
    part def Vehicle { }
    part def Engine { }
    action def StartEngine { }
}
"#;
        let parse = crate::parse(source);
        let result = extract_all(&parse, true);
        assert_eq!(result.components.len(), 3);
    }

    // ── YAML escaping tests ──────────────────────────────────────

    #[test]
    fn yaml_escape_str_double_quote() {
        assert_eq!(yaml_escape_str(r#"say "hello""#), r#"say \"hello\""#);
    }

    #[test]
    fn yaml_escape_str_backslash() {
        assert_eq!(yaml_escape_str(r"a\b"), r"a\\b");
    }

    #[test]
    fn yaml_escape_str_newline_tab() {
        assert_eq!(yaml_escape_str("a\nb\tc"), r"a\nb\tc");
    }

    #[test]
    fn yaml_escape_str_carriage_return() {
        assert_eq!(yaml_escape_str("a\rb"), r"a\rb");
    }

    #[test]
    fn yaml_escape_applied_in_requirement_title() {
        // Craft a requirement whose name contains a double-quote character.
        // We can't get the parser to emit such a name, but we can exercise
        // write_requirement_yaml directly.
        let req = ExtractedRequirement {
            id: "SYSML-REQ-X".into(),
            title: r#"Req "A""#.into(),
            description: "desc".into(),
            tags: vec![],
            satisfies: vec![],
            verifies: vec![],
            refines: vec![],
            allocates: vec![],
            derives: vec![],
        };
        let mut yaml = String::new();
        write_requirement_yaml(&req, &mut yaml);
        // The title must be escaped so YAML stays valid
        assert!(yaml.contains(r#"title: "Req \"A\"""#), "yaml: {yaml}");
    }

    // ── Case-insensitive relationship matching ───────────────────

    #[test]
    fn relationship_match_case_insensitive() {
        let source = r#"
requirement def latencyReq { }
satisfy LatencyReq by controller;
"#;
        let parse = crate::parse(source);
        let reqs = extract_requirements_list(&parse);
        assert_eq!(reqs.len(), 1);
        // Even though the satisfy uses "LatencyReq" and the def uses
        // "latencyReq", the relationship should still attach.
        assert_eq!(
            reqs[0].satisfies.len(),
            1,
            "case-insensitive match should attach satisfy: {:?}",
            reqs[0]
        );
    }
}
