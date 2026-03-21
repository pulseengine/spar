# Deployment Configuration Solver: Cross-Domain Research Landscape

**Date:** 2026-03-21
**Status:** Research synthesis — pre-design

## The Problem

Every safety-critical domain deploys software to hardware **informally**:

| Domain | How deployment is decided | Pre-deployment validation |
|--------|--------------------------|--------------------------|
| Automotive (AUTOSAR) | ARXML manifests, engineer experience | Manual, "over-dependent on experience" |
| Aerospace (DO-178C) | ARINC 653 XML, custom tools | Manual + some TASTE |
| Drones (PX4/ArduPilot) | Ad hoc, hardware separation | None |
| Medical (IEC 62304) | Word/PDF documents | None |
| Industry 4.0 (IEC 61499) | Drag-and-drop in 4diac | None |
| Edge/Cloud (K8s) | YAML manifests, label selectors | Runtime only (deploy and hope) |
| Space (ESA) | TASTE/AADL (most mature) | Analysis but no optimization |

**No tool validates deployment configurations before deployment across all constraint dimensions simultaneously.** No tool finds globally optimal configurations.

## The Constraint Dimensions

A deployment decision must satisfy ALL of these simultaneously:

```
Timing:     RTA, end-to-end latency, deadlines, jitter
Safety:     ASIL/SIL/DAL decomposition, FFI, partitioning
Security:   Trust zones, E2E protection, SecOC, encryption overhead
Resources:  CPU utilization, memory budget, bus bandwidth, power, weight
Physical:   Hardware topology, bus connectivity, protocol compatibility
Cost:       ECU cost, wiring weight, installation space
```

Current tools optimize **at most one or two** dimensions. PREEvision does cost+weight. OSATE does scheduling. Cheddar does timing. Nobody does all at once.

## What Exists Today

### AADL Tools

| Tool | Method | Optimal? | Active? | Input |
|------|--------|----------|---------|-------|
| OSATE (CMU SEI) | Greedy bin-packing | No | Yes | AADL |
| ArcheOpterix (Monash) | GA, ACO, MILP | Near-Pareto | Dead (~2016) | AADL |
| DeSyDe/IDeSyDe (KTH) | Constraint Programming | **Yes (certificates)** | Yes | ForSyDe (not AADL) |
| TASTE/Ocarina (ESA) | Analysis only | N/A | Aging | AADL |
| Cheddar | Scheduling simulation | N/A | Yes | AADL/custom |

### Commercial E/E Tools

| Tool | Vendor | Focus | Optimization |
|------|--------|-------|-------------|
| PREEvision | Vector | Full E/E architecture | Cost, weight, bandwidth (heuristic) |
| Capital | Siemens | E/E systems | Cost, weight (heuristic) |
| SystemWeaver | Systemite | Traceability | None (data model only) |
| EB tresos | Elektrobit | AUTOSAR config | None (configuration tool) |
| DaVinci | Vector | AUTOSAR config | None (configuration tool) |

**Key gap:** PREEvision is the closest commercial tool but costs six-figure annual licenses, is closed-source, and uses heuristics (not provably optimal).

### Academic DSE

The 2017 KTH gap analysis (interviews with 5 automotive OEMs) found:
> "A large body of work exists on DSE methods, however **almost none is successfully adopted in automotive industry.**"

Reasons: tools don't reflect real constraints, don't integrate with workflows, use simplified models.

## Patent Landscape

Dominated by **1990s co-synthesis heuristics** (Lucent/Princeton: US6178542B1, US6289488B1). Runtime scheduling patents from Qualcomm (US9785481B2). **No patents on:**
- Exact multi-objective deployment optimization with optimality certificates
- AADL-based deployment solving
- Joint safety+security+timing optimization
- WASM-deployable architecture optimization

**This is white space.**

## Solver Technology (Rust-Compatible)

| Solver | Type | Global optimal? | WASM? | Rust crate |
|--------|------|----------------|-------|------------|
| microlp/Clarabel | LP/QP | LP-relaxation | **Yes** | `microlp`, `clarabel` |
| HiGHS | MILP | **Yes (certificates)** | No (C++) | `good_lp` |
| Google CP-SAT | CP-SAT | **Yes (certificates)** | No (C++) | `cp_sat` |
| Z3/nuZ | SMT/MaxSMT | **Yes (certificates)** | No (C++) | `z3` |
| NSGA-II | Multi-obj EA | No (Pareto approx) | **Yes** | Custom |
| Clingo (ASP) | Answer Set | **Yes** | No (C++) | C API |

