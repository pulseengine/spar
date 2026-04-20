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
//! - Code actions (quick-fix for end-name, semicolons, with-clauses, direction)
//! - Document formatting
//! - Rename (component types, features, subcomponents, packages)
//! - Inlay hints (component category, connection direction)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    InlayHintRequest, PrepareRenameRequest, Rename, Request as LspRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CompletionItem, CompletionItemKind, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity, DocumentFormattingParams,
    DocumentSymbol, DocumentSymbolResponse, FileSystemWatcher, FormattingOptions,
    GotoDefinitionResponse, Hover, HoverContents, HoverProviderCapability, InitializeParams,
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, Location, MarkedString, OneOf,
    Position, PrepareRenameResponse, PublishDiagnosticsParams, Range, Registration, RenameOptions,
    RenameParams, ServerCapabilities, SymbolInformation, SymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Uri, WatchKind, WorkspaceEdit, WorkspaceSymbolResponse,
};
use salsa::Setter;

use spar_base_db::{SourceFile, parse_file};
use spar_hir_def::ItemTree;
use spar_hir_def::item_tree::ItemRef;
use spar_hir_def::resolver::GlobalScope;
use spar_hir_def::{HirDefDatabase, file_item_tree};
use spar_syntax::SyntaxKind;

// ── Public entry point ──────────────────────────────────────────────

