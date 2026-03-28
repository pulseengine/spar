//! Requirements extraction from SysML v2 CST.
//!
//! Walks the parsed SysML v2 CST, finds `REQUIREMENT_DEF` and `REQUIREMENT_USAGE`
//! nodes, and extracts them as rivet YAML artifacts.

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
}

/// Extract requirements from a parsed SysML v2 source file.
///
/// Returns a list of extracted requirements found in the CST.
pub fn extract_requirements_list(parse: &crate::Parse) -> Vec<ExtractedRequirement> {
    let root = parse.syntax_node();
    let mut reqs = Vec::new();
    let mut satisfies = Vec::new();
    let mut verifies = Vec::new();
    collect_requirements(&root, &mut reqs, &mut satisfies, &mut verifies);

    // Attach relationships to requirements
    for req in &mut reqs {
        for (req_name, by_name) in &satisfies {
            if req_name == &req.title {
                req.satisfies.push((req_name.clone(), by_name.clone()));
            }
        }
        for (req_name, by_name) in &verifies {
            if req_name == &req.title {
                req.verifies.push((req_name.clone(), by_name.clone()));
            }
        }
    }

    reqs
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
        yaml.push_str(&format!("  - id: {}\n", req.id));
        yaml.push_str("    type: requirement\n");
        yaml.push_str(&format!("    title: \"{}\"\n", req.title));
        yaml.push_str(&format!("    description: >\n      {}\n", req.description));
        yaml.push_str("    tags: [sysml2, extracted]\n");

        if !req.satisfies.is_empty() || !req.verifies.is_empty() {
            yaml.push_str("    links:\n");
            for (_, target) in &req.satisfies {
                yaml.push_str("      - type: satisfies\n");
                yaml.push_str(&format!("        target: {}\n", target));
            }
            for (_, target) in &req.verifies {
                yaml.push_str("      - type: verifies\n");
                yaml.push_str(&format!("        target: {}\n", target));
            }
        }
    }

    yaml
}

/// Recursively collect requirement nodes from the CST.
fn collect_requirements(
    node: &SyntaxNode,
    reqs: &mut Vec<ExtractedRequirement>,
    satisfies: &mut Vec<(String, String)>,
    verifies: &mut Vec<(String, String)>,
) {
    match node.kind() {
        SyntaxKind::REQUIREMENT_DEF => {
            if let Some(req) = extract_requirement_node(node) {
                reqs.push(req);
            }
        }
        SyntaxKind::REQUIREMENT_USAGE => {
            if let Some(req) = extract_requirement_node(node) {
                reqs.push(req);
            }
        }
        SyntaxKind::SATISFY_REQ => {
            if let Some((req_name, by_name)) = extract_relationship(node) {
                satisfies.push((req_name, by_name));
            }
        }
        SyntaxKind::VERIFY_REQ => {
            if let Some((req_name, by_name)) = extract_relationship(node) {
                verifies.push((req_name, by_name));
            }
        }
        _ => {}
    }

    for child in node.children() {
        collect_requirements(&child, reqs, satisfies, verifies);
    }
}

/// Extract a single requirement from a REQUIREMENT_DEF or REQUIREMENT_USAGE node.
fn extract_requirement_node(node: &SyntaxNode) -> Option<ExtractedRequirement> {
    let name = extract_name(node)?;

    // Look for doc comment in the body
    let description =
        extract_doc_comment(node).unwrap_or_else(|| "Extracted from SysML v2 model".to_string());

    let id = format!("SYSML-REQ-{name}");

    Some(ExtractedRequirement {
        id,
        title: name,
        description,
        tags: vec!["sysml2".to_string(), "extracted".to_string()],
        satisfies: Vec::new(),
        verifies: Vec::new(),
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
}