**Key finding:** ASP was **3 orders of magnitude faster** than ILP for multiprocessor synthesis (Ishebabi 2009). CP-SAT (DeSyDe) provides optimality certificates for embedded DSE.

**Recommended strategy:**
- WASM: pure-Rust LP + custom branch-and-bound + NSGA-II for Pareto fronts
- Native CLI: `good_lp` + HiGHS (MILP) or `cp_sat` (CP-SAT) for exact solving
- Verification: `z3` for SMT-based certificate generation

## The Opportunity

### What spar already has
- Complete AADL v2.2/v2.3 parser (rowan CST, lossless)
- Salsa incremental computation (change → recompute only affected)
- Instance model with components, features, connections, bindings, properties
- 21 analysis passes (scheduling, RTA, latency, bus bandwidth, memory budget, weight/power, ARINC 653, EMV2, wRPC binding)
- WASM compilation (1.3MB component)
- Rivet integration (requirements → architecture → verification traceability)
- Source rewriting capability (rowan preserves formatting)
- `spar diff` for structural comparison + regression detection
- Property assertion engine with rowan CST

### What spar needs to add
1. **Topology graph extraction** — hardware topology as petgraph (processors, buses, memories, their connections)
2. **Virtual bus library** — AADL package with protocol types (DDS, SOME/IP, shared memory, CAN, etc.) and their properties (latency overhead, bandwidth, payload size, security profile)
3. **Constraint formulation** — translate AADL model + properties into solver input (variables: bindings; constraints: timing/safety/security/resources; objectives: latency/cost/utilization)
4. **Solver integration** — CP-SAT or MILP backend for exact solving; NSGA-II for large-scale Pareto approximation
5. **Source rewriting** — rowan-based `spar refactor` that applies solver output as AADL property changes
6. **Impact preview** — salsa-powered what-if analysis before committing changes
7. **Optimality certificates** — machine-checkable proof that the deployment is optimal (or within N% of optimal)

### What makes this a breakthrough

No existing tool:
1. Takes **AADL** and produces **provably globally optimal** deployments
2. Handles **all constraint dimensions** simultaneously (timing + safety + security + resources + cost)
3. Runs as **WASM** (browser-deployable, embeddable)
4. Provides **optimality certificates** (not just "good enough")
5. Integrates with **lifecycle traceability** (rivet)
6. Can **rewrite source** to apply the optimized deployment (rowan)
7. Validates **incrementally** after changes (salsa)

## Target Markets

### Tier 1: Direct AADL alignment
- **Aerospace** (DO-178C, ARINC 653) — AADL was designed for this; TASTE users want modern tooling
- **Space** (ESA, NASA) — ESA already uses AADL via TASTE; spar is lighter and more modern
- **Defense** (FACE) — DoD standard for portable avionics, maps cleanly to AADL

### Tier 2: Strong problem-solution fit
- **Automotive SDV** — AUTOSAR deployment is manual and NP-hard; TU Munich published ILP formulations but no integrated tool exists
- **Drones/UAV** — Zero formal architecture tools; BVLOS certification creating demand for exactly this
- **Medical devices** — FDA pushing for more rigorous architecture documentation; IEC 62304 safety classification needs formal partitioning analysis

### Tier 3: Adjacent opportunities
- **Industry 4.0** — IEC 61499 → AADL mapping already researched; IEC 62443 zones/conduits naturally model as AADL
- **Edge/IoT** — Zephyr community requesting "system devicetree" (multi-board deployment) = what AADL already provides
- **Autonomous vehicles** — Multi-SoC architectures (NVIDIA DRIVE, Mobileye EyeQ) need formal deployment validation

## Communication Protocol Catalog (Virtual Bus Library)

The "box of possibilities" modeled as AADL virtual bus types:

### Automotive
- SOME/IP over Ethernet (service discovery, serialization)
- DDS over Ethernet (pub/sub, QoS policies)
- CAN / CAN FD (priority-based, 8B/64B frames)
- LIN (low-cost sensor bus)
- FlexRay (deterministic, dual-channel)
- SecOC (authenticated messages)
- E2E Protection profiles (CRC + sequence counter)

### Aerospace
- ARINC 429 (unidirectional, 32-bit words)
- ARINC 664 (AFDX — deterministic Ethernet)
- MIL-STD-1553 (command/response bus)
- SpaceWire (ESA point-to-point)
- TTP (Time-Triggered Protocol)

