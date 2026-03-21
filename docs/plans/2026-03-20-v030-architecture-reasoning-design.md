# v0.3.0: Architecture Reasoning Engine

**Date:** 2026-03-20
**Status:** Draft
**Scope:** Transform spar from an AADL parser+analyzer into a system architecture reasoning engine with formal verification, temporal reasoning, and AI agent integration.

## Vision

An engineer (or AI agent) should be able to ask:
- "If I move this component to processor B, does the system still meet timing?"
- "What's the blast radius of changing this data type?"
- "What changed between v1 and v2 and did it break any invariants?"
- "Are all safety requirements covered by evidence?"

And get precise, formally-backed answers — not approximations.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                   Interfaces                     │
│  CLI  │  LSP  │  MCP Server  │  VS Code  │ WASM │
├─────────────────────────────────────────────────┤
│              Query / Reasoning Layer             │
│  spar query  │  spar diff  │  spar verify       │
│  property assertions │ impact analysis           │
├─────────────────────────────────────────────────┤
│              Analysis Passes (formal)            │
│  RTA │ bus BW │ memory │ weight/power │ latency  │
│  scheduling │ connectivity │ EMV2 │ modes │ ...  │
├─────────────────────────────────────────────────┤
│              Knowledge Base                      │
│  Instance Model (arena-indexed, salsa-cached)    │
│  Property Maps │ Connection Graph │ Binding Map  │
│  Temporal: git-aware version comparison          │
├─────────────────────────────────────────────────┤
│              AADL Frontend                       │
│  Parser (v2.2, v2.3) │ HIR │ Instance Builder    │
├─────────────────────────────────────────────────┤
│              Artifact Layer (rivet)              │
│  Requirements │ Architecture │ Verification      │
│  STPA │ Traceability Links                       │
└─────────────────────────────────────────────────┘
```

## Design

### 1. New Analysis Passes

All follow the existing `Analysis` trait. Each is an independent module in `spar-analysis`.

**RTA (Response Time Analysis)** — `rta.rs`
- Fixed-point iteration per thread: R(i+1) = C_i + Σ ⌈R(i)/T_j⌉ × C_j
- Uses `scheduling_verified.rs` (Lean-proven) `compute_response_time()` core
- Input: Compute_Execution_Time, Period, Deadline, Priority, Actual_Processor_Binding
- Output: response_time per thread, error if response_time > Deadline
- Cross-checks: RMA utilization (existing) + RTA deadline (new) + EDF feasibility (existing)

**Bus Bandwidth** — `bus_bandwidth.rs`
- For each bus: identify all connections bound to it via Actual_Connection_Binding
- Sum transfer demand: Σ(Data_Size × message_rate) per connection
- Compare against bus Bandwidth property
- Error if demand > capacity, warning if utilization > 80%
- Report per-bus utilization summary

**Memory Budget** — `memory_budget.rs`
- For each memory component: sum Code_Size + Data_Size of bound processes/threads
- Compare against Memory_Size property
- Walk binding hierarchy (process → memory, thread inherits from process)

**Weight/Power** — `resource_aggregation.rs`
- Hierarchical property aggregation: sum Weight, Power_Budget up the component tree
- Compare against parent's Weight_Limit, Power_Budget
- Report per-system/per-processor totals

### 2. AADL v2.3 Parser

- Accept v2.3 syntax additions without error
- Key additions: enhanced abstract features, refined property expressions, annex improvements
- Scope: parse without error; semantic analysis of new constructs follows incrementally
- This enables opening models from OSATE 2.14+ and Ellidiss AADL Inspector

### 3. Property Assertions (Verify)

Extend `spar verify` with a structural assertion language over the instance model:

```toml
[[assertion]]
id = "ASSERT-TIMING-001"
description = "All threads must have Period and Compute_Execution_Time"
check = "components.where(category == 'thread').all(has('Timing_Properties::Period') and has('Timing_Properties::Compute_Execution_Time'))"
severity = "error"

[[assertion]]
id = "ASSERT-BINDING-001"
description = "All threads must be bound to a processor"
check = "components.where(category == 'thread').all(has('Deployment_Properties::Actual_Processor_Binding'))"
severity = "warning"

