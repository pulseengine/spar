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
//! | `attribute` | AADL property |
//! | `constraint def` (timing) | Timing property |
//! | `allocate` | `Actual_Processor_Binding` |

use spar_hir_def::item_tree::{
    ComponentCategory, ComponentImplItem, ComponentTypeItem, ConnectedElementRef, ConnectionItem,
    ConnectionKind, Direction, Feature, FeatureKind, ItemRef, ItemTree, Package, SubcomponentItem,
};
use spar_hir_def::name::Name;

use crate::SyntaxNode;
use crate::syntax_kind::SyntaxKind;

/// Lower a parsed SysML v2 source file to an AADL [`ItemTree`].
///
/// Walks the rowan CST produced by [`crate::parse`] and maps SysML v2
/// constructs to their AADL equivalents following SEI guidelines.
pub fn lower_to_aadl(parse: &crate::Parse) -> ItemTree {
    let root = parse.syntax_node();
    let mut ctx = LowerCtx::default();
    lower_node(&root, &mut ctx);
    ctx.tree
}

/// Internal lowering context accumulating the AADL item tree.
#[derive(Default)]
struct LowerCtx {
    tree: ItemTree,
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
        _ => {}
    }
}

/// Lower a SysML v2 `package` to an AADL package.
fn lower_package(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    // Collect items from the namespace body
    let mut public_items = Vec::new();

    // Remember current type/impl counts so we can track what gets added
    let type_start = ctx.tree.component_types.len();
    let impl_start = ctx.tree.component_impls.len();

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

    ctx.tree.packages.alloc(Package {
        name,
        with_clauses: Vec::new(),
        public_items,
        private_items: Vec::new(),
        renames: Vec::new(),
    });
}

/// Lower a SysML v2 `part def` to an AADL component type.
///
/// Heuristic for category selection:
/// - If the part def body contains `action` or `state` usages -> `process`
/// - Otherwise -> `system` (default for hardware-like or generic parts)
fn lower_part_def(node: &SyntaxNode, ctx: &mut LowerCtx) {
    let name = match extract_name(node) {
        Some(n) => n,
        None => return,
    };

    let category = infer_category(node);

    // Collect features from port usages inside the part def body
    let mut features = Vec::new();
    let mut subcomponents = Vec::new();
    let mut connections = Vec::new();

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
                SyntaxKind::CONNECTION_USAGE => {
                    if let Some(conn) = lower_connection_node_to_item(&child) {
                        let idx = ctx.tree.connections.alloc(conn);
                        connections.push(idx);
                    }
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
        extends: None,
        features,
        flow_specs: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        prototypes: Vec::new(),
        property_associations: Vec::new(),
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

/// Convert a `part name : Type;` inside a part def body to an AADL subcomponent.
fn lower_part_usage_to_subcomponent(node: &SyntaxNode) -> Option<SubcomponentItem> {
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

// ---------------------------------------------------------------------------
// CST extraction helpers
// ---------------------------------------------------------------------------

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

        for item_ref in &pkg.public_items {
            match item_ref {
                ItemRef::ComponentType(idx) => {
                    let ct = &tree.component_types[*idx];
                    out.push_str(&format!("  {} type {}\n", ct.category, ct.name));
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
                            let cls = sub
                                .classifier
                                .as_ref()
                                .map(|c| {
                                    if let Some(pkg) = &c.package {
                                        format!(" {}::{}", pkg, c.type_name)
                                    } else {
                                        format!(" {}", c.type_name)
                                    }
                                })
                                .unwrap_or_default();
                            out.push_str(&format!("    {} : {} {};", sub.name, sub.category, cls));
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
        out.push_str(&format!("-- standalone {} type {}\n", ct.category, ct.name));
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
            aadl.contains("system type Vehicle"),
            "expected system type Vehicle in output"
        );
        assert!(
            aadl.contains("system type Sensor"),
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
}
