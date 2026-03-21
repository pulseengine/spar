//! Source rewriting for AADL deployment binding properties.
//!
//! Provides text-level editing of AADL source files to insert or update
//! property associations (e.g., `Actual_Processor_Binding`) inside
//! component implementation declarations. Uses the lossless CST from
//! spar-syntax to locate edit positions precisely, then applies edits
//! as string replacements.
//!
//! Safety: SOLVER-REQ-016 — every rewrite is validated by re-parsing
//! the result and checking for zero parse errors.

use spar_parser::SyntaxKind;
use spar_syntax::{SyntaxNode, parse};

/// A description of a single binding property edit to apply.
#[derive(Debug, Clone)]
pub(crate) struct BindingEdit {
    /// Component implementation name (e.g., "T.impl" or "s.i").
    pub component_impl: String,
    /// Property to set (e.g., "Deployment_Properties::Actual_Processor_Binding").
    pub property: String,
    /// Value to assign (e.g., "reference (cpu1)").
    pub value: String,
}

/// An error that occurred during source rewriting.
#[derive(Debug, Clone)]
pub(crate) struct RefactorError {
    pub message: String,
}

impl std::fmt::Display for RefactorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Apply a binding property edit to AADL source text.
///
/// 1. Parses the source to build a CST.
/// 2. Locates the component implementation matching `edit.component_impl`.
/// 3. Inserts or updates the property in the properties section.
/// 4. Re-parses the result to validate (SOLVER-REQ-016).
pub(crate) fn apply_binding_edit(source: &str, edit: &BindingEdit) -> Result<String, RefactorError> {
    let parsed = parse(source);
    let root = parsed.syntax_node();

    // Find the COMPONENT_IMPL node matching edit.component_impl
    let impl_node = find_component_impl(&root, &edit.component_impl).ok_or_else(|| {
        RefactorError {
            message: format!(
                "component implementation '{}' not found in source",
                edit.component_impl
            ),
        }
    })?;

    // Determine the property name to match (just the property name part)
    let prop_name_to_match = property_short_name(&edit.property);

    // Build the full property text
    let property_text = format!("{} => {};\n", edit.property, edit.value);

    // Look for existing PROPERTY_SECTION
    let prop_section = impl_node
        .children()
        .find(|n| n.kind() == SyntaxKind::PROPERTY_SECTION);

    let result = if let Some(ref section) = prop_section {
        // Check if the property already exists
        if let Some(existing) = find_property_association(section, prop_name_to_match) {
            // Replace the existing property association
            replace_property_association(source, &existing, &edit.property, &edit.value)
        } else {
            // Insert at the end of the properties section (before the next section or end)
            insert_into_properties_section(source, section, &property_text)
        }
    } else {
        // No properties section — insert one before `end`
        insert_properties_section(source, &impl_node, &property_text)
    };

    // SOLVER-REQ-016: re-parse to validate
    let reparse = parse(&result);
    if !reparse.ok() {
        return Err(RefactorError {
            message: format!(
                "rewrite produced invalid AADL: {}",
                reparse
                    .errors()
                    .iter()
                    .map(|e| format!("offset {}: {}", e.offset, e.msg))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        });
    }

    Ok(result)
}

/// Find a COMPONENT_IMPL node whose "TypeName.ImplName" matches the target.
///
/// The CST structure is:
/// ```text
/// COMPONENT_IMPL
///   COMPONENT_CATEGORY
///     SYSTEM_KW "system"
///   IMPLEMENTATION_KW "implementation"
///   REALIZATION
///     IDENT "TypeName"
///   DOT "."
///   IDENT "ImplName"
///   ...
/// ```
fn find_component_impl(root: &SyntaxNode, target: &str) -> Option<SyntaxNode> {
    for node in root.descendants() {
        if node.kind() != SyntaxKind::COMPONENT_IMPL {
            continue;
        }
        if let Some(name) = extract_impl_name(&node)
            && name.eq_ignore_ascii_case(target)
        {
            return Some(node);
        }
    }
    None
}

/// Extract the "TypeName.ImplName" string from a COMPONENT_IMPL CST node.
fn extract_impl_name(node: &SyntaxNode) -> Option<String> {
    // Walk the direct children tokens/nodes to find REALIZATION, DOT, IDENT
    let mut realization_name = None;
    let mut impl_name = None;
    let mut saw_dot = false;

    for child in node.children_with_tokens() {
        match child.kind() {
            SyntaxKind::REALIZATION => {
                // The realization node contains the type name IDENT(s)
                let real_node = child.as_node()?;
                let text: Vec<String> = real_node
                    .children_with_tokens()
                    .filter_map(|c| c.into_token())
                    .filter(|t| t.kind() == SyntaxKind::IDENT || t.kind() == SyntaxKind::COLON_COLON)
                    .map(|t| t.text().to_string())
                    .collect();
                realization_name = Some(text.join(""));
            }
            SyntaxKind::DOT if realization_name.is_some() && !saw_dot => {
                saw_dot = true;
            }
            SyntaxKind::IDENT if saw_dot && impl_name.is_none() => {
                impl_name = Some(child.as_token()?.text().to_string());
            }
            _ => {}
        }
    }

    match (realization_name, impl_name) {
        (Some(tn), Some(im)) => Some(format!("{}.{}", tn, im)),
        _ => None,
    }
}

/// Get the short property name (after the last `::`) for matching.
fn property_short_name(full_name: &str) -> &str {
    full_name.rsplit("::").next().unwrap_or(full_name)
}

/// Find a PROPERTY_ASSOCIATION in a PROPERTY_SECTION whose property name matches.
fn find_property_association(section: &SyntaxNode, prop_name: &str) -> Option<SyntaxNode> {
    for child in section.children() {
        if child.kind() != SyntaxKind::PROPERTY_ASSOCIATION {
            continue;
        }
        // The first child should be PROPERTY_REF containing the property name
        if let Some(prop_ref) = child
            .children()
            .find(|n| n.kind() == SyntaxKind::PROPERTY_REF)
        {
            let ref_text = prop_ref.text().to_string();
            let ref_short = property_short_name(ref_text.trim());
            if ref_short.eq_ignore_ascii_case(prop_name) {
                return Some(child);
            }
        }
    }
    None
}

/// Replace an existing PROPERTY_ASSOCIATION node with new property text.
fn replace_property_association(
    source: &str,
    assoc_node: &SyntaxNode,
    property: &str,
    value: &str,
) -> String {
    let start = assoc_node.text_range().start().into();
    let end: usize = assoc_node.text_range().end().into();

    // Detect the indentation of the existing property line
    let indent = detect_indent(source, start);
    let replacement = format!("{}{} => {};", indent, property, value);

    let mut result = String::with_capacity(source.len());
    result.push_str(&source[..start]);
    result.push_str(&replacement);
    // Skip the old association text but preserve trailing whitespace if present
    result.push_str(&source[end..]);
    result
}

/// Insert a new property into an existing PROPERTY_SECTION.
///
/// The property is added after the last PROPERTY_ASSOCIATION in the section,
/// or after the `properties` keyword if the section is empty.
fn insert_into_properties_section(
    source: &str,
    section: &SyntaxNode,
    property_text: &str,
) -> String {
    // Find the last property association in the section, or the properties keyword
    let insert_offset: usize = if let Some(last_assoc) = section
        .children()
        .filter(|n| n.kind() == SyntaxKind::PROPERTY_ASSOCIATION)
        .last()
    {
        last_assoc.text_range().end().into()
    } else {
        // Empty properties section: after the PROPERTIES_KW token
        section
            .children_with_tokens()
            .find(|c| c.kind() == SyntaxKind::PROPERTIES_KW)
            .map(|c| {
                let end: usize = c.text_range().end().into();
                end
            })
            .unwrap_or_else(|| {
                let end: usize = section.text_range().start().into();
                end
            })
    };

    // Detect indentation from existing properties or from the section
    let indent = if let Some(first_assoc) = section
        .children()
        .find(|n| n.kind() == SyntaxKind::PROPERTY_ASSOCIATION)
    {
        let offset: usize = first_assoc.text_range().start().into();
        detect_indent(source, offset)
    } else {
        // Default: use the indentation of the properties keyword + extra tab
        let section_start: usize = section.text_range().start().into();
        let base_indent = detect_indent(source, section_start);
        format!("{}\t", base_indent)
    };

    let mut result = String::with_capacity(source.len() + property_text.len() + 20);
    result.push_str(&source[..insert_offset]);
    result.push('\n');
    result.push_str(&indent);
    result.push_str(property_text.trim_end());
    result.push_str(&source[insert_offset..]);
    result
}

/// Insert a new `properties` section with the given property text
/// before the `end` keyword of a COMPONENT_IMPL.
fn insert_properties_section(
    source: &str,
    impl_node: &SyntaxNode,
    property_text: &str,
) -> String {
    // Find the END_KW token in the implementation
    let end_kw = impl_node
        .children_with_tokens()
        .find(|c| c.kind() == SyntaxKind::END_KW)
        .expect("COMPONENT_IMPL must have an END_KW token");

    let end_offset: usize = end_kw.text_range().start().into();
    let end_indent = detect_indent(source, end_offset);

    // The property indent should be one level deeper than the end keyword
    let prop_indent = format!("{}\t", end_indent);

    let mut result = String::with_capacity(source.len() + property_text.len() + 40);
    result.push_str(&source[..end_offset]);
    result.push_str(&end_indent);
    result.push_str("properties\n");
    result.push_str(&prop_indent);
    result.push_str(property_text.trim_end());
    result.push('\n');
    result.push_str(&source[end_offset..]);
    result
}

/// Detect the indentation (leading whitespace) of the line containing the given byte offset.
fn detect_indent(source: &str, offset: usize) -> String {
    // Walk backwards from offset to the beginning of the line
    let before = &source[..offset];
    let line_start = before.rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let line = &source[line_start..offset];
    // Extract leading whitespace
    let indent_len = line.len() - line.trim_start().len();
    line[..indent_len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AADL_WITH_PROPERTIES: &str = "\
package Pkg
public
  system T
  end T;

  system implementation T.impl
    subcomponents
      t1 : thread;
    properties
      Timing_Properties::Period => 10 ms;
  end T.impl;
end Pkg;
";

    const AADL_WITHOUT_PROPERTIES: &str = "\
package Pkg
public
  system T
  end T;

  system implementation T.impl
    subcomponents
      t1 : thread;
  end T.impl;
end Pkg;
";

    const AADL_WITH_BINDING: &str = "\
package Pkg
public
  system T
  end T;

  system implementation T.impl
    subcomponents
      t1 : thread;
      cpu1 : processor;
    properties
      Deployment_Properties::Actual_Processor_Binding => reference (cpu1);
  end T.impl;
end Pkg;
";

    #[test]
    fn insert_binding_into_existing_properties() {
        let edit = BindingEdit {
            component_impl: "T.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu1)".to_string(),
        };
        let result = apply_binding_edit(AADL_WITH_PROPERTIES, &edit).unwrap();
        assert!(
            result.contains("Deployment_Properties::Actual_Processor_Binding => reference (cpu1);"),
            "Should contain the new binding property. Got:\n{}",
            result
        );
        // Original property should still be there
        assert!(
            result.contains("Timing_Properties::Period => 10 ms;"),
            "Should preserve existing properties. Got:\n{}",
            result
        );
    }

    #[test]
    fn update_existing_binding() {
        let edit = BindingEdit {
            component_impl: "T.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu2)".to_string(),
        };
        let result = apply_binding_edit(AADL_WITH_BINDING, &edit).unwrap();
        assert!(
            result.contains("Actual_Processor_Binding => reference (cpu2);"),
            "Should contain updated binding. Got:\n{}",
            result
        );
        assert!(
            !result.contains("reference (cpu1)"),
            "Should NOT contain old binding value. Got:\n{}",
            result
        );
    }

    #[test]
    fn insert_when_no_properties_section() {
        let edit = BindingEdit {
            component_impl: "T.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu1)".to_string(),
        };
        let result = apply_binding_edit(AADL_WITHOUT_PROPERTIES, &edit).unwrap();
        assert!(
            result.contains("properties"),
            "Should insert a properties section. Got:\n{}",
            result
        );
        assert!(
            result.contains("Deployment_Properties::Actual_Processor_Binding => reference (cpu1);"),
            "Should contain the new binding. Got:\n{}",
            result
        );
    }

    #[test]
    fn rewrite_preserves_other_content() {
        let edit = BindingEdit {
            component_impl: "T.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu1)".to_string(),
        };
        let result = apply_binding_edit(AADL_WITH_PROPERTIES, &edit).unwrap();

        // Package declaration preserved
        assert!(result.contains("package Pkg"), "Package declaration lost");
        // Component type preserved
        assert!(result.contains("system T"), "Component type lost");
        assert!(result.contains("end T;"), "Component type end lost");
        // Subcomponents preserved
        assert!(
            result.contains("t1 : thread;"),
            "Subcomponent declaration lost"
        );
        // End impl preserved
        assert!(result.contains("end T.impl;"), "Impl end lost");
        // End package preserved
        assert!(result.contains("end Pkg;"), "Package end lost");
    }

    #[test]
    fn rewrite_produces_valid_parse() {
        // SOLVER-REQ-016: re-parse succeeds after rewrite
        let edit = BindingEdit {
            component_impl: "T.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu1)".to_string(),
        };

        // Test all three paths: insert into existing, create new section, update existing
        for source in [
            AADL_WITH_PROPERTIES,
            AADL_WITHOUT_PROPERTIES,
            AADL_WITH_BINDING,
        ] {
            let result = apply_binding_edit(source, &edit).unwrap();
            let reparse = parse(&result);
            assert!(
                reparse.ok(),
                "Rewrite should produce valid AADL.\nSource:\n{}\nResult:\n{}\nErrors: {:?}",
                source,
                result,
                reparse.errors()
            );
        }
    }

    #[test]
    fn error_on_missing_component_impl() {
        let edit = BindingEdit {
            component_impl: "NonExistent.impl".to_string(),
            property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
            value: "reference (cpu1)".to_string(),
        };
        let result = apply_binding_edit(AADL_WITH_PROPERTIES, &edit);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("not found"));
    }
}