[[assertion]]
id = "ASSERT-UTIL-001"
description = "No processor utilization above 80%"
check = "analysis('scheduling').diagnostics.none(severity == 'warning' and message.contains('exceeds'))"
severity = "error"

[[assertion]]
id = "ASSERT-LATENCY-001"
description = "All E2E flows meet their latency bound"
check = "analysis('latency').diagnostics.none(severity == 'error')"
severity = "error"
```

Expression language: simple dot-notation path queries over instance model + analysis results. Not a full query language — enough for common safety/quality assertions. Think Resolute-lite.

### 4. Git-Aware Diff

`spar diff` — compare two versions of an AADL model, report structural changes and analysis impact.

**Three input modes:**
```bash
spar diff --base v0.1.0 --head v0.2.0 --root Pkg::Sys.Impl *.aadl    # git refs
spar diff --old dir1/ --new dir2/ --root Pkg::Sys.Impl                 # directories
spar diff --base main --root Pkg::Sys.Impl *.aadl                      # HEAD vs branch
```

**Diff output:**
- Structural: added/removed/modified components, connections, bindings, properties
- Analysis impact: before/after metrics for scheduling, latency, bus bandwidth, memory
- Regressions: new errors/warnings that didn't exist in the base version

**Three-way merge support:**
```bash
spar diff --base ancestor --ours branch-a --theirs branch-b --root Pkg::Sys.Impl *.aadl
```
Detects conflicting architectural changes: both branches modified the same binding, both added connections to the same port, etc.

**SARIF output:** `--format sarif` produces GitHub Code Scanning results. Analysis regressions appear as PR annotations.

**Implementation:** Uses git worktrees (or temp directories) to checkout revisions, runs full parse → instantiate → analyze pipeline on each, then compares results structurally.

### 5. MCP Server

`spar mcp` — exposes spar's analysis and model query capabilities as an MCP server (stdio transport, like LSP).

**Tools (agent-callable functions):**
- `spar/analyze` — run analysis passes, return structured diagnostics
- `spar/instantiate` — get instance model as JSON
- `spar/diff` — compare two model versions
- `spar/verify` — check assertions
- `spar/render` — generate SVG/HTML diagram
- `spar/query` — query components, connections, properties by path

**Resources (agent-readable data):**
- `aadl://components` — all component instances
- `aadl://component/{id}` — component details (features, properties, children)
- `aadl://connections` — all connections with endpoints
- `aadl://bindings` — processor/memory/bus bindings
- `aadl://analysis/{pass}` — latest analysis results

**Why MCP?** Research shows MCP is the de facto standard for AI agent-to-tool communication (97M+ monthly SDK downloads). An MCP server makes spar directly usable by Claude, GPT, and custom agents for architecture review, safety analysis, and deployment planning. The stdio transport means it works identically to LSP — agents start the process and communicate via JSON-RPC.

### 6. Knowledge Base Model

The instance model is already an in-memory graph (arena-indexed, salsa-cached). What's missing is making it **queryable** and **temporally aware**.

**Query layer:** A path-expression language that traverses the instance model:
```
components.where(category == 'thread' and parent.category == 'process')
connections.where(crosses_processor_boundary)
component('sensor').downstream(depth: 3)
```

This powers both `spar verify` assertions and `spar mcp` queries. It's not SQL or Cypher — it's a domain-specific path language over the AADL instance model.

**Temporal awareness:** The diff infrastructure provides the temporal dimension. Combined with rivet's baseline/versioning, this enables:
- "When did this binding change?" (git blame on AADL properties)
- "What was the scheduling utilization at v1.0?" (checkout + analyze)
- "Has this safety requirement been continuously met?" (baseline comparison)

### 7. Integration with Rivet

Rivet's YAML artifact approach is validated by the research as the right pattern for AI-friendly lifecycle management:
- Flat files: git-friendly, diffable, trivially parseable by any AI
- Typed links (traces-to, satisfies, verifies): graph semantics without graph DB complexity
- At query time: materialize into in-memory graph (petgraph) for traversal

