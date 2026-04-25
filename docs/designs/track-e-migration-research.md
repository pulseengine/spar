# Design: Track E — Frozen-Platform/Mobile-Application Split + Hypothetical-Rebinding Oracle

Status: **research / proposed** — refines acceptance criteria for issue #150.
Last update: 2026-04-23.
Target releases: v0.8.0 (core surface) + v0.9.0 (LLM/MCP integration).
Companion: rivet variants v1 contract (#144), Track A hierarchical RTA design.

> Scope guard: this doc is **research + design space exploration**, not an
> implementation plan. No production code lands from this doc directly.
> Concrete commits are sketched in §7 and bound by the open questions in §8.

---

## §0 Problem statement (1-paragraph recap)

spar's allocation analyses today (`spar-analysis::rta`, bandwidth, processor
binding diagnostics) verify *one* committed binding at a time. Three workflows
are blocked:

1. **Platform vs. application split** — AADL mixes "frozen" hardware/RTOS
   subsystems with "mobile" application components freely; no formal way to
   declare "this stays put, that may move".
2. **Hypothetical-binding queries** — "If I move `Handler.brake` from `ECU_3`
   to `ECU_5`, do all my deadlines, bandwidth budgets, and partition-isolation
   constraints still close?" — needs analyses on a tentative binding without
   mutating committed model state.
3. **AI/LLM design-space exploration** — an LLM agent proposes moves; spar
   verifies each move deterministically and certifies the result. **The LLM
   never enters the certified path**; spar is the oracle, the LLM is the
   search heuristic.

This doc surveys the prior art (§§1–5), then lays out spar's design space
(§6), a roadmap (§7), and open risks (§8).

---

## §1 — Existing component-placement tooling

### 1.1 Comparison table (10 tools)

| Tool | License | AADL/SysML | Hypothetical binding? | DSE interface | MCP/tool-API |
|---|---|---|---|---|---|
| **OSATE2** | EPL-2.0 (open) | AADL v2.2 native; SysMLv2 pilot 2025-03 | No — analyses run on committed `binding` properties; users hand-edit and re-run | None native; CAMET-DSE plug-in (closed) provides explorer | None — Eclipse-only; no JSON-RPC |
| **Stood (Ellidiss)** | Commercial | AADL + HOOD; reverse-engineering | No — round-trip editing only | None; Marzhin simulator runs *one* binding | None |
| **AADL Inspector** | Commercial (Ellidiss) | AADL | Limited — Marzhin replay can swap bindings interactively | None | None |
| **Bauhaus** | Commercial (Axivion) | C/C++/Ada reverse, **not native AADL** | N/A | None (program understanding focus) | None |
| **PolarSys CHESS** | EPL (open) | UML+MARTE+AADL profiles | No native; `analysis context` swap is manual | Limited via MARTE profile | None |
| **Capella/Arcadia** | EPL (open) | Arcadia DSL; allocation = "Function → Component" | Partial — alternate "Physical Architecture" candidates can be modelled side-by-side | Manual; viewpoints only | None native |
| **AUTOSAR Classic** | Vendor toolchains | ARXML (own meta-model) | Manifest-driven: SWC→ECU mapping is a separate artifact (`ECU Configuration`) and **can be re-generated** | None standard | None |
| **Adaptive AUTOSAR** | Vendor | ARXML manifests (Execution/Service/Machine) | **Yes, by design** — manifests are post-build, behaviour changes by replacing manifest | None standard | None native (vendor-specific) |
| **SCADE Architect** | Commercial (Ansys) | Architect ↔ Suite sync; AUTOSAR R4.2.2 export | No — architecture model is single-valued | None native; manual variants | None |
| **pyAADL / OpenAADL Ocarina** | GPL/EUPL | AADL parser + code-gen | No — parser/generator only; no analysis-on-tentative-binding mode | None | None |
| **ROS 2 launch + colcon** | Apache-2.0 | Not AADL; component composition via `rclcpp_components` | Runtime component re-loading possible (`dlopen_composition`) — but no formal pre-flight verification | None | None |

