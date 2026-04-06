import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { execFileSync } from 'child_process';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';
import { VirtualFs, buildWasiImports } from './wasi-shim';

let client: LanguageClient | undefined;
let diagramPanel: vscode.WebviewPanel | undefined;
let rootClassifier: string | undefined;
let statusBarItem: vscode.StatusBarItem;
let renderTimer: ReturnType<typeof setTimeout> | undefined;
let wasmRenderer: any = undefined;
let virtualFs: VirtualFs | undefined;
let diagramCodeLens: DiagramCodeLensProvider | undefined;

export async function activate(context: vscode.ExtensionContext) {
  // --- Commands (register FIRST, before anything that can fail) ---
  context.subscriptions.push(
    vscode.commands.registerCommand('spar.showDiagram', () => showDiagram(context)),
    vscode.commands.registerCommand('spar.selectRoot', () => selectRoot(context)),
  );

  // --- CodeLens provider for system implementation declarations ---
  diagramCodeLens = new DiagramCodeLensProvider();
  context.subscriptions.push(
    vscode.languages.registerCodeLensProvider(
      { language: 'aadl', scheme: 'file' },
      diagramCodeLens,
    ),
  );

  // --- Status Bar ---
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBarItem.command = 'spar.selectRoot';
  statusBarItem.tooltip = 'Click to select AADL root system for diagram';
  updateStatusBar();
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // --- WASM renderer disabled for now (WASI shim needs more work) ---
  // TODO: Enable once WASI filesystem shim is complete
  // try { await initWasmRenderer(context); } catch {}

  // --- LSP Client ---
  const sparPath = findSparBinary(context);
  if (sparPath) {
    try {
      const serverOptions: ServerOptions = {
        command: sparPath,
        args: ['lsp'],
        transport: TransportKind.stdio,
      };

      const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'aadl' }],
        synchronize: {
          fileEvents: vscode.workspace.createFileSystemWatcher('**/*.aadl'),
        },
      };

      client = new LanguageClient('spar', 'AADL (spar)', serverOptions, clientOptions);
      await client.start();
      context.subscriptions.push({ dispose: () => client?.stop() });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      vscode.window.showWarningMessage(`spar LSP failed to start: ${msg}`);
    }
  }

  // --- Re-render on save ---
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (doc.languageId === 'aadl' && diagramPanel && rootClassifier) {
        if (renderTimer) clearTimeout(renderTimer);
        renderTimer = setTimeout(() => renderDiagram(context), 300);
      }
    })
  );

  // --- Auto-detect root ---
  await autoDetectRoot(context);
}

export function deactivate() {
  if (renderTimer) clearTimeout(renderTimer);
  return client?.stop();
}

// --- WASM Renderer ---

async function initWasmRenderer(context: vscode.ExtensionContext) {
  const wasmDir = path.join(context.extensionPath, 'assets', 'wasm');
  const jsPath = path.join(wasmDir, 'spar_wasm.js');

  if (!fs.existsSync(jsPath)) {
    console.log('spar WASM: no assets at', wasmDir);
    return;
  }

  virtualFs = new VirtualFs();
  const imports = buildWasiImports(virtualFs);

  // Load the CJS-converted jco module (converted by scripts/convert-esm-to-cjs.js)
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const wasmModule = require(jsPath);
  console.log('spar WASM: module loaded, instantiate type:', typeof wasmModule.instantiate);

  // getCoreModule callback: loads .core.wasm files by name
  const getCoreModule = async (name: string) => {
    const wasmPath = path.join(wasmDir, name);
    console.log('spar WASM: loading core module:', name);
    const bytes = fs.readFileSync(wasmPath);
    return WebAssembly.compile(bytes);
  };

  console.log('spar WASM: calling instantiate...');
  const instance = await wasmModule.instantiate(getCoreModule, imports);
  console.log('spar WASM: instantiate returned, keys:', instance ? Object.keys(instance) : 'null');
  wasmRenderer = instance?.renderer ?? instance?.['pulseengine:rivet/renderer@0.1.0'];
  console.log('spar WASM renderer:', wasmRenderer ? 'initialized' : 'FAILED');
}

// --- Binary discovery ---

