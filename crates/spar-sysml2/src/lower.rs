//! SysML v2 to AADL lowering.
//!
//! Walks a SysML v2 rowan CST and produces an AADL [`ItemTree`] using the
//! SEI mapping rules:
//!
//! | SysML v2 | AADL |
//! |----------|------|
//! | `part def` (with hardware-like features) | `system type` |
//! | `part def` (with software-like features) | `process type` / `thread type` |
//! | `part` usage inside a `part def` | Subcomponent |
//! | `port def` with `out item` | Data port feature (out) |
//! | `port def` with `in item` | Data port feature (in) |
//! | `connection def` / `connect` | Connection |
//! | `attribute def` | `data type` (property carrier) |
//! | `attribute` usage | AADL property association |
//! | `item def` | `data type` |
//! | `enum def` | `data type` with enumeration property |
//! | `interface def` | Feature group type |
//! | `action def` | `subprogram type` |
//! | `state def` | mode declaration |
//! | `constraint def` | Timing property annotation |
//! | `calc def` | `subprogram type` |
//! | `allocation def` | Processor binding annotation |
//! | `requirement def` | property annotation |
//! | `specializes` | AADL `extends` |
//! | `abstract part def` | `abstract` component type |
//! | `ref part` | subcomponent reference |
//! | `import` | `with` clause |

use spar_hir_def::item_tree::{
    ComponentCategory, ComponentImplItem, ComponentTypeItem, ConnectedElementRef, ConnectionItem,
    ConnectionKind, Direction, Feature, FeatureGroupTypeItem, FeatureKind, ItemRef, ItemTree,
    Package, PropertyAssociationItem, PropertyExpr, SubcomponentItem,
};
use spar_hir_def::name::{Name, PropertyRef};

use crate::SyntaxNode;
use crate::syntax_kind::SyntaxKind;

/// Diagnostic emitted when the SysML v2→AADL lowering encounters a construct
/// it cannot translate.
#[derive(Debug, Clone)]
pub struct LowerDiagnostic {
    /// Byte offset into the source where the unrecognized construct starts.
    pub offset: usize,
    /// Human-readable description of what was skipped.
    pub msg: String,
}

/// Lower a parsed SysML v2 source file to an AADL [`ItemTree`].
///
/// Walks the rowan CST produced by [`crate::parse`] and maps SysML v2
/// constructs to their AADL equivalents following SEI guidelines.
///
/// Diagnostics about unrecognized constructs are silently discarded.
/// Use [`lower_to_aadl_with_diagnostics`] to collect them.
pub fn lower_to_aadl(parse: &crate::Parse) -> ItemTree {
    let (tree, _diagnostics) = lower_to_aadl_with_diagnostics(parse);
    tree
}

/// Lower a parsed SysML v2 source file to an AADL [`ItemTree`], also
/// returning any diagnostics about constructs that could not be translated.
pub fn lower_to_aadl_with_diagnostics(parse: &crate::Parse) -> (ItemTree, Vec<LowerDiagnostic>) {
    let root = parse.syntax_node();
    let mut ctx = LowerCtx::default();
    lower_node(&root, &mut ctx);
    resolve_subcomponent_categories(&mut ctx.tree);
    let diagnostics = std::mem::take(&mut ctx.diagnostics);
    (ctx.tree, diagnostics)
}

/// Internal lowering context accumulating the AADL item tree.
#[derive(Default)]
struct LowerCtx {
    tree: ItemTree,
    diagnostics: Vec<LowerDiagnostic>,
}

/// Walk a SysML v2 CST node and lower children to AADL items.
fn lower_node(node: &SyntaxNode, ctx: &mut LowerCtx) {
    match node.kind() {
        SyntaxKind::SOURCE_FILE => {
            // Process all top-level children
            for child in node.children() {
                lower_node(&child, ctx);
            }
        }
        SyntaxKind::PACKAGE => {
            lower_package(node, ctx);
        }
        SyntaxKind::NAMESPACE_BODY => {
            for child in node.children() {
                lower_node(&child, ctx);
            }
        }
        SyntaxKind::PART_DEF => {
            lower_part_def(node, ctx);
        }
        SyntaxKind::PORT_DEF => {
            lower_port_def(node, ctx);
        }
        SyntaxKind::CONNECTION_DEF => {
            lower_connection_def(node, ctx);
        }
        SyntaxKind::CONNECTION_USAGE => {
            lower_connection_usage(node, ctx);
        }
        SyntaxKind::ACTION_DEF => {
            lower_action_def(node, ctx);
        }
        SyntaxKind::STATE_DEF => {
            lower_state_def(node, ctx);
        }
        SyntaxKind::ATTRIBUTE_DEF => {
            lower_attribute_def(node, ctx);
        }
        SyntaxKind::ITEM_DEF => {
            lower_item_def(node, ctx);
        }
        SyntaxKind::ENUM_DEF => {
            lower_enum_def(node, ctx);
        }
        SyntaxKind::INTERFACE_DEF => {
            lower_interface_def(node, ctx);
        }
        SyntaxKind::REQUIREMENT_DEF => {
            lower_requirement_def(node, ctx);
        }
        SyntaxKind::CONSTRAINT_DEF => {
            lower_constraint_def(node, ctx);
        }
        SyntaxKind::CALC_DEF => {
            lower_calc_def(node, ctx);
        }
        SyntaxKind::ALLOCATION_DEF => {
            lower_allocation_def(node, ctx);
        }
        other => {
            // Nodes that are internal structure (trivia, tokens, etc.) are
            // expected and should not produce diagnostics.
            if !is_ignorable_kind(other) {
                let offset = node.text_range().start().into();
                ctx.diagnostics.push(LowerDiagnostic {
                    offset,
                    msg: format!("unsupported SysML v2 construct: {other:?}"),
                });
            }
        }
    }
}

/// Returns `true` for `SyntaxKind`s that are expected to appear in the CST
/// but have no AADL equivalent and should not trigger a diagnostic.
fn is_ignorable_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        // Trivia / structural
        SyntaxKind::WHITESPACE
            | SyntaxKind::LINE_COMMENT
            | SyntaxKind::BLOCK_COMMENT
            | SyntaxKind::ERROR
            // Sub-node kinds (children of definitions/usages, not stand-alone)
            | SyntaxKind::NAME
            | SyntaxKind::QUALIFIED_NAME
            | SyntaxKind::TYPING
            | SyntaxKind::SPECIALIZATION
            | SyntaxKind::DIRECTION
            | SyntaxKind::MULTIPLICITY
            | SyntaxKind::CONNECT_ENDPOINT
            | SyntaxKind::FEATURE_CHAIN
            // Usages (lowered when encountered inside their parent)
            | SyntaxKind::IMPORT_DECL
            | SyntaxKind::PART_USAGE
            | SyntaxKind::PORT_USAGE
            | SyntaxKind::ATTRIBUTE_USAGE
            | SyntaxKind::ITEM_USAGE
            | SyntaxKind::ACTION_USAGE
            | SyntaxKind::STATE_USAGE
            | SyntaxKind::REF_USAGE
            | SyntaxKind::CONSTRAINT_USAGE
            | SyntaxKind::REQUIREMENT_USAGE
            | SyntaxKind::CONNECTION_USAGE
            // Requirement linkage
            | SyntaxKind::SATISFY_REQ
            | SyntaxKind::VERIFY_REQ
            | SyntaxKind::REFINE_REQ
            | SyntaxKind::ALLOCATE_REQ
            | SyntaxKind::DERIVE_REQ
            // Documentation
            | SyntaxKind::DOC_NODE
            | SyntaxKind::DOC_MEMBER
            | SyntaxKind::COMMENT_NODE
            // Declarations handled elsewhere
            | SyntaxKind::FEATURE_DECL
    )
}

/// Lower a SysML v2 `package` to an AADL package.
fn lower_package(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Collect with-clauses from import declarations
    let mut with_clauses = Vec::new();

    // Collect items from the namespace body
    let mut public_items = Vec::new();

    // Remember current type/impl/fgt counts so we can track what gets added
    let type_start = ctx.tree.component_types.len();
    let impl_start = ctx.tree.component_impls.len();
    let fgt_start = ctx.tree.feature_group_types.len();

    // Scan for import declarations before processing children
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            if child.kind() == SyntaxKind::IMPORT_DECL
                && let Some(import_name) = extract_import_package(&child)
            {
                with_clauses.push(import_name);
            }
        }
    }

    // Process children
    for child in node.children() {
        lower_node(&child, ctx);
    }

    // Collect newly added items as public refs
    for i in type_start..ctx.tree.component_types.len() {
        let idx = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(i as u32));
        public_items.push(ItemRef::ComponentType(idx));
    }
    for i in impl_start..ctx.tree.component_impls.len() {
        let idx = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(i as u32));
        public_items.push(ItemRef::ComponentImpl(idx));
    }
    for i in fgt_start..ctx.tree.feature_group_types.len() {
        let idx = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(i as u32));
        public_items.push(ItemRef::FeatureGroupType(idx));
    }

    ctx.tree.packages.alloc(Package {
        name,
        with_clauses,
        public_items,
        private_items: Vec::new(),
        renames: Vec::new(),
    });
}

