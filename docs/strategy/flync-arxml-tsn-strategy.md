# FLYNC / ARXML / TSN ingest-emit-validate strategy

_Decision memo. Drafted 2026-06-13. Evidence: three rounds of fan-out web research
(10 subagents, primary-sourced) + a direct audit of the spar codebase. Confidence
tags below are the researchers': **[SOLID]** = multi-source/primary; **[LIKELY]**
= one decent source or a strong-but-negative search; **[UNVERIFIED]** = could not
confirm._

> ## DECISION (2026-06-13, maintainer): build the OSS TSN competitor
>
> The maintainer has chosen the **TSN-config-synthesis product direction** — an
> open-source Rust competitor to tsnkit / TTTech Slate / RTaW-Pegase, with AADL
> as the front-end/IR and ARXML/DBC as the ingest feed, extensible to any
> WCTT/network technology. This **overrides** the earlier "park Decision 2 until
> a named buyer" caution below. Round-3 evidence (tsnkit audit, commercial
> matrix, codebase audit) makes the bet defensible:
>
> - **The synthesis vacuum is real.** Outside RTaW-Pegase and TTTech Slate, *no*
>   shipping vendor synthesizes TSN schedules; silicon/switch vendors are config
>   loaders, and Vector PREEvision (an MBSE front-end) *delegates synthesis to
>   RTaW via ARXML*. **No Rust project synthesizes Qbv GCLs today.** [SOLID]
> - **spar already owns the two hardest pieces** a credible competitor needs and
>   tsnkit lacks: an **MBSE/AADL front-end** and **sound network-calculus
>   verification** (PMOO/LUDB/TFA in `spar-network`, wired through `wctt.rs`).
>   tsnkit (17 offline schedulers, TAS-only, MIT, research-alpha) has *no* NC
>   bounds, *no* verification, *no* cert, *no* online reconfig, weak config
>   export. [SOLID]
> - **The defensible moat is a triple, not a single axis:** {machine-checked
>   NC-bound proof} × {AADL-native} × {open-source}. "Synthesis" alone = a worse
>   RTaW; "qualified tool" alone is occupied (TTTech ships a DO-330 TQL-4
>   package). But **"synthesize a GCL → emit a machine-checkable certificate of
>   its NC latency bound, end-to-end" exists nowhere** — RTaW's "mathematically
>   verified" is sound DNC + an Isabelle *result-checker prototype*, not a
>   shipped proof. The 4 Lean `sorry`s sit exactly in front of this gap. [SOLID]
>
> **The known risk, and why the chosen path mitigates it:** AADL users live in
> aerospace (AFDX), while TSN-config demand is automotive/industrial (ARXML/
> AUTOSAR). The AADL→TSN absence *may* reflect "AADL users aren't the TSN buyers"
> rather than pure opportunity [UNVERIFIED — firm up via RTSS/RTAS/WFCS]. The
> **ARXML/DBC ingest track is therefore load-bearing, not optional**: it carries
> spar's engine to where the demand actually is (exactly RTaW's own play — ARXML
> in, internal model, synthesize out). AADL becomes spar's IR + an authoring
> option, not the sole input. And as **OSS**, adoption needs users/contributors,
> not a single enterprise buyer — a different and lower bar than the commercial
> tools face.
>
> Full release roadmap (v0.16→v0.22+) and rivet requirements: see
> [`tsn-competitor-roadmap.md`](./tsn-competitor-roadmap.md) (in progress).
> The analysis below predates the decision and is retained as the evidence trail.

## TL;DR — the honest version

The original framing was **ingest (ARXML→AADL) vs emit (AADL→YANG, FLYNC-style)
vs validate (proof-carrying TSN config as a service)**. Round-2 evidence forces
three corrections that shrink the opportunity from "own a category" to "occupy a
narrow, defensible niche — if we choose to invest":