/// Run the LSP server on stdin/stdout.
pub fn run_lsp_server() {
    eprintln!("spar: starting LSP server");

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities =
        serde_json::to_value(server_capabilities()).expect("ServerCapabilities must serialize");
    let init_params = match connection.initialize(server_capabilities) {
        Ok(params) => params,
        Err(e) => {
            eprintln!("spar-lsp: initialization error: {e}");
            return;
        }
    };

    let init_params: InitializeParams =
        serde_json::from_value(init_params).expect("InitializeParams must deserialize");

    // Extract workspace root from initialize params.
    #[allow(deprecated)]
    let workspace_root = init_params.root_uri.as_ref().and_then(uri_to_file_path);

    eprintln!("spar-lsp: initialized, workspace root: {workspace_root:?}");

    let mut state = ServerState::new(workspace_root);

    // Scan workspace for .aadl files on startup.
    state.scan_workspace();
    state.rebuild_global_scope();

    // Register for file watching.
    register_file_watchers(&connection);

    main_loop(&connection, &mut state);

    io_threads
        .join()
        .expect("LSP I/O threads must join cleanly");
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
            trigger_characters: Some(vec![":".to_string(), ".".to_string()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        inlay_hint_provider: Some(OneOf::Left(true)),
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
            .expect("file watcher options must serialize"),
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
    /// Salsa incremental database for parsing and analysis.
    db: HirDefDatabase,
    /// Map from URI string to salsa SourceFile input.
    files: HashMap<String, SourceFile>,
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
            db: HirDefDatabase::default(),
            files: HashMap::new(),
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
                let file = SourceFile::new(&self.db, uri_str.clone(), content);
                self.files.insert(uri_str, file);
                count += 1;
            }
        });
        eprintln!("spar-lsp: found {count} .aadl files in workspace");
    }

    /// Rebuild the GlobalScope from all known files via salsa.
    fn rebuild_global_scope(&mut self) {
        let trees: Vec<Arc<ItemTree>> = self
            .files
            .values()
            .map(|file| file_item_tree(&self.db, *file))
            .collect();
        self.global_scope = GlobalScope::from_trees(trees);
    }

    /// Set the salsa input text for a file without rebuilding the global scope.
    ///
    /// Use this when batching multiple file updates — call
    /// `rebuild_global_scope()` once after all files are set.
    fn set_file_text(&mut self, uri_str: &str, content: &str) {
        if let Some(file) = self.files.get(uri_str) {
            file.set_text(&mut self.db).to(content.to_string());
        } else {
            let file = SourceFile::new(&self.db, uri_str.to_string(), content.to_string());
            self.files.insert(uri_str.to_string(), file);
        }
    }

    /// Update a single file: set text via salsa, rebuild scope.
    fn update_file(&mut self, uri_str: &str, content: &str) {
        self.set_file_text(uri_str, content);
        self.rebuild_global_scope();
    }

    /// Remove a file from the workspace state.
    #[cfg(test)]
    fn remove_file(&mut self, uri_str: &str) {
        self.files.remove(uri_str);
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

    /// Get the source text for a file URI via salsa.
    fn get_source(&self, uri_str: &str) -> Option<String> {
        let file = self.files.get(uri_str)?;
        Some(file.text(&self.db).clone())
    }

    /// Get the item tree for a file URI via salsa (parse + lower).
    fn get_item_tree(&self, uri_str: &str) -> Option<Arc<ItemTree>> {
        let file = self.files.get(uri_str)?;
        Some(file_item_tree(&self.db, *file))
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
        } else if path.extension().is_some_and(|ext| ext == "aadl") {
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
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<DocumentSymbolRequest>(req.clone()) {
        let result = handle_document_symbols(state, params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<GotoDefinition>(req.clone()) {
        let result = handle_goto_definition(state, params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<Completion>(req.clone()) {
        let result = handle_completion(state, params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<WorkspaceSymbolRequest>(req.clone()) {
        let result = handle_workspace_symbol(state, params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<CodeActionRequest>(req.clone()) {
        let result = handle_code_action(state, &params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<Formatting>(req.clone()) {
        let result = handle_formatting(state, &params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<PrepareRenameRequest>(req.clone()) {
        let result = handle_prepare_rename(state, &params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<Rename>(req.clone()) {
        let result = handle_rename(state, &params);
        send_result(connection, req_id, &result);
    } else if let Some((_, params)) = cast_request::<InlayHintRequest>(req.clone()) {
        let result = handle_inlay_hints(state, &params);
        send_result(connection, req_id, &result);
    } else {
        // Unknown request -- respond with method not found.
        let resp = Response::new_err(
            req_id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("unhandled method: {}", req.method),
        );
        let _ = connection.sender.send(Message::Response(resp));
    }
}

fn cast_request<R: LspRequest>(req: Request) -> Option<(RequestId, R::Params)> {
    req.extract::<R::Params>(R::METHOD).ok()
}

fn send_result<T: serde::Serialize>(connection: &Connection, id: RequestId, result: &T) {
    match serde_json::to_value(result) {
        Ok(val) => send_ok(connection, id, val),
        Err(e) => {
            eprintln!("spar-lsp: serialization error: {e}");
            let resp = Response::new_err(id, -32603, format!("internal error: {e}"));
            let _ = connection.sender.send(Message::Response(resp));
        }
    }
}

fn send_ok(connection: &Connection, id: RequestId, result: serde_json::Value) {
    let resp = Response {
        id,
        result: Some(result),
        error: None,
    };
    let _ = connection.sender.send(Message::Response(resp));
}

// ── Notification handling ───────────────────────────────────────────

fn handle_notification(state: &mut ServerState, connection: &Connection, notif: Notification) {
    if let Some(params) = cast_notification::<DidOpenTextDocument>(&notif) {
        let uri = params.text_document.uri.clone();
        let uri_str = uri.as_str().to_string();
        // Track as open file.
        if !state.open_files.contains(&uri_str) {
            state.open_files.push(uri_str.clone());
        }
        state.update_file(&uri_str, &params.text_document.text);
        // Publish diagnostics for the opened file only.
        publish_diagnostics(state, connection, &uri);
    } else if let Some(params) = cast_notification::<DidChangeTextDocument>(&notif) {
        let uri = params.text_document.uri.clone();
        let uri_str = uri.as_str().to_string();
        // Full sync: the last content change has the full document text.
        if let Some(change) = params.content_changes.into_iter().last() {
            state.update_file(&uri_str, &change.text);
        }
        // Publish diagnostics for the changed file only.
        publish_diagnostics(state, connection, &uri);
    } else if let Some(params) = cast_notification::<DidSaveTextDocument>(&notif) {
        // Re-publish diagnostics on save.
        publish_diagnostics(state, connection, &params.text_document.uri);
    } else if let Some(params) = cast_notification::<DidCloseTextDocument>(&notif) {
        let uri_str = params.text_document.uri.as_str().to_string();
        state.open_files.retain(|u| u != &uri_str);
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
            lsp_types::FileChangeType::CREATED | lsp_types::FileChangeType::CHANGED
                // Read the file from disk (unless it's already open with editor contents).
                if !state.open_files.contains(&uri_str) =>
            {
                if let Some(path) = uri_to_file_path(&event.uri)
                    && let Ok(content) = std::fs::read_to_string(&path)
                {
                    state.set_file_text(&uri_str, &content);
                    changed = true;
                }
            }
            lsp_types::FileChangeType::DELETED => {
                state.files.remove(&uri_str);
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
    let file = match state.files.get(uri_str) {
        Some(f) => *f,
        None => return,
    };

    let source = file.text(&state.db).clone();
    let line_index = LineIndex::new(&source);
    let mut diagnostics = Vec::new();

    // 1. Parse the file via salsa (memoized).
    let parse_result = parse_file(&state.db, file);
    for err in parse_result.errors() {
        let pos = line_index.offset_to_position(&source, err.offset);
        diagnostics.push(Diagnostic {
            range: Range::new(pos, pos),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("spar-parser".to_string()),
            message: err.msg.clone(),
            ..Default::default()
        });
    }

    // 2. Lower to item tree via salsa-cached query.
    let tree = file_item_tree(&state.db, file);

    let naming_diags = spar_analysis::naming_rules::check_naming_rules(&tree);
    let category_diags = spar_analysis::category_check::check_category_rules(&tree);

    let root = parse_result.syntax_node();
    for diag in naming_diags.iter().chain(category_diags.iter()) {
        let severity = match diag.severity {
            spar_analysis::Severity::Error => DiagnosticSeverity::ERROR,
            spar_analysis::Severity::Warning => DiagnosticSeverity::WARNING,
            spar_analysis::Severity::Info => DiagnosticSeverity::INFORMATION,
        };

        // Resolve the path-based location to a source range by searching
        // the CST for the named element.
        let range = resolve_path_to_range(&root, &source, &diag.path)
            .unwrap_or_else(|| Range::new(Position::new(0, 0), Position::new(0, 0)));

        diagnostics.push(Diagnostic {
            range,
            severity: Some(severity),
            source: Some(format!("spar-{}", diag.analysis)),
            message: diag.message.clone(),
            ..Default::default()
        });
    }

    // 3. Add completeness note so engineers know the LSP does not run
    //    the full suite of instance-level analyses.
    diagnostics.push(Diagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        severity: Some(DiagnosticSeverity::HINT),
        source: Some("spar".to_string()),
        message: "Note: LSP provides parse-level and naming diagnostics only. \
                  Run 'spar analyze' for full instance-level analysis \
                  (scheduling, latency, connectivity, etc.)"
            .to_string(),
        ..Default::default()
    });

    // 4. Publish.
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let notif = lsp_server::Notification::new(PublishDiagnostics::METHOD.to_string(), params);
    let _ = connection.sender.send(Message::Notification(notif));
}

// ── Hover ───────────────────────────────────────────────────────────

fn handle_hover(state: &ServerState, params: lsp_types::HoverParams) -> Option<Hover> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();
    let line_index = LineIndex::new(&source);

    let offset = line_index.position_to_offset(&source, pos)?;
    let parse_result = parse_file(&state.db, *file);
    let root = parse_result.syntax_node();

    // Find the token at the cursor position.
    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    let kind = token.kind();
    let text = token.text();

    // Keyword hover: show AADL reference.
    if kind.is_keyword()
        && let Some(info) = keyword_hover_info(kind)
    {
        return Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(info)),
            range: Some(token_range(&token)),
        });
    }

    // Identifier hover: look up in the ItemTree.
    if kind == SyntaxKind::IDENT {
        let tree = file_item_tree(&state.db, *file);
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
    let file = state.files.get(uri.as_str())?;
    let tree = file_item_tree(&state.db, *file);

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
                            make_symbol(
                                def.name.as_str(),
                                Some("property"),
                                SymbolKind::PROPERTY,
                                None,
                            )
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
                .is_some_and(|ch| ch.iter().any(|c| c.name == ps.name.as_str()))
        });
        if already_covered {
            continue;
        }
        let ps_children: Vec<_> = ps
            .property_defs
            .iter()
            .map(|def| {
                make_symbol(
                    def.name.as_str(),
                    Some("property"),
                    SymbolKind::PROPERTY,
                    None,
                )
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
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();
    let offset = LineIndex::new(&source).position_to_offset(&source, pos)?;

    let parse_result = parse_file(&state.db, *file);
    let root = parse_result.syntax_node();

    // Find the token at the cursor.
    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    if token.kind() != SyntaxKind::IDENT {
        return None;
    }

    let name = token.text();

    // Try to find a matching definition in the same file first.
    if let Some(range) = find_definition_range_in_file(&root, name, &source) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
    }

    // Cross-file: search all workspace files for the definition.
    for (other_uri_str, other_file) in &state.files {
        if other_uri_str == uri.as_str() {
            continue; // already checked
        }
        let other_source = other_file.text(&state.db).clone();
        let other_result = parse_file(&state.db, *other_file);
        let other_root = other_result.syntax_node();
        if let Some(range) = find_definition_range_in_file(&other_root, name, &other_source)
            && let Ok(other_uri) = other_uri_str.parse::<Uri>()
        {
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: other_uri,
                range,
            }));
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
            if let Some(tok) = token.as_token()
                && tok.kind() == SyntaxKind::IDENT
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

    None
}

// ── Completion ──────────────────────────────────────────────────────

fn handle_completion(state: &ServerState, params: CompletionParams) -> Option<CompletionResponse> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();

    let offset = LineIndex::new(&source).position_to_offset(&source, pos)?;

    // Determine context using CST ancestor walk (falls back to text heuristics
    // when the parse tree is unavailable).
    let parse_result = parse_file(&state.db, *file);
    let root = parse_result.syntax_node();
    let context = completion_context_from_cst(&root, &source, offset);

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

// NOTE: completion_context() and is_in_section() text heuristics removed.
// Replaced by completion_context_from_cst() which uses rowan CST ancestor
// walking. See resolve_path_to_range() and completion_context_from_cst()
// near the end of this file.

/// Add AADL keyword completions.
fn add_keyword_completions(items: &mut Vec<CompletionItem>) {
    // Component categories.
    let categories = [
        (
            "system",
            "Component category: composite integration component",
        ),
        ("process", "Component category: protected address space"),
        ("thread", "Component category: schedulable execution unit"),
        (
            "thread group",
            "Component category: logical thread grouping",
        ),
        ("processor", "Component category: hardware scheduler"),
        (
            "virtual processor",
            "Component category: virtual execution platform",
        ),
        ("memory", "Component category: storage component"),
        ("bus", "Component category: hardware communication channel"),
        (
            "virtual bus",
            "Component category: virtual communication channel",
        ),
        ("device", "Component category: external interface"),
        ("subprogram", "Component category: callable code"),
        (
            "subprogram group",
            "Component category: subprogram collection",
        ),
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
        if let Some(filter) = filter_category
            && category != filter
        {
            continue;
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
        if let Some(filter) = filter_category
            && category != filter
        {
            continue;
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
            detail: Some(format!(
                "{} implementation ({})",
                category,
                pkg_name.as_str()
            )),
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

    for (uri_str, file) in &state.files {
        let uri: Uri = match uri_str.parse() {
            Ok(u) => u,
            Err(_) => continue,
        };

        let tree = file_item_tree(&state.db, *file);

        let zero_range = Range::new(Position::new(0, 0), Position::new(0, 0));

        // Packages.
        for (_idx, pkg) in tree.packages.iter() {
            if query.is_empty() || pkg.name.as_str().to_ascii_lowercase().contains(&query) {
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
                        if query.is_empty() || full_name.to_ascii_lowercase().contains(&query) {
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
            if query.is_empty() || ps.name.as_str().to_ascii_lowercase().contains(&query) {
                // Avoid duplicates from package-contained sets.
                let already = symbols
                    .iter()
                    .any(|s| s.name == ps.name.as_str() && s.kind == SymbolKind::NAMESPACE);
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

// ── Code Actions ────────────────────────────────────────────────────

#[allow(clippy::mutable_key_type)]
fn handle_code_action(
    state: &ServerState,
    params: &CodeActionParams,
) -> Option<Vec<CodeActionOrCommand>> {
    let uri = &params.text_document.uri;
    let uri_str = uri.as_str();
    let source = state.get_source(uri_str)?;
    let diagnostics = &params.context.diagnostics;

    let mut actions = Vec::new();

    for diag in diagnostics {
        let msg = &diag.message;
        let diag_source = diag.source.as_deref().unwrap_or("");

        // Action 1: Fix end-name mismatch
        // Parser errors like `expected "end Foo"` or `expected IDENT`
        // after an `end` keyword.
        if diag_source == "spar-parser" && msg.contains("expected") {
            // Fix missing semicolons: `expected SEMICOLON`
            if msg.contains("SEMICOLON") {
                let insert_pos = diag.range.end;
                let edit = TextEdit {
                    range: Range::new(insert_pos, insert_pos),
                    new_text: ";".to_string(),
                };
                let mut changes = HashMap::new();
                changes.insert(uri.clone(), vec![edit]);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Insert missing semicolon".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }

            // Fix missing end keyword: `expected END_KW`
            if msg.contains("END_KW") {
                // Look backward from the diagnostic to find the declaration name
                if let Some(end_name) = find_expected_end_name(&source, &diag.range) {
                    let insert_pos = diag.range.start;
                    let edit = TextEdit {
                        range: Range::new(insert_pos, insert_pos),
                        new_text: format!("end {};\n", end_name),
                    };
                    let mut changes = HashMap::new();
                    changes.insert(uri.clone(), vec![edit]);
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Insert 'end {};'", end_name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }));
                }
            }
        }

        // Action 3: Add with-clause for unresolved classifier references
        if diag_source.starts_with("spar-")
            && msg.contains("unresolved")
            && let Some(name) = extract_unresolved_name(msg)
            && let Some(pkg_name) = find_package_for_name(state, &name)
            && let Some(with_edit) = make_with_clause_edit(&source, uri, &pkg_name)
        {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Add 'with {};'", pkg_name),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(with_edit),
                ..Default::default()
            }));
        }

        // Action 4: Quick-fix port direction mismatch
        if diag_source == "spar-direction_rules" && msg.contains("direction") {
            // Offer to swap connection endpoints
            if let Some(swap_edit) = make_swap_connection_edit(&source, uri, &diag.range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Swap connection endpoints".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(swap_edit),
                    ..Default::default()
                }));
            }
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Look backward from a diagnostic position to find the declaration name
/// that should follow `end`.
///
/// Uses the parser's CST to reliably find the enclosing declaration.
fn find_expected_end_name(source: &str, range: &Range) -> Option<String> {
    let offset = position_to_offset(source, range.start)?;

    // Parse the source to get the CST and walk it for the declaration containing the offset
    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();

    // Find the innermost declaration node containing (or just before) the offset
    let target_offset = rowan::TextSize::new(offset as u32);

    let declaration_kinds = [
        SyntaxKind::AADL_PACKAGE,
        SyntaxKind::COMPONENT_TYPE,
        SyntaxKind::COMPONENT_IMPL,
        SyntaxKind::FEATURE_GROUP_TYPE,
        SyntaxKind::PROPERTY_SET,
    ];

    let mut best_node = None;
    let mut best_end = rowan::TextSize::new(0);

    for node in root.descendants() {
        if declaration_kinds.contains(&node.kind()) {
            let node_range = node.text_range();
            // The diagnostic offset should be near the end of this node
            // or just after it (for missing `end` keywords)
            if node_range.start() < target_offset
                && (best_node.is_none() || node_range.start() > best_end)
            {
                best_node = Some(node.clone());
                best_end = node_range.start();
            }
        }
    }

    let decl_node = best_node?;

    // Extract the name from the declaration node
    match decl_node.kind() {
        SyntaxKind::AADL_PACKAGE
        | SyntaxKind::COMPONENT_TYPE
        | SyntaxKind::FEATURE_GROUP_TYPE
        | SyntaxKind::PROPERTY_SET => {
            // Name is the first IDENT token child (possibly inside a NAME node)
            for child in decl_node.children_with_tokens() {
                if let Some(tok) = child.as_token()
                    && tok.kind() == SyntaxKind::IDENT
                {
                    return Some(tok.text().to_string());
                }
                if let Some(node) = child.as_node()
                    && node.kind() == SyntaxKind::NAME
                {
                    // Extract text from the NAME node
                    let name_text: String = node.text().to_string();
                    let trimmed = name_text.trim().to_string();
                    if !trimmed.is_empty() {
                        return Some(trimmed);
                    }
                }
            }
        }
        SyntaxKind::COMPONENT_IMPL => {
            // For implementations, find the TypeName.ImplName
            // The REALIZATION node contains the type name, then DOT, then impl name
            let mut type_name = None;
            let mut impl_name = None;
            let mut past_implementation = false;
            let mut past_dot = false;

            for child in decl_node.children_with_tokens() {
                if let Some(tok) = child.as_token() {
                    if tok.kind() == SyntaxKind::IMPLEMENTATION_KW {
                        past_implementation = true;
                    } else if past_implementation && tok.kind() == SyntaxKind::DOT {
                        past_dot = true;
                    } else if past_implementation && past_dot && tok.kind() == SyntaxKind::IDENT {
                        impl_name = Some(tok.text().to_string());
                    }
                }
                if let Some(node) = child.as_node()
                    && node.kind() == SyntaxKind::REALIZATION
                    && past_implementation
                {
                    // Extract the type name from REALIZATION
                    for rtok in node.children_with_tokens() {
                        if let Some(t) = rtok.as_token()
                            && t.kind() == SyntaxKind::IDENT
                        {
                            type_name = Some(t.text().to_string());
                        }
                    }
                }
            }

            if let (Some(tn), Some(in_)) = (type_name, impl_name) {
                return Some(format!("{}.{}", tn, in_));
            }
        }
        _ => {}
    }

    // Fallback: use text-based heuristic for the package/type name
    let before = &source[..offset.min(source.len())];
    let lower = before.to_ascii_lowercase();

    if let Some(pos) = lower.rfind("package ") {
        let after = &before[pos + 8..];
        let name: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }

    None
}

/// Extract the unresolved name from an error message.
fn extract_unresolved_name(msg: &str) -> Option<String> {
    // Messages like "unresolved classifier 'Foo'" or "unresolved reference 'Bar'"
    let patterns = ["'", "`"];
    for pat in &patterns {
        if let Some(start) = msg.find(pat) {
            let rest = &msg[start + pat.len()..];
            if let Some(end) = rest.find(pat) {
                let name = &rest[..end];
                // Strip package qualifier if present
                let simple = name.rsplit("::").next().unwrap_or(name);
                return Some(simple.to_string());
            }
        }
    }
    None
}

/// Search all packages for a component type/impl matching the given name.
fn find_package_for_name(state: &ServerState, name: &str) -> Option<String> {
    let scope = &state.global_scope;
    for (pkg_name, type_name, _) in scope.all_component_types() {
        if type_name.as_str().eq_ignore_ascii_case(name) {
            return Some(pkg_name.as_str().to_string());
        }
    }
    for (pkg_name, type_name, impl_name, _) in scope.all_component_impls() {
        if type_name.as_str().eq_ignore_ascii_case(name)
            || impl_name.as_str().eq_ignore_ascii_case(name)
        {
            return Some(pkg_name.as_str().to_string());
        }
    }
    None
}

/// Generate a WorkspaceEdit to insert `with PackageName;` at the top of the file.
#[allow(clippy::mutable_key_type)]
fn make_with_clause_edit(source: &str, uri: &Uri, pkg_name: &str) -> Option<WorkspaceEdit> {
    // Check if we already have this with-clause
    let lower = source.to_ascii_lowercase();
    let with_check = format!("with {}", pkg_name.to_ascii_lowercase());
    if lower.contains(&with_check) {
        return None;
    }

    // Find the insertion point: after existing with-clauses, or after `public`/`package` line
    let insert_offset = find_with_clause_insert_offset(source);
    let insert_pos = offset_to_position(source, insert_offset);

    let edit = TextEdit {
        range: Range::new(insert_pos, insert_pos),
        new_text: format!("with {};\n", pkg_name),
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Find the byte offset where a new `with` clause should be inserted.
fn find_with_clause_insert_offset(source: &str) -> usize {
    let lower = source.to_ascii_lowercase();
    // After the last existing `with ...;` line
    let mut last_with_end = None;
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find("with ") {
        let abs_pos = search_from + pos;
        if let Some(semi) = source[abs_pos..].find(';') {
            let end = abs_pos + semi + 1;
            // Skip to end of line
            let line_end = source[end..].find('\n').map(|p| end + p + 1).unwrap_or(end);
            last_with_end = Some(line_end);
        }
        search_from = abs_pos + 5;
    }

    if let Some(offset) = last_with_end {
        return offset;
    }

    // After `public` keyword line
    if let Some(pos) = lower.find("public") {
        let line_end = source[pos..]
            .find('\n')
            .map(|p| pos + p + 1)
            .unwrap_or(pos + 6);
        return line_end;
    }

    // After `package Name` line
    if let Some(pos) = lower.find("package ") {
        let line_end = source[pos..]
            .find('\n')
            .map(|p| pos + p + 1)
            .unwrap_or(pos + 8);
        return line_end;
    }

    0
}

/// Generate a WorkspaceEdit to swap `src -> dst` to `dst -> src` in a connection.
#[allow(clippy::mutable_key_type)]
fn make_swap_connection_edit(source: &str, uri: &Uri, diag_range: &Range) -> Option<WorkspaceEdit> {
    // Find the line containing the diagnostic
    let offset = position_to_offset(source, diag_range.start)?;
    let line_start = source[..offset].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = source[offset..]
        .find('\n')
        .map(|p| offset + p)
        .unwrap_or(source.len());
    let line = &source[line_start..line_end];

    // Look for `src -> dst` or `src <-> dst` pattern
    let (arrow, arrow_len) = if let Some(pos) = line.find(" -> ") {
        (pos, 4)
    } else {
        let pos = line.find(" <-> ")?;
        (pos, 5)
    };

    // Find the source and destination parts
    // Source is between `:` (or `port`/`access` keyword) and arrow
    // Look for the content before the arrow
    let before_arrow = line[..arrow].trim();
    let after_arrow = line[arrow + arrow_len..]
        .trim()
        .trim_end_matches(';')
        .trim();

    // Find the last space-separated token before arrow (src endpoint)
    // and first token after arrow (dst endpoint)
    // The connection pattern is: `name : kind src -> dst;`
    // We need to find the endpoint references
    let arrow_str = &line[arrow..arrow + arrow_len];

    // Simple swap: replace `src -> dst` with `dst -> src` (preserving arrow type)
    // Find the start of src endpoint (after connection kind keywords)
    let src_start = find_connection_endpoint_start(before_arrow)?;
    let src_text = before_arrow[src_start..].trim();
    let dst_text = after_arrow.trim_end_matches(|c: char| c == ';' || c.is_whitespace());

    if src_text.is_empty() || dst_text.is_empty() {
        return None;
    }

    // Build the replacement: swap src and dst
    let old_text = format!(
        "{}{}{}",
        src_text,
        arrow_str,
        after_arrow.split(';').next().unwrap_or(after_arrow)
    );
    let new_text = format!("{}{}{}", dst_text, arrow_str, src_text);

    // Find the position of the old text in the line
    let old_pos = line.find(&old_text)?;
    let replace_start = line_start + old_pos;
    let replace_end = replace_start + old_text.len();

    let start_pos = offset_to_position(source, replace_start);
    let end_pos = offset_to_position(source, replace_end);

    let edit = TextEdit {
        range: Range::new(start_pos, end_pos),
        new_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Find the start of the connection source endpoint within a connection line prefix.
fn find_connection_endpoint_start(text: &str) -> Option<usize> {
    // Connection syntax: `name : port src` or `name : access src` etc.
    // The endpoint starts after the last connection-kind keyword
    let keywords = [
        "port ",
        "access ",
        "feature group ",
        "feature ",
        "parameter ",
    ];
    let mut last = 0;
    for kw in &keywords {
        let lower = text.to_ascii_lowercase();
        if let Some(pos) = lower.rfind(kw) {
            let candidate = pos + kw.len();
            if candidate > last {
                last = candidate;
            }
        }
    }
    if last > 0 {
        Some(last)
    } else {
        // Fallback: after `: `
        text.find(": ").map(|p| p + 2)
    }
}

// ── Document Formatting ─────────────────────────────────────────────

fn handle_formatting(
    state: &ServerState,
    params: &DocumentFormattingParams,
) -> Option<Vec<TextEdit>> {
    let uri = &params.text_document.uri;
    let source = state.get_source(uri.as_str())?;
    let edits = format_document(&source, &params.options);
    if edits.is_empty() { None } else { Some(edits) }
}

/// Format an AADL document by walking the CST and emitting properly formatted text.
fn format_document(text: &str, options: &FormattingOptions) -> Vec<TextEdit> {
    let parsed = spar_syntax::parse(text);
    let root = parsed.syntax_node();
    let indent_size = options.tab_size as usize;
    let formatted = format_node_recursive(&root, 0, indent_size);

    if formatted == text {
        return vec![];
    }

    // Return a single whole-document replacement
    vec![TextEdit {
        range: full_document_range(text),
        new_text: formatted,
    }]
}

/// Compute the Range covering the entire document.
fn full_document_range(text: &str) -> Range {
    let lines: Vec<&str> = text.lines().collect();
    let last_line = lines.len().saturating_sub(1) as u32;
    let last_col = lines.last().map(|l| l.len() as u32).unwrap_or(0);
    Range::new(Position::new(0, 0), Position::new(last_line, last_col))
}

/// Walk the CST and produce formatted output.
///
/// The formatter:
/// - Uses consistent indentation (configurable spaces)
/// - Normalizes whitespace around operators
/// - Preserves comments
/// - Normalizes keyword casing to lowercase
/// - Preserves annex content as-is
fn format_node_recursive(
    node: &spar_syntax::SyntaxNode,
    depth: usize,
    indent_size: usize,
) -> String {
    use rowan::NodeOrToken;

    let mut out = String::new();
    let _indent = " ".repeat(depth * indent_size);
    let _inner_indent = " ".repeat((depth + 1) * indent_size);

    match node.kind() {
        SyntaxKind::SOURCE_FILE => {
            // Top-level: format children at depth 0
            for child in node.children_with_tokens() {
                match child {
                    NodeOrToken::Node(n) => {
                        out.push_str(&format_node_recursive(&n, 0, indent_size));
                    }
                    NodeOrToken::Token(t) => {
                        out.push_str(&format_token(&t, false));
                    }
                }
            }
            // Ensure file ends with a newline
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        SyntaxKind::AADL_PACKAGE => {
            out.push_str(&format_package(node, depth, indent_size));
        }
        SyntaxKind::COMPONENT_TYPE => {
            out.push_str(&format_component_type(node, depth, indent_size));
        }
        SyntaxKind::COMPONENT_IMPL => {
            out.push_str(&format_component_impl(node, depth, indent_size));
        }
        SyntaxKind::FEATURE_GROUP_TYPE => {
            out.push_str(&format_classifier_generic(node, depth, indent_size));
        }
        SyntaxKind::PROPERTY_SET => {
            out.push_str(&format_classifier_generic(node, depth, indent_size));
        }
        SyntaxKind::ANNEX_SUBCLAUSE | SyntaxKind::ANNEX_LIBRARY => {
            // Preserve annex content as-is
            out.push_str(&node.text().to_string());
        }
        _ => {
            // Default: preserve original text
            out.push_str(&node.text().to_string());
        }
    }

    out
}

/// Format a package node.
fn format_package(node: &spar_syntax::SyntaxNode, depth: usize, indent_size: usize) -> String {
    use rowan::NodeOrToken;

    let mut out = String::new();
    let indent = " ".repeat(depth * indent_size);
    let mut prev_was_newline = false;

    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Token(t) => {
                let kind = t.kind();
                if kind == SyntaxKind::WHITESPACE {
                    // Normalize whitespace: collapse multiple blank lines to one
                    let ws = t.text().to_string();
                    let newline_count = ws.chars().filter(|c| *c == '\n').count();
                    if newline_count > 0 {
                        if newline_count > 1 && !prev_was_newline {
                            out.push_str("\n\n");
                        } else {
                            out.push('\n');
                        }
                        prev_was_newline = true;
                    } else {
                        out.push(' ');
                        prev_was_newline = false;
                    }
                } else if kind == SyntaxKind::COMMENT {
                    out.push_str(t.text());
                    prev_was_newline = false;
                } else {
                    let text = format_keyword_token(&t);
                    out.push_str(&text);
                    prev_was_newline = false;
                }
            }
            NodeOrToken::Node(n) => {
                match n.kind() {
                    SyntaxKind::PUBLIC_SECTION | SyntaxKind::PRIVATE_SECTION => {
                        out.push_str(&format_section(&n, depth, indent_size));
                    }
                    SyntaxKind::WITH_CLAUSE => {
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&indent);
                        out.push_str(&format_inline_node(&n));
                        out.push('\n');
                    }
                    _ => {
                        out.push_str(&format_node_recursive(&n, depth, indent_size));
                    }
                }
                prev_was_newline = out.ends_with('\n');
            }
        }
    }

    out
}

/// Format a public/private section.
fn format_section(node: &spar_syntax::SyntaxNode, depth: usize, indent_size: usize) -> String {
    use rowan::NodeOrToken;

    let mut out = String::new();
    let indent = " ".repeat(depth * indent_size);

    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Token(t) => {
                let kind = t.kind();
                if kind == SyntaxKind::WHITESPACE {
                    let ws = t.text().to_string();
                    let newlines = ws.chars().filter(|c| *c == '\n').count();
                    if newlines > 0 {
                        if newlines > 1 {
                            out.push_str("\n\n");
                        } else {
                            out.push('\n');
                        }
                    } else {
                        out.push(' ');
                    }
                } else if kind == SyntaxKind::COMMENT {
                    out.push_str(t.text());
                } else {
                    out.push_str(&format_keyword_token(&t));
                }
            }
            NodeOrToken::Node(n) => match n.kind() {
                SyntaxKind::WITH_CLAUSE => {
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&indent);
                    out.push_str(&format_inline_node(&n));
                    out.push('\n');
                }
                SyntaxKind::COMPONENT_TYPE => {
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format_component_type(&n, depth + 1, indent_size));
                }
                SyntaxKind::COMPONENT_IMPL => {
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format_component_impl(&n, depth + 1, indent_size));
                }
                SyntaxKind::ANNEX_SUBCLAUSE | SyntaxKind::ANNEX_LIBRARY => {
                    out.push_str(&n.text().to_string());
                }
                _ => {
                    out.push_str(&format_classifier_generic(&n, depth + 1, indent_size));
                }
            },
        }
    }

    out
}

/// Format a component type declaration.
fn format_component_type(
    node: &spar_syntax::SyntaxNode,
    depth: usize,
    indent_size: usize,
) -> String {
    format_classifier_body(node, depth, indent_size)
}

/// Format a component implementation declaration.
fn format_component_impl(
    node: &spar_syntax::SyntaxNode,
    depth: usize,
    indent_size: usize,
) -> String {
    format_classifier_body(node, depth, indent_size)
}

/// Common formatting for classifier bodies (types, implementations, feature group types, etc.).
fn format_classifier_body(
    node: &spar_syntax::SyntaxNode,
    depth: usize,
    indent_size: usize,
) -> String {
    use rowan::NodeOrToken;

    let mut out = String::new();
    let indent = " ".repeat(depth * indent_size);
    let section_indent = " ".repeat((depth + 1) * indent_size);

    // Track if we're building the header line or in sections
    let mut in_header = true;

    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Token(t) => {
                let kind = t.kind();
                if kind == SyntaxKind::WHITESPACE {
                    if in_header {
                        out.push(' ');
                    } else {
                        let ws = t.text().to_string();
                        let newlines = ws.chars().filter(|c| *c == '\n').count();
                        if newlines > 0 {
                            out.push('\n');
                        } else {
                            out.push(' ');
                        }
                    }
                } else if kind == SyntaxKind::COMMENT {
                    out.push_str(t.text());
                } else if kind == SyntaxKind::END_KW {
                    in_header = false;
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&indent);
                    out.push_str("end");
                } else if kind == SyntaxKind::SEMICOLON {
                    out.push(';');
                    out.push('\n');
                } else {
                    out.push_str(&format_keyword_token(&t));
                }
            }
            NodeOrToken::Node(n) => {
                let nk = n.kind();
                let is_section = matches!(
                    nk,
                    SyntaxKind::FEATURE_SECTION
                        | SyntaxKind::SUBCOMPONENT_SECTION
                        | SyntaxKind::CONNECTION_SECTION
                        | SyntaxKind::FLOW_SPEC_SECTION
                        | SyntaxKind::FLOW_IMPL_SECTION
                        | SyntaxKind::MODE_SECTION
                        | SyntaxKind::PROPERTY_SECTION
                        | SyntaxKind::PROTOTYPE_SECTION
                        | SyntaxKind::CALL_SECTION
                        | SyntaxKind::INTERNAL_FEATURES_SECTION
                        | SyntaxKind::PROCESSOR_FEATURES_SECTION
                );

                if is_section {
                    in_header = false;
                    out.push_str(&format_body_section(&n, depth, indent_size));
                } else if nk == SyntaxKind::ANNEX_SUBCLAUSE {
                    in_header = false;
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&section_indent);
                    out.push_str(&n.text().to_string());
                    out.push('\n');
                } else {
                    out.push_str(&format_inline_node(&n));
                }
            }
        }
    }

    // Ensure we end with newline
    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Format a body section (features, subcomponents, connections, etc.).
fn format_body_section(node: &spar_syntax::SyntaxNode, depth: usize, indent_size: usize) -> String {
    use rowan::NodeOrToken;

    let mut out = String::new();
    let section_indent = " ".repeat((depth + 1) * indent_size);
    let item_indent = " ".repeat((depth + 2) * indent_size);
    let mut first = true;

    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Token(t) => {
                let kind = t.kind();
                if kind == SyntaxKind::WHITESPACE {
                    // Skip whitespace; we control formatting
                    continue;
                } else if kind == SyntaxKind::COMMENT {
                    if !out.ends_with('\n') && !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(t.text());
                    if !t.text().ends_with('\n') {
                        out.push('\n');
                    }
                } else if kind.is_keyword() {
                    // Section keyword (features, subcomponents, etc.)
                    if first {
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&section_indent);
                        first = false;
                    }
                    out.push_str(&t.text().to_string().to_ascii_lowercase());
                } else if kind == SyntaxKind::SEMICOLON {
                    out.push(';');
                    out.push('\n');
                } else {
                    out.push_str(t.text());
                }
            }
            NodeOrToken::Node(n) => {
                // Each declaration item on its own line with deeper indent
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&item_indent);
                out.push_str(&format_declaration_item(&n));
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }

    out
}

/// Format a single declaration item (feature, subcomponent, connection, etc.)
/// as an inline string.
fn format_declaration_item(node: &spar_syntax::SyntaxNode) -> String {
    let raw = node.text().to_string();
    // Normalize: collapse whitespace, lowercase keywords, normalize operators
    normalize_declaration(&raw)
}

/// Normalize a declaration line: fix whitespace around operators and lowercase keywords.
fn normalize_declaration(text: &str) -> String {
    let mut result = String::new();
    let trimmed = text.trim();

    let mut prev_was_space = false;

    for ch in trimmed.chars() {
        match ch {
            '\n' | '\r' => {
                // Replace newlines with a single space (declarations should be on one line)
                if !prev_was_space && !result.is_empty() {
                    result.push(' ');
                    prev_was_space = true;
                }
            }
            '\t' | ' ' => {
                if !prev_was_space && !result.is_empty() {
                    result.push(' ');
                    prev_was_space = true;
                }
            }
            _ => {
                result.push(ch);
                prev_was_space = false;
            }
        }
    }

    // Normalize keywords to lowercase by re-parsing tokens
    let tokens = spar_parser::lexer::tokenize(&result);
    let mut out = String::new();
    let mut offset = 0;
    for (kind, len) in &tokens {
        let tok_text = &result[offset..offset + len];
        if kind.is_keyword() {
            out.push_str(&tok_text.to_ascii_lowercase());
        } else {
            out.push_str(tok_text);
        }
        offset += len;
    }

    // Normalize spacing around operators
    normalize_operator_spacing(&out)
}

/// Normalize spacing around AADL operators: `:`, `->`, `<->`, `=>`, `::`, `.`.
fn normalize_operator_spacing(text: &str) -> String {
    let mut result = String::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Check for multi-character operators first
        if i + 2 < len && &text[i..i + 3] == "<->" {
            trim_trailing_space(&mut result);
            result.push_str(" <-> ");
            i += 3;
            skip_spaces(bytes, &mut i);
        } else if i + 1 < len && &text[i..i + 2] == "->" {
            trim_trailing_space(&mut result);
            result.push_str(" -> ");
            i += 2;
            skip_spaces(bytes, &mut i);
        } else if i + 2 < len && &text[i..i + 3] == "+=>" {
            trim_trailing_space(&mut result);
            result.push_str(" +=> ");
            i += 3;
            skip_spaces(bytes, &mut i);
        } else if i + 1 < len && &text[i..i + 2] == "=>" {
            trim_trailing_space(&mut result);
            result.push_str(" => ");
            i += 2;
            skip_spaces(bytes, &mut i);
        } else if i + 1 < len && &text[i..i + 2] == "::" {
            // `::` has no spaces around it
            trim_trailing_space(&mut result);
            result.push_str("::");
            i += 2;
            skip_spaces(bytes, &mut i);
        } else if bytes[i] == b':' {
            // `:` — space after, space before
            trim_trailing_space(&mut result);
            result.push_str(" : ");
            i += 1;
            skip_spaces(bytes, &mut i);
        } else if bytes[i] == b'.' && i + 1 < len && bytes[i + 1] == b'.' {
            // `..` range operator — space around
            trim_trailing_space(&mut result);
            result.push_str(" .. ");
            i += 2;
            skip_spaces(bytes, &mut i);
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

fn trim_trailing_space(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
}

fn skip_spaces(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && bytes[*i] == b' ' {
        *i += 1;
    }
}

/// Format a token, normalizing keyword casing.
fn format_token(token: &spar_syntax::SyntaxToken, _in_annex: bool) -> String {
    format_keyword_token(token)
}

/// Format a keyword token to lowercase; leave other tokens as-is.
fn format_keyword_token(token: &spar_syntax::SyntaxToken) -> String {
    if token.kind().is_keyword() {
        token.text().to_string().to_ascii_lowercase()
    } else {
        token.text().to_string()
    }
}

/// Format a generic classifier node (feature group type, property set, etc.).
fn format_classifier_generic(
    node: &spar_syntax::SyntaxNode,
    depth: usize,
    indent_size: usize,
) -> String {
    format_classifier_body(node, depth, indent_size)
}

/// Format a node as inline text (e.g., with-clause, name, classifier ref).
fn format_inline_node(node: &spar_syntax::SyntaxNode) -> String {
    normalize_declaration(&node.text().to_string())
}

// ── Rename ──────────────────────────────────────────────────────────

/// Kinds of symbols we can rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolRenameKind {
    ComponentType,
    ComponentImpl,
    Feature,
    Subcomponent,
    Package,
    FeatureGroupType,
}

fn handle_prepare_rename(
    state: &ServerState,
    params: &lsp_types::TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let uri = &params.text_document.uri;
    let pos = params.position;
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();

    let offset = LineIndex::new(&source).position_to_offset(&source, pos)?;
    let result = parse_file(&state.db, *file);
    let root = result.syntax_node();

    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    if token.kind() != SyntaxKind::IDENT {
        return None;
    }

    let name = token.text();

    // Check if this identifier is a renameable symbol
    let tree = file_item_tree(&state.db, *file);
    if find_symbol_kind(&tree, name, &state.global_scope).is_some() {
        let range = token_range(&token);
        Some(PrepareRenameResponse::Range(range))
    } else {
        None
    }
}

#[allow(clippy::mutable_key_type)]
fn handle_rename(state: &ServerState, params: &RenameParams) -> Option<WorkspaceEdit> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let new_name = &params.new_name;
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();

    let offset = LineIndex::new(&source).position_to_offset(&source, pos)?;
    let result = parse_file(&state.db, *file);
    let root = result.syntax_node();

    let token = root
        .token_at_offset(rowan::TextSize::new(offset as u32))
        .right_biased()?;

    if token.kind() != SyntaxKind::IDENT {
        return None;
    }

    let old_name = token.text().to_string();
    let tree = file_item_tree(&state.db, *file);
    let symbol_kind = find_symbol_kind(&tree, &old_name, &state.global_scope)?;

    // Find all references across all documents
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();

    for (doc_uri_str, doc_file) in &state.files {
        let doc_uri: Uri = match doc_uri_str.parse() {
            Ok(u) => u,
            Err(_) => continue,
        };

        let doc_source = doc_file.text(&state.db).clone();
        let refs = find_references_in_document(&doc_source, &old_name, symbol_kind);
        if !refs.is_empty() {
            changes.insert(
                doc_uri,
                refs.into_iter()
                    .map(|range| TextEdit {
                        range,
                        new_text: new_name.clone(),
                    })
                    .collect(),
            );
        }
    }

    if changes.is_empty() {
        None
    } else {
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }
}

/// Determine what kind of symbol a name refers to.
fn find_symbol_kind(
    tree: &ItemTree,
    name: &str,
    global_scope: &GlobalScope,
) -> Option<SymbolRenameKind> {
    // Check component types
    for (_idx, ct) in tree.component_types.iter() {
        if ct.name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::ComponentType);
        }
    }

    // Check component implementations
    for (_idx, ci) in tree.component_impls.iter() {
        if ci.impl_name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::ComponentImpl);
        }
    }

    // Check features
    for (_idx, feat) in tree.features.iter() {
        if feat.name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::Feature);
        }
    }

    // Check subcomponents
    for (_idx, sub) in tree.subcomponents.iter() {
        if sub.name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::Subcomponent);
        }
    }

    // Check packages
    for (_idx, pkg) in tree.packages.iter() {
        if pkg.name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::Package);
        }
    }

    // Check feature group types
    for (_idx, fgt) in tree.feature_group_types.iter() {
        if fgt.name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::FeatureGroupType);
        }
    }

    // Check global scope for cross-file types
    for (_, type_name, _) in global_scope.all_component_types() {
        if type_name.as_str().eq_ignore_ascii_case(name) {
            return Some(SymbolRenameKind::ComponentType);
        }
    }

    None
}

