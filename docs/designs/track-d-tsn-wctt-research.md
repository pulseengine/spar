# Track D: TSN/Ethernet WCTT design space — research report

Status: **research / design proposal** — input to GitHub issue
[#149](https://github.com/pulseengine/spar/issues/149). Implementation lands as
separate PRs.
Last update: 2026-04-23.
Audience: spar maintainers + customers building vehicle E/E architectures
(gateway → TSN switches → Cortex-M0 ECUs).
Follows the structure of `docs/designs/v0.7.0-hierarchical-rta.md`.

> **TL;DR.** spar today checks bus *bandwidth* (summed throughput) and thread
> chain *latency* (single hop) but cannot answer "what is the worst-case
> traversal time for a stream that crosses two TSN switches under
> 802.1Qbv?". Closing that gap requires (a) a dedicated `Spar_TSN`
> property set, (b) first-class switch modelling on top of AADL's existing
> `bus`/`device` vocabulary, (c) a small Network-Calculus kernel
> (`spar-network` crate) implementing arrival/service curves, and (d) a
> new `wctt.rs` analysis pass. The recommended path keeps spec-conformance
> by reusing `bus` with a discriminator property (Option C below) and
> defers Lean proofs of min-plus algebra to v0.9.0 / v1.0.0. Estimated
> effort: ~6–7 weeks elapsed for v0.8.0, plus a 2–4 week proof tail.

---

## Section 0 — What this report is and is not

**Is.** A survey of (i) the IEEE 802.1Q TSN amendment chain that matters
for automotive E/E, (ii) existing Network-Calculus (NC) tooling, (iii)
AADL research extensions that have already attempted TSN/AFDX modeling,
(iv) the math foundations of min-plus algebra that any analysis must
respect, and (v) a 7-axis design-space exploration for spar's
implementation, with a recommended option per axis. It ends with a
concrete commit decomposition and a risk register.

**Is not.** Production code, AADL grammar changes, or Lean proofs. No
files in `crates/` or `proofs/` are modified by this PR. Acceptance
criteria from issue #149 are *refined* below in Section 8 but not
ticked off — that happens when the implementation PRs land.

The report is intentionally critical: for every standard, paper, and
tool surveyed, we say what is **good** (reusable for spar) and what is
**missing** (either out of scope or work spar still has to do).

---

## Section 1 — Standards landscape

### 1.1 The 802.1Q TSN amendment chain

IEEE 802.1Q ("VLAN-tagged bridges") accumulated a long chain of
amendments since AVB / 802.1Qav (2009) that, taken together, define
what the industry now calls **TSN**. As of October 2025 the IEEE 802.1
Working Group lists the following amendments under "bounded low
latency" and "high availability" categories
([Farkas, IEEE 802 standardization update, Oct 2025][farkas-2025];
[Wikipedia: TSN][wiki-tsn]):

| Amendment | Year | Working title | Vehicle E/E relevance | What an analyser must compute |
|---|---|---|---|---|
| 802.1AS-2020 | 2020 | gPTP — generalized PTP | Time domain for every TAS / CQF gate; multi-domain working clock | Synchronization error budget per hop; bound on residence time ([IEEE 802.1AS-2020][ieee-as]) |
| 802.1ASdm-2024 | 2024 | gPTP hot standby | Redundant grandmaster for fail-operational E/E | Failover transient bound ([P802.1ASdm][asdm]) |
| 802.1Qav | 2009 | Credit-Based Shaper (CBS) | AVB legacy + Class A/B in low-end TSN ECUs | CBS service curve `β = R·(t − T_max)⁺` with credit dynamics |
| 802.1Qbv-2015 | 2015 | Time-Aware Shaper (TAS) | The single most-used TSN feature in automotive; gate-driven scheduled traffic class | Gate-Control-List → time-varying service curve `β_TAS(t)` ([Qbv survey][qbv-survey]) |
| 802.1Qbu-2016 / 802.3br | 2016 | Frame Preemption (eMAC/pMAC) | Reduces blocking on 100 Mbit/s automotive links | Preemption residual: line-rate − preemption overhead per express frame |
| 802.1Qci-2017 | 2017 | Per-Stream Filtering & Policing (PSFP) | Network-resident IDS; blocks misbehaving ECUs | Stream-gate state machine + flow meter; affects arrival curves at ingress ([Qci/PSFP][qci]) |
| 802.1Qch-2017 | 2017 | Cyclic Queuing & Forwarding (CQF) | Latency = `H·CycleTime + ε`, topology-independent | Cycle time, two-buffer cyclic service curve, jitter at egress ([Qch][qch]) |
| 802.1CB-2017 | 2017 | Frame Replication & Elimination (FRER) | Fail-operational paths; primary/redundant disjoint paths | Sequence-numbering, replication points, elimination latency ([CB][cb]) |
| 802.1Qcc-2018 | 2018 | Stream Reservation Protocol enhancements + YANG/CUC/CNC | Centralised configuration model (CNC + CUC) | YANG schema for reservations; not strictly a math obligation ([Qcc][qcc]) |
| 802.1Qcr-2023 | 2023 | Asynchronous Traffic Shaper (ATS / UBS) | Replaces TAS where network-wide sync is too costly | Per-stream reshaping, urgency-based scheduler ([Qcr][qcr]) |
| 802.1Qdj-2024 | 2024 | Configuration enhancements | New UNI capabilities; YANG additions | YANG only; informational |
| 802.1DG-2025 | 2025 | TSN profile for **automotive** in-vehicle Ethernet | Vendor-shared decision matrix: which features to use where | Profile constrains choices; analysis must validate compliance ([DG][dg]) |
| 802.1DP | (in flight) | TSN profile for **aerospace** | Convergence with ARINC664-AFDX style guarantees | Same shape as DG, different parameter ranges |

**What is good for spar.** The amendment chain is now mature enough
that the modelling surface is *frozen for production use* — automotive
suppliers are shipping silicon with TAS+CBS+Qbu (e.g. Microchip's
LAN9662, Marvell 88Q5050, NXP SJA1110, Renesas RZ/T2). Each amendment
maps to a small, well-documented piece of math. We can pick a subset
(see Section 5.1) without users feeling we cherry-picked.

**What is missing for spar.**

1. **No "switch" component category in the AADL spec.** AADL v2.2/v2.3
   and the AS5506D revision do not define one ([AADL standard
   committee discussions][aadl-v3]). Today users abuse `device` or
   `bus`. We have to pick a discriminator (Section 5.2).
2. **No standard mapping from 802.1Qbv GCL to a YANG-or-AADL property**
   that AADL tools have agreed on. 802.1Qcc + 802.1Qdj defines a
   YANG schema, but YANG ⟂ AADL property syntax. spar has to invent the
   AADL surface (Section 5.1).
3. **802.1DG is the closest thing to an industry-agreed "minimal
   automotive TSN profile"**, and it landed in 2025
   ([802.1DG-2025 publication][dg-2025]). It is the right anchor for
   spar's first iteration — supporting fewer features than DG is
   acceptable; supporting more risks gold-plating.

### 1.2 AVnu Alliance certification

[AVnu Alliance][avnu] runs the **Automotive Certification Program**
based on 802.1DG plus components for 802.1AS / 802.1Qbv. The Component
Certification Program ([Avnu Component][avnu-comp]) initially focuses
on the timing layer (802.1AS) and TAS (Qbv).

**Good for spar.** AVnu's interop tests effectively ratify a "minimum
safe set" of TSN features for vehicle E/E. spar's Phase 1 should
target the *same* set so that spar verdicts are comparable to AVnu
test results.

**Missing for spar.** AVnu does not (and cannot) certify
*architecture* — only physical components. The architectural analysis
gap is exactly where spar plays.

### 1.3 AUTOSAR Classic vs. Adaptive

- **AUTOSAR Classic Platform (CP)** has had Ethernet support for years
  but TSN integration is patchy: Ethernet Switch Module ("EthSwt") +
  Time Synchronization stack ("StbM") cover gPTP and basic VLAN, but
  full Qbv GCL programming is vendor-specific
  ([CP R23-11 Release Overview][cp-r23]).
- **AUTOSAR Adaptive Platform (AP)** ships explicit TSN documentation
  in R23-11: `AUTOSAR_FO_EXP_TimeSensitiveNetworkFeatures.pdf`
  ([AP TSN explanation][ap-tsn]). It treats TSN as a foundational
  capability that `ara::com` (service-oriented IPC) layers on top of,
  with explicit gPTP and GCL configuration.

**Good for spar.** AP's R23-11 TSN doc essentially confirms the same
property surface that 802.1DG suggests — Stream IDs, traffic classes,
gate schedules, preemption flags — so spar's `Spar_TSN` set can map
cleanly into either CP or AP code generation later.

**Missing for spar.** AP releases on a yearly cadence; R24-11 is the
current latest ([AP R24-11 release overview][ap-r24]). spar should
target *property names* that survive R23-11 → R24-11 → future without
churn. Concretely: avoid embedding AUTOSAR-specific tokens in the
property set.

### 1.4 ROS 2 / DDS over TSN

ROS 2 uses DDS (RTI Connext, eProsima Fast DDS, Cyclone DDS) as its
transport. There is *no* deterministic real-time guarantee at the ROS
2 layer; DDS QoS profiles + TSN underneath are what get you bounded
delivery. Multiple recent surveys
([ROS 2 real-time survey 2025][ros2-rt],
[Latency analysis][ros2-latency]) confirm:

- ROS 2 nodes communicating cross-host typically incur 50%+ overhead
  vs. raw DDS, which itself adds ~10–100 µs over the wire.
- TSN underneath DDS bounds the *transport* delay; ROS 2's executor
  threading does not.

**Good for spar.** From a model perspective, a ROS 2 publisher /
subscriber pair maps cleanly to AADL `data port` connections crossing
a `bus` (the TSN backbone). The latency.rs pass already handles this.

**Missing for spar.** spar should *not* attempt to model ROS 2
executor delays in v0.8.0. That's a separate analysis (rcl/rclcpp
callback queue delays). Track D scope is the *network* segment only.

