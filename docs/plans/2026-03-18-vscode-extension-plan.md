# VS Code Extension for AADL Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a VS Code extension (`pulseengine.spar-aadl`) with AADL syntax highlighting, LSP-based editing, and live interactive architecture diagrams powered by spar-wasm.

**Architecture:** Extension spawns native `spar lsp` for editing intelligence. spar-wasm (jco-transpiled to JS) runs in-process for diagram rendering. WASI filesystem calls are shimmed to read `.aadl` files from the VS Code workspace. Webview panel shows interactive HTML (etch output with ports, orthogonal routing, pan/zoom/selection).

**Tech Stack:** TypeScript, VS Code Extension API, vscode-languageclient, jco (bytecodealliance), Rust (spar-wasm upgrade), @vscode/test-electron

**Spec:** `docs/plans/2026-03-18-vscode-extension-design.md`

---

## Chunk 1: Upgrade spar-wasm to use etch rendering

### Task 1: Wire spar-render into spar-wasm

**Files:**
- Modify: `crates/spar-wasm/Cargo.toml`
- Modify: `crates/spar-wasm/src/render.rs`

Currently `spar-wasm/src/render.rs` has its own inline SVG renderer. Replace with calls to `spar_render::render_instance_html()` for interactive HTML output.

- [ ] **Step 1: Add spar-render and etch deps to spar-wasm/Cargo.toml**
- [ ] **Step 2: Replace render_aadl_from_fs() to use spar_render::render_instance_html()**
- [ ] **Step 3: Replace render_aadl() string-based function similarly**
- [ ] **Step 4: Remove inline render_graph_to_svg() and helpers**
- [ ] **Step 5: Build:** `cargo build --target wasm32-wasip2 -p spar-wasm --release`
- [ ] **Step 6: Update tests** (output is now HTML not bare SVG)
- [ ] **Step 7: Run:** `cargo test -p spar-wasm`
- [ ] **Step 8: Commit**

---

## Chunk 2: VS Code Extension Scaffold + TextMate Grammar

### Task 2: Create extension project

**Files:**
- Create: `vscode-spar/package.json` (manifest with language, grammar, commands, config)
- Create: `vscode-spar/tsconfig.json`
- Create: `vscode-spar/src/extension.ts` (minimal activate/deactivate)

- [ ] **Step 1: Create package.json** with language contribution for `.aadl`, commands `spar.showDiagram` and `spar.selectRoot`, config `spar.binaryPath`
- [ ] **Step 2: Create tsconfig.json** (commonjs, ES2022, outDir: out)
- [ ] **Step 3: Create minimal extension.ts** (activation message only)
- [ ] **Step 4: npm install && npm run compile**
- [ ] **Step 5: Commit**

### Task 3: TextMate grammar + language configuration

**Files:**
- Create: `vscode-spar/syntaxes/aadl.tmLanguage.json`
- Create: `vscode-spar/language-configuration.json`

- [ ] **Step 1: Create aadl.tmLanguage.json** covering keywords, categories, features, connections, properties, comments, strings, numbers, annexes
- [ ] **Step 2: Create language-configuration.json** (comment toggling `--`, brackets, auto-closing, indentation rules)
- [ ] **Step 3: F5 debug test** — open .aadl file, verify syntax coloring
- [ ] **Step 4: Commit**

---

## Chunk 3: LSP Client

### Task 4: Connect to spar LSP

**Files:**
- Modify: `vscode-spar/src/extension.ts`

- [ ] **Step 1: Add LSP client code**

Use `vscode-languageclient`. Binary discovery: check `spar.binaryPath` setting, then PATH via `execFileSync('which', ['spar'])` (safe, no shell injection).

```typescript
import { LanguageClient, ServerOptions, LanguageClientOptions, TransportKind } from 'vscode-languageclient/node';
```

ServerOptions: `{ command: sparPath, args: ['lsp'], transport: TransportKind.stdio }`

- [ ] **Step 2: F5 test** — open .aadl, verify diagnostics/hover/completion
- [ ] **Step 3: Commit**

---

## Chunk 4: WASM Renderer + WASI Shim

### Task 5: Build + transpile WASM

**Files:**
- Create: `vscode-spar/scripts/build-wasm.sh`

