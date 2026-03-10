//! Lowering from CST to ItemTree.
//!
//! Walks the syntax tree produced by spar-syntax and extracts
//! a condensed item tree with only the declaration shapes.

use spar_syntax::ast::{self, AstNode};
use spar_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::item_tree::*;
use crate::name::{ClassifierRef, Name, PropertyRef};

/// Lower a parsed source file into an item tree.
pub fn lower_file(root: &SyntaxNode) -> ItemTree {
    let mut tree = ItemTree::default();
    let mut diagnostics = Vec::new();
    let file = match ast::SourceFile::cast(root.clone()) {
        Some(f) => f,
        None => return tree,
    };

    for pkg in file.packages() {
        lower_package(&pkg, &mut tree, &mut diagnostics);
    }

    for ps in file.property_sets() {
        lower_property_set(&ps, &mut tree);
    }

    tree.diagnostics = diagnostics;
    tree
}

fn lower_package(
    pkg: &ast::AadlPackage,
    tree: &mut ItemTree,
    diagnostics: &mut Vec<LoweringDiagnostic>,
) {
    // Package name is inside a NAME node child
    let name = match extract_name_text(pkg.syntax()) {
        Some(n) => n,
        None => return,
    };

    let mut with_clauses = Vec::new();
    let mut public_items = Vec::new();
    let mut private_items = Vec::new();
    let mut renames = Vec::new();

    // Walk children to find sections and with clauses
    for child in pkg.syntax().children() {
        match child.kind() {
            SyntaxKind::WITH_CLAUSE => {
                collect_with_names(&child, &mut with_clauses);
            }
            SyntaxKind::PUBLIC_SECTION => {
                lower_section_with_visibility(
                    &child,
                    tree,
                    &mut public_items,
                    &mut with_clauses,
                    &mut renames,
                    true,
                    diagnostics,
                );
            }
            SyntaxKind::PRIVATE_SECTION => {
                lower_section_with_visibility(
                    &child,
                    tree,
                    &mut private_items,
                    &mut with_clauses,
                    &mut renames,
                    false,
                    diagnostics,
                );
            }
            _ => {}
        }
    }

    tree.packages.alloc(Package {
        name,
        with_clauses,
        public_items,
        private_items,
        renames,
    });
}

fn collect_with_names(with_node: &SyntaxNode, names: &mut Vec<Name>) {
    // Collect all NAME nodes in the with clause.
    // NAME nodes contain the IDENT (possibly dotted for qualified names).
    // We use the full text (trimmed) so dotted names like "Pkg.Sub" are preserved.
    for child in with_node.children() {
        if child.kind() == SyntaxKind::NAME {
            // For simple names: NAME → IDENT "DataTypes"
            // For dotted names: NAME → IDENT.IDENT "Pkg.Sub"
            let text = child.text().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                names.push(Name::new(trimmed));
            }
        }
    }
}

fn lower_section_with_visibility(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    items: &mut Vec<ItemRef>,
    with_clauses: &mut Vec<Name>,
    renames_out: &mut Vec<RenamesIdx>,
    is_public: bool,
    diagnostics: &mut Vec<LoweringDiagnostic>,
) {
    for child in section.children() {
        match child.kind() {
            SyntaxKind::WITH_CLAUSE => {
                collect_with_names(&child, with_clauses);
            }
            SyntaxKind::COMPONENT_TYPE => {
                if let Some(item_ref) =
                    lower_component_type_with_visibility(&child, tree, is_public)
                {
                    items.push(item_ref);
                }
            }
            SyntaxKind::COMPONENT_IMPL => {
                if let Some(item_ref) =
                    lower_component_impl_with_visibility(&child, tree, is_public)
                {
                    items.push(item_ref);
                }
            }
            SyntaxKind::FEATURE_GROUP_TYPE => {
                if let Some(item_ref) =
                    lower_feature_group_type_with_visibility(&child, tree, is_public)
                {
                    items.push(item_ref);
                }
            }
            SyntaxKind::PROPERTY_SET => {
                if let Some(item_ref) = lower_property_set_in_section(&child, tree) {
                    items.push(item_ref);
                }
            }
            SyntaxKind::ANNEX_LIBRARY => {
                items.push(ItemRef::AnnexLibrary);
                // STPA-REQ-002: warn that annex content was not processed
                diagnostics.push(LoweringDiagnostic {
                    message: "annex library content not processed (no registered annex parser)"
                        .to_string(),
                    severity: LoweringSeverity::Warning,
                });
            }
            SyntaxKind::RENAMES_CLAUSE => {
                if let Some(idx) = lower_renames_clause(&child, tree) {
                    renames_out.push(idx);
                }
            }
            // STPA-REQ-004: Non-semantic kinds intentionally ignored
            SyntaxKind::ERROR
            | SyntaxKind::WHITESPACE
            | SyntaxKind::COMMENT
            | SyntaxKind::NAME
            | SyntaxKind::END_KW
            | SyntaxKind::SEMICOLON
            | SyntaxKind::PACKAGE_PROPERTIES
            | SyntaxKind::ANNEX_SUBCLAUSE => {}
            // STPA-REQ-004: Warn on any unhandled semantic construct
            other => {
                diagnostics.push(LoweringDiagnostic {
                    message: format!("unhandled syntax construct in package section: {:?}", other),
                    severity: LoweringSeverity::Warning,
                });
            }
        }
    }
}

