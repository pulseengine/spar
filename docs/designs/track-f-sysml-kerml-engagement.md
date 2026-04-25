# Track F — SysML v2 / KerML community engagement strategy

Status: **proposed** — synthesizing two parallel research streams (spar-sysml2
audit + community state survey) into an actionable engagement plan.
Last update: 2026-04-25.

Companion to issue #149 (Track D, TSN/WCTT) and #150 (Track E, migration
oracle). This track is **community-and-standards-engagement-led**, not
implementation-led; the technical roadmap follows from the engagement
positioning, not the other way around.

## Executive summary

Three findings reshape what we thought we knew:

1. **`spar-sysml2` is production-grade, not a stub.** 7,167 LOC, zero
   `todo!()` / `unimplemented!()` macros, 59+ tests, conformance tests
   against official Systems-Modeling spec examples, lossless parsing
   verified. Eight concepts are fully bidirectional (parse + lower +
   extract + generate), including the **entire requirements roundtrip**
   (`satisfy` / `verify` / `refine` / `allocate` / `derive`). That is
   the single most important credibility marker for community engagement.

2. **The OMG `Systems-Modeling/SysML-v2-AADL-Release` repo exists and is
   skeletal.** Three commits, no tagged release, four named maintainers
   from Galois (Hugues), CMU/SEI (Seibel + Wrage), and Ellidiss (Dissaux).
   Goal: SAE+OMG joint AADL+SysML v2 standard for safety-critical
   (ARP4754 / DO-178C/330/331 / DO-254). Flows + modes are explicitly
   "not yet translated" per the README. **This is exactly where spar's
   first contributions belong.**

3. **spar is not duplicating an existing Rust effort.** Jade Wilson's
   `syster-base` / `syster-lsp` (Microsoft, MIT, alpha, ~10 stars) is
   the most mature Rust SysML v2 parser. It targets the SysML v2 side;
   spar targets AADL with SysML v2 emit. Adjacent, not duplicate. Direct
   collaboration on grammar conformance tests is plausible.

The engagement plan therefore prioritizes the **AADL-Release repo** + the
**OMG RTESC working group** (whose output that repo is) as the primary
venue, with the Google Group + OMG Issue Tracker as supporting channels.
Eclipse SysON is a secondary track, lower priority.

## §1 — spar-sysml2 audit (verified, 2026-04-25)

### 1.1 Coverage matrix

Grouped by status. Full audit data preserved in agent report archive.

**Fully bidirectional** (parse + lower + extract + generate):

| Concept | Notes |
|---|---|
| `part def`, `part usage` | Maps to AADL `system implementation` |
| `port def`, `port usage` | Maps to AADL `data port` / `event port` |
| `requirement def`, `requirement usage` | Lossless requirement roundtrip |
| `action def`, `action usage` | Maps to AADL `subprogram` |
| `state def`, `state usage` | Maps to AADL `mode` |
| `satisfy req by` | Requirement-to-architecture link |
| `verify req by` | Requirement-to-test link |
| `refine req by` | Requirement decomposition |
| `allocate task to processor` | AADL processor binding |
| `derive req from` | Requirement traceability |

**Parse + Lower** (no extract / generate yet):

| Concept | Why it matters |
|---|---|
| `connect`, `bind` | Lowered to AADL connection; `bind` aliased to `connect` |
| `interface def`, `interface` | Lowered to AADL feature group type |
| `attribute def`, `attribute` | Lowered to AADL data type |
| `enum def`, `enum` | Lowered with `Data_Model::Enumerators` property |
| `constraint def`, `constraint` | Lowered as timing-constraint property |
| `calc def` | Lowered to AADL subprogram |
| `allocation def` | Lowered to processor binding |
| specialization (`:>`, `:>>`, `specializes`, `subsets`, `redefines`) | Lowered to AADL `extends` |
| multiplicity (`[N]`, `[0..*]`) | Lowered to AADL array subcomponents |

**Parse-only** (keywords recognized, no semantic action):

- Behavioral: `transition`, `entry`/`do`/`exit` actions, `flow`, `succession`, `perform`
- Variants: `variant`, `variation def`, `select`
- Meta-modeling: `view def`, `viewpoint def`, `metadata def`, `annotation`
- Verification: `assert`, `verify` (as a keyword separate from the relationship verb)
- KerML: `feature def`, `class def` *(not recognized)*, `metaclass def` *(not recognized)*
- Type system: conjugation (`~Type`), end features, complex expressions

