# STPA Safety Gaps Implementation Design

## Goal

Implement 9 not-implemented STPA-derived safety requirements across 4 independent workstreams, closing all safety gaps identified in `safety/stpa/requirements.yaml`.

## Workstreams

### A: Lowering Safety (STPA-REQ-002, STPA-REQ-004)

**Files**: `crates/spar-hir-def/src/item_tree/lower.rs`, `crates/spar-hir-def/src/item_tree/mod.rs`

- **REQ-002**: Emit warning diagnostic when annex content cannot be parsed by a registered annex parser.
- **REQ-004**: Replace wildcard `_ => {}` match arm with explicit arms for all intentionally-ignored SyntaxKind variants. Catch-all emits warning for unhandled semantic constructs.

### B: Property & Resolution Validation (STPA-REQ-006, STPA-REQ-007)

**Files**: `crates/spar-hir-def/src/properties.rs`, `crates/spar-hir-def/src/resolver.rs`

- **REQ-006**: Validate property expressions against declared property types during lowering. Emit error on type mismatch.
- **REQ-007**: When multiple classifiers match an unqualified reference, emit warning listing candidates and selected match.

### C: Instance Builder Safety (STPA-REQ-009, STPA-REQ-010, STPA-REQ-011, STPA-REQ-012)

**Files**: `crates/spar-hir-def/src/instance.rs`

- **REQ-012**: Build classifier reference graph before instantiation, detect cycles via DFS, emit error with cycle path. Increase max_depth to 100.
- **REQ-009**: Validate array dimensions are positive integers >= 1.
- **REQ-010**: Validate connection pattern indices fall within declared array dimensions.
- **REQ-011**: Ensure feature group connection matching uses member names, not positional indices.

### D: Modal-Aware Analysis (STPA-REQ-017)

**Files**: `crates/spar-analysis/src/scheduling.rs`, `crates/spar-analysis/src/latency.rs`, `crates/spar-analysis/src/resource_budget.rs`

- Add modal property evaluation helper that returns per-mode property values.
- Refactor scheduling, latency, and resource budget analyses to iterate over SOMs.
- Report per-mode or worst-case results. Unchanged behavior when no SOMs exist.

## Diagnostics Architecture

Add `diagnostics: Vec<LoweringDiagnostic>` to `ItemTree` and `diagnostics: Vec<InstanceDiagnostic>` to `SystemInstance`. Consumers read diagnostics from these structs. No trait changes needed.

## Parallelism

All 4 workstreams touch different files in different subsystems. Fully independent, run in 4 parallel worktrees.
