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
  // --- LSP Client ---
  const sparPath = findSparBinary();
  if (sparPath) {
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
  } else {
    vscode.window.showWarningMessage(
      'spar binary not found. Install spar or set spar.binaryPath for LSP features.'
    );
  }

  // --- Status Bar ---
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBarItem.command = 'spar.selectRoot';
  statusBarItem.tooltip = 'AADL root system for diagram';
  updateStatusBar();
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // --- Commands ---
  context.subscriptions.push(
    vscode.commands.registerCommand('spar.showDiagram', () => showDiagram(context)),
    vscode.commands.registerCommand('spar.selectRoot', () => selectRoot(context)),
  );

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
  if (!rootClassifier) {
    await autoDetectRoot(context);
  }
}

export function deactivate() {
  if (renderTimer) clearTimeout(renderTimer);
  return client?.stop();
}

// --- Binary discovery (uses execFileSync — safe, no shell injection) ---

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

  diagramPanel = vscode.window.createWebviewPanel(
    'aadlDiagram',
    `AADL: ${rootClassifier ?? 'No Root'}`,
    vscode.ViewColumn.Beside,
    { enableScripts: true, retainContextWhenHidden: true }
  );

  diagramPanel.onDidDispose(() => { diagramPanel = undefined; });

  if (rootClassifier) {
    renderDiagram(context);
  } else {
    diagramPanel.webview.html = noRootHtml();
  }
}

async function renderDiagram(_context: vscode.ExtensionContext) {
  if (!diagramPanel || !rootClassifier) return;

  const sparPath = findSparBinary();
  if (!sparPath) {
    diagramPanel.webview.html = errorHtml('spar binary not found');
    return;
  }

  try {
    const files = await vscode.workspace.findFiles('**/*.aadl');
    if (files.length === 0) {
      diagramPanel.webview.html = errorHtml('No .aadl files found in workspace');
      return;
    }

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
    diagramPanel.webview.html = errorHtml(message);
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
    vscode.window.showWarningMessage('No system implementations found in workspace');
    return;
  }

  const picked = await vscode.window.showQuickPick(roots, {
    placeHolder: 'Select root system implementation',
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
    : '$(circuit-board) AADL: No Root';
}

// --- HTML templates ---

function noRootHtml(): string {
  return `<!DOCTYPE html>
<html><head><style>
body{background:#1e1e2e;color:#cdd6f4;font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh}
.msg{text-align:center}code{background:#313244;padding:2px 6px;border-radius:4px}
</style></head><body><div class="msg">
<h2>No Root System Selected</h2>
<p>Run <code>AADL: Select Root System</code> from the command palette.</p>
</div></body></html>`;
}

function errorHtml(message: string): string {
  const e = message.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return `<!DOCTYPE html>
<html><head><style>
body{background:#1e1e2e;color:#f38ba8;font-family:system-ui;padding:2em}
pre{background:#313244;padding:1em;border-radius:8px;white-space:pre-wrap;color:#cdd6f4}
</style></head><body><h2>Render Error</h2><pre>${e}</pre></body></html>`;
}