**Missing entirely:**

- `class def`, `metaclass def` — affects very little real modeling, but they're
  KerML kernel concepts; should be addressed before any KerML contribution
- Arithmetic / function-call expressions — parsed as identifiers only

### 1.2 Test coverage

| Test file | LOC | Tests | Scope |
|---|---:|---:|---|
| `conformance_tests.rs` | 85 | 8 | Lossless roundtrip on official spec examples (Annex A SimpleVehicleModel + 6 others) |
| `validation_tests.rs` | 251 | 21 | End-to-end parse → lower → AADL on 20 numbered scenarios + 1 lossless guard |
| `fuzz_sysml2.rs` | 520+ | 30+ | Adversarial parse / lower / extract; no panics on malformed input |

**Gaps the tests admit (worth filing as tracking issues):**

- No tests for bidirectional roundtrip of behavioral content (state machines, flow specs)
- No tests for expression evaluation
- No tests for variant management constructs

### 1.3 Public API

```rust
parse(&str) -> Parse                                  // CST
lower_to_aadl(&Parse) -> ItemTree                     // → AADL HIR
lower_to_aadl_with_diagnostics(&Parse) -> (ItemTree, Vec<LowerDiagnostic>)
extract_requirements_list(&Parse) -> Vec<ExtractedRequirement>
extract_all(&Parse, include_architecture: bool) -> ExtractionResult
extract_all_yaml(&Parse, include_architecture: bool) -> String     // rivet YAML
parse_rivet_yaml(&str) -> Vec<RivetArtifact>
generate_sysml2(&[RivetArtifact]) -> String           // rivet YAML → SysML v2
```

Consumed by `spar-cli` `extract` / `generate` / `parse` / `lower` commands.

### 1.4 Spec-conformance signal

- README and `lower.rs:1–29` document the SEI mapping table SysML v2 ↔ AADL.
- `conformance_tests.rs:1–3` references the official Systems-Modeling/SysML-v2-Release
  example corpus.
- No version pinning to a specific OMG spec revision (KerML 1.0 / SysML 2.0 final
  was adopted June 2025; spar's parser predates that, so a spec-version annotation
  pass is a reasonable v0.8.x housekeeping commit).
- One known limitation noted at `lower.rs:276–279`: specialization-cycle detection
  not implemented (deferred to AADL backend).

### 1.5 What this means for engagement positioning

- **Don't market spar as "a SysML v2 parser stub".** It's a working bidirectional
  translator on the requirements + structural axis.
- **Do market spar as "AADL-side translator with full requirements roundtrip".**
  That's the sentence that signals competence to the OMG RTESC WG.
- **Be honest about the behavioral gap.** State machines and flow specs parse
  to keywords but don't lower to AADL behavior annex constructs. Calling this
  out preempts the obvious reviewer question.

## §2 — Standards & community landscape (verified, 2026-04-25)

### 2.1 Spec status

- KerML 1.0 + SysML v2 1.0 + Systems Modeling API 1.0 final adoption: **30 June 2025**
  (announced 21 July 2025).
- Latest release packaged in `Systems-Modeling/SysML-v2-Release`: **2026-03**
  (released 2026-04-10), 55 releases lifetime, 819 GitHub stars.
- KerML 1.1 / SysML 2.1 / Systems Modeling API 1.1 RTFs are **active** (no resolutions yet).
- A "round 2" FTF cleanup tracker (`SYSML2_-NNN`) is still being closed out.

### 2.2 The Systems-Modeling GitHub org — verified inventory

