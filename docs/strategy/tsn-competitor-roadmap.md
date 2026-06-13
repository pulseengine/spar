# TSN-competitor roadmap (v0.16 → v0.22+)

_Release-by-release delivery plan for the OSS TSN-config-synthesis direction the
maintainer chose on 2026-06-13 (see
[`flync-arxml-tsn-strategy.md`](./flync-arxml-tsn-strategy.md) for the decision
and evidence trail). This document is the **plan brought for sign-off**; no rivet
scope is captured until the maintainer approves the tiers and sequencing below._

_Confidence tags are the researchers' and are carried verbatim from four rounds of
primary-sourced research: **[SOLID]** = read in a primary/multiple source;
**[LIKELY]** = one decent source or strong negative search; **[UNVERIFIED]** =
single weak source. **Tags travel next to the claim on purpose** — the maintainer's
decision overrode the "wait for a named buyer" *park*, not the underlying *risk*._

---

## Maintainer reframing (2026-06-13): OSS-first, cross-PulseEngine substrate

The sign-off broadened the framing in a way that **materially de-risks Tier 2**.
The decision is **not** "an automotive TSN product chasing one buyer." It is:

- **OSS-first, anchored on Tiers 1 + 3** — the ingest + network-calculus + proof
  *substrate* is the durable core; TSN-config synthesis (Tier 2) is **one
  application among several**, not the whole bet.
- **The timing/NC math is already needed elsewhere in PulseEngine.** The
  maintainer named concrete internal demand: **WCET analysis** and the **math
  behind kiln's async scheduler**. So Tier 1 (PLP/TFA) and Tier 3 (min-plus
  proofs) are not speculative — they have an existing internal customer. This is
  the single biggest change: the "no named buyer" objection against Tier 2 does
  **not** apply to the substrate, which has pull regardless.
- **Application domains broaden beyond automotive/aerospace** — explicitly:
  **home automation** (with Wohl) and **music studios**. The latter matters:
  **pro-audio AVB is the *original* 802.1 use case** (802.1Qav CBS was born from
  audio/video bridging, which spar already implements — REQ-TSN-003). These are
  OSS-native, latency-sensitive domains with neither aerospace's AFDX lock-in nor
  automotive's ~0-proof-demand testing culture — they sidestep the known risk
  rather than confronting it.
- **The substrate must be FABRIC-AGNOSTIC, not TSN-only.** Drone software in the
  **relay** and **jess** repos is outgrowing single-small-processor designs and
  going **multi-fabric — Ethernet + air/wireless + PCIe + CAN**. The
  network-calculus + timing engine therefore has to model arbitrary fabrics, not
  just 802.1 TSN. This is the original "extend to any WCTT/network technology,"
  now concrete with named internal consumers.
- **"We will need this for more"** — treat the substrate as extensible to any
  WCTT/network technology and any consuming application, not scoped to TSN.

Net effect on the tiering: **Tiers 1 and 3 graduate from "low-regret" to
"already-demanded internal substrate."** Tier 2 remains the product bet, but now
sits on a foundation that pays for itself through other applications even if the
TSN-config product never finds a single enterprise buyer. That is exactly the
OSS thesis (adoption = users/contributors, not one buyer) made concrete.

---

## How to read this: three tiers, different epistemic status

The honest evidence does **not** flatten into one committed version-by-version
line. It splits into three tiers that must stay visibly separate, because they
carry different risk and the sign-off is only meaningful if the bets are surfaced,
not buried:

| Tier | What | Risk | Gated on a buyer? |
|---|---|---|---|
| **1 — Committed spine** | ARXML/DBC ingest + PLP/TFA++ network-calculus upgrade | Low ([SOLID], stands alone) | **No** — pure analyzer wins, build regardless |
| **2 — The product bet** | TSN synthesis → config export | Medium–high (the gamble the maintainer is approving) | Overridden, but kill-gated per release |
| **3 — Internal soundness** | 4 Lean `sorry`s → min-plus certificate checker | Research (parallel, never gating, never marketed) | n/a |

