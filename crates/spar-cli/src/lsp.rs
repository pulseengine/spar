//! LSP server for AADL files.
//!
//! Provides IDE features using the existing spar infrastructure:
//! - Diagnostics on open/save/change (parser errors + analysis)
//! - Hover information for components, features, keywords
//! - Document symbols (packages, types, implementations, property sets)
//! - Go-to-definition for classifier references (same-file + cross-file)
//! - Multi-file workspace support with GlobalScope
//! - textDocument/completion (keywords, classifiers, properties, packages)
//! - workspace/symbol search

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidOpenTextDocument, DidSaveTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as LspRequest,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DocumentSymbol, DocumentSymbolResponse,
    FileSystemWatcher, GotoDefinitionResponse, Hover, HoverContents, HoverProviderCapability,
    InitializeParams, Location, MarkedString, OneOf, Position, PublishDiagnosticsParams, Range,
    Registration, ServerCapabilities, SymbolInformation, SymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WatchKind, WorkspaceSymbolResponse,
};

use spar_hir_def::item_tree::ItemRef;
use spar_hir_def::resolver::GlobalScope;
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

    let init_params: InitializeParams = serde_json::from_value(init_params).unwrap();

    // Extract workspace root from initialize params.
    #[allow(deprecated)]
    let workspace_root = init_params
        .root_uri
        .as_ref()
        .and_then(|uri| uri_to_file_path(uri));

    eprintln!("spar-lsp: initialized, workspace root: {workspace_root:?}");

    let mut state = ServerState::new(workspace_root);

    // Scan workspace for .aadl files on startup.
    state.scan_workspace();
    state.rebuild_global_scope();

    // Register for file watching.
    register_file_watchers(&connection);

    main_loop(&connection, &mut state);

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
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                ":".to_string(),
                ".".to_string(),
            ]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

/// Register file watchers for .aadl files.
fn register_file_watchers(connection: &Connection) {
    let registration = Registration {
        id: "aadl-watcher".to_string(),
        method: "workspace/didChangeWatchedFiles".to_string(),
        register_options: Some(
            serde_json::to_value(lsp_types::DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![FileSystemWatcher {
                    glob_pattern: lsp_types::GlobPattern::String("**/*.aadl".to_string()),
                    kind: Some(WatchKind::Create | WatchKind::Delete | WatchKind::Change),
                }],
            })
            .unwrap(),
        ),
    };

    let params = lsp_types::RegistrationParams {
        registrations: vec![registration],
    };
    let req = Request::new(
        RequestId::from("register-file-watchers".to_string()),
        "client/registerCapability".to_string(),
        params,
    );
    // Best-effort: some clients may not support dynamic registration.
    let _ = connection.sender.send(Message::Request(req));
}

// ── Main loop ───────────────────────────────────────────────────────

fn main_loop(connection: &Connection, state: &mut ServerState) {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap_or(false) {
                    return;
                }
                handle_request(state, connection, req);
            }
            Message::Notification(notif) => {
                handle_notification(state, connection, notif);
            }
            Message::Response(_) => {
                // We don't send requests to the client that expect responses
                // (except registerCapability, which we fire-and-forget).
            }
        }
    }
}

// ── Server state ────────────────────────────────────────────────────

struct ServerState {
    /// Open document contents, keyed by URI string.
    documents: HashMap<String, String>,
    /// Parsed item trees for all known files, keyed by URI string.
    item_trees: HashMap<String, Arc<ItemTree>>,
    /// Workspace root directory (if known).
    workspace_root: Option<PathBuf>,
    /// Global scope built from all item trees.
    global_scope: GlobalScope,
    /// URIs of files that are open in the editor (receive diagnostics).
    open_files: Vec<String>,
}

impl ServerState {
    fn new(workspace_root: Option<PathBuf>) -> Self {
        Self {
            documents: HashMap::new(),
            item_trees: HashMap::new(),
            workspace_root,
            global_scope: GlobalScope::default(),
            open_files: Vec::new(),
        }
    }

    /// Scan the workspace root for all .aadl files and load them.
    fn scan_workspace(&mut self) {
        let root = match &self.workspace_root {
            Some(r) => r.clone(),
            None => return,
        };

        eprintln!("spar-lsp: scanning workspace {}", root.display());
        let mut count = 0;
        scan_aadl_files_recursive(&root, &mut |path| {
            if let Ok(content) = std::fs::read_to_string(path) {
                let uri_str = path_to_uri_string(path);
                // Parse and store item tree.
                let parsed = spar_syntax::parse(&content);
                let tree = spar_hir_def::item_tree::lower::lower_file(&parsed.syntax_node());
                self.item_trees.insert(uri_str.clone(), Arc::new(tree));
                // Store content for workspace files (but not as "open").
                self.documents.insert(uri_str, content);
                count += 1;
            }
        });
        eprintln!("spar-lsp: found {count} .aadl files in workspace");
    }