/// Lower a SysML v2 `part def` to an AADL component type.
///
/// Heuristic for category selection:
/// - If the part def body contains `action` or `state` usages -> `process`
/// - If prefixed with `abstract` -> `abstract`
/// - Otherwise -> `system` (default for hardware-like or generic parts)
fn lower_part_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    let is_abstract = node_has_abstract_prefix(node);
    let category = if is_abstract {
        ComponentCategory::Abstract
    } else {
        infer_category(node)
    };

    // Check for specialization (extends)
    // NOTE: known limitation — specialization cycles (e.g., `A specializes A`)
    // are not detected here.  A full cycle check would require a post-lowering
    // graph walk across all ComponentTypeItems; for now the AADL backend will
    // emit the `extends` clause as-is and downstream validation should catch it.
    let extends = extract_specialization(node);

    // Guard against trivial self-specialization (A specializes A).
    let extends = extends.filter(|r| r.type_name.as_str() != name.as_str());

    // Collect features from port usages inside the part def body
    let mut features = Vec::new();
    let mut subcomponents = Vec::new();
    let mut connections = Vec::new();
    let mut property_associations = Vec::new();
    let mut modes = Vec::new();

    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            match child.kind() {
                SyntaxKind::PORT_USAGE => {
                    if let Some(feat) = lower_port_usage_to_feature(&child) {
                        let idx = ctx.tree.features.alloc(feat);
                        features.push(idx);
                    }
                }
                SyntaxKind::PART_USAGE => {
                    if let Some(sub) = lower_part_usage_to_subcomponent(&child) {
                        let idx = ctx.tree.subcomponents.alloc(sub);
                        subcomponents.push(idx);
                    }
                }
                SyntaxKind::REF_USAGE => {
                    // `ref part name : Type;` -> subcomponent
                    if let Some(sub) = lower_ref_usage_to_subcomponent(&child) {
                        let idx = ctx.tree.subcomponents.alloc(sub);
                        subcomponents.push(idx);
                    }
                }
                SyntaxKind::CONNECTION_USAGE => {
                    if let Some(conn) = lower_connection_node_to_item(&child) {
                        let idx = ctx.tree.connections.alloc(conn);
                        connections.push(idx);
                    }
                }
                SyntaxKind::ATTRIBUTE_USAGE => {
                    if let Some(pa) = lower_attribute_usage_to_property(&child) {
                        let idx = ctx.tree.property_associations.alloc(pa);
                        property_associations.push(idx);
                    }
                }
                SyntaxKind::STATE_USAGE => {
                    if let Some(mode) = lower_state_usage_to_mode(&child) {
                        let idx = ctx.tree.modes.alloc(mode);
                        modes.push(idx);
                    }
                }
                SyntaxKind::CONSTRAINT_USAGE => {
                    if let Some(pa) = lower_constraint_usage_to_property(&child) {
                        let idx = ctx.tree.property_associations.alloc(pa);
                        property_associations.push(idx);
                    }
                }
                SyntaxKind::ITEM_USAGE => {
                    // `out item` / `in item` inside a part def => feature
                    if let Some(feat) = lower_item_to_feature(&child) {
                        let idx = ctx.tree.features.alloc(feat);
                        features.push(idx);
                    }
                }
                other if !is_ignorable_kind(other) => {
                    let offset: usize = child.text_range().start().into();
                    ctx.diagnostics.push(LowerDiagnostic {
                        offset,
                        msg: format!("unsupported construct inside part def body: {other:?}"),
                    });
                }
                _ => {}
            }
        }
    }

    // If there are subcomponents or connections, create both a type and impl.
    // Otherwise, just a type.
    let type_idx = ctx.tree.component_types.alloc(ComponentTypeItem {
        name: name.clone(),
        category,
        is_public: true,
        extends: extends.clone(),
        features,
        flow_specs: Vec::new(),
        modes: modes.clone(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: property_associations.clone(),
        requires_modes: false,
    });

    if !subcomponents.is_empty() || !connections.is_empty() {
        ctx.tree.component_impls.alloc(ComponentImplItem {
            type_name: name.clone(),
            impl_name: Name::new("impl"),
            category,
            is_public: true,
            extends: None,
            subcomponents,
            connections,
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
    }

    let _ = type_idx;
}

/// Lower a SysML v2 `port def` to an AADL component type with features.
///
/// A port definition becomes a feature group type-like construct. We create
/// a data component type for it.
fn lower_port_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Scan the body for `in item` / `out item` declarations to create features
    let mut features = Vec::new();

    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            match child.kind() {
                SyntaxKind::ITEM_USAGE | SyntaxKind::FEATURE_DECL => {
                    if let Some(feat) = lower_item_to_feature(&child) {
                        let idx = ctx.tree.features.alloc(feat);
                        features.push(idx);
                    }
                }
                SyntaxKind::ATTRIBUTE_USAGE => {
                    // Attributes inside port def -> features
                    if let Some(feat) = lower_attribute_to_feature(&child) {
                        let idx = ctx.tree.features.alloc(feat);
                        features.push(idx);
                    }
                }
                other if !is_ignorable_kind(other) => {
                    let offset: usize = child.text_range().start().into();
                    ctx.diagnostics.push(LowerDiagnostic {
                        offset,
                        msg: format!("unsupported construct inside port def body: {other:?}"),
                    });
                }
                _ => {}
            }
        }
    }

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `connection def` to an AADL component type.
fn lower_connection_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Bus,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `action def` to an AADL subprogram type.
///
/// Action definitions represent behavioral computations, which map
/// to AADL subprogram types.
fn lower_action_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Subprogram,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `state def` to an AADL data type.
///
/// State definitions represent state machines. They are lowered to
/// a data type that carries the state information. State *usages*
/// inside a part def become AADL modes.
fn lower_state_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `attribute def` to an AADL data type.
///
/// Attribute definitions declare typed properties. They map to AADL
/// data component types (property carriers).
fn lower_attribute_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `item def` to an AADL data type.
///
/// Item definitions represent data carriers in SysML v2. They map
/// directly to AADL data component types.
fn lower_item_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `enum def` to an AADL data type with enumeration property.
///
/// Enum definitions map to AADL data types. The enumeration variants are
/// captured as a property association using `Data_Model::Enumerators`.
fn lower_enum_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Extract enumeration literals from the body
    let mut variants = Vec::new();
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            // Enum variants are parsed as FEATURE_DECL nodes (bare names with `;`)
            if let Some(variant_name) = extract_name(&child) {
                variants.push(variant_name);
            }
        }
    }

    // Create a property association for the enumerators if we found any
    let mut property_associations = Vec::new();
    if !variants.is_empty() {
        let variant_names: Vec<Name> = variants;
        let pa = PropertyAssociationItem {
            name: PropertyRef {
                property_set: Some(Name::new("Data_Model")),
                property_name: Name::new("Enumerators"),
            },
            value: variant_names
                .iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            typed_value: Some(PropertyExpr::List(
                variant_names
                    .iter()
                    .map(|n| PropertyExpr::StringLit(n.as_str().to_string()))
                    .collect(),
            )),
            is_append: false,
            applies_to: None,
            in_modes: Vec::new(),
        };
        let idx = ctx.tree.property_associations.alloc(pa);
        property_associations.push(idx);
    }

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations,
        requires_modes: false,
    });
}

/// Lower a SysML v2 `interface def` to an AADL feature group type.
///
/// Interface definitions declare bundles of ports and connections between
/// parts. They map to AADL feature group types.
fn lower_interface_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Scan body for `end` port declarations -> features
    let mut features = Vec::new();
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            // `end source : DataOut;` is parsed as FEATURE_DECL or PART_USAGE
            match child.kind() {
                SyntaxKind::FEATURE_DECL | SyntaxKind::PART_USAGE => {
                    if let Some(feat_name) = extract_name(&child) {
                        let classifier = extract_type_ref(&child);
                        let feat = Feature {
                            name: feat_name,
                            kind: FeatureKind::DataPort,
                            direction: Some(Direction::InOut),
                            access_kind: None,
                            classifier,
                            is_refined: false,
                            array_dimensions: Vec::new(),
                            property_associations: Vec::new(),
                        };
                        let idx = ctx.tree.features.alloc(feat);
                        features.push(idx);
                    }
                }
                other if !is_ignorable_kind(other) => {
                    let offset: usize = child.text_range().start().into();
                    ctx.diagnostics.push(LowerDiagnostic {
                        offset,
                        msg: format!("unsupported construct inside interface def body: {other:?}"),
                    });
                }
                _ => {}
            }
        }
    }

    ctx.tree.feature_group_types.alloc(FeatureGroupTypeItem {
        name,
        is_public: true,
        extends: None,
        inverse_of: None,
        features,
        prototypes: Vec::new(),
    });
}

/// Lower a SysML v2 `requirement def` to an AADL data type with annotation.
///
/// Requirements don't have a direct AADL equivalent. We lower them to
/// data types that serve as requirement carriers, with the requirement
/// text stored as a property annotation.
fn lower_requirement_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Extract doc text if present
    let mut property_associations = Vec::new();
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            if child.kind() == SyntaxKind::DOC_NODE || child.kind() == SyntaxKind::DOC_MEMBER {
                let doc_text = extract_doc_text(&child);
                if !doc_text.is_empty() {
                    let pa = PropertyAssociationItem {
                        name: PropertyRef {
                            property_set: Some(Name::new("AADL_Properties")),
                            property_name: Name::new("Description"),
                        },
                        value: doc_text.clone(),
                        typed_value: Some(PropertyExpr::StringLit(doc_text)),
                        is_append: false,
                        applies_to: None,
                        in_modes: Vec::new(),
                    };
                    let idx = ctx.tree.property_associations.alloc(pa);
                    property_associations.push(idx);
                }
            }
        }
    }

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Abstract,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations,
        requires_modes: false,
    });
}

