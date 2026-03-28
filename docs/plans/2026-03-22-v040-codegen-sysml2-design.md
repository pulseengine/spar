---
id: DESIGN-V040-CODEGEN-SYSML2
type: design-decision
title: "v0.4.0: Code Generation Pipeline + SysML v2 Parser"
status: draft
links:
  - type: satisfies
    target: REQ-CODEGEN-001
  - type: satisfies
    target: REQ-INTEROP-001
tags: [codegen, sysml2, v040, design]
---

# v0.4.0: Code Generation + SysML v2 Parser

**Date:** 2026-03-22
**Status:** Draft
**Scope:** Two parallel tracks — (A) AADL → WIT + Rust code generation with
three-layer verification, and (B) SysML v2 rowan-based parser with requirement
extraction. Together they complete the roundtrip:
SysML v2 (requirements) → AADL (architecture) → Rust/WIT (code) → Tests/Proofs (evidence) → rivet (traceability).

## 1. Problem

spar can analyze AADL models but cannot generate code from them. Engineers
manually translate architecture decisions into implementation, breaking
traceability and introducing drift between design and code. There is no way
to prove that generated code faithfully implements the AADL specification.

Additionally, system-level requirements in SysML v2 cannot flow into spar's
analysis pipeline. The SysML v2 pilot implementation is Java-only — no Rust
parser exists.

## 2. Vision

```
SysML v2 (.sysml)              Requirements source (system-level)
    ↓ spar-sysml2 parse
rivet requirements (YAML)       Traceability hub
    ↓ satisfies
AADL model (.aadl)              Architecture (deployment-level)
    ↓ spar codegen
WIT interfaces (.wit)           Component contracts
Rust crates (src/)              Implementation skeletons
    ↓ engineer implements
Implementation                  Behavior
    ↓ verify (3 layers)
Build checks (#[aadl])          Structural conformance
Tests (contract + timing)       Behavioral conformance
Proofs (Lean4 + Kani)           Mathematical conformance
    ↓ evidence
rivet verification (YAML)       Traces back to requirements
```

## 3. Track A: Code Generation Pipeline

### 3.1 New Crate: `spar-codegen`

Located at `crates/spar-codegen/`. Depends on `spar-hir-def` (instance model),
`spar-analysis` (property accessors), `spar-transform` (WIT generation).

### 3.2 AADL → Output Mapping

The AADL component hierarchy maps to Rust build artifacts:

| AADL | Rust/WIT | Build |
|------|----------|-------|
| System implementation | Cargo workspace + WAC composition | `Cargo.toml` + `BUILD.bazel` |
| Process | Crate (lib) + WASM component | `{name}/Cargo.toml` |
| Thread | `async fn` / task entry point | `{name}/src/lib.rs` |
| Data type | `struct` + WIT `record` | `{name}/src/types.rs` + `wit/{name}.wit` |
| Data port (in) | `DataPort<T, In>` / WIT `stream<T>` | typed channel |
| Data port (out) | `DataPort<T, Out>` / WIT `stream<T>` | typed channel |
| Event port | `EventPort` / WIT `future<()>` | notification channel |
| Event data port | `EventDataPort<T>` / WIT `future<T>` | typed event channel |
| Bus access | Transport trait impl | protocol-specific adapter |
| Connection | Channel wiring in parent | `connections.rs` |
| Property (Period) | `#[aadl(period_ps = N)]` | config.rs |
| Property (Deadline) | `#[aadl(deadline_ps = N)]` | config.rs |
| Property (WCET) | `#[aadl(wcet_ps = N)]` | config.rs + Kani bound |
| Property (Binding) | `#[aadl(processor_binding = "X")]` | affinity config |

### 3.3 Output Structure

For a system `Vehicle::ECU.Impl` with processes `sensor_fusion` and `controller`:

```
generated/
├── Cargo.toml                          # workspace
├── BUILD.bazel                         # Bazel workspace
├── WORKSPACE.bazel                     # Bazel external deps
├── wit/
│   ├── sensor-fusion.wit               # WIT interface
│   └── controller.wit                  # WIT interface
├── sensor-fusion/
│   ├── Cargo.toml                      # crate (lib + cdylib for WASM)
│   ├── BUILD.bazel                     # Bazel build rules
│   ├── src/
│   │   ├── lib.rs                      # trait impl skeleton
│   │   ├── bindings.rs                 # wit-bindgen output (checked in)
│   │   ├── ports.rs                    # typed port channels
│   │   ├── types.rs                    # data type structs
│   │   └── config.rs                   # #[aadl(...)] scheduling config
│   ├── tests/
│   │   ├── contract_test.rs            # port type + direction checks
│   │   ├── timing_test.rs             # WCET measurement
│   │   └── kani_harness.rs            # bounded model checking
│   ├── proofs/
│   │   └── scheduling.lean             # Lean4 timing proof
│   └── docs/
│       └── sensor-fusion-design.md     # rivet frontmatter design doc
├── controller/
│   └── ...                             # same structure
└── verification/
    ├── sensor-fusion.yaml              # rivet verification records
    ├── controller.yaml                 # rivet verification records
    └── codegen-evidence.yaml           # generation metadata
```

### 3.4 Two Build Paths

**Cargo (development):**
- `wit-bindgen` output checked in as `bindings.rs` (no proc macro)
- `#[aadl]` attributes checked by `spar-verify-macros` proc macro
- `cargo test` runs contract + timing tests
- `cargo kani` runs bounded model checking

**Bazel (CI / release / verification):**
- `rules_wasm_component`: WIT → bindgen → compile → .wasm component
- `rules_verus`: Verus SMT checking on verified functions
- `rules_lean`: Lean4 proof checking
- `rules_kani`: Kani bounded model checking
- All hermetic, reproducible, cacheable

### 3.5 Three Verification Layers

**Layer 1 — Build-time (structural conformance)**

Generated `config.rs`:
```rust
/// AADL properties for SensorFusion::Ctrl.Impl
/// Generated by spar codegen — do not edit manually.
#[spar_verify::aadl_config]
pub mod ctrl {
    pub const COMPONENT: &str = "SensorFusion::Ctrl.Impl";
    pub const CATEGORY: &str = "thread";
    pub const PERIOD_PS: u64 = 10_000_000_000;    // 10ms
    pub const DEADLINE_PS: u64 = 8_000_000_000;    // 8ms
    pub const WCET_PS: u64 = 2_000_000_000;        // 2ms
    pub const PROCESSOR_BINDING: &str = "cpu1";
    pub const DISPATCH: &str = "periodic";
}
```

The `spar_verify::aadl_config` proc macro (or Bazel build action) reads the
AADL model file and compares these constants. Build fails if they diverge.
This catches: property value drift, renamed components, changed bindings.

**Layer 2 — Test-time (behavioral conformance)**

Generated `contract_test.rs`:
```rust
#[test]
fn sensor_data_port_matches_aadl() {
    assert_eq!(SensorDataPort::TYPE_NAME, "SensorLib::SensorReading");
    assert_eq!(SensorDataPort::DIRECTION, Direction::In);
    assert_eq!(SensorDataPort::KIND, PortKind::Data);
}

#[test]
fn all_required_ports_connected() {
    let sys = SystemUnderTest::new();
    for port in sys.required_ports() {
        assert!(port.is_connected(), "port {} not connected", port.name());
    }
}
```

Generated `timing_test.rs`:
```rust
#[test]
fn ctrl_wcet_within_bound() {
    let input = test_fixtures::sensor_reading();
    let start = std::time::Instant::now();
    ctrl_thread_body(&input);
    let elapsed_us = start.elapsed().as_micros();
    assert!(elapsed_us <= 2000,
        "WCET violation: {}us > 2000us (AADL Compute_Execution_Time)", elapsed_us);
}
```

Test results → rivet verification evidence.

**Layer 3 — Formal proof (mathematical conformance)**

Generated `proofs/scheduling.lean`:
```lean
import Proofs.Scheduling.RTA

/-- Generated from AADL: thread Ctrl in SensorFusion
    Period = 10ms, WCET = 2ms, Deadline = 8ms
    Higher priority: [filter_thread (Period=5ms, WCET=1ms)] -/
theorem ctrl_meets_deadline :
    let hp := [(5_000_000_000, 1_000_000_000)]  -- filter thread
    match compute_response_time 2_000_000_000 8_000_000_000 hp with
    | .converged r => r ≤ 8_000_000_000
    | .diverged => False := by
  simp [compute_response_time]
  omega
```