fn lower_component_type_with_visibility(
    node: &SyntaxNode,
    tree: &mut ItemTree,
    is_public: bool,
) -> Option<ItemRef> {
    let category = extract_category(node)?;
    let name = extract_decl_name(node)?;

    let extends = node
        .children()
        .find(|c| c.kind() == SyntaxKind::TYPE_EXTENSION)
        .and_then(|ext| extract_classifier_ref_from_child(&ext));

    let mut features = Vec::new();
    if let Some(feat_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::FEATURE_SECTION)
    {
        lower_features(&feat_section, tree, &mut features);
    }

    let mut flow_specs = Vec::new();
    if let Some(flow_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::FLOW_SPEC_SECTION)
    {
        lower_flow_specs(&flow_section, tree, &mut flow_specs);
    }

    let mut modes = Vec::new();
    let mut mode_transitions = Vec::new();
    if let Some(mode_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MODE_SECTION)
    {
        lower_modes(&mode_section, tree, &mut modes, &mut mode_transitions);
    }

    let mut prototypes = Vec::new();
    if let Some(proto_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROTOTYPE_SECTION)
    {
        lower_prototypes(&proto_section, tree, &mut prototypes);
    }

    let mut property_associations = Vec::new();
    if let Some(prop_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
    {
        lower_property_associations(&prop_section, tree, &mut property_associations);
    }

    let idx = tree.component_types.alloc(ComponentTypeItem {
        name,
        category,
        is_public,
        extends,
        features,
        flow_specs,
        modes,
        mode_transitions,
        prototypes,
        property_associations,
    });
    Some(ItemRef::ComponentType(idx))
}

fn lower_component_impl_with_visibility(
    node: &SyntaxNode,
    tree: &mut ItemTree,
    is_public: bool,
) -> Option<ItemRef> {
    let category = extract_category(node)?;

    // Extract type_name from REALIZATION child
    let realization = node
        .children()
        .find(|c| c.kind() == SyntaxKind::REALIZATION)?;
    let type_name = Name::new(&realization.text().to_string());

    // Extract impl_name: the IDENT token after the DOT
    let impl_name = extract_impl_name(node)?;

    let extends = node
        .children()
        .find(|c| c.kind() == SyntaxKind::IMPL_EXTENSION)
        .and_then(|ext| extract_classifier_ref_from_child(&ext));

    let mut prototypes = Vec::new();
    if let Some(proto_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROTOTYPE_SECTION)
    {
        lower_prototypes(&proto_section, tree, &mut prototypes);
    }

    let mut subcomponents = Vec::new();
    if let Some(sub_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::SUBCOMPONENT_SECTION)
    {
        lower_subcomponents(&sub_section, tree, &mut subcomponents);
    }

    let mut connections = Vec::new();
    if let Some(conn_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::CONNECTION_SECTION)
    {
        lower_connections(&conn_section, tree, &mut connections);
    }

    let mut end_to_end_flows = Vec::new();
    let mut flow_impls = Vec::new();
    if let Some(flow_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::FLOW_IMPL_SECTION)
    {
        lower_flow_impl_section(&flow_section, tree, &mut end_to_end_flows, &mut flow_impls);
    }

    let mut call_sequences = Vec::new();
    if let Some(call_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::CALL_SECTION)
    {
        lower_call_sequences(&call_section, tree, &mut call_sequences);
    }

    let mut modes = Vec::new();
    let mut mode_transitions = Vec::new();
    if let Some(mode_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MODE_SECTION)
    {
        lower_modes(&mode_section, tree, &mut modes, &mut mode_transitions);
    }

    let mut property_associations = Vec::new();
    if let Some(prop_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
    {
        lower_property_associations(&prop_section, tree, &mut property_associations);
    }

    let idx = tree.component_impls.alloc(ComponentImplItem {
        type_name,
        impl_name,
        category,
        is_public,
        extends,
        subcomponents,
        connections,
        end_to_end_flows,
        flow_impls,
        modes,
        mode_transitions,
        prototypes,
        call_sequences,
        property_associations,
    });
    Some(ItemRef::ComponentImpl(idx))
}

fn lower_feature_group_type_with_visibility(
    node: &SyntaxNode,
    tree: &mut ItemTree,
    is_public: bool,
) -> Option<ItemRef> {
    let name = extract_decl_name(node)?;

    let extends = node
        .children()
        .find(|c| c.kind() == SyntaxKind::TYPE_EXTENSION)
        .and_then(|ext| extract_classifier_ref_from_child(&ext));

    // Look for `inverse of ClassifierRef`
    let inverse_of = extract_inverse_of(node);

    let mut features = Vec::new();
    if let Some(feat_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::FEATURE_SECTION)
    {
        lower_features(&feat_section, tree, &mut features);
    }

    let mut prototypes = Vec::new();
    if let Some(proto_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROTOTYPE_SECTION)
    {
        lower_prototypes(&proto_section, tree, &mut prototypes);
    }

    let idx = tree.feature_group_types.alloc(FeatureGroupTypeItem {
        name,
        is_public,
        extends,
        inverse_of,
        features,
        prototypes,
    });
    Some(ItemRef::FeatureGroupType(idx))
}

// ── Renames lowering ────────────────────────────────────────────────

fn lower_renames_clause(node: &SyntaxNode, tree: &mut ItemTree) -> Option<RenamesIdx> {
    // A RENAMES_CLAUSE can be:
    //   Named:   IDENT RENAMES_KW [PACKAGE_KW NAME | category CLASSIFIER_REF | FEATURE_KW GROUP_KW CLASSIFIER_REF]
    //   Unnamed: RENAMES_KW [PACKAGE_KW NAME | category CLASSIFIER_REF | ...]
    //
    // We look for an alias (first IDENT before RENAMES_KW) and the target name.

    let mut alias: Option<String> = None;
    let mut kind = RenamesKind::Classifier;
    let mut has_package_kw = false;
    let mut has_feature_group = false;
    let mut past_renames = false;

    // Determine if this is a named renames (first token is IDENT before RENAMES_KW)
    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::IDENT if !past_renames && alias.is_none() => {
                    alias = Some(tok.text().to_string());
                }
                SyntaxKind::RENAMES_KW => {
                    past_renames = true;
                }
                SyntaxKind::PACKAGE_KW if past_renames => {
                    has_package_kw = true;
                    kind = RenamesKind::Package;
                }
                SyntaxKind::FEATURE_KW if past_renames => {
                    has_feature_group = true;
                }
                SyntaxKind::GROUP_KW if past_renames && has_feature_group => {
                    kind = RenamesKind::FeatureGroup;
                }
                _ => {}
            }
        }
    }

    // We need an alias name for named renames; skip unnamed renames for now
    let alias_name = alias?;

    // Extract the target name from NAME or CLASSIFIER_REF children after renames kw
    let original_name = if has_package_kw {
        // `alias renames package PkgName;` — look for NAME child
        node.children()
            .find(|c| c.kind() == SyntaxKind::NAME)
            .map(|n| n.text().to_string().trim().to_string())
    } else {
        // `alias renames <category> Pkg::Classifier;` or
        // `alias renames feature group Pkg::FGT;`
        // Look for CLASSIFIER_REF child or collect remaining idents
        node.children()
            .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
            .map(|cr| cr.text().to_string().trim().to_string())
            .or_else(|| {
                // Fallback: collect text after the keyword(s) before semicolon
                let mut collecting = false;
                let mut parts = Vec::new();
                for elem in node.children_with_tokens() {
                    if let Some(tok) = elem.as_token() {
                        if tok.kind() == SyntaxKind::RENAMES_KW {
                            collecting = true;
                            continue;
                        }
                        if tok.kind() == SyntaxKind::SEMICOLON {
                            break;
                        }
                        if collecting && tok.kind() == SyntaxKind::IDENT {
                            parts.push(tok.text().to_string());
                        }
                    }
                }
                if parts.len() > 1 {
                    // Skip alias and category tokens, take the last part as target
                    Some(parts.last().unwrap().clone())
                } else {
                    None
                }
            })
    };

    let original = original_name?;
    if original.is_empty() {
        return None;
    }

    Some(tree.renames.alloc(RenamesItem {
        alias: Name::new(&alias_name),
        original: Name::new(&original),
        kind,
    }))
}

fn lower_property_set(ps: &ast::PropertySet, tree: &mut ItemTree) {
    let name = match extract_name_text(ps.syntax()) {
        Some(n) => n,
        None => return,
    };

    let (property_defs, property_type_defs, property_constants) =
        extract_property_set_items(ps.syntax());

    tree.property_sets.alloc(PropertySetItem {
        name,
        property_defs,
        property_type_defs,
        property_constants,
    });
}

fn lower_property_set_in_section(node: &SyntaxNode, tree: &mut ItemTree) -> Option<ItemRef> {
    let name = extract_name_text(node)?;

    let (property_defs, property_type_defs, property_constants) = extract_property_set_items(node);

    let idx = tree.property_sets.alloc(PropertySetItem {
        name,
        property_defs,
        property_type_defs,
        property_constants,
    });
    Some(ItemRef::PropertySet(idx))
}

fn extract_property_set_items(
    node: &SyntaxNode,
) -> (
    Vec<PropertyDefItem>,
    Vec<PropertyTypeDefItem>,
    Vec<PropertyConstantItem>,
) {
    let mut defs = Vec::new();
    let mut type_defs = Vec::new();
    let mut constants = Vec::new();

    for child in node.children() {
        match child.kind() {
            SyntaxKind::PROPERTY_DEFINITION => {
                if let Some(tok) = first_ident_token(&child) {
                    let type_def = child
                        .children()
                        .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                        .and_then(|pt| lower_property_type_def(&pt));

                    let default_value = extract_property_def_default(&child);
                    let applies_to = extract_applies_to_list(&child);

                    defs.push(PropertyDefItem {
                        name: Name::new(tok.text()),
                        type_def,
                        default_value,
                        applies_to,
                    });
                }
            }
            SyntaxKind::PROPERTY_TYPE_DECL => {
                if let Some(tok) = first_ident_token(&child) {
                    let type_def = child
                        .children()
                        .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                        .and_then(|pt| lower_property_type_def(&pt));

                    type_defs.push(PropertyTypeDefItem {
                        name: Name::new(tok.text()),
                        type_def,
                    });
                }
            }
            SyntaxKind::PROPERTY_CONSTANT => {
                if let Some(tok) = first_ident_token(&child) {
                    let type_def = child
                        .children()
                        .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                        .and_then(|pt| lower_property_type_def(&pt));

                    // The constant value is after the FAT_ARROW
                    let value =
                        find_property_value_node(&child).and_then(|vn| lower_property_expr(&vn));

                    constants.push(PropertyConstantItem {
                        name: Name::new(tok.text()),
                        type_def,
                        value,
                    });
                }
            }
            _ => {}
        }
    }

    (defs, type_defs, constants)
}

// ── Property type definition lowering ──────────────────────────────