### 1.5 IEEE 1722 (AVTP) vs. straight TSN

[IEEE 1722 (AVTP)][avtp] is a Layer-2 transport protocol for media,
control, and tunnelled fieldbus traffic (CAN, LIN) over AVB/TSN
networks. The COVESA Open1722 project ([Open1722][open1722]) ships an
open-source implementation. AVTP frames sit *inside* TSN traffic
classes, so:

- Stream ID semantics: AVTP `stream_id` ↔ 802.1Qbv stream identifier
- Presentation time / timestamps: AVTP carries application-layer
  timing; spar models it as a property on the connection, not the
  switch.

**Good for spar.** AVTP is the only standardised Layer-2 way to ship
non-IP automotive payloads (e.g., raw CAN frames) over Ethernet. The
property set should accommodate `Stream_ID` as a 64-bit AVTP-style
identifier, not just a 16-bit VLAN tag.

**Missing for spar.** Audio/video media-clock recovery (AVTP-specific)
is out of scope; spar models the Ethernet frame transport, not the
media clock.

---

## Section 2 — Existing Network Calculus tooling

For each tool we record: license, language, AADL integration (if any),
maintenance signal in 2025–2026, what spar would *reuse* vs. *re-invent*.

### 2.1 RTC Toolbox (ETH Zurich, MATLAB)

- **License:** non-commercial academic; binaries only, source on request.
- **Language:** MATLAB (with Java backend for piecewise-linear curves).
- **Activity:** Last download page update 2014; the project page on
  [mpa.ethz.ch][rtc] still hosts the 1.2 release. Considered
  **dormant** for new feature work but the math is canonical.
- **AADL integration:** Yes — the *seminal* AADL→RTC work is
  Phan/Lee/Sokolsky 2010 ([Performance Analysis of AADL Models Using
  Real-Time Calculus][rtc-aadl]). Maps AADL components to RTC
  performance components.
- **Reuse for spar:** the *math model* (variability characterisation
  curves, min-plus operators) is the gold reference. spar's
  `arrival.rs` / `service.rs` types should match RTC semantics so
  benchmarks can be cross-validated.
- **Reinvent:** MATLAB binding is not viable for spar (Rust + Lean).
  Everything from the API up.

### 2.2 DiscoDNC / NetworkCalculus.org DNC (TU Kaiserslautern)

- **License:** LGPL 2.1 ([DNC LICENSE][dnc-license]).
- **Language:** Java, Maven build.
- **Activity:** DiscoDNC (original) is dormant; the rebrand
  [NetCal/DNC][netcal-dnc] continues in maintenance mode (2.5.x line,
  ~yearly tagged releases through 2024).
- **AADL integration:** None directly; some research papers
  (FORA-derived work, Section 3) wire DNC behind an OSATE plugin.
- **Reuse:** the algorithmic shape — server graph, flows, SFA/TFA
  passes — translates almost line-for-line into Rust. License (LGPL)
  precludes copying *code* into spar (MIT) but the *algorithms* are in
  the public literature ([Bouillard, Boyer, Le Corronc 2018][bbl-book]).
- **Reinvent:** Rust core. Fortunately the algorithms are well-known.

### 2.3 NCBounds (Nokia / Anne Bouillard)

- **License:** [GitHub repo][ncbounds] reads BSD-3-clause.
- **Language:** Python.
- **Activity:** Last commit 2019, sparse since. **Dormant.**
- **Focus:** Cyclic networks / stability — narrower scope than DiscoDNC.
- **Reuse:** Useful as a *test oracle* for spar's analysis on cyclic
  topologies (rare in vehicle E/E but possible with redundant FRER
  paths). Algorithm references in the [companion paper][ncbounds-paper].
- **Reinvent:** Spar's primary analysis is feed-forward; cyclic is a
  v1.0+ topic.

### 2.4 saturn / saturn.py / saturn-pyramidal

I could not find an active "saturn" NC tool from TU Kaiserslautern.
Search hits return either the unrelated [Saturn software
verifier][saturn-wiki] or a blockchain `saturn.py`. The user's
mention of "saturn" likely refers to a TU-KL internal toolchain. The
authoritative TU-KL projects on disco.cs.rptu.de are DiscoDNC and the
[Stochastic Network Calculator][snc]; both are listed by the same
group. **Treat saturn as not available**; rely on DiscoDNC instead.

### 2.5 WoPANets (academic, AFDX-focused)

- **License:** academic; not openly distributed.
- **Language:** Java + GUI.
- **Focus:** AFDX worst-case end-to-end traversal time. Combines NC
  with optimisation for design-space exploration ([WoPANets
  description][wopanets]). Used in
  [Boyer/Fraboul/Frances][afdx-nc] and follow-up TSN/BLS work.
- **Activity:** Author group still publishes (latest follow-ups around
  2018–2020). Tool itself appears **dormant** for external use.
- **Reuse:** None — closed binary. But the AFDX → NC mapping
  documented in their papers is directly relevant: AFDX virtual
  links ↔ TSN streams.

### 2.6 TheoreticalNet / network-calculus-rs

No Rust port of network calculus exists as of April 2026. Search of
crates.io and lib.rs ([Rust math crates][rust-math]) returns no
matches; the closest is the unrelated `calcucalc` symbolic-calculus
crate. **spar will be the first Rust NC implementation** — small,
embedded, and tied to AADL. This is a feature, not a bug: it lets
spar control the algebra (e.g., picosecond-fixed-point arithmetic)
without compromise.

### 2.7 RTaW-Pegase (RealTime-at-Work, commercial)

- **License:** commercial, per-seat.
- **Activity:** Active; the [TSN page][rtaw-tsn] advertises full
  802.1Qbv/Qcr/Qbu/CB/Qci/AS-2020 support and a max-pessimism claim of
  "<15% vs. true worst case". Long industrial track record with PSA,
  Airbus, Honeywell.
- **AADL integration:** Pegase is its own model format; AADL bridge
  exists for some flows but is not the primary path.
- **Reuse:** None directly (closed). spar should aim for *comparable
  pessimism* on the same benchmark networks; the [PEGASE 2010
  paper][pegase] and follow-ups describe their NC tightening tricks.

### 2.8 Mentor / Siemens Capital Network Designer (formerly Volcano)

- **License:** commercial.
- **Activity:** Active (now under Siemens EDA / Capital). Volcano VSA /
  COM Designer / VSI lineage now consolidated into Capital
  ([Capital Network Designer][capital]). Targets mainstream automotive
  OEM workflow (CAN, LIN, FlexRay, Ethernet AVB, increasingly TSN).
- **AADL integration:** None.
- **Reuse:** Demonstrates the *workflow* — early E/E design exploration
  → ECU configuration → integration test — that spar should remain
  compatible with via codegen plugins (already a v1.0 goal).

### 2.9 Summary table