Generated `kani_harness.rs`:
```rust
#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(20)]
    fn verify_no_deadline_miss() {
        let wcet: u64 = kani::any();
        kani::assume(wcet <= 2_000_000_000);
        let hp = [(5_000_000_000u64, 1_000_000_000u64)];
        let result = compute_response_time(wcet, 8_000_000_000, &hp);
        assert!(matches!(result, RtaResult::Converged(r) if r <= 8_000_000_000));
    }

    #[kani::proof]
    fn verify_port_type_safety() {
        let data: SensorReading = kani::any();
        // Port accepts the AADL-declared type
        let port = DataPort::<SensorReading, In>::new();
        port.write(data); // type-safe by construction
    }
}
```

### 3.6 Rivet Document Generation

Each generated crate includes a design document with rivet frontmatter:

```markdown
---
id: DESIGN-GEN-SENSOR-FUSION
type: design-decision
title: "Generated: SensorFusion crate from AADL SensorFusion::Ctrl.Impl"
status: generated
fields:
  rationale: >
    Auto-generated by spar codegen from AADL model. WIT interface defines
    component contract. Rust skeleton implements the contract. Three
    verification layers ensure code-architecture conformance.
  generator-version: "spar 0.4.0"
  aadl-source: "sensor_system.aadl"
  aadl-root: "Vehicle::ECU.Impl"
links:
  - type: satisfies
    target: REQ-SENSOR-001
  - type: allocated-from
    target: ARCH-ECU-SENSOR
tags: [generated, codegen]
---
```

Plus `verification/{name}.yaml`:
```yaml
artifacts:
  - id: VAL-SENSOR-BUILD
    type: verification-verdict
    title: "Build-time: AADL attributes match SensorFusion::Ctrl.Impl"
    fields:
      method: build-check
      verdict: pending
    links:
      - type: verifies
        target: REQ-SENSOR-001

  - id: VAL-SENSOR-TIMING
    type: verification-verdict
    title: "Test-time: WCET within 2ms bound"
    fields:
      method: timing-measurement
      verdict: pending
      bound: "2000us"
      evidence-path: "sensor-fusion/tests/timing_test.rs"
    links:
      - type: verifies
        target: REQ-TIMING-001

  - id: VAL-SENSOR-RTA
    type: verification-verdict
    title: "Formal: Lean4 response time proof"
    fields:
      method: formal-proof
      verdict: pending
      prover: lean4
      evidence-path: "sensor-fusion/proofs/scheduling.lean"
    links:
      - type: verifies
        target: REQ-TIMING-001
```

### 3.7 SysML v2 Forward Compatibility

rivet link types are chosen to match SysML v2 relationship semantics:

| rivet link | SysML v2 relationship | Direction |
|-----------|----------------------|-----------|
| `satisfies` | `satisfy` | requirement ← architecture |
| `verifies` | `verify` | requirement ← test/proof |
| `traces-to` | `refine` | requirement ← derived-req |
| `allocated-from` | `allocate` | component ← function |

When `spar-sysml2` extracts requirements from `.sysml` files, they become
rivet artifacts with the same link types. Generated verification records
already point to rivet requirements — adding SysML v2 upstream is just
adding more `satisfies` targets to existing artifacts.

### 3.8 CLI

```bash
# Generate code from AADL
spar codegen --root Vehicle::ECU.Impl --output ./generated *.aadl

# Options
--format rust|wit|both        # Output format (default: both)
--build cargo|bazel|both      # Build system (default: both)
--verify all|build|test|proof # Verification layers (default: all)
--rivet                       # Generate rivet artifacts + design docs
--dry-run                     # Show what would be generated
```

## 4. Track B: SysML v2 Parser

### 4.1 New Crate: `spar-sysml2`

Located at `crates/spar-sysml2/`. Same architecture as `spar-parser`:
hand-written recursive descent, rowan lossless CST, error recovery.

### 4.2 Why Now

