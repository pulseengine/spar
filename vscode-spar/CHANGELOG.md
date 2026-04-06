# Changelog

## 0.4.0 (2026-04-05)

### Features
- Editor title button: circuit-board icon in the editor toolbar when editing `.aadl` files opens the architecture diagram
- CodeLens: "Show Architecture Diagram" link appears above every `system implementation` declaration
- Active-file root detection: `spar.showDiagram` auto-detects the root system from the current file
- `spar.binaryPath` setting: configure a custom path to the spar binary
- Binary discovery now checks settings, bundled binary, and PATH (in that order)

### Bug Fixes
- Fixed stale regex `lastIndex` in `findSystemImplementations` when scanning multiple files
- Package name pattern now supports AADL double-colon separators (e.g., `PulseEngine::FlightControl`)

## 0.2.0 (2026-03-19)

Initial release.

### Features
- AADL v2.2 syntax highlighting (TextMate grammar)
- LSP client with 10 IDE features (diagnostics, hover, go-to-definition, completion, rename, formatting, code actions, document symbols, workspace symbols, inlay hints)
- Live architecture diagram webview with interactive HTML
- Port visualization with direction indicators and type colors
- Orthogonal edge routing with obstacle avoidance
- Pan/zoom/selection in diagram
- Root system auto-detection and QuickPick selector
- Status bar indicator for current root system