1. **spar does not today have machine-checked network calculus.** PMOO/LUDB are
   unverified Rust (good_lp/HiGHS LP). The Lean NC proofs carry **four `sorry`s**
   on the foundational theorems (`backlog_bound_classical`,
   `delay_bound_classical`, `output_dominates_input`, `compose_delays_dominates`),
   all `TODO(v1.0.0)`. `COMPLIANCE` states the unverified Rust is "the
   load-bearing artifact for NC bounds." → The "verified network calculus" wedge
   is a **roadmap item, not an asset**, and claiming it now would overclaim. [SOLID]

2. **Demand for proof-carrying (Tier-3) timing certificates is near-zero today.**
   The market pays for **Tier-2 = sound analysis from a qualified tool** (AbsInt
   aiT WCET; AFDX network-calculus reports accepted for A380/A350 cert). Authorities
   already accept qualified-tool output *without re-checking it*, so an
   independently re-checkable certificate "solves a trust problem tool
   qualification already solved." Money lives one tier below the thesis. [SOLID on
   tier structure; LIKELY on automotive-TSN proof demand being ~0]

3. **"ARXML→TSN" and "verified TSN synthesis" are not green field.**
   RTaW-Pegase already ingests ARXML/PREEvision → synthesizes 802.1Qbv schedules
   (ZeroConfig-TSN) → verifies via network calculus, certification-accepted in
   aerospace. tsnkit (17 algorithms), TTTech Slate (Z3) also synthesize. None emit
   a machine-checked certificate — but the *synthesis* and the *NC-bound
   verification* are occupied. [SOLID]