| Repo | Lang | Stars | Last update | Notes |
|---|---|---:|---|---|
| `SysML-v2-Release` | HTML+spec | 819 | 2026-04-17 | "Start here" — spec PDFs + installers + libraries |
| `SysML-v2-Pilot-Implementation` | Java + Xtext | 219 | 2026-04-22 | Reference impl; LGPL-3.0/GPL-3.0; Maven build; **no `CONTRIBUTING.md`**; issue #571 (LSP/tree-sitter, Jun 2024) has zero maintainer responses 22 months later |
| **`SysML-v2-AADL-Release`** | (n/a) | 8 | 2026-03-01 | **THE strategically critical repo — see §2.3** |
| `SysML-v2-API-Services` | Java | 83 | 2025-06-16 | Reference impl of API; ~10mo old |
| `SysML-v2-API-Java-Client` | Java | 17 | 2025-04-30 | |
| `SysML-v2-API-Cookbook` | Jupyter | 54 | 2025-03-10 | |
| `SysML-v2-API-Python-Client` | Python | 57 | 2021-10-14 | **Effectively abandoned (4.5 yr stale)** |

### 2.3 `SysML-v2-AADL-Release` — strategic anchor

- **What:** Domain extension library merging AADLv2 into SysML v2 — translates
  "most AADLv2 core language features" to SysML v2 parts/ports/attributes.
- **Status:** 3 commits on master, no tagged releases, README explicitly notes
  flows + modes are **not yet translated**.
- **Maintainers (named in README):**
  - Jérôme Hugues — Galois
  - Joe Seibel — CMU/SEI
  - Lutz Wrage — CMU/SEI
  - Pierre Dissaux — Ellidiss
- **Governance:** Output of OMG RTESC (Real-Time Embedded Safety-Critical)
  working group — an SMC working group. Joint SAE+OMG effort. Stated
  alignment goals: ARP4754, DO-178C, DO-330, DO-331, DO-254.
- **Related precedent:** Galois has previously released a SysML→AADL
  bidirectional bridge (referenced on Galois LinkedIn).

This is where spar contributes first.

### 2.4 Community channels

| Channel | Cost | What it gets |
|---|---|---|
| `groups.google.com/g/sysml-v2-release` | $0 (apply for membership) | Discussion with RIWG (Friedenthal, Seidewitz, et al.). Approval reportedly fast. **Public archive not available without joining.** |
| `github.com/Systems-Modeling/*` | $0 | Issue + PR submission. No NDA. |
| `issues.omg.org/issues/create-new-issue` | $0 (sign NCLA) | Spec issues against KerML 1.1, SysML 2.1, API 1.1 RTFs. JIRA login + Non-Member Contribution and License Agreement. |
| `github.com/eclipse-syson/syson` | $0 (sign ECA, 3-yr) | PR contributions to Eclipse SysON. Author email must match ECA-registered email. |

### 2.5 Membership cost matrix (caveat: OMG fee page locked from external fetch)

| Tier | Annual cost | Voting | SMC seat | Notes |
|---|---:|---|---|---|
| (Just file issues / PRs) | $0 (NCLA) | No | No | One-shot contributions, no WG seat |
| Google Group only | $0 | No | No | Discussion observer status |
| University Member | $550 | TF/SIG (no DTC/PTC) | Free | **Requires academic affiliation** |
| Trial Member | $2,150 once for 1 yr | No | Free | One named individual, time-limited |
| **Influencing Member** (≤$10M revenue tier) | $3,000 | TF/SIG (no DTC/PTC) | Free | Unlimited individuals, renewable, **right-sized for spar** |

**One indirect search snippet** matched the tier brackets stated above
($3,000 / $5,500 / $11,000 / $21,500 / $37,500 by revenue band), but the
OMG fee page is locked behind authentication and could not be independently
fetched. **Recommend re-confirming with `accounting@omg.org` or via a
logged-in browser session before quoting the exact figures publicly.**

