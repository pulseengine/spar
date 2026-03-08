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
    let file = match ast::SourceFile::cast(root.clone()) {
        Some(f) => f,
        None => return tree,
    };

    for pkg in file.packages() {
        lower_package(&pkg, &mut tree);
    }

    for ps in file.property_sets() {
        lower_property_set(&ps, &mut tree);
    }

    tree
}

fn lower_package(pkg: &ast::AadlPackage, tree: &mut ItemTree) {
    // Package name is inside a NAME node child
    let name = match extract_name_text(pkg.syntax()) {
        Some(n) => n,
        None => return,
    };

    let mut with_clauses = Vec::new();
    let mut public_items = Vec::new();
    let mut private_items = Vec::new();

    // Walk children to find sections and with clauses
    for child in pkg.syntax().children() {
        match child.kind() {
            SyntaxKind::WITH_CLAUSE => {
                collect_with_names(&child, &mut with_clauses);
            }
            SyntaxKind::PUBLIC_SECTION => {
                lower_section(&child, tree, &mut public_items, &mut with_clauses);
            }
            SyntaxKind::PRIVATE_SECTION => {
                lower_section(&child, tree, &mut private_items, &mut with_clauses);
            }
            _ => {}
        }
    }

    tree.packages.alloc(Package {
        name,
        with_clauses,
        public_items,
        private_items,
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

fn lower_section(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    items: &mut Vec<ItemRef>,
    with_clauses: &mut Vec<Name>,
) {
    for child in section.children() {
        match child.kind() {
            SyntaxKind::WITH_CLAUSE => {
                collect_with_names(&child, with_clauses);
            }
            SyntaxKind::COMPONENT_TYPE => {
                if let Some(item_ref) = lower_component_type(&child, tree) {
                    items.push(item_ref);
                }
            }
            SyntaxKind::COMPONENT_IMPL => {
                if let Some(item_ref) = lower_component_impl(&child, tree) {
                    items.push(item_ref);
                }
            }
            SyntaxKind::FEATURE_GROUP_TYPE => {
                if let Some(item_ref) = lower_feature_group_type(&child, tree) {
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
            }
            _ => {}
        }
    }
}

fn lower_component_type(node: &SyntaxNode, tree: &mut ItemTree) -> Option<ItemRef> {
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

fn lower_component_impl(node: &SyntaxNode, tree: &mut ItemTree) -> Option<ItemRef> {
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

fn lower_feature_group_type(node: &SyntaxNode, tree: &mut ItemTree) -> Option<ItemRef> {
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
        extends,
        inverse_of,
        features,
        prototypes,
    });
    Some(ItemRef::FeatureGroupType(idx))
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

    let (property_defs, property_type_defs, property_constants) =
        extract_property_set_items(node);

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
                    defs.push(PropertyDefItem {
                        name: Name::new(tok.text()),
                        type_def: None,
                        default_value: None,
                        applies_to: Vec::new(),
                    });
                }
            }
            SyntaxKind::PROPERTY_TYPE_DECL => {
                if let Some(tok) = first_ident_token(&child) {
                    type_defs.push(PropertyTypeDefItem {
                        name: Name::new(tok.text()),
                        type_def: None,
                    });
                }
            }
            SyntaxKind::PROPERTY_CONSTANT => {
                if let Some(tok) = first_ident_token(&child) {
                    constants.push(PropertyConstantItem {
                        name: Name::new(tok.text()),
                        type_def: None,
                        value: None,
                    });
                }
            }
            _ => {}
        }
    }

    (defs, type_defs, constants)
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

        let is_refined = node
            .children()
            .any(|c| c.kind() == SyntaxKind::REFINED_TO);

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

fn lower_subcomponents(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    out: &mut Vec<SubcomponentIdx>,
) {
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

        let is_refined = child
            .children()
            .any(|c| c.kind() == SyntaxKind::REFINED_TO);

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

        let is_refined = child
            .children()
            .any(|c| c.kind() == SyntaxKind::REFINED_TO);

        // Extract source and destination CONNECTED_ELEMENT nodes
        let mut connected_elements = child
            .children()
            .filter(|c| c.kind() == SyntaxKind::CONNECTED_ELEMENT);

        let src = connected_elements.next().and_then(|ce| parse_connected_element(&ce));
        let dst = connected_elements.next().and_then(|ce| parse_connected_element(&ce));

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
            .filter_map(|fe| {
                first_ident_token(&fe).map(|tok| Name::new(tok.text()))
            })
            .collect();

        let (source_feature, sink_feature) = match kind {
            FlowKind::Source => (flow_ends.first().cloned(), None),
            FlowKind::Sink => (flow_ends.first().cloned(), None),
            FlowKind::Path => (
                flow_ends.first().cloned(),
                flow_ends.get(1).cloned(),
            ),
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

fn lower_single_end_to_end_flow(
    node: &SyntaxNode,
    tree: &mut ItemTree,
) -> Option<EndToEndFlowIdx> {
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
            for tok in child.children_with_tokens().filter_map(|it| it.into_token()) {
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

fn lower_prototypes(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    out: &mut Vec<PrototypeIdx>,
) {
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

fn lower_call_sequences(
    section: &SyntaxNode,
    tree: &mut ItemTree,
    out: &mut Vec<CallSequenceIdx>,
) {
    for child in section.children() {
        if child.kind() != SyntaxKind::CALL_SEQUENCE {
            continue;
        }

        let name = first_ident_token(&child).map(|tok| Name::new(tok.text()));

        let mut calls = Vec::new();
        for call_node in child.children() {
            if call_node.kind() == SyntaxKind::SUBPROGRAM_CALL {
                if let Some(call_name) = first_ident_token(&call_node) {
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

    Some(PropertyAssociationItem {
        name,
        value,
        typed_value: None,
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
                    for tok in size_node.children_with_tokens().filter_map(|it| it.into_token()) {
                        if tok.kind() == SyntaxKind::INTEGER_LIT {
                            if let Ok(n) = tok.text().parse::<u64>() {
                                return Some(ArraySize::Literal(n));
                            }
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
        if let Some(tok) = child.as_token() {
            if tok.kind() == SyntaxKind::INVERSE_KW {
                saw_inverse = true;
            }
        }
        if saw_inverse {
            if let Some(n) = child.as_node() {
                if n.kind() == SyntaxKind::CLASSIFIER_REF {
                    return parse_classifier_ref_node(n);
                }
            }
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
