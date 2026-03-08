//! LSP server for AADL files.
//!
//! Provides IDE features using the existing spar infrastructure:
//! - Diagnostics on open/save/change (parser errors + analysis)
//! - Hover information for components, features, keywords
//! - Document symbols (packages, types, implementations, property sets)
//! - Go-to-definition for classifier references (same-file)

use std::collections::HashMap;
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as LspRequest,
};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbol, DocumentSymbolResponse, GotoDefinitionResponse,
    Hover, HoverContents, HoverProviderCapability, InitializeParams, Location, MarkedString, OneOf,
    Position, PublishDiagnosticsParams, Range, ServerCapabilities, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use spar_hir_def::item_tree::ItemRef;
use spar_hir_def::ItemTree;
use spar_syntax::SyntaxKind;

// ── Public entry point ──────────────────────────────────────────────

/// Run the LSP server on stdin/stdout.
pub fn run_lsp_server() {
    eprintln!("spar: starting LSP server");

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(server_capabilities()).unwrap();
    let init_params = match connection.initialize(server_capabilities) {
        Ok(params) => params,
        Err(e) => {
            eprintln!("spar-lsp: initialization error: {e}");
            return;
        }
    };

    let _init_params: InitializeParams = serde_json::from_value(init_params).unwrap();
    eprintln!("spar-lsp: initialized");

    main_loop(&connection);

    io_threads.join().unwrap();
    eprintln!("spar-lsp: shutdown complete");
}

// ── Capabilities ────────────────────────────────────────────────────

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

// ── Main loop ───────────────────────────────────────────────────────

fn main_loop(connection: &Connection) {
    let mut state = ServerState::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap_or(false) {
                    return;
                }
                handle_request(&mut state, connection, req);
            }
            Message::Notification(notif) => {
                handle_notification(&mut state, connection, notif);
            }
            Message::Response(_) => {
                // We don't send requests to the client that expect responses.
            }
        }
    }
}

// ── Server state ────────────────────────────────────────────────────

struct ServerState {
    /// Open document contents, keyed by URI string.
    documents: HashMap<String, String>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            documents: HashMap::new(),
        }
    }
}

// ── Request handling ────────────────────────────────────────────────

fn handle_request(state: &mut ServerState, connection: &Connection, req: Request) {
    let req_id = req.id.clone();

    if let Some((_, params)) = cast_request::<HoverRequest>(req.clone()) {
        let result = handle_hover(state, params);
        send_ok(connection, req_id, serde_json::to_value(&result).unwrap());
    } else if let Some((_, params)) = cast_request::<DocumentSymbolRequest>(req.clone()) {
        let result = handle_document_symbols(state, params);
        send_ok(connection, req_id, serde_json::to_value(&result).unwrap());
    } else if let Some((_, params)) = cast_request::<GotoDefinition>(req.clone()) {
        let result = handle_goto_definition(state, params);
        send_ok(connection, req_id, serde_json::to_value(&result).unwrap());
    } else {
        // Unknown request -- respond with method not found.
        let resp = Response::new_err(
            req_id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("unhandled method: {}", req.method),
        );
        connection.sender.send(Message::Response(resp)).unwrap();
    }
}

fn cast_request<R: LspRequest>(req: Request) -> Option<(RequestId, R::Params)> {
    match req.extract::<R::Params>(R::METHOD) {
        Ok(pair) => Some(pair),
        Err(_) => None,
    }
}

fn send_ok(connection: &Connection, id: RequestId, result: serde_json::Value) {
    let resp = Response {
        id,
        result: Some(result),
        error: None,
    };
    connection.sender.send(Message::Response(resp)).unwrap();
}

// ── Notification handling ───────────────────────────────────────────