- SysML v2 spec finalized (2023), textual notation stable
- No Rust parser exists (pilot is Java: github.com/Systems-Modeling/SysML-v2-Release)
- KerML is just another grammar — rowan handles it the same way
- SEI published SysML v2 → AADL mapping rules (2023 annual review)
- Requirements extraction enables the full traceability pipeline

### 4.3 KerML/SysML v2 Grammar Scope

The SysML v2 textual notation is defined by KerML (Kernel Modeling Language)
plus the SysML v2 profile. Key constructs to parse:

**KerML (kernel):**
- `package`, `namespace`, `import`
- `struct`, `class`, `datatype`, `enum`
- `feature`, `connector`, `binding`
- `behavior`, `step`, `action`, `state`

**SysML v2 (profile):**
- `part def`, `part` (components)
- `port def`, `port` (interfaces)
- `connection def`, `connect` (connections)
- `interface def`, `interface` (interaction points)
- `requirement def`, `requirement` (requirements with `satisfy`, `verify`)
- `constraint def`, `constraint` (parametric constraints)
- `action def`, `action` (behavior)
- `state def`, `state` (state machines)
- `allocation def`, `allocate` (deployment)
- `flow`, `succession` (data/control flow)

### 4.4 Crate Structure

```
crates/spar-sysml2/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API: parse, lower, extract
│   ├── syntax_kind.rs      # SysML2SyntaxKind enum (~200 variants)
│   ├── lexer.rs            # KerML tokenizer
│   ├── parser.rs           # Recursive descent parser
│   ├── grammar/
│   │   ├── mod.rs          # Top-level grammar
│   │   ├── packages.rs     # package, namespace, import
│   │   ├── parts.rs        # part def, part, port, connection
│   │   ├── requirements.rs # requirement def, satisfy, verify
│   │   ├── constraints.rs  # constraint def, constraint
│   │   ├── actions.rs      # action def, action, state
│   │   └── expressions.rs  # feature expressions, operators
│   ├── lower.rs            # SysML v2 CST → AADL ItemTree
│   └── extract.rs          # SysML v2 requirements → rivet YAML
└── tests/
    ├── parser_tests.rs     # SysML v2 syntax test cases
    └── lower_tests.rs      # SysML v2 → AADL conformance
```

### 4.5 SysML v2 → AADL Lowering (SEI Mapping)

Per SEI 2023 mapping specification:

| SysML v2 | AADL |
|----------|------|
| `part def` (hardware) | `system type` / `processor type` / `memory type` |
| `part def` (software) | `process type` / `thread type` |
| `port def` (flow) | `data port` / `event data port` |
| `port def` (service) | `subprogram access` |
| `connection def` | `connection` |
| `constraint def` (timing) | AADL timing properties |
| `allocate` | `Actual_Processor_Binding` / `Actual_Memory_Binding` |
| `requirement def` | rivet requirement artifact |
| `satisfy` | rivet `satisfies` link |
| `verify` | rivet `verifies` link |

### 4.6 Requirements Extraction

```bash
spar sysml2 extract --requirements model.sysml --output requirements.yaml
```

Reads `.sysml` file, finds all `requirement def` elements, generates rivet YAML:

```yaml
artifacts:
  - id: SYSML-REQ-TIMING-001
    type: requirement
    title: "Sensor-to-actuator latency < 20ms"
    description: >
      Extracted from SysML v2 model: requirement def SensorLatency
    fields:
      sysml-source: "vehicle.sysml"
      sysml-element: "Vehicle::SensorLatency"
    tags: [sysml2, extracted, timing]
```

### 4.7 CLI

```bash
# Parse SysML v2 (like spar parse for AADL)
spar sysml2 parse model.sysml

# Lower to AADL
spar sysml2 lower --output vehicle.aadl model.sysml

# Extract requirements to rivet
spar sysml2 extract --requirements --output reqs.yaml model.sysml

# Full pipeline: parse + lower + extract + analyze
spar sysml2 analyze --root Vehicle::ECU --output report.json model.sysml
```

## 5. Integration: The Full Roundtrip

With both tracks complete, the full pipeline is:

