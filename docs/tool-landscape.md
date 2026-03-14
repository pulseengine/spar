# AADL Tool Landscape & Spar Competitive Position

> Artifact data lives in `artifacts/tools.yaml` — this document tells the story.
> Schema: `aadl-tool` type in `schemas/aadl.yaml`.

## The Landscape

The AADL ecosystem has 22+ tools spanning IDEs, analysis, verification,
code generation, scheduling, safety, and optimization. Most are academic
prototypes or Eclipse plugins tightly coupled to OSATE2. The commercial
segment is small (Ellidiss: [[TOOL-STOOD]], [[TOOL-AADL-INSPECTOR]]).
Notable recent entrants: [[TOOL-HAMR]] (Rust code gen), [[TOOL-AWAS]]
(security flow analysis), [[TOOL-MASIW]] (IMA/AFDX).

Spar ([[TOOL-SPAR]]) occupies a unique position: the only AADL toolchain
that is (a) a standalone Rust binary, (b) compiles to WASM, (c) has
incremental computation, and (d) provides an LSP server. No other tool
offers sub-second re-analysis or a 1.3MB deployment artifact.

## Where Spar Leads

| Capability | Spar | Nearest Competitor |
|---|---|---|
| Incremental analysis | salsa (sub-second) | OSATE2 (full rebuild) |
| Deployment size | 4MB binary / 1.3MB WASM | OSATE2 (500MB+ Eclipse) |
| LSP support | Native | None |
| WASM component transforms | WIT/WAC/wRPC | None |
| WASM compilation target | wasm32-wasip2 | None |
| Analysis pass count | 22 pluggable passes | OSATE2 (~8 built-in) |

## Where Spar Should Invest

Ranked by impact, derived from [[TOOL-CHEDDAR]], [[TOOL-MAST]],
[[TOOL-ARCHEOPTERIX]], [[TOOL-FASTAR]], and [[TOOL-AGREE]]:

### 1. Advanced Scheduling (from [[TOOL-CHEDDAR]] and [[TOOL-MAST]])

Spar's scheduling analysis is RM-only with basic utilization bounds.
The critical gaps:

- **Response Time Analysis (RTA)** — exact worst-case response times
  with blocking, preemption, and priority inversion. ~500 LOC.
- **EDF analysis** — Earliest Deadline First feasibility. ~200 LOC.
- **Audsley's OPA** — Optimal Priority Assignment. ~150 LOC.
- **Sensitivity analysis** — how much timing margin exists before
  deadline miss. From [[TOOL-MAST]]. ~200 LOC.

This is the single highest-impact improvement. Every avionics model
needs scheduling validation beyond RM utilization bounds.

### 2. Deployment Optimization (from [[TOOL-ARCHEOPTERIX]])

No AADL tool besides ArcheOpterix does multi-objective optimization.
Spar could be the second, and the first in a production toolchain.

- **Task-to-processor allocation** — bin packing with CPU/memory
  constraints. Formulate as constraint satisfaction or NSGA-II.
- **Bus binding inference** ("auto-routing") — given connections
  between components on different processors, find valid bus bindings
  that satisfy bandwidth and latency constraints.
- **Pareto front** — show architects the trade-off space (cost vs
  reliability vs latency) rather than a single solution.

Rust crates: `moors` (NSGA-II/III), `cp_sat` (CP solver). ~1500 LOC.

### 3. Fault Tree Generation (from [[TOOL-FASTAR]])

Spar has EMV2 parsing and basic error model analysis but lacks the
classical safety outputs:

- **Fault Tree Construction** — follow error propagation paths
  backward from system-level hazards. ~800 LOC.
- **Minimal Cut Set computation** — which combinations of component
  failures lead to system-level hazards. ~300 LOC.
- **FMEA table generation** — tabular format from error model. ~200 LOC.

Completes the EMV2 story and connects to rivet's STPA analysis.

### 4. Compositional Verification (from [[TOOL-AGREE]])

Longest-term, highest-ceiling investment. AGREE's assume/guarantee
contracts enable modular reasoning about system properties.

Incremental path:
1. Parse AGREE annex (currently opaque) → structured contracts
2. Export contracts to external model checker
3. (Eventually) integrate k-induction checking

### 5. UPPAAL Timed Automata Export

Spar already exports NuSMV for mode reachability. Adding UPPAAL timed
automata export enables timing property verification — a natural
extension. ~400 LOC.

### 6. Rust Skeleton Code Generation (from [[TOOL-HAMR]])

Sireum HAMR is the only tool generating Rust from AADL. Spar is uniquely
positioned to do this better — we already have the instance model and
WIT transforms. Generate:
- Thread task function stubs with dispatch protocol
- Port read/write API (matching AADL port semantics)
- Cargo.toml with correct dependencies
- ARINC 653 partition boilerplate

~800 LOC. Pairs with our existing WIT/WAC transforms for a complete
AADL-to-WASM-component pipeline.

### 7. Security Information Flow Analysis (from [[TOOL-AWAS]])

No current spar analysis covers security flows. With DO-326A airborne
security gaining traction, tracking information flow paths through
the architecture (which components can influence which data) would
differentiate spar. Extends our existing flow_check. ~600 LOC.

## What Spar Should NOT Adopt

- **Graphical editor** ([[TOOL-STOOD]]) — LSP + VS Code/Neovim is
  better for AADL development velocity. Graphical editors are slow.
- **Requirements management** ([[TOOL-ALISA]]) — rivet handles this
  with better traceability than ALISA. No duplication.
- **Code generation for Ada/C** ([[TOOL-OCARINA]]) — niche. If spar
  does code gen, it should target Rust skeletons, not Ada.
- **SDL behavioral modeling** ([[TOOL-TASTE]]) — overlaps with AGREE.
- **BLESS formal proofs** ([[TOOL-BLESS]]) — research-only, poor ROI.

## Implementation Priority

| Priority | Capability | Source Tool | LOC | Impact |
|---|---|---|---|---|
| 1 | RTA + EDF + OPA scheduling | [[TOOL-CHEDDAR]] | ~850 | Critical |
| 2 | Deployment optimization | [[TOOL-ARCHEOPTERIX]] | ~1500 | High |
| 3 | FTA + minimal cut sets | [[TOOL-EMFTA]] | ~1100 | High |
| 4 | Rust skeleton code gen | [[TOOL-HAMR]] | ~800 | High |
| 5 | UPPAAL timed automata export | [[TOOL-UPPAAL]] | ~400 | Medium |
| 6 | AGREE annex parsing | [[TOOL-AGREE]] | ~600 | Medium |
| 7 | Sensitivity analysis | [[TOOL-MAST]] | ~200 | Medium |
| 8 | Security flow analysis | [[TOOL-AWAS]] | ~600 | Medium |
| 9 | Cyber threat analysis | [[TOOL-VERDICT]] | ~1000 | Low |