fn handle_notification(state: &mut ServerState, connection: &Connection, notif: Notification) {
    if let Some(params) = cast_notification::<DidOpenTextDocument>(&notif) {
        let uri_str = params.text_document.uri.as_str().to_string();
        state
            .documents
            .insert(uri_str.clone(), params.text_document.text);
        publish_diagnostics(state, connection, &params.text_document.uri);
    } else if let Some(params) = cast_notification::<DidChangeTextDocument>(&notif) {
        let uri_str = params.text_document.uri.as_str().to_string();
        // Full sync: the last content change has the full document text.
        if let Some(change) = params.content_changes.into_iter().last() {
            state.documents.insert(uri_str, change.text);
        }
        publish_diagnostics(state, connection, &params.text_document.uri);
    } else if let Some(params) = cast_notification::<DidSaveTextDocument>(&notif) {
        // Re-publish diagnostics on save.
        publish_diagnostics(state, connection, &params.text_document.uri);
    }
}

fn cast_notification<N: lsp_types::notification::Notification>(
    notif: &Notification,
) -> Option<N::Params> {
    if notif.method == N::METHOD {
        serde_json::from_value(notif.params.clone()).ok()
    } else {
        None
    }
}

// ── Diagnostics ─────────────────────────────────────────────────────

fn publish_diagnostics(state: &ServerState, connection: &Connection, uri: &Uri) {
    let uri_str = uri.as_str();
    let source = match state.documents.get(uri_str) {
        Some(s) => s,
        None => return,
    };

    let mut diagnostics = Vec::new();

    // 1. Parse the file.
    let parsed = spar_syntax::parse(source);
    for err in parsed.errors() {
        let pos = offset_to_position(source, err.offset);
        diagnostics.push(Diagnostic {
            range: Range::new(pos, pos),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("spar-parser".to_string()),
            message: err.msg.clone(),
            ..Default::default()
        });
    }

    // 2. Build ItemTree and run declarative analyses.
    let tree = spar_hir_def::item_tree::lower::lower_file(&parsed.syntax_node());
    let tree = Arc::new(tree);

    let naming_diags = spar_analysis::naming_rules::check_naming_rules(&tree);
    let category_diags = spar_analysis::category_check::check_category_rules(&tree);

    for diag in naming_diags.iter().chain(category_diags.iter()) {
        let severity = match diag.severity {
            spar_analysis::Severity::Error => DiagnosticSeverity::ERROR,
            spar_analysis::Severity::Warning => DiagnosticSeverity::WARNING,
            spar_analysis::Severity::Info => DiagnosticSeverity::INFORMATION,
        };

        // Analysis diagnostics don't have byte offsets (they use path-based
        // locations), so we place them at the beginning of the file. In a
        // future version we can resolve the path to an actual source range.
        diagnostics.push(Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            severity: Some(severity),
            source: Some(format!("spar-{}", diag.analysis)),
            message: format!("{} (at {})", diag.message, diag.path.join("/")),
            ..Default::default()
        });
    }

    // 3. Publish.
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let notif = lsp_server::Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        params,
    );
    connection
        .sender
        .send(Message::Notification(notif))
        .unwrap();
}

// ── Hover ───────────────────────────────────────────────────────────

fn handle_hover(state: &ServerState, params: lsp_types::HoverParams) -> Option<Hover> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let source = state.documents.get(uri.as_str())?;

    let offset = position_to_offset(source, pos)?;
    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();

    // Find the token at the cursor position.
    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    let kind = token.kind();
    let text = token.text();

    // Keyword hover: show AADL reference.
    if kind.is_keyword() {
        if let Some(info) = keyword_hover_info(kind) {
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(info)),
                range: Some(token_range(&token)),
            });
        }
    }

    // Identifier hover: look up in the ItemTree.
    if kind == SyntaxKind::IDENT {
        let tree = spar_hir_def::item_tree::lower::lower_file(&root);
        if let Some(info) = identifier_hover_info(&tree, text) {
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(info)),
                range: Some(token_range(&token)),
            });
        }
    }

    None
}

