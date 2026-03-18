# VS Code Extension for AADL (v0.2.0) Design

**Date:** 2026-03-18
**Status:** Approved
**Scope:** New VS Code extension `pulseengine.spar-aadl` providing AADL editing, analysis, and live architecture visualization

## Problem

AADL engineers currently use OSATE (Eclipse-based, heavy) or the minimal Ellidiss VS Code extension (stale, no analysis, no visualization). Spar already has a full LSP server (10 IDE features), 21 analysis passes, and interactive HTML rendering with ports and orthogonal routing — but no VS Code integration to bring it to engineers.

## Approach

Single VS Code extension combining:
- **TextMate grammar** for immediate syntax highlighting
- **Native LSP** for editing intelligence (all 10 existing features)
- **WASM renderer** via jco-transpiled spar-wasm for in-process diagram generation
- **Webview panel** showing interactive architecture diagram, live-updating on save

The WASM path reuses the existing `spar-wasm` component (wasm32-wasip2), transpiled by jco. WASI filesystem calls are shimmed to read all `.aadl` files from the VS Code workspace — same pattern as rivet's dashboard.

## Design

### Extension Structure

```
vscode-spar/
  package.json              # Extension manifest, language contribution, commands
  tsconfig.json
  src/
    extension.ts            # activate(), LSP client, webview lifecycle, WASM loading
    wasi-shim.ts            # Maps WASI fs to VS Code workspace documents
    diagram-panel.ts        # WebviewPanel management, postMessage protocol
  syntaxes/
    aadl.tmLanguage.json    # TextMate grammar for AADL syntax highlighting
  assets/
    spar_wasm.js            # jco-transpiled (generated at build time)
    spar_wasm.core.wasm     # Core WASM modules
    spar_wasm.core2.wasm
    spar_wasm.d.ts          # TypeScript types
  scripts/
    build-wasm.sh           # cargo build --target wasm32-wasip2 + jco transpile
  bin/
    esbuild.js              # Bundle for distribution
```

### TextMate Grammar

AADL syntax highlighting via `syntaxes/aadl.tmLanguage.json`:

- Keywords: `package`, `system`, `process`, `thread`, `device`, `bus`, `memory`, `processor`, `data`, `subprogram`, `implementation`, `end`, `with`, `extends`, `public`, `private`, `applies to`
- Sections: `features`, `subcomponents`, `connections`, `properties`, `flows`, `modes`
- Feature keywords: `in`, `out`, `inout`, `data port`, `event port`, `event data port`, `access`, `feature group`
- Connections: `->`, `<->`, `port`, `bus access`, `feature group`
- Properties: `=>` assignment, `::` property set access
- Modes: `initial mode`, `mode transition`
- Comments: `--` line comments
- Strings: `"..."` literals
- Numbers: integers, reals, based literals (`16#FF#`)
- Annexes: `{** ... **}` blocks

**Language configuration** (`language-configuration.json`):
- Comment toggling: `--` line comments
- Brackets: `()`, `{** **}`, `[ ]`
- Auto-closing pairs: `()`, `""`, `{** **}`
- Indentation rules: increase after `features`, `subcomponents`, `connections`, `properties`, `flows`, `modes`; decrease at `end`
- Word pattern: AADL identifiers (letters, digits, underscores)

Language contribution in `package.json`:
```json
{
  "languages": [{
    "id": "aadl",
    "extensions": [".aadl"],
    "aliases": ["AADL", "aadl"],
    "configuration": "./language-configuration.json"
  }],
  "grammars": [{
    "language": "aadl",
    "scopeName": "source.aadl",
    "path": "./syntaxes/aadl.tmLanguage.json"
  }]
}
```

### LSP Client

Spawns native `spar lsp` binary as a child process:

```typescript
const serverOptions: ServerOptions = {
  command: sparBinaryPath,
  args: ['lsp'],
  transport: TransportKind.stdio,
};

const clientOptions: LanguageClientOptions = {
  documentSelector: [{ scheme: 'file', language: 'aadl' }],
  synchronize: {
    fileEvents: workspace.createFileSystemWatcher('**/*.aadl'),
  },
};
```

**Binary discovery:** Check (in order):
1. `spar.binaryPath` setting (user-configured)
2. `spar` on PATH
3. Bundled binary in extension's `bin/` directory

All 10 LSP features activate automatically: diagnostics, hover, go-to-definition, completion, document symbols, workspace symbols, code actions, formatting, rename, inlay hints.

### WASM Renderer

**Prerequisite:** Wire `spar-render` and `etch` into `spar-wasm` so the WIT `render()` function returns interactive HTML (with ports, orthogonal routing, pan/zoom/selection) instead of the current plain SVG. This requires:
- Add `spar-render` and `etch` as dependencies of `spar-wasm`
- Replace `spar-wasm/src/render.rs`'s inline SVG renderer with calls to `spar_render::render_instance_html()`
- Confirm `etch` compiles cleanly to `wasm32-wasip2` (petgraph + etch have no platform-specific code — expected to work)

The upgraded `spar-wasm` component (wasm32-wasip2) is transpiled by jco:

```bash
cargo build --target wasm32-wasip2 -p spar-wasm --release
jco transpile --instantiation async spar_wasm.wasm -o assets/
```

**WASI filesystem shim** (`wasi-shim.ts`):

The jco transpiled output requires implementations for multiple WASI interfaces (filesystem, I/O streams, clocks, CLI, random). Most can be stubs. The critical implementations:

- `wasi:filesystem/preopens` — `getDirectories()` returns virtual root
- `wasi:filesystem/types` — `Descriptor.readDirectory()` returns all `.aadl` files from `workspace.findFiles('**/*.aadl')`, `Descriptor.openAt()` + `readViaStream()` returns file content
- `wasi:io/streams` — `InputStream.blockingRead()` delivers file content bytes
- `wasi:cli/stderr`, `stdout`, `stdin` — stubs (write to output channel for debug)
- `wasi:clocks`, `wasi:random` — simple stubs

File content sources (save-only — consistent with diagram updating on save):
- For open documents: `TextDocument.getText()` (includes unsaved edits at save time)
- For closed files: `workspace.fs.readFile()` (disk content)

The shim pattern is proven — rivet already does this for its dashboard (see `/Volumes/Home/git/sdlc/scripts/build-wasm.sh` and the transpiled output at `rivet-cli/assets/wasm/js/`).

**Rendering call:**
```typescript
const result = await sparWasm.render(rootClassifier, highlightIds);
// result is interactive HTML (etch output with pan/zoom/selection)
diagramPanel.webview.postMessage({ type: 'update', html: result });
```

**Error handling:**
- WASM instantiation failure → show error notification, offer fallback to `spar render` binary
- Render error (parse error, no root) → show error message in webview panel with diagnostic details
- Memory limits → note in docs that very large models (500+ components) may need the native binary path

**Debounce:** Re-render is debounced at 300ms to prevent rapid-fire updates on burst saves.

### Diagram Webview Panel

Opens beside the active editor:

```typescript
const panel = vscode.window.createWebviewPanel(
  'aadlDiagram',
  `AADL: ${rootName}`,
  vscode.ViewColumn.Beside,
  { enableScripts: true, retainContextWhenHidden: true }
);
```

**Features:**
- Interactive HTML from etch (pan/zoom/selection/semantic zoom — already built)
- Updates on any `.aadl` file save (all workspace files fed to WASM renderer, debounced 300ms)
- Status bar item showing current root classifier with picker
- Diagram-to-code navigation deferred to follow-up (requires source location mapping not yet in spar-wasm)

**Update protocol:**
- Extension posts `{ type: 'update', html }` to webview
- Webview posts `{ type: 'select', ids }` back on node selection
- Webview posts `{ type: 'ready' }` on initial load

### Root Classifier Selection

The extension needs to know which system implementation to render.

**Auto-detection:** On activation, scan workspace `.aadl` files for `system implementation` declarations. If exactly one found, use it. If multiple, prompt user.

**Manual selection:** Status bar item `$(circuit-board) RenderTest::Top.Impl` — click opens QuickPick with all system implementations found in workspace.

**Persistence:** Store last selection in `context.workspaceState`.

### package.json Manifest

```json
{
  "name": "spar-aadl",
  "displayName": "AADL (spar)",
  "description": "AADL v2.2 language support with live architecture visualization",
  "publisher": "pulseengine",
  "version": "0.2.0",
  "engines": { "vscode": "^1.100.0" },
  "categories": ["Programming Languages", "Visualization"],
  "activationEvents": ["onLanguage:aadl"],
  "main": "./out/extension",
  "contributes": {
    "languages": [{ "id": "aadl", "extensions": [".aadl"] }],
    "grammars": [{ "language": "aadl", "scopeName": "source.aadl", "path": "./syntaxes/aadl.tmLanguage.json" }],
    "commands": [
      { "command": "spar.showDiagram", "title": "Show Architecture Diagram", "category": "AADL" },
      { "command": "spar.selectRoot", "title": "Select Root System", "category": "AADL" }
    ],
    "configuration": {
      "title": "AADL (spar)",
      "properties": {
        "spar.binaryPath": { "type": "string", "description": "Path to spar binary" }
      }
    }
  },
  "dependencies": {
    "vscode-languageclient": "^9.0.0"
  }
}
```

### Multi-File Workspace Support

AADL models span multiple files. Every change must consider the full workspace:

- The WASI shim provides ALL `.aadl` files on every render call
- spar-wasm internally builds a GlobalScope from all files, resolves cross-file references
- On any `.aadl` save, the full render pipeline runs (parse all → instantiate root → render)
- Performance: typical AADL projects (10-50 files) render in <100ms; salsa incremental computation in WASM is a future optimization, not v0.2.0 scope

### Build and Distribution

**Build pipeline:**
1. `cargo build --target wasm32-wasip2 -p spar-wasm --release` — build WASM component
2. `jco transpile --instantiation async spar_wasm.wasm -o assets/` — transpile for browser
3. `npm run compile` — compile TypeScript
4. `npm run esbuild` — bundle extension
5. `vsce package` — create .vsix

**Release assets (added to spar release workflow):**
- `spar-aadl-{version}.vsix` — VS Code extension package
- Published to VS Code Marketplace and Open VSX Registry

### Testing

**Unit tests (TypeScript):**
- WASI shim correctly maps workspace files
- Root auto-detection finds system implementations
- Webview postMessage protocol

**Integration tests (VS Code test runner):**
- Extension activates on .aadl file
- LSP provides diagnostics
- Diagram webview opens and renders
- File save triggers diagram update

**E2E (Playwright):**
- Reuse existing rendering tests for HTML output quality

## Future Enhancements (not v0.2.0)

- Salsa incremental computation in WASM for large projects
- WASM LSP server (no native binary needed, works on vscode.dev)
- Bidirectional navigation: click diagram node → jump to source location (requires source span mapping in spar-wasm)
- Live-as-you-type diagram updates (`onDidChangeTextDocument` with debounce)
- EMV2 fault tree visualization in separate webview
- CodeLens showing analysis results inline
- Multi-root workspace support (scan all workspace folders for .aadl)