    /// Rebuild the GlobalScope from all known item trees.
    fn rebuild_global_scope(&mut self) {
        let trees: Vec<Arc<ItemTree>> = self.item_trees.values().cloned().collect();
        self.global_scope = GlobalScope::from_trees(trees);
    }

    /// Update a single file: parse, store item tree, rebuild scope.
    fn update_file(&mut self, uri_str: &str, content: &str) {
        let parsed = spar_syntax::parse(content);
        let tree = spar_hir_def::item_tree::lower::lower_file(&parsed.syntax_node());
        self.item_trees
            .insert(uri_str.to_string(), Arc::new(tree));
        self.documents
            .insert(uri_str.to_string(), content.to_string());
        self.rebuild_global_scope();
    }

    /// Remove a file from the workspace state.
    fn remove_file(&mut self, uri_str: &str) {
        self.item_trees.remove(uri_str);
        self.documents.remove(uri_str);
        self.rebuild_global_scope();
    }

    /// Publish diagnostics for all open files.
    fn publish_all_diagnostics(&self, connection: &Connection) {
        for uri_str in &self.open_files {
            if let Ok(uri) = uri_str.parse::<Uri>() {
                publish_diagnostics(self, connection, &uri);
            }
        }
    }
}

/// Recursively scan a directory for .aadl files.
fn scan_aadl_files_recursive(dir: &Path, callback: &mut dyn FnMut(&Path)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_aadl_files_recursive(&path, callback);
        } else if path
            .extension()
            .map_or(false, |ext| ext == "aadl")
        {
            callback(&path);
        }
    }
}