fn keyword_hover_info(kind: SyntaxKind) -> Option<String> {
    let info = match kind {
        SyntaxKind::PACKAGE_KW => "**package** -- Top-level AADL namespace (AS5506 \u{00a7}4.2)",
        SyntaxKind::SYSTEM_KW => {
            "**system** -- Composite component for integration (AS5506 \u{00a7}5.1)"
        }
        SyntaxKind::PROCESS_KW => {
            "**process** -- Protected address space with threads (AS5506 \u{00a7}5.2)"
        }
        SyntaxKind::THREAD_KW => {
            "**thread** -- Schedulable unit of concurrent execution (AS5506 \u{00a7}5.4)"
        }
        SyntaxKind::PROCESSOR_KW => {
            "**processor** -- Hardware that schedules threads (AS5506 \u{00a7}5.5)"
        }
        SyntaxKind::MEMORY_KW => "**memory** -- Storage component (AS5506 \u{00a7}5.7)",
        SyntaxKind::BUS_KW => "**bus** -- Hardware communication channel (AS5506 \u{00a7}5.8)",
        SyntaxKind::DEVICE_KW => {
            "**device** -- Interface to external environment (AS5506 \u{00a7}5.9)"
        }
        SyntaxKind::VIRTUAL_KW => {
            "**virtual** -- Modifier for virtual processor/bus (AS5506 \u{00a7}5.6, \u{00a7}5.8)"
        }
        SyntaxKind::SUBPROGRAM_KW => {
            "**subprogram** -- Callable sequential code (AS5506 \u{00a7}5.10)"
        }
        SyntaxKind::DATA_KW => "**data** -- Data type/structure component (AS5506 \u{00a7}5.12)",
        SyntaxKind::ABSTRACT_KW => {
            "**abstract** -- Generic component, any category (AS5506 \u{00a7}5.13)"
        }
        SyntaxKind::IMPLEMENTATION_KW => {
            "**implementation** -- Declares internal structure of a component type (AS5506 \u{00a7}4.4)"
        }
        SyntaxKind::FEATURES_KW => {
            "**features** -- Section declaring ports, access, feature groups (AS5506 \u{00a7}8)"
        }
        SyntaxKind::SUBCOMPONENTS_KW => {
            "**subcomponents** -- Section declaring contained components (AS5506 \u{00a7}4.5)"
        }
        SyntaxKind::CONNECTIONS_KW => {
            "**connections** -- Section declaring data/event/access links (AS5506 \u{00a7}9)"
        }
        SyntaxKind::FLOWS_KW => {
            "**flows** -- Section declaring flow specifications or implementations (AS5506 \u{00a7}10)"
        }
        SyntaxKind::MODES_KW => {
            "**modes** -- Section declaring operational modes (AS5506 \u{00a7}12)"
        }
        SyntaxKind::PROPERTIES_KW => {
            "**properties** -- Section declaring property associations (AS5506 \u{00a7}11)"
        }
        SyntaxKind::PORT_KW => "**port** -- Communication interface point (AS5506 \u{00a7}8.1-8.3)",
        SyntaxKind::EVENT_KW => {
            "**event** -- Modifier for event port or event data port (AS5506 \u{00a7}8.2-8.3)"
        }
        SyntaxKind::ACCESS_KW => {
            "**access** -- Shared data/bus/subprogram access (AS5506 \u{00a7}8.4-8.6)"
        }
        SyntaxKind::FLOW_KW => "**flow** -- Flow source, sink, or path (AS5506 \u{00a7}10)",
        SyntaxKind::ANNEX_KW => {
            "**annex** -- Embedded sublanguage (EMV2, Behavior, etc.) (AS5506 \u{00a7}3.5)"
        }
        SyntaxKind::WITH_KW => "**with** -- Import package or property set (AS5506 \u{00a7}4.2)",
        SyntaxKind::END_KW => "**end** -- Closes a package, type, or implementation declaration",
        SyntaxKind::EXTENDS_KW => {
            "**extends** -- Inherit from a parent type or implementation (AS5506 \u{00a7}4.6)"
        }
        _ => return None,
    };
    Some(info.to_string())
}

