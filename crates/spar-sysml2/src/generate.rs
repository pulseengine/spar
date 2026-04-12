//! Roundtrip generation: rivet YAML artifacts → SysML v2 source.
//!
//! Reads rivet requirement and component artifacts and generates SysML v2
//! source with `requirement def`, `satisfy`, `verify`, `refine`, `allocate`,
//! and `derive` relationships.

use std::collections::HashMap;

/// A rivet artifact parsed from YAML.
#[derive(Debug, Clone)]
pub struct RivetArtifact {
    pub id: String,
    pub artifact_type: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub links: Vec<RivetLink>,
}

/// A link between rivet artifacts.
#[derive(Debug, Clone)]
pub struct RivetLink {
    pub link_type: String,
    pub target: String,
}

/// Parse rivet YAML artifacts from a string.
///
/// Handles the `artifacts:` top-level key with a list of artifact maps.
pub fn parse_rivet_yaml(yaml: &str) -> Vec<RivetArtifact> {
    let mut artifacts = Vec::new();
    let mut current: Option<RivetArtifactBuilder> = None;
    let mut in_links = false;
    let mut current_link: Option<(String, String)> = None;

    for line in yaml.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("- id:") {
            // Flush previous artifact
            if let Some(builder) = current.take() {
                if let Some((lt, tgt)) = current_link.take() {
                    let mut b = builder;
                    b.links.push(RivetLink {
                        link_type: lt,
                        target: tgt,
                    });
                    artifacts.push(b.build());
                } else {
                    artifacts.push(builder.build());
                }
            }
            in_links = false;
            current_link = None;

            let id = trimmed.trim_start_matches("- id:").trim().to_string();
            current = Some(RivetArtifactBuilder {
                id,
                ..Default::default()
            });
            continue;
        }

        let Some(ref mut builder) = current else {
            continue;
        };

        if trimmed.starts_with("type:") {
            builder.artifact_type = trimmed.trim_start_matches("type:").trim().to_string();
        } else if trimmed.starts_with("title:") {
            let val = trimmed.trim_start_matches("title:").trim();
            builder.title = val.trim_matches('"').to_string();
        } else if trimmed.starts_with("description:") {
            let val = trimmed.trim_start_matches("description:").trim();
            if val != ">" {
                builder.description = val.to_string();
            }
        } else if trimmed.starts_with("tags:") {
            let val = trimmed.trim_start_matches("tags:").trim();
            let val = val.trim_start_matches('[').trim_end_matches(']');
            builder.tags = val.split(',').map(|s| s.trim().to_string()).collect();
        } else if trimmed == "links:" {
            in_links = true;
        } else if in_links && trimmed.starts_with("- type:") {
            // Flush previous link
            if let Some((lt, tgt)) = current_link.take() {
                builder.links.push(RivetLink {
                    link_type: lt,
                    target: tgt,
                });
            }
            let lt = trimmed.trim_start_matches("- type:").trim().to_string();
            current_link = Some((lt, String::new()));
        } else if in_links && trimmed.starts_with("target:") {
            if let Some((_, ref mut tgt)) = current_link {
                *tgt = trimmed.trim_start_matches("target:").trim().to_string();
            }
        } else if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("artifacts:")
            && !trimmed.starts_with("status:")
        {
            // Could be continuation of description
            if builder.description.is_empty() || builder.description == ">" {
                builder.description = trimmed.to_string();
            }
        }
    }

    // Flush last artifact
    if let Some(mut builder) = current {
        if let Some((lt, tgt)) = current_link {
            builder.links.push(RivetLink {
                link_type: lt,
                target: tgt,
            });
        }
        artifacts.push(builder.build());
    }

    artifacts
}

#[derive(Default)]
struct RivetArtifactBuilder {
    id: String,
    artifact_type: String,
    title: String,
    description: String,
    tags: Vec<String>,
    links: Vec<RivetLink>,
}