### Industrial
- PROFINET (real-time Ethernet, IRT mode)
- EtherCAT (on-the-fly processing)
- OPC UA + TSN (time-sensitive networking)
- Modbus RTU/TCP (legacy)

### Embedded
- Shared memory + interrupt (same SoC)
- DMA transfer (same board, PCIe)
- SPI/I2C/UART (low-speed peripheral)
- MAVLink (drone inter-processor)
- uProtocol/Zenoh (transport-agnostic)

### Security overlays
- TLS/DTLS (encryption + authentication)
- IPsec (network-layer encryption)
- MACsec (link-layer encryption)
- Application-level E2E (CRC + MAC)

Each entry includes: latency overhead, bandwidth capacity, max payload, security profile, reliability model, bus type compatibility.

## Safety + Security Co-Analysis

### ASIL Decomposition Rules (ISO 26262)
- ASIL-D = ASIL-B(D) + ASIL-B(D) on sufficiently independent elements
- ASIL-D = ASIL-C(D) + ASIL-A(D)
- ASIL-C = ASIL-B(C) + ASIL-A(C)
- Independence requires: no common-cause failures, freedom from interference
- **Deployment constraint:** decomposed requirements may need separate processors or ARINC 653 partitions

### Freedom from Interference (FFI)
- Spatial: MMU/MPU memory protection
- Temporal: scheduling guarantees (WCET budgets)
- Communication: controlled interfaces between safety levels
- **Deployment constraint:** QM software cannot affect ASIL-D without proven isolation

### Security Zones (IEC 62443 / ISO 21434)
- Zone = group of components with same security level
- Conduit = communication channel between zones with defined protection
- **Deployment constraint:** cross-zone connections require encryption/authentication, adding latency overhead that the timing analysis must account for

### Joint Safety-Security
- A brake-by-wire message needs ASIL-D integrity AND protection against spoofing
- Encryption overhead (TLS: ~1ms per handshake) must be included in end-to-end latency calculation
- The solver must jointly satisfy safety partitioning AND security zoning AND timing deadlines

## Competitive Positioning

| Capability | PREEvision | OSATE | TASTE | App4MC | **spar** |
|-----------|-----------|-------|-------|--------|---------|
| AADL input | No | Yes | Yes | No | **Yes** |
| Global optimization | No | No | No | No | **Target** |
| Optimality certificates | No | No | No | No | **Target** |
| Multi-objective | Partial | No | No | Partial | **Target** |
| Safety + Security + Timing | Partial | Partial | Partial | Partial | **Target** |
| WASM deployment | No | No | No | No | **Yes** |
| Lifecycle traceability | No | No | No | No | **Yes (rivet)** |
| Source rewriting | No | No | No | No | **Yes (rowan)** |
| Incremental validation | No | No | No | No | **Yes (salsa)** |
| Open source | No | Yes | Yes | Yes | **Yes** |
| License cost | $$$$$ | Free | Free | Free | **Free** |

## Key References

### Academic
- Aleti et al. 2013 — "Software Architecture Optimization: A Systematic Literature Review" (188 papers surveyed)
- Ishebabi 2009 — ASP 3 orders of magnitude faster than ILP for multiprocessor synthesis
- KTH 2017 — Gap analysis: why automotive doesn't adopt DSE tools
- IDeSyDe 2024 — Modular CP-based DSE with optimality certificates
- TU Munich 2022 — ILP formulation for SDV resource allocation

### Patents
- US6178542B1 — COSYN co-synthesis (Lucent, 2001) — greedy heuristic
- US6289488B1 — COHRA hierarchical co-synthesis (Lucent, 1998)
- EP2386949A1 — Linear equations task allocation
- **Gap:** No patents on exact multi-objective AADL deployment optimization

### Tools
- DeSyDe/IDeSyDe (KTH ForSyDe) — closest to what we're building, but for ForSyDe not AADL
- ArcheOpterix (Monash) — AADL evolutionary optimization, dead since 2016
- Eclipse Ankaios — Rust-based automotive workload orchestrator (deployment, not optimization)
- Eclipse App4MC — AUTOSAR timing/mapping (heuristic)

### Standards
- AADL AS5506C, AUTOSAR R25-11, ARINC 653, DO-178C, ISO 26262, ISO 21434, IEC 61508, IEC 62443, IEC 62304, JARUS SORA
