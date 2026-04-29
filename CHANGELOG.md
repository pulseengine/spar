# Changelog

All notable changes to spar are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.1] — 2026-04-29

Track D Phase 2: TSN-shaped service curves. Implements the three
classical IEEE 802.1Q-suite shaping mechanisms — TAS gate-window
scheduling (802.1Qbv), CBS credit-based shaping (802.1Qav), and frame
preemption (802.1Qbu) — as residual service curves in the WCTT
analysis pipeline. Models without `Spar_TSN::*` annotations produce
byte-identical output to v0.8.0.

### Added — Track D Phase 2: TSN service curves (5/5 commits)

- **`Spar_TSN::*` property set** (#177) — six properties: `Stream_ID`,
  `Class_of_Service` (0..=7 per 802.1Q PCP), `Gate_Control_List`
  (raw blob in v0.8.1, structured `GateSchedule` from c2),
  `Max_Frame_Size`, `Frame_Preemption`, and `Bandwidth_Reservation`
  (added in c3 for CBS idleSlope). `crates/spar-network::tsn` skeleton
  with `GateWindow`, `ClassOfService`, and `CreditPool` placeholder
  types plus typed-first / string-fallback property accessors. Total
  predeclared property count: 122 → 128.
- **TAS — IEEE 802.1Qbv gate-window service curve** (#180) — parses
  Gate_Control_List into a structured `GateSchedule` (`offset:duration:cos_mask`
  triples that tile the cycle period). Computes ρ_K (open fraction
  per CoS) and T_K (worst-case gate latency). `tas_residual_service`
  emits a rate-latency `ServiceCurve` with rate = R_link · ρ_K and
  latency = T_K. WCTT dispatch emits `WcttTasGated` per stream when
  bus has GCL + stream has CoS.
- **CBS — IEEE 802.1Qav credit-pool service curve** (#182) —
  per-class `CbsReservation` from `Bandwidth_Reservation` (idleSlope).
  `cbs_residual_service` produces rate-latency form: rate =
  idleSlope, latency = (max_competing_frame / link_rate) +
  (loCredit / |sendSlope|). Class isolation suppresses
  competing-flow residual decomposition. Emits `WcttCbsShaped` per
  stream when the dispatch routes through the CBS arm.
- **Frame preemption — IEEE 802.1Qbu** (#181) — replaces the legacy
  `max_frame_bytes / link_rate` blocking term with the preemption
  fragment term `(MIN_FRAGMENT_BYTES + PREEMPTION_HEADER_BYTES) /
  link_rate` (typically 68 B vs. 1518 B → 5.4 µs vs. 121 µs at 100
  Mbps). Express-stream identification by explicit `Frame_Preemption =>
  true` or default by CoS ≥ 6. Emits `WcttPreemptionApplied` per hop.
- **Phase 2 integration** (#183) — end-to-end test
  `phase2_dispatch_routes_each_stream_to_its_shaping_path` exercises
  TAS + CBS + preemption arms in the same `WcttAnalysis::analyze`
  run with three sub-systems, asserting all three diagnostics
  (`WcttTasGated`, `WcttCbsShaped`, `WcttPreemptionApplied`) coexist.

### Unified TSN dispatch

The WCTT analysis pass now has a 4-arm priority cascade for TSN
switches with orthogonal preconditions:

1. **TAS** — bus has parsed GCL **AND** stream has CoS
2. **CBS** — stream has `Bandwidth_Reservation` (idleSlope)
3. **Preemption** — bus has `Frame_Preemption=>true` **AND** stream is
   express
4. **Deferred** — `WcttDeferred` Info diagnostic, hop skipped

Orthogonal preconditions mean each arm fires only when its specific
shaping is in play; models that mix TAS + CBS + preemption across
different switches all work in the same analysis run.

### Changed

- COMPLIANCE.md narrative for v0.8.1 with per-PR breakdown.
- Test count: 2759+ across 18 crates (was 2200+).
- `docs/designs/track-d-tsn-wctt-research.md` §5.8 status table
  marks Phase 2 DONE with PR refs.

### Deferred (Phase 2 v0.8.x follow-ups)

- Piecewise-affine NC composition for cascaded TSN switches.
- Multi-stream sharing within one CBS class's reserved bandwidth.
- Advanced TAS guards (GCL gap detection across cascaded switches,
  modal GCL transients).

---

## [0.8.0] — 2026-04-28

This release promotes the Track D Phase 1 (TSN/Ethernet WCTT analysis) and
Track E (frozen-platform / mobile-application + hypothetical-rebinding
oracle) features that have been on `main` since v0.7.1. Also adds the
`spar moves verify` and `spar moves enumerate` user-facing CLIs.

### Added — Track D Phase 1: TSN/Ethernet WCTT analysis (6/6 commits)

- **New crate `spar-network`** — Network Calculus primitives, NetworkGraph
  extraction from `SystemInstance`, and supporting types. All values in
  `u64` picoseconds / bytes / bits-per-second — no floating-point drift.
- **`Spar_Network::*` property set** — `Switch_Type` (FIFO / Priority /
  TSN), `Queue_Depth`, `Forwarding_Latency` (Time_Range), `Output_Rate`,
  `WCTT_Budget`. Switches are modelled as `bus implementation` carrying
  the `Switch_Type` discriminator (Option C of the design — AADL-spec-
  conformant, no grammar extension).
- **NC primitives** — `ArrivalCurve`, `ServiceCurve`, `backlog_bound`,
  `delay_bound`, `residual_service`, `output_bound`. Closed-form for
  the affine + rate-latency case.
- **`WcttAnalysis` pass** — per-stream end-to-end traversal-time bounds
  across the device/bus graph. New diagnostics: `WcttBound`,
  `WcttExceedsBudget`, `WcttUnservable`, `WcttSwitchOverloaded`.
- **`latency.rs` integration** — RTA-derived WCET on compute hops
  alternates with WCTT-derived bounds on network hops, end-to-end.
  `Bus_Properties::Latency` scalar remains the fallback for unannotated
  buses, preserving v0.7.x behaviour.
- **Lean theorems** — `proofs/Proofs/Network/MinPlus.lean` mirrors classical
  NC closed-forms with monotonicity proved + `sorry`-with-`TODO(v1.0.0)` for
  the universally-quantified arithmetic statements.

Phase 2 (TSN-shaped service curves: TAS, CBS, frame preemption) is
deferred to v0.8.x.

### Added — Track E: hypothetical-rebinding oracle (7/8 commits)

- **`Spar_Migration::*` property set** — `Frozen`, `Mobile`,
  `Allowed_Targets`, `Pinned_Reason`. Plus `is_frozen` / `is_mobile`
  helpers for HIR-level mobility queries.
- **`BindingOverlay`** — HIR-level overlay so any analysis can run on a
  hypothetical binding without mutating the `SystemInstance`. Validates
  against Frozen / Allowed_Targets returning structured `FrozenViolation` /
  `AllowedTargetsViolation` diagnostics.
- **`spar moves verify --component X --to Y`** — first user-facing
  surface of the migration oracle. Builds a `BindingOverlay`, runs
  validation + analyses, returns structured pass/fail JSON. Exit codes
  0/1/2 distinguish ok / analysis-error / binding-violation.
- **`spar moves enumerate --component X`** — design-space exploration.
  Lists every valid hypothetical rebinding target within `Allowed_Targets`
  with verification status and a multi-objective ranking metric.
- **Multi-objective ranking** — `--objective max-response | total-load |
  total-power | total-weight | balanced`. Adds `Spar_Power::Power_Budget`
  property. Score uses the same RTA + property-accessor machinery as
  direct verification, so `verify` and `enumerate` stay consistent.
- **Rivet variant integration** — both `verify` and `enumerate` accept
  `--variant NAME` (implicit; shells out to `rivet`) or
  `--variant-context PATH` (explicit; reads JSON blob). Variant filter
  applies before overlay validation per the v1 contract's intersection
  semantics.
- **Documentation** — `docs/cli/moves.md` with flags, exit codes,
  output schemas, candidate-set derivation, ranking semantics, worked
  example.

Commit 8 (MCP tool surface for `spar.verify_move` / `spar.enumerate_moves`)
is v0.9.0 scope by design — read-only / idempotent only, deterministic
apply stays CLI-exclusive.

### Added — Track F: SysML v2 / KerML community engagement strategy

- `docs/designs/track-f-sysml-kerml-engagement.md` — research-backed
  engagement plan for the OMG SysML v2 ecosystem. Anchors on the
  `Systems-Modeling/SysML-v2-AADL-Release` repo + named contacts at
  Galois / CMU-SEI / Ellidiss. Includes verified `spar-sysml2` audit
  (production-grade with full requirements roundtrip) + Rust ecosystem
  positioning (`syster`, `Sysand`, `tree-sitter-sysml`).

### Changed

- COMPLIANCE.md narrative updated for v0.8.0 release; v0.9.0 horizon
  noted (Track E commit 8 MCP, spar-insight, Track D Phase 2 conditional
  on demand, syster license clarification).
- Test count: ~2200+ across 18 crates (previously ~1900+ across 17).

### Documentation

- `docs/cli/moves.md` — comprehensive `spar moves verify` / `enumerate` reference.
- `docs/designs/v0.7.0-hierarchical-rta.md`, `track-d-tsn-wctt-research.md`,
  `track-e-migration-research.md`, `track-f-sysml-kerml-engagement.md`.
- `docs/contracts/rivet-spar-variant-v1.md` — variant interchange contract.
- `proofs/README.md` — proof tree overview.

---

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