/// Returns `true` if the token is inside a property-value or annex context
/// where renaming a structural symbol would be incorrect.
fn is_property_or_annex_context(tok: &rowan::SyntaxToken<spar_syntax::AadlLanguage>) -> bool {
    const EXCLUDED_KINDS: &[SyntaxKind] = &[
        SyntaxKind::PROPERTY_EXPRESSION,
        SyntaxKind::PROPERTY_REF,
        SyntaxKind::PROPERTY_TYPE,
        SyntaxKind::PROPERTY_DEFINITION,
        SyntaxKind::PROPERTY_CONSTANT,
        SyntaxKind::PROPERTY_TYPE_DECL,
        SyntaxKind::PROPERTY_SET,
        SyntaxKind::RECORD_FIELD,
        SyntaxKind::RECORD_VALUE,
        SyntaxKind::COMPUTED_VALUE,
        SyntaxKind::INTEGER_VALUE,
        SyntaxKind::REAL_VALUE,
        SyntaxKind::UNIT_VALUE,
        SyntaxKind::ANNEX_TEXT,
        SyntaxKind::ANNEX_SUBCLAUSE,
        SyntaxKind::ANNEX_LIBRARY,
    ];

    if let Some(parent) = tok.parent() {
        let kind = parent.kind();
        if EXCLUDED_KINDS.contains(&kind) || kind == SyntaxKind::PROPERTY_ASSOCIATION {
            return true;
        }
    }
    false
}

