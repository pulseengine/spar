# Spar + Rivet Integration Design

## Context

Spar is a 10-crate Rust AADL v2.2 toolchain (parser, HIR, 11 analyses, LSP, WIT transform). Rivet is a schema-driven SDLC artifact manager (YAML artifacts in git, traceability, HTMX dashboard, WASM adapter runtime). Both share the `etch` crate for SVG graph rendering.

The goal: use spar and rivet together so AADL architecture models are first-class lifecycle artifacts — traceable from stakeholder requirements down through architecture, analysis results, and verification evidence.

## Integration Architecture

### Layer 1 — CLI + JSON (immediate, zero-coupling)

Rivet calls `spar analyze --root Pkg::Sys.Impl --format json *.aadl` and pipes JSON through an AADL adapter.

### Layer 2 — Shared Rust library (medium-term)

A rivet adapter depending on `spar-hir` as a Rust library.

### Layer 3 — WASM adapter (long-term)

Compile spar to WASM component implementing `pulseengine:rivet/adapter` WIT interface.

**Recommended**: Start Layer 1, build Layer 2 in parallel, defer Layer 3.

## Phases

- **Phase 0**: Foundations — property evaluation (#6, #13), serde/JSON (#15), model completeness (#5, #4, #14)
- **Phase 1**: Integration seams — AADL schema, rivet adapter, document system
- **Phase 2**: Analysis depth (#12, #3, #7), visualization (#18), LSP (#17)
- **Phase 3**: WASM (#16), component architecture (#11, #10, #9, #8)

## Issue #19 Resolution

Requirements tracing handled by rivet, not spar. Spar recognizes `-- @traces REQ-042` comments and emits as metadata in JSON output.