SMC scope (verified via OMG's 2023-11-30 press release): the Systems Modeling
Community is **free for all eligible OMG member companies**, includes the
RIWG (Reference Implementation Working Group, runs the Google Group) and
the RTESC WG (the AADL+SysML v2 effort).

### 2.6 Eclipse SysON — separate ecosystem

- Project page: <https://mbse-syson.org/>
- Repo: <https://github.com/eclipse-syson/syson> — 273 stars, 1,422 commits,
  125/8 open issues/PRs.
- Latest release: **v2026.3.0** (2026-03-25), flagged "Major release with API breakage".
- Maintainers: **Obeo + CEA List**. Obeo handles UX/product; CEA List leads
  spec compliance for OMG. Powers the SysML v2 editing in Papyrus.
- 8-week release cadence, dual EPL-2.0 / LGPL-3.0.
- ECA: 3-year validity, free for individuals, copyright stays with author.
- Web-based modeler (browser, no install). Native Capella interop.

**vs. OMG Pilot-Implementation:** Eclipse SysON moves much faster, has a real
contribution path (ECA), and ships a graphical+textual UI. The Pilot-Implementation
is the reference for spec conformance. spar engages both, weighted toward OMG.

## §3 — Rust SysML v2 ecosystem (positioning)

| Project | Author | Scope | Status | License | Stars |
|---|---|---|---|---|---:|
| `syster-base` / `syster-lsp` / `syster-codegen` | **Jade Wilson (Microsoft)** | Rust SysML v2 parser + LSP + codegen | Alpha | MIT | ~10 |
| Sensmetry `Sysand` | Sensmetry | Rust SysML v2 package manager + registry | Active 2026 | MIT/Apache-2.0 | (varies) |
| `tree-sitter-sysml` | Community | tree-sitter grammar | Active 2026-03 | (varies) | (varies) |
| `kerml` (crate) | Community | Stand-alone | Lower activity | (varies) | (varies) |
| `sysml-parser` (crate) | Community | "Heavy construction" Mar 2025 | Stalled? | (varies) | (varies) |
| `sysml.rs` | artob | "🚧" marked | Unclear | (varies) | (varies) |
| **`spar-sysml2`** | PulseEngine | AADL-side parser + lower + extract + generate | Production | (workspace, MIT pending) | (in-tree) |

**spar's positioning sentence:** *the AADL-side Rust toolchain that produces
SysML v2 artifacts and round-trips requirements bidirectionally; complementary
to `syster` (which targets SysML v2 directly) and to the OMG Pilot-Implementation
(which is Java/Xtext).*

**Avoid:** any messaging that reads as "another Rust SysML v2 parser".
spar is on the AADL side and emits SysML v2; that's the differentiator.

## §4 — Strategic anchors

### 4.1 Direct contribution opportunities (high-leverage, low-cost)

1. **`SysML-v2-AADL-Release`** flows + modes mapping. The README explicitly
   names them as gaps. spar already lowers many of these AADL constructs
   in its own analyses; converting the mapping rules into the AADL-Release
   repo's library form is a focused contribution.
2. **OMG issue tracker** — KerML 1.1 / SysML 2.1 RTFs are active. Recent
   issues like the ones referenced in the research (e.g., `stakeholder-node`
   not defined in BNF, `FlowConnectionDefinitions` violating KerML structure
   restrictions) demonstrate the genre. spar's experience parsing the
   official spec examples puts it in a position to surface similar issues.
3. **Pilot-Implementation issue #571** (LSP/tree-sitter request, 22 months
   without maintainer response). Both spar's parser and `syster`'s LSP
   exist; offering them as community options is a visible contribution.
4. **`syster-cli` issue #4** (`sysml.library` doesn't parse cleanly in
   syster). spar has parsed the same library successfully in conformance
   tests; cross-validation would help both projects.

### 4.2 Tailwinds — DARPA / DoD / industry signals

- **DARPA PROVERS / INSPECTA** (Collins Aerospace + CMU + Dornerworks +
  UNSW + Kansas State / Hatcliff group) — extends Sireum HAMR to use
  SysML v2 instead of AADL, generates code to **Rust + seL4 microkit**
  among other targets. Direct US-defense validation that SysML v2 + AADL
  + Rust + safety-critical is the right combination.
- DoDI 5000.97 (Digital Engineering) + Aug 2025 SysML v2 info sheet from
  DoD CTO — public guidance on transitioning to v2.
- INCOSE + NDIA-TVC running 2026 MBSE symposia with v2 tracks.
- Tool maturity caveat: 2027–2028 is the production-tooling window per
  industry consensus. The current period favors implementation-grade
  contributors.

## §5 — 30 / 60 / 90 day plan

### Day 0–7 (this week)

1. **Apply to `sysml-v2-release` Google Group** with the application text
   in §7 below. ~5 minutes; days-to-hours approval. Provides a discussion
   surface to the RIWG.
2. **Sign the Eclipse Contributor Agreement (ECA)** at <accounts.eclipse.org>.
   Free, 3-year validity. No project commitment yet — just enables PRs
   to Eclipse SysON if/when one becomes useful.