**The trap this document refuses:** the maintainer deciding to *pursue* the
product bet did not make the [UNVERIFIED] findings true. Green-field items are
labelled green-field (research bets we'd be *inventing*, not scheduled
deliverables). And **nowhere does this roadmap promise a "verified end-to-end
delay bound"** — a certificate would certify the *min-plus computation / a
computation trace*, not a proven end-to-end delay. The moat's proof-third arrives
*late*; near-term releases ship the OSS + AADL-native two-thirds.

### Research log (2026-06-13) — validations that *confirmed* the plan

- **ARXML ingest stays on the direct `autosar-data` track; do NOT route it
  through sysml2.** Background research found **no ARXML→SysML v2 path** in any
  vendor, OSS project, OMG working group, or paper as of mid-2026 — all real flow
  is one-way *SysML→AUTOSAR refinement*, and reverse ingest is a synthesis/
  abstraction problem (the **same category mismatch as DBC→YANG**). Routing ARXML
  through sysml2 would mean inventing the very abstraction step the industry has
  not built. `spar-sysml2` and REQ-INGEST-ARXML-001 stay **peer front-ends**.
  [LIKELY — negative result across the angles checked]
- **SysML v2 is now a finalized standard but a moving grammar.** OMG final
  adoption **2025-07-21**, published **2025-09-03**; the textual notation still
  ships **monthly** Pilot-Implementation tags (current **2026-04**), with at least
  one breaking change (exponentiation `**`/`^` is now right-associative). So
  `spar-sysml2` tracks a *stabilized-but-not-frozen* target → it must **grammar-
  diff its pinned tag against a current tag** (prose changelogs are insufficient).
  Captured as **REQ-INGEST-SYSML2-DIFF-001** (v0.19.0). [SOLID on adoption dates]
- **kiln cross-repo dependency triaged.** spar issue **#272** (the maintainer's
  own filing, correctly in spar where the proofs live — *not* in kiln) asks spar
  to expose its fully-proved scheduling theory (RTA/RMBound/EDF) as a reusable
  Lake/Bazel `lean_library` boundary for kiln-async's fuel-quantum scheduler.
  This is the **scheduling analog of the NC-substrate export** — concrete
  evidence for the cross-PulseEngine-substrate thesis above. Captured as
  **REQ-PROOF-SCHED-002** (v0.18.0), building on the already-shipped
  REQ-PROOF-SCHED-001. [SOLID]

---

## Tier 1 — Committed spine (build regardless of the product bet)

These two are the highest value-to-risk ratio in the whole analysis. Each stands
on its own: every analysis pass spar already has gets more valuable the moment it
can read real models (ingest) and produce tighter, generic-topology bounds (PLP).
Neither depends on any TSN-product decision.

### 1a. ARXML / DBC ingest

- **What:** read shipped automotive/industrial models, not just hand-written
  `.aadl`. ARXML uniquely carries multi-bus topology *and* TIMEX timing
  constraints; DBC is the cheap CAN-only secondary.
- **How (license-clean, no XSD to bundle):** `autosar-data` +
  `autosar-data-specification` (dual MIT/Apache, schema derived from official
  XSDs) for ARXML; `can-dbc` (MIT/Apache) for DBC. [SOLID]
- **Not differentiating on its own** — RTaW already does ARXML→TSN. Its role is
  **load-bearing enabler**: it carries spar's engine to where TSN demand actually
  is (automotive/industrial), since AADL's own user base leans aerospace/AFDX.
- **Under-explored sliver** (optional, later): the AADL↔AUTOSAR component/ECU
  bridge — no direct AADL→ARXML transform exists; EAST-ADL↔AUTOSAR is the
  precedent. [LIKELY]
- **Kill-criterion (carried verbatim):** kill ARXML-beyond-DBC if the AADL↔ARXML
  mapping needs full SWC/TIMEX fidelity we can't justify *and* DBC carries enough
  for the target analysis.

### 1b. PLP + FP-TFA/TFA++ network-calculus upgrade