fn identifier_hover_info(tree: &ItemTree, name: &str) -> Option<String> {
    // Search component types.
    for (_idx, ct) in tree.component_types.iter() {
        if ct.name.as_str().eq_ignore_ascii_case(name) {
            let n_features = ct.features.len();
            let n_flows = ct.flow_specs.len();
            let n_modes = ct.modes.len();
            return Some(format!(
                "**{} type** `{}`\n\n\
                 - {} feature(s)\n\
                 - {} flow spec(s)\n\
                 - {} mode(s)",
                ct.category, ct.name, n_features, n_flows, n_modes
            ));
        }
    }

    // Search component implementations.
    for (_idx, ci) in tree.component_impls.iter() {
        let qualified = format!("{}.{}", ci.type_name, ci.impl_name);
        if ci.type_name.as_str().eq_ignore_ascii_case(name)
            || ci.impl_name.as_str().eq_ignore_ascii_case(name)
            || qualified.eq_ignore_ascii_case(name)
        {
            let n_subs = ci.subcomponents.len();
            let n_conns = ci.connections.len();
            let n_e2e = ci.end_to_end_flows.len();
            return Some(format!(
                "**{} implementation** `{}.{}`\n\n\
                 - {} subcomponent(s)\n\
                 - {} connection(s)\n\
                 - {} end-to-end flow(s)",
                ci.category, ci.type_name, ci.impl_name, n_subs, n_conns, n_e2e
            ));
        }
    }

    // Search packages.
    for (_idx, pkg) in tree.packages.iter() {
        if pkg.name.as_str().eq_ignore_ascii_case(name) {
            let n_pub = pkg.public_items.len();
            let n_priv = pkg.private_items.len();
            let n_with = pkg.with_clauses.len();
            return Some(format!(
                "**package** `{}`\n\n\
                 - {} public item(s)\n\
                 - {} private item(s)\n\
                 - {} with clause(s)",
                pkg.name, n_pub, n_priv, n_with
            ));
        }
    }

    // Search features.
    for (_idx, feat) in tree.features.iter() {
        if feat.name.as_str().eq_ignore_ascii_case(name) {
            let dir = feat
                .direction
                .map(|d| format!("{} ", d))
                .unwrap_or_default();
            let cls = feat
                .classifier
                .as_ref()
                .map(|c| format!(" {}", c))
                .unwrap_or_default();
            return Some(format!(
                "**feature** `{}` : {}{}{}",
                feat.name, dir, feat.kind, cls
            ));
        }
    }

    // Search property sets.
    for (_idx, ps) in tree.property_sets.iter() {
        if ps.name.as_str().eq_ignore_ascii_case(name) {
            let n_defs = ps.property_defs.len();
            let n_types = ps.property_type_defs.len();
            let n_consts = ps.property_constants.len();
            return Some(format!(
                "**property set** `{}`\n\n\
                 - {} property definition(s)\n\
                 - {} property type(s)\n\
                 - {} constant(s)",
                ps.name, n_defs, n_types, n_consts
            ));
        }
    }

    // Search property definitions.
    for (_idx, ps) in tree.property_sets.iter() {
        for def in &ps.property_defs {
            if def.name.as_str().eq_ignore_ascii_case(name) {
                let type_str = def
                    .type_def
                    .as_ref()
                    .map(|t| format!("{:?}", t))
                    .unwrap_or_else(|| "unknown".to_string());
                return Some(format!(
                    "**property** `{}::{}` : {}",
                    ps.name, def.name, type_str
                ));
            }
        }
    }

    None
}

// ── Document Symbols ────────────────────────────────────────────────