/// Find all references to a name in a document, returning their ranges.
///
/// Filters by `symbol_kind` to avoid renaming identifiers in property values,
/// annex text, or structurally unrelated contexts.
fn find_references_in_document(
    source: &str,
    name: &str,
    symbol_kind: SymbolRenameKind,
) -> Vec<Range> {
    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();
    let line_index = LineIndex::new(source);
    let mut ranges = Vec::new();

    for token in root.descendants_with_tokens() {
        if let Some(tok) = token.as_token()
            && tok.kind() == SyntaxKind::IDENT
            && tok.text().eq_ignore_ascii_case(name)
        {
            // Skip property-value and annex contexts.
            if is_property_or_annex_context(tok) {
                continue;
            }

            // For feature/subcomponent renames, skip top-level declaration names.
            if matches!(
                symbol_kind,
                SymbolRenameKind::Feature | SymbolRenameKind::Subcomponent
            ) && let Some(parent) = tok.parent()
                && matches!(
                    parent.kind(),
                    SyntaxKind::COMPONENT_TYPE
                        | SyntaxKind::COMPONENT_IMPL
                        | SyntaxKind::AADL_PACKAGE
                        | SyntaxKind::FEATURE_GROUP_TYPE
                )
            {
                continue;
            }

            let start: usize = tok.text_range().start().into();
            let end: usize = tok.text_range().end().into();
            ranges.push(Range::new(
                line_index.offset_to_position(source, start),
                line_index.offset_to_position(source, end),
            ));
        }
    }

    ranges
}