impl RivetArtifactBuilder {
    fn build(self) -> RivetArtifact {
        RivetArtifact {
            id: self.id,
            artifact_type: self.artifact_type,
            title: self.title,
            description: self.description,
            tags: self.tags,
            links: self.links,
        }
    }
}

/// Generate SysML v2 source from rivet artifacts.
///
/// Produces a SysML v2 source string with:
/// - `requirement def` for each requirement artifact
/// - `part def` for each design-decision tagged `sysml-part` (with port comments)
/// - `port def` for each `sysml-interface` artifact
/// - `satisfy`, `verify`, `refine`, `allocate`, `derive` relationships
/// - Package wrapping based on ID prefixes
pub fn generate_sysml2(artifacts: &[RivetArtifact]) -> String {
    let mut out = String::new();
    out.push_str("// Generated from rivet artifacts by spar\n\n");

    let id_to_title: HashMap<&str, &str> = artifacts
        .iter()
        .map(|a| (a.id.as_str(), a.title.as_str()))
        .collect();

    // Collect port defs (sysml-interface artifacts).
    let interfaces: Vec<&RivetArtifact> = artifacts
        .iter()
        .filter(|a| a.artifact_type == "sysml-interface")
        .collect();

    // Group: requirements first, then components
    let requirements: Vec<&RivetArtifact> = artifacts
        .iter()
        .filter(|a| a.artifact_type == "requirement" || a.artifact_type == "sysml-requirement")
        .collect();

    let components: Vec<&RivetArtifact> = artifacts
        .iter()
        .filter(|a| a.artifact_type == "design-decision" || a.artifact_type == "sysml-component")
        .filter(|a| {
            a.tags.iter().any(|t| {
                t == "sysml-part"
                    || t == "sysml-action"
                    || t == "sysml-state"
                    || t == "sysml-connection"
                    || t == "sysml2"
            })
        })
        .collect();

    // Build a reverse map: component ID -> list of interface titles that link to it.
    // An interface links to a component via "allocated-to" or "satisfies".
    let component_ports: HashMap<&str, Vec<&str>> = {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for iface in &interfaces {
            for link in &iface.links {
                if link.link_type == "allocated-to" || link.link_type == "satisfies" {
                    map.entry(link.target.as_str())
                        .or_default()
                        .push(iface.title.as_str());
                }
            }
        }
        map
    };

    // Compute package groups from ID prefixes.
    // An ID like "REQ-001" has prefix "REQ"; "SYS-CTRL-001" has prefix "SYS-CTRL".
    // Only create package wrapping if there are multiple distinct prefixes.
    let prefixes = collect_id_prefixes(artifacts);
    let use_packages = prefixes.len() > 1;

    // Group all "definition" output by prefix for package wrapping.
    let mut package_bodies: HashMap<String, String> = HashMap::new();

    // Emit port defs for interfaces.
    for iface in &interfaces {
        let name = sanitize_sysml_name(&iface.title);
        let body = package_bodies.entry(id_prefix(&iface.id)).or_default();
        if !iface.description.is_empty() {
            body.push_str(&format!(
                "port def {name} {{\n    doc \"{}\"\n}}\n\n",
                escape_string(&iface.description)
            ));
        } else {
            body.push_str(&format!("port def {name} {{ }}\n\n"));
        }
    }

    // Emit requirement defs
    for req in &requirements {
        let name = sanitize_sysml_name(&req.title);
        let body = package_bodies.entry(id_prefix(&req.id)).or_default();
        body.push_str(&format!("requirement def {name}"));
        if !req.description.is_empty() {
            body.push_str(&format!(
                " {{\n    doc \"{}\"\n}}\n\n",
                escape_string(&req.description)
            ));
        } else {
            body.push_str(" { }\n\n");
        }
    }

    // Emit part defs for components, with port comments.
    for comp in &components {
        let name = sanitize_sysml_name(&comp.title);
        let keyword = if comp.tags.iter().any(|t| t == "sysml-action") {
            "action"
        } else if comp.tags.iter().any(|t| t == "sysml-state") {
            "state"
        } else if comp.tags.iter().any(|t| t == "sysml-connection") {
            "connection"
        } else {
            "part"
        };

        let body = package_bodies.entry(id_prefix(&comp.id)).or_default();

        // Check if this component has associated ports (from sysml-interface links).
        let ports = component_ports.get(comp.id.as_str());
        if let Some(port_names) = ports {
            body.push_str(&format!("{keyword} def {name} {{\n"));
            for port_name in port_names {
                let sanitized = sanitize_sysml_name(port_name);
                body.push_str(&format!("    // port: {sanitized}\n"));
            }
            body.push_str("}\n\n");
        } else {
            body.push_str(&format!("{keyword} def {name} {{ }}\n\n"));
        }
    }

    // Emit relationships
    for req in &requirements {
        let name = sanitize_sysml_name(&req.title);
        let body = package_bodies.entry(id_prefix(&req.id)).or_default();
        for link in &req.links {
            let target_name = id_to_title
                .get(link.target.as_str())
                .map(|t| sanitize_sysml_name(t))
                .unwrap_or_else(|| sanitize_sysml_name(&link.target));

            match link.link_type.as_str() {
                "satisfies" => {
                    body.push_str(&format!("satisfy {name} by {target_name};\n"));
                }
                "verifies" => {
                    body.push_str(&format!("verify {name} by {target_name};\n"));
                }
                "refines" => {
                    body.push_str(&format!("refine {name} by {target_name};\n"));
                }
                "allocated-to" => {
                    body.push_str(&format!("allocate {name} to {target_name};\n"));
                }
                "derives-from" => {
                    body.push_str(&format!("derive {name} from {target_name};\n"));
                }
                _ => {} // Skip unknown link types
            }
        }
    }

    // Assemble output: if using packages, wrap each prefix group.
    if use_packages {
        let mut sorted_prefixes: Vec<&String> = package_bodies.keys().collect();
        sorted_prefixes.sort();
        for prefix in sorted_prefixes {
            let body = &package_bodies[prefix];
            if body.is_empty() {
                continue;
            }
            let pkg_name = sanitize_sysml_name(prefix);
            out.push_str(&format!("package {pkg_name} {{\n"));
            // Indent each line of body.
            for line in body.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&format!("    {line}\n"));
                }
            }
            out.push_str("}\n\n");
        }
    } else {
        // Single prefix or no prefix — emit without package wrapping.
        let mut sorted_prefixes: Vec<&String> = package_bodies.keys().collect();
        sorted_prefixes.sort();
        for prefix in sorted_prefixes {
            out.push_str(&package_bodies[prefix]);
        }
    }

    out
}