/// Lower a PROPERTY_TYPE CST node into a `PropertyTypeDef`.
///
/// The PROPERTY_TYPE node wraps the type keywords and their arguments
/// as produced by the parser's `property_type` rule.
fn lower_property_type_def(node: &SyntaxNode) -> Option<PropertyTypeDef> {
    // Collect tokens and children from the PROPERTY_TYPE node
    let mut tokens: Vec<(SyntaxKind, String)> = Vec::new();
    let mut child_nodes: Vec<SyntaxNode> = Vec::new();

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token()
            && !tok.kind().is_trivia()
        {
            tokens.push((tok.kind(), tok.text().to_string()));
        }
        if let Some(n) = elem.as_node() {
            child_nodes.push(n.clone());
        }
    }

    // Find the primary type keyword
    let first_kw = tokens.iter().find(|(k, _)| {
        matches!(
            k,
            SyntaxKind::AADLINTEGER_KW
                | SyntaxKind::AADLREAL_KW
                | SyntaxKind::AADLSTRING_KW
                | SyntaxKind::AADLBOOLEAN_KW
                | SyntaxKind::ENUMERATION_KW
                | SyntaxKind::LIST_KW
                | SyntaxKind::RANGE_KW
                | SyntaxKind::RECORD_KW
                | SyntaxKind::UNITS_KW
                | SyntaxKind::CLASSIFIER_KW
                | SyntaxKind::REFERENCE_KW
        )
    });

    match first_kw.map(|(k, _)| *k) {
        Some(SyntaxKind::AADLINTEGER_KW) => {
            let units = extract_units_ref(&tokens, &child_nodes);
            Some(PropertyTypeDef::AadlInteger { range: None, units })
        }
        Some(SyntaxKind::AADLREAL_KW) => {
            let units = extract_units_ref(&tokens, &child_nodes);
            Some(PropertyTypeDef::AadlReal { range: None, units })
        }
        Some(SyntaxKind::AADLSTRING_KW) => Some(PropertyTypeDef::AadlString),
        Some(SyntaxKind::AADLBOOLEAN_KW) => Some(PropertyTypeDef::AadlBoolean),
        Some(SyntaxKind::ENUMERATION_KW) => {
            let mut variants = Vec::new();
            let mut in_parens = false;
            for (kind, text) in &tokens {
                match kind {
                    SyntaxKind::L_PAREN => in_parens = true,
                    SyntaxKind::R_PAREN => break,
                    SyntaxKind::IDENT if in_parens => {
                        variants.push(Name::new(text));
                    }
                    _ => {}
                }
            }
            Some(PropertyTypeDef::Enumeration(variants))
        }
        Some(SyntaxKind::LIST_KW) => {
            // list of <type> -- inner type is a nested PROPERTY_TYPE child
            let inner = child_nodes
                .iter()
                .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                .and_then(lower_property_type_def)
                .unwrap_or(PropertyTypeDef::AadlString);
            Some(PropertyTypeDef::ListOf(Box::new(inner)))
        }
        Some(SyntaxKind::RANGE_KW) => {
            // range of <type> -- inner type is a nested PROPERTY_TYPE child
            let inner = child_nodes
                .iter()
                .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                .and_then(lower_property_type_def)
                .unwrap_or(PropertyTypeDef::AadlInteger {
                    range: None,
                    units: None,
                });
            Some(PropertyTypeDef::Range(Box::new(inner)))
        }
        Some(SyntaxKind::RECORD_KW) => {
            let mut fields = Vec::new();
            for child in &child_nodes {
                if child.kind() == SyntaxKind::RECORD_FIELD {
                    let field_name = first_ident_token(child).map(|t| Name::new(t.text()));
                    let field_type = child
                        .children()
                        .find(|c| c.kind() == SyntaxKind::PROPERTY_TYPE)
                        .and_then(|pt| lower_property_type_def(&pt))
                        .unwrap_or(PropertyTypeDef::AadlString);
                    if let Some(name) = field_name {
                        fields.push((name, field_type));
                    }
                }
            }
            Some(PropertyTypeDef::RecordType(fields))
        }
        Some(SyntaxKind::UNITS_KW) => {
            let mut units = Vec::new();
            let mut in_parens = false;
            let mut idx = 0;
            while idx < tokens.len() {
                let (kind, text) = &tokens[idx];
                match kind {
                    SyntaxKind::L_PAREN => in_parens = true,
                    SyntaxKind::R_PAREN => break,
                    SyntaxKind::IDENT if in_parens => {
                        let unit_name = Name::new(text);
                        // Check for `=> base * factor`
                        if idx + 1 < tokens.len() && tokens[idx + 1].0 == SyntaxKind::FAT_ARROW {
                            // Skip =>
                            idx += 2;
                            // base unit name
                            let base_name =
                                if idx < tokens.len() && tokens[idx].0 == SyntaxKind::IDENT {
                                    let b = Name::new(&tokens[idx].1);
                                    idx += 1;
                                    b
                                } else {
                                    Name::new("?")
                                };
                            // * factor
                            if idx < tokens.len() && tokens[idx].0 == SyntaxKind::STAR {
                                idx += 1;
                                let factor = if idx < tokens.len()
                                    && (tokens[idx].0 == SyntaxKind::INTEGER_LIT
                                        || tokens[idx].0 == SyntaxKind::REAL_LIT)
                                {
                                    let f = tokens[idx].1.clone();
                                    idx += 1;
                                    f
                                } else {
                                    "1".to_string()
                                };
                                units.push((unit_name, Some((base_name, factor))));
                            } else {
                                units.push((unit_name, Some((base_name, "1".to_string()))));
                            }
                            continue; // skip normal idx increment
                        } else {
                            // Base unit (no conversion factor)
                            units.push((unit_name, None));
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
            Some(PropertyTypeDef::UnitsType(units))
        }
        Some(SyntaxKind::CLASSIFIER_KW) => {
            // classifier [(category)]
            let category = child_nodes
                .iter()
                .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
                .and_then(|cr| {
                    let text = cr.text().to_string().trim().to_string();
                    parse_category(&text)
                });
            Some(PropertyTypeDef::Classifier(category))
        }
        Some(SyntaxKind::REFERENCE_KW) => {
            // reference [(category)]
            let category = child_nodes
                .iter()
                .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
                .and_then(|cr| {
                    let text = cr.text().to_string().trim().to_string();
                    parse_category(&text)
                });
            Some(PropertyTypeDef::Reference(category))
        }
        None => {
            // No type keyword -- could be a type reference (IDENT)
            // The parser wraps `IDENT` in a CLASSIFIER_REF child
            if let Some(cr) = child_nodes
                .iter()
                .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
            {
                let text = cr.text().to_string().trim().to_string();
                if !text.is_empty() {
                    return Some(PropertyTypeDef::TypeRef(Name::new(&text)));
                }
            }
            // Fallback: use the text of any IDENT token
            for (kind, text) in &tokens {
                if *kind == SyntaxKind::IDENT {
                    return Some(PropertyTypeDef::TypeRef(Name::new(text)));
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract `units UnitTypeName` reference from token list.
///
/// Looks for UNITS_KW followed by a CLASSIFIER_REF child node or IDENT token.
fn extract_units_ref(tokens: &[(SyntaxKind, String)], children: &[SyntaxNode]) -> Option<Name> {
    let has_units_kw = tokens.iter().any(|(k, _)| *k == SyntaxKind::UNITS_KW);
    if !has_units_kw {
        return None;
    }
    // First try CLASSIFIER_REF child
    if let Some(cr) = children
        .iter()
        .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
    {
        let text = cr.text().to_string().trim().to_string();
        if !text.is_empty() {
            return Some(Name::new(&text));
        }
    }
    // Fallback: IDENT token after UNITS_KW
    let mut saw_units = false;
    for (kind, text) in tokens {
        if *kind == SyntaxKind::UNITS_KW {
            saw_units = true;
            continue;
        }
        if saw_units && *kind == SyntaxKind::IDENT {
            return Some(Name::new(text));
        }
    }
    None
}

/// Extract the default value expression from a PROPERTY_DEFINITION node.
///
/// The default value appears after FAT_ARROW but before APPLIES_KW.
fn extract_property_def_default(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut past_type = false;
    let mut past_arrow = false;

    for elem in node.children_with_tokens() {
        if let Some(n) = elem.as_node() {
            if n.kind() == SyntaxKind::PROPERTY_TYPE {
                past_type = true;
                continue;
            }
            if past_arrow {
                // Try to lower this node as a property expression
                if let Some(expr) = lower_property_expr(n) {
                    return Some(expr);
                }
            }
        }
        if let Some(tok) = elem.as_token() {
            if tok.kind() == SyntaxKind::FAT_ARROW && past_type {
                past_arrow = true;
                continue;
            }
            if tok.kind() == SyntaxKind::APPLIES_KW {
                break;
            }
        }
    }
    None
}

/// Extract the `applies to (cat1, cat2, ...)` list from a PROPERTY_DEFINITION node.
///
/// Returns a list of `AppliesToKind` values.
fn extract_applies_to_list(node: &SyntaxNode) -> Vec<AppliesToKind> {
    let mut result = Vec::new();
    let mut past_applies_to = false;
    let mut in_parens = false;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::APPLIES_KW => past_applies_to = true,
                SyntaxKind::TO_KW if past_applies_to => {}
                SyntaxKind::L_PAREN if past_applies_to => in_parens = true,
                SyntaxKind::R_PAREN if in_parens => break,
                SyntaxKind::ALL_KW if in_parens => {
                    result.push(AppliesToKind::All);
                }
                SyntaxKind::IDENT if in_parens => {
                    let text = tok.text();
                    result.push(parse_applies_to_name(text));
                }
                _ => {}
            }
        }
        if let Some(n) = elem.as_node()
            && in_parens
            && n.kind() == SyntaxKind::COMPONENT_CATEGORY
        {
            let text = n.text().to_string();
            let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(cat) = parse_category(&normalized) {
                result.push(AppliesToKind::Category(cat));
            } else {
                result.push(AppliesToKind::Named(Name::new(&normalized)));
            }
        }
    }

    result
}

/// Parse a single `applies to` entry from its text form.
fn parse_applies_to_name(text: &str) -> AppliesToKind {
    match text.to_lowercase().as_str() {
        "all" => AppliesToKind::All,
        "connection" | "connections" => AppliesToKind::Connection,
        "flow" | "flows" => AppliesToKind::Flow,
        "mode" | "modes" => AppliesToKind::Mode,
        "port" | "ports" => AppliesToKind::Port,
        "access" => AppliesToKind::Access,
        _ => AppliesToKind::Named(Name::new(text)),
    }
}

// ── Feature lowering ───────────────────────────────────────────────

fn lower_features(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<FeatureIdx>) {
    for child in section.children() {
        let (kind, node) = match child.kind() {
            SyntaxKind::DATA_PORT => (FeatureKind::DataPort, child),
            SyntaxKind::EVENT_PORT => (FeatureKind::EventPort, child),
            SyntaxKind::EVENT_DATA_PORT => (FeatureKind::EventDataPort, child),
            SyntaxKind::PARAMETER => (FeatureKind::Parameter, child),
            SyntaxKind::DATA_ACCESS => (FeatureKind::DataAccess, child),
            SyntaxKind::BUS_ACCESS => (FeatureKind::BusAccess, child),
            SyntaxKind::SUBPROGRAM_ACCESS => (FeatureKind::SubprogramAccess, child),
            SyntaxKind::SUBPROGRAM_GROUP_ACCESS => (FeatureKind::SubprogramGroupAccess, child),
            SyntaxKind::FEATURE_GROUP => (FeatureKind::FeatureGroup, child),
            SyntaxKind::ABSTRACT_FEATURE => (FeatureKind::AbstractFeature, child),
            _ => continue,
        };

        let name = match first_ident_token(&node) {
            Some(tok) => Name::new(tok.text()),
            None => continue,
        };

        let direction = extract_direction(&node);

        let access_kind = extract_access_kind(&node);

        let is_refined = node.children().any(|c| c.kind() == SyntaxKind::REFINED_TO);

        let classifier = node
            .children()
            .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
            .and_then(|cr| parse_classifier_ref_node(&cr));

        let array_dimensions = extract_array_dimensions(&node);

        let mut property_associations = Vec::new();
        if let Some(prop_section) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
        {
            lower_property_associations(&prop_section, tree, &mut property_associations);
        }

        let idx = tree.features.alloc(Feature {
            name,
            kind,
            direction,
            access_kind,
            classifier,
            is_refined,
            array_dimensions,
            property_associations,
        });
        out.push(idx);
    }
}

// ── Subcomponent lowering ──────────────────────────────────────────

fn lower_subcomponents(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<SubcomponentIdx>) {
    for child in section.children() {
        if child.kind() != SyntaxKind::SUBCOMPONENT {
            continue;
        }

        let name = match first_ident_token(&child) {
            Some(tok) => Name::new(tok.text()),
            None => continue,
        };

        let category = match extract_category(&child) {
            Some(c) => c,
            None => continue,
        };

        let is_refined = child.children().any(|c| c.kind() == SyntaxKind::REFINED_TO);

        let classifier = child
            .children()
            .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
            .and_then(|cr| parse_classifier_ref_node(&cr));

        let array_dimensions = extract_array_dimensions(&child);
        let in_modes = extract_in_modes(&child);

        let mut property_associations = Vec::new();
        if let Some(prop_section) = child
            .children()
            .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
        {
            lower_property_associations(&prop_section, tree, &mut property_associations);
        }

        let idx = tree.subcomponents.alloc(SubcomponentItem {
            name,
            category,
            classifier,
            is_refined,
            array_dimensions,
            in_modes,
            property_associations,
        });
        out.push(idx);
    }
}

// ── Connection lowering ────────────────────────────────────────────

fn lower_connections(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<ConnectionIdx>) {
    for child in section.children() {
        let kind = match child.kind() {
            SyntaxKind::PORT_CONNECTION => ConnectionKind::Port,
            SyntaxKind::ACCESS_CONNECTION => ConnectionKind::Access,
            SyntaxKind::FEATURE_GROUP_CONNECTION => ConnectionKind::FeatureGroup,
            SyntaxKind::FEATURE_CONNECTION => ConnectionKind::Feature,
            SyntaxKind::PARAMETER_CONNECTION => ConnectionKind::Parameter,
            _ => continue,
        };

        let name = match first_ident_token(&child) {
            Some(tok) => Name::new(tok.text()),
            None => continue,
        };

        let is_bidirectional = child
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .any(|tok| tok.kind() == SyntaxKind::BIDI_ARROW);

        let is_refined = child.children().any(|c| c.kind() == SyntaxKind::REFINED_TO);

        // Extract source and destination CONNECTED_ELEMENT nodes
        let mut connected_elements = child
            .children()
            .filter(|c| c.kind() == SyntaxKind::CONNECTED_ELEMENT);

        let src = connected_elements
            .next()
            .and_then(|ce| parse_connected_element(&ce));
        let dst = connected_elements
            .next()
            .and_then(|ce| parse_connected_element(&ce));

        let in_modes = extract_in_modes(&child);

        let mut property_associations = Vec::new();
        if let Some(prop_section) = child
            .children()
            .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
        {
            lower_property_associations(&prop_section, tree, &mut property_associations);
        }

        let idx = tree.connections.alloc(ConnectionItem {
            name,
            kind,
            is_bidirectional,
            is_refined,
            src,
            dst,
            in_modes,
            property_associations,
        });
        out.push(idx);
    }
}

/// Parse a CONNECTED_ELEMENT node into a ConnectedElementRef.
///
/// If the node has two IDENT tokens separated by DOT, it's `subcomponent.feature`.
/// If it has one IDENT token, it's just `feature` on the containing component.
fn parse_connected_element(node: &SyntaxNode) -> Option<ConnectedElementRef> {
    let mut idents: Vec<String> = Vec::new();

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::IDENT => idents.push(tok.text().to_string()),
                k if k.is_keyword() && k != SyntaxKind::SELF_KW => {
                    idents.push(tok.text().to_string());
                }
                _ => {}
            }
        }
    }

    match idents.len() {
        0 => None,
        1 => Some(ConnectedElementRef {
            subcomponent: None,
            feature: Name::new(&idents[0]),
        }),
        _ => {
            // Last ident is the feature, preceding idents form the subcomponent path
            // For now, handle the common case of `subcomponent.feature`
            let feature = Name::new(idents.last().unwrap());
            let subcomponent = Name::new(&idents[0]);
            Some(ConnectedElementRef {
                subcomponent: Some(subcomponent),
                feature,
            })
        }
    }
}

// ── Flow spec lowering ─────────────────────────────────────────────

fn lower_flow_specs(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<FlowSpecIdx>) {
    for child in section.children() {
        if child.kind() != SyntaxKind::FLOW_SPEC {
            continue;
        }

        let name = match first_ident_token(&child) {
            Some(tok) => Name::new(tok.text()),
            None => continue,
        };

        let kind = child
            .children()
            .find(|c| c.kind() == SyntaxKind::FLOW_KIND)
            .and_then(|fk| {
                let text = fk.text().to_string();
                match text.trim() {
                    "source" => Some(FlowKind::Source),
                    "sink" => Some(FlowKind::Sink),
                    "path" => Some(FlowKind::Path),
                    _ => None,
                }
            })
            .unwrap_or(FlowKind::Source);

        // Extract flow endpoint features from FLOW_END nodes
        let flow_ends: Vec<Name> = child
            .children()
            .filter(|c| c.kind() == SyntaxKind::FLOW_END)
            .filter_map(|fe| first_ident_token(&fe).map(|tok| Name::new(tok.text())))
            .collect();

        let (source_feature, sink_feature) = match kind {
            FlowKind::Source => (flow_ends.first().cloned(), None),
            FlowKind::Sink => (flow_ends.first().cloned(), None),
            FlowKind::Path => (flow_ends.first().cloned(), flow_ends.get(1).cloned()),
        };

        let in_modes = extract_in_modes(&child);

        let mut property_associations = Vec::new();
        if let Some(prop_section) = child
            .children()
            .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
        {
            lower_property_associations(&prop_section, tree, &mut property_associations);
        }

        let idx = tree.flow_specs.alloc(FlowSpecItem {
            name,
            kind,
            source_feature,
            sink_feature,
            in_modes,
            property_associations,
        });
        out.push(idx);
    }
}

// ── Flow implementation section lowering ───────────────────────────

fn lower_flow_impl_section(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    e2e_out: &mut Vec<EndToEndFlowIdx>,
    flow_impl_out: &mut Vec<FlowImplIdx>,
) {
    for child in section.children() {
        match child.kind() {
            SyntaxKind::END_TO_END_FLOW => {
                if let Some(idx) = lower_single_end_to_end_flow(&child, tree) {
                    e2e_out.push(idx);
                }
            }
            SyntaxKind::FLOW_IMPL => {
                if let Some(idx) = lower_single_flow_impl(&child, tree) {
                    flow_impl_out.push(idx);
                }
            }
            _ => {}
        }
    }
}

fn lower_single_end_to_end_flow(node: &SyntaxNode, tree: &mut ItemTree) -> Option<EndToEndFlowIdx> {
    let name = first_ident_token(node).map(|tok| Name::new(tok.text()))?;

    // Collect segment names from FLOW_SEGMENT children
    let mut segments = Vec::new();
    for seg in node.children() {
        if seg.kind() == SyntaxKind::FLOW_SEGMENT {
            let seg_text = seg.text().to_string();
            let trimmed = seg_text.trim();
            if !trimmed.is_empty() {
                segments.push(Name::new(trimmed));
            }
        }
    }

    let in_modes = extract_in_modes(node);

    let mut property_associations = Vec::new();
    if let Some(prop_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
    {
        lower_property_associations(&prop_section, tree, &mut property_associations);
    }

    Some(tree.end_to_end_flows.alloc(EndToEndFlowItem {
        name,
        segments,
        in_modes,
        property_associations,
    }))
}

fn lower_single_flow_impl(node: &SyntaxNode, tree: &mut ItemTree) -> Option<FlowImplIdx> {
    let name = first_ident_token(node).map(|tok| Name::new(tok.text()))?;

    let kind = node
        .children()
        .find(|c| c.kind() == SyntaxKind::FLOW_KIND)
        .and_then(|fk| {
            let text = fk.text().to_string();
            match text.trim() {
                "source" => Some(FlowKind::Source),
                "sink" => Some(FlowKind::Sink),
                "path" => Some(FlowKind::Path),
                _ => None,
            }
        })
        .unwrap_or(FlowKind::Source);

    let mut segments = Vec::new();
    for seg in node.children() {
        if seg.kind() == SyntaxKind::FLOW_SEGMENT {
            let seg_text = seg.text().to_string();
            let trimmed = seg_text.trim();
            if !trimmed.is_empty() {
                // Parse "sub.flow" into FlowSegment
                let parts: Vec<&str> = trimmed.splitn(2, '.').collect();
                if parts.len() == 2 {
                    segments.push(FlowSegment {
                        subcomponent: Some(Name::new(parts[0])),
                        element: Name::new(parts[1]),
                    });
                } else {
                    segments.push(FlowSegment {
                        subcomponent: None,
                        element: Name::new(trimmed),
                    });
                }
            }
        }
    }

    let in_modes = extract_in_modes(node);

    let mut property_associations = Vec::new();
    if let Some(prop_section) = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_SECTION)
    {
        lower_property_associations(&prop_section, tree, &mut property_associations);
    }

    Some(tree.flow_impls.alloc(FlowImplItem {
        name,
        kind,
        segments,
        in_modes,
        property_associations,
    }))
}

// ── Mode lowering ──────────────────────────────────────────────────

fn lower_modes(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    modes: &mut Vec<ModeIdx>,
    transitions: &mut Vec<ModeTransitionIdx>,
) {
    for child in section.children() {
        match child.kind() {
            SyntaxKind::MODE => {
                if let Some(mode_name) = first_ident_token(&child) {
                    let is_initial = child
                        .children_with_tokens()
                        .filter_map(|it| it.into_token())
                        .any(|tok| tok.kind() == SyntaxKind::INITIAL_KW);
                    let idx = tree.modes.alloc(ModeItem {
                        name: Name::new(mode_name.text()),
                        is_initial,
                    });
                    modes.push(idx);
                }
            }
            SyntaxKind::MODE_TRANSITION => {
                if let Some(idx) = lower_mode_transition(&child, tree) {
                    transitions.push(idx);
                }
            }
            _ => {}
        }
    }
}

fn lower_mode_transition(node: &SyntaxNode, tree: &mut ItemTree) -> Option<ModeTransitionIdx> {
    // Mode transitions: [name :] source -[ trigger, ... ]-> destination ;
    // Collect all IDENT tokens and triggers
    let mut idents: Vec<String> = Vec::new();
    let mut triggers = Vec::new();

    // Check for MODE_TRIGGER children
    for child in node.children() {
        if child.kind() == SyntaxKind::MODE_TRIGGER {
            // Collect trigger port names
            for tok in child
                .children_with_tokens()
                .filter_map(|it| it.into_token())
            {
                if tok.kind() == SyntaxKind::IDENT {
                    triggers.push(Name::new(tok.text()));
                }
            }
        }
    }

    // Collect identifiers from the transition itself
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        if tok.kind() == SyntaxKind::IDENT {
            idents.push(tok.text().to_string());
        }
    }

    // Parse based on number of idents:
    // 2 idents: source, destination (unnamed transition)
    // 3 idents: name, source, destination (named transition)
    match idents.len() {
        2 => Some(tree.mode_transitions.alloc(ModeTransitionItem {
            name: None,
            source: Name::new(&idents[0]),
            triggers,
            destination: Name::new(&idents[1]),
        })),
        n if n >= 3 => Some(tree.mode_transitions.alloc(ModeTransitionItem {
            name: Some(Name::new(&idents[0])),
            source: Name::new(&idents[1]),
            triggers,
            destination: Name::new(&idents[n - 1]),
        })),
        _ => None,
    }
}

// ── Prototype lowering ─────────────────────────────────────────────

fn lower_prototypes(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<PrototypeIdx>) {
    for child in section.children() {
        if child.kind() != SyntaxKind::PROTOTYPE {
            continue;
        }

        let name = match first_ident_token(&child) {
            Some(tok) => Name::new(tok.text()),
            None => continue,
        };

        let category = extract_category(&child);
        let constraining_classifier = child
            .children()
            .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
            .and_then(|cr| parse_classifier_ref_node(&cr));

        let idx = tree.prototypes.alloc(PrototypeItem {
            name,
            category,
            constraining_classifier,
        });
        out.push(idx);
    }
}

// ── Call sequence lowering ─────────────────────────────────────────

fn lower_call_sequences(section: &SyntaxNode, tree: &mut ItemTree, out: &mut Vec<CallSequenceIdx>) {
    for child in section.children() {
        if child.kind() != SyntaxKind::CALL_SEQUENCE {
            continue;
        }

        let name = first_ident_token(&child).map(|tok| Name::new(tok.text()));

        let mut calls = Vec::new();
        for call_node in child.children() {
            if call_node.kind() == SyntaxKind::SUBPROGRAM_CALL
                && let Some(call_name) = first_ident_token(&call_node)
            {
                let called_subprogram = call_node
                    .children()
                    .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
                    .and_then(|cr| parse_classifier_ref_node(&cr));

                let call_idx = tree.subprogram_calls.alloc(SubprogramCallItem {
                    name: Name::new(call_name.text()),
                    called_subprogram,
                });
                calls.push(call_idx);
            }
        }

        let in_modes = extract_in_modes(&child);

        let idx = tree.call_sequences.alloc(CallSequenceItem {
            name,
            calls,
            in_modes,
        });
        out.push(idx);
    }
}

// ── Property association lowering ──────────────────────────────────

fn lower_property_associations(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    out: &mut Vec<PropertyAssociationIdx>,
) {
    for child in section.children() {
        if child.kind() != SyntaxKind::PROPERTY_ASSOCIATION {
            continue;
        }

        if let Some(item) = lower_single_property_association(&child) {
            let idx = tree.property_associations.alloc(item);
            out.push(idx);
        }
    }
}

fn lower_single_property_association(node: &SyntaxNode) -> Option<PropertyAssociationItem> {
    // Extract property reference
    let prop_ref_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::PROPERTY_REF)?;
    let name = parse_property_ref_node(&prop_ref_node)?;

    // Check for +=> (append) vs => (assign)
    let is_append = node
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .any(|tok| tok.kind() == SyntaxKind::PLUS_ARROW);

    // Extract the property value as raw text.
    // We collect all value-bearing children between the arrow and the semicolon.
    let value = extract_property_value_text(node);

    // Extract optional `applies to` path
    let applies_to = node
        .children()
        .find(|c| c.kind() == SyntaxKind::APPLIES_TO)
        .map(|at| {
            // Collect the containment path text
            at.children()
                .filter(|c| c.kind() == SyntaxKind::CONTAINMENT_PATH)
                .map(|cp| cp.text().to_string().trim().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        });

    // Extract modal qualifiers (in modes)
    let in_modes = node
        .children()
        .find(|c| c.kind() == SyntaxKind::MODAL_PROPERTY_VALUE)
        .map(|mpv| extract_mode_names_from_modal(&mpv))
        .unwrap_or_default();

    // Try to lower the property value into a typed PropertyExpr.
    // Find the first value-bearing CST node child of the PROPERTY_ASSOCIATION.
    let typed_value = find_property_value_node(node).and_then(|vn| lower_property_expr(&vn));

    Some(PropertyAssociationItem {
        name,
        value,
        typed_value,
        is_append,
        applies_to,
        in_modes,
    })
}

/// Extract the raw text of the property value from a PROPERTY_ASSOCIATION node.
///
/// The value is everything between the `=>` / `+=>` and the `;` (or `applies`
/// or `in binding`), excluding modal qualifiers for now.
fn extract_property_value_text(node: &SyntaxNode) -> String {
    let mut parts = Vec::new();
    let mut collecting = false;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            if tok.kind() == SyntaxKind::FAT_ARROW || tok.kind() == SyntaxKind::PLUS_ARROW {
                collecting = true;
                continue;
            }
            if tok.kind() == SyntaxKind::SEMICOLON {
                break;
            }
            // Skip the `constant` keyword that can appear right after =>
            if tok.kind() == SyntaxKind::CONSTANT_KW && collecting {
                continue;
            }
        }
        if collecting {
            // Stop if we hit APPLIES_TO, IN_BINDING, or MODAL_PROPERTY_VALUE
            if let Some(n) = elem.as_node() {
                match n.kind() {
                    SyntaxKind::APPLIES_TO | SyntaxKind::IN_BINDING => break,
                    SyntaxKind::MODAL_PROPERTY_VALUE => break,
                    _ => {
                        parts.push(n.text().to_string());
                        continue;
                    }
                }
            }
            if let Some(tok) = elem.as_token() {
                // Skip whitespace-only tokens for cleaner output
                let text = tok.text();
                if !text.trim().is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }

    let raw = parts.join(" ");
    // Normalize whitespace
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Find the first property value CST node within a PROPERTY_ASSOCIATION.
///
/// Looks for value-bearing nodes (INTEGER_VALUE, REAL_VALUE, STRING_VALUE, etc.)
/// that appear after the `=>` / `+=>` token.
fn find_property_value_node(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut past_arrow = false;
    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token()
            && (tok.kind() == SyntaxKind::FAT_ARROW || tok.kind() == SyntaxKind::PLUS_ARROW)
        {
            past_arrow = true;
            continue;
        }
        if past_arrow && let Some(n) = elem.as_node() {
            match n.kind() {
                SyntaxKind::INTEGER_VALUE
                | SyntaxKind::REAL_VALUE
                | SyntaxKind::STRING_VALUE
                | SyntaxKind::BOOLEAN_VALUE
                | SyntaxKind::LIST_VALUE
                | SyntaxKind::RECORD_VALUE
                | SyntaxKind::RANGE_VALUE
                | SyntaxKind::CLASSIFIER_VALUE
                | SyntaxKind::REFERENCE_VALUE
                | SyntaxKind::COMPUTED_VALUE
                | SyntaxKind::PROPERTY_EXPRESSION => {
                    return Some(n.clone());
                }
                SyntaxKind::APPLIES_TO
                | SyntaxKind::IN_BINDING
                | SyntaxKind::MODAL_PROPERTY_VALUE => {
                    // Stop searching — we've passed the value region
                    return None;
                }
                _ => {}
            }
        }
    }
    None
}

/// Lower a CST property value node into a typed `PropertyExpr`.
///
/// Handles all property expression node kinds produced by the parser:
/// INTEGER_VALUE, REAL_VALUE, STRING_VALUE, BOOLEAN_VALUE, LIST_VALUE,
/// RECORD_VALUE, RANGE_VALUE, CLASSIFIER_VALUE, REFERENCE_VALUE,
/// COMPUTED_VALUE, and PROPERTY_EXPRESSION (for named values/enums).
///
/// Returns `None` if the node cannot be lowered (the caller should fall back
/// to `PropertyExpr::Opaque`).
fn lower_property_expr(node: &SyntaxNode) -> Option<PropertyExpr> {
    match node.kind() {
        SyntaxKind::INTEGER_VALUE => lower_integer_value(node),
        SyntaxKind::REAL_VALUE => lower_real_value(node),
        SyntaxKind::STRING_VALUE => lower_string_value(node),
        SyntaxKind::BOOLEAN_VALUE => lower_boolean_value(node),
        SyntaxKind::LIST_VALUE => lower_list_value(node),
        SyntaxKind::RECORD_VALUE => lower_record_value(node),
        SyntaxKind::RANGE_VALUE => lower_range_value(node),
        SyntaxKind::CLASSIFIER_VALUE => lower_classifier_value(node),
        SyntaxKind::REFERENCE_VALUE => lower_reference_value(node),
        SyntaxKind::COMPUTED_VALUE => lower_computed_value(node),
        SyntaxKind::PROPERTY_EXPRESSION => lower_named_value(node),
        _ => None,
    }
}

/// Lower an INTEGER_VALUE node.
///
/// Structure: INTEGER_LIT [IDENT(unit)]
/// The parser may also produce a signed wrapper: PROPERTY_EXPRESSION → PLUS/MINUS INTEGER_VALUE
fn lower_integer_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut int_text: Option<String> = None;
    let mut unit: Option<Name> = None;
    let mut sign: i64 = 1;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::INTEGER_LIT => {
                    int_text = Some(tok.text().to_string());
                }
                SyntaxKind::IDENT => {
                    unit = Some(Name::new(tok.text()));
                }
                SyntaxKind::MINUS => {
                    sign = -1;
                }
                SyntaxKind::PLUS => {
                    sign = 1;
                }
                _ => {}
            }
        }
    }

    let text = int_text?;
    let val = parse_aadl_integer(&text)?;
    let val = val * sign;

    if let Some(u) = unit {
        Some(PropertyExpr::Integer(val, Some(u)))
    } else {
        Some(PropertyExpr::Integer(val, None))
    }
}