/// Convert a file path to a `file://` URI string.
fn path_to_uri_string(path: &Path) -> String {
    // Canonicalize if possible, otherwise use the path as-is.
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.display())
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
    } else if let Some((_, params)) = cast_request::<Completion>(req.clone()) {
        let result = handle_completion(state, params);
        send_ok(connection, req_id, serde_json::to_value(&result).unwrap());
    } else if let Some((_, params)) = cast_request::<WorkspaceSymbolRequest>(req.clone()) {
        let result = handle_workspace_symbol(state, params);
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
        // Track as open file.
        if !state.open_files.contains(&uri_str) {
            state.open_files.push(uri_str.clone());
        }
        state.update_file(&uri_str, &params.text_document.text);
        // Publish diagnostics for all open files (cross-file references may change).
        state.publish_all_diagnostics(connection);
    } else if let Some(params) = cast_notification::<DidChangeTextDocument>(&notif) {
        let uri_str = params.text_document.uri.as_str().to_string();
        // Full sync: the last content change has the full document text.
        if let Some(change) = params.content_changes.into_iter().last() {
            state.update_file(&uri_str, &change.text);
        }
        // Publish diagnostics for all open files.
        state.publish_all_diagnostics(connection);
    } else if let Some(params) = cast_notification::<DidSaveTextDocument>(&notif) {
        // Re-publish diagnostics on save.
        publish_diagnostics(state, connection, &params.text_document.uri);
    } else if let Some(params) = cast_notification::<DidChangeWatchedFiles>(&notif) {
        handle_watched_file_changes(state, connection, params);
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

/// Handle workspace/didChangeWatchedFiles: re-scan for new/deleted files.
fn handle_watched_file_changes(
    state: &mut ServerState,
    connection: &Connection,
    params: lsp_types::DidChangeWatchedFilesParams,
) {
    let mut changed = false;

    for event in &params.changes {
        let uri_str = event.uri.as_str().to_string();

        match event.typ {
            lsp_types::FileChangeType::CREATED | lsp_types::FileChangeType::CHANGED => {
                // Read the file from disk (unless it's already open with editor contents).
                if !state.open_files.contains(&uri_str) {
                    if let Some(path) = uri_to_file_path(&event.uri) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let parsed = spar_syntax::parse(&content);
                            let tree = spar_hir_def::item_tree::lower::lower_file(
                                &parsed.syntax_node(),
                            );
                            state.item_trees.insert(uri_str.clone(), Arc::new(tree));
                            state.documents.insert(uri_str, content);
                            changed = true;
                        }
                    }
                }
            }
            lsp_types::FileChangeType::DELETED => {
                state.remove_file(&uri_str);
                changed = true;
            }
            _ => {}
        }
    }

    if changed {
        state.rebuild_global_scope();
        state.publish_all_diagnostics(connection);
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

    // 2. Use the cached item tree for analysis.
    let tree = match state.item_trees.get(uri_str) {
        Some(t) => t.clone(),
        None => {
            let t = spar_hir_def::item_tree::lower::lower_file(&parsed.syntax_node());
            Arc::new(t)
        }
    };

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

    // Try to find a matching definition in the same file first.
    if let Some(range) = find_definition_range_in_file(&root, name, source) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
    }

    // Cross-file: search all workspace files for the definition.
    for (other_uri_str, other_source) in &state.documents {
        if other_uri_str == uri.as_str() {
            continue; // already checked
        }
        let other_parsed = spar_syntax::parse(other_source);
        let other_root = other_parsed.syntax_node();
        if let Some(range) = find_definition_range_in_file(&other_root, name, other_source) {
            if let Ok(other_uri) = other_uri_str.parse::<Uri>() {
                return Some(GotoDefinitionResponse::Scalar(Location {
                    uri: other_uri,
                    range,
                }));
            }
        }
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

// ── Completion ──────────────────────────────────────────────────────

fn handle_completion(
    state: &ServerState,
    params: CompletionParams,
) -> Option<CompletionResponse> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let source = state.documents.get(uri.as_str())?;

    let offset = position_to_offset(source, pos)?;

    // Determine context by examining the text before the cursor.
    let context = completion_context(source, offset);

    let mut items = Vec::new();

    match context {
        CompletionContext::AfterColon => {
            // After `:` in a feature/subcomponent declaration: offer classifier names.
            add_classifier_completions(state, None, &mut items);
            // Also offer keywords that follow `:` in features.
            add_feature_keyword_completions(&mut items);
        }
        CompletionContext::AfterWith => {
            // After `with`: offer package names.
            add_package_completions(state, &mut items);
        }
        CompletionContext::InPropertiesSection => {
            // Inside a properties section: offer property names.
            add_property_completions(state, &mut items);
        }
        CompletionContext::AfterDataPort | CompletionContext::AfterEventDataPort => {
            // After `data port` or `event data port`: offer data classifiers.
            add_classifier_completions(
                state,
                Some(spar_hir_def::item_tree::ComponentCategory::Data),
                &mut items,
            );
        }
        CompletionContext::General => {
            // General context: offer keywords + all classifiers.
            add_keyword_completions(&mut items);
            add_classifier_completions(state, None, &mut items);
            add_property_completions(state, &mut items);
        }
    }

    if items.is_empty() {
        None
    } else {
        Some(CompletionResponse::Array(items))
    }
}

/// Coarse completion context detection.
#[derive(Debug)]
enum CompletionContext {
    /// After `:` -- classifier or feature keyword expected.
    AfterColon,
    /// After `with` -- package names expected.
    AfterWith,
    /// Inside a `properties` section -- property names expected.
    InPropertiesSection,
    /// After `data port` -- data classifiers expected.
    AfterDataPort,
    /// After `event data port` -- data classifiers expected.
    AfterEventDataPort,
    /// General context.
    General,
}

fn completion_context(source: &str, offset: usize) -> CompletionContext {
    // Get text before cursor, trimming trailing partial identifier.
    let before = &source[..offset.min(source.len())];
    let trimmed = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
    let trimmed = trimmed.trim_end();

    if trimmed.ends_with(':') {
        // Check if this is `::` (qualified name) vs `:` (type annotation).
        if trimmed.ends_with("::") {
            // Inside a qualified name -- offer classifier completions.
            return CompletionContext::AfterColon;
        }
        return CompletionContext::AfterColon;
    }

    // Check for `data port` or `event data port` before cursor.
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("event data port") {
        return CompletionContext::AfterEventDataPort;
    }
    if lower.ends_with("data port") {
        return CompletionContext::AfterDataPort;
    }

    // Check if we're after `with`.
    if lower.ends_with("with") {
        return CompletionContext::AfterWith;
    }

    // Check if we're in a properties section by scanning backward for `properties` keyword.
    if is_in_section(source, offset, "properties") {
        return CompletionContext::InPropertiesSection;
    }

    CompletionContext::General
}