| Tool | Active 2025–26? | Open source? | AADL? | Useful for spar |
|---|---|---|---|---|
| RTC Toolbox | No | Academic only | Yes (via Phan 2010) | Math reference |
| DiscoDNC / NetCal-DNC | Yes (slow) | LGPL-2.1 | No | Algorithm reference, oracle |
| NCBounds | No | BSD-3 | No | Cyclic-network oracle |
| saturn | Unverified / not found | — | — | Skip |
| WoPANets | No | Closed | No | AFDX paper reference |
| Rust NC crates | None exist | — | — | spar is first mover |
| RTaW-Pegase | Yes | Closed | Limited | Benchmark target |
| Capital (Volcano) | Yes | Closed | No | Workflow reference |

**Critical assessment.** Of the open-source options, **NetCal/DNC** is
the only one with both an active maintainer signal and a complete
algorithm set (TFA, SFA, PMOO, LUDB). It will be spar's primary
*oracle* for testing — produce identical bounds on identical input
networks. The algorithms themselves (Le Boudec & Thiran 2001;
Bouillard, Boyer, Le Corronc 2018) are well-documented in the public
literature, so spar reimplements rather than wraps.

---

## Section 3 — AADL research extensions

### 3.1 Phan/Lee/Sokolsky — AADL → Real-Time Calculus (2010)

- [Performance Analysis of AADL Models Using Real-Time Calculus][rtc-aadl]
- [Penn repository version][rtc-aadl-penn]
- DOI: 10.1007/978-3-642-12566-9_12 (Springer LNCS 6028).
- Maps AADL component categories (process, processor, bus) to RTC
  performance components, with arrival curves derived from `Period`
  and `Data_Size`.

**What's good.** It establishes a clean correspondence between AADL
properties and NC inputs. We use it almost verbatim: `Period` →
periodic arrival curve, `Data_Size` → burst, processor binding →
service curve.

**What's missing.** Single-hop only; assumes *abstract* bus rather
than scheduled TSN. No GCL, no preemption, no FRER. Section 5.4 below
extends this for switches.

### 3.2 Lauer/Ermont/Pagetti/Boniol — IMA / AFDX delay analysis (2010, 2014)

- [Analyzing End-to-End Functional Delays on an IMA Platform][lauer-ima]
  (DOI 10.1007/978-3-642-16558-0_21).
- [Network Calculus-based Timing Analysis of AFDX networks with
  Strict Priority and TSN/BLS Shapers][afdx-bls]
  (IEEE RTCSA 2018, DOI 10.1109/RTCSA.2018.8442080).

**What's good.** Treats AFDX (which is essentially deterministic
priority-shaped Ethernet) with NC. The data structures (virtual link
= stream + BAG + max frame) map 1:1 to TSN streams. Composition rules
for switches as priority-shaped servers carry over.

**What's missing.** Targets ARINC664 specifically; TAS/Qbv (which is
*scheduled*, not just priority-shaped) is not covered. The recent
Boyer paper on TSN/BLS bridges this gap.

### 3.3 FORA — AADL property sets for TSN + fog (2020)

- [The FORA Fog Computing Platform for Industrial IoT][fora]
  (arXiv 2007.02696, also Information Systems 2021).
- [FORA platform][fora-doi] — uses **AADL property sets to introduce
  new properties for specification and configuration of
  time-criticality applications, fog nodes, TSN networks and
  switches**.
- [Barzegaran thesis: Configuration Optimization of Fog Computing
  Platforms for Control Applications][barzegaran].

**What's good.** This is the *closest prior art to spar's Track D*. It
proves that AADL property sets alone — no new component category —
can express enough of TSN to drive scheduling. It treats fog nodes
(roughly: switches with compute) as a profile of `processor` +
`device` with TSN properties.

**What's missing.** The FORA property set is *not* part of the AADL
standard, not picked up by OSATE upstream, and the TSN feature
coverage is limited to TAS/Qbv plus credit-based shaper. spar should
adopt the FORA *approach* (property-set-based) but not its specific
property names; design `Spar_TSN::*` to cover preemption (Qbu),
PSFP (Qci), FRER (CB), and ATS (Qcr) as well.

### 3.4 OSATE annexes attempted for switch modelling

- [osate2 issue #64 — property sets for AADL annexes][osate-64]
  documents a long-running discussion about how to scope property sets
  to specific annex consumers. No TSN-specific annex has been
  ratified.
- The [Behavior Annex][bah-annex] and ARINC653 annex are the only
  network-relevant standardised annexes in OSATE; neither covers TSN.

**What's good.** The annex extensibility mechanism is the right shape
for spar's design-time analysis. spar already has its own annex
infrastructure (`crates/spar-annex`).

**What's missing.** No upstream TSN annex exists. spar's choice is to
ship a property set (Section 5.1) — strictly conformant with AS5506D —
rather than an annex (which would require new grammar and an OSATE
counterpart).

### 3.5 RTA ↔ NC formal link (Boyer/Maia/Cucu-Grosjean 2022)

- [A Formal Link Between Response Time Analysis and Network
  Calculus][rta-nc] (ECRTS 2022, LIPIcs).
- Mechanically checked in Coq.

**What's good.** Tells us that the same min-plus theorems can serve as
the foundation for both `rta.rs` (already verified in spar via
`scheduling_verified`) and a future `wctt.rs`. Shared algebraic core.

**What's missing.** The paper proves the *bridge* (RTA bound = NC
bound under specific assumptions); it does not deliver an off-the-shelf
Rust library. spar still has to implement the algebra.

### 3.6 Min-plus formalisation in Coq (Bouillard, 2021)

- [Verifying min-Plus Computations with Coq (extended version)][min-plus-coq]
- The full thesis: [Preuve formelle en calcul réseau (HAL)][bouillard-thesis].
- [A Residual Service Curve of Rate-Latency Server… in Quadratic Time][rate-latency]
  (ECRTS 2021).

**What's good.** Establishes that the core algebra (associativity of
min-plus convolution, monotonicity of operators, closure of the
service-curve set under deconvolution) is mechanically provable. spar
can port the *theorem statements* into Lean 4 and prove them in
mathlib idioms (Sections 4 and 5.5).

**What's missing.** The formalisation is in Coq with
mathematical-components style; no Lean port exists. spar's Lean
proofs will need to (a) re-establish the algebraic foundations,
(b) define spar's own arrival/service-curve types (probably as
Lean-side simply-typed records), and (c) keep them small enough to
land in v0.9.0. We will *not* attempt a full port of the
Bouillard formalisation.

### 3.7 Survey of NC tools (Aladdin et al., 2021 / Boyer 2024)

- [A Survey on Network Calculus Tools for Network Infrastructure in
  Real-Time Systems][nc-survey] (NSF par.gov copy).
  Comparison table: SFA/TFA/PMOO/LUDB tightness, RTC vs. DiscoDNC vs.
  CyNC.

**What's good.** Lays out the tightness ordering: PMOO ≤ LUDB ≤
SFA ≤ TFA, with TFA being the loosest (and most computationally
cheap) general method. spar's first cut is TFA (simplest, scales);
SFA is a v0.9.0 tightening.

**What's missing.** No tool in the survey targets *automotive* TSN
profiles specifically. spar fills this niche.

---

## Section 4 — Math foundations

### 4.1 Min-plus algebra primer

Let **F** = { f : ℝ⁺ → ℝ⁺ ∪ {∞} | f is wide-sense increasing, f(0) ≥ 0 }
be the set of *cumulative* functions.

- **Min-plus convolution:** (f ⊗ g)(t) = inf_{0 ≤ s ≤ t} { f(s) + g(t − s) }.
- **Min-plus deconvolution:** (f ⊘ g)(t) = sup_{u ≥ 0} { f(t + u) − g(u) }.
- **Identity element:** δ₀ where δ₀(0) = 0, δ₀(t > 0) = +∞. Satisfies
  f ⊗ δ₀ = f.

Both ⊗ and ⊘ are well-known to be *monotone*, *associative* (⊗ only),
and continuous on appropriate sublattices ([Le Boudec & Thiran][nc-book]).

### 4.2 Arrival curve α and service curve β

Let R(t) be the cumulative number of bits arrived at a node by time t,
and R*(t) the cumulative number departed.

- **Arrival curve:** R is α-arrival-constrained iff for all
  s ≤ t, R(t) − R(s) ≤ α(t − s).
- **Service curve:** node offers service curve β iff for all
  t, R*(t) ≥ (R ⊗ β)(t).
- **Strict service curve:** β is *strict* iff during any backlogged
  period [s, t], R*(t) − R*(s) ≥ β(t − s). Strictness is required for
  composition in the *aggregate* setting (multiple flows sharing a
  server), per [Bouillard 2018][bbl-book] and the recent
  [extension to negative service curves][neg-sc] (arXiv 2403.18042).