#[allow(deprecated)]
fn handle_document_symbols(
    state: &ServerState,
    params: lsp_types::DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
    let uri = &params.text_document.uri;
    let source = state.documents.get(uri.as_str())?;
    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();
    let tree = spar_hir_def::item_tree::lower::lower_file(&root);

    let mut symbols = Vec::new();

    // Packages.
    for (_idx, pkg) in tree.packages.iter() {
        let mut children = Vec::new();

        for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
            match item_ref {
                ItemRef::ComponentType(ct_idx) => {
                    let ct = &tree.component_types[*ct_idx];
                    let mut ct_children = Vec::new();

                    // Features as children of the type.
                    for &fi in &ct.features {
                        let f = &tree.features[fi];
                        let detail = format!(
                            "{}{}",
                            f.direction.map(|d| format!("{} ", d)).unwrap_or_default(),
                            f.kind
                        );
                        ct_children.push(make_symbol(
                            f.name.as_str(),
                            Some(&detail),
                            SymbolKind::FIELD,
                            None,
                        ));
                    }

                    children.push(make_symbol(
                        ct.name.as_str(),
                        Some(&format!("{} type", ct.category)),
                        SymbolKind::CLASS,
                        if ct_children.is_empty() {
                            None
                        } else {
                            Some(ct_children)
                        },
                    ));
                }
                ItemRef::ComponentImpl(ci_idx) => {
                    let ci = &tree.component_impls[*ci_idx];
                    let mut ci_children = Vec::new();

                    // Subcomponents.
                    for &si in &ci.subcomponents {
                        let s = &tree.subcomponents[si];
                        let detail = format!(
                            "{}{}",
                            s.category,
                            s.classifier
                                .as_ref()
                                .map(|c| format!(" {}", c))
                                .unwrap_or_default()
                        );
                        ci_children.push(make_symbol(
                            s.name.as_str(),
                            Some(&detail),
                            SymbolKind::FIELD,
                            None,
                        ));
                    }

                    // Connections.
                    for &coni in &ci.connections {
                        let c = &tree.connections[coni];
                        let arrow = if c.is_bidirectional { "<->" } else { "->" };
                        ci_children.push(make_symbol(
                            c.name.as_str(),
                            Some(&format!("{:?} {}", c.kind, arrow)),
                            SymbolKind::EVENT,
                            None,
                        ));
                    }

                    children.push(make_symbol(
                        &format!("{}.{}", ci.type_name, ci.impl_name),
                        Some(&format!("{} implementation", ci.category)),
                        SymbolKind::METHOD,
                        if ci_children.is_empty() {
                            None
                        } else {
                            Some(ci_children)
                        },
                    ));
                }
                ItemRef::FeatureGroupType(fgt_idx) => {
                    let fgt = &tree.feature_group_types[*fgt_idx];
                    children.push(make_symbol(
                        fgt.name.as_str(),
                        Some("feature group type"),
                        SymbolKind::INTERFACE,
                        None,
                    ));
                }
                ItemRef::PropertySet(ps_idx) => {
                    let ps = &tree.property_sets[*ps_idx];
                    let ps_children: Vec<_> = ps
                        .property_defs
                        .iter()
                        .map(|def| {
                            make_symbol(def.name.as_str(), Some("property"), SymbolKind::PROPERTY, None)
                        })
                        .collect();
                    children.push(make_symbol(
                        ps.name.as_str(),
                        Some("property set"),
                        SymbolKind::NAMESPACE,
                        if ps_children.is_empty() {
                            None
                        } else {
                            Some(ps_children)
                        },
                    ));
                }
                ItemRef::AnnexLibrary => {}
            }
        }

        symbols.push(make_symbol(
            pkg.name.as_str(),
            Some("package"),
            SymbolKind::MODULE,
            if children.is_empty() {
                None
            } else {
                Some(children)
            },
        ));
    }

    // Top-level property sets (outside packages).
    for (_idx, ps) in tree.property_sets.iter() {
        // Skip if already covered by a package.
        let already_covered = symbols.iter().any(|s| {
            s.children
                .as_ref()
                .map_or(false, |ch| ch.iter().any(|c| c.name == ps.name.as_str()))
        });
        if already_covered {
            continue;
        }
        let ps_children: Vec<_> = ps
            .property_defs
            .iter()
            .map(|def| {
                make_symbol(def.name.as_str(), Some("property"), SymbolKind::PROPERTY, None)
            })
            .collect();
        symbols.push(make_symbol(
            ps.name.as_str(),
            Some("property set"),
            SymbolKind::NAMESPACE,
            if ps_children.is_empty() {
                None
            } else {
                Some(ps_children)
            },
        ));
    }

    Some(DocumentSymbolResponse::Nested(symbols))
}