/// Heuristic: check if the cursor position is within a particular section.
///
/// Scans backward for the section keyword and checks that we haven't
/// exited the section (by seeing another section keyword or `end`).
fn is_in_section(source: &str, offset: usize, section: &str) -> bool {
    let before = &source[..offset.min(source.len())];
    let lower = before.to_ascii_lowercase();

    // Find the last occurrence of the section keyword.
    let section_pos = match lower.rfind(section) {
        Some(p) => p,
        None => return false,
    };

    // Check that no other section keyword appears after it.
    let after_section = &lower[section_pos + section.len()..];
    let other_sections = [
        "features", "subcomponents", "connections", "flows", "modes",
        "calls", "prototypes",
    ];
    for other in &other_sections {
        if *other != section && after_section.contains(other) {
            // Could be inside a different section now. Simple heuristic.
            // Only return false if the other section keyword appears on its own line.
            if let Some(pos) = after_section.rfind(other) {
                let before_other = &after_section[..pos];
                // Check if this looks like a section header (preceded by newline).
                if before_other.ends_with('\n') || before_other.trim_end().is_empty() {
                    return false;
                }
            }
        }
    }

    true
}

/// Add AADL keyword completions.
fn add_keyword_completions(items: &mut Vec<CompletionItem>) {
    // Component categories.
    let categories = [
        ("system", "Component category: composite integration component"),
        ("process", "Component category: protected address space"),
        ("thread", "Component category: schedulable execution unit"),
        ("thread group", "Component category: logical thread grouping"),
        ("processor", "Component category: hardware scheduler"),
        ("virtual processor", "Component category: virtual execution platform"),
        ("memory", "Component category: storage component"),
        ("bus", "Component category: hardware communication channel"),
        ("virtual bus", "Component category: virtual communication channel"),
        ("device", "Component category: external interface"),
        ("subprogram", "Component category: callable code"),
        ("subprogram group", "Component category: subprogram collection"),
        ("data", "Component category: data type/structure"),
        ("abstract", "Component category: generic component"),
    ];
    for (kw, detail) in &categories {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }

    // Structural keywords.
    let structural = [
        ("package", "Top-level namespace"),
        ("public", "Public section of a package"),
        ("private", "Private section of a package"),
        ("with", "Import package or property set"),
        ("end", "Close a declaration"),
        ("features", "Section: feature declarations"),
        ("subcomponents", "Section: subcomponent declarations"),
        ("connections", "Section: connection declarations"),
        ("flows", "Section: flow declarations"),
        ("modes", "Section: mode declarations"),
        ("properties", "Section: property associations"),
        ("implementation", "Component implementation"),
        ("extends", "Inherit from a parent classifier"),
        ("type", "Type declaration"),
        ("annex", "Embedded sublanguage block"),
    ];
    for (kw, detail) in &structural {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }

    // Flow keywords.
    let flow_kws = [
        ("flow", "Flow declaration"),
        ("source", "Flow source"),
        ("sink", "Flow sink"),
        ("path", "Flow path"),
    ];
    for (kw, detail) in &flow_kws {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }

    // Property keywords.
    let prop_kws = [
        ("applies", "Property applies-to clause"),
        ("to", "Part of applies-to"),
        ("constant", "Property constant"),
        ("inherit", "Property inheritance"),
        ("true", "Boolean literal"),
        ("false", "Boolean literal"),
        ("classifier", "Classifier value expression"),
        ("reference", "Reference value expression"),
        ("compute", "Computed value expression"),
    ];
    for (kw, detail) in &prop_kws {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }
}

/// Add feature-related keyword completions (after `:`).
fn add_feature_keyword_completions(items: &mut Vec<CompletionItem>) {
    let kws = [
        ("in", "Direction: input"),
        ("out", "Direction: output"),
        ("in out", "Direction: bidirectional"),
        ("port", "Port feature"),
        ("event", "Event modifier"),
        ("event port", "Event port feature"),
        ("event data port", "Event data port feature"),
        ("data port", "Data port feature"),
        ("data access", "Data access feature"),
        ("bus access", "Bus access feature"),
        ("subprogram access", "Subprogram access feature"),
        ("subprogram group access", "Subprogram group access feature"),
        ("access", "Access feature"),
        ("provides", "Provides access"),
        ("requires", "Requires access"),
        ("feature", "Abstract feature"),
        ("feature group", "Feature group"),
        ("parameter", "Subprogram parameter"),
    ];
    for (kw, detail) in &kws {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }
}