/// Extract the prefix from an artifact ID (everything before the last '-' and digits).
///
/// Examples:
/// - `"REQ-001"` -> `"REQ"`
/// - `"SYS-CTRL-002"` -> `"SYS-CTRL"`
/// - `"FEAT-042"` -> `"FEAT"`
/// - `"NoPrefix"` -> `"NoPrefix"`
fn id_prefix(id: &str) -> String {
    // Find the last '-' followed by only digits.
    if let Some(pos) = id.rfind('-') {
        let suffix = &id[pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return id[..pos].to_string();
        }
    }
    id.to_string()
}

/// Collect distinct ID prefixes from all artifacts.
fn collect_id_prefixes(artifacts: &[RivetArtifact]) -> Vec<String> {
    let mut prefixes: Vec<String> = artifacts
        .iter()
        .map(|a| id_prefix(&a.id))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    prefixes.sort();
    prefixes
}

/// Sanitize a string to a valid SysML v2 identifier.
fn sanitize_sysml_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            result.push(c);
        } else if c == ' ' || c == '-' {
            result.push('_');
        }
        // Skip other characters
    }
    if result.is_empty() {
        return "Unnamed".to_string();
    }
    // SysML identifiers cannot start with a digit; prefix with underscore.
    if result.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        result.insert(0, '_');
    }
    result
}