3. **Run the OMG Pilot-Implementation locally** following its README
   (Eclipse 2025-12 + Maven). Translate one of spar's existing AADL
   fixtures to SysML v2 via spar's `generate` command, then load into
   the Pilot-Implementation's Jupyter kernel. Document what survives
   the roundtrip and what doesn't.
4. **Watch `Systems-Modeling/SysML-v2-AADL-Release`** repo. Read the
   commit history. Note the build process (which is barely documented
   today — itself a contribution opportunity).

### Day 8–30 (this month)

5. **First OMG issue submission** via NCLA at issues.omg.org. Use spar's
   parsing experience as the source. Specifically look for: BNF tokens
   that aren't defined, examples in the spec that don't parse cleanly,
   property semantics that are spec-ambiguous. spar's existing
   `spec_gaps.md` memory is a draft list.
6. **First PR or issue against `SysML-v2-AADL-Release`** with a flow
   or mode mapping rule. Even a draft is signal.
7. **Direct outreach to one of the four named maintainers** (Hugues at
   Galois is the natural first contact given his SysML→AADL bridge
   precedent). Subject: introducing spar as the AADL-side Rust toolchain
   that complements their library work. Don't pitch — offer to discuss.

### Day 31–60

8. **Cross-validate against `syster-base`** by parsing the same `sysml.library`
   and comparing diagnostics. File a joint issue if both find the same
   spec ambiguity.
9. **Attend at least one RIWG / RTESC meeting if invited** (the Google
   Group post velocity should make this clear by then).
