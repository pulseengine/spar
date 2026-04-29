# AADL (spar) — VS Code Extension

AADL v2.2/v2.3 language support with live interactive architecture diagrams.

Powered by [spar](https://github.com/pulseengine/spar), an open-source AADL toolchain with 21 analysis passes, port-aware rendering, and orthogonal edge routing.

## Features

### Syntax Highlighting
Full TextMate grammar for AADL v2.2/v2.3 — keywords, component categories, features, connections, properties, modes, and annexes.

### Language Server (10 IDE Features)
- **Diagnostics** — parser errors + 21 analysis passes on save
- **Hover** — component types, features, keywords
- **Go-to-Definition** — cross-file classifier references
- **Completion** — keywords, classifiers, properties, packages
- **Document Symbols** — packages, types, implementations
- **Workspace Symbols** — search across all files
- **Code Actions** — quick-fix end-names, semicolons, with-clauses
- **Formatting** — hierarchical indentation
- **Rename** — components, features, subcomponents
- **Inlay Hints** — component category, connection direction

### Live Architecture Diagram
Interactive HTML diagram that updates on every save:
- **Port visualization** with direction indicators and type colors
- **Orthogonal edge routing** with obstacle avoidance
- **Pan/zoom** — mouse wheel + drag
- **Selection** — click nodes, Ctrl+click for multi-select
- **Semantic zoom** — detail collapses at overview levels

## Quick Start

1. **Install spar binary** — download from [GitHub Releases](https://github.com/pulseengine/spar/releases/latest) and add to PATH
2. **Open an AADL workspace** — any folder with `.aadl` files
3. **Show diagram** — run `AADL: Show Architecture Diagram` from the command palette (Ctrl+Shift+P)
4. **Select root** — if multiple system implementations exist, pick one from the QuickPick

## Configuration

| Setting | Description | Default |
|---------|-------------|---------|
| `spar.binaryPath` | Path to spar binary | (auto-detect from PATH) |

## Commands

| Command | Description |
|---------|-------------|
| `AADL: Show Architecture Diagram` | Open interactive diagram beside editor |
| `AADL: Select Root System` | Choose which system implementation to render |

## Requirements

- **spar CLI** — download from [releases](https://github.com/pulseengine/spar/releases/latest) (Linux, macOS, Windows)
- VS Code 1.100+

## Links

- [spar on GitHub](https://github.com/pulseengine/spar)
- [Latest Release](https://github.com/pulseengine/spar/releases/latest)
- [Issue Tracker](https://github.com/pulseengine/spar/issues)
- [AADL Standard (SAE AS5506D)](https://www.sae.org/standards/content/as5506d/)

## License

MIT
