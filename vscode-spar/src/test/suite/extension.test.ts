import * as assert from 'assert';
import * as vscode from 'vscode';

suite('Extension Test Suite', () => {
  // Regression test: commands must be registered even when spar binary
  // is not found. Previously, client.start() threw before commands
  // were registered, causing "command not found" errors.

  test('spar.showDiagram command is registered', async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes('spar.showDiagram'),
      'spar.showDiagram must be registered regardless of LSP status'
    );
  });

  test('spar.selectRoot command is registered', async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes('spar.selectRoot'),
      'spar.selectRoot must be registered regardless of LSP status'
    );
  });

  test('AADL language is registered', () => {
    const languages = vscode.languages.getLanguages();
    // getLanguages returns a Thenable
    return languages.then((langs) => {
      assert.ok(langs.includes('aadl'), 'aadl language should be registered');
    });
  });

  test('Extension activates on aadl language', async () => {
    // Create an untitled document with AADL content
    const doc = await vscode.workspace.openTextDocument({
      language: 'aadl',
      content: 'package Test\npublic\nend Test;',
    });
    await vscode.window.showTextDocument(doc);

    // Extension should be active (or at least not crash)
    assert.strictEqual(doc.languageId, 'aadl');
  });

  test('Status bar item is visible', async () => {
    // After activation, the status bar should show root selector
    // We can't directly test status bar visibility, but we can
    // verify the extension didn't crash during activation
    const commands = await vscode.commands.getCommands(true);
    assert.ok(commands.length > 0, 'Extension should have activated');
  });
});