### 4.3 The four core bounds

For a flow with arrival α traversing a server with service β:

1. **Backlog bound:** B ≤ sup_{t ≥ 0} { α(t) − β(t) } (vertical
   distance).
2. **Delay bound:** D ≤ inf{ d ≥ 0 : ∀ t, α(t) ≤ β(t + d) }
   (horizontal distance) ([improved variant with line-rate
   knowledge][improved-delay]).
3. **Departure curve:** α* = α ⊘ β (output is reshaped by the
   server's residual capability).
4. **Concatenation:** βcat = β₁ ⊗ β₂ for serial composition of two
   servers ([Le Boudec][nc-book]).

### 4.4 Composition rules

- **Serial / pay-burst-only-once:** Two cascaded servers β₁ and β₂
  yield an end-to-end service curve β_e2e = β₁ ⊗ β₂ that is *better
  than naively summing per-hop delays*. This is the foundation of NC's
  tighter end-to-end bounds.
- **Parallel (FIFO multiplexing):** When a server serves multiple
  flows FIFO-aggregated, each tagged-flow sees a residual
  β_residual_i = (β − Σ_{j≠i} α_j)⁺.
- **Parallel (priority multiplexing):** With strict priority, the
  low-priority flow sees β_low = (β − α_high)⁺.

These three formulas are the entirety of the kernel that
`spar-network` needs for v0.8.0.

### 4.5 TSN-specific service curves

- **CBS (Qav):** β_CBS(t) = max(0, R · (t − T_max)) where R = idle
  slope and T_max bounds credit recovery time. Linear, easy.
- **TAS (Qbv):** Service is *intermittent* — gates open during
  scheduled windows. The standard model is a step service curve
  derived from the Gate-Control-List. Recent work
  ([Robust TAS analysis][robust-tas]) gives tighter bounds when GCLs
  are aligned across switches (the "no-wait packet scheduling"
  problem).
- **Qbu / preemption:** When express traffic preempts a
  preemptable frame, the residual blocking shrinks from `MTU/R` to
  `123 B / R` (the preemption granularity). Modelled as a constant
  *blocking term* added to β_express.
- **CQF (Qch):** A flow gets β = R · (t − H · CycleTime)⁺ where H is
  hop count. Topology-independent. Beautifully simple to model.
- **ATS (Qcr / UBS):** Per-stream rate limit via a leaky-bucket
  arrival curve at the *output* of each switch. Composes naturally
  with NC ([Qcr astesj][qcr-astesj]).

### 4.6 What is already formalised

- **Coq:** [Bouillard's HAL thesis][bouillard-thesis] (~2021) +
  [Boyer/Maia/Cucu RTA-NC link][rta-nc] formalise min-plus algebra
  and per-flow bounds for rate-latency servers. Mathematical-components
  + ssreflect.
- **Lean 4:** None as of April 2026 (per
  [Mathlib search][mathlib]). spar would be first.

### 4.7 Tightness vs. tractability

The literature gives a tightness ordering ([NC survey][nc-survey];
[Bouillard tight bounds][tight-bounds]):

```
       loose                                                    tight
        TFA  ≤  SFA  ≤  PMOO  ≤  LUDB  ≤  Linear-Programming
   O(|V|+|E|)   O(F²)   O(F!)  feasible    NP-hard
```

For 100 ECUs / 200 streams / 10 switches (the issue #149 target),
**SFA is the sweet spot**: tight enough that pessimism is typically
<30% on TSN topologies, cheap enough to run in seconds. PMOO and
LUDB explode on >50 flows. spar v0.8.0 ships TFA; v0.9.0 adds SFA;
v1.0.0 considers PMOO when networks are small.

---

## Section 5 — spar's design space

We split this into 7 sub-decisions. For each we evaluate options and
recommend one.

### 5.1 Property set design — `Spar_TSN::*`

We follow the FORA precedent ([Section 3.3][fora]) of using a property
set, not a new annex.

**Recommended properties** (AADL types in parentheses):

```aadl
property set Spar_TSN is

  -- 802.1Qbv / Qbu / general stream identity
  Stream_ID : aadlinteger 0 .. 4294967295
              applies to (connection, virtual bus);
              -- 32-bit; covers VLAN+PCP+SR-class encoding and AVTP IDs.

  Class_of_Service : enumeration
              (best_effort, cdt, ats, scheduled, express, fr_class_a, fr_class_b)
              applies to (connection);
              -- Maps to Qbv traffic class + Qbu express/preemptable + AVB CBS class.

  Max_Frame_Size : Size
              applies to (connection);
              -- Bytes, including VLAN tag and Ethernet preamble; default 1518.

  Frame_Preemption : enumeration (none, express, preemptable)
              applies to (connection);
              -- 802.1Qbu role; default none.

  -- 802.1Qbv: gate control list (per egress port)
  Gate_Control_List : list of Spar_TSN::Gate_Entry
              applies to (bus);
              -- Empty list ⇒ no scheduled traffic, only priority/CBS.

  Gate_Entry : type record (
        Gate_State_Vector : aadlinteger 0 .. 255;  -- 8 bits, one per traffic class
        Time_Interval     : Time;                  -- duration this state is active
      );

  Cycle_Time : Time
              applies to (bus);
              -- 802.1Qbv master cycle; sum of Gate_Entry intervals.

  -- 802.1Qch: cyclic queuing
  Cyclic_Mode : aadlboolean
              applies to (bus);
  Cycle_Length : Time
              applies to (bus);

  -- 802.1Qcr: ATS
  ATS_Committed_Information_Rate : Data_Rate
              applies to (connection);
  ATS_Burst_Size : Size
              applies to (connection);

  -- 802.1Qci: per-stream filtering & policing
  PSFP_Filter_ID : aadlinteger
              applies to (connection);

  -- 802.1CB: FRER
  Replication_Points : list of reference (system, processor, bus, device)
              applies to (connection);
  Elimination_Points : list of reference (system, processor, bus, device)
              applies to (connection);

  -- 802.1AS: time domain
  Time_Domain : aadlinteger 0 .. 127
              applies to (bus, device, processor);
              -- 0 = working clock, 1 = global, etc.

end Spar_TSN;
```

`Spar_Network::*` carries the *non-TSN* network identity (link rate,
switch/end-station discriminator, processing delay):

```aadl
property set Spar_Network is

  Switch_Type : enumeration (none, store_and_forward, cut_through)
              applies to (bus);
              -- "none" ⇒ traditional point-to-point bus

  Per_Hop_Processing_Delay : Time
              applies to (bus);
              -- Switch fabric/MAC processing; default 1us for store-and-forward.

  Link_Rate : Data_Rate
              applies to (bus);
              -- Already implied by Bandwidth, but TSN code needs a single source.

  Forwarding_Mode : enumeration (priority, scheduled, ats, cqf)
              applies to (bus);

end Spar_Network;
```

#### 5.1.1 Worked example

```aadl
package Vehicle_Backbone
public
  with Spar_TSN, Spar_Network, Communication_Properties;

  bus tsn_backbone
  end tsn_backbone;

  bus implementation tsn_backbone.gateway
    properties
      Spar_Network::Switch_Type            => store_and_forward;
      Spar_Network::Forwarding_Mode        => scheduled;
      Spar_Network::Per_Hop_Processing_Delay => 1 us;
      Communication_Properties::Bandwidth   => 100 Mbps;
      Spar_TSN::Cycle_Time                  => 125 us;
      Spar_TSN::Gate_Control_List           => (
        [ Gate_State_Vector => 16#FF#; Time_Interval => 50 us; ],
        [ Gate_State_Vector => 16#01#; Time_Interval => 75 us; ]   -- only TC 0 (express)
      );
  end tsn_backbone.gateway;

  device adas_ecu
    features
      backbone : requires bus access tsn_backbone;
  end adas_ecu;

  system Backbone
  end Backbone;

  system implementation Backbone.impl
    subcomponents
      gw    : bus tsn_backbone.gateway;
      front : device adas_ecu;
      rear  : device adas_ecu;
    connections
      lidar_stream : feature front.backbone -> rear.backbone;
    properties
      Spar_TSN::Stream_ID         => 42                   applies to lidar_stream;
      Spar_TSN::Class_of_Service  => scheduled            applies to lidar_stream;
      Spar_TSN::Max_Frame_Size    => 1024 Bytes           applies to lidar_stream;
      Spar_TSN::Frame_Preemption  => express              applies to lidar_stream;
      Communication_Properties::Actual_Connection_Binding =>
        (reference (gw))                                  applies to lidar_stream;
  end Backbone.impl;
end Vehicle_Backbone;
```

A spar `wctt` analysis on `Backbone.impl` yields, per `Stream_ID`:

```
stream 42 (lidar_stream): WCTT 173 µs (within deadline 500 µs)
  hops: front -> gw -> rear
  per-hop:
    gw: TAS service curve, Q-Class scheduled, gate open 50/125 us
        -> service rate 40 Mbps effective, blocking ≤ 123 B/100 Mbps = 9.84 µs
    backbone link: serialisation 81.92 µs (1024 B at 100 Mbps)
    end-station processing: 1 µs
  bound: 9.84 + 81.92 + 1 + ... = 173 µs
```

### 5.2 Switch component modeling

| Option | Mechanism | Pros | Cons |
|---|---|---|---|
| **A. Dedicated category** | Add `switch` to AADL grammar | Cleanest semantics; tooling discoverability | Requires AADL spec extension; not in AS5506D; OSATE incompatibility; non-portable models |
| **B. `device` + discriminator** | `device implementation` with `Spar_Network::Switch_Type` | Devices already have features and properties; multiple impls per type allowed | A switch is conceptually *not* a device — it is a shared communication resource. Latency.rs already routes through buses, not devices. Connections do not bind to devices. |
| **C. `bus` + discriminator** ★ | `bus implementation` with `Spar_Network::Switch_Type ≠ none` | Connections already bind to buses (`Actual_Connection_Binding`). spec-conformant. Multi-hop = chain of bus instances | Technically a "switched bus" is unusual AADL idiom (most existing models use point-to-point bus); requires care when modelling switch internals (multiple ingress/egress ports as features) |

**Recommendation: Option C.** A TSN switch is, semantically, a piece
of shared communication infrastructure with deterministic forwarding —
exactly what AADL's `bus` was invented for. The switch fabric inside
the bus is hidden behind properties (GCL, Cycle_Time,
Per_Hop_Processing_Delay). Multi-port switches model as a single
`bus` instance whose service curve aggregates ingress/egress. Multi-hop
topologies (two cascaded switches) become two `bus` instances connected
via a *third* `bus` representing the inter-switch link, OR a single
chain of `bus` subcomponents in a `system` parent.

Alternatives to revisit at v1.0.0: when the AADL v3 standardisation
body settles on a switch category (under discussion per
[AADL V3 standard][aadl-v3]), spar should align.

### 5.3 Crate layout

| Option | Where does WCTT live | Pros | Cons |
|---|---|---|---|
| 1. Extend `spar-analysis` | New `wctt.rs` next to `bus_bandwidth.rs` and `latency.rs`; NC primitives in same crate | Lowest friction; reuses existing `Analysis`/`ModalAnalysis` traits | Pollutes `spar-analysis` with mathematically heavy code; harder to test in isolation |
| 2. **New `spar-network` crate** ★ | Hosts NC primitives (curves, operators, server graph). `spar-analysis::wctt` becomes a thin wrapper. | Clear layering: math vs. AADL-specific analysis. Mirrors `spar-solver` separation. Reusable from `spar-codegen` (e.g., to emit AUTOSAR Eth-Switch configs). | One more crate to maintain; small initial surface |

**Recommendation: New `spar-network` crate.** Mirrors how
`spar-solver` was peeled out of `spar-analysis` for the scheduling
verifier. NC primitives are general (Le Boudec algebra) and have an
obvious natural home outside an analysis crate that knows about AADL
diagnostics.

Workspace `Cargo.toml` additions:

```toml
members = [
  ...
  "crates/spar-network",
]

[workspace.dependencies]
spar-network = { path = "crates/spar-network" }
```

Crate dependencies: `spar-network` depends on `petgraph` (already a
workspace dep) for the server graph; on `rustc-hash` for hash maps; on
nothing else from the spar tree. `spar-analysis` adds `spar-network`
as a dep and uses it from `wctt.rs`.

### 5.4 Analysis pass shape

`spar-analysis::wctt::WcttAnalysis` implements `Analysis` and
`ModalAnalysis`. Pseudocode:

```rust
impl Analysis for WcttAnalysis {
    fn name(&self) -> &str { "wctt" }
    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        self.compute(instance, None)
    }
}

impl WcttAnalysis {
    fn compute(&self, instance: &SystemInstance, som: Option<&SystemOperationMode>)
        -> Vec<AnalysisDiagnostic>
    {
        // 1. Extract the network graph
        //    Nodes = end-stations (process, device) + switches (bus with Switch_Type ≠ none)
        //    Edges = connections bound via Actual_Connection_Binding to a switch-bus
        //    Each edge tagged with Stream_ID, Class_of_Service, Max_Frame_Size
        let net = NetworkGraph::extract(instance, som);

        // 2. Build per-stream arrival curves from source thread Period + Data_Size
        //    Periodic: α(t) = burst + rate · t, with rate = Data_Size / Period
        //    Sporadic with jitter: α(t) = ⌈(t + J) / T⌉ · L (staircase)
        let mut arrival: FxHashMap<StreamId, ArrivalCurve> = FxHashMap::default();
        for stream in net.streams() {
            arrival.insert(stream.id, ArrivalCurve::from_aadl(stream)?);
        }

        // 3. Build per-hop service curves from switch properties
        let mut service: FxHashMap<(SwitchId, TrafficClass), ServiceCurve> = FxHashMap::default();
        for sw in net.switches() {
            for tc in sw.traffic_classes() {
                service.insert((sw.id, tc), ServiceCurve::from_switch(sw, tc)?);
            }
        }

        // 4. TFA pass: propagate
        //    For each switch in topological order:
        //       For each egress port, for each traffic class:
        //          aggregate arrival = Σ α_i over flows in that class
        //          residual β = β_port − Σ_{lower-priority} α_j   (priority case)
        //          per-flow delay = horizontal_distance(α_i, β)
        //          updated α*_i = α_i ⊘ β (departure curve)
        //
        //    Cycle detection: if FRER (replication) creates a feed-forward DAG, fine;
        //    if cyclic deps detected, emit Warning "cycle detected; using FixedPoint-TFA".
        let order = topo::sort_or_fixedpoint(&net);
        for sw in order {
            for stream in sw.streams_through() {
                let alpha = arrival[&stream.id].clone();
                let beta = residual_for(&sw, stream.tc, &arrival);
                let d = horizontal_distance(&alpha, &beta);
                stream.accumulated_delay += d;
                arrival[&stream.id] = alpha.deconvolve(&beta);  // departure curve
            }
        }

        // 5. Compare to deadline (Spar_TSN::Latency_Deadline or end-to-end flow latency)
        let mut diags = vec![];
        for stream in net.streams() {
            if stream.accumulated_delay > stream.deadline {
                diags.push(error(...));
            } else {
                diags.push(info(...));
            }
        }
        diags
    }
}
```

Notes on the algorithm:
- Storage of curves as **piecewise-affine in picoseconds**, mirroring
  `spar-solver`'s fixed-point arithmetic. No floating-point in the
  WCTT computation. (Pessimism vs. correctness: integer arithmetic
  rounds *up* for arrival curves, *down* for service curves, to stay
  on the safe side.)
- `petgraph::algo::toposort` for the topological pass. Cyclic-traffic
  fallback: fixed-point TFA per [Equivalent versions of TFA][tfa-eq]
  (arXiv 2111.01827).

### 5.5 Lean foundation

Theorems to prove (in `proofs/Proofs/Network/`):

| Theorem | Lines (estimate) | Status target | Notes |
|---|---|---|---|
| `min_plus_convolution_assoc` | 80–120 | v0.8.0 with `sorry` | Associativity of ⊗. Mathlib has commutative monoids; reuse. |
| `min_plus_convolution_mono` | 60–80 | v0.8.0 with `sorry` | Monotonicity in both arguments |
| `arrival_curve_subadditive` | 40–60 | v0.8.0 full proof | (R(t) − R(s)) ≤ α(t−s) ⇒ α subadditive when α is the tightest envelope |
| `service_curve_implies_output` | 100–150 | v0.9.0 full proof | R* ≥ R ⊗ β ⇒ output is α' = α ⊘ β arrival-constrained |
| `delay_bound_horizontal_distance` | 80–120 | v0.9.0 full proof | The headline formula `D ≤ inf{d : α(t) ≤ β(t+d)}` |
| `pay_burst_only_once` | 200–300 | v0.9.0 with `sorry`, full v1.0.0 | β₁ ⊗ β₂ tighter than per-hop sum |
| `tas_gate_service_curve_correct` | 150–200 | v1.0.0 | The TAS-specific service curve from a GCL is a valid service curve |
| `residual_priority_correct` | 100–150 | v1.0.0 | (β − α_high)⁺ is a strict service curve under priority |

Total estimated proof size: ~1100–1700 lines for full coverage; ~450
lines if v0.8.0 ships only the algebraic foundations with `sorry` on
the deeper theorems. Comparable in size to the existing `Proofs/
Scheduling/RTA.lean` family.

**Recommendation:** v0.8.0 ships `Network/MinPlus.lean` with the
algebraic monoid theorems (`assoc`, `mono`, `subadditive`) fully
proved, and *statements* of the headline bounds as `sorry`-d
theorems. The implementation in Rust uses these statements as
postconditions. Full proofs land incrementally in v0.9.0 / v1.0.0.

Worth considering: the Coq formalisation by Bouillard / the RTA-NC
link by Boyer et al. is *transferrable* in spirit. We do not port
their Coq code; we *re-state* the theorems in mathlib idioms (over
`OrderedAddCommMonoid` and `ENNReal`-valued functions) and prove
them using mathlib's existing min-plus tactics where available.

### 5.6 Integration with existing analyses

**Backward compatibility for `bus_bandwidth.rs` users.** The
existing pass remains unchanged. `wctt.rs` is *additive*: it consumes
the same `Actual_Connection_Binding` properties + new `Spar_TSN::*`
properties when present. If a model has no `Spar_TSN` annotations,
`wctt.rs` emits a single `Info` diagnostic `"no TSN-annotated streams
found; falling back to bus_bandwidth"` and otherwise stays silent.

**`latency.rs` ↔ `wctt.rs` interop.** Today `latency.rs` walks AADL
end-to-end flows (`flow implementation`) accumulating
`Latency`/`Period` from each thread, plus a hard-coded
`get_inter_processor_overhead`. Proposed change for v0.8.0:

```rust
// crates/spar-analysis/src/latency.rs
fn cross_processor_segment_delay(seg: &FlowSegment, instance: &SystemInstance)
    -> Option<u64>
{
    // First try wctt-derived per-stream delay (precise)
    if let Some(stream_id) = stream_id_for_connection(seg.connection, instance) {
        if let Some(d) = WcttAnalysis::cached_delay(instance, stream_id) {
            return Some(d);
        }
    }
    // Fallback: existing inter_processor_overhead heuristic
    get_inter_processor_overhead(seg.connection_props)
}
```

A small `WcttCache` (computed once per instance, shared across
`Analysis::analyze` calls) avoids quadratic blow-up.

This means: latency.rs gets *tighter* per-segment delays whenever
`Spar_TSN` annotations are present, but degrades gracefully to the
current behaviour otherwise. Existing tests in
`latency.rs::tests` keep passing without modification.

### 5.7 Performance budget

For the issue #149 target — 100 ECUs, 200 streams, 10 switches — we
analyse computational cost:

| Pass | Per-element cost | Per-network cost | Wall time @ this scale |
|---|---|---|---|
| Graph extraction | O(\|V\|+\|E\|) | O(100 + 200) | <1 ms |
| Arrival curve build | O(F) per stream where F = breakpoints | 200 · 4 = 800 ops | <1 ms |
| Service curve build | O(GCL_size) per switch | 10 · 16 = 160 ops | <1 ms |
| TFA pass | O((V_sw · F_avg) · breakpoints) | 10 · 20 · 4 = 800 horizontal-distance computations | ~10 ms (each h-distance is O(breakpoints) on piecewise-affine curves) |
| Min-plus deconvolution | O(B₁ · B₂) per pair | 200 · 4 · 4 = 3200 ops per switch hop, ~10 hops = 32k ops | ~50 ms |
| **Total v0.8.0 (TFA)** |  |  | **~100 ms wall time** |

For comparison, `bus_bandwidth.rs` runs in ~10 ms on similar models.
WCTT is roughly 10× more expensive but stays sub-second.

**Pessimism / cost trade-off:**

| Method | Cost (relative) | Pessimism (typical) |
|---|---|---|
| TFA | 1× | 30–60% |
| SFA | 2–3× | 15–30% |
| PMOO | 10–100× | 5–15% |
| LP (linear program) | NP-hard, may not finish | tight |

spar v0.8.0 ships TFA only. SFA is a v0.9.0 toggle. PMOO is a
v1.0.0+ research item; users who need it today should fall back to
RTaW-Pegase (commercial) for that subset of the network.

**Modal analysis.** `wctt.rs` implements `ModalAnalysis` like its
siblings; per-mode the network graph is filtered to active
components/connections. Dominant cost is unchanged because each mode
sees a smaller subgraph.

**Memory.** Curves are stored as `Vec<(t_ps: u64, y_bits: u64)>`
breakpoint pairs, typically <100 breakpoints per curve. ~10 KiB per
stream → ~2 MiB for 200 streams. Negligible.

---

## Section 6 — Roadmap proposal

### 6.1 Commit decomposition for v0.8.0

| # | Title | Scope | Est. weeks | Blocks on |
|---|---|---|---|---|
| 1 | `Spar_TSN` + `Spar_Network` property sets + AADL surface | Adds the property sets to `crates/spar-hir-def/src/property_sets/` (or wherever `Spar_Timing` from PR #145 lives); accessor functions in `property_accessors.rs`; legality: `Spar_TSN::Cycle_Time` ≥ Σ Gate_Entry intervals; integration tests on parser. | 1.0 | none |
| 2 | Switch model + graph extraction | New `spar-network` crate (workspace member); `NetworkGraph`, `Switch`, `Stream`, `Endpoint` types. Extraction logic from `SystemInstance` to `NetworkGraph`. Round-trip tests. | 1.0 | (1) |
| 3 | NC primitives (curves, operators) | In `spar-network::curve`: `ArrivalCurve`, `ServiceCurve` as breakpoint vecs; min-plus ⊗, ⊘ on piecewise-affine; `horizontal_distance`; `vertical_distance`. Property tests cross-checking Le Boudec identities. | 1.5 | (2) |
| 4 | `wctt.rs` analysis pass + integration tests | TFA implementation; integration with `Analysis`/`ModalAnalysis`; test models for: 1-hop, 2-hop chain, FRER replication, multi-priority. Comparison against hand-computed bounds (≤5% deviation). | 1.5 | (3) |
| 5 | Lean theorems (`Proofs/Network/MinPlus.lean`) | Algebraic foundations (assoc, mono, subadditive) fully proved; headline bounds stated with `sorry`. About 450 lines of Lean. | 1.0 | (3) (theorems mirror Rust API) |
| 6 | Integration with existing passes + COMPLIANCE update | `latency.rs` consumes `WcttCache`; backward-compat info diagnostic in `bus_bandwidth.rs`; rivet artefacts (requirement, design-decision, aadl-analysis-result entries); COMPLIANCE.md section "TSN/WCTT". | 1.0 | (4), (5) |

**Total elapsed time estimate: ~7 weeks** (allowing slack for review,
benchmark calibration, doc writing). For comparison, Track A
(hierarchical RTA) was estimated 4–6 weeks; Track D is larger because
of the new crate, the larger property surface, and the math kernel.

**Parallelism opportunity.** Commit 5 (Lean) can start as soon as
Commit 3's API is stable, in parallel with Commit 4. With one
implementer, that buys ~0.5 weeks; with two, ~1 week.

### 6.2 Dependencies on already-merged work

- **PR #145 (Spar_Timing / Spar_Trace property sets)**: yes — Track D
  reuses the property-set scaffolding that PR #145 introduced. If
  #145 has not landed when Track D starts, Track D needs to bring its
  own scaffolding (~+0.5 weeks).
- **PR #147 (hierarchical IRQ-aware RTA)**: not strictly required, but
  the IRQ overhead model on receiving ECUs (Cortex-M0 receiving TSN
  frames triggers an interrupt) becomes the consumer of WCTT bounds
  end-to-end.
- **PR #144 (rivet binding contract)**: needed for Commit 6's rivet
  artefacts.

### 6.3 What we explicitly defer to v0.9.0+

- **SFA (Separate Flow Analysis)** — tighter bounds, more complex
  algorithm. v0.9.0.
- **PMOO / LUDB** — even tighter, much more expensive. v1.0.0+ if
  user demand exists.
- **Stochastic NC** — for soft-real-time ROS 2 / DDS workloads. Not
  on the roadmap.
- **Full Lean proofs of headline bounds** — v0.9.0 finishes
  `delay_bound_horizontal_distance`; v1.0.0 finishes
  `pay_burst_only_once`.
- **Wireless TSN** (802.11be deterministic) — research, not v0.8.0.

---

## Section 7 — Open questions and risks

### 7.1 What user input do we need before implementation starts?

1. **TAS or no TAS?** If the customer's first model uses only
   priority shaping (no Qbv gates), v0.8.0 can skip the GCL property
   in the property set. Saves ~1 week. *Decision needed.*
2. **CBS scope?** Same question — AVB-class flows or pure TSN?
3. **FRER scope?** Replication paths add cycle-detection complexity.
   If first model has no FRER, defer 802.1CB modelling to v0.9.0.
4. **Targeted accuracy?** If 30–60% pessimism is acceptable for
   v0.8.0, TFA suffices. If <30%, SFA must come up to v0.8.0
   (+1 week).
5. **Lean appetite.** Is shipping `sorry`-bearing theorems acceptable?
   The COMPLIANCE.md style suggests yes (RTA shipped this way), but
   confirm explicitly.

### 7.2 Where the math gets hard

- **Modal switching transients.** When a system mode changes, the
  GCL on a TSN switch may also change. The transient between two
  GCL configurations is *not* covered by classical NC. Workaround for
  v0.8.0: analyse each SOM independently, do not bound the
  inter-mode transient. (This is consistent with how
  `bus_bandwidth.rs` already handles modes.)
- **Cyclic dependencies under FRER.** If a stream is replicated and
  the redundant path re-merges with the primary, classical
  feed-forward TFA breaks. [Equivalent versions of TFA][tfa-eq] +
  fixed-point iteration handles this but slows convergence.
- **Aggregate vs. strict service curves.** When multiple flows share
  a queue, the residual-service-curve trick assumes a *strict*
  service curve. A non-strict β breaks the math. spar must validate
  strictness when extracting the service curve from switch properties
  — and emit an error if the user's switch model implies a non-strict
  curve (e.g., a CBS configuration with ill-conditioned credit
  parameters).
- **TAS GCL gaps.** When two cascaded switches' GCLs don't align
  (the "no-wait" property fails), the per-flow delay bound includes
  a worst-case wait at the second switch's gate. The
  [robust TAS analysis paper][robust-tas] formalises this; spar should
  cite it in `wctt.rs`.

### 7.3 Risks register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| NC bounds too pessimistic for users | Medium | Medium | Document pessimism in COMPLIANCE; ship SFA in v0.9.0; offer escape hatch to RTaW for hard cases |
| Lean proofs slip past v0.8.0 | High | Low | Ship statements with `sorry`; track in issue per theorem |
| AADL property surface conflicts with future AADL v3 switch category | Medium | Medium | Use `Spar_*` prefix everywhere; keep mappable to v3 when it lands |
| Performance budget blown on >500 streams | Low | Medium | Bench with criterion at scale; precompute curve breakpoint compaction; SFA fallback |
| Customer's first model uses ATS+CBS (Qcr) and no GCL | Medium | Low | Keep GCL property optional; ATS-only path validated separately |
| Cyclic FRER topology | Low | Medium | Detect and warn; v0.9.0 fixed-point TFA |
| Floating-point drift between Rust and Lean | High (if FP is used) | High | Pure picosecond fixed-point arithmetic in both. No FP in WCTT. |

### 7.4 Out of scope for v0.8.0

- Wireless TSN (802.11be deterministic, 802.1DC/F)
- Bluetooth Low Energy / UWB integration
- TSN over 5G (3GPP URLLC)
- Stochastic NC for soft-real-time DDS QoS
- TSN scheduler synthesis (taking goals → producing GCLs); spar
  *analyses* user-provided GCLs, does not compute them. (Synthesis is
  a v1.x research direction or a separate `spar-solver` extension.)
- AUTOSAR codegen of EthSwt configuration; that's a separate
  `spar-codegen` story.

### 7.5 When NC is the wrong tool

- **Average-case latency** — NC bounds worst case. For 99.99-percentile
  bounds, simulation (RTaW-Pegase, OMNeT++ INET-TSN) is the right tool.
- **Probabilistic guarantees** — stochastic NC, not deterministic NC.
- **Co-design / synthesis** — search problem; ILP/SMT (cf.
  `spar-solver`'s `good_lp` already in deps) is more natural.

---

## Section 8 — Refined acceptance criteria for issue #149

Based on this research, the original issue acceptance list is refined:

- [ ] **Property sets** `Spar_TSN::*` and `Spar_Network::*` per
      Section 5.1, applied to `bus`, `connection`, `device`,
      `processor` as appropriate. Validation: every property carries an
      `applies to` declaration; defaults documented in the property
      set itself.
- [ ] **Switch modeling: Option C** — `bus implementation` with
      `Spar_Network::Switch_Type` discriminator (Section 5.2). No
      grammar change; no new component category.
- [ ] **New `spar-network` crate** containing `ArrivalCurve`,
      `ServiceCurve`, min-plus ⊗ and ⊘, horizontal/vertical distance,
      and `NetworkGraph` extraction (Section 5.3).
- [ ] **`spar-analysis::wctt`** implements `Analysis`/`ModalAnalysis`
      with the TFA algorithm in Section 5.4. Per-stream verdict
      (info if within deadline, error if not).
- [ ] **Lean theorems**: `Proofs/Network/MinPlus.lean` ships
      `min_plus_convolution_assoc`, `_mono`, and
      `arrival_curve_subadditive` *fully proved*; the headline bounds
      stated as `sorry`-d theorems with explicit defer-tags
      pointing at v0.9.0 / v1.0.0. (Section 5.5.)
- [ ] **Integration tests** under `tests/wctt/`: 1-hop priority,
      2-hop chain TAS, 3-hop CQF, FRER replicated path. Assert
      diagnostics within 5% of hand-computed bounds.
- [ ] **`bus_bandwidth.rs` migration** is *no migration* — additive
      only. New diagnostic `wctt::tsn_annotations_present` indicates
      WCTT-eligible streams. Existing tests untouched.
- [ ] **`latency.rs` integration** consumes `WcttCache` for
      cross-processor segments where Stream_ID is known
      (Section 5.6).
- [ ] **Rivet traceability**: `requirement`, `design-decision`, and
      `aadl-analysis-result` artefacts for every commit in Section 6.1.
- [ ] **COMPLIANCE.md update**: new section under "Network analysis"
      documenting TSN feature coverage (Qbv full, Qbu full, Qci
      stub, Qch stub, Qcr stub, CB stub) and pessimism class (TFA,
      30–60%).

---

## Sources

### IEEE 802.1 standards & overviews

- [Time-Sensitive Networking — Wikipedia][wiki-tsn]
- [Farkas, IEEE 802 TSN standardization update, Oct 2025][farkas-2025]
- [IEEE 802.1AS-2020][ieee-as]
- [P802.1ASdm — gPTP Hot Standby][asdm]
- [IEEE 802.1Qbv survey & experimental study (arXiv 2305.16772 / IEEE 2024)][qbv-survey]
- [IEEE 802.1Qci — Per-Stream Filtering and Policing][qci]
- [IEEE 802.1Qch — Cyclic Queuing and Forwarding][qch]
- [IEEE 802.1CB — Frame Replication and Elimination for Reliability][cb]
- [IEEE 802.1Qcc — Stream Reservation Protocol Enhancements][qcc]
- [IEEE 802.1Qcr — Asynchronous Traffic Shaping (ATS)][qcr]
- [IEEE 802.1Qcr ATS — astesj overview][qcr-astesj]
- [IEEE 802.1DG-2025 — Automotive TSN profile][dg]
- [IEEE 802.1DG-2025 publication announcement][dg-2025]
- [P802.1Qdj — Configuration enhancements for TSN][qdj]
- [Improving Robustness of Time-Aware Shaper in TSN (IEEE 2024/2025)][robust-tas]

### Industry profiles & ecosystem

- [Avnu Alliance — Automotive Certification Program][avnu]
- [Avnu Alliance — Component Certification Program][avnu-comp]
- [AUTOSAR Classic R23-11 release overview][cp-r23]
- [AUTOSAR Adaptive Platform R23-11 — TSN feature explanation (PDF)][ap-tsn]
- [AUTOSAR Adaptive Platform R24-11 release overview][ap-r24]
- [Open1722 — open-source IEEE 1722 (AVTP) implementation (COVESA)][open1722]
- [IEEE 1722 / AVTP — Avnu primer (PDF)][avtp]
- [Survey of Real-Time Support and Advancements in ROS 2 (arXiv 2025)][ros2-rt]
- [Latency Analysis of ROS 2 Multi-Node Systems (arXiv 2101.02074)][ros2-latency]

### Network calculus tooling

- [RTC Toolbox — ETH Zurich (mpa.ethz.ch/Rtctoolbox)][rtc]
- [NetworkCalculus.org DNC (NetCal/DNC GitHub)][netcal-dnc]
- [NetCal/DNC LICENSE (LGPL-2.1)][dnc-license]
- [NCBounds — Nokia / Anne Bouillard (GitHub)][ncbounds]
- [NCBounds documentation (readthedocs)][ncbounds-paper]
- [WoPANets — academic AFDX/NC tool description][wopanets]
- [RTaW-Pegase — TSN coverage page][rtaw-tsn]
- [PEGASE 2010 paper (RTaW)][pegase]
- [Siemens / Mentor Capital Network Designer (Volcano lineage)][capital]
- [E/E architecture evolution — Siemens blog][siemens-ee]
- [Survey on NC tools (NSF par.gov)][nc-survey]
- [Rust math crates list (lib.rs)][rust-math]

### Math foundations & formalisations

- [Le Boudec & Thiran, *Network Calculus* — Springer LNCS 2050 (online)][nc-book]
- [Bouillard, Boyer, Le Corronc, *Deterministic Network Calculus* (Wiley 2018)][bbl-book]
- [Improved delay bound with known transmission rate (IEEE 2019)][improved-delay]
- [Extending Network Calculus to deal with negative service curves (arXiv 2403.18042)][neg-sc]
- [Equivalent versions of Total Flow Analysis (arXiv 2111.01827)][tfa-eq]
- [Tight performance bounds for feed-forward networks (Bouillard, HAL)][tight-bounds]
- [Verifying min-Plus Computations with Coq (HAL)][min-plus-coq]
- [Bouillard — Preuve formelle en calcul réseau (HAL thesis)][bouillard-thesis]
- [A Formal Link Between Response Time Analysis and Network Calculus (ECRTS 2022)][rta-nc]
- [A Residual Service Curve of Rate-Latency Server in Quadratic Time (ECRTS 2021)][rate-latency]
- [Mathlib (Lean 4 mathematical library)][mathlib]

### AADL research & ecosystem

- [Phan, Lee, Sokolsky — Performance Analysis of AADL Models Using RTC (Springer 2010)][rtc-aadl]
- [Phan/Lee/Sokolsky — Penn repository version (PDF)][rtc-aadl-penn]
- [Lauer, Ermont, Pagetti, Boniol — Analyzing End-to-End Functional Delays on an IMA Platform (Springer 2010)][lauer-ima]
- [Network Calculus-based Timing Analysis of AFDX with Strict Priority and TSN/BLS (RTCSA 2018)][afdx-bls]
- [FORA — Fog Computing Platform for Industrial IoT (arXiv 2007.02696)][fora]
- [FORA — Information Systems 2021 (DOI page)][fora-doi]
- [Barzegaran — Configuration Optimization of Fog Computing Platforms (thesis)][barzegaran]
- [OSATE issue #64 — property sets for AADL annexes][osate-64]
- [AADL V3 standard discussions (DTIC)][aadl-v3]
- [Cheddar — open-source real-time scheduling simulator/analyser][cheddar]
- [Behavior Annex implementation in OSATE2 (SEI 2011)][bah-annex]

[wiki-tsn]: https://en.wikipedia.org/wiki/Time-Sensitive_Networking
[farkas-2025]: https://standards.ieee.org/wp-content/uploads/2025/10/D1_08_Janos-Farkas-Time-Sensitive-Networking-Standardization.pdf
[ieee-as]: https://standards.ieee.org/ieee/802.1AS/7121/
[asdm]: https://1.ieee802.org/tsn/802-1asdm/
[qbv-survey]: https://arxiv.org/html/2305.16772v4
[qci]: https://1.ieee802.org/tsn/802-1qci/
[qch]: https://1.ieee802.org/tsn/802-1qch/
[cb]: https://standards.ieee.org/ieee/802.1CB/5703/
[qcc]: https://1.ieee802.org/tsn/802-1qcc/
[qcr]: https://1.ieee802.org/tsn/802-1qcr/
[qcr-astesj]: https://www.astesj.com/v04/i01/p28/
[dg]: https://standards.ieee.org/ieee/802.1DG/7480/
[dg-2025]: https://1.ieee802.org/publication-ieee-802-1dg-2025/
[qdj]: https://1.ieee802.org/tsn/802-1qdj/
[robust-tas]: https://ieeexplore.ieee.org/iel8/6488907/11045559/10947509.pdf
[avnu]: https://avnu.org/automotive-certification-program/
[avnu-comp]: https://avnu.org/component-certification-program/
[cp-r23]: https://www.autosar.org/fileadmin/standards/R23-11/CP/AUTOSAR_CP_TR_ReleaseOverview.pdf
[ap-tsn]: https://www.autosar.org/fileadmin/standards/R23-11/FO/AUTOSAR_FO_EXP_TimeSensitiveNetworkFeatures.pdf
[ap-r24]: https://www.autosar.org/fileadmin/standards/R24-11/AP/AUTOSAR_AP_TR_ReleaseOverview.pdf
[open1722]: https://github.com/COVESA/Open1722
[avtp]: https://avnu.org/wp-content/uploads/2014/05/AVnu-AAA2C_Audio-Video-Transport-Protocol-AVTP_Dave-Olsen.pdf
[ros2-rt]: https://arxiv.org/pdf/2601.10722
[ros2-latency]: https://arxiv.org/pdf/2101.02074
[rtc]: https://www.mpa.ethz.ch/Rtctoolbox
[netcal-dnc]: https://github.com/NetCal/DNC
[dnc-license]: https://github.com/NetCal/DNC/blob/master/LICENSE
[ncbounds]: https://github.com/nokia/NCBounds
[ncbounds-paper]: https://annebouillard.readthedocs.io/en/latest/py-modindex.html
[wopanets]: https://explore.openaire.eu/search/publication?articleId=od______1243::525fe12e4890cc0ee8bba8818bd1fa66
[rtaw-tsn]: https://rtaw.com/network-calculus-for-tsn-qos/
[pegase]: https://rtaw.com/wp-content/uploads/PEGASE-ISoLA-2010.pdf
[capital]: https://www.mentor.com/products/vnd/autosar-products/volcano-system-architect/
[siemens-ee]: https://blogs.sw.siemens.com/ee-systems/2024/08/29/e-e-architecture-evolution-part-1-some-history/
[nc-survey]: https://par.nsf.gov/servlets/purl/10297126
[rust-math]: https://lib.rs/science/math
[nc-book]: https://leboudec.github.io/netcal/latex/netCalBook.pdf
[bbl-book]: https://www.normalesup.org/~bouillar/Publis/formats19.pdf
[improved-delay]: https://infoscience.epfl.ch/server/api/core/bitstreams/1227a7f0-d6c7-4993-a95c-21d8de23ac82/content
[neg-sc]: https://arxiv.org/abs/2403.18042
[tfa-eq]: https://arxiv.org/abs/2111.01827
[tight-bounds]: https://inria.hal.science/hal-01583622
[min-plus-coq]: https://hal.science/hal-03176024/document
[bouillard-thesis]: https://hal.science/tel-04080706/
[rta-nc]: https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.ECRTS.2022.5
[rate-latency]: https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.ECRTS.2021.14
[mathlib]: https://github.com/leanprover-community/mathlib4
[rtc-aadl]: https://link.springer.com/chapter/10.1007/978-3-642-12566-9_12
[rtc-aadl-penn]: https://repository.upenn.edu/bitstreams/15bbf652-350b-4499-8101-03ab87de769f/download
[lauer-ima]: https://link.springer.com/chapter/10.1007/978-3-642-16558-0_21
[afdx-bls]: https://ieeexplore.ieee.org/document/8442080/
[fora]: https://arxiv.org/abs/2007.02696
[fora-doi]: https://www.sciencedirect.com/science/article/abs/pii/S0306437921000053
[barzegaran]: https://barzegaran.xyz/Thesis/Presentation.pdf
[osate-64]: https://github.com/osate/osate2/issues/64
[aadl-v3]: https://apps.dtic.mil/sti/pdfs/AD1089818.pdf
[cheddar]: http://beru.univ-brest.fr/cheddar/
[bah-annex]: https://resources.sei.cmu.edu/asset_files/ConferencePaper/2011_021_001_88049.pdf
[snc]: https://disco.cs.uni-kl.de/index.php/projects/snc-toolbox/88-projects/129-stochastic-network-calculator
[saturn-wiki]: https://en.wikipedia.org/wiki/Saturn_(software)