```bash
# 1. Parse SysML v2 requirements
spar sysml2 extract --requirements vehicle.sysml -o requirements.yaml

# 2. Lower SysML v2 to AADL
spar sysml2 lower vehicle.sysml -o vehicle.aadl

# 3. Analyze AADL architecture
spar analyze --root Vehicle::ECU.Impl vehicle.aadl

# 4. Generate code + verification
spar codegen --root Vehicle::ECU.Impl --rivet -o generated/ vehicle.aadl

# 5. Engineer implements behavior in generated skeletons

# 6. Run verification (Cargo)
cd generated && cargo test

# 7. Run formal verification (Bazel)
bazel test //sensor-fusion/...

# 8. Collect evidence in rivet
rivet validate
rivet coverage
```

rivet shows the complete traceability:
```
SYSML-REQ-TIMING-001 (SysML v2 requirement)
  ← satisfies ← AADL Vehicle::ECU.Impl (architecture)
    ← generates ← sensor-fusion/ (Rust crate)
      ← verifies ← VAL-SENSOR-BUILD (build check: PASS)
      ← verifies ← VAL-SENSOR-TIMING (test: 1.8ms < 2.0ms: PASS)
      ← verifies ← VAL-SENSOR-RTA (Lean4 proof: QED)
```

## 6. Verification Guide Integration

Per the PulseEngine Verification Guide (pulseengine.eu/guides/VERIFICATION-GUIDE.md):

- Generated code targets the **feature intersection** of Verus, Kani, Lean4,
  and plain Rust (no trait objects, no closures in verified contexts, no
  async in verified contexts)
- Kani harnesses use `kani::any()` + `kani::assume()` pattern
- Lean4 proofs follow the `scheduling_verified.rs` extraction pattern
- Multiple proof candidates generated (3-5 per property) per guide recommendation
- Error classification before fix per guide principle

The AGENTS.md file references the verification guide for agent-driven
development of verified code.

## 7. Rivet Artifact Traceability

### New Requirements (to add to artifacts/requirements.yaml)

- REQ-CODEGEN-WIT: Generate WIT interfaces from AADL process components
- REQ-CODEGEN-RUST: Generate Rust crate skeletons from AADL
- REQ-CODEGEN-BAZEL: Generate Bazel BUILD files for WASM + verification
- REQ-CODEGEN-VERIFY-BUILD: Build-time AADL attribute verification
- REQ-CODEGEN-VERIFY-TEST: Test-time contract + timing verification
- REQ-CODEGEN-VERIFY-PROOF: Formal proof generation (Lean4 + Kani)
- REQ-CODEGEN-RIVET: Generate rivet design docs + verification artifacts
- REQ-SYSML2-PARSE: Parse SysML v2 textual notation (KerML grammar)
- REQ-SYSML2-LOWER: Lower SysML v2 to AADL per SEI mapping
- REQ-SYSML2-EXTRACT: Extract requirements to rivet YAML

### New Architecture Decisions (to add to artifacts/architecture.yaml)

- ARCH-CODEGEN-001: spar-codegen crate with dual output (WIT + native Rust)
- ARCH-CODEGEN-002: Three-layer verification (build + test + proof)
- ARCH-CODEGEN-003: Dual build system (Cargo dev + Bazel CI)
- ARCH-CODEGEN-004: rivet frontmatter in generated design docs
- ARCH-SYSML2-002: Rowan-based KerML parser (same pattern as spar-parser)
- ARCH-SYSML2-003: SysML v2 → AADL lowering via SEI mapping rules

## 8. Implementation Order

**Phase 1 (parallel):**
- Track A: `spar-codegen` — WIT generation + Rust skeleton + config.rs
- Track B: `spar-sysml2` — lexer + parser for core KerML subset

**Phase 2 (parallel):**
- Track A: Verification layers (build-time macros, test generation, Lean4/Kani)
- Track B: SysML v2 → AADL lowering + requirements extraction

**Phase 3 (integration):**
- Bazel rules (`rules_wasm_component`, `rules_verus`, `rules_lean`)
- rivet document generation + evidence collection
- Full roundtrip pipeline test

**Phase 4 (solver, parallel with above):**
- Exact solver (MILP/CP-SAT) — REQ-SOLVER-005
- NSGA-II Pareto fronts — REQ-SOLVER-006