/// Parse an AADL integer literal.
///
/// Supports decimal and based literals like `16#FF#`.
fn parse_aadl_integer(s: &str) -> Option<i64> {
    let s = s.replace('_', "");
    // Check for based notation: base#digits#
    if let Some(hash_pos) = s.find('#') {
        let base_str = &s[..hash_pos];
        let rest = &s[hash_pos + 1..];
        if let Some(end_hash) = rest.find('#') {
            let digits = &rest[..end_hash];
            let base = base_str.parse::<u32>().ok()?;
            return i64::from_str_radix(digits, base).ok();
        }
    }
    s.parse::<i64>().ok()
}

/// Lower a REAL_VALUE node.
///
/// Structure: REAL_LIT [IDENT(unit)]
fn lower_real_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut real_text: Option<String> = None;
    let mut unit: Option<Name> = None;
    let mut negative = false;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::REAL_LIT => {
                    real_text = Some(tok.text().to_string());
                }
                SyntaxKind::IDENT => {
                    unit = Some(Name::new(tok.text()));
                }
                SyntaxKind::MINUS => {
                    negative = true;
                }
                _ => {}
            }
        }
    }

    let text = real_text?;
    let display = if negative { format!("-{}", text) } else { text };

    if let Some(u) = unit {
        Some(PropertyExpr::Real(display, Some(u)))
    } else {
        Some(PropertyExpr::Real(display, None))
    }
}