/// Add classifier name completions from the GlobalScope.
fn add_classifier_completions(
    state: &ServerState,
    filter_category: Option<spar_hir_def::item_tree::ComponentCategory>,
    items: &mut Vec<CompletionItem>,
) {
    let scope = &state.global_scope;

    // Component types.
    for (pkg_name, type_name, category) in scope.all_component_types() {
        if let Some(filter) = filter_category {
            if category != filter {
                continue;
            }
        }

        let qualified = format!("{}::{}", pkg_name.as_str(), type_name.as_str());
        items.push(CompletionItem {
            label: type_name.as_str().to_string(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(format!("{} type ({})", category, pkg_name.as_str())),
            insert_text: Some(qualified.clone()),
            ..Default::default()
        });
        // Also offer the unqualified name (useful for same-package refs).
        items.push(CompletionItem {
            label: qualified,
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(format!("{} type", category)),
            ..Default::default()
        });
    }

    // Component implementations.
    for (pkg_name, type_name, impl_name, category) in scope.all_component_impls() {
        if let Some(filter) = filter_category {
            if category != filter {
                continue;
            }
        }

        let impl_qualified = format!(
            "{}::{}.{}",
            pkg_name.as_str(),
            type_name.as_str(),
            impl_name.as_str()
        );
        let impl_short = format!("{}.{}", type_name.as_str(), impl_name.as_str());
        items.push(CompletionItem {
            label: impl_short.clone(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("{} implementation ({})", category, pkg_name.as_str())),
            insert_text: Some(impl_qualified.clone()),
            ..Default::default()
        });
        items.push(CompletionItem {
            label: impl_qualified,
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("{} implementation", category)),
            ..Default::default()
        });
    }

    // Feature group types.
    for (pkg_name, fgt_name) in scope.all_feature_group_types() {
        let qualified = format!("{}::{}", pkg_name.as_str(), fgt_name.as_str());
        items.push(CompletionItem {
            label: fgt_name.as_str().to_string(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some(format!("feature group type ({})", pkg_name.as_str())),
            insert_text: Some(qualified.clone()),
            ..Default::default()
        });
        items.push(CompletionItem {
            label: qualified,
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some("feature group type".to_string()),
            ..Default::default()
        });
    }
}

/// Add package name completions.
fn add_package_completions(state: &ServerState, items: &mut Vec<CompletionItem>) {
    for pkg_name in state.global_scope.package_names() {
        items.push(CompletionItem {
            label: pkg_name.as_str().to_string(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("package".to_string()),
            ..Default::default()
        });
    }
}

/// Add property name completions.
fn add_property_completions(state: &ServerState, items: &mut Vec<CompletionItem>) {
    let scope = &state.global_scope;

    for ps_name in scope.property_set_names() {
        let ps_str = ps_name.as_str();

        // Offer the property set name itself.
        items.push(CompletionItem {
            label: ps_str.to_string(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("property set".to_string()),
            ..Default::default()
        });

        // Offer all properties in this set as qualified names.
        for prop_name in scope.property_names_in_set(ps_str) {
            let qualified = format!("{}::{}", ps_str, prop_name.as_str());
            items.push(CompletionItem {
                label: qualified,
                kind: Some(CompletionItemKind::PROPERTY),
                detail: Some(format!("property ({})", ps_str)),
                ..Default::default()
            });
        }
    }

    // Also add standard property names from predeclared sets with type info.
    for prop in spar_hir_def::standard_properties::all_standard_properties() {
        let qualified = format!("{}::{}", prop.property_set, prop.name);
        // Avoid duplicates -- the GlobalScope already registered these,
        // but we add them with richer documentation.
        if items.iter().any(|i| i.label == qualified) {
            continue;
        }
        items.push(CompletionItem {
            label: qualified,
            kind: Some(CompletionItemKind::PROPERTY),
            detail: Some(format!("{} ({})", prop.type_description, prop.property_set)),
            ..Default::default()
        });
    }
}

// ── Workspace Symbol ────────────────────────────────────────────────

#[allow(deprecated)]
fn handle_workspace_symbol(
    state: &ServerState,
    params: lsp_types::WorkspaceSymbolParams,
) -> Option<WorkspaceSymbolResponse> {
    let query = params.query.to_ascii_lowercase();
    let mut symbols: Vec<SymbolInformation> = Vec::new();

    for (uri_str, tree) in &state.item_trees {
        let uri: Uri = match uri_str.parse() {
            Ok(u) => u,
            Err(_) => continue,
        };

        let zero_range = Range::new(Position::new(0, 0), Position::new(0, 0));

        // Packages.
        for (_idx, pkg) in tree.packages.iter() {
            if query.is_empty()
                || pkg.name.as_str().to_ascii_lowercase().contains(&query)
            {
                symbols.push(SymbolInformation {
                    name: pkg.name.as_str().to_string(),
                    kind: SymbolKind::MODULE,
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: uri.clone(),
                        range: zero_range,
                    },
                    container_name: None,
                });
            }

            // Component types within packages.
            for item_ref in pkg.public_items.iter().chain(pkg.private_items.iter()) {
                match item_ref {
                    ItemRef::ComponentType(ct_idx) => {
                        let ct = &tree.component_types[*ct_idx];
                        if query.is_empty()
                            || ct.name.as_str().to_ascii_lowercase().contains(&query)
                        {
                            symbols.push(SymbolInformation {
                                name: ct.name.as_str().to_string(),
                                kind: SymbolKind::CLASS,
                                tags: None,
                                deprecated: None,
                                location: Location {
                                    uri: uri.clone(),
                                    range: zero_range,
                                },
                                container_name: Some(pkg.name.as_str().to_string()),
                            });
                        }
                    }
                    ItemRef::ComponentImpl(ci_idx) => {
                        let ci = &tree.component_impls[*ci_idx];
                        let full_name =
                            format!("{}.{}", ci.type_name.as_str(), ci.impl_name.as_str());
                        if query.is_empty()
                            || full_name.to_ascii_lowercase().contains(&query)
                        {
                            symbols.push(SymbolInformation {
                                name: full_name,
                                kind: SymbolKind::METHOD,
                                tags: None,
                                deprecated: None,
                                location: Location {
                                    uri: uri.clone(),
                                    range: zero_range,
                                },
                                container_name: Some(pkg.name.as_str().to_string()),
                            });
                        }
                    }
                    ItemRef::FeatureGroupType(fgt_idx) => {
                        let fgt = &tree.feature_group_types[*fgt_idx];
                        if query.is_empty()
                            || fgt.name.as_str().to_ascii_lowercase().contains(&query)
                        {
                            symbols.push(SymbolInformation {
                                name: fgt.name.as_str().to_string(),
                                kind: SymbolKind::INTERFACE,
                                tags: None,
                                deprecated: None,
                                location: Location {
                                    uri: uri.clone(),
                                    range: zero_range,
                                },
                                container_name: Some(pkg.name.as_str().to_string()),
                            });
                        }
                    }
                    ItemRef::PropertySet(ps_idx) => {
                        let ps = &tree.property_sets[*ps_idx];
                        if query.is_empty()
                            || ps.name.as_str().to_ascii_lowercase().contains(&query)
                        {
                            symbols.push(SymbolInformation {
                                name: ps.name.as_str().to_string(),
                                kind: SymbolKind::NAMESPACE,
                                tags: None,
                                deprecated: None,
                                location: Location {
                                    uri: uri.clone(),
                                    range: zero_range,
                                },
                                container_name: Some(pkg.name.as_str().to_string()),
                            });
                        }
                    }
                    ItemRef::AnnexLibrary => {}
                }
            }
        }

        // Top-level property sets (outside packages).
        for (_idx, ps) in tree.property_sets.iter() {
            if query.is_empty()
                || ps.name.as_str().to_ascii_lowercase().contains(&query)
            {
                // Avoid duplicates from package-contained sets.
                let already = symbols.iter().any(|s| {
                    s.name == ps.name.as_str() && s.kind == SymbolKind::NAMESPACE
                });
                if !already {
                    symbols.push(SymbolInformation {
                        name: ps.name.as_str().to_string(),
                        kind: SymbolKind::NAMESPACE,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: uri.clone(),
                            range: zero_range,
                        },
                        container_name: None,
                    });
                }
            }
        }
    }

    Some(WorkspaceSymbolResponse::Flat(symbols))
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

/// Convert a `file://` URI to a local file path.
///
/// Returns `None` if the URI doesn't start with `file://`.
fn uri_to_file_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    if let Some(path_str) = s.strip_prefix("file://") {
        // Handle percent-encoded paths (basic decoding).
        let decoded = percent_decode(path_str);
        Some(PathBuf::from(decoded))
    } else {
        None
    }
}

/// Basic percent-decoding for URI paths.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().and_then(|c| hex_val(c));
            let lo = chars.next().and_then(|c| hex_val(c));
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h << 4 | l) as char);
            } else {
                result.push('%');
            }
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
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