10. **v0.8.0 commit**: align spar-sysml2 with current spec rev. Add a
    spec-version annotation to the parser ("aligned to SysML v2 1.0
    final, 2026-03 release") and document conformance status per
    §1.1's coverage matrix.

### Day 61–90

11. **Decision point: Influencing Member ($3,000/yr) via PulseEngine.**
    Triggers if: at least one merged PR / accepted issue has landed,
    AND at least 3 spar-filed RTF issues are accepted, AND v0.8.0
    Track D/E work demonstrates spar's strategic seriousness.
12. **Roadmap commit for v0.9.0+**: Track F technical milestones —
    KerML kernel coverage (`class def`, expression grammar), behavioral
    lower (state transitions, flow specs), variant lower
    (`variation def`, `variant`, `select`).

## §6 — Investment ladder & decision triggers

```
$0 path                       $3,000/yr path                 $X,XXX commitment
─────────                     ─────────────                  ────────────────
NCLA + Google Group           Phase 1: triggered by ≥1       Phase 2: triggered by
+ GitHub PRs                  AADL-Release PR or issue       sustained engagement
+ direct outreach             accepted by named maintainer.  + voting weight desired
+ ECA (Eclipse parallel)      Path: Trial Member ($2,150     in DTC/PTC.
                              one-time, no voting) for 1 yr  Path: Domain or Platform
                              evaluation, then upgrade to    Member tier (~$5,500+
                              Influencing Member ($3,000/yr  at <$10M revenue per
                              renewable, full TF/SIG voting, indirect snippet).
                              free SMC seat).
```

**Phase 1 trigger criteria (specific):**
- A PR or issue submitted by spar to `SysML-v2-AADL-Release` is accepted.
- A spar-filed issue at issues.omg.org is acknowledged by an RTF chair.
- spar's roundtrip output on at least one industry-relevant model is referenced
  publicly (LinkedIn, conference, paper).

**Phase 2 trigger criteria (longer horizon):**
- spar-as-PulseEngine has produced ≥2 specification influence outputs (PR
  merged, issue accepted, paper presented at INCOSE / MODELS / FMICS).
- AADL-Release reaches its first tagged release with spar contributions visible.
- The SAE/OMG joint AADL+SysML v2 standard publishes its first draft.

## §7 — Application text

For the `sysml-v2-release` Google Group. Both lengths shipped — pick the
one that fits the form; the long version is preferred for setting up
direct credibility.

### Long version (~180 words — recommended)

> Name: Ralf Anton Beier
> Organizational affiliation: PulseEngine (pulseengine.eu) — independent
> open-source toolchain for safety-critical systems
> Interest in SysML:
> I maintain spar (github.com/pulseengine/spar), an open-source AADL v2.3
> toolchain in Rust with 27+ analysis passes, deployment allocation with
> ASIL decomposition, and an LSP server. spar has a working SysML v2
> parser-and-translator (spar-sysml2: 7,000+ LOC, lossless parsing,
> bidirectional roundtrip for the requirements domain via the
> satisfy/verify/refine/allocate/derive relationships, conformance
> tests against the official Systems-Modeling spec examples).
>
> I am also developing rivet (github.com/pulseengine/rivet), a
> schema-driven traceability validator for safety-critical SDLCs,
> where SysML v2's textual notation and the Systems Modeling API are
> directly relevant for AI-agent-driven workflows.
>
> My interest in joining is to follow the Reference Implementation work,
> understand spec ambiguities and edge cases as they're discussed, and
> contribute test models, OMG issues, and AADL-side mappings into the
> SysML-v2-AADL-Release repo. Background: 22 years of automotive software
> architecture, focused on formal verification, MBSE, and AI-assisted
> safety-critical development.

### Short version (~70 words)

> I maintain spar (an open-source AADL v2.3 toolchain in Rust with formal
> verification integration) and rivet (a schema-driven traceability
> validator), both at github.com/pulseengine. spar-sysml2 is a working
> bidirectional SysML v2 translator on the requirements domain. I would
> like to follow the Reference Implementation work, contribute mappings
> into SysML-v2-AADL-Release, and file OMG issues from spar's parsing
> experience. Background: 22 years automotive software architecture.

## §8 — Risks & unknowns

| Risk | Mitigation |
|---|---|
| OMG fee figures could not be independently fetch-verified (page locked) | Re-confirm with `accounting@omg.org` before quoting publicly |
| Google Group post velocity opaque without joining | Phase 0 application is itself the cost-of-information |
| `syster-base` (Microsoft, alpha) might accelerate | Quarterly re-check; if it does, spar's Rust angle weakens — but spar's AADL angle is not affected |
| Pilot-Implementation external contribution path is undocumented | Engage via Google Group + named maintainers, not via blind PRs |
| Influencing Member ($3,000/yr) is a real recurring cost | Phase 1 trigger criteria are explicit; don't upgrade without them |
| AADL-Release has only 3 commits — could stall | Outreach to maintainers de-risks this; if it stalls, pivot to direct OMG issue track |
| Behavioral / variant gaps in spar-sysml2 are real | Address in v0.8.x roadmap; messaging stays honest about the gap |

## §9 — Open questions for the user

1. **Re-confirm OMG fees** in a logged-in browser before any public quote.
2. **Decide $3,000/yr threshold** — is PulseEngine ready to budget that
   if Phase 1 triggers fire? (Not a blocker; just sets the deciding
   criterion when triggers arrive.)
3. **University-affiliation path** — is there a Fraunhofer / TU / industry
   research collaboration that could make University Member ($550/yr)
   viable? Best $/voting-power on the menu if so.
4. **Direct outreach order** — Hugues (Galois, AADL+formal-methods natural
   match) → Seibel/Wrage (CMU/SEI, AADL canonical authors) → Dissaux
   (Ellidiss, real-time/safety) is the recommended sequence. Confirm.
5. **Eclipse SysON priority** — secondary track. If a small first
   contribution there (doc fix, test case) happens early, it broadens
   the surface; if not, fine.

## §10 — Appendix: agent-collected raw data

Both research agents produced detailed reports during this session:

- **spar-sysml2 audit** — full coverage matrix (SysML v2 + KerML core +
  cross-cutting), test inventory, public-API surface, spec-conformance
  notes. Available in agent-completion log.
- **Community state research** — verified Systems-Modeling repo inventory,
  Pilot-Implementation deep dive, AADL-Release deep dive (the §2.3 above),
  OMG Issue Tracker semantics, Eclipse SysON parameters, OMG fee tier
  table (with auth-locked-page caveat), Rust ecosystem inventory, vendor
  landscape, DoD/DARPA/AUTOSAR signals, ~50 sources cited inline.

The synthesized doc above is the actionable derivative; raw data is
preserved in the run logs and in the project memory files
(`project_sysml_v2_engagement.md`, `project_post_v070.md`).