/// Lower a STRING_VALUE node.
///
/// Structure: STRING_LIT
fn lower_string_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        if tok.kind() == SyntaxKind::STRING_LIT {
            let raw = tok.text();
            // Strip surrounding quotes
            let unquoted = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
                &raw[1..raw.len() - 1]
            } else {
                raw
            };
            return Some(PropertyExpr::StringLit(unquoted.to_string()));
        }
    }
    None
}

/// Lower a BOOLEAN_VALUE node.
///
/// Structure: TRUE_KW | FALSE_KW | NOT_KW BOOLEAN_VALUE
fn lower_boolean_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    // Check for `not` prefix
    let mut has_not = false;
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        match tok.kind() {
            SyntaxKind::NOT_KW => {
                has_not = true;
            }
            SyntaxKind::TRUE_KW => {
                return Some(PropertyExpr::Boolean(!has_not));
            }
            SyntaxKind::FALSE_KW => {
                return Some(PropertyExpr::Boolean(has_not));
            }
            _ => {}
        }
    }
    // If there are child nodes (e.g., `not` wrapping another boolean expression)
    for child in node.children() {
        if child.kind() == SyntaxKind::BOOLEAN_VALUE
            && let Some(PropertyExpr::Boolean(val)) = lower_boolean_value(&child)
        {
            return Some(PropertyExpr::Boolean(if has_not { !val } else { val }));
        }
    }
    None
}