**The one genuinely novel, defensible combination** = an **AADL/MBSE front-end
driving TSN analysis whose output carries a machine-checked proof.** Both pieces
are individually open (no MBSE→TSN bridge from AADL; no proof-carrying GCL in any
prover), and the *combination* is unoccupied. But it faces a real
value-proposition objection ("why is a proof worth more than RTaW's
already-cert-accepted NC bounds?") and requires finishing the Lean NC proofs we
don't yet have.

## Your "DBC → YAML → YANG?" instinct was correct

That chain is a **category error fused with a disguised synthesis problem**:
- DBC↔FLYNC-YAML is a real, *implemented* in-domain CAN conversion (FLYNC ships a
  bidirectional `dbc_converter.py`). [SOLID]
- **YAML→YANG does not exist in FLYNC** (zero `yang`/`netconf`/`802.1Qcc` refs in
  their codebase) and *cannot* be mechanical: DBC = CAN serial-bus signals;
  802.1Qcc YANG = Ethernet TSN streams/gate-schedules. Bridging them means
  *inventing* talkers/listeners, VLANs, priorities, and GCLs — **synthesis, not
  transcode**. So "emit YANG like FLYNC does" rests on a false premise: FLYNC
  doesn't emit YANG. [SOLID]

## Per-direction verdict

### INGEST (ARXML / DBC / FLYNC → AADL)
- **Technically clean and cheap.** `autosar-data` + `autosar-data-specification`
  (dual MIT/Apache-2.0, full 4.0.1→R25-11 schema *derived from the official
  XSDs*, active, Rust-native) → spar reads ARXML **without bundling the AUTOSAR
  XSD**, sidestepping the one concrete licensing restriction. ARXML uniquely
  carries multi-bus topology *and* TIMEX timing constraints. DBC via `can-dbc`
  (MIT/Apache) is the cheap CAN-only secondary. **FLYNC is not a Rust on-ramp**
  (Python-only, Pydantic schema, doesn't even ingest ARXML itself). [SOLID]
- **But not differentiating on its own** — RTaW already does ARXML→TSN. The
  *under-explored* sliver is the **AADL↔AUTOSAR component/ECU bridge** (deflects
  to EAST-ADL↔AUTOSAR precedent; no direct AADL→ARXML transform found). [LIKELY]
- **Role:** enabler / pragmatic "read real models," not a market wedge by itself.

### EMIT (AADL → YANG/TSN config)
- Premise partly false (FLYNC≠YANG). TSN synthesis is occupied (RTaW, tsnkit,
  TTTech). Heaviest lift, weakest standalone case. **Deprioritize as an opener.**

### VALIDATE (proof-carrying TSN config-as-a-service)
- Technical gap is **real and still open in 2026** (no prover has PMOO/LUDB/tight
  bounds; verified-NC stops at min-plus convolution; Prosa verifies scheduling but
  not TSN). [LIKELY-strong]
- **But** (a) spar doesn't have the proofs yet (4 sorries), and (b) the *paying*
  demand for Tier-3 certificates is ~0 — buyers fund Tier-2 soundness-as-a-
  qualified-tool. So "validate" as a *sold product* is weak; as an *internal
  correctness/qualification asset* it's valuable.

## Recommended posture — two separable decisions, not one ranked pick

The honest evidence splits cleanly into one low-regret capability we should do
regardless, and one unvalidated product bet that needs a named buyer before it
gets roadmap weight. Conflating them (as the round-1 "lead with the bridge" framing
did) over-commits: it competes in RTaW's entrenched Tier-2 lane on a differentiator
(AADL front-end) whose buyer value the research never established.

**Decision 1 — low-regret, do regardless of the product bet: ARXML/DBC ingest.**
`autosar-data` + `can-dbc` (both Rust, dual MIT/Apache, license-clean, no AUTOSAR
XSD to bundle). Lets spar analyze *real* shipped models instead of hand-written
`.aadl`. This stands on its own as a capability — every analysis pass we already
have gets more valuable the moment it can read what automotive/aero teams actually
author. It does **not** depend on any TSN-product decision. Safe to start.

**Decision 2 — unvalidated, needs a named buyer before betting the roadmap: the
TSN-config product ambition** (any of ingest-as-wedge / emit-YANG / proof-carrying
validate). The technical gap is real (no AADL→TSN MBSE bridge; no proof-carrying
GCL in any prover) **but the evidence says nobody is paying to fill it**: RTaW owns
the cert-accepted Tier-2 ARXML→TSN+NC lane, and Tier-3 proof demand is ~0. The
discriminating question the research *cannot* answer — and the user must — is
**which segment, and what does an AADL front-end give that buyer that RTaW's ARXML
front-end doesn't?** Plausible answer: aerospace/defense already models in
AADL/OSATE and RTaW doesn't reach there — but that segment leans AFDX/TTEthernet
over 802.1 TSN, and automotive (RTaW's turf) is a testing culture with ~0 proof
demand. This is a design-partner / willingness-to-pay judgment, not a
web-resolvable one. **Do not commit roadmap to it without a concrete buyer.**

In both cases: keep the Lean proofs as an **internal soundness ratchet + credibility
asset**, and do **not** market "verified network calculus" until the 4 sorries are
discharged.

### Kill-criteria
- **Kill ARXML-beyond-DBC** if the AADL↔ARXML mapping proves to need full SWC/
  TIMEX fidelity we can't justify, *and* DBC carries enough for the target
  analysis.
- **Kill the AADL→TSN bridge** if we can't articulate why our integrated
  architecture-to-timing story beats RTaW's already-cert-accepted ARXML→TSN+NC.
- **Kill the "proof-carrying" marketing angle** (keep it internal only) unless a
  concrete cert-driven buyer asks for a re-checkable certificate — none found yet.
- **Defer the Lean NC proofs** if the 4 sorries + PMOO/LUDB mechanization estimate
  exceeds the credibility payoff; they don't gate near-term analysis features.

## What remains UNVERIFIED (don't bet on these)
- Whether reading user-supplied ARXML commercially counts as "commercial
  exploitation" under AUTOSAR terms — legally unresolved; worth counsel before any
  commercial ship, not a blocker for the technical decision. [UNVERIFIED]
- Whether RTaW's "mathematically verified" TSN.configurator ships a Coq-checked
  core in-product. [UNVERIFIED]
- Whether any non-English / paywalled 2024-26 work has already done proof-carrying
  TSN synthesis. [LIKELY-not, but negative-by-search]