`spar diff` integrates with `rivet diff`:
- rivet compares artifact changes (requirements, architecture decisions)
- spar compares AADL model changes (components, bindings, analysis results)
- Together they answer: "this requirement changed AND the architecture that satisfies it changed — here's the impact"

## Rivet Requirements

All tracked in `artifacts/requirements.yaml`:

| ID | Title | Priority |
|----|-------|----------|
| REQ-ANALYSIS-001 | RTA per-thread response time with deadline checking | High |
| REQ-ANALYSIS-002 | Bus bandwidth utilization analysis | High |
| REQ-ANALYSIS-003 | Memory budget analysis | Medium |
| REQ-ANALYSIS-004 | Weight/power property aggregation | Medium |
| REQ-PARSER-001 | AADL v2.3 syntax acceptance | High |
| REQ-VERIFY-001 | Property assertion rules in verify | High |
| REQ-DIFF-001 | Git-aware structural diff with analysis impact | High |
| REQ-DIFF-002 | SARIF output for GitHub Code Scanning | Medium |
| REQ-DIFF-003 | Three-way merge conflict detection | Medium |
| REQ-MCP-001 | MCP server exposing analysis tools and model resources | High |
| REQ-QUERY-001 | Path-expression query language over instance model | High |

## Architecture Decisions

All tracked in `artifacts/architecture.yaml`:

| ID | Decision |
|----|----------|
| ARCH-ANALYSIS-001 | New passes follow existing Analysis trait, independent modules |
| ARCH-DIFF-001 | Git worktrees for revision comparison, reuse existing pipeline |
| ARCH-MCP-001 | MCP server as CLI command, stdio transport, JSON-RPC protocol |
| ARCH-VERIFY-001 | Property assertion DSL over HIR, not full query language |
| ARCH-QUERY-001 | Path-expression language: components.where().property() pattern |
| ARCH-KNOWLEDGE-001 | Instance model is the knowledge base; query layer on top, not separate DB |

## Implementation Order

**Wave 1 (parallel, independent):**
- Analysis passes: RTA, bus bandwidth, memory budget, weight/power
- Property assertions in verify

**Wave 2 (builds on Wave 1):**
- AADL v2.3 parser
- Git-aware diff (uses analysis passes for impact comparison)
- SARIF output

**Wave 3 (builds on Wave 2):**
- MCP server (exposes everything)
- Query language (powers verify assertions + MCP queries)
- Three-way merge

## Formal Foundations (rules_lean)

Where possible, analysis algorithms should be **proven correct** using rules_lean (Lean4 + Mathlib). This provides machine-checked guarantees that the analysis is sound — critical for DO-178C and safety certification.

**Already proven:**
- `compute_response_time()` — RTA convergence and monotonicity (proofs/Proofs/Scheduling/RTA.lean)
- `ceil_div()` — integer division used in interference calculation
- `rmBound_ge_ln2` — RM utilization bound lower bound
- `edf_two_tasks_demand` — EDF demand bound

**v0.3.0 proof targets:**
- Bus bandwidth summation: `Σ demand_i ≤ capacity → no_overload`
- Memory budget: `Σ (code_size_i + data_size_i) ≤ memory_size → fits`
- Property assertion soundness: if an assertion passes on the model, the property holds on any conforming implementation
- Diff preservation: if invariant holds on base model and diff only changes non-interfering properties, invariant holds on target model

**Approach:**
1. Define the analysis algorithm in Lean4
2. Prove correctness theorems
3. Generate Rust via `lake exe codegen` (existing infrastructure)
4. Conformance tests verify Lean output matches Rust implementation

This builds on the existing `proofs/` directory and `scheduling_verified.rs` pattern. Each new analysis pass gets a corresponding proof file.

## Future (v0.4.0+)

- SysML v2 textual notation import/export (spar-transform)
- SurrealDB optional backend for large-scale model querying
- Code generation (C/Rust task skeletons)
- wasmCloud deployment manifest reconciliation
- Temporal knowledge graph for lifecycle evolution