/// Lower a LIST_VALUE node.
///
/// Structure: L_PAREN [property_expression, ...] R_PAREN
fn lower_list_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut items = Vec::new();
    for child in node.children() {
        if let Some(expr) = lower_property_expr(&child) {
            items.push(expr);
        } else if is_value_node(child.kind()) {
            // Fallback: store as opaque
            items.push(PropertyExpr::Opaque(
                child.text().to_string().trim().to_string(),
            ));
        }
    }
    Some(PropertyExpr::List(items))
}

/// Lower a RECORD_VALUE node.
///
/// Structure: L_BRACKET [RECORD_FIELD ...] R_BRACKET
/// RECORD_FIELD: IDENT FAT_ARROW property_expression SEMICOLON
fn lower_record_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut fields = Vec::new();
    for child in node.children() {
        if child.kind() == SyntaxKind::RECORD_FIELD {
            let field_name = first_ident_token(&child).map(|t| Name::new(t.text()));
            // Find the value node within the record field
            let field_value = child
                .children()
                .find_map(|c| lower_property_expr(&c))
                .unwrap_or_else(|| {
                    // Fallback: extract text after => as opaque
                    let text = child.text().to_string();
                    PropertyExpr::Opaque(text.trim().to_string())
                });
            if let Some(name) = field_name {
                fields.push((name, field_value));
            }
        }
    }
    Some(PropertyExpr::Record(fields))
}

/// Lower a RANGE_VALUE node.
///
/// Structure: property_expression DOT_DOT property_expression [DELTA_VALUE]
/// DELTA_VALUE: DELTA_KW property_expression
fn lower_range_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let mut value_nodes: Vec<SyntaxNode> = Vec::new();
    let mut delta_node: Option<SyntaxNode> = None;

    for child in node.children() {
        if child.kind() == SyntaxKind::DELTA_VALUE {
            delta_node = Some(child);
        } else if is_value_node(child.kind()) {
            value_nodes.push(child);
        }
    }

    // We need at least two value nodes (min and max)
    if value_nodes.len() < 2 {
        // Might be that the min is a token directly in the range node
        // (e.g., `10 .. 20` where 10 is the leading token before the range was created)
        // Try to extract them from the full node
        return lower_range_from_tokens(node);
    }

    let min_expr = lower_property_expr(&value_nodes[0]).unwrap_or_else(|| {
        PropertyExpr::Opaque(value_nodes[0].text().to_string().trim().to_string())
    });
    let max_expr = lower_property_expr(&value_nodes[1]).unwrap_or_else(|| {
        PropertyExpr::Opaque(value_nodes[1].text().to_string().trim().to_string())
    });

    let delta = delta_node.and_then(|dn| {
        dn.children()
            .find_map(|c| lower_property_expr(&c))
            .map(Box::new)
    });

    Some(PropertyExpr::Range {
        min: Box::new(min_expr),
        max: Box::new(max_expr),
        delta,
    })
}