/// Lower a SysML v2 `constraint def` to an AADL data type with properties.
///
/// Constraint definitions (especially timing constraints) map to AADL
/// data types carrying timing property annotations.
fn lower_constraint_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Collect attribute usages inside the constraint as properties
    let mut property_associations = Vec::new();
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            if child.kind() == SyntaxKind::ATTRIBUTE_USAGE
                && let Some(pa) = lower_attribute_usage_to_property(&child)
            {
                let idx = ctx.tree.property_associations.alloc(pa);
                property_associations.push(idx);
            }
        }
    }

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Data,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations,
        requires_modes: false,
    });
}

/// Lower a SysML v2 `calc def` to an AADL subprogram type.
///
/// Calc definitions represent computations (calculations), mapping
/// to AADL subprogram types.
fn lower_calc_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Subprogram,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `allocation def` to an AADL processor type.
///
/// Allocation definitions describe task-to-processor bindings.
/// They map to AADL processor types that carry binding information.
fn lower_allocation_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    ctx.tree.component_types.alloc(ComponentTypeItem {
        name,
        category: ComponentCategory::Processor,
        is_public: true,
        extends: None,
        features: Vec::new(),
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
        requires_modes: false,
    });
}

/// Lower a SysML v2 `connect a.p to b.p;` to an AADL connection item.
fn lower_connection_usage(node: &SyntaxNode, ctx: &mut LowerCtx) {
    if let Some(conn) = lower_connection_node_to_item(node) {
        ctx.tree.connections.alloc(conn);
    }
}

// ---------------------------------------------------------------------------
// Helper: lower port usage inside a part def to an AADL feature
// ---------------------------------------------------------------------------

