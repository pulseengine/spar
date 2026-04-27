# Changelog

All notable changes to spar are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.1] — 2026-04-27

This release closes the v0.7.x line. Headline: full IRQ-aware response-time
analysis with priority-inheritance / priority-ceiling blocking, machine-checked
in Lean. Plus the entire v0.7.x verification-infrastructure ratchet.

Track D Phase 1 (TSN/Ethernet WCTT) and Track E (migration oracle, commits 1-4)
are also on `main` at the time of this tag — they will be promoted in the next
release (v0.8.0). They are functional and tested but the Track E surface is
not yet at its commit-8 close-out.

### Added — Track A v0.7.0 (IRQ-aware RTA)

- `Spar_Timing::*` and `Spar_Trace::*` non-standard property sets
  (`ISR_Priority`, `ISR_Execution_Time`, `Interrupt_Latency_Bound`,
  `Bottom_Half_Server`; `Probe_Point`, `Expected_BCET`, `Expected_WCET`,
  `Expected_Mean`).
- Hierarchical two-tier RTA: ISR layer steals CPU capacity first, residual
  feeds task RTA. `Dispatch_Jitter` woven into the Tindell-Clark recurrence.
  `Compute_Execution_Time`'s Time_Range consumed as `(BCET, WCET)`.
- New diagnostics: `IrqResponseBudget`, `IrqBudgetViolated`,
  `IsrOverloadedCpu`, `MissingBottomHalfServer`, `ResponseBand`.
- Lean theorems for jittered RTA convergence (`proofs/Proofs/Scheduling/RTAJittered.lean`).
- Non-regression: models without `Spar_Timing::*` produce byte-identical
  RTA output to v0.6.0.

### Added — Track A v0.7.1 (PIP/PCP blocking)

- `Thread_Properties::Locking_Protocol` (`Priority_Inheritance_Protocol`,
  `Priority_Ceiling_Protocol`, `Stop_For_Lock`, `None`) +
  `Spar_Timing::Critical_Section_Blocking` property recognition.
- Blocking term `B_i` folded into the hierarchical-RTA recurrence per
  Joseph & Pandya 1986 / Buttazzo. New `BlockingInflated` Info diagnostic.
- Non-regression: models without `Locking_Protocol` produce byte-identical
  output to v0.7.0.

### Added — Track B v0.7.x foundation (variants)

- `docs/contracts/rivet-spar-variant-v1.md` — interchange contract between
  rivet (PLE truth) and spar (HIR consumer). Shape 1: rivet emits a JSON
  context blob; spar consumes and filters HIR.
- New crate `spar-variants`: reads the v1 context blob, applies
  intersection-semantics binding rules, exposes `keep_in_variant` predicate.
  CLI integration arrives once rivet's emitter side ships.

### Added — v0.7.x verification infrastructure

- **Lean + Bazel + proptest CI gates** (`.github/workflows/proofs.yml` +
  `bazel-test` + `Rivet validate (artifacts)` jobs in `ci.yml`).
  Lean proofs now machine-checked on every PR via Mathlib precompiled cache.
  Closes #135.
- **Kani harnesses** (`crates/spar-{solver,codegen}/tests/kani_*.rs`) bounded-
  model-check ARINC653 solver invariants (closes #136).
- **cargo-fuzz scaffolding** (`fuzz/`, three targets: parser, solver,
  codegen-roundtrip, with PR smoke + nightly soak workflows) (closes #138).
- **Criterion benchmarks** (`crates/spar-{solver,codegen}/benches/`,
  PR compile-gate + nightly baseline) (closes #137).
- Status badges + AGENTS.md regeneration via `rivet init --agents`.

### Added — v0.8.0 in flight (on main, not feature-promoted in this release)

- **Track D Phase 1 — TSN/Ethernet WCTT analysis (6/6 commits)**: new
  `spar-network` crate with NetworkGraph extraction + Network Calculus
  primitives + Lean theorems. New `WcttAnalysis` pass produces per-stream
  end-to-end traversal-time bounds. `latency.rs` integration alternates
  RTA-derived WCET on compute hops with WCTT on network hops, replacing the
  scalar `Bus_Properties::Latency` placeholder when `Spar_Network::*` is
  annotated.
- **Track E commits 1-4 (4/8)**: `Spar_Migration::*` property set,
  `BindingOverlay` for hypothetical-binding queries, `spar moves verify`
  CLI returning JSON pass/fail, `spar moves enumerate` listing valid
  rebinding candidates ranked by slack.

### Changed

- COMPLIANCE.md narrative updated for v0.7.0 / v0.7.1 / partial v0.8.0.
- Test count: ~1900+ across 17 crates (previously ~1200 across 16).
- `rivet validate` pin in CI bumped from v0.1.0 to v0.4.3 to match the schema-
  tolerance behaviour of current artifacts.
- Migration: `cargo-fuzz` job now pinned to `x86_64-unknown-linux-gnu`
  (avoids ASan / static-libc conflict).

### Fixed

- Two Lean import-order / comment-style issues in `RTAJittered.lean` and
  `Network/MinPlus.lean` surfaced (and resolved) by the new Lean CI gate.
- Cargo-vet exemption ordering bug after appending criterion + pretty_assertions
  dev-deps; sorter Python script now keeps the store-format check happy.

### Documentation

- `docs/designs/v0.7.0-hierarchical-rta.md` — design doc for Track A commit 2.
- `docs/designs/track-d-tsn-wctt-research.md` — TSN/WCTT design space + commercial-tool comparison (RTaW-Pegase et al.).
- `docs/designs/track-e-migration-research.md` — migration / design-space oracle research, MCP boundary design.
- `docs/designs/track-f-sysml-kerml-engagement.md` — SysML v2 / KerML community engagement strategy.
- `docs/contracts/rivet-spar-variant-v1.md` — variant-context interchange contract.

---

## [0.6.0] — 2026-04-05

Earlier releases — see git history (no formal changelog kept before v0.7.1).