/// Lower a range value by parsing tokens directly.
///
/// Handles the case where the range node contains the min value as tokens
/// (e.g., INTEGER_LIT [IDENT] DOT_DOT ...) rather than as a child node.
fn lower_range_from_tokens(node: &SyntaxNode) -> Option<PropertyExpr> {
    // The RANGE_VALUE node was created by the parser wrapping everything:
    //   INTEGER_LIT [IDENT] DOT_DOT property_expression [DELTA_VALUE]
    // or
    //   REAL_LIT [IDENT] DOT_DOT property_expression [DELTA_VALUE]
    //
    // The min value's tokens are directly in this node, while the max
    // is a child node.

    let mut min_num: Option<String> = None;
    let mut min_unit: Option<Name> = None;
    let mut min_is_real = false;
    let mut min_sign: i64 = 1;
    let mut past_dot_dot = false;

    let mut max_expr: Option<PropertyExpr> = None;
    let mut delta_expr: Option<PropertyExpr> = None;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::INTEGER_LIT if !past_dot_dot => {
                    min_num = Some(tok.text().to_string());
                    min_is_real = false;
                }
                SyntaxKind::REAL_LIT if !past_dot_dot => {
                    min_num = Some(tok.text().to_string());
                    min_is_real = true;
                }
                SyntaxKind::IDENT if !past_dot_dot && min_num.is_some() => {
                    min_unit = Some(Name::new(tok.text()));
                }
                SyntaxKind::MINUS if !past_dot_dot && min_num.is_none() => {
                    min_sign = -1;
                }
                SyntaxKind::DOT_DOT => {
                    past_dot_dot = true;
                }
                _ => {}
            }
        }
        if let Some(n) = elem.as_node()
            && past_dot_dot
        {
            if n.kind() == SyntaxKind::DELTA_VALUE {
                delta_expr = n.children().find_map(|c| lower_property_expr(&c));
            } else if max_expr.is_none() {
                max_expr = lower_property_expr(n);
            }
        }
    }

    let min = if let Some(num_text) = min_num {
        if min_is_real {
            let display = if min_sign < 0 {
                format!("-{}", num_text)
            } else {
                num_text
            };
            PropertyExpr::Real(display, min_unit)
        } else {
            let val = parse_aadl_integer(&num_text).unwrap_or(0) * min_sign;
            PropertyExpr::Integer(val, min_unit)
        }
    } else {
        return None;
    };

    let max = max_expr.unwrap_or_else(|| PropertyExpr::Opaque("?".to_string()));

    Some(PropertyExpr::Range {
        min: Box::new(min),
        max: Box::new(max),
        delta: delta_expr.map(Box::new),
    })
}

/// Lower a CLASSIFIER_VALUE node.
///
/// Structure: CLASSIFIER_KW L_PAREN CLASSIFIER_REF R_PAREN
fn lower_classifier_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let cr = node
        .children()
        .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
        .and_then(|cr| parse_classifier_ref_node(&cr))?;
    Some(PropertyExpr::ClassifierValue(cr))
}

/// Lower a REFERENCE_VALUE node.
///
/// Structure: REFERENCE_KW L_PAREN CONTAINMENT_PATH R_PAREN
fn lower_reference_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    let path = node
        .children()
        .find(|c| c.kind() == SyntaxKind::CONTAINMENT_PATH)
        .map(|cp| cp.text().to_string().trim().to_string())
        .unwrap_or_default();
    Some(PropertyExpr::ReferenceValue(path))
}

/// Lower a COMPUTED_VALUE node.
///
/// Structure: COMPUTE_KW L_PAREN IDENT R_PAREN
fn lower_computed_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    // Find the IDENT inside (skip the compute keyword)
    let mut past_lparen = false;
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        match tok.kind() {
            SyntaxKind::L_PAREN => {
                past_lparen = true;
            }
            SyntaxKind::IDENT if past_lparen => {
                return Some(PropertyExpr::ComputedValue(Name::new(tok.text())));
            }
            _ => {}
        }
    }
    None
}

/// Lower a PROPERTY_EXPRESSION node (named/enum values).
///
/// Structure: IDENT [COLON_COLON IDENT ...] (for qualified property constant refs)
/// or: PLUS/MINUS property_expression (signed value wrapper)
///
/// A bare IDENT in property context is treated as an enum literal.
/// A qualified reference like `PS::Const` is also treated as an enum/named value.
fn lower_named_value(node: &SyntaxNode) -> Option<PropertyExpr> {
    // Check if this is a signed wrapper (PLUS/MINUS followed by a child expr)
    let first_tok = node
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .next();

    if let Some(ref tok) = first_tok
        && (tok.kind() == SyntaxKind::PLUS || tok.kind() == SyntaxKind::MINUS)
    {
        // Signed value wrapper — find the inner expression
        let sign_negative = tok.kind() == SyntaxKind::MINUS;
        for child in node.children() {
            if let Some(inner) = lower_property_expr(&child) {
                return Some(apply_sign(inner, sign_negative));
            }
        }
        return None;
    }

    // Collect all identifiers and separators to form the name
    let mut idents: Vec<String> = Vec::new();
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        match tok.kind() {
            SyntaxKind::IDENT => idents.push(tok.text().to_string()),
            k if k.is_keyword() => idents.push(tok.text().to_string()),
            _ => {}
        }
    }

    if idents.is_empty() {
        return None;
    }

    // A single identifier is an enum literal
    // Multiple identifiers (qualified) are joined with "::"
    let full_name = idents.join("::");
    Some(PropertyExpr::Enum(Name::new(&full_name)))
}

/// Apply a sign to a property expression (negate integers/reals).
fn apply_sign(expr: PropertyExpr, negative: bool) -> PropertyExpr {
    if !negative {
        return expr;
    }
    match expr {
        PropertyExpr::Integer(val, unit) => PropertyExpr::Integer(-val, unit),
        PropertyExpr::Real(val, unit) => {
            if let Some(stripped) = val.strip_prefix('-') {
                PropertyExpr::Real(stripped.to_string(), unit)
            } else {
                PropertyExpr::Real(format!("-{}", val), unit)
            }
        }
        other => other,
    }
}

/// Check whether a SyntaxKind represents a property value node.
fn is_value_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::INTEGER_VALUE
            | SyntaxKind::REAL_VALUE
            | SyntaxKind::STRING_VALUE
            | SyntaxKind::BOOLEAN_VALUE
            | SyntaxKind::LIST_VALUE
            | SyntaxKind::RECORD_VALUE
            | SyntaxKind::RANGE_VALUE
            | SyntaxKind::CLASSIFIER_VALUE
            | SyntaxKind::REFERENCE_VALUE
            | SyntaxKind::COMPUTED_VALUE
            | SyntaxKind::PROPERTY_EXPRESSION
    )
}

/// Parse a PROPERTY_REF node into a PropertyRef.
///
/// Formats:
/// - `PropName` → unqualified
/// - `PropSet::PropName` → qualified
fn parse_property_ref_node(node: &SyntaxNode) -> Option<PropertyRef> {
    let mut idents: Vec<String> = Vec::new();
    let mut has_colon_colon = false;

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::IDENT => idents.push(tok.text().to_string()),
                SyntaxKind::COLON_COLON => has_colon_colon = true,
                k if k.is_keyword() => idents.push(tok.text().to_string()),
                _ => {}
            }
        }
    }

    match (idents.len(), has_colon_colon) {
        (1, false) => Some(PropertyRef {
            property_set: None,
            property_name: Name::new(&idents[0]),
        }),
        (2, true) => Some(PropertyRef {
            property_set: Some(Name::new(&idents[0])),
            property_name: Name::new(&idents[1]),
        }),
        _ => {
            if !idents.is_empty() {
                Some(PropertyRef {
                    property_set: None,
                    property_name: Name::new(idents.last().unwrap()),
                })
            } else {
                None
            }
        }
    }
}

// ── Helper functions ───────────────────────────────────────────────

fn first_ident_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|tok| tok.kind() == SyntaxKind::IDENT)
}

/// Extract name from a node that may contain the IDENT directly
/// or via a NAME child node.
fn extract_name_text(node: &SyntaxNode) -> Option<Name> {
    // First try: NAME child node containing IDENT(s)
    for child in node.children() {
        if child.kind() == SyntaxKind::NAME {
            let text = child
                .children_with_tokens()
                .filter_map(|it| it.into_token())
                .find(|tok| tok.kind() == SyntaxKind::IDENT)?;
            return Some(Name::new(text.text()));
        }
    }
    // Fallback: direct IDENT token child
    first_ident_token(node).map(|tok| Name::new(tok.text()))
}

fn extract_decl_name(node: &SyntaxNode) -> Option<Name> {
    // Component types/impls have IDENT directly as children
    first_ident_token(node).map(|tok| Name::new(tok.text()))
}