export function findSparBinary(context: vscode.ExtensionContext): string | undefined {
  const binaryName = process.platform === 'win32' ? 'spar.exe' : 'spar';

  // 1. User-configured path
  const configPath = vscode.workspace.getConfiguration('spar').get<string>('binaryPath');
  if (configPath && fs.existsSync(configPath)) {
    return configPath;
  }

  // 2. Bundled binary in extension's bin/ directory
  const bundled = path.join(context.extensionPath, 'bin', binaryName);
  if (fs.existsSync(bundled)) {
    // Ensure it's executable on Unix
    if (process.platform !== 'win32') {
      try { fs.chmodSync(bundled, 0o755); } catch { /* ignore */ }
    }
    return bundled;
  }

  // 3. Search PATH
  const pathOnPath = findOnPath(binaryName);
  if (pathOnPath) {
    return pathOnPath;
  }

  vscode.window.showErrorMessage(
    'spar binary not found. Set spar.binaryPath in settings, add spar to PATH, or reinstall the extension.'
  );
  return undefined;
}

function findOnPath(binaryName: string): string | undefined {
  const pathEnv = process.env['PATH'] ?? '';
  const separator = process.platform === 'win32' ? ';' : ':';
  for (const dir of pathEnv.split(separator)) {
    if (!dir) continue;
    const candidate = path.join(dir, binaryName);
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      return candidate;
    } catch { /* not found or not executable */ }
  }
  return undefined;
}

// --- Diagram ---

async function showDiagram(context: vscode.ExtensionContext) {
  if (diagramPanel) {
    diagramPanel.reveal(vscode.ViewColumn.Beside);
    // If we already have a root, trigger a fresh render
    if (rootClassifier) {
      renderDiagram(context);
    }
    return;
  }

  // Try to detect root from active file if not already set
  if (!rootClassifier) {
    const activeDoc = vscode.window.activeTextEditor?.document;
    if (activeDoc && activeDoc.languageId === 'aadl') {
      const detected = detectRootFromText(activeDoc.getText());
      if (detected) {
        rootClassifier = detected;
        await context.workspaceState.update('spar.lastRoot', rootClassifier);
        updateStatusBar();
      }
    }
  }

  // Still no root — ask the user
  if (!rootClassifier) {
    await selectRoot(context);
    if (!rootClassifier) return;
  }

  diagramPanel = vscode.window.createWebviewPanel(
    'aadlDiagram',
    `AADL: ${rootClassifier}`,
    vscode.ViewColumn.Beside,
    { enableScripts: true, retainContextWhenHidden: true }
  );

  diagramPanel.onDidDispose(() => { diagramPanel = undefined; });
  renderDiagram(context);
}

/**
 * Detect the first system implementation in the given AADL text.
 * Returns the qualified name (Package::Type.Impl) or undefined.
 */
export function detectRootFromText(text: string): string | undefined {
  const pkgPattern = /^\s*package\s+([\w:]+)/m;
  const implPattern = /^\s*system\s+implementation\s+(\w+)\.(\w+)/m;
  const pkgMatch = pkgPattern.exec(text);
  const implMatch = implPattern.exec(text);
  if (pkgMatch && implMatch) {
    return `${pkgMatch[1]}::${implMatch[1]}.${implMatch[2]}`;
  }
  return undefined;
}

async function renderDiagram(_context: vscode.ExtensionContext) {
  if (!diagramPanel || !rootClassifier) return;

  diagramPanel.webview.html = loadingHtml(rootClassifier);

  try {
    // Collect all .aadl files
    const files = await vscode.workspace.findFiles('**/*.aadl');
    if (files.length === 0) {
      diagramPanel.webview.html = errorHtml('No .aadl files found', 'Open a workspace containing AADL files.');
      return;
    }

    console.log('renderDiagram: wasmRenderer=', !!wasmRenderer, 'virtualFs=', !!virtualFs);
    if (wasmRenderer && virtualFs) {
      // --- WASM path (preferred) ---
      virtualFs.clear();
      for (const file of files) {
        const doc = vscode.workspace.textDocuments.find(d => d.uri.toString() === file.toString());
        const content = doc
          ? doc.getText()
          : Buffer.from(await vscode.workspace.fs.readFile(file)).toString('utf8');
        const name = path.basename(file.fsPath);
        virtualFs.setFile(name, content);
      }

      const html = wasmRenderer.render(rootClassifier, []);
      diagramPanel.webview.html = html;
      diagramPanel.title = `AADL: ${rootClassifier}`;
    } else {
      // --- Fallback: spar binary ---
      const sparPath = findSparBinary(_context);
      if (!sparPath) {
        diagramPanel.webview.html = errorHtml(
          'No renderer available',
          'spar binary not found. Please reinstall the extension or download from GitHub Releases.'
        );
        return;
      }

      const filePaths = files.map(f => f.fsPath);
      const html = execFileSync(sparPath, [
        'render', '--root', rootClassifier, '--format', 'html', ...filePaths,
      ], { encoding: 'utf8', timeout: 30000, maxBuffer: 10 * 1024 * 1024 });

      diagramPanel.webview.html = html;
      diagramPanel.title = `AADL: ${rootClassifier}`;
    }
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err);
    diagramPanel.webview.html = errorHtml('Render failed', message);
  }
}