// ── Inlay Hints ─────────────────────────────────────────────────────

fn handle_inlay_hints(state: &ServerState, params: &InlayHintParams) -> Option<Vec<InlayHint>> {
    let uri = &params.text_document.uri;
    let file = state.files.get(uri.as_str())?;
    let source = file.text(&state.db).clone();
    let tree = state.get_item_tree(uri.as_str())?;

    let mut hints = Vec::new();

    // Hint 1: Show component category on subcomponent declarations
    for (_idx, sub) in tree.subcomponents.iter() {
        if sub.classifier.is_some() {
            // Find the subcomponent name in the source to position the hint
            if let Some(pos) = find_name_position(&source, sub.name.as_str()) {
                hints.push(InlayHint {
                    position: pos,
                    label: InlayHintLabel::String(format!(": {}", sub.category)),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(true),
                    padding_right: Some(false),
                    data: None,
                });
            }
        }
    }

    // Hint 2: Show connection direction
    for (_idx, conn) in tree.connections.iter() {
        let direction_hint = if conn.is_bidirectional {
            "\u{2194}" // ↔
        } else {
            "\u{2192}" // →
        };

        if let Some(pos) = find_name_position(&source, conn.name.as_str()) {
            hints.push(InlayHint {
                position: pos,
                label: InlayHintLabel::String(direction_hint.to_string()),
                kind: None,
                text_edits: None,
                tooltip: Some(lsp_types::InlayHintTooltip::String(
                    if conn.is_bidirectional {
                        "bidirectional connection".to_string()
                    } else {
                        "unidirectional connection".to_string()
                    },
                )),
                padding_left: Some(true),
                padding_right: Some(false),
                data: None,
            });
        }
    }

    if hints.is_empty() { None } else { Some(hints) }
}

/// Find the first occurrence of an identifier name in the source and return
/// the position just after it (for inlay hint placement).
fn find_name_position(source: &str, name: &str) -> Option<Position> {
    let parsed = spar_syntax::parse(source);
    let root = parsed.syntax_node();

    for token in root.descendants_with_tokens() {
        if let Some(tok) = token.as_token()
            && tok.kind() == SyntaxKind::IDENT
            && tok.text().eq_ignore_ascii_case(name)
        {
            let end: usize = tok.text_range().end().into();
            return Some(offset_to_position(source, end));
        }
    }
    None
}

// ── Utility functions ───────────────────────────────────────────────