fn extract_impl_name(node: &SyntaxNode) -> Option<Name> {
    // In a COMPONENT_IMPL, the impl name is the IDENT after the DOT
    let mut saw_dot = false;
    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            if tok.kind() == SyntaxKind::DOT {
                saw_dot = true;
            } else if saw_dot && tok.kind() == SyntaxKind::IDENT {
                return Some(Name::new(tok.text()));
            }
        }
    }
    None
}

fn extract_category(node: &SyntaxNode) -> Option<ComponentCategory> {
    let cat_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::COMPONENT_CATEGORY)?;
    let text = cat_node.text().to_string();
    parse_category(&text)
}

fn parse_category(text: &str) -> Option<ComponentCategory> {
    let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match normalized.as_str() {
        "system" => Some(ComponentCategory::System),
        "process" => Some(ComponentCategory::Process),
        "thread" => Some(ComponentCategory::Thread),
        "thread group" => Some(ComponentCategory::ThreadGroup),
        "processor" => Some(ComponentCategory::Processor),
        "virtual processor" => Some(ComponentCategory::VirtualProcessor),
        "memory" => Some(ComponentCategory::Memory),
        "bus" => Some(ComponentCategory::Bus),
        "virtual bus" => Some(ComponentCategory::VirtualBus),
        "device" => Some(ComponentCategory::Device),
        "subprogram" => Some(ComponentCategory::Subprogram),
        "subprogram group" => Some(ComponentCategory::SubprogramGroup),
        "data" => Some(ComponentCategory::Data),
        "abstract" => Some(ComponentCategory::Abstract),
        _ => None,
    }
}

fn extract_direction(node: &SyntaxNode) -> Option<Direction> {
    let dir_node = node
        .children()
        .find(|c| c.kind() == SyntaxKind::DIRECTION)?;
    let text = dir_node.text().to_string();
    let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match normalized.as_str() {
        "in" => Some(Direction::In),
        "out" => Some(Direction::Out),
        "in out" => Some(Direction::InOut),
        _ => None,
    }
}

/// Extract `provides` or `requires` from an access feature node.
fn extract_access_kind(node: &SyntaxNode) -> Option<AccessKind> {
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        match tok.kind() {
            SyntaxKind::PROVIDES_KW => return Some(AccessKind::Provides),
            SyntaxKind::REQUIRES_KW => return Some(AccessKind::Requires),
            _ => {}
        }
    }
    None
}

/// Extract array dimensions from ARRAY_DIMENSION child nodes.
fn extract_array_dimensions(node: &SyntaxNode) -> Vec<ArrayDimension> {
    let mut dims = Vec::new();
    for child in node.children() {
        if child.kind() == SyntaxKind::ARRAY_DIMENSION {
            let size = child
                .children()
                .find(|c| c.kind() == SyntaxKind::ARRAY_SIZE)
                .and_then(|size_node| {
                    // Try to parse integer literal
                    for tok in size_node
                        .children_with_tokens()
                        .filter_map(|it| it.into_token())
                    {
                        if tok.kind() == SyntaxKind::INTEGER_LIT
                            && let Ok(n) = tok.text().parse::<u64>()
                        {
                            return Some(ArraySize::Literal(n));
                        }
                    }
                    None
                });
            dims.push(ArrayDimension { size });
        }
    }
    dims
}

/// Extract `in modes (m1, m2)` clause from a node.
fn extract_in_modes(node: &SyntaxNode) -> Vec<Name> {
    // Look for MODAL_PROPERTY_VALUE or a pattern of `in modes (...)` tokens
    for child in node.children() {
        if child.kind() == SyntaxKind::MODAL_PROPERTY_VALUE {
            return extract_mode_names_from_modal(&child);
        }
    }
    // Also look for inline `in modes` at token level
    let mut in_modes = false;
    let mut in_parens = false;
    let mut modes = Vec::new();
    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::MODES_KW => in_modes = true,
                SyntaxKind::L_PAREN if in_modes => in_parens = true,
                SyntaxKind::R_PAREN if in_parens => {
                    return modes;
                }
                SyntaxKind::IDENT if in_parens => {
                    modes.push(Name::new(tok.text()));
                }
                _ => {}
            }
        }
    }
    modes
}

/// Extract mode names from a MODAL_PROPERTY_VALUE node.
fn extract_mode_names_from_modal(node: &SyntaxNode) -> Vec<Name> {
    let mut modes = Vec::new();
    let mut in_parens = false;
    for tok in node.children_with_tokens().filter_map(|it| it.into_token()) {
        match tok.kind() {
            SyntaxKind::L_PAREN => in_parens = true,
            SyntaxKind::R_PAREN => break,
            SyntaxKind::IDENT if in_parens => {
                modes.push(Name::new(tok.text()));
            }
            _ => {}
        }
    }
    modes
}

fn extract_inverse_of(node: &SyntaxNode) -> Option<ClassifierRef> {
    // Look for INVERSE_KW followed by OF_KW followed by CLASSIFIER_REF
    let mut saw_inverse = false;
    for child in node.children_with_tokens() {
        if let Some(tok) = child.as_token()
            && tok.kind() == SyntaxKind::INVERSE_KW
        {
            saw_inverse = true;
        }
        if saw_inverse
            && let Some(n) = child.as_node()
            && n.kind() == SyntaxKind::CLASSIFIER_REF
        {
            return parse_classifier_ref_node(n);
        }
    }
    None
}

fn extract_classifier_ref_from_child(node: &SyntaxNode) -> Option<ClassifierRef> {
    node.children()
        .find(|c| c.kind() == SyntaxKind::CLASSIFIER_REF)
        .and_then(|cr| parse_classifier_ref_node(&cr))
}

/// Parse a CLASSIFIER_REF node into a ClassifierRef.
///
/// Formats:
/// - `Name` → type_only
/// - `Pkg::Name` → qualified
/// - `Name.Impl` → implementation (no package)
/// - `Pkg::Name.Impl` → implementation (with package)
fn parse_classifier_ref_node(node: &SyntaxNode) -> Option<ClassifierRef> {
    let mut idents: Vec<String> = Vec::new();
    let mut separators: Vec<SyntaxKind> = Vec::new();

    for elem in node.children_with_tokens() {
        if let Some(tok) = elem.as_token() {
            match tok.kind() {
                SyntaxKind::IDENT => idents.push(tok.text().to_string()),
                SyntaxKind::COLON_COLON | SyntaxKind::DOT => separators.push(tok.kind()),
                k if k.is_keyword() => idents.push(tok.text().to_string()),
                _ => {}
            }
        }
    }

    match (idents.len(), separators.as_slice()) {
        // `Name`
        (1, []) => Some(ClassifierRef::type_only(Name::new(&idents[0]))),
        // `Pkg::Name`
        (2, [SyntaxKind::COLON_COLON]) => Some(ClassifierRef::qualified(
            Name::new(&idents[0]),
            Name::new(&idents[1]),
        )),
        // `Name.Impl`
        (2, [SyntaxKind::DOT]) => Some(ClassifierRef::implementation(
            None,
            Name::new(&idents[0]),
            Name::new(&idents[1]),
        )),
        // `Pkg::Name.Impl`
        (3, [SyntaxKind::COLON_COLON, SyntaxKind::DOT]) => Some(ClassifierRef::implementation(
            Some(Name::new(&idents[0])),
            Name::new(&idents[1]),
            Name::new(&idents[2]),
        )),
        _ => {
            // Fallback: treat everything as a single name
            if !idents.is_empty() {
                Some(ClassifierRef::type_only(Name::new(&idents[0])))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod lowering_diagnostic_tests {
    use super::*;
    use spar_syntax::parse;

    #[test]
    fn annex_library_emits_diagnostic() {
        let src = r#"
package Pkg
public
  annex EMV2 {**
    error model behavior
  **};
end Pkg;
"#;
        let parsed = parse(src);
        let tree = lower_file(&parsed.syntax_node());
        assert!(
            tree.diagnostics
                .iter()
                .any(|d| d.message.contains("annex") && d.severity == LoweringSeverity::Warning),
            "should emit warning diagnostic for unparsed annex: {:?}",
            tree.diagnostics
        );
    }

    #[test]
    fn known_syntax_kinds_no_spurious_warnings() {
        let src = r#"
package Pkg
public
  system S
  end S;
end Pkg;
"#;
        let parsed = parse(src);
        let tree = lower_file(&parsed.syntax_node());
        let unhandled: Vec<_> = tree
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unhandled"))
            .collect();
        assert!(
            unhandled.is_empty(),
            "known constructs should not produce unhandled warnings: {:?}",
            unhandled
        );
    }

    #[test]
    fn no_diagnostics_for_clean_package() {
        let src = r#"
package Clean
public
  system A
    features
      inp: in data port;
  end A;

  system implementation A.Impl
    subcomponents
      sub1: system B;
  end A.Impl;

  system B
  end B;
end Clean;
"#;
        let parsed = parse(src);
        let tree = lower_file(&parsed.syntax_node());
        assert!(
            tree.diagnostics.is_empty(),
            "clean package should produce no diagnostics: {:?}",
            tree.diagnostics
        );
    }
}
