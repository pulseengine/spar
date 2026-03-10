# WASM-as-Architecture Design (Issues #8-#11)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Map WebAssembly component model artifacts (wRPC, WASI P3, Rust crates, WAC compositions) into AADL for architectural analysis by spar.

**Architecture:** Extend spar-transform with four new transform modules, each mapping a WASM ecosystem concept to AADL constructs. Add corresponding analysis passes and property sets.

**Tech Stack:** Rust, spar-transform, spar-analysis, spar-hir-def, serde_json (for cargo metadata)

---

## Vision

Developers build WebAssembly component systems using standard tooling. Spar automatically maps these artifacts into AADL, then runs architectural analyses — scheduling feasibility, latency bounds, connectivity completeness, resource budgets — that would otherwise require manual AADL authoring.

```
Rust crates (#10)  ──cargo metadata──▶  virtual AADL packages
WIT files          ──existing──────▶  AADL types/interfaces
WAC files (#11)    ──new transform──▶  AADL system implementations
wRPC config (#8)   ──new transform──▶  AADL bus/deployment model
WASI P3 (#9)       ──extend WIT────▶  AADL threads/scheduling
                                              │
                                              ▼
                                    spar-analysis passes
                               (scheduling, latency, connectivity,
                                resource budgets, binding validation)
```

---

## Issue #9: WASI P3 Async/Stream/Future → AADL Threads

### Concept Mapping

| WASI P3 concept | AADL construct | Rationale |
|---|---|---|
| `async func f(...)` | `thread` with `Dispatch_Protocol => Aperiodic` | Async functions are demand-driven tasks |
| `stream<T>` | `event data port` with data classifier T | Continuous typed data flow |
| `future<T>` | `event data port` with `WASI_P3::Delivery => OneShot` | Single-value async delivery |
| Component task runtime | `virtual processor` with async properties | The async scheduler context |
| `subtask` | Child `thread` in same `process` | Structured concurrency |
| Backpressure buffer | `Queue_Size` property on event data port | Maps to AADL queuing discipline |

### New Property Set

```aadl
property set WASI_P3 is
  Delivery : enumeration (Streaming, OneShot) applies to (port);
  Runtime : enumeration (Async, Sync) applies to (virtual processor);
  Max_Concurrent_Tasks : aadlinteger applies to (virtual processor);
  Backpressure_Buffer : aadlinteger applies to (port);
end WASI_P3;
```

### Implementation

**Files to modify:**
- `crates/spar-transform/src/wit_parser.rs` — Add `Stream(Box<WitType>)`, `Future(Box<WitType>)` variants, `is_async` flag on `WitFunction`
- `crates/spar-transform/src/wit.rs` — Generate thread components for async functions, event data ports for streams/futures
- `crates/spar-hir-def/src/standard_properties.rs` — Add WASI_P3 property set
- `crates/spar-analysis/src/scheduling.rs` — Extend for aperiodic threads on virtual processors

### Analysis Opportunities

- **Scheduling**: Aperiodic task count vs. `Max_Concurrent_Tasks` capacity
- **Backpressure**: Producer rate vs. consumer rate with buffer size
- **Latency**: Async queueing delay in end-to-end flows

### Example

```wit
interface data-pipeline {
    process: async func(input: stream<sensor-reading>) -> stream<f64>;
}
```

Generates:
```aadl
thread Process
  features
    input : in event data port SensorReading { WASI_P3::Delivery => Streaming; };
    result_out : out event data port { WASI_P3::Delivery => Streaming; };
  properties
    Dispatch_Protocol => Aperiodic;
end Process;
```

---

## Issue #8: wRPC Transport → AADL Bus Components

### Concept Mapping

| wRPC concept | AADL construct | Rationale |
|---|---|---|
| wRPC transport (NATS, TCP, QUIC) | `bus type` | AADL buses model communication infrastructure |
| Transport config | Property associations | Deployment_Properties pattern |
| wRPC protocol | `virtual bus type` | Protocol overlay on physical transport |
| Server (serving WIT) | System with `provides subprogram_group access` | Server exports interface |
| Client (invoking WIT) | System with `requires subprogram_group access` | Client imports interface |
| Cross-host connection | Port connection with bus binding | AADL deployment binding |

### New Property Set

```aadl
property set WRPC_Properties is
  Transport_Kind : enumeration (NATS, TCP, QUIC, UDS) applies to (bus);
  Endpoint_Address : aadlstring applies to (bus);
  Subject_Prefix : aadlstring applies to (bus);
  Invocation_Overhead : Time_Range applies to (connection);
  Serialization_Overhead : Time_Range applies to (connection);
end WRPC_Properties;
```

### Implementation

**New files:**
- `crates/spar-transform/src/wrpc.rs` — Standard bus type library builder + deployment descriptor transform
- `crates/spar-analysis/src/wrpc_binding.rs` — Binding validation analysis

**Reuse:** Existing `resource_budget.rs` for bandwidth, `connectivity.rs` for connection validation.

### New Analysis: wrpc_binding

- Connections between components on different processors MUST have bus binding
- Bus binding to WRPC_* type requires transport properties set
- Latency analysis adds `Invocation_Overhead + Serialization_Overhead` for wRPC connections