// --- Root selection ---

async function autoDetectRoot(context: vscode.ExtensionContext) {
  const stored = context.workspaceState.get<string>('spar.lastRoot');
  if (stored) {
    rootClassifier = stored;
    updateStatusBar();
    return;
  }

  const roots = await findSystemImplementations();
  if (roots.length === 1) {
    rootClassifier = roots[0];
    await context.workspaceState.update('spar.lastRoot', rootClassifier);
    updateStatusBar();
  }
}

async function selectRoot(context: vscode.ExtensionContext) {
  const roots = await findSystemImplementations();
  if (roots.length === 0) {
    vscode.window.showWarningMessage('No system implementations found in workspace .aadl files');
    return;
  }

  const picked = await vscode.window.showQuickPick(roots, {
    placeHolder: 'Select root system implementation for diagram',
  });

  if (picked) {
    rootClassifier = picked;
    await context.workspaceState.update('spar.lastRoot', rootClassifier);
    updateStatusBar();
    if (diagramPanel) renderDiagram(context);
  }
}

async function findSystemImplementations(): Promise<string[]> {
  const files = await vscode.workspace.findFiles('**/*.aadl');
  const roots: string[] = [];

  for (const file of files) {
    try {
      const content = await vscode.workspace.fs.readFile(file);
      const text = Buffer.from(content).toString('utf8');
      const pkgPattern = /^\s*package\s+([\w:]+)/m;
      const implPattern = /^\s*system\s+implementation\s+(\w+)\.(\w+)/gm;
      const pkgMatch = pkgPattern.exec(text);
      const pkg = pkgMatch?.[1] ?? 'Unknown';
      let match;
      while ((match = implPattern.exec(text)) !== null) {
        roots.push(`${pkg}::${match[1]}.${match[2]}`);
      }
    } catch { /* skip */ }
  }
  return roots;
}

function updateStatusBar() {
  statusBarItem.text = rootClassifier
    ? `$(circuit-board) ${rootClassifier}`
    : '$(circuit-board) AADL: Select Root';
}

// --- CodeLens provider ---

class DiagramCodeLensProvider implements vscode.CodeLensProvider {
  private _onDidChangeCodeLenses = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this._onDidChangeCodeLenses.event;

  provideCodeLenses(document: vscode.TextDocument): vscode.CodeLens[] {
    const lenses: vscode.CodeLens[] = [];
    const text = document.getText();
    const implPattern = /^\s*system\s+implementation\s+(\w+)\.(\w+)/gm;
    let match;
    while ((match = implPattern.exec(text)) !== null) {
      const pos = document.positionAt(match.index);
      const range = new vscode.Range(pos, pos);
      lenses.push(new vscode.CodeLens(range, {
        title: '$(circuit-board) Show Architecture Diagram',
        command: 'spar.showDiagram',
        tooltip: 'Open live architecture diagram for this system implementation',
      }));
    }
    return lenses;
  }
}

// --- HTML templates ---

function loadingHtml(root: string): string {
  const e = root.replace(/</g, '&lt;');
  return `<!DOCTYPE html><html><head><style>
body{background:#1e1e2e;color:#cdd6f4;font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh}
.spinner{width:40px;height:40px;border:3px solid #313244;border-top:3px solid #89b4fa;border-radius:50%;animation:spin 1s linear infinite;margin:0 auto 1em}
@keyframes spin{to{transform:rotate(360deg)}}
</style></head><body><div style="text-align:center"><div class="spinner"></div><p>Rendering ${e}...</p></div></body></html>`;
}

function errorHtml(title: string, detail: string): string {
  const t = title.replace(/</g, '&lt;');
  const d = detail.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return `<!DOCTYPE html><html><head><style>
body{background:#1e1e2e;color:#cdd6f4;font-family:system-ui;padding:2em}
h2{color:#f38ba8}pre{background:#313244;padding:1em;border-radius:8px;white-space:pre-wrap;overflow-x:auto}
a{color:#89b4fa}
</style></head><body><h2>${t}</h2><pre>${d}</pre>
<p><a href="https://github.com/pulseengine/spar/releases/latest">Download latest spar release</a></p>
</body></html>`;
}