- **What:** add **PLP (polynomial LP, generic topology)** and **FP-TFA/TFA++** to
  the network-calculus backend. This is a **pure analyzer win** — it *dominates
  the TFA spar already has*: PLP bounds are always ≤ TFA and converge where TFA
  doesn't at high utilization. [SOLID — Bouillard arXiv:2010.09263 2021;
  Tabatabaee/Bouillard/Le Boudec arXiv:2208.11400 2023]
- **Why early:** highest value-to-risk in the report. Generic topology where the
  existing LUDB/PMOO are tandem-only; tractable; zero dependency on synthesis.
  FP-TFA also serves as PLP's initialization phase and gives valid bounds under
  cyclic dependencies.
- **Optional extension:** PLP-DRR for best-known DRR-class bounds (IEEE/ACM ToN
  2024) — only if DRR traffic classes matter to a target. [SOLID]
- **Audit-before-trust caveat (carried verbatim):** Jiang (arXiv:2403.13656,
  2024) shows packetization was overlooked in standard CBS/SP service curves,
  **invalidating some bounds**. Audit spar's CBS service curves before any
  synthesis or export leans on them. [SOLID]
- **Validation target:** check our PLP output against Panco / Saihu reference
  implementations.

---

## Tier 2 — The product bet (the gamble being approved)

This is **the decision the maintainer is signing off**. The synthesis vacuum is
real ([SOLID]: no Rust project synthesizes Qbv GCLs; outside RTaW/Slate no vendor
synthesizes at all). But the buyer-value question the research could **not**
answer remains open, so each step carries an explicit kill-gate. Sequence:
synthesis first (it must exist before anything can export it), then config export.

### 2a. TSN synthesis — on the existing HiGHS / good_lp MILP backend

Buildable, citable algorithms (NOT green-field), slotted by scale strategy:

- **MIP → VNS-GA handoff** (Wang et al., *Sensors* 2025): exact MIP ≤ ~1,000
  flows, then VNS-GA (genetic + 2-opt/or-opt/exchange) up to ~3,000 flows. The
  cleanest concrete recipe, sits directly on our MILP backend. [SOLID]
- **CQF / Multi-CQF synthesis** — the simpler-config, larger-scale direction with
  real 2026 momentum (TCQF adopted as IETF DetNet WG item
  `draft-ietf-detnet-tcqf-00`, 14 Jan 2026; CENI 2,000 km / 100 Gbps). Pick one:
  - *Hyper-flow graph decomposition* (arXiv:2309.06690, 2023): 2,000 flows,
    <100 ms @1,000 flows. [SOLID]
  - *GASA injection planning* (Debnath/Steinhorst arXiv:2506.22671, 2025):
    GA+SA hybrid, +15% flows scheduled. [SOLID]
- **Qbv direct, smaller scale:** dependency-aware priority adjustment
  (arXiv:2407.00987, 2024) — 300 flows, mixed-criticality, +20.6% success. [SOLID]
- **Online/incremental admission** (later): NC deadline-adaptive admission
  (arXiv:2503.09093, 2025) — accept/reject new flows without full recompute;
  this is the reconfiguration story tsnkit lacks entirely. [SOLID relative]

**Green-field — labelled as research bets, NOT scheduled deliverables:**
- A TSN **ALNS / GRASP** GCL synthesizer: *no prior art exists* (negative search,
  [SOLID, absence]). Genuinely novel if pursued — but it's an invention, not a
  citable algorithm to slot into a version line.

- **Kill-criterion (carried verbatim):** kill the AADL→TSN bridge if we can't
  articulate why our integrated architecture-to-timing story beats RTaW's
  already-cert-accepted ARXML→TSN+NC.

### 2b. Config export (802.1Qcw-YANG / NETCONF)

- **Only after 2a synthesizes something to export.** "Emit YANG" was the weakest
  standalone case in the analysis (and the FLYNC-YANG premise was false — FLYNC
  emits no YANG). It earns its place only as the output stage of a working
  synthesizer.