/// Convert a `port name : PortDef;` or `in port name : PortDef;` to an AADL feature.
fn lower_port_usage_to_feature(node: &SyntaxNode) -> Option<Feature> {
    let name = extract_name(node)?;
    let direction = extract_direction(node);
    let classifier = extract_type_ref(node);

    Some(Feature {
        name,
        kind: FeatureKind::DataPort,
        direction: Some(direction.unwrap_or(Direction::InOut)),
        access_kind: None,
        classifier,
        is_refined: false,
        array_dimensions: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Convert an `item name : Type;` or `out item data : Type;` to an AADL feature.
fn lower_item_to_feature(node: &SyntaxNode) -> Option<Feature> {
    let name = extract_name(node)?;
    let direction = extract_direction(node);

    Some(Feature {
        name,
        kind: FeatureKind::DataPort,
        direction: Some(direction.unwrap_or(Direction::InOut)),
        access_kind: None,
        classifier: None,
        is_refined: false,
        array_dimensions: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Convert an `attribute name : Type;` inside a port def body to an AADL feature.
fn lower_attribute_to_feature(node: &SyntaxNode) -> Option<Feature> {
    let name = extract_name(node)?;
    let classifier = extract_type_ref(node);

    Some(Feature {
        name,
        kind: FeatureKind::DataPort,
        direction: Some(Direction::InOut),
        access_kind: None,
        classifier,
        is_refined: false,
        array_dimensions: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Convert a `part name : Type;` inside a part def body to an AADL subcomponent.
fn lower_part_usage_to_subcomponent(node: &SyntaxNode) -> Option<SubcomponentItem> {
    let name = extract_name(node)?;
    let classifier = extract_type_ref(node);
    let array_dimensions = extract_array_dimensions(node);

    Some(SubcomponentItem {
        name,
        category: ComponentCategory::System,
        classifier,
        is_refined: false,
        array_dimensions,
        in_modes: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Convert a `ref part name : Type;` to an AADL subcomponent.
fn lower_ref_usage_to_subcomponent(node: &SyntaxNode) -> Option<SubcomponentItem> {
    let name = extract_name(node)?;
    let classifier = extract_type_ref(node);

    Some(SubcomponentItem {
        name,
        category: ComponentCategory::System,
        classifier,
        is_refined: false,
        array_dimensions: Vec::new(),
        in_modes: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Convert an `attribute name : Type;` or `attribute name : Type = 10 ms;`
/// usage to a property association.
fn lower_attribute_usage_to_property(node: &SyntaxNode) -> Option<PropertyAssociationItem> {
    let name = extract_name(node)?;
    let type_ref = extract_type_ref(node);

    // First, try to extract an explicit default value (after `=`).
    let (value_str, typed_value) = if let Some((v_str, v_expr)) = extract_default_value(node) {
        (v_str, Some(v_expr))
    } else {
        // Fallback: use the type reference text as an opaque value.
        let vs = type_ref
            .as_ref()
            .map(|c| c.type_name.as_str().to_string())
            .unwrap_or_default();
        let tv = if vs.is_empty() {
            None
        } else {
            Some(PropertyExpr::Opaque(vs.clone()))
        };
        (vs, tv)
    };

    Some(PropertyAssociationItem {
        name: PropertyRef {
            property_set: None,
            property_name: name,
        },
        value: value_str,
        typed_value,
        is_append: false,
        applies_to: None,
        in_modes: Vec::new(),
    })
}

/// Convert a `state name : StateDef;` usage to an AADL mode.
fn lower_state_usage_to_mode(node: &SyntaxNode) -> Option<spar_hir_def::item_tree::ModeItem> {
    let name = extract_name(node)?;
    Some(spar_hir_def::item_tree::ModeItem {
        name,
        is_initial: false,
    })
}

/// Convert a `constraint name : ConstraintDef;` or one with an explicit
/// value (e.g. `constraint period = 10 ms;`) to a property association.
fn lower_constraint_usage_to_property(node: &SyntaxNode) -> Option<PropertyAssociationItem> {
    let name = extract_name(node)?;
    let type_ref = extract_type_ref(node);

    // First, try to extract an explicit default value (after `=`).
    let (value_str, typed_value) = if let Some((v_str, v_expr)) = extract_default_value(node) {
        (v_str, Some(v_expr))
    } else {
        // Fallback: use the type reference text as an opaque value.
        let vs = type_ref
            .as_ref()
            .map(|c| c.type_name.as_str().to_string())
            .unwrap_or_default();
        let tv = if vs.is_empty() {
            None
        } else {
            Some(PropertyExpr::Opaque(vs.clone()))
        };
        (vs, tv)
    };

    Some(PropertyAssociationItem {
        name: PropertyRef {
            property_set: Some(Name::new("Timing_Properties")),
            property_name: name,
        },
        value: value_str,
        typed_value,
        is_append: false,
        applies_to: None,
        in_modes: Vec::new(),
    })
}

/// Convert a `connect a.p to b.p;` to an AADL connection item.
fn lower_connection_node_to_item(node: &SyntaxNode) -> Option<ConnectionItem> {
    let endpoints: Vec<_> = node
        .children()
        .filter(|c| c.kind() == SyntaxKind::CONNECT_ENDPOINT)
        .collect();

    let src = endpoints.first().map(extract_connected_element);
    let dst = endpoints.get(1).map(extract_connected_element);

    // Generate a connection name from the endpoints
    let conn_name = match (&src, &dst) {
        (Some(Some(s)), Some(Some(d))) => {
            let src_name = s
                .subcomponent
                .as_ref()
                .map(|n| n.as_str())
                .unwrap_or(s.feature.as_str());
            let dst_name = d
                .subcomponent
                .as_ref()
                .map(|n| n.as_str())
                .unwrap_or(d.feature.as_str());
            Name::new(&format!("{src_name}_to_{dst_name}"))
        }
        _ => Name::new("conn"),
    };

    Some(ConnectionItem {
        name: conn_name,
        kind: ConnectionKind::Port,
        is_bidirectional: false,
        is_refined: false,
        src: src.flatten(),
        dst: dst.flatten(),
        in_modes: Vec::new(),
        property_associations: Vec::new(),
    })
}

/// Extract a connected element reference from a CONNECT_ENDPOINT node.
///
/// A CONNECT_ENDPOINT contains a FEATURE_CHAIN like `a.b` where `a` is the
/// subcomponent and `b` is the feature, or just `a` for a local feature.
fn extract_connected_element(node: &SyntaxNode) -> Option<ConnectedElementRef> {
    let chain = find_child(node, SyntaxKind::FEATURE_CHAIN)?;
    let idents: Vec<String> = chain
        .children_with_tokens()
        .filter_map(|elem| {
            let tok = elem.into_token()?;
            if tok.kind() == SyntaxKind::IDENT || tok.kind().is_keyword() {
                Some(tok.text().to_string())
            } else {
                None
            }
        })
        .collect();

    match idents.len() {
        0 => None,
        1 => Some(ConnectedElementRef {
            subcomponent: None,
            feature: Name::new(&idents[0]),
        }),
        _ => Some(ConnectedElementRef {
            subcomponent: Some(Name::new(&idents[0])),
            feature: Name::new(&idents[idents.len() - 1]),
        }),
    }
}

// ---------------------------------------------------------------------------
// Heuristics
// ---------------------------------------------------------------------------

/// Infer the AADL component category from a SysML v2 `part def`.
///
/// - Contains `action` or `state` usages -> `Process`
/// - Otherwise -> `System`
fn infer_category(node: &SyntaxNode) -> ComponentCategory {
    if let Some(body) = find_child(node, SyntaxKind::NAMESPACE_BODY) {
        for child in body.children() {
            match child.kind() {
                SyntaxKind::ACTION_USAGE | SyntaxKind::ACTION_DEF => {
                    return ComponentCategory::Process;
                }
                SyntaxKind::STATE_USAGE | SyntaxKind::STATE_DEF => {
                    return ComponentCategory::Process;
                }
                _ => {}
            }
        }
    }
    ComponentCategory::System
}

/// Second pass: walk every component implementation and propagate the
/// category from the referenced component type to its subcomponents.
///
/// During the first pass, `lower_part_usage_to_subcomponent` and
/// `lower_ref_usage_to_subcomponent` default the category to `System`
/// because the type may not have been lowered yet.  This function
/// fixes that by looking up each subcomponent's classifier in the
/// already-lowered component types.
fn resolve_subcomponent_categories(tree: &mut ItemTree) {
    // Build a map: type_name -> category from all known component types.
    let type_categories: std::collections::HashMap<String, ComponentCategory> = tree
        .component_types
        .iter()
        .map(|(_, ct)| (ct.name.as_str().to_string(), ct.category))
        .collect();

    // Walk every subcomponent and propagate the category.
    for (_idx, sub) in tree.subcomponents.iter_mut() {
        if let Some(cls) = &sub.classifier {
            let type_name = cls.type_name.as_str();
            if let Some(&cat) = type_categories.get(type_name) {
                sub.category = cat;
            }
        }
    }
}

/// Check whether a node was prefixed with `abstract` keyword.
///
/// The `abstract` keyword is consumed during parsing and appears as an
/// ABSTRACT_KW token child of the definition node.
fn node_has_abstract_prefix(node: &SyntaxNode) -> bool {
    node.children_with_tokens().any(|elem| {
        elem.as_token()
            .is_some_and(|t| t.kind() == SyntaxKind::ABSTRACT_KW)
    })
}

// ---------------------------------------------------------------------------
// CST extraction helpers
// ---------------------------------------------------------------------------

/// Extract a default value expression from a node that may contain `= <expr>`.
///
/// Recognises the following patterns produced by the grammar:
///   - `= <integer>`        → `PropertyExpr::Integer(n, None)`
///   - `= <integer> <unit>` → `PropertyExpr::Integer(n, Some(unit))`
///   - `= <real>`           → `PropertyExpr::Real(s, None)`
///   - `= <real> <unit>`    → `PropertyExpr::Real(s, Some(unit))`
///   - `= <string>`         → `PropertyExpr::StringLit(s)`
///   - `= <ident>`          → `PropertyExpr::Opaque(s)` (fallback)
///
/// Returns `(display_string, PropertyExpr)` or `None` if no `=` is found.
fn extract_default_value(node: &SyntaxNode) -> Option<(String, PropertyExpr)> {
    let mut tokens = node.children_with_tokens().peekable();

    // Advance past `=`
    let mut found_eq = false;
    for elem in tokens.by_ref() {
        if let Some(tok) = elem.as_token()
            && tok.kind() == SyntaxKind::EQ
        {
            found_eq = true;
            break;
        }
    }
    if !found_eq {
        return None;
    }

    // Skip whitespace after `=`
    while tokens
        .peek()
        .and_then(|e| e.as_token())
        .is_some_and(|t| t.kind() == SyntaxKind::WHITESPACE)
    {
        tokens.next();
    }

    // Read the value token
    let val_tok = tokens.next()?;
    let val_token = val_tok.as_token()?;
    let val_kind = val_token.kind();
    let val_text = val_token.text().to_string();

    match val_kind {
        SyntaxKind::INTEGER_LIT => {
            let n: i64 = val_text.parse().ok()?;
            // Check for optional unit identifier
            let unit = extract_following_unit(&mut tokens);
            let display = match &unit {
                Some(u) => format!("{n} {u}"),
                None => val_text,
            };
            Some((
                display,
                PropertyExpr::Integer(n, unit.as_deref().map(Name::new)),
            ))
        }
        SyntaxKind::REAL_LIT => {
            let unit = extract_following_unit(&mut tokens);
            let display = match &unit {
                Some(u) => format!("{val_text} {u}"),
                None => val_text.clone(),
            };
            Some((
                display,
                PropertyExpr::Real(val_text, unit.as_deref().map(Name::new)),
            ))
        }
        SyntaxKind::STRING_LIT => {
            let unquoted = val_text
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string();
            let display = unquoted.clone();
            Some((display, PropertyExpr::StringLit(unquoted)))
        }
        SyntaxKind::TRUE_KW => Some(("true".to_string(), PropertyExpr::Boolean(true))),
        SyntaxKind::FALSE_KW => Some(("false".to_string(), PropertyExpr::Boolean(false))),
        SyntaxKind::IDENT => Some((val_text.clone(), PropertyExpr::Opaque(val_text))),
        _ => None,
    }
}

/// After a numeric literal token, skip whitespace and check if the next
/// token is an identifier (unit name).  Returns the unit string if found.
fn extract_following_unit<I>(tokens: &mut std::iter::Peekable<I>) -> Option<String>
where
    I: Iterator<Item = rowan::NodeOrToken<SyntaxNode, crate::SyntaxToken>>,
{
    // Skip whitespace
    while tokens
        .peek()
        .and_then(|e| e.as_token())
        .is_some_and(|t| t.kind() == SyntaxKind::WHITESPACE)
    {
        tokens.next();
    }
    // Check for IDENT or keyword used as unit (ms, us, ns, s, etc.)
    if let Some(elem) = tokens.peek()
        && let Some(tok) = elem.as_token()
        && (tok.kind() == SyntaxKind::IDENT || tok.kind().is_keyword())
    {
        let unit_text = tok.text().to_string();
        if !unit_text.is_empty() {
            tokens.next(); // consume the unit token
            return Some(unit_text);
        }
    }
    None
}

/// Extract the first NAME child's text from a node.
fn extract_name(node: &SyntaxNode) -> Option<Name> {
    for child in node.children() {
        if child.kind() == SyntaxKind::NAME {
            let text = child.text().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(Name::new(trimmed));
            }
        }
    }
    // Fallback: look for QUALIFIED_NAME
    for child in node.children() {
        if child.kind() == SyntaxKind::QUALIFIED_NAME {
            let text = child.text().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                // Take only the last segment for the name
                let segments: Vec<&str> = trimmed.split("::").collect();
                return Some(Name::new(segments.last().unwrap_or(&trimmed)));
            }
        }
    }
    None
}

/// Extract a direction from a node that may have a DIRECTION child.
fn extract_direction(node: &SyntaxNode) -> Option<Direction> {
    for child in node.children() {
        if child.kind() == SyntaxKind::DIRECTION {
            let text = child.text().to_string();
            return match text.trim() {
                "in" => Some(Direction::In),
                "out" => Some(Direction::Out),
                "inout" => Some(Direction::InOut),
                _ => None,
            };
        }
    }
    None
}

/// Extract a type reference from a TYPING node child.
fn extract_type_ref(node: &SyntaxNode) -> Option<spar_hir_def::name::ClassifierRef> {
    for child in node.children() {
        if child.kind() == SyntaxKind::TYPING {
            // TYPING contains a QUALIFIED_NAME
            for sub in child.children() {
                if sub.kind() == SyntaxKind::QUALIFIED_NAME {
                    let text = sub.text().to_string();
                    let trimmed = text.trim();
                    let segments: Vec<&str> = trimmed.split("::").collect();
                    return match segments.len() {
                        1 => Some(spar_hir_def::name::ClassifierRef {
                            package: None,
                            type_name: Name::new(segments[0]),
                            impl_name: None,
                        }),
                        _ => Some(spar_hir_def::name::ClassifierRef {
                            package: Some(Name::new(segments[0])),
                            type_name: Name::new(segments[segments.len() - 1]),
                            impl_name: None,
                        }),
                    };
                }
            }
        }
    }
    None
}

/// Extract a specialization reference from a SPECIALIZATION node child.
///
/// This handles `specializes SuperType`, `:> SuperType`, and `:>> SuperType`.
fn extract_specialization(node: &SyntaxNode) -> Option<spar_hir_def::name::ClassifierRef> {
    for child in node.children() {
        if child.kind() == SyntaxKind::SPECIALIZATION {
            for sub in child.children() {
                if sub.kind() == SyntaxKind::QUALIFIED_NAME {
                    let text = sub.text().to_string();
                    let trimmed = text.trim();
                    let segments: Vec<&str> = trimmed.split("::").collect();
                    return match segments.len() {
                        1 => Some(spar_hir_def::name::ClassifierRef {
                            package: None,
                            type_name: Name::new(segments[0]),
                            impl_name: None,
                        }),
                        _ => Some(spar_hir_def::name::ClassifierRef {
                            package: Some(Name::new(segments[0])),
                            type_name: Name::new(segments[segments.len() - 1]),
                            impl_name: None,
                        }),
                    };
                }
            }
        }
    }
    None
}

/// Extract array dimensions from a MULTIPLICITY node child.
///
/// Maps `[4]` to a single `ArrayDimension { size: Some(Literal(4)) }`.
fn extract_array_dimensions(node: &SyntaxNode) -> Vec<spar_hir_def::item_tree::ArrayDimension> {
    for child in node.children() {
        if child.kind() == SyntaxKind::MULTIPLICITY {
            // Look for integer literal tokens inside multiplicity
            for tok in child.children_with_tokens() {
                if let Some(token) = tok.as_token()
                    && token.kind() == SyntaxKind::INTEGER_LIT
                    && let Ok(n) = token.text().parse::<u64>()
                {
                    return vec![spar_hir_def::item_tree::ArrayDimension {
                        size: Some(spar_hir_def::item_tree::ArraySize::Literal(n)),
                    }];
                }
            }
            // If we found a multiplicity but no parseable integer, return unspecified
            return vec![spar_hir_def::item_tree::ArrayDimension { size: None }];
        }
    }
    Vec::new()
}

/// Extract the package name from an import declaration.
///
/// For `import Pkg::*;` returns `Pkg`.
/// For `import Pkg::Sub::Name;` returns `Pkg`.
fn extract_import_package(node: &SyntaxNode) -> Option<Name> {
    for child in node.children() {
        if child.kind() == SyntaxKind::QUALIFIED_NAME {
            let text = child.text().to_string();
            let trimmed = text.trim();
            let segments: Vec<&str> = trimmed.split("::").collect();
            if !segments.is_empty() {
                // Return the first segment (the package name)
                let pkg_name = segments[0].trim();
                if pkg_name != "*" && !pkg_name.is_empty() {
                    return Some(Name::new(pkg_name));
                }
            }
        }
    }
    None
}

/// Extract documentation text from a DOC_NODE or DOC_MEMBER.
fn extract_doc_text(node: &SyntaxNode) -> String {
    for tok in node.children_with_tokens() {
        if let Some(token) = tok.as_token()
            && token.kind() == SyntaxKind::STRING_LIT
        {
            let text = token.text().to_string();
            // Strip surrounding quotes
            return text
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string();
        }
    }
    String::new()
}

/// Find the first child node with a specific kind.
fn find_child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

// ---------------------------------------------------------------------------
// AADL text generation
// ---------------------------------------------------------------------------

/// Render an AADL [`ItemTree`] as human-readable AADL text.
///
/// This is a best-effort pretty-printer for the lowered model. It produces
/// syntactically valid AADL that can be round-tripped through `spar parse`.
pub fn item_tree_to_aadl(tree: &ItemTree) -> String {
    let mut out = String::new();

    for (_idx, pkg) in tree.packages.iter() {
        out.push_str(&format!("package {}\npublic\n", pkg.name));

        // Emit with clauses
        for wc in &pkg.with_clauses {
            out.push_str(&format!("  with {};\n", wc));
        }

        for item_ref in &pkg.public_items {
            match item_ref {
                ItemRef::ComponentType(idx) => {
                    let ct = &tree.component_types[*idx];
                    if let Some(ext) = &ct.extends {
                        out.push_str(&format!("  {} {} extends {}\n", ct.category, ct.name, ext));
                    } else {
                        out.push_str(&format!("  {} {}\n", ct.category, ct.name));
                    }
                    if !ct.features.is_empty() {
                        out.push_str("  features\n");
                        for feat_idx in &ct.features {
                            let feat = &tree.features[*feat_idx];
                            let dir = feat.direction.map(|d| format!("{d} ")).unwrap_or_default();
                            let cls = feat
                                .classifier
                                .as_ref()
                                .map(|c| format!(" {}", c.type_name))
                                .unwrap_or_default();
                            out.push_str(&format!(
                                "    {} : {}{}{};",
                                feat.name, dir, feat.kind, cls
                            ));
                            out.push('\n');
                        }
                    }
                    if !ct.modes.is_empty() {
                        out.push_str("  modes\n");
                        for mode_idx in &ct.modes {
                            let mode = &tree.modes[*mode_idx];
                            let initial = if mode.is_initial { " initial" } else { "" };
                            out.push_str(&format!("    {} :{} mode;", mode.name, initial));
                            out.push('\n');
                        }
                    }
                    if !ct.property_associations.is_empty() {
                        out.push_str("  properties\n");
                        for pa_idx in &ct.property_associations {
                            let pa = &tree.property_associations[*pa_idx];
                            out.push_str(&format!("    {} => {};", pa.name, pa.value));
                            out.push('\n');
                        }
                    }
                    out.push_str(&format!("  end {};\n\n", ct.name));
                }
                ItemRef::ComponentImpl(idx) => {
                    let ci = &tree.component_impls[*idx];
                    out.push_str(&format!(
                        "  {} implementation {}.{}\n",
                        ci.category, ci.type_name, ci.impl_name
                    ));
                    if !ci.subcomponents.is_empty() {
                        out.push_str("  subcomponents\n");
                        for sub_idx in &ci.subcomponents {
                            let sub = &tree.subcomponents[*sub_idx];
                            let cls = sub.classifier.as_ref().map(|c| {
                                if let Some(pkg) = &c.package {
                                    format!("{}::{}", pkg, c.type_name)
                                } else {
                                    c.type_name.to_string()
                                }
                            });
                            if let Some(cls) = &cls {
                                out.push_str(&format!(
                                    "    {} : {} {};",
                                    sub.name, sub.category, cls
                                ));
                            } else {
                                out.push_str(&format!("    {} : {};", sub.name, sub.category));
                            }
                            out.push('\n');
                        }
                    }
                    if !ci.connections.is_empty() {
                        out.push_str("  connections\n");
                        for conn_idx in &ci.connections {
                            let conn = &tree.connections[*conn_idx];
                            let arrow = if conn.is_bidirectional { "<->" } else { "->" };
                            let src = conn
                                .src
                                .as_ref()
                                .map(format_endpoint)
                                .unwrap_or_else(|| "?".to_string());
                            let dst = conn
                                .dst
                                .as_ref()
                                .map(format_endpoint)
                                .unwrap_or_else(|| "?".to_string());
                            out.push_str(&format!(
                                "    {} : port {} {arrow} {};",
                                conn.name, src, dst
                            ));
                            out.push('\n');
                        }
                    }
                    out.push_str(&format!("  end {}.{};\n\n", ci.type_name, ci.impl_name));
                }
                ItemRef::FeatureGroupType(idx) => {
                    let fgt = &tree.feature_group_types[*idx];
                    out.push_str(&format!("  feature group {}\n", fgt.name));
                    if !fgt.features.is_empty() {
                        out.push_str("  features\n");
                        for feat_idx in &fgt.features {
                            let feat = &tree.features[*feat_idx];
                            let dir = feat.direction.map(|d| format!("{d} ")).unwrap_or_default();
                            let cls = feat
                                .classifier
                                .as_ref()
                                .map(|c| format!(" {}", c.type_name))
                                .unwrap_or_default();
                            out.push_str(&format!(
                                "    {} : {}{}{};",
                                feat.name, dir, feat.kind, cls
                            ));
                            out.push('\n');
                        }
                    }
                    out.push_str(&format!("  end {};\n\n", fgt.name));
                }
                _ => {}
            }
        }

        out.push_str(&format!("end {};\n", pkg.name));
    }

    // Also emit component types not inside packages
    for (_idx, ct) in tree.component_types.iter() {
        // Skip if already emitted as part of a package
        let in_package = tree.packages.iter().any(|(_, pkg)| {
            pkg.public_items
                .iter()
                .any(|item| matches!(item, ItemRef::ComponentType(i) if *i == _idx))
        });
        if in_package {
            continue;
        }
        out.push_str(&format!("-- standalone {} {}\n", ct.category, ct.name));
    }

    out
}

fn format_endpoint(ep: &ConnectedElementRef) -> String {
    if let Some(sub) = &ep.subcomponent {
        format!("{}.{}", sub, ep.feature)
    } else {
        ep.feature.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_def_lowers_to_system_type() {
        let parse = crate::parse("part def Vehicle { }");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "Vehicle");
        assert_eq!(ct.category, ComponentCategory::System);
    }

    #[test]
    fn port_def_lowers_to_feature() {
        let parse = crate::parse("port def SensorPort { out item data; }");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "SensorPort");
        assert_eq!(ct.category, ComponentCategory::Data);
        assert_eq!(ct.features.len(), 1);
        let feat = &tree.features[ct.features[0]];
        assert_eq!(feat.name.as_str(), "data");
        assert_eq!(feat.direction, Some(Direction::Out));
        assert_eq!(feat.kind, FeatureKind::DataPort);
    }

    #[test]
    fn connection_lowers_to_connection() {
        let parse = crate::parse("connect a.p to b.p;");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.connections.len(), 1);
        let conn = tree.connections.iter().next().unwrap().1;
        assert!(conn.src.is_some());
        assert!(conn.dst.is_some());
        let src = conn.src.as_ref().unwrap();
        assert_eq!(src.subcomponent.as_ref().unwrap().as_str(), "a");
        assert_eq!(src.feature.as_str(), "p");
        let dst = conn.dst.as_ref().unwrap();
        assert_eq!(dst.subcomponent.as_ref().unwrap().as_str(), "b");
        assert_eq!(dst.feature.as_str(), "p");
    }

    #[test]
    fn vehicle_example_lowers() {
        let source = r#"
package VehicleSystem {
    port def SensorPort {
        out item data;
    }

    port def ProcessorPort {
        in item data;
    }

    part def Sensor {
        port sensorOut : SensorPort;
    }

    part def Processor {
        port processorIn : ProcessorPort;
    }

    part def Vehicle {
        part sensor : Sensor;
        part processor : Processor;
        connect sensor.sensorOut to processor.processorIn;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // Should have component types: SensorPort, ProcessorPort, Sensor, Processor, Vehicle
        assert!(
            tree.component_types.len() >= 5,
            "expected >= 5 component types, got {}",
            tree.component_types.len()
        );

        // Vehicle should have an implementation with subcomponents
        assert!(
            !tree.component_impls.is_empty(),
            "expected at least one component implementation"
        );

        // Check Vehicle impl has 2 subcomponents and 1 connection
        let vehicle_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Vehicle");
        assert!(vehicle_impl.is_some(), "expected Vehicle implementation");
        let (_, vi) = vehicle_impl.unwrap();
        assert_eq!(
            vi.subcomponents.len(),
            2,
            "Vehicle should have 2 subcomponents"
        );
        assert_eq!(vi.connections.len(), 1, "Vehicle should have 1 connection");

        // Verify AADL text generation
        let aadl = item_tree_to_aadl(&tree);
        assert!(
            aadl.contains("system Vehicle"),
            "expected system type Vehicle in output"
        );
        assert!(
            aadl.contains("system Sensor"),
            "expected system type Sensor in output"
        );
    }

    #[test]
    fn part_def_with_action_lowers_to_process() {
        let source = r#"
part def Controller {
    action processData { }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "Controller");
        assert_eq!(ct.category, ComponentCategory::Process);
    }

    #[test]
    fn connection_def_lowers_to_bus() {
        let parse = crate::parse("connection def SensorLink { }");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "SensorLink");
        assert_eq!(ct.category, ComponentCategory::Bus);
    }

    // -----------------------------------------------------------------------
    // Validation model tests
    // -----------------------------------------------------------------------

    #[test]
    fn val_01_nested_packages() {
        let source = r#"
package OuterPkg {
    package InnerPkg {
        part def Sensor;
    }
    part def Controller;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // Should have 2 packages: OuterPkg and InnerPkg
        assert_eq!(tree.packages.len(), 2, "expected 2 packages");
        let inner = tree
            .packages
            .iter()
            .find(|(_, p)| p.name.as_str() == "InnerPkg");
        assert!(inner.is_some(), "expected InnerPkg");

        // Sensor should be inside InnerPkg, Controller inside OuterPkg
        let (_, inner_pkg) = inner.unwrap();
        assert_eq!(
            inner_pkg.public_items.len(),
            1,
            "InnerPkg should have 1 item (Sensor)"
        );
    }

    #[test]
    fn val_02_nested_parts() {
        let source = r#"
package PartsValidation {
    part def Engine {
        part cylinder : Cylinder;
    }
    part def Cylinder;
    part def Vehicle {
        part eng : Engine;
        part body : Body;
    }
    part def Body;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // Engine and Vehicle should have implementations
        let engine_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Engine");
        assert!(engine_impl.is_some(), "expected Engine impl");
        let (_, ei) = engine_impl.unwrap();
        assert_eq!(
            ei.subcomponents.len(),
            1,
            "Engine should have 1 subcomponent"
        );

        let vehicle_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Vehicle");
        assert!(vehicle_impl.is_some(), "expected Vehicle impl");
        let (_, vi) = vehicle_impl.unwrap();
        assert_eq!(
            vi.subcomponents.len(),
            2,
            "Vehicle should have 2 subcomponents"
        );
    }

    #[test]
    fn val_03_ports_with_direction() {
        let source = r#"
package PortsValidation {
    port def SensorPort {
        out item data;
    }
    port def ActuatorPort {
        in item command;
    }
    part def Sensor {
        out port sOut : SensorPort;
    }
    part def Actuator {
        in port aIn : ActuatorPort;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor");
        assert!(sensor.is_some(), "expected Sensor type");
        let (_, sensor_type) = sensor.unwrap();
        assert_eq!(sensor_type.features.len(), 1);
        let feat = &tree.features[sensor_type.features[0]];
        assert_eq!(feat.name.as_str(), "sOut");
        assert_eq!(feat.direction, Some(Direction::Out));

        let actuator = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Actuator");
        assert!(actuator.is_some(), "expected Actuator type");
        let (_, actuator_type) = actuator.unwrap();
        let afeat = &tree.features[actuator_type.features[0]];
        assert_eq!(afeat.name.as_str(), "aIn");
        assert_eq!(afeat.direction, Some(Direction::In));
    }

    #[test]
    fn val_04_connections() {
        let source = r#"
package ConnectionsValidation {
    port def OutPort { out item data; }
    port def InPort { in item data; }
    part def Producer { out port pOut : OutPort; }
    part def Consumer { in port cIn : InPort; }
    part def System {
        part producer : Producer;
        part consumer : Consumer;
        connect producer.pOut to consumer.cIn;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let sys_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "System");
        assert!(sys_impl.is_some(), "expected System impl");
        let (_, si) = sys_impl.unwrap();
        assert_eq!(si.subcomponents.len(), 2);
        assert_eq!(si.connections.len(), 1);

        let conn = &tree.connections[si.connections[0]];
        let src = conn.src.as_ref().unwrap();
        assert_eq!(src.subcomponent.as_ref().unwrap().as_str(), "producer");
        assert_eq!(src.feature.as_str(), "pOut");
    }

    #[test]
    fn val_05_actions() {
        let source = r#"
package ActionsValidation {
    action def ProcessData;
    action def ReadSensor;
    part def Controller {
        action process : ProcessData;
        action read : ReadSensor;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // action defs -> subprogram types
        let process_data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "ProcessData");
        assert!(process_data.is_some(), "expected ProcessData type");
        assert_eq!(
            process_data.unwrap().1.category,
            ComponentCategory::Subprogram
        );

        // Controller with actions -> process category
        let controller = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Controller");
        assert!(controller.is_some(), "expected Controller type");
        assert_eq!(controller.unwrap().1.category, ComponentCategory::Process);
    }

    #[test]
    fn val_06_states() {
        let source = r#"
package StatesValidation {
    state def Operational;
    state def Degraded;
    part def Controller {
        state operational : Operational;
        state degraded : Degraded;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // state defs -> data types
        let operational = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Operational");
        assert!(operational.is_some(), "expected Operational type");
        assert_eq!(operational.unwrap().1.category, ComponentCategory::Data);

        // Controller with states -> process, and has modes
        let controller = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Controller");
        assert!(controller.is_some(), "expected Controller type");
        let (_, ct) = controller.unwrap();
        assert_eq!(ct.category, ComponentCategory::Process);
        assert_eq!(ct.modes.len(), 2, "expected 2 modes from state usages");
    }

    #[test]
    fn val_07_requirements() {
        let source = r#"
package RequirementsValidation {
    requirement def LatencyReq {
        doc "System shall respond within 100ms";
        subject s : Controller;
    }
    part def Controller;
    requirement sysLatency : LatencyReq;
    satisfy sysLatency by Controller;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let latency_req = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "LatencyReq");
        assert!(latency_req.is_some(), "expected LatencyReq type");
        let (_, ct) = latency_req.unwrap();
        assert_eq!(ct.category, ComponentCategory::Abstract);

        // Should have a doc annotation property
        assert!(
            !ct.property_associations.is_empty(),
            "expected property associations from doc"
        );
    }

    #[test]
    fn val_08_constraints() {
        let source = r#"
package ConstraintsValidation {
    constraint def TimingConstraint {
        attribute maxLatency : Real;
    }
    part def Controller {
        constraint timing : TimingConstraint;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let timing = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "TimingConstraint");
        assert!(timing.is_some(), "expected TimingConstraint type");
        let (_, ct) = timing.unwrap();
        assert_eq!(ct.category, ComponentCategory::Data);
        // Should have the attribute as a property
        assert!(
            !ct.property_associations.is_empty(),
            "expected property associations from attributes"
        );

        // Controller should have a timing property
        let controller = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Controller");
        assert!(controller.is_some(), "expected Controller type");
        let (_, ctrl_ct) = controller.unwrap();
        assert!(
            !ctrl_ct.property_associations.is_empty(),
            "expected property associations from constraint usage"
        );
    }

    #[test]
    fn val_09_interfaces() {
        let source = r#"
package InterfacesValidation {
    port def DataOut { out item payload; }
    port def DataIn { in item payload; }
    interface def DataLink {
        end source : DataOut;
        end sink : DataIn;
    }
    part def Sender { port txPort : DataOut; }
    part def Receiver { port rxPort : DataIn; }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // interface def -> feature group type
        assert_eq!(
            tree.feature_group_types.len(),
            1,
            "expected 1 feature group type"
        );
        let fgt = tree.feature_group_types.iter().next().unwrap().1;
        assert_eq!(fgt.name.as_str(), "DataLink");
        assert_eq!(fgt.features.len(), 2, "expected 2 features (source, sink)");
    }

    #[test]
    fn val_10_attributes() {
        let source = r#"
package AttributesValidation {
    attribute def Mass;
    attribute def Voltage;
    part def Sensor {
        attribute mass : Mass;
        attribute voltage : Voltage;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // attribute defs -> data types
        let mass = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Mass");
        assert!(mass.is_some(), "expected Mass type");
        assert_eq!(mass.unwrap().1.category, ComponentCategory::Data);

        // Sensor should have property associations for its attributes
        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor");
        assert!(sensor.is_some(), "expected Sensor type");
        let (_, sensor_ct) = sensor.unwrap();
        assert_eq!(
            sensor_ct.property_associations.len(),
            2,
            "expected 2 property associations from attributes"
        );
    }

    #[test]
    fn val_11_items() {
        let source = r#"
package ItemsValidation {
    item def SensorData;
    item def CommandData;
    part def Sensor {
        out item reading : SensorData;
    }
    part def Actuator {
        in item command : CommandData;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // item defs -> data types
        let sensor_data = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "SensorData");
        assert!(sensor_data.is_some(), "expected SensorData type");
        assert_eq!(sensor_data.unwrap().1.category, ComponentCategory::Data);

        // Sensor should have an out item as a feature
        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor");
        assert!(sensor.is_some(), "expected Sensor type");
        let (_, sensor_ct) = sensor.unwrap();
        assert_eq!(sensor_ct.features.len(), 1);
        let feat = &tree.features[sensor_ct.features[0]];
        assert_eq!(feat.name.as_str(), "reading");
        assert_eq!(feat.direction, Some(Direction::Out));
    }

    #[test]
    fn val_12_enums() {
        let source = r#"
package EnumsValidation {
    enum def Color {
        Red;
        Green;
        Blue;
    }
    enum def Status {
        Active;
        Inactive;
        Fault;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // enum defs -> data types with enumeration properties
        let color = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Color");
        assert!(color.is_some(), "expected Color type");
        let (_, color_ct) = color.unwrap();
        assert_eq!(color_ct.category, ComponentCategory::Data);
        assert!(
            !color_ct.property_associations.is_empty(),
            "expected enumerator property association"
        );

        // Check the property contains the variant names
        let pa = &tree.property_associations[color_ct.property_associations[0]];
        assert!(pa.value.contains("Red"), "expected Red in enumerators");
        assert!(pa.value.contains("Green"), "expected Green in enumerators");
        assert!(pa.value.contains("Blue"), "expected Blue in enumerators");
    }

    #[test]
    fn val_13_allocations() {
        let source = r#"
package AllocationsValidation {
    allocation def TaskAllocation;
    part def Controller {
        action process;
    }
    part def ECU;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // allocation def -> processor type
        let alloc = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "TaskAllocation");
        assert!(alloc.is_some(), "expected TaskAllocation type");
        assert_eq!(alloc.unwrap().1.category, ComponentCategory::Processor);
    }

    #[test]
    fn val_14_specialization() {
        let source = r#"
package SpecializationValidation {
    part def Vehicle;
    part def Car specializes Vehicle;
    part def Truck specializes Vehicle;
    part def SportsCar specializes Car;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let car = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Car");
        assert!(car.is_some(), "expected Car type");
        let (_, car_ct) = car.unwrap();
        assert!(car_ct.extends.is_some(), "Car should extend Vehicle");
        assert_eq!(
            car_ct.extends.as_ref().unwrap().type_name.as_str(),
            "Vehicle"
        );

        let sports_car = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "SportsCar");
        assert!(sports_car.is_some(), "expected SportsCar type");
        let (_, sc_ct) = sports_car.unwrap();
        assert!(sc_ct.extends.is_some(), "SportsCar should extend Car");
        assert_eq!(sc_ct.extends.as_ref().unwrap().type_name.as_str(), "Car");
    }

    #[test]
    fn val_15_multiplicity() {
        let source = r#"
package MultiplicityValidation {
    part def Wheel;
    part def Engine;
    part def Vehicle {
        part wheels [4] : Wheel;
        part engine [1] : Engine;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let vehicle_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Vehicle");
        assert!(vehicle_impl.is_some(), "expected Vehicle impl");
        let (_, vi) = vehicle_impl.unwrap();
        assert_eq!(vi.subcomponents.len(), 2);

        // Check wheels has array dimension of 4
        let wheels_sub = &tree.subcomponents[vi.subcomponents[0]];
        assert_eq!(wheels_sub.name.as_str(), "wheels");
        assert_eq!(wheels_sub.array_dimensions.len(), 1);
        assert_eq!(
            wheels_sub.array_dimensions[0].size,
            Some(spar_hir_def::item_tree::ArraySize::Literal(4))
        );

        // Check engine has array dimension of 1
        let engine_sub = &tree.subcomponents[vi.subcomponents[1]];
        assert_eq!(engine_sub.name.as_str(), "engine");
        assert_eq!(engine_sub.array_dimensions.len(), 1);
    }

    #[test]
    fn val_16_imports() {
        let source = r#"
package ImportSource {
    part def Sensor;
}
package ImportTarget {
    import ImportSource::*;
    part def System {
        part s : Sensor;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // ImportTarget should have a with clause for ImportSource
        let target = tree
            .packages
            .iter()
            .find(|(_, p)| p.name.as_str() == "ImportTarget");
        assert!(target.is_some(), "expected ImportTarget package");
        let (_, target_pkg) = target.unwrap();
        assert!(
            target_pkg
                .with_clauses
                .iter()
                .any(|w| w.as_str() == "ImportSource"),
            "expected ImportSource in with clauses"
        );
    }

    #[test]
    fn val_17_visibility() {
        let source = r#"
package VisibilityValidation {
    part def PublicPart;
    part def InternalPart;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        assert_eq!(tree.packages.len(), 1);
        let pkg = tree.packages.iter().next().unwrap().1;
        // All items should be in public_items by default
        assert_eq!(pkg.public_items.len(), 2, "expected 2 public items");
    }

    #[test]
    fn val_18_calcs() {
        let source = r#"
package CalcsValidation {
    calc def Latency;
    calc def Throughput;
    part def Controller {
        attribute latency : Latency;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // calc defs -> subprogram types
        let latency = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Latency");
        assert!(latency.is_some(), "expected Latency type");
        assert_eq!(latency.unwrap().1.category, ComponentCategory::Subprogram);

        let throughput = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Throughput");
        assert!(throughput.is_some(), "expected Throughput type");
        assert_eq!(
            throughput.unwrap().1.category,
            ComponentCategory::Subprogram
        );
    }

    #[test]
    fn val_19_abstract() {
        let source = r#"
package AbstractValidation {
    abstract part def Component;
    part def Sensor specializes Component;
    part def Actuator specializes Component;
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        let component = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Component");
        assert!(component.is_some(), "expected Component type");
        assert_eq!(
            component.unwrap().1.category,
            ComponentCategory::Abstract,
            "abstract part def should be abstract category"
        );

        // Sensor and Actuator should extend Component
        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor");
        assert!(sensor.is_some(), "expected Sensor type");
        let (_, sensor_ct) = sensor.unwrap();
        assert!(sensor_ct.extends.is_some());
        assert_eq!(
            sensor_ct.extends.as_ref().unwrap().type_name.as_str(),
            "Component"
        );
    }

    #[test]
    fn val_20_ref_usages() {
        let source = r#"
package RefUsagesValidation {
    part def Sensor;
    part def Controller {
        ref part mySensor : Sensor;
    }
}
"#;
        let parse = crate::parse(source);
        assert!(parse.ok(), "parse errors: {:?}", parse.errors());
        let tree = lower_to_aadl(&parse);

        // ref part -> subcomponent in implementation
        let ctrl_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Controller");
        assert!(ctrl_impl.is_some(), "expected Controller impl");
        let (_, ci) = ctrl_impl.unwrap();
        assert_eq!(ci.subcomponents.len(), 1);
        let sub = &tree.subcomponents[ci.subcomponents[0]];
        assert_eq!(sub.name.as_str(), "mySensor");
        assert!(sub.classifier.is_some());
        assert_eq!(
            sub.classifier.as_ref().unwrap().type_name.as_str(),
            "Sensor"
        );
    }

    // -----------------------------------------------------------------------
    // AADL text generation tests
    // -----------------------------------------------------------------------

    #[test]
    fn text_gen_extends() {
        let source = r#"
package SpecPkg {
    part def Base;
    part def Derived specializes Base;
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let aadl = item_tree_to_aadl(&tree);
        assert!(
            aadl.contains("extends Base"),
            "expected 'extends Base' in AADL output, got: {aadl}"
        );
    }

    #[test]
    fn text_gen_feature_group() {
        let source = r#"
package FGPkg {
    interface def MyInterface {
        end portA : PortTypeA;
        end portB : PortTypeB;
    }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let aadl = item_tree_to_aadl(&tree);
        assert!(
            aadl.contains("feature group MyInterface"),
            "expected feature group in AADL output, got: {aadl}"
        );
    }

    // -- Edge-case coverage tests (from PR #81) --

    #[test]
    fn empty_part_def_no_body() {
        let parse = crate::parse("part def Empty;");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "Empty");
        assert!(ct.features.is_empty());
        assert!(tree.component_impls.is_empty());
    }

    #[test]
    fn port_def_with_in_item() {
        let parse = crate::parse("port def InputPort { in item cmd; }");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.name.as_str(), "InputPort");
        assert_eq!(ct.features.len(), 1);
        let feat = &tree.features[ct.features[0]];
        assert_eq!(feat.name.as_str(), "cmd");
        assert_eq!(feat.direction, Some(Direction::In));
    }

    #[test]
    fn part_def_with_state_lowers_to_process() {
        let source = "part def StateMachine { state idle { } }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.category, ComponentCategory::Process);
    }

    #[test]
    fn part_def_with_port_usage_typed() {
        let source = "part def Sensor { out port sensorOut : SensorPort; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.features.len(), 1);
        let feat = &tree.features[ct.features[0]];
        assert_eq!(feat.name.as_str(), "sensorOut");
        assert_eq!(feat.direction, Some(Direction::Out));
        assert!(feat.classifier.is_some());
        assert_eq!(
            feat.classifier.as_ref().unwrap().type_name.as_str(),
            "SensorPort"
        );
    }

    #[test]
    fn connection_with_local_features() {
        let parse = crate::parse("connect srcPort to dstPort;");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.connections.len(), 1);
        let conn = tree.connections.iter().next().unwrap().1;
        let src = conn.src.as_ref().unwrap();
        assert!(src.subcomponent.is_none());
        assert_eq!(src.feature.as_str(), "srcPort");
        let dst = conn.dst.as_ref().unwrap();
        assert!(dst.subcomponent.is_none());
        assert_eq!(dst.feature.as_str(), "dstPort");
    }

    #[test]
    fn lower_empty_source() {
        let parse = crate::parse("");
        let tree = lower_to_aadl(&parse);
        assert!(tree.component_types.is_empty());
        assert!(tree.component_impls.is_empty());
        assert!(tree.packages.is_empty());
    }

    #[test]
    fn item_tree_to_aadl_empty() {
        let parse = crate::parse("");
        let tree = lower_to_aadl(&parse);
        let aadl = item_tree_to_aadl(&tree);
        assert!(aadl.is_empty());
    }

    #[test]
    fn item_tree_to_aadl_standalone_type() {
        let parse = crate::parse("part def StandaloneWidget { }");
        let tree = lower_to_aadl(&parse);
        let aadl = item_tree_to_aadl(&tree);
        assert!(
            aadl.contains("standalone"),
            "expected standalone marker: {aadl}"
        );
        assert!(
            aadl.contains("StandaloneWidget"),
            "expected type name: {aadl}"
        );
    }

    #[test]
    fn package_lowering_collects_items() {
        let source = r#"
package Lib {
    part def Alpha { }
    part def Beta {
        part child : Alpha;
    }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.packages.len(), 1);
        let pkg = tree.packages.iter().next().unwrap().1;
        assert!(
            pkg.public_items.len() >= 3,
            "expected >= 3 public items, got {}",
            pkg.public_items.len()
        );
    }

    #[test]
    fn item_tree_to_aadl_with_connections() {
        let source = r#"
package Net {
    part def Hub {
        part a : NodeA;
        part b : NodeB;
        connect a.out to b.in;
    }
    part def NodeA { }
    part def NodeB { }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let aadl = item_tree_to_aadl(&tree);
        assert!(
            aadl.contains("connections"),
            "expected connections section: {aadl}"
        );
        assert!(
            aadl.contains("subcomponents"),
            "expected subcomponents section: {aadl}"
        );
        assert!(aadl.contains("->"), "expected arrow in connection: {aadl}");
    }

    #[test]
    fn lower_part_def_inout_port() {
        let source = "part def Bridge { inout port bidir : DataPort; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.features.len(), 1);
        let feat = &tree.features[ct.features[0]];
        assert_eq!(feat.direction, Some(Direction::InOut));
    }

    #[test]
    fn lower_port_def_no_items() {
        let parse = crate::parse("port def EmptyPort { }");
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        let ct = tree.component_types.iter().next().unwrap().1;
        assert_eq!(ct.category, ComponentCategory::Data);
        assert!(ct.features.is_empty());
    }

    #[test]
    fn lower_qualified_type_ref() {
        let source = "part def Sys { part cpu : HwLib::Processor; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        assert!(!tree.component_impls.is_empty());
        let ci = tree.component_impls.iter().next().unwrap().1;
        let sub = &tree.subcomponents[ci.subcomponents[0]];
        let cls = sub.classifier.as_ref().unwrap();
        assert!(cls.package.is_some(), "expected package in classifier ref");
        assert_eq!(cls.package.as_ref().unwrap().as_str(), "HwLib");
        assert_eq!(cls.type_name.as_str(), "Processor");
    }

    // -- Bug 1: Diagnostics for unrecognized constructs --

    #[test]
    fn diagnostics_for_known_constructs_are_empty() {
        let source = r#"
package Pkg {
    part def A { }
    part def B { part a : A; }
}
"#;
        let parse = crate::parse(source);
        let (_tree, diags) = lower_to_aadl_with_diagnostics(&parse);
        assert!(
            diags.is_empty(),
            "expected no diagnostics for well-known constructs, got: {diags:?}"
        );
    }

    #[test]
    fn diagnostics_for_empty_source() {
        let parse = crate::parse("");
        let (_tree, diags) = lower_to_aadl_with_diagnostics(&parse);
        assert!(diags.is_empty());
    }

    #[test]
    fn lower_to_aadl_still_works_after_refactor() {
        // Ensure the non-diagnostic entry point returns the same tree.
        let source = "part def X { }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);
        assert_eq!(tree.component_types.len(), 1);
        assert_eq!(
            tree.component_types.iter().next().unwrap().1.name.as_str(),
            "X"
        );
    }

    // -- Bug 2: Subcomponent category propagation --

    #[test]
    fn subcomponent_category_propagated_from_process_type() {
        let source = r#"
package CatTest {
    part def Controller {
        action processData { }
    }
    part def System {
        part ctrl : Controller;
    }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        // Controller should be Process (has action)
        let ctrl_type = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Controller");
        assert_eq!(
            ctrl_type.unwrap().1.category,
            ComponentCategory::Process,
            "Controller type should be Process"
        );

        // The subcomponent `ctrl` in System's impl should also be Process
        let sys_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "System");
        assert!(sys_impl.is_some(), "expected System impl");
        let (_, si) = sys_impl.unwrap();
        let sub = &tree.subcomponents[si.subcomponents[0]];
        assert_eq!(sub.name.as_str(), "ctrl");
        assert_eq!(
            sub.category,
            ComponentCategory::Process,
            "subcomponent ctrl should inherit Process category from Controller type"
        );
    }

    #[test]
    fn subcomponent_category_propagated_data_type() {
        let source = r#"
package DataCatTest {
    attribute def Mass;
    part def Sensor {
        part mass_data : Mass;
    }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "Sensor");
        assert!(sensor_impl.is_some());
        let (_, si) = sensor_impl.unwrap();
        let sub = &tree.subcomponents[si.subcomponents[0]];
        assert_eq!(sub.name.as_str(), "mass_data");
        assert_eq!(
            sub.category,
            ComponentCategory::Data,
            "subcomponent should inherit Data category from attribute def"
        );
    }

    #[test]
    fn subcomponent_unknown_type_stays_system() {
        // When the referenced type is not defined locally, category stays System.
        let source = "part def Outer { part inner : UnknownType; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let ci = tree.component_impls.iter().next().unwrap().1;
        let sub = &tree.subcomponents[ci.subcomponents[0]];
        assert_eq!(
            sub.category,
            ComponentCategory::System,
            "unknown type reference should default to System"
        );
    }

    #[test]
    fn ref_usage_category_propagated() {
        let source = r#"
package RefCatTest {
    part def Controller {
        action doWork { }
    }
    part def System {
        ref part ctrl : Controller;
    }
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sys_impl = tree
            .component_impls
            .iter()
            .find(|(_, ci)| ci.type_name.as_str() == "System");
        let (_, si) = sys_impl.unwrap();
        let sub = &tree.subcomponents[si.subcomponents[0]];
        assert_eq!(
            sub.category,
            ComponentCategory::Process,
            "ref part subcomponent should inherit Process from Controller"
        );
    }

    // -- Bug 3: Property value parsing --

    #[test]
    fn attribute_usage_integer_value() {
        let source = "part def Sensor { attribute period = 10; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor")
            .unwrap()
            .1;
        assert_eq!(sensor.property_associations.len(), 1);
        let pa = &tree.property_associations[sensor.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::Integer(10, None)),
            "expected Integer(10, None)"
        );
    }

    #[test]
    fn attribute_usage_integer_with_unit() {
        let source = "part def Sensor { attribute period = 10 ms; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor")
            .unwrap()
            .1;
        let pa = &tree.property_associations[sensor.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::Integer(10, Some(Name::new("ms")))),
            "expected Integer(10, Some(ms))"
        );
        assert_eq!(pa.value, "10 ms");
    }

    #[test]
    fn attribute_usage_real_value() {
        let source = "part def Sensor { attribute rate = 1.5; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor")
            .unwrap()
            .1;
        let pa = &tree.property_associations[sensor.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::Real("1.5".to_string(), None)),
            "expected Real(1.5, None)"
        );
    }

    #[test]
    fn attribute_usage_string_value() {
        let source = r#"part def Sensor { attribute label = "hello"; }"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor")
            .unwrap()
            .1;
        let pa = &tree.property_associations[sensor.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::StringLit("hello".to_string())),
            "expected StringLit"
        );
    }

    #[test]
    fn constraint_usage_integer_with_unit() {
        let source = r#"
part def Controller {
    constraint period = 500 us;
}
"#;
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let ctrl = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Controller")
            .unwrap()
            .1;
        assert!(!ctrl.property_associations.is_empty());
        let pa = &tree.property_associations[ctrl.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::Integer(500, Some(Name::new("us")))),
            "constraint should parse 500 us"
        );
        assert_eq!(
            pa.name.property_set.as_ref().unwrap().as_str(),
            "Timing_Properties"
        );
    }

    #[test]
    fn attribute_usage_without_default_falls_back_to_opaque() {
        // No `= value`, so the type reference becomes the opaque value.
        let source = "part def Sensor { attribute mass : Mass; }";
        let parse = crate::parse(source);
        let tree = lower_to_aadl(&parse);

        let sensor = tree
            .component_types
            .iter()
            .find(|(_, ct)| ct.name.as_str() == "Sensor")
            .unwrap()
            .1;
        let pa = &tree.property_associations[sensor.property_associations[0]];
        assert_eq!(
            pa.typed_value,
            Some(PropertyExpr::Opaque("Mass".to_string())),
            "without default value, should fall back to Opaque(type_name)"
        );
    }
}