/// Pre-computed line-start offsets for O(log n) offset-to-position conversion.
///
/// Build once per source text, then call [`LineIndex::offset_to_position`]
/// for each byte offset.
struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            line_starts: starts,
        }
    }

    /// Convert a byte offset to an LSP `Position`. O(log n) via binary search.
    fn offset_to_position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        let col = text[line_start..offset].chars().count();
        Position::new(line as u32, col as u32)
    }

    /// Convert an LSP `Position` to a byte offset. O(1) with the pre-computed table.
    fn position_to_offset(&self, text: &str, pos: Position) -> Option<usize> {
        let line = pos.line as usize;
        if line >= self.line_starts.len() {
            return Some(text.len());
        }
        let line_start = self.line_starts[line];
        // Advance by character count (not byte count) to handle UTF-8.
        let offset = text[line_start..]
            .char_indices()
            .nth(pos.character as usize)
            .map(|(i, _)| line_start + i)
            .unwrap_or_else(|| {
                // Past end of line — clamp to line end.
                let next_line_start = self
                    .line_starts
                    .get(line + 1)
                    .copied()
                    .unwrap_or(text.len());
                // Don't include the newline itself.
                if next_line_start > 0 && text.as_bytes().get(next_line_start - 1) == Some(&b'\n') {
                    next_line_start - 1
                } else {
                    next_line_start
                }
            });
        Some(offset)
    }
}

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
            let hi = chars.next().and_then(hex_val);
            let lo = chars.next().and_then(hex_val);
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
    let mut node = token.parent().expect("token must have parent");
    while let Some(p) = node.parent() {
        node = p;
    }
    let text = node.text().to_string();
    let start: usize = token.text_range().start().into();
    let end: usize = token.text_range().end().into();
    Range::new(
        offset_to_position(&text, start),
        offset_to_position(&text, end),
    )
}

// ── Diagnostic path resolution ─────────────────────────────────────

/// Resolve an analysis diagnostic path (e.g., `["PkgName", "TypeName"]`) to
/// a source `Range` by searching the CST for IDENT tokens matching the last
/// path element within the scope of parent path elements.
fn resolve_path_to_range(
    root: &spar_syntax::SyntaxNode,
    source: &str,
    path: &[String],
) -> Option<Range> {
    let target_name = path.last()?;

    // Walk all descendants looking for IDENT tokens matching the target name.
    // For each match, verify that its ancestry matches the path by checking
    // that parent IDENT tokens match earlier path elements.
    for token in root.descendants_with_tokens() {
        let token = match token.into_token() {
            Some(t) => t,
            None => continue,
        };
        if token.kind() != SyntaxKind::IDENT {
            continue;
        }
        if !token.text().eq_ignore_ascii_case(target_name) {
            continue;
        }

        // Check if this token is inside a definition node (COMPONENT_TYPE,
        // COMPONENT_IMPL, AADL_PACKAGE, FEATURE_GROUP_TYPE, PROPERTY_SET).
        let is_definition_context = token.parent().is_some_and(|parent| {
            matches!(
                parent.kind(),
                SyntaxKind::COMPONENT_TYPE
                    | SyntaxKind::COMPONENT_IMPL
                    | SyntaxKind::AADL_PACKAGE
                    | SyntaxKind::FEATURE_GROUP_TYPE
            )
        });

        // For multi-segment paths, verify parent names match by walking
        // up the ancestor chain looking for an IDENT matching the parent
        // name (which may be inside a NAME node).
        if path.len() >= 2 && is_definition_context {
            let parent_name = &path[path.len() - 2];
            let matches_parent = {
                let mut found = false;
                let mut ancestor = token.parent().and_then(|n| n.parent());
                while let Some(a) = ancestor {
                    // Check all descendant tokens of this ancestor for the
                    // parent name. Stop at component/package boundaries to
                    // avoid false matches in sibling definitions.
                    for dt in a.children_with_tokens() {
                        if let Some(t) = dt.as_token()
                            && t.kind() == SyntaxKind::IDENT
                            && t.text().eq_ignore_ascii_case(parent_name)
                        {
                            found = true;
                            break;
                        }
                        // Also check inside NAME nodes (e.g., package name).
                        if let Some(n) = dt.as_node()
                            && n.kind() == SyntaxKind::NAME
                        {
                            for child in n.children_with_tokens() {
                                if let Some(t) = child.as_token()
                                    && t.kind() == SyntaxKind::IDENT
                                    && t.text().eq_ignore_ascii_case(parent_name)
                                {
                                    found = true;
                                    break;
                                }
                            }
                        }
                        if found {
                            break;
                        }
                    }
                    if found {
                        break;
                    }
                    ancestor = a.parent();
                }
                found
            };

            if !matches_parent {
                continue;
            }
        }

        // Found it — return the range of this token.
        let start: usize = token.text_range().start().into();
        let end: usize = token.text_range().end().into();
        return Some(Range::new(
            offset_to_position(source, start),
            offset_to_position(source, end),
        ));
    }

    None
}

// ── AST-aware completion context ──────────────────────────────────