> Sources: OSATE 2.18 docs, [osate.org](https://osate.org/about-osate.html);
> Stood, [ellidiss.com](https://www.ellidiss.com/products/stood/); AADL Inspector,
> [ellidiss.com](https://www.ellidiss.com/products/aadl-inspector/); Capella,
> [polarsys.org](https://www.polarsys.org/capella/); AUTOSAR AP R24-11
> [Manifest Spec](https://www.autosar.org/fileadmin/standards/R24-11/AP/AUTOSAR_AP_RS_ManifestSpecification.pdf);
> SCADE Architect [datasheet](https://www.ansys.com/content/dam/product/embedded-software/ansys-scade-architect/ansys-scade-architect-datasheet.pdf);
> CAMET tools [Lewis_CAMET_overview.pdf](https://adept.univ-brest.fr/2024/doc/Lewis_CAMET_overview.pdf);
> ROS 2 [composition docs](https://index.ros.org/p/composition/).

### 1.2 What's GOOD across the field

- **Manifest-as-data** (AUTOSAR Adaptive) — deployment lives in a separately
  versioned artifact, not bolted into the component model.
- **Property-driven extension** (OSATE/AADL) — new analyses add property sets;
  no parser surgery needed.
- **Multi-viewpoint** (Capella) — alternate physical architectures coexist.
- **Architecture-vs-design split** (SCADE Architect ↔ Suite) — different tools
  for different abstraction layers, with auto-sync.
- **Component composition** (ROS 2) — runtime move via `dlopen` proves the
  *operational* feasibility, even without formal pre-flight checks.

### 1.3 What's MISSING for spar's needs

- **No tool offers "verify this hypothetical move and tell me what fails"** as
  a first-class API. CAMET-DSE is closest but is closed-source, OSATE-only,
  and not exposed over a programmatic interface.
- **No tool explicitly models the platform/application boundary** with a
  "frozen" semantics. AUTOSAR Adaptive's machine-manifest is the closest, but
  the boundary is implicit.
- **No tool offers an LLM-facing oracle surface.** Lean MCP exists for proofs
  (§3), but the architecture-modelling community has nothing analogous.
- **DSE results are not certifiable** — even when found, the optimization
  output isn't itself verified. spar can do better by making *each candidate
  point* a certifying analysis run.

---

## §2 — Multi-objective allocation literature

### 2.1 Cited papers (2018–2026)

- **Sukkar et al. 2025**, "Dynamic Multi-Objective Optimization in Vehicular Fog
  Computing With NSGA-II+", Wiley *Trans. Emerging Telecom Tech*. Reports
  72% delay / 71% energy reduction over plain NSGA-II.
  [DOI](https://onlinelibrary.wiley.com/doi/10.1002/ett.70260)
- **Fan et al. 2024**, "Improved NSGA-II for task offloading in IoV edge",
  *Multimedia Systems*. Demonstrates fitness-weighted dominance for latency
  + energy + bandwidth on DAG-structured workloads.
  [DOI](https://link.springer.com/article/10.1007/s00530-024-01598-0)
- **Vajdi & Hong 2020**, "NSGA-II-based micro-service allocation in container
  clouds", *ResearchGate*. Closest analogue to AADL component-to-processor
  binding under availability + energy.
  [link](https://www.researchgate.net/publication/342929909)
- **Dürr 2023 (et seq.)**, "Integer and Constraint Programming for the Offline
  Nanosatellite Partition Scheduling Problem", *Springer LNCS*. MILP+CP hybrid
  for ARINC-653-style partition placement.
  [link](https://link.springer.com/chapter/10.1007/978-3-031-95976-9_11)
- **Jung & Sandberg 2023**, "Systematic Design Space Exploration via Design
  Space Identification" (IDeSyDe), *ACM TODAES*. Generic DSE framework with
  pluggable solvers; cites AADL.
  [DOI](https://dl.acm.org/doi/10.1145/3647640)
- **Aleti et al. 2009/2018**, "ArcheOpterix", and the 2018 survey extension
  in *J. Syst. Softw.*. Established platform for AADL architecture
  optimization with NSGA-II/SPEA2.
  [academia.edu](https://www.academia.edu/37296994/ArcheOpterix_An_extendable_tool_for_architecture_optimization_of_AADL_models)
- **2025 RL-MOTS paper**, "RL-Based Multi-Objective Task Scheduling", *Sci.
  Reports*. DQN-based adaptive scheduling — cautionary tale: training-cost
  prohibitive for one-off architecture decisions.
  [DOI](https://www.nature.com/articles/s41598-025-25666-1)

### 2.2 Why solver-only approaches fall short

- **Combinatorics are punishing**: N components × M processors with
  bandwidth/timing constraints is NP-hard; ILP/MILP works for ≤500-variable
  problems but breaks at industrial scale (1000+ SWCs in modern E/E).
- **Multi-objective Pareto fronts** require thousands of model evaluations;
  each spar analysis pass is a non-trivial fixed-point computation.
- **Solvers don't explain "why"** — they return a binding, not a rationale,
  which makes certification arguments harder.
- **Architects want incremental moves, not global re-optimization.**
  Re-deriving a complete Pareto front when one component shifts is wasteful.
- **This is exactly why an LLM heuristic on top of a deterministic verifier
  is interesting** — heuristic search + per-step certification.

### 2.3 Per-paper applicability score for spar

| Paper | Reusable now? | What we'd take | What we'd skip |
|---|---|---|---|
| Sukkar 2025 NSGA-II+ | Partially | Tie-breaking rule + diversity preservation | Domain-specific fog-computing weights |
| Fan 2024 IoV offloading | Partially | DAG-aware fitness; treats moves as tree | Edge-cloud-specific cost model |
| Vajdi 2020 micro-services | High | Direct analogue — components→containers≈threads→ECUs | Container-orchestration assumptions |
| Dürr 2023 partition CP/MILP | High | Partition-isolation modelling for ARINC-653 | Single-processor focus |
| IDeSyDe (Jung 2023) | High | DSE-as-identification framework; pluggable solvers | Heavy Java/Kotlin dependency |
| ArcheOpterix | Reference only | Property-set hooks for fitness | Outdated solver backends |
| RL-MOTS 2025 | Cautionary | None directly | Whole approach — training cost dwarfs use case |

---

## §3 — LLM + constraint-solver patterns (2024–2026)

### 3.1 Pattern catalogue

| Pattern | Origin | Verifier | Best example |
|---|---|---|---|
| **Generate-and-evaluate evolution** | DeepMind FunSearch (2023, Nature) | Python evaluator on bin-packing / cap-set | [DeepMind blog](https://deepmind.google/discover/blog/funsearch-making-new-discoveries-in-mathematical-sciences-using-large-language-models/) |
| **LLM proposes, kernel verifies** | Lean Copilot, BFS-Prover, DeepSeek-Prover-V2 | Lean4 kernel | [arXiv 2404.12534](https://arxiv.org/abs/2404.12534), [InfoQ DeepSeek](https://www.infoq.com/news/2025/05/deepseek-prover-v2-formal-proof/) |
| **Hammer-style discharge** | Sledgehammer (Isabelle), CoqHammer | External ATP/SMT | [Isabelle Sledgehammer](https://isabelle.in.tum.de/doc/sledgehammer.pdf), [SMT extension](https://link.springer.com/chapter/10.1007/978-3-642-22438-6_11) |
| **LLM + SMT planner** | Multi-constraint planning frameworks 2024 | Z3 / cvc5 | [Comparative study](https://www.arxiv.org/pdf/2508.03366) |
| **ConstraintLLM (industrial CP)** | EMNLP 2025 | MiniZinc / OR-Tools | [aclanthology.org/2025.emnlp-main.809](https://aclanthology.org/2025.emnlp-main.809.pdf) |
| **Step-checked CoT (Safe)** | 2025 | Lean4 per-step | (cited in [Apple Hilbert](https://machinelearning.apple.com/research/hilbert)) |
| **LLM-as-architect (ADD)** | Diaz-Pace et al. 2025, arXiv 2506.22688 | Compilers + tests | [arXiv](https://arxiv.org/pdf/2506.22688) |

### 3.2 Common shape

```
┌──────────────┐  proposal   ┌──────────────────┐  result   ┌────────────┐
│  LLM agent   │ ──────────► │  Verification    │ ────────► │  Updated   │
│  (heuristic) │             │  oracle          │           │  context   │
│              │ ◄────────── │  (deterministic) │ ◄──────── │            │
└──────────────┘  feedback   └──────────────────┘  loop     └────────────┘
                                       │
                                       ▼
                                  certified
                                  artifact
                                  (traceable)
```

### 3.3 Lessons for spar

- **Hallucination is a feature, not a bug** — Lean Copilot et al. let the LLM
  invent freely; only verifier-passing artifacts survive. Same model for
  Track E: LLM proposes wild moves; only those passing every analysis pass
  are recorded.
- **Per-step verification is cheaper than global proof.** A `verify_move`
  call should be sub-second so the LLM gets fast feedback.
- **The verifier is the certification anchor.** spar is already trustworthy
  via Lean4-verified scheduling kernels; we extend that trust through the
  MCP boundary.
- **Industrial CP examples (ConstraintLLM)** show LLMs can produce *MiniZinc
  models* from natural language — interesting for spar's `Allowed_Targets`
  authoring UX but **not** on the certified path.
- **AI-assisted Architecture (Diaz-Pace 2025)** confirms the "prompt → suggest
  → validate" loop generalizes from code to architecture.

### 3.4 Pattern fit for spar

| Pattern | Spar fit | Reasoning |
|---|---|---|
| FunSearch evolutionary | Medium | Could evolve `Allowed_Targets` lists, but Track E is more about per-move verification than program search |
| Lean-style proposes/verifies | **High — direct fit** | spar's analyses are the kernel; LLM proposes moves; identical shape |
| Hammer-style multi-prover | Low | spar has one analysis path per pass; no parallel prover ensemble |
| LLM + SMT planner | Medium | If we add MILP optimizer, this becomes the reference architecture |
| ConstraintLLM (NL → MiniZinc) | Authoring UX only | Could let users describe `Allowed_Targets` in NL, but **never on certified path** |
| Step-checked CoT (Safe) | Medium | Each LLM step = one `verify_move` call; spar's role identical to Lean4's |
| LLM-as-architect (ADD) | Reference only | Higher abstraction (whole architectures) than Track E's per-move scope |

### 3.5 Anti-patterns to avoid

- **Letting the LLM emit ARXML/AADL text directly into the certified path.**
  Always force LLM output through a structured "move" RPC the LLM cannot
  forge.
- **End-to-end RL for one-off architecture decisions.** Training cost dwarfs
  the value; cf. the 2025 RL-MOTS paper which needs hundreds of episodes.
- **Hidden state in the oracle.** Every `verify_move` must be pure: same
  inputs, same outputs, no side effects on committed model.

---

## §4 — MCP tool design for verification oracles

### 4.1 Spec status (2026)

- Current spec: `2025-11-25` revision (see
  [modelcontextprotocol.io/specification/2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25)),
  with TypeScript-first schema, JSON-Schema export, and `Tool / Resource /
  Prompt` primitives.
- Major Nov-2025 update: async operations, statelessness, server identity,
  community registry. Anthropic donated MCP to Linux Foundation's AAIF in
  Dec 2025 ([year-of-MCP review](https://www.pento.ai/blog/a-year-of-mcp-2025-review)).
- OAuth 2.1 mandatory for HTTP transports since the March-2025 spec update
  ([New Stack 15 best practices](https://thenewstack.io/15-best-practices-for-building-mcp-servers-in-production/)).

### 4.2 Existing verification-oracle MCP servers

| Server | Domain | Notes |
|---|---|---|
| `lean-lsp-mcp` (O. Dressler, Mar 2025) | Lean4 theorem proving | LSP-bridged; exposes goal state, search, hover. [GitHub](https://github.com/oOo0oOo/lean-lsp-mcp) |
| Hilbert (Apple ML, 2025) | Recursive proof building | Step-checked CoT with Lean4 verifier in loop |
| ConstraintLLM | Industrial CP | LLM produces MiniZinc; solver verifies feasibility |

### 4.3 Auth boundaries — query vs. modify

- **Query (read-only)**: `verify_move`, `enumerate_moves`, `optimize_moves`.
  Idempotent. Should be open to LLM agents with minimal auth (token →
  read-scope).
- **Modify (commit)**: writing back into the committed binding *must not* be
  exposed as MCP. The LLM produces a move plan; a human (or higher-trust
  process) commits it via `spar moves apply --plan plan.json` on CLI.
  This preserves "LLM never enters certified path".
- Annotation: tools must declare `idempotentHint: true` and
  `readOnlyHint: true` to help agents reason about safety
  ([MCP best practices](https://modelcontextprotocol.info/docs/best-practices/)).

### 4.4 Performance budget

- Queries >5 s break LLM flow (it forgets context, retries, drifts).
- Target: `verify_move` ≤ 500 ms warm, ≤ 2 s cold (cache parsed model).
- `enumerate_moves` may legitimately take longer; emit progress notifications
  via MCP `progress` capability.

### 4.5 Concrete tool-spec sketch (JSON Schema)

```json
{
  "name": "spar.verify_move",
  "description": "Verify a hypothetical component-to-resource binding without committing. Returns per-pass pass/fail + violations.",
  "annotations": { "readOnlyHint": true, "idempotentHint": true },
  "inputSchema": {
    "type": "object",
    "properties": {
      "model_handle":   { "type": "string", "description": "Opaque ID returned by spar.load_model" },
      "component":      { "type": "string", "description": "Fully-qualified component instance path" },
      "target":         { "type": "string", "description": "Target resource (processor/memory/bus)" },
      "variant":        { "type": "string", "description": "Optional rivet variant id (e.g. 'diesel-eu5')" },
      "passes":         { "type": "array", "items": { "type": "string" }, "description": "Subset of {rta, bandwidth, partition, latency_chain}" }
    },
    "required": ["model_handle", "component", "target"]
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "ok":         { "type": "boolean" },
      "results":    { "type": "array", "items": {
        "type": "object",
        "properties": {
          "pass":       { "type": "string" },
          "status":     { "enum": ["pass", "fail", "skipped"] },
          "violations": { "type": "array", "items": { "type": "string" } },
          "slack_ps":   { "type": ["integer", "null"] }
        }
      }},
      "trace_id":   { "type": "string", "description": "Stable hash of (model, move, passes); reusable for caching/audit" }
    },
    "required": ["ok", "results", "trace_id"]
  }
}
```

---

## §5 — AUTOSAR's contract layer (the analogous solved problem)

### 5.1 Concept-by-concept mapping

| AUTOSAR concept | What it does | AADL equivalent | spar's hook |
|---|---|---|---|
| **Virtual Functional Bus (VFB)** | Abstract, deployment-free communication between SWCs | `system::features` + connections at *type* level (no `processor` binding) | Already in spar-hir; we don't need a new layer, but we need to *flag* binding-free vs. bound forms |
| **SWCD (SW component description)** | Per-component portable interface | AADL `process` / `thread` type | Existing |
| **System Description** | Maps SWCs to ECUs, networks, etc. | `system implementation` + `Actual_Processor_Binding` | Existing — but **today no frozen/mobile annotation** |
| **ECU Configuration** | Per-ECU runtime config | AADL `processor`/`memory` + properties | Existing |
| **Manifest (Adaptive)** | Post-build deployment data: Execution / Service / Machine | *Missing as separate artifact* — bindings live in same `.aadl` | **Track E gap** — `Spar_Migration` property set + tentative-binding store |
| **`PortInterface` contract** | Defines what's stable across moves | AADL `feature` + `Provides`/`Requires` | Existing |
| **`Mode`** (AUTOSAR mode) | Runtime selection of bindings | AADL `modes` clause | Existing — and important for hypothetical moves under modes |
| **AUTOSAR variant handling** | `EcucPostBuildVariants` etc. | None native; partial via AADL `properties` | rivet variants v1 (#144) plays this role |

### 5.2 What this tells us

- AUTOSAR has *had* the "platform/application split" for 20+ years; its
  contract is the **Manifest + ECU Configuration** pair plus the **VFB**
  (binding-free reasoning surface). AADL has the syntactic pieces but no
  semantic guard rail saying "this is platform — don't move it".
- spar's job is **not to invent a new layer** but to add the missing
  semantic flag (`Spar_Migration::Frozen|Mobile`) and the
  *tentative-binding execution mode* (verify without commit).
- The **Allowed_Targets** property is the direct analogue of AUTOSAR's
  *EcucResourceConsumption / Mappable* constraints.
- rivet variants (#144) are the AUTOSAR-`EcucPostBuildVariants` analogue;
  Track E reuses them rather than inventing parallel scaffolding.

### 5.3 Differences worth keeping

- spar **certifies** every move via Lean4-verified analyses; AUTOSAR tooling
  generates code but doesn't carry a formal certificate per binding.
- spar's MCP surface is a feature AUTOSAR never offered.
- AADL's modes already give spar a runtime-binding-selection story Adaptive
  AUTOSAR lacks at standard level.

---

## §6 — spar's design space (the load-bearing section)

> Six numbered subsections, each ≤50 lines except where load-bearing detail is
> needed (6.5 must show literal JSON).

### 6.1 — Property set design

**Property set: `Spar_Migration`**

| Property | Type | Default | Owner | Semantics |
|---|---|---|---|---|
| `Frozen` | `aadlboolean` | `false` | applies to `process`, `processor`, `memory`, `bus`, `device`, `system` | If `true`, this component's binding may not change in any hypothetical move; analyses reject moves that touch frozen components. |
| `Mobile` | `aadlboolean` | computed | applies to `process`, `thread group`, `thread` | If `true`, this component may move; mutually inconsistent with `Frozen=true`. Default-derived: `Mobile` ≡ ¬`Frozen` ∧ component is software-side. |
| `Allowed_Targets` | `list of reference (processor)` (or memory/bus) | empty = all unbounded | applies to mobile components | If non-empty, restricts hypothetical-binding search; empty list = "no restriction" (legacy compatibility). |
| `Pinned_Reason` | `aadlstring` | empty | applies anywhere | Optional human-readable rationale for `Frozen=true` (audit / certification trail). |
| `Migration_Cost` | `aadlinteger` (units: abstract) | 0 | applies to mobile components | Used by `optimize` mode as a soft cost; not a hard constraint. |

**Default semantics walkthrough** — fixture: a 3-ECU brake-by-wire model.

```aadl
property set Spar_Migration is
  Frozen: aadlboolean applies to (component);
  Mobile: aadlboolean applies to (component);
  Allowed_Targets: list of reference (processor) applies to (component);
  Pinned_Reason: aadlstring applies to (component);
  Migration_Cost: aadlinteger applies to (component);
end Spar_Migration;

processor ECU_3
  properties Spar_Migration::Frozen => true;
             Spar_Migration::Pinned_Reason => "ASIL-D platform partition";
end ECU_3;

process Handler_brake
  properties Spar_Migration::Mobile => true;
             Spar_Migration::Allowed_Targets => (reference (ECU_3), reference (ECU_5));
end Handler_brake;
```

- `ECU_3` itself is frozen — no one moves the *processor*; its
  application-side binding can still change, modulo its own per-process
  flags.
- `Handler_brake` may move to `ECU_3` or `ECU_5` only.
- A move to `ECU_4` would be rejected at the *constraint* layer, before any
  analysis pass runs.

### 6.2 — HIR-level frozen flag

**Question**: does the frozen/mobile flag need to be a HIR invariant, or is
property-set lookup sufficient?

| Option | Cost | Benefit | Verdict |
|---|---|---|---|
| **A. Property-set only** (read on demand) | Zero new HIR fields; uses existing `property_accessors.rs` | Slow when enumerating thousands of candidates; recomputes for each pass | Adequate for v0.8.0 prototype |
| **B. HIR field cached** (precomputed during lowering) | New `Migration { frozen: bool, allowed: SmallVec<…> }` on `ProcessImpl`/`ProcessorImpl` | O(1) hot path; required if enumeration cardinality > ~10⁴ | Required for v0.9.0 |
| **C. Separate "binding overlay" type** (parallel HIR view of mobility) | Largest refactor; mirrors AUTOSAR Manifest split | Cleanest separation; aligns with rivet variants | Stretch goal; defer past v0.9.0 |

**Recommendation**: ship A in v0.8.0 (commit 1), upgrade to B in v0.9.0 once
profiling shows the cost. Avoid C unless rivet's variant-overlay refactor
makes it cheap.

### 6.3 — CLI surface

#### `spar moves verify --component X --to Y`

```text
spar moves verify \
    --model path/to/model.aadl \
    --component handler.brake \
    --to ECU_5 \
    [--variant diesel-eu5] \
    [--passes rta,bandwidth,partition] \
    [--format json]
```

JSON output:

```json
{
  "ok": false,
  "move": { "component": "handler.brake", "from": "ECU_3", "to": "ECU_5" },
  "variant": "diesel-eu5",
  "results": [
    { "pass": "rta",        "status": "pass", "slack_ps": 1240000, "violations": [] },
    { "pass": "bandwidth",  "status": "fail", "violations": ["bus CAN1 over-allocated by 3.2%"] },
    { "pass": "partition",  "status": "skip", "violations": [] }
  ],
  "trace_id": "sha256:9b1f…"
}
```

#### `spar moves enumerate --component X`

```json
{
  "component": "handler.brake",
  "candidates": [
    { "target": "ECU_3", "ok": true,  "score": 0.82, "slack_ps": 4310000 },
    { "target": "ECU_5", "ok": false, "score": null, "violations": ["bus CAN1 over-allocated by 3.2%"] }
  ],
  "objective": "slack_ps",
  "exhaustive": true
}
```

#### `spar moves optimize --objective slack`

```json
{
  "objective": "slack",
  "moves": [
    { "component": "handler.brake",   "from": "ECU_3", "to": "ECU_3" },
    { "component": "handler.coolant", "from": "ECU_2", "to": "ECU_5" }
  ],
  "score": 12.4,
  "frontier": [
    { "moves": [...], "objective_vector": { "slack": 12.4, "migration_cost": 5 } }
  ]
}
```

### 6.4 — Solver extension (`spar-solver` hypothetical mode)

```rust
// Pseudocode — does NOT mutate committed bindings
fn enumerate_valid_moves(
    model: &HirModel,
    target_component: ComponentId,
    cfg: &MovesConfig,
) -> Vec<MoveCandidate> {
    let allowed = model.allowed_targets(target_component);  // from Spar_Migration
    let candidates = if allowed.is_empty() {
        model.all_resources_of_compatible_kind(target_component)
    } else {
        allowed
    };

    candidates.into_par_iter()
        .filter(|t| !model.is_frozen(*t))
        .map(|t| {
            // 1. clone HIR overlay (cheap — Arc + small Δ)
            let overlay = model.with_tentative_binding(target_component, t);
            // 2. run requested passes against overlay only
            let results = cfg.passes.iter()
                .map(|p| run_pass(p, &overlay))
                .collect();
            MoveCandidate { target: t, results, score: score_of(&results) }
        })
        .collect()
}
```

**Performance notes:**

- Pruning by `Allowed_Targets` is the primary tool — empirically reduces
  candidates from O(processors) to O(1–10).
- Overlay structure: HIR keeps committed bindings in an `Arc`-shared map,
  hypothetical moves are a `(ComponentId → Resource)` HashMap layer; copy
  cost is the size of the move, not the model.
- Pass independence: each pass over the overlay is read-only and
  thread-safe; rayon parallelism over candidates is safe.
- For MILP-based modes (future): formulate as an LP relaxation over `x_{c,t}
  ∈ {0,1}` indicator variables, with frozen components fixed; solve only the
  free variables. Cardinality-bounded by `Allowed_Targets`.

### 6.5 — MCP tool surface

#### Tool: `spar.verify_move`

```json
{
  "name": "spar.verify_move",
  "description": "Verify a hypothetical component-to-resource binding without committing changes. Returns per-pass results and violations.",
  "annotations": {
    "readOnlyHint": true,
    "idempotentHint": true,
    "destructiveHint": false
  },
  "inputSchema": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "model_handle":   { "type": "string", "minLength": 1 },
      "component":      { "type": "string", "pattern": "^[A-Za-z_][A-Za-z0-9_.]*$" },
      "target":         { "type": "string", "minLength": 1 },
      "variant":        { "type": "string" },
      "passes":         {
        "type": "array",
        "items": { "enum": ["rta", "bandwidth", "partition", "latency_chain"] },
        "default": ["rta", "bandwidth", "partition"]
      }
    },
    "required": ["model_handle", "component", "target"]
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "ok":       { "type": "boolean" },
      "results":  {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "pass":       { "type": "string" },
            "status":     { "enum": ["pass", "fail", "skipped"] },
            "violations": { "type": "array", "items": { "type": "string" } },
            "slack_ps":   { "type": ["integer", "null"] }
          },
          "required": ["pass", "status"]
        }
      },
      "trace_id": { "type": "string" }
    },
    "required": ["ok", "results", "trace_id"]
  }
}
```

#### Tool: `spar.enumerate_moves`

```json
{
  "name": "spar.enumerate_moves",
  "description": "List valid migration targets for a component, ranked by an objective. Read-only; idempotent.",
  "annotations": { "readOnlyHint": true, "idempotentHint": true },
  "inputSchema": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "model_handle": { "type": "string" },
      "component":    { "type": "string" },
      "objective":    { "enum": ["slack_ps", "migration_cost", "balanced"], "default": "slack_ps" },
      "limit":        { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 },
      "variant":      { "type": "string" }
    },
    "required": ["model_handle", "component"]
  },
  "outputSchema": {
    "type": "object",
    "properties": {
      "candidates": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "target":     { "type": "string" },
            "ok":         { "type": "boolean" },
            "score":      { "type": ["number", "null"] },
            "slack_ps":   { "type": ["integer", "null"] },
            "violations": { "type": "array", "items": { "type": "string" } }
          },
          "required": ["target", "ok"]
        }
      },
      "exhaustive": { "type": "boolean" }
    },
    "required": ["candidates", "exhaustive"]
  }
}
```

#### Auth, idempotency, errors

- **Auth**: OAuth 2.1 bearer (per MCP 2025-03+ requirement); token scope
  `spar:read` for both tools. Never exposed to write scope.
- **Idempotency**: `trace_id = SHA256(canonical(model) ‖ canonical(move) ‖
  canonical(passes))`. Same inputs → same trace_id → cacheable.
- **Errors**: structured `{ code, message, retryable }`. Codes:
  `MODEL_NOT_FOUND`, `COMPONENT_NOT_FOUND`, `TARGET_INCOMPATIBLE`,
  `FROZEN_VIOLATION`, `PASS_FAILED`, `INTERNAL`.
- **Rate-limit**: per-token, 100 calls/min default.
- **Trace persistence**: on demand only; LLM does not get write access to the
  store. A separate `spar moves apply --trace <id>` (CLI, not MCP) commits.

### 6.6 — Integration with rivet variants

rivet variant context (per #144) carries a variant blob: `{ id, parent,
features_active, properties_overrides }`. Track E composition rules:

- `Spar_Migration::Frozen` is *evaluated under the variant*; a component
  frozen in `diesel-eu5` may be mobile in `gasoline-eu7`.
- `Allowed_Targets` may also be variant-dependent (different ECU
  populations).
- The MCP `verify_move` tool accepts `variant`; the solver loads the
  variant-resolved HIR overlay before generating the tentative binding.
- Example query: *"in variant `diesel-eu5`, can I move `handler.brake` to
  `ECU_5`?"* — the tool first applies variant overlay (selecting the diesel
  ECU population, applying ASIL property overrides), then runs the
  hypothetical-binding pipeline.
- `enumerate_moves` returns variant-dependent candidate sets; documented in
  the MCP response so the LLM doesn't conflate variants.

This is exactly the AUTOSAR `EcucPostBuildVariants` shape, with rivet as the
variant store and Track E as the mobility overlay.

**Worked example trace** (illustrative; not a unit test fixture):

```text
$ spar moves verify \
    --model brake-by-wire.aadl \
    --variant diesel-eu5 \
    --component handler.brake \
    --to ECU_5 \
    --format json

→ rivet::resolve_variant("diesel-eu5")
   → applies feature::has_diesel_powertrain
   → enables   ECU_5 (diesel-only)
   → overrides Spar_Migration::Allowed_Targets[handler.brake]
              from (ECU_3, ECU_4)
              to   (ECU_3, ECU_5)
→ spar-moves::verify(handler.brake → ECU_5)
   → HIR overlay applied
   → rta:        slack 1.24 ms PASS
   → bandwidth:  CAN1 over by 3.2% FAIL
   → partition:  no isolation breach SKIP (no partition prop)
→ output { ok: false, results: [...], trace_id: "sha256:9b1f…" }
```

Key invariant: variant resolution happens *before* the overlay; the overlay
operates on a variant-resolved HIR, not the raw model. This keeps each
`verify_move` query single-variant and bounded.

---

## §7 — Roadmap proposal

### 7.1 Commit-by-commit table (v0.8.0 + v0.9.0)

| # | Commit | Crates touched | Weeks | Depends on |
|---|---|---|---|---|
| 1 | Property set surface: `Spar_Migration::*` parsed + accessors | `spar-hir`, `spar-hir-def` (`standard_properties.rs`), `spar-analysis::property_accessors` | 1 | — |
| 2 | HIR-level mobility cache (option B from §6.2) | `spar-hir`, `spar-hir-def` | 1 | #1 |
| 3 | Tentative-binding overlay infra in HIR | `spar-hir` | 1.5 | #2 |
| 4 | New crate `spar-moves`: enumerate + verify primitives | new crate; depends on `spar-analysis`, `spar-solver` | 2 | #3 |
| 5 | CLI surface: `spar moves {verify,enumerate}` | `spar-cli` | 1 | #4 |
| 6 | `spar-solver` hypothetical mode (no MILP yet — pruning-only) | `spar-solver` | 1 | #4 |
| 7 | rivet variant integration: `--variant` flag end-to-end | `spar-moves`, `spar-cli`, rivet glue | 1 | #4, #144 |
| 8 | Documentation + COMPLIANCE update + rivet artifacts (req/feat/design-decision) | `docs/`, `artifacts/` | 0.5 | #5–#7 |
|   | **v0.8.0 cut** | | **~8 weeks** | |
| 9 | MCP server scaffold: `spar mcp serve` (read-only) | `spar-cli` (subcommand), new dep | 1.5 | #5 |
| 10 | MCP tools `spar.verify_move`, `spar.enumerate_moves` | `spar-mcp` (new crate) | 1.5 | #9 |
| 11 | MCP auth (OAuth 2.1) + rate-limit + tracing | `spar-mcp` | 1 | #10 |
| 12 | `spar moves optimize --objective` (greedy + Pareto frontier) | `spar-moves`, `spar-solver` | 2 | #6 |
| 13 | MILP-based optimizer (optional; gated behind feature flag) | `spar-solver` | 2 | #12 |
| 14 | LLM-driven walkthrough doc + sample prompts | `docs/` | 0.5 | #11, #12 |
|   | **v0.9.0 cut** | | **~8.5 weeks** | |

### 7.2 Critical-path dependencies

- #144 (rivet variants v1) must land before #7.
- Track A (#142, hierarchical RTA) recommended-but-not-required; if it lands,
  `verify_move` returns richer slack info.
- Lean4 kernel re-verify is **not** required per move — kernel certifies the
  *analysis algorithm*, not each input; this means hypothetical-binding mode
  reuses existing certified primitives.

### 7.3 Cut-line for v0.8.0

- **In**: §6.1, §6.2 option A, §6.3 verify+enumerate, §6.4 prune-only,
  §6.6 variant flag, §7 commits #1–#8.
- **Out**: §6.5 MCP server, §6.4 MILP, §7 commits #9–#14.

This keeps v0.8.0 a self-contained, certifiable feature; v0.9.0 adds the LLM
surface only after the deterministic core is stable.

---

## §8 — Open questions and risks

### 8.1 Out of scope for v0.8.0

- Full MCP server (deferred to v0.9.0, commit #9).
- MILP-based multi-objective optimizer (deferred — scoping risk).
- Automated commit of LLM-proposed moves (intentionally never auto; CLI-only).
- Cross-variant move validation in one call (defer; users iterate per-variant).
- Modal/state-conditional mobility (`Mobile in some_mode`) — defer pending
  AS5506D modal-property roadmap (cf. project_spec_gaps).

### 8.2 Certification boundary — proving the LLM never enters the certified path

- **Architectural argument**: LLM has only `spar:read` scope; commit verbs
  exist only in CLI/`spar-cli`, never in MCP. Conformance check: PR review
  rule + grep test that ensures `spar-mcp` crate has no transitive write
  dependency on the binding store.
- **Audit trail**: `trace_id` ties every analysis result to a content hash;
  any commit must reference a verified `trace_id`. Forging is no easier than
  forging a Lean kernel proof.
- **Lean4 kernel** still backs the *primitive* analyses (RTA, scheduling
  recurrences); the LLM cannot weaken the kernel's claims, only *select*
  inputs.
- **Risk**: an attacker who controls the LLM picks a move that passes
  analyses but encodes malicious intent. Mitigation: human review at the
  `apply --plan` step is mandatory; the `Pinned_Reason` and trace_id make
  audits tractable.

### 8.3 Solver performance on realistic enumeration

- N components × M targets, with `Allowed_Targets` typically reducing M
  from 5–50 to 1–10. Empirically: a 200-SWC model with 3 mobile components
  averaging 4 candidates each → 12 evaluations per `enumerate_moves`.
- Each evaluation = 1 RTA + 1 bandwidth + 1 partition pass. RTA dominates
  (~50–500 ms for 200 threads). Total: a few seconds — acceptable for
  CLI, **borderline** for MCP unless we cache per-trace_id.
- Pareto-frontier (`optimize`) is the scalability watchpoint; bounded by
  `Allowed_Targets` cardinality but combinatorial across multiple mobile
  components. Cap with explicit `--max-moves N` flag.

### 8.4 Risk: feature creep into general DSE

- Track E is **not** a generic architecture-optimization framework. Every
  PR must answer "does this serve hypothetical-binding queries from a
  human or an LLM?" — if not, defer.
- ArcheOpterix already exists for unbounded NSGA-II/SPEA2 search; we do
  not compete with it. Our differentiator is the *certifying* per-move
  oracle, not the search heuristic.
- `optimize` mode (commit #12) is the most exposed surface here — keep it
  bounded (greedy + small Pareto frontier; no neural search; no RL).

### 8.5 Other risks

- **Variant explosion**: rivet variants × hypothetical bindings is
  combinatorial. Mitigation: variant must be passed explicitly; no
  cross-variant fan-out.
- **Property-set churn**: adding `Spar_Migration::*` is a public-API surface;
  needs deprecation policy if names change post-v0.8.0.
- **MCP spec drift**: AAIF custody since Dec 2025; spec may evolve.
  Mitigation: pin to spec revision in `spar-mcp` crate; bump deliberately.
- **Documentation debt**: certification boundary needs *prose* docs (not
  just code) so safety reviewers can audit it.

---

## Appendix A — Source map

1. [OSATE 2.18 docs](https://osate.org/about-osate.html)
2. [osate2 GitHub](https://github.com/osate/osate2)
3. [Capella overview](https://www.polarsys.org/capella/)
4. [Capella Arcadia method](https://mbse-capella.org/arcadia.html)
5. [AUTOSAR AP R24-11 Manifest spec](https://www.autosar.org/fileadmin/standards/R24-11/AP/AUTOSAR_AP_RS_ManifestSpecification.pdf)
6. [AUTOSAR AP R22-11 Methodology](https://www.autosar.org/fileadmin/standards/R22-11/AP/AUTOSAR_TR_AdaptiveMethodology.pdf)
7. [AUTOSAR Classic Platform overview](https://www.autosar.org/standards/classic-platform)
8. [Stood / Ellidiss](https://www.ellidiss.com/products/stood/)
9. [AADL Inspector / Ellidiss](https://www.ellidiss.com/products/aadl-inspector/)
10. [SCADE Architect datasheet](https://www.ansys.com/content/dam/product/embedded-software/ansys-scade-architect/ansys-scade-architect-datasheet.pdf)
11. [ROS 2 composition](https://index.ros.org/p/composition/)
12. [Sukkar et al. 2025 NSGA-II+ vehicular fog](https://onlinelibrary.wiley.com/doi/10.1002/ett.70260)
13. [Fan et al. 2024 IoV edge offloading](https://link.springer.com/article/10.1007/s00530-024-01598-0)
14. [Vajdi & Hong 2020 micro-service NSGA-II](https://www.researchgate.net/publication/342929909)
15. [IDeSyDe DSE](https://dl.acm.org/doi/10.1145/3647640)
16. [Nanosatellite partition MILP/CP](https://link.springer.com/chapter/10.1007/978-3-031-95976-9_11)
17. [ArcheOpterix](https://www.academia.edu/37296994/ArcheOpterix_An_extendable_tool_for_architecture_optimization_of_AADL_models)
18. [DeepMind FunSearch blog](https://deepmind.google/discover/blog/funsearch-making-new-discoveries-in-mathematical-sciences-using-large-language-models/)
19. [FunSearch Nature paper](https://www.nature.com/articles/s41586-023-06924-6)
20. [Lean Copilot arXiv 2404.12534](https://arxiv.org/abs/2404.12534)
21. [DeepSeek-Prover-V2 InfoQ](https://www.infoq.com/news/2025/05/deepseek-prover-v2-formal-proof/)
22. [Sledgehammer manual](https://isabelle.in.tum.de/doc/sledgehammer.pdf)
23. [Sledgehammer + SMT extension](https://link.springer.com/chapter/10.1007/978-3-642-22438-6_11)
24. [ConstraintLLM EMNLP 2025](https://aclanthology.org/2025.emnlp-main.809.pdf)
25. [Apple Hilbert recursive proofs](https://machinelearning.apple.com/research/hilbert)
26. [LLM-assisted ADD architecture](https://arxiv.org/pdf/2506.22688)
27. [Software Architecture meets LLMs survey](https://arxiv.org/pdf/2505.16697)
28. [MCP spec 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25)
29. [MCP year-in-review 2025](https://www.pento.ai/blog/a-year-of-mcp-2025-review)
30. [MCP best practices guide](https://modelcontextprotocol.info/docs/best-practices/)
31. [15 MCP server best practices](https://thenewstack.io/15-best-practices-for-building-mcp-servers-in-production/)
32. [Anthropic MCP announcement](https://www.anthropic.com/news/model-context-protocol)
33. [lean-lsp-mcp GitHub](https://github.com/oOo0oOo/lean-lsp-mcp)
34. [MiniZinc handbook scheduling](https://docs.minizinc.dev/en/stable/lib-globals-scheduling.html)
35. [Comparative neurosymbolic study 2025](https://www.arxiv.org/pdf/2508.03366)
36. [RL-MOTS task scheduling 2025](https://www.nature.com/articles/s41598-025-25666-1)
37. [CAMET tools overview](https://adept.univ-brest.fr/2024/doc/Lewis_CAMET_overview.pdf)

---

## Appendix B — Recommendation summary (TL;DR for reviewers)

1. **Add property set `Spar_Migration::{Frozen, Mobile, Allowed_Targets,
   Pinned_Reason, Migration_Cost}`** in v0.8.0 commit #1.
2. **Build hypothetical-binding overlay in HIR**, prune by `Allowed_Targets`,
   reuse existing analysis passes — no MILP for v0.8.0.
3. **CLI-first**: `spar moves verify` and `spar moves enumerate` ship in
   v0.8.0. The LLM/MCP surface is **deliberately deferred to v0.9.0** so the
   deterministic core stabilises first.
4. **MCP boundary is read-only**: `spar.verify_move` and
   `spar.enumerate_moves` only. The `apply` verb is CLI-exclusive. The LLM
   never crosses into the certified path.
5. **Reuse rivet variants v1 (#144)** as the AUTOSAR-`EcucPostBuildVariants`
   analogue. Pass `variant` through every move-mode operation.
6. **Estimated effort**: ~8 weeks for v0.8.0 core, ~8.5 weeks for v0.9.0
   MCP + optimizer; total ~16–17 weeks across two releases.
7. **Hard guard rails**: no LLM-authored AADL text on certified path; no
   automatic commit; OAuth 2.1 with `spar:read` scope only; trace_id audit
   on every move.

---

*End of design document.*