- **Kill-criterion:** if no synthesis target consumes the export, it's premature.

---

## Tier 3 — Internal soundness ratchet (parallel, never gating, never marketed)

- **What:** discharge the 4 Lean `sorry`s in `proofs/Proofs/Network/MinPlus.lean`
  (`backlog_bound_classical`, `delay_bound_classical`, `output_dominates_input`,
  `compose_delays_dominates`, all `TODO(v1.0.0)`), then a min-plus **certificate
  checker** (Minerve / Isabelle-style: untrusted engine emits a trace → trusted
  checker re-validates it).
- **The honest ceiling (carried verbatim, non-negotiable):** such a certificate
  certifies **the min-plus computation / trace, NOT a proven end-to-end delay
  bound**. Every machine-checked NC artifact in the literature stops here; the
  operation→delay theorem link is **not yet mechanized by anyone**, in any prover
  (Lean has *nothing* in NC — [SOLID, absence]). The synthesize→certificate
  pipeline at the *bound* level is green-field; CertiCAN (CAN, not NC) is the only
  template.
- **Marketing rule (carried verbatim):** do **not** market "verified network
  calculus" until the sorries are discharged. Keep the proof-carrying angle
  internal unless a concrete cert-driven buyer asks — none found yet.
- **Why it's the moat's third leg:** {machine-checked min-plus certificate} ×
  {AADL-native} × {open-source} is a triple that exists nowhere. Near-term
  releases ship the AADL-native + OSS two-thirds; the proof leg completes *late*.

---

## Indicative release slotting (for discussion — NOT yet committed scope)

This is a **strawman sequencing** to make the sign-off concrete. Exact version
boundaries are the maintainer's call; the ordering constraints (ingest & PLP
before synthesis; synthesis before export; proofs in parallel) are what matter.

| Release | Tier 1 (spine) | Tier 2 (product bet) | Tier 3 (proof) |
|---|---|---|---|
| **v0.16.0** | _(shipping now — parser-gap closure)_ | — | — |
| **v0.17.0** | DBC ingest (`can-dbc`); FP-TFA/TFA++ | — | (continue) |
| **v0.18.0** | ARXML ingest (`autosar-data`); **PLP** | — | — |
| **v0.19.0** | PLP validation vs Panco/Saihu; CBS-curve audit | Qbv direct synth (dep-aware, 300-flow) | sorry #1–2 |
| **v0.20.0** | (AADL↔AUTOSAR sliver?) | MIP→VNS-GA spine; CQF decomposition | sorry #3–4 |
| **v0.21.0** | EMV2 annex-path (REQ-PLUGFEST-002/005) | Config export (YANG/NETCONF) | min-plus checker |
| **v0.22.0+** | — | Online admission; ALNS (green-field bet) | certificate→synth wire |

_Parser/oracle backlog already in flight (REQ-PLUGFEST-002 EMV2 redo →v0.21.0,
REQ-PLUGFEST-005 EMV2 annex-path →v0.21.0) is shown so the TSN tracks don't
silently displace committed parser scope._

---

## What the sign-off actually approves

1. **Tier 1 is low-regret** — approve to start ingest + PLP now, independent of
   everything else. (Recommended yes regardless.)
2. **Tier 2 is the bet** — approving it means committing roadmap weight to the
   TSN-config product *without a named buyer*, on the strength of the synthesis
   vacuum + spar already owning the two hardest pieces. The kill-gates above are
   the exits. **This is the real decision.**
3. **Tier 3 runs in parallel** — internal correctness asset; never on the
   critical path of a shipped feature; never marketed until the sorries close.

**Open question the research cannot answer and the maintainer owns:** which
segment buys this, and what does an AADL front-end give that buyer that RTaW's
ARXML front-end doesn't? Plausible: aerospace/defense already models in
AADL/OSATE and RTaW doesn't reach there — but that segment leans AFDX/TTEthernet
over 802.1 TSN. As OSS the bar is users/contributors, not a single enterprise
buyer — a lower and different bar than the commercial tools face.
