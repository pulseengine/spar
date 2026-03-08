<div align="center">

# Spar

<sup>Architecture analysis and design language toolchain</sup>

&nbsp;

![Rust](https://img.shields.io/badge/Rust-CE422B?style=flat-square&logo=rust&logoColor=white&labelColor=1a1b27)
![WebAssembly](https://img.shields.io/badge/WebAssembly-654FF0?style=flat-square&logo=webassembly&logoColor=white&labelColor=1a1b27)
![AADL](https://img.shields.io/badge/AADL_v2.2-AS5506D-654FF0?style=flat-square&labelColor=1a1b27)
![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=flat-square&labelColor=1a1b27)

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

Meld fuses. Loom weaves. Synth transpiles. Kiln fires. Sigil seals. **Spar structures.**

A Rust implementation of a complete AADL (Architecture Analysis and Design Language) toolchain. Parses, validates, analyzes, transforms, and visualizes system architectures per SAE AS5506D. Designed for safety-critical systems modeling — vehicle software, avionics, WASM component architectures, and AI agent workflows.

Spar replaces the Eclipse/Java-based OSATE2 toolchain with a fast, embeddable, WASM-compilable alternative built on rust-analyzer's proven architecture patterns.

## Quick Start

```bash
# Clone and build
git clone https://github.com/pulseengine/spar
cd spar
cargo build

# Parse an AADL model
./target/debug/spar parse vehicle.aadl

# Validate a model
./target/debug/spar check vehicle.aadl
```

## Architecture

- **`crates/spar-parser/`** — Hand-written recursive descent parser with error recovery
- **`crates/spar-syntax/`** — Lossless concrete syntax tree (rowan red-green trees)
- **`crates/spar-cli/`** — Command-line interface

### Planned

- **`spar-hir`** — Semantic model with incremental computation (salsa)
- **`spar-analysis`** — Pluggable analyses (scheduling, latency, resource budgets, EMV2)
- **`spar-transform`** — Format transforms (AADL ↔ WIT, JSON, SVG)
- **`spar-mcp`** — Model Context Protocol server for AI agent integration
- **`spar-wasm`** — WebAssembly component for kiln deployment

## Usage

```bash
# Parse and show syntax tree
spar parse model.aadl --tree

# Parse and show only errors
spar parse model.aadl --errors
```

## Building

```bash
# Build all crates
cargo build --workspace

# Run tests
cargo test --workspace
```

## Current Status

**Early Development** — AADL v2.2 parsing is the current focus.

### Working

- AADL lexer (all token types)
- Recursive descent parser with error recovery
- Lossless syntax tree (every byte preserved)
- CLI with parse command

### In Progress

- Complete AADL v2.2 grammar coverage
- Typed AST layer
- Semantic model (name resolution, property evaluation)

## AADL

AADL (Architecture Analysis and Design Language) is an SAE aerospace standard (AS5506) for modeling real-time, safety-critical embedded systems. It describes software architecture, hardware platforms, and deployment bindings in a single analyzable notation.

Component categories: `system` · `process` · `thread` · `processor` · `memory` · `bus` · `device` · `data` · `subprogram` · and more.

## License

MIT License — see [LICENSE](LICENSE).

---

<div align="center">

<sub>Part of <a href="https://github.com/pulseengine">PulseEngine</a> — formally verified WebAssembly toolchain for safety-critical systems</sub>

</div>