/// Helper to create a DocumentSymbol with zeroed ranges.
///
/// In a future version, we can resolve actual source ranges from the CST.
#[allow(deprecated)]
fn make_symbol(
    name: &str,
    detail: Option<&str>,
    kind: SymbolKind,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    let zero_range = Range::new(Position::new(0, 0), Position::new(0, 0));
    DocumentSymbol {
        name: name.to_string(),
        detail: detail.map(|s| s.to_string()),
        kind,
        tags: None,
        deprecated: None,
        range: zero_range,
        selection_range: zero_range,
        children,
    }
}

// ── Go to Definition ────────────────────────────────────────────────

fn handle_goto_definition(
    state: &ServerState,
    params: lsp_types::GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let source = state.documents.get(uri.as_str())?;
    let offset = position_to_offset(source, pos)?;

    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();

    // Find the token at the cursor.
    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    if token.kind() != SyntaxKind::IDENT {
        return None;
    }

    let name = token.text();

    // Try to find a matching definition in the same file.
    if let Some(range) = find_definition_range_in_file(&root, name, source) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
    }

    None
}

/// Search for the definition of `name` in the CST, returning its range.
///
/// For v1, this searches within the same file for component types,
/// component implementations, packages, and property sets.
fn find_definition_range_in_file(
    root: &spar_syntax::SyntaxNode,
    name: &str,
    source: &str,
) -> Option<Range> {
    let target_kinds = [
        SyntaxKind::COMPONENT_TYPE,
        SyntaxKind::COMPONENT_IMPL,
        SyntaxKind::AADL_PACKAGE,
        SyntaxKind::PROPERTY_SET,
        SyntaxKind::FEATURE_GROUP_TYPE,
    ];

    for node in root.descendants() {
        if !target_kinds.contains(&node.kind()) {
            continue;
        }

        // Walk direct token children looking for a matching IDENT.
        for token in node.children_with_tokens() {
            if let Some(tok) = token.as_token() {
                if tok.kind() == SyntaxKind::IDENT
                    && tok.text().eq_ignore_ascii_case(name)
                {
                    let start: usize = tok.text_range().start().into();
                    let end: usize = tok.text_range().end().into();
                    let start_pos = offset_to_position(source, start);
                    let end_pos = offset_to_position(source, end);
                    return Some(Range::new(start_pos, end_pos));
                }
            }
        }
    }

    None
}

// ── Utility functions ───────────────────────────────────────────────

/// Convert a byte offset to an LSP Position (0-based line/character).
fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

/// Convert an LSP Position to a byte offset.
fn position_to_offset(text: &str, pos: Position) -> Option<usize> {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in text.char_indices() {
        if line == pos.line && col == pos.character {
            return Some(i);
        }
        if ch == '\n' {
            if line == pos.line {
                // Position is past the end of the line.
                return Some(i);
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    // Position at end of file.
    Some(text.len())
}

/// Compute an LSP Range for a rowan SyntaxToken.
fn token_range(token: &spar_syntax::SyntaxToken) -> Range {
    let root = token.parent().unwrap();
    let text = root.text().to_string();
    let start: usize = token.text_range().start().into();
    let end: usize = token.text_range().end().into();
    Range::new(
        offset_to_position(&text, start),
        offset_to_position(&text, end),
    )
}
