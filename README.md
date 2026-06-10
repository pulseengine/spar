<div align="center">

# Spar

<sup>AADL v2.3 toolchain + deployment solver</sup>

&nbsp;

![Rust](https://img.shields.io/badge/Rust-CE422B?style=flat-square&logo=rust&logoColor=white&labelColor=1a1b27)
![WebAssembly](https://img.shields.io/badge/WebAssembly-654FF0?style=flat-square&logo=webassembly&logoColor=white&labelColor=1a1b27)
![AADL](https://img.shields.io/badge/AADL_v2.3-AS5506D-654FF0?style=flat-square&labelColor=1a1b27)
![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square&labelColor=1a1b27)

[![CI](https://github.com/pulseengine/spar/actions/workflows/ci.yml/badge.svg)](https://github.com/pulseengine/spar/actions/workflows/ci.yml)
[![Lean Proofs](https://github.com/pulseengine/spar/actions/workflows/proofs.yml/badge.svg)](https://github.com/pulseengine/spar/actions/workflows/proofs.yml)
[![Rivet validate](https://img.shields.io/github/actions/workflow/status/pulseengine/spar/ci.yml?branch=main&label=rivet%20validate&logo=githubactions&logoColor=white)](https://github.com/pulseengine/spar/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/pulseengine/spar/graph/badge.svg)](https://codecov.io/gh/pulseengine/spar)

&nbsp;

<h6>
  <a href="https://github.com/pulseengine/meld">Meld</a>
  &middot;
  <a href="https://github.com/pulseengine/loom">Loom</a>
  &middot;
  <a href="https://github.com/pulseengine/synth">Synth</a>
  &middot;
  <a href="https://github.com/pulseengine/kiln">Kiln</a>
  &middot;
  <a href="https://github.com/pulseengine/sigil">Sigil</a>
  &middot;
  <a href="https://github.com/pulseengine/spar">Spar</a>
</h6>

</div>

&nbsp;

A Rust implementation of a complete AADL (Architecture Analysis and Design Language) toolchain. Parses, validates, analyzes, transforms, and visualizes system architectures per SAE AS5506D. Includes a deployment solver for automated thread-to-processor allocation. Designed for safety-critical systems modeling -- avionics, vehicle software, WASM component architectures, and AI agent workflows.

Spar replaces the Eclipse/Java-based OSATE2 toolchain with a fast, embeddable, WASM-compilable alternative built on rust-analyzer's proven architecture patterns.

## Installation

```bash
# From source
cargo install --git https://github.com/pulseengine/spar

# Or download a pre-built binary from releases
# https://github.com/pulseengine/spar/releases
```

## Quick Start

```bash
# Parse an AADL model and show the syntax tree
spar parse vehicle.aadl --tree

# List all declared items
spar items vehicle.aadl

# Instantiate a system hierarchy
spar instance --root Pkg::System.Impl vehicle.aadl test-data/sensor_lib.aadl

# Run all analysis passes
spar analyze --root Pkg::System.Impl vehicle.aadl test-data/sensor_lib.aadl

# Allocate threads to processors (deployment solver)
spar allocate --root Pkg::System.Impl vehicle.aadl test-data/sensor_lib.aadl

# Render the architecture as SVG
spar render --root Pkg::System.Impl -o arch.svg vehicle.aadl test-data/sensor_lib.aadl

# Run verification assertions
spar verify --root Pkg::System.Impl --rules rules.toml vehicle.aadl
```

## CLI Commands

| Command    | Description                                                  |
|------------|--------------------------------------------------------------|
| `parse`    | Parse AADL files and show syntax tree or errors              |
| `items`    | List declared packages, types, implementations               |
| `instance` | Build the system instance hierarchy                          |
| `analyze`  | Run all analysis passes (SARIF/JSON/text output)             |
| `allocate` | Solve thread-to-processor deployment bindings                |
| `diff`     | Compare two model versions for structural/diagnostic changes |
| `modes`    | List operational modes and mode transitions                  |
| `render`   | Generate SVG/HTML architecture diagrams                      |
| `verify`   | Evaluate verification assertions against the model           |
| `lsp`      | Start the Language Server Protocol server                    |

## Architecture

20 crates, layered from low-level parsing to high-level analysis:

```
spar-syntax        Lossless CST (rowan red-green trees)
spar-parser        Recursive descent parser with error recovery
spar-annex         AADL annex sublanguage parsing (EMV2, BLESS, BA)
spar-base-db       Salsa database for incremental computation
spar-hir-def       HIR definitions -- item tree, instance model, arenas
spar-hir           Public semantic facade (name resolution, properties)
spar-analysis      30 pluggable analysis passes
spar-transform     Format transforms (AADL <-> WIT, WAC, Rust crates, wRPC)
spar-solver        Deployment solver (thread-to-processor allocation)
spar-render        SVG architecture diagrams (compound Sugiyama layout)
spar-network       Network topology and WCTT analysis support
spar-variants      Product-line variant selection and HIR filtering
spar-verify        Requirements verification engine
spar-verify-macros Procedural macros for verification rules
spar-codegen       Rust + WIT code generation from instance models
spar-insight       Discrepancy assistant (compare CTF traces vs expected)
spar-sysml2        SysML v2 / KerML extraction and generation
spar-mcp           Model Context Protocol server (read-only oracles)
spar-cli           Command-line interface
spar-wasm          WebAssembly component (WASI P2)
```

## Key Features

- **30 analysis passes** -- scheduling, latency, connectivity, resource budgets, ARINC 653, EMV2 fault trees, bus bandwidth, weight/power, mode reachability, and more
- **Assertion engine** -- declarative verification rules in TOML (`spar verify`)
- **Deployment solver** -- automated thread-to-processor allocation with constraint satisfaction
- **SARIF output** -- analysis results in SARIF format for CI integration
- **VS Code extension** -- live AADL rendering and diagnostics via LSP
- **WASM component** -- compiles to a 1.3 MB wasm32-wasip2 component
- **Incremental** -- salsa-based memoization for fast re-analysis
- **Lossless parsing** -- every byte preserved in the syntax tree

## Editor support

A first-party VS Code extension lives in [`vscode-spar/`](vscode-spar/). It pairs the `spar lsp` server (diagnostics, hover, go-to-definition, completion, rename, inlay hints) with a live architecture-diagram webview that re-renders on save. See [`vscode-spar/README.md`](vscode-spar/README.md) for install + setup.

## Documentation

- [Quickstart](docs/quickstart.md) -- build spar, parse + analyze the sample model in ~30 minutes
- [`spar moves` reference](docs/cli/moves.md) -- hypothetical-rebinding oracle (verify + enumerate)
- [WASM-as-architecture design](docs/plans/2026-03-10-wasm-as-architecture-design.md) -- WIT/WAC/wRPC transforms
- [VS Code extension design](docs/plans/2026-03-18-vscode-extension-design.md) -- editor integration
- [Deployment solver plan](docs/plans/2026-03-21-deployment-solver-plan.md) -- allocation algorithm

## Safety

Full STPA (System-Theoretic Process Analysis) safety analysis:

- [STPA analysis](safety/stpa/analysis.yaml) -- losses, hazards, UCAs, loss scenarios
- [Safety requirements](safety/stpa/requirements.yaml) -- 23 STPA-derived requirements
- [Rivet artifacts](artifacts/) -- requirements, architecture decisions, verification records

## License

MIT License -- see [LICENSE](LICENSE).

---

<div align="center">

<sub>Part of <a href="https://github.com/pulseengine">PulseEngine</a> -- formally verified WebAssembly toolchain for safety-critical systems</sub>

</div>
