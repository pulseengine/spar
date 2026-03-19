import * as vscode from 'vscode';
import { execFileSync } from 'child_process';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let diagramPanel: vscode.WebviewPanel | undefined;
let rootClassifier: string | undefined;
let statusBarItem: vscode.StatusBarItem;
let renderTimer: ReturnType<typeof setTimeout> | undefined;

export async function activate(context: vscode.ExtensionContext) {
  // --- Commands (register FIRST, before anything that can fail) ---
  context.subscriptions.push(
    vscode.commands.registerCommand('spar.showDiagram', () => showDiagram(context)),
    vscode.commands.registerCommand('spar.selectRoot', () => selectRoot(context)),
  );

  // --- Status Bar ---
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBarItem.command = 'spar.selectRoot';
  statusBarItem.tooltip = 'Click to select AADL root system for diagram';
  updateStatusBar();
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // --- LSP Client (may fail if binary not found — must not break commands) ---
  const sparPath = findSparBinary();
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
  } else {
    const action = await vscode.window.showWarningMessage(
      'spar binary not found. LSP features require the spar CLI.',
      'Download spar',
      'Set Path',
    );
    if (action === 'Download spar') {
      vscode.env.openExternal(vscode.Uri.parse('https://github.com/pulseengine/spar/releases/latest'));
    } else if (action === 'Set Path') {
      vscode.commands.executeCommand('workbench.action.openSettings', 'spar.binaryPath');
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

// --- Binary discovery (execFileSync — safe, no shell injection) ---

function findSparBinary(): string | undefined {
  const configured = vscode.workspace.getConfiguration('spar').get<string>('binaryPath');
  if (configured && configured.length > 0) return configured;

  try {
    return execFileSync('which', ['spar'], { encoding: 'utf8' }).trim();
  } catch {
    return undefined;
  }
}

// --- Diagram ---

function showDiagram(context: vscode.ExtensionContext) {
  if (diagramPanel) {
    diagramPanel.reveal(vscode.ViewColumn.Beside);
    return;
  }

  if (!rootClassifier) {
    selectRoot(context).then(() => {
      if (rootClassifier) {
        showDiagram(context);
      }
    });
    return;
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

async function renderDiagram(_context: vscode.ExtensionContext) {
  if (!diagramPanel || !rootClassifier) return;

  const sparPath = findSparBinary();
  if (!sparPath) {
    diagramPanel.webview.html = errorHtml(
      'spar binary not found',
      'Install spar from <a href="https://github.com/pulseengine/spar/releases/latest">GitHub Releases</a> or set <code>spar.binaryPath</code> in settings.'
    );
    return;
  }

  try {
    const files = await vscode.workspace.findFiles('**/*.aadl');
    if (files.length === 0) {
      diagramPanel.webview.html = errorHtml('No .aadl files found', 'Open a workspace containing AADL files.');
      return;
    }

    diagramPanel.webview.html = loadingHtml(rootClassifier);

    const filePaths = files.map((f) => f.fsPath);

    // execFileSync is safe — no shell injection, arguments are array elements
    const html = execFileSync(sparPath, [
      'render', '--root', rootClassifier, '--format', 'html', ...filePaths,
    ], {
      encoding: 'utf8',
      timeout: 30000,
      maxBuffer: 10 * 1024 * 1024,
    });

    diagramPanel.webview.html = html;
    diagramPanel.title = `AADL: ${rootClassifier}`;
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
    if (diagramPanel) {
      renderDiagram(context);
    }
  }
}

async function findSystemImplementations(): Promise<string[]> {
  const files = await vscode.workspace.findFiles('**/*.aadl');
  const roots: string[] = [];
  const implPattern = /^\s*system\s+implementation\s+(\w+)\.(\w+)/gm;
  const pkgPattern = /^\s*package\s+(\w+)/m;

  for (const file of files) {
    try {
      const content = await vscode.workspace.fs.readFile(file);
      const text = Buffer.from(content).toString('utf8');
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

// --- HTML templates ---

function loadingHtml(root: string): string {
  return `<!DOCTYPE html>
<html><head><style>
body{background:#1e1e2e;color:#cdd6f4;font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh}
.loader{text-align:center}
.spinner{width:40px;height:40px;border:3px solid #313244;border-top:3px solid #89b4fa;border-radius:50%;animation:spin 1s linear infinite;margin:0 auto 1em}
@keyframes spin{to{transform:rotate(360deg)}}
</style></head><body><div class="loader">
<div class="spinner"></div>
<p>Rendering ${root.replace(/</g, '&lt;')}...</p>
</div></body></html>`;
}

function errorHtml(title: string, detail: string): string {
  return `<!DOCTYPE html>
<html><head><style>
body{background:#1e1e2e;color:#cdd6f4;font-family:system-ui;padding:2em}
h2{color:#f38ba8}
pre{background:#313244;padding:1em;border-radius:8px;white-space:pre-wrap;overflow-x:auto}
a{color:#89b4fa}code{background:#313244;padding:2px 6px;border-radius:4px}
</style></head><body>
<h2>${title.replace(/</g, '&lt;')}</h2>
<pre>${detail.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')}</pre>
<p><a href="https://github.com/pulseengine/spar/releases/latest">Download latest spar release</a></p>
</body></html>`;
}