---

## Issue #10: Rust Crate Introspection → Virtual AADL

### Concept Mapping

| Rust/Cargo concept | AADL construct | Rationale |
|---|---|---|
| Crate | `package` | Top-level namespace |
| Dependency | `with` clause | Package import |
| `pub fn` | `subprogram` type | Public function |
| `pub struct` | `data` type | Public data structure |
| `pub trait` | `subprogram group` type | Interface contract |
| Feature flags | AADL modes | Conditional compilation |
| Workspace | Top-level `system` | Multi-crate architecture |
| `[[bin]]` target | `process` | Executable |
| `[lib]` target | `subprogram group` | Library |

### Implementation

**New files:**
- `crates/spar-transform/src/cargo_metadata.rs` — Parse `cargo metadata --format-version=1` JSON
- `crates/spar-transform/src/rust_crate.rs` — Transform trait implementation

**Phase 1 (this issue):** Dependency graph only (cargo metadata). Each crate → package, dependencies → with clauses, targets → component types, feature flags → modes.

**Phase 2 (future):** `rustdoc --output-format json` for public API extraction → subprograms, data types.

### CLI Integration

```
spar import --rust ./path/to/crate    # runs cargo metadata, generates AADL
spar import --rust-json metadata.json  # pre-captured metadata
```

### Analysis Opportunities

- Circular dependency detection
- Cross-validation: Rust crate public API vs. WIT-derived AADL declarations
- Resource estimation from build artifacts

---

## Issue #11: WAC Composition → AADL System Implementations

### Concept Mapping

| WAC concept | AADL construct | Rationale |
|---|---|---|
| `package` directive | AADL `package` with `_WAC` suffix | Composition namespace |
| `let x = new pkg:comp { ... }` | `subcomponent x : system Pkg_WIT::CompWorld` | Component instantiation |
| `import name: iface` | `requires subprogram_group access` | Composition import |
| `export expr` | `provides subprogram_group access` | Composition export |
| Named argument `a: b.c` | `connection : feature group x.a -> y.c` | Wiring |
| `...` implicit pass-through | Auto-generated features + connections | Import forwarding |

### Key Insight

A WAC file is structurally equivalent to an AADL system implementation:
- `new` statements = subcomponent declarations
- argument wiring = connection declarations
- imports = features on containing system type
- exports = features on containing system type

### Implementation

**New files:**
- `crates/spar-transform/src/wac_parser.rs` — Hand-written WAC parser (reuses wit_parser for type-level declarations)
- `crates/spar-transform/src/wac.rs` — Transform trait implementation

**Integration with WIT:** WAC references WIT-derived packages by name. Uses existing name resolution (GlobalScope) to resolve cross-references.

### Analysis Opportunities

- **Connectivity**: All component imports satisfied (no dangling requires)
- **Interface compatibility**: Wired exports/imports have matching types
- **Composition completeness**: All target world exports provided
- **Dead component detection**: Subcomponents with no wired exports

---

## Implementation Order

```
    #9 (WASI P3)     #10 (Rust crates)
         │                    │
    extends WIT          independent
         │                    │
    #8 (wRPC)          #11 (WAC)
         │                    │
    independent       needs WIT packages
         │                    │
         └────────┬───────────┘
                  │
         Integration: full pipeline
```

**Recommended order:** #9 → #8 → #11 → #10

- **#9**: Smallest scope, extends existing code, immediate scheduling analysis payoff
- **#8**: Independent, adds bus types + new analysis pass
- **#11**: New parser + transform, benefits from stable WIT transform
- **#10**: Largest scope, external tool dependency, most useful when others are done

Issues #8, #9, #10, #11 can all be parallelized since they touch different modules.

---

## Shared Infrastructure

- **Standard library ItemTrees**: #8 and #9 introduce property sets. Follow pattern in `standard_properties.rs`.
- **Analysis registration**: New passes implement `Analysis` trait, registered via `AnalysisRunner::register`.
- **Naming convention**: WIT → `{Ns}_{Name}_WIT`, WAC → `{Ns}_{Name}_WAC`, Rust → PascalCase crate name.
- **Test infrastructure**: Extract `TestBuilder` pattern into `spar-analysis/src/test_helpers.rs`.

---

## End-to-End Scenario

```bash
# Import from all sources
spar import --rust ./sensor-driver
spar import --wit interfaces/*.wit
spar import --wac compose.wac
spar analyze --wrpc deploy.toml

# Run full analysis
spar analyze --root MySystem::Sys.composed *.aadl
#  → scheduling: async tasks fit within virtual processor budget
#  → latency: end-to-end flow from sensor to cloud within 50ms
#  → connectivity: all component imports satisfied
#  → resource budget: NATS bus bandwidth sufficient
#  → binding: cross-host connections have wRPC bus bindings
```

---

## Verification

- **#9**: Parse WIT with async/stream/future, verify thread generation, run scheduling analysis
- **#8**: Build model with cross-processor connection + bus binding, verify binding analysis
- **#11**: Parse WAC, verify system implementation generation, run connectivity analysis
- **#10**: Feed cargo metadata JSON, verify package generation, verify dependency graph
- **Integration**: Load all four sources, instantiate combined system, run all analyses