/// Escape a string for use in SysML v2 string literals.
fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_rivet_yaml() {
        let yaml = r#"artifacts:
  - id: REQ-001
    type: requirement
    title: "Safety requirement"
    description: The system shall be safe
    tags: [sysml2, extracted]
"#;
        let artifacts = parse_rivet_yaml(yaml);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id, "REQ-001");
        assert_eq!(artifacts[0].title, "Safety requirement");
        assert_eq!(artifacts[0].artifact_type, "requirement");
    }

    #[test]
    fn parse_rivet_yaml_with_links() {
        let yaml = r#"artifacts:
  - id: REQ-001
    type: requirement
    title: "SafetyReq"
    description: Safety requirement
    links:
      - type: satisfies
        target: IMPL-001
      - type: verifies
        target: TEST-001
"#;
        let artifacts = parse_rivet_yaml(yaml);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].links.len(), 2);
        assert_eq!(artifacts[0].links[0].link_type, "satisfies");
        assert_eq!(artifacts[0].links[0].target, "IMPL-001");
        assert_eq!(artifacts[0].links[1].link_type, "verifies");
        assert_eq!(artifacts[0].links[1].target, "TEST-001");
    }

    #[test]
    fn parse_multiple_artifacts() {
        let yaml = r#"artifacts:
  - id: REQ-001
    type: requirement
    title: "First"
    description: first req
  - id: REQ-002
    type: requirement
    title: "Second"
    description: second req
"#;
        let artifacts = parse_rivet_yaml(yaml);
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].id, "REQ-001");
        assert_eq!(artifacts[1].id, "REQ-002");
    }

    #[test]
    fn generate_simple_requirement() {
        let artifacts = vec![RivetArtifact {
            id: "REQ-001".into(),
            artifact_type: "requirement".into(),
            title: "SafetyReq".into(),
            description: "The system shall be safe".into(),
            tags: vec!["sysml2".into()],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(
            sysml.contains("requirement def SafetyReq"),
            "sysml: {sysml}"
        );
        assert!(sysml.contains("The system shall be safe"), "sysml: {sysml}");
    }

    #[test]
    fn generate_requirement_with_satisfy() {
        let artifacts = vec![
            RivetArtifact {
                id: "REQ-001".into(),
                artifact_type: "requirement".into(),
                title: "SafetyReq".into(),
                description: String::new(),
                tags: vec![],
                links: vec![RivetLink {
                    link_type: "satisfies".into(),
                    target: "IMPL-001".into(),
                }],
            },
            RivetArtifact {
                id: "IMPL-001".into(),
                artifact_type: "design-decision".into(),
                title: "SafetyController".into(),
                description: String::new(),
                tags: vec!["sysml-part".into()],
                links: vec![],
            },
        ];
        let sysml = generate_sysml2(&artifacts);
        assert!(
            sysml.contains("satisfy SafetyReq by SafetyController;"),
            "sysml: {sysml}"
        );
    }

    #[test]
    fn generate_all_relationship_types() {
        let artifacts = vec![RivetArtifact {
            id: "REQ-001".into(),
            artifact_type: "requirement".into(),
            title: "MainReq".into(),
            description: String::new(),
            tags: vec![],
            links: vec![
                RivetLink {
                    link_type: "satisfies".into(),
                    target: "controller".into(),
                },
                RivetLink {
                    link_type: "verifies".into(),
                    target: "test".into(),
                },
                RivetLink {
                    link_type: "refines".into(),
                    target: "detailed".into(),
                },
                RivetLink {
                    link_type: "allocated-to".into(),
                    target: "ecu".into(),
                },
                RivetLink {
                    link_type: "derives-from".into(),
                    target: "parent".into(),
                },
            ],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("satisfy MainReq by controller;"), "{sysml}");
        assert!(sysml.contains("verify MainReq by test;"), "{sysml}");
        assert!(sysml.contains("refine MainReq by detailed;"), "{sysml}");
        assert!(sysml.contains("allocate MainReq to ecu;"), "{sysml}");
        assert!(sysml.contains("derive MainReq from parent;"), "{sysml}");
    }

    #[test]
    fn generate_part_def_from_component() {
        let artifacts = vec![RivetArtifact {
            id: "PART-001".into(),
            artifact_type: "design-decision".into(),
            title: "Vehicle".into(),
            description: String::new(),
            tags: vec!["sysml-part".into()],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("part def Vehicle"), "sysml: {sysml}");
    }

    #[test]
    fn generate_action_def() {
        let artifacts = vec![RivetArtifact {
            id: "ACTION-001".into(),
            artifact_type: "design-decision".into(),
            title: "ProcessSensor".into(),
            description: String::new(),
            tags: vec!["sysml-action".into()],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("action def ProcessSensor"), "sysml: {sysml}");
    }

    #[test]
    fn sanitize_name_spaces() {
        assert_eq!(
            sanitize_sysml_name("Safety Requirement"),
            "Safety_Requirement"
        );
    }

    #[test]
    fn sanitize_name_hyphens() {
        assert_eq!(sanitize_sysml_name("high-level-req"), "high_level_req");
    }

    #[test]
    fn sanitize_name_empty() {
        assert_eq!(sanitize_sysml_name(""), "Unnamed");
    }

    #[test]
    fn roundtrip_extract_then_generate() {
        // Extract from SysML v2, then generate back
        let source = r#"
requirement def LatencyReq {
    doc "System latency shall be under 100ms"
}
satisfy LatencyReq by controller;
"#;
        let parse = crate::parse(source);
        let yaml = crate::extract::extract_requirements(&parse);

        // Parse the YAML back
        let artifacts = parse_rivet_yaml(&yaml);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].title, "LatencyReq");

        // Generate SysML v2
        let sysml = generate_sysml2(&artifacts);
        assert!(
            sysml.contains("requirement def LatencyReq"),
            "sysml: {sysml}"
        );
        assert!(
            sysml.contains("satisfy LatencyReq by controller;"),
            "sysml: {sysml}"
        );
    }

    // ── sanitize_sysml_name leading-digit tests ──────────────────

    #[test]
    fn sanitize_name_leading_digit() {
        assert_eq!(sanitize_sysml_name("123abc"), "_123abc");
    }

    #[test]
    fn sanitize_name_leading_digit_with_spaces() {
        assert_eq!(sanitize_sysml_name("1st Requirement"), "_1st_Requirement");
    }

    #[test]
    fn sanitize_name_no_leading_digit() {
        // No underscore prefix when the first char is already valid.
        assert_eq!(sanitize_sysml_name("abc123"), "abc123");
    }

    // ── New tests: port def from sysml-interface ───────────────────

    #[test]
    fn generate_port_def_from_interface() {
        let artifacts = vec![RivetArtifact {
            id: "IF-001".into(),
            artifact_type: "sysml-interface".into(),
            title: "SensorPort".into(),
            description: "Temperature sensor data port".into(),
            tags: vec![],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("port def SensorPort"), "sysml: {sysml}");
        assert!(
            sysml.contains("Temperature sensor data port"),
            "sysml: {sysml}"
        );
    }

    #[test]
    fn generate_port_def_empty_description() {
        let artifacts = vec![RivetArtifact {
            id: "IF-002".into(),
            artifact_type: "sysml-interface".into(),
            title: "CommandPort".into(),
            description: String::new(),
            tags: vec![],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("port def CommandPort { }"), "sysml: {sysml}");
    }

    // ── New tests: part def with port comments ─────────────────────

    #[test]
    fn generate_part_def_with_port_comments() {
        let artifacts = vec![
            RivetArtifact {
                id: "PART-001".into(),
                artifact_type: "design-decision".into(),
                title: "Controller".into(),
                description: String::new(),
                tags: vec!["sysml-part".into()],
                links: vec![],
            },
            RivetArtifact {
                id: "IF-001".into(),
                artifact_type: "sysml-interface".into(),
                title: "SensorInput".into(),
                description: String::new(),
                tags: vec![],
                links: vec![RivetLink {
                    link_type: "allocated-to".into(),
                    target: "PART-001".into(),
                }],
            },
            RivetArtifact {
                id: "IF-002".into(),
                artifact_type: "sysml-interface".into(),
                title: "CommandOutput".into(),
                description: String::new(),
                tags: vec![],
                links: vec![RivetLink {
                    link_type: "allocated-to".into(),
                    target: "PART-001".into(),
                }],
            },
        ];
        let sysml = generate_sysml2(&artifacts);
        assert!(sysml.contains("part def Controller"), "sysml: {sysml}");
        assert!(sysml.contains("// port: SensorInput"), "sysml: {sysml}");
        assert!(sysml.contains("// port: CommandOutput"), "sysml: {sysml}");
    }

    #[test]
    fn generate_part_def_no_ports_no_comments() {
        // Part with no interfaces linking to it should have empty body.
        let artifacts = vec![RivetArtifact {
            id: "PART-001".into(),
            artifact_type: "design-decision".into(),
            title: "StandalonePart".into(),
            description: String::new(),
            tags: vec!["sysml-part".into()],
            links: vec![],
        }];
        let sysml = generate_sysml2(&artifacts);
        assert!(
            sysml.contains("part def StandalonePart { }"),
            "sysml: {sysml}"
        );
        assert!(
            !sysml.contains("// port:"),
            "no port comments expected: {sysml}"
        );
    }

    // ── New tests: package wrapping ────────────────────────────────

    #[test]
    fn generate_package_wrapping_multiple_prefixes() {
        let artifacts = vec![
            RivetArtifact {
                id: "REQ-001".into(),
                artifact_type: "requirement".into(),
                title: "SafetyReq".into(),
                description: String::new(),
                tags: vec![],
                links: vec![],
            },
            RivetArtifact {
                id: "PART-001".into(),
                artifact_type: "design-decision".into(),
                title: "Vehicle".into(),
                description: String::new(),
                tags: vec!["sysml-part".into()],
                links: vec![],
            },
        ];
        let sysml = generate_sysml2(&artifacts);
        assert!(
            sysml.contains("package PART"),
            "expected PART package: {sysml}"
        );
        assert!(
            sysml.contains("package REQ"),
            "expected REQ package: {sysml}"
        );
        // Definitions should be inside package blocks.
        assert!(
            sysml.contains("    requirement def SafetyReq"),
            "requirement should be indented inside package: {sysml}"
        );
        assert!(
            sysml.contains("    part def Vehicle"),
            "part should be indented inside package: {sysml}"
        );
    }

    #[test]
    fn generate_no_package_wrapping_single_prefix() {
        // All artifacts share the same prefix -- no package wrapping.
        let artifacts = vec![
            RivetArtifact {
                id: "REQ-001".into(),
                artifact_type: "requirement".into(),
                title: "FirstReq".into(),
                description: String::new(),
                tags: vec![],
                links: vec![],
            },
            RivetArtifact {
                id: "REQ-002".into(),
                artifact_type: "requirement".into(),
                title: "SecondReq".into(),
                description: String::new(),
                tags: vec![],
                links: vec![],
            },
        ];
        let sysml = generate_sysml2(&artifacts);
        assert!(
            !sysml.contains("package "),
            "single prefix should not generate package wrapping: {sysml}"
        );
        assert!(sysml.contains("requirement def FirstReq"), "sysml: {sysml}");
        assert!(
            sysml.contains("requirement def SecondReq"),
            "sysml: {sysml}"
        );
    }

    // ── New tests: id_prefix extraction ────────────────────────────

    #[test]
    fn id_prefix_simple() {
        assert_eq!(id_prefix("REQ-001"), "REQ");
        assert_eq!(id_prefix("PART-042"), "PART");
    }

    #[test]
    fn id_prefix_compound() {
        assert_eq!(id_prefix("SYS-CTRL-002"), "SYS-CTRL");
        assert_eq!(id_prefix("HW-IO-100"), "HW-IO");
    }
}