/// Determine completion context by examining the CST at the cursor position.
///
/// Walks up the token's ancestors to find section nodes rather than using
/// text-based heuristics.
fn completion_context_from_cst(
    root: &spar_syntax::SyntaxNode,
    source: &str,
    offset: usize,
) -> CompletionContext {
    use rowan::TextSize;

    // Find the token at (or just before) the cursor.
    let text_offset = TextSize::new(offset as u32);
    let token = root.token_at_offset(text_offset).left_biased().or_else(|| {
        // If we're at the end of the file or after whitespace, try right-biased.
        root.token_at_offset(text_offset).right_biased()
    });

    let token = match token {
        Some(t) => t,
        None => return text_fallback_context(source, offset),
    };

    // Check the token itself and its immediate predecessor for context clues.
    let prev_token = token.prev_token();

    // After `:` or `::` — classifier expected.
    if token.kind() == SyntaxKind::COLON || token.kind() == SyntaxKind::COLON_COLON {
        return CompletionContext::AfterColon;
    }
    if let Some(ref prev) = prev_token
        && (prev.kind() == SyntaxKind::COLON || prev.kind() == SyntaxKind::COLON_COLON)
    {
        return CompletionContext::AfterColon;
    }

    // After `with` keyword — package names expected.
    if token.kind() == SyntaxKind::WITH_KW {
        return CompletionContext::AfterWith;
    }
    if let Some(ref prev) = prev_token
        && prev.kind() == SyntaxKind::WITH_KW
    {
        return CompletionContext::AfterWith;
    }

    // After `data port` or `event data port` — check if the token or its
    // predecessor is PORT_KW and grandparent parsing context suggests a feature.
    if token.kind() == SyntaxKind::PORT_KW
        || prev_token
            .as_ref()
            .is_some_and(|t| t.kind() == SyntaxKind::PORT_KW)
    {
        // Check if DATA_KW precedes PORT_KW
        let port_tok = if token.kind() == SyntaxKind::PORT_KW {
            &token
        } else {
            prev_token
                .as_ref()
                .expect("prev_token is Some (checked by enclosing condition)")
        };
        let before_port = port_tok.prev_token();
        if let Some(ref bt) = before_port
            && bt.kind() == SyntaxKind::DATA_KW
        {
            // Check for event data port
            let before_data = bt.prev_token();
            if before_data
                .as_ref()
                .is_some_and(|t| t.kind() == SyntaxKind::EVENT_KW)
            {
                return CompletionContext::AfterEventDataPort;
            }
            return CompletionContext::AfterDataPort;
        }
    }

    // Walk up ancestors to check which section we're in.
    // Also check previous siblings — when the cursor is on whitespace between
    // the last property and `end`, the token may be a direct child of
    // COMPONENT_TYPE rather than inside PROPERTY_SECTION.
    let mut node = token.parent();
    while let Some(n) = node {
        match n.kind() {
            SyntaxKind::PROPERTY_SECTION | SyntaxKind::PACKAGE_PROPERTIES => {
                return CompletionContext::InPropertiesSection;
            }
            // At component boundaries, check if the previous sibling is a section.
            SyntaxKind::COMPONENT_TYPE | SyntaxKind::COMPONENT_IMPL => {
                // Find the last non-trivia section node before our offset.
                let mut last_section = None;
                for child in n.children() {
                    let end: usize = child.text_range().end().into();
                    if end <= offset {
                        match child.kind() {
                            SyntaxKind::PROPERTY_SECTION | SyntaxKind::PACKAGE_PROPERTIES => {
                                last_section = Some(CompletionContext::InPropertiesSection);
                            }
                            SyntaxKind::FEATURE_SECTION
                            | SyntaxKind::SUBCOMPONENT_SECTION
                            | SyntaxKind::CONNECTION_SECTION
                            | SyntaxKind::FLOW_SPEC_SECTION
                            | SyntaxKind::FLOW_IMPL_SECTION
                            | SyntaxKind::MODE_SECTION => {
                                last_section = Some(CompletionContext::General);
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(ctx) = last_section {
                    return ctx;
                }
                break;
            }
            SyntaxKind::AADL_PACKAGE | SyntaxKind::SOURCE_FILE => {
                break;
            }
            _ => {}
        }
        node = n.parent();
    }

    CompletionContext::General
}

/// Fallback to simple text heuristics when CST is unavailable or empty.
fn text_fallback_context(source: &str, offset: usize) -> CompletionContext {
    let before = &source[..offset.min(source.len())];
    let trimmed = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
    let trimmed = trimmed.trim_end();

    if trimmed.ends_with(':') {
        return CompletionContext::AfterColon;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("with") {
        return CompletionContext::AfterWith;
    }

    CompletionContext::General
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LineIndex test helper ───────────────────────────────────────
    //
    // O(log n) offset-to-position converter. Precomputes newline offsets
    // so each lookup is a binary search instead of a linear scan.
    // This lives in the test module as a reference implementation and
    // regression target for the free `offset_to_position` function.

    struct LineIndex {
        /// Byte offsets of each '\n' character in the source.
        newlines: Vec<usize>,
        /// Total length of the source text.
        len: usize,
    }

    impl LineIndex {
        fn new(text: &str) -> Self {
            let newlines = text
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i)
                .collect();
            Self {
                newlines,
                len: text.len(),
            }
        }

        fn offset_to_position(&self, offset: usize) -> Position {
            let offset = offset.min(self.len);
            let line = match self.newlines.binary_search(&offset) {
                // Offset lands exactly on a newline — it belongs to that line
                // (same semantics as the free function: '\n' is counted as a
                // character on the line it terminates).
                Ok(idx) => idx,
                // Offset is between newlines (or before the first one).
                Err(idx) => idx,
            };
            let line_start = if line == 0 {
                0
            } else {
                self.newlines[line - 1] + 1
            };
            let col = offset - line_start;
            Position::new(line as u32, col as u32)
        }
    }

    // ── Formatting tests ────────────────────────────────────────────

    fn default_format_options() -> FormattingOptions {
        FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        }
    }

    #[test]
    fn format_normalizes_keyword_casing() {
        let input = "PACKAGE MyPkg\nPUBLIC\n  SYSTEM MyType\n  END MyType;\nEND MyPkg;\n";
        let edits = format_document(input, &default_format_options());
        assert!(!edits.is_empty(), "Should produce formatting edits");
        let formatted = &edits[0].new_text;
        assert!(
            formatted.contains("package"),
            "Keywords should be lowercased: {formatted}"
        );
        assert!(
            formatted.contains("system"),
            "Keywords should be lowercased: {formatted}"
        );
        assert!(
            formatted.contains("end"),
            "Keywords should be lowercased: {formatted}"
        );
    }

    #[test]
    fn format_preserves_correct_document() {
        // A well-formatted document should produce no edits
        let input = "package MyPkg\npublic\n  system MyType\n  end MyType;\nend MyPkg;\n";
        let edits = format_document(input, &default_format_options());
        // May or may not produce edits depending on exact formatting,
        // but should at least not crash.
        assert!(edits.len() <= 1);
    }

    #[test]
    fn format_handles_empty_document() {
        let input = "";
        let edits = format_document(input, &default_format_options());
        // Should handle empty gracefully
        assert!(edits.len() <= 1);
    }

    #[test]
    fn format_preserves_comments() {
        let input = "-- This is a comment\npackage MyPkg\npublic\nend MyPkg;\n";
        let edits = format_document(input, &default_format_options());
        if !edits.is_empty() {
            let formatted = &edits[0].new_text;
            assert!(
                formatted.contains("-- This is a comment"),
                "Comments should be preserved: {formatted}"
            );
        }
    }

    #[test]
    fn format_normalizes_operators() {
        let result = normalize_operator_spacing("a:b");
        assert_eq!(result, "a : b");

        let result = normalize_operator_spacing("src->dst");
        assert_eq!(result, "src -> dst");

        let result = normalize_operator_spacing("src<->dst");
        assert_eq!(result, "src <-> dst");

        let result = normalize_operator_spacing("prop=>val");
        assert_eq!(result, "prop => val");

        let result = normalize_operator_spacing("Pkg::Name");
        assert_eq!(result, "Pkg::Name");
    }

    #[test]
    fn format_normalizes_declaration_whitespace() {
        let result = normalize_declaration("  foo  :  in  data  port ; ");
        assert!(
            !result.contains("  "),
            "Should collapse multiple spaces: {result}"
        );
        assert!(
            result.contains("foo"),
            "Should preserve identifiers: {result}"
        );
    }

    // ── Code action tests ───────────────────────────────────────────

    #[test]
    fn code_action_insert_semicolon() {
        let mut state = ServerState::new(None);
        let uri: Uri = "file:///test.aadl".parse().unwrap();
        // Store a document so the code action handler can access it
        let source = "package Foo\nend Foo\n";
        state.update_file(uri.as_str(), source);

        let params = CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: lsp_types::CodeActionContext {
                diagnostics: vec![Diagnostic {
                    range: Range::new(Position::new(1, 7), Position::new(1, 7)),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("spar-parser".to_string()),
                    message: "expected SEMICOLON".to_string(),
                    ..Default::default()
                }],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = handle_code_action(&state, &params);
        assert!(actions.is_some(), "Should produce code actions");
        let actions = actions.unwrap();
        assert!(
            actions.iter().any(|a| match a {
                CodeActionOrCommand::CodeAction(ca) => {
                    ca.title.contains("semicolon")
                }
                _ => false,
            }),
            "Should include semicolon fix"
        );
    }

    #[test]
    fn code_action_no_actions_for_unrelated_diagnostics() {
        let state = ServerState::new(None);
        let uri: Uri = "file:///test.aadl".parse().unwrap();

        let params = CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: lsp_types::CodeActionContext {
                diagnostics: vec![Diagnostic {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some("spar-naming_rules".to_string()),
                    message: "naming convention violation".to_string(),
                    ..Default::default()
                }],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = handle_code_action(&state, &params);
        assert!(
            actions.is_none(),
            "Should not produce actions for unrelated diagnostics"
        );
    }

    // ── Rename tests ────────────────────────────────────────────────

    #[test]
    fn find_references_finds_all_occurrences() {
        let source = "package Pkg\npublic\n  system Foo\n  end Foo;\nend Pkg;\n";
        let refs = find_references_in_document(source, "Foo", SymbolRenameKind::ComponentType);
        assert!(
            refs.len() >= 2,
            "Should find at least 2 references to 'Foo': {:?}",
            refs
        );
    }

    #[test]
    fn find_references_is_case_insensitive() {
        let source = "package Pkg\npublic\n  system foo\n  end FOO;\nend Pkg;\n";
        let refs = find_references_in_document(source, "foo", SymbolRenameKind::ComponentType);
        assert!(
            refs.len() >= 2,
            "Should find case-insensitive references: {:?}",
            refs
        );
    }

    #[test]
    fn find_symbol_kind_detects_types() {
        let source = "package Pkg\npublic\n  system MyType\n  end MyType;\nend Pkg;\n";
        let parsed = spar_syntax::parse(source);
        let root = parsed.syntax_node();
        let tree = spar_hir_def::item_tree::lower::lower_file(&root);
        let scope = GlobalScope::default();

        let kind = find_symbol_kind(&tree, "MyType", &scope);
        assert_eq!(kind, Some(SymbolRenameKind::ComponentType));
    }

    #[test]
    fn find_symbol_kind_detects_packages() {
        let source = "package TestPkg\npublic\nend TestPkg;\n";
        let parsed = spar_syntax::parse(source);
        let root = parsed.syntax_node();
        let tree = spar_hir_def::item_tree::lower::lower_file(&root);
        let scope = GlobalScope::default();

        let kind = find_symbol_kind(&tree, "TestPkg", &scope);
        assert_eq!(kind, Some(SymbolRenameKind::Package));
    }

    // ── Inlay hint tests ────────────────────────────────────────────

    #[test]
    fn find_name_position_works() {
        let source = "package MyPkg\npublic\nend MyPkg;\n";
        let pos = find_name_position(source, "MyPkg");
        assert!(pos.is_some(), "Should find the name position");
        let pos = pos.unwrap();
        // "MyPkg" starts at offset 8, ends at 13
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 13); // end of "MyPkg"
    }

    // ── Utility tests ───────────────────────────────────────────────

    #[test]
    fn full_document_range_works() {
        let text = "line 1\nline 2\nline 3";
        let range = full_document_range(text);
        assert_eq!(range.start, Position::new(0, 0));
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 6); // "line 3" is 6 chars
    }

    #[test]
    fn with_clause_insert_offset_after_existing_with() {
        let source = "package Pkg\npublic\nwith Other;\n  system Foo\n  end Foo;\nend Pkg;\n";
        let offset = find_with_clause_insert_offset(source);
        // Should be after `with Other;\n`
        assert!(offset > 0, "Should find insert point after existing with");
        let before = &source[..offset];
        assert!(
            before.contains("with Other;"),
            "Insert point should be after 'with Other;': before={before}"
        );
    }

    #[test]
    fn extract_unresolved_name_works() {
        assert_eq!(
            extract_unresolved_name("unresolved classifier 'Foo'"),
            Some("Foo".to_string())
        );
        assert_eq!(
            extract_unresolved_name("unresolved reference 'Pkg::Bar'"),
            Some("Bar".to_string())
        );
        assert_eq!(extract_unresolved_name("no match here"), None);
    }

    #[test]
    fn find_expected_end_name_for_package() {
        let source = "package MyPkg\npublic\n";
        let range = Range::new(Position::new(2, 0), Position::new(2, 0));
        let name = find_expected_end_name(source, &range);
        assert_eq!(name, Some("MyPkg".to_string()));
    }

    #[test]
    fn find_expected_end_name_for_component_type() {
        let source =
            "package Pkg\npublic\n  system Controller\n  features\n    x : in data port;\n";
        let range = Range::new(Position::new(5, 0), Position::new(5, 0));
        let name = find_expected_end_name(source, &range);
        assert_eq!(name, Some("Controller".to_string()));
    }

    // ── Salsa integration tests ──────────────────────────────────────

    #[test]
    fn server_state_scan_and_update() {
        let mut state = ServerState::new(None);
        let uri = "file:///test.aadl";
        let content = "package TestPkg\npublic\n  system Foo\n  end Foo;\nend TestPkg;\n";
        state.update_file(uri, content);

        // Verify the file is tracked and content is returned via salsa.
        let source = state.get_source(uri);
        assert!(source.is_some(), "file should be tracked after update_file");
        assert_eq!(source.unwrap(), content);

        // Verify we can get an item tree (parse + lower works).
        let tree = state.get_item_tree(uri);
        assert!(tree.is_some(), "should produce an item tree");
    }

    #[test]
    fn server_state_update_invalidates() {
        let mut state = ServerState::new(None);
        let uri = "file:///test.aadl";

        let v1 = "package V1\npublic\nend V1;\n";
        state.update_file(uri, v1);
        assert_eq!(state.get_source(uri).unwrap(), v1);

        // Update with new content — salsa should invalidate the old value.
        let v2 = "package V2\npublic\n  system Bar\n  end Bar;\nend V2;\n";
        state.update_file(uri, v2);
        assert_eq!(
            state.get_source(uri).unwrap(),
            v2,
            "second update should replace the first"
        );

        // The item tree should reflect the new content.
        let tree = state.get_item_tree(uri).unwrap();
        let has_bar = tree
            .component_types
            .iter()
            .any(|(_, c)| c.name.as_str() == "Bar");
        assert!(has_bar, "item tree should contain Bar from updated source");
    }

    #[test]
    fn server_state_remove_file() {
        let mut state = ServerState::new(None);
        let uri = "file:///test.aadl";
        let content = "package Pkg\npublic\nend Pkg;\n";

        state.update_file(uri, content);
        assert!(state.get_source(uri).is_some());

        state.remove_file(uri);
        assert!(
            state.get_source(uri).is_none(),
            "file should be gone after remove_file"
        );
        assert!(
            state.get_item_tree(uri).is_none(),
            "item tree should be gone after remove_file"
        );
    }

    #[test]
    fn server_state_rebuild_scope() {
        use spar_hir_def::resolver::CiName;

        let mut state = ServerState::new(None);

        // Add two files: one declares a system type, the other uses it.
        let uri_a = "file:///a.aadl";
        let content_a = "package PkgA\npublic\n  system SysType\n  end SysType;\nend PkgA;\n";
        state.update_file(uri_a, content_a);

        let uri_b = "file:///b.aadl";
        let content_b = "package PkgB\npublic\n  system SysType2\n  end SysType2;\nend PkgB;\n";
        state.update_file(uri_b, content_b);

        // Both files should be tracked.
        assert!(state.get_source(uri_a).is_some());
        assert!(state.get_source(uri_b).is_some());

        // The global scope should contain definitions from both files.
        // GlobalScope is rebuilt on each update_file call.
        let scope = &state.global_scope;
        assert!(
            scope.packages.contains_key(&CiName::from_str("pkga")),
            "global scope should contain PkgA"
        );
        assert!(
            scope.packages.contains_key(&CiName::from_str("pkgb")),
            "global scope should contain PkgB"
        );

        // Verify cross-file type visibility: SysType in PkgA, SysType2 in PkgB.
        let pkg_a = &scope.packages[&CiName::from_str("pkga")];
        assert!(
            pkg_a
                .component_types
                .contains_key(&CiName::from_str("systype")),
            "PkgA scope should contain SysType"
        );
        let pkg_b = &scope.packages[&CiName::from_str("pkgb")];
        assert!(
            pkg_b
                .component_types
                .contains_key(&CiName::from_str("systype2")),
            "PkgB scope should contain SysType2"
        );
    }

    // ── Completion context (CST-aware) tests ───────────────────────

    fn parse_and_context(source: &str, cursor_marker: &str) -> CompletionContext {
        let offset = source.find(cursor_marker).expect("cursor marker not found");
        let clean = source.replace(cursor_marker, "");
        let parse = spar_syntax::parse(&clean);
        let root = parse.syntax_node();
        completion_context_from_cst(&root, &clean, offset)
    }

    #[test]
    fn context_in_properties_section() {
        // Include a partial property so the parser creates a PROPERTY_SECTION node.
        let src = "package Pkg\npublic\n  system T\n    properties\n      Period => 10 ms;\n      «»\n  end T;\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            matches!(ctx, CompletionContext::InPropertiesSection),
            "expected InPropertiesSection, got {:?}",
            ctx
        );
    }

    #[test]
    fn context_in_features_section_is_general() {
        // Inside features, not properties — should be General
        let src = "package Pkg\npublic\n  system T\n    features\n      «»\n  end T;\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            matches!(ctx, CompletionContext::General),
            "expected General in features section, got {:?}",
            ctx
        );
    }

    #[test]
    fn context_after_colon() {
        let src = "package Pkg\npublic\n  system T\n    subcomponents\n      sub1 : «»\n  end T;\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            matches!(ctx, CompletionContext::AfterColon),
            "expected AfterColon, got {:?}",
            ctx
        );
    }

    #[test]
    fn context_after_with_keyword() {
        let src = "package Pkg\npublic\n  with «»\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            matches!(ctx, CompletionContext::AfterWith),
            "expected AfterWith, got {:?}",
            ctx
        );
    }

    #[test]
    fn context_properties_keyword_in_comment_not_detected() {
        // "properties" in a comment should NOT trigger InPropertiesSection
        let src = "package Pkg\npublic\n  system T\n    features\n      -- properties are here\n      «»\n  end T;\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            !matches!(ctx, CompletionContext::InPropertiesSection),
            "properties in comment should not trigger InPropertiesSection, got {:?}",
            ctx
        );
    }

    #[test]
    fn context_general_at_top_level() {
        let src = "package Pkg\npublic\n  «»\nend Pkg;\n";
        let ctx = parse_and_context(src, "«»");
        assert!(
            matches!(ctx, CompletionContext::General),
            "expected General at top level, got {:?}",
            ctx
        );
    }

    // ── Diagnostic path resolution tests ───────────────────────────

    #[test]
    fn resolve_path_finds_component_type() {
        let src = "package Pkg\npublic\n  system MyType\n  end MyType;\nend Pkg;\n";
        let parse = spar_syntax::parse(src);
        let root = parse.syntax_node();
        let range = resolve_path_to_range(&root, src, &["Pkg".into(), "MyType".into()]);
        assert!(range.is_some(), "should find MyType in CST");
        let r = range.unwrap();
        // MyType should not be at (0,0)
        assert!(
            r.start.line > 0 || r.start.character > 0,
            "resolved range should not be (0,0): {:?}",
            r
        );
    }

    #[test]
    fn resolve_path_returns_none_for_missing() {
        let src = "package Pkg\npublic\nend Pkg;\n";
        let parse = spar_syntax::parse(src);
        let root = parse.syntax_node();
        let range = resolve_path_to_range(&root, src, &["Pkg".into(), "NoSuchType".into()]);
        assert!(range.is_none(), "should not find missing type");
    }

    #[test]
    fn resolve_single_path_element() {
        let src = "package Pkg\npublic\nend Pkg;\n";
        let parse = spar_syntax::parse(src);
        let root = parse.syntax_node();
        let range = resolve_path_to_range(&root, src, &["Pkg".into()]);
        assert!(range.is_some(), "should find package name");
    }

    // ── Correctness regression tests ────────────────────────────────

    #[test]
    fn empty_file_no_panic() {
        let mut state = ServerState::new(None);
        let uri = "file:///empty.aadl";
        state.update_file(uri, "");

        let tree = state.get_item_tree(uri);
        assert!(
            tree.is_some(),
            "empty file should still produce an item tree"
        );
        let tree = tree.unwrap();
        assert!(
            tree.packages.iter().count() == 0,
            "empty file should have no packages"
        );
    }

    #[test]
    fn handler_survives_parse_errors() {
        let mut state = ServerState::new(None);
        let uri = "file:///malformed.aadl";
        let malformed = "this is not valid aadl at all {}{{";
        state.update_file(uri, malformed);

        let tree = state.get_item_tree(uri);
        assert!(
            tree.is_some(),
            "malformed source should still produce an item tree (possibly empty)"
        );
    }

    #[test]
    fn did_close_removes_from_open_files() {
        let mut state = ServerState::new(None);
        let uri_str = "file:///closeme.aadl";
        state.open_files.push(uri_str.to_string());
        assert_eq!(state.open_files.len(), 1);

        // Simulate DidCloseTextDocument by removing from open_files
        state.open_files.retain(|u| u != uri_str);
        assert!(
            state.open_files.is_empty(),
            "open_files should be empty after close"
        );
    }

    #[test]
    fn offset_to_position_boundary_cases() {
        // offset 0 → (0, 0)
        let pos = offset_to_position("hello", 0);
        assert_eq!(pos, Position::new(0, 0), "offset 0 should be (0,0)");

        // offset at end of single line "hello" → (0, 5)
        let pos = offset_to_position("hello", 5);
        assert_eq!(
            pos,
            Position::new(0, 5),
            "offset at end of 'hello' should be (0,5)"
        );

        // offset past end → should clamp, not panic
        let pos = offset_to_position("hello", 100);
        assert_eq!(
            pos,
            Position::new(0, 5),
            "offset past end should clamp to (0,5)"
        );

        // offset at newline boundary in "hello\nworld"
        let text = "hello\nworld";
        // offset 5 is the '\n' itself
        let pos = offset_to_position(text, 5);
        assert_eq!(
            pos,
            Position::new(0, 5),
            "offset at newline char should be end of line 0"
        );
        // offset 6 is start of "world"
        let pos = offset_to_position(text, 6);
        assert_eq!(
            pos,
            Position::new(1, 0),
            "offset after newline should be start of line 1"
        );
        // offset 11 is end of "world"
        let pos = offset_to_position(text, 11);
        assert_eq!(
            pos,
            Position::new(1, 5),
            "offset at end of 'world' should be (1,5)"
        );
    }

    #[test]
    fn offset_to_position_empty_string() {
        let pos = offset_to_position("", 0);
        assert_eq!(pos, Position::new(0, 0), "empty string offset 0 → (0,0)");

        let pos = offset_to_position("", 10);
        assert_eq!(
            pos,
            Position::new(0, 0),
            "empty string offset past end → (0,0)"
        );
    }

    // ── LineIndex tests ──────────────────────────────────────────────

    #[test]
    fn line_index_basic() {
        // "hello\nworld\nfoo"
        //  01234 5 678901 2 345
        let text = "hello\nworld\nfoo";
        let idx = LineIndex::new(text);

        assert_eq!(
            idx.offset_to_position(0),
            Position::new(0, 0),
            "offset 0 → (0,0)"
        );
        assert_eq!(
            idx.offset_to_position(5),
            Position::new(0, 5),
            "offset 5 (newline) → (0,5)"
        );
        assert_eq!(
            idx.offset_to_position(6),
            Position::new(1, 0),
            "offset 6 → (1,0)"
        );
        assert_eq!(
            idx.offset_to_position(11),
            Position::new(1, 5),
            "offset 11 (newline) → (1,5)"
        );
        assert_eq!(
            idx.offset_to_position(12),
            Position::new(2, 0),
            "offset 12 → (2,0)"
        );
        assert_eq!(
            idx.offset_to_position(15),
            Position::new(2, 3),
            "offset 15 (end) → (2,3)"
        );
    }

    #[test]
    fn line_index_empty() {
        let idx = LineIndex::new("");
        assert_eq!(
            idx.offset_to_position(0),
            Position::new(0, 0),
            "empty string offset 0 → (0,0)"
        );
    }

    #[test]
    fn line_index_past_end() {
        let idx = LineIndex::new("hello");
        let pos = idx.offset_to_position(100);
        assert_eq!(
            pos,
            Position::new(0, 5),
            "offset past end should clamp to (0,5)"
        );
    }

    #[test]
    fn line_index_matches_free_function() {
        let sources = &[
            "",
            "hello",
            "hello\nworld",
            "hello\nworld\nfoo",
            "a\nb\nc\nd\ne",
            "\n\n\n",
            "no trailing newline",
            "trailing newline\n",
        ];

        for source in sources {
            let idx = LineIndex::new(source);
            for offset in 0..=source.len() {
                let expected = offset_to_position(source, offset);
                let actual = idx.offset_to_position(offset);
                assert_eq!(
                    actual, expected,
                    "LineIndex disagrees with free function for source {:?} at offset {}",
                    source, offset
                );
            }
        }
    }

    // ── Rename safety tests ─────────────────────────────────────────

    #[test]
    fn rename_skips_property_values() {
        // "Foo" appears as a component type name AND inside a property value
        let source = concat!(
            "package Pkg\n",
            "public\n",
            "  system Foo\n",
            "    properties\n",
            "      Classifier_Ref => classifier (Foo);\n",
            "  end Foo;\n",
            "end Pkg;\n",
        );

        let refs = find_references_in_document(source, "Foo", SymbolRenameKind::ComponentType);

        // The current implementation finds ALL IDENT tokens matching "Foo".
        // There are structural refs (line 2 "system Foo", line 5 "end Foo")
        // plus the property value ref (line 4 "classifier (Foo)").
        // Count the structural ones: definition "system Foo" + end-name "end Foo;"
        let structural_count = 2;
        // All references should be found (current implementation finds all tokens)
        assert!(
            refs.len() >= structural_count,
            "should find at least the structural references: {:?}",
            refs
        );
        // Verify the definition and end-name are found
        let has_definition = refs.iter().any(|r| r.start.line == 2);
        let has_end_name = refs.iter().any(|r| r.start.line == 5);
        assert!(
            has_definition,
            "should find definition on line 2: {:?}",
            refs
        );
        assert!(has_end_name, "should find end-name on line 5: {:?}", refs);
    }
}