- [ ] **Step 1: Write build script** — `cargo build --target wasm32-wasip2` + `jco transpile --instantiation async`
- [ ] **Step 2: Run script, verify assets/js/ output**
- [ ] **Step 3: Add assets/ to .gitignore** (generated)
- [ ] **Step 4: Commit**

### Task 6: WASI filesystem shim

**Files:**
- Create: `vscode-spar/src/wasi-shim.ts`

Read the jco-generated `spar_wasm.d.ts` to understand exact import signatures. Implement:
- `wasi:filesystem/preopens.getDirectories()` — virtual root
- `wasi:filesystem/types.Descriptor.readDirectory()` — workspace `.aadl` files
- `wasi:filesystem/types.Descriptor.openAt()` + `readViaStream()` — file content from VS Code docs
- `wasi:io/streams.InputStream.blockingRead()` — content bytes
- Stubs for: `wasi:cli/*`, `wasi:clocks/*`, `wasi:random/*`

File content: open docs via `TextDocument.getText()`, closed files via `workspace.fs.readFile()`.

- [ ] **Step 1: Implement shim matching jco import contract**
- [ ] **Step 2: Test with simple WASM instantiation**
- [ ] **Step 3: Commit**

### Task 7: WASM renderer module

**Files:**
- Create: `vscode-spar/src/wasm-renderer.ts`

- [ ] **Step 1: Create WasmRenderer class** — loads jco module, instantiates with shim, exposes async `render(root, highlight)` and `analyze(root)`
- [ ] **Step 2: Add error handling** — catch instantiation failure, show fallback message
- [ ] **Step 3: Wire into extension.ts activate()**
- [ ] **Step 4: Commit**

---

## Chunk 5: Diagram Webview + Integration

### Task 8: Diagram panel

**Files:**
- Create: `vscode-spar/src/diagram-panel.ts`
- Modify: `vscode-spar/src/extension.ts`

- [ ] **Step 1: Create DiagramPanel class** — manages WebviewPanel, `show()`, `update(html)`, handles dispose
- [ ] **Step 2: Register `spar.showDiagram` command** — opens panel, renders current root
- [ ] **Step 3: Wire onDidSaveTextDocument** — debounce 300ms, re-render all workspace files, update panel
- [ ] **Step 4: Implement root selection** — status bar item, QuickPick, auto-detect `system implementation`, persist in workspaceState
- [ ] **Step 5: F5 end-to-end test** — open .aadl → show diagram → edit → save → verify update
- [ ] **Step 6: Commit**

---

## Chunk 6: Testing + Release

### Task 9: Extension tests

**Files:**
- Create: `vscode-spar/src/test/runTest.ts`
- Create: `vscode-spar/src/test/suite/index.ts`
- Create: `vscode-spar/src/test/suite/extension.test.ts`

- [ ] **Step 1: Set up @vscode/test-electron runner**
- [ ] **Step 2: Write tests:** extension activates on .aadl, commands registered, language ID correct, WASM renderer initializes
- [ ] **Step 3: Run:** `npm test`
- [ ] **Step 4: Commit**

### Task 10: Build pipeline + release

**Files:**
- Create: `vscode-spar/bin/esbuild.js` (bundle, copy WASM assets)
- Create: `vscode-spar/.vscodeignore`
- Modify: `.github/workflows/release.yml` (add vsix build job)

- [ ] **Step 1: Create esbuild bundler** (external WASM files, copy to out/)
- [ ] **Step 2: Create .vscodeignore** (exclude src/, node_modules/, scripts/)
- [ ] **Step 3: Add vsix job to release.yml** — install node, build wasm, compile TS, `vsce package`
- [ ] **Step 4: Test:** `npx vsce package` → produces `spar-aadl-0.2.0.vsix`
- [ ] **Step 5: Commit**

### Task 11: Version bump + tag v0.2.0

- [ ] **Step 1: Bump workspace version to 0.2.0** in root Cargo.toml
- [ ] **Step 2: Update rivet.yaml version**
- [ ] **Step 3: Commit, push, create PR, merge**
- [ ] **Step 4: Tag v0.2.0, push tag** → triggers release (binaries + vsix + test evidence)
