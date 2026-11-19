# v0.50.0 plan: bets measured against the North Star

Status: draft, work in progress. Nothing here is tagged, and nothing here claims
production readiness. The scope change it proposes waits on the maintainer
(factory-notice decision `release-v0.50`).

The North Star (pulseengine.eu `data/north-star.toml`, updated 2026-11-18):

> A safety-critical product team can take a WebAssembly component from
> architecture model to signed code running on a microcontroller, with evidence
> an assessor can re-check, using open tools only.

spar is the architecture model, the first link of that chain. This plan asks
which spar work moves an objective's indicator as the indicator is written,
not which work is nearby.

## Method

Two passes, each by a separate agent in a fresh context:

1. A planner read the North Star, spar's rivet state (v0.50.0 scope: 26 items, 2
   verified), the 29 open issues, the v0.49.0 notes and the seams. It proposed
   four bets.
2. A checker that saw only the North Star and the draft answered
   confirm/refute/cannot-verify per bet: does it move an indicator, can its
   criterion fail, is anything overclaimed, is any of it busywork?

The checker refuted three of the four objective claims as drafted. This plan is
the revision. One fact was settled by measurement rather than argument:
`scripts/north-star-o2.py` counts the products listed with `pulls = ["O2"]`
(relay, wohl), so a spar release does not change the O2 number.

## Result in one table

| # | Bet | Objective | Moves the number? |
|---|---|---|---|
| 1 | Model-to-target handoff manifest | O1 | No. It supplies the join key synth's three O1 numbers attach to. |
| 2 | Connection-aware WIT contracts (#282) | none directly | No. It is a prerequisite: without it a multi-component model does not compose. |
| 3 | Proof check with axiom audit and trust-escape ledger, required | O3 | Yes, once O3 has an indicator script: spar becomes a repository the indicator counts. |
| 4 | spar-wasm bundle as reference fixture | O2 (enabler) | No. spar is not an O2 consumer. It is the first probe of witness on a component, ahead of the 2027-Q1 relay stretch. |
| 5 | Gate inventory and one signed WASI gate | O5 | Yes, from unmeasured to k/N with an independent N. k may stay 0. |
| side | Ready issues for other contributors | O4 | Possibly. Measured by the O4 script. |

## Bet 1: model-to-target handoff manifest

`spar codegen` writes one manifest per generated component. Each record binds:

- the AADL instance path and model digest;
- the WIT world and component digest;
- the meld-fused core-module digest;
- the processor and memory binding;
- per-thread Period, Deadline, Compute_Execution_Time and the computed response time;
- an ELF digest field that stays null until synth fills it.

`spar trace check` re-derives every field and fails on any difference. The
existing kiln oracle (spar, component, meld, core module, kiln) writes and
checks the manifest.

- **Indicator.** This moves no O1 number. O1 counts jess's rungs and synth's
  reach, execution match and WCET, per target and version. The manifest is the
  key those three numbers join on, back to a model element and its budget.
- **Acceptance.**
  - `spar trace check` passes on every oracle component. N is counted
    independently from the instance model, not from the manifest.
  - A planted control mutates each bound field once, one field per control,
    and every control turns the check red.
  - A WCET above the budget turns the comparison red.
  - At least one component carries a synth-reported WCET compared to its model
    budget. Threads without a synth number are reported as failed, never as
    "not attempted" (O1's guard: a skipped hardware run counts as a failure).
- **Kill.**
  - If sigil or witness already record these digests, spar emits their format
    and drops its own. This check comes first, before any code.
  - If synth cannot take a meld-fused core module from spar codegen without
    hand edits, the bet stops at the kiln leg and the seam is filed on synth.
- **Never claimed.** Execution on target. Translation validation.
- **Dependencies.** meld and kiln (already in the oracle), synth (WCET report
  format), gust (later consumer).
- **rivet.** REQ-TRACE-HANDOFF-001, REQ-TRACE-HANDOFF-CHECK-001,
  REQ-TRACE-BUDGET-WCET-001.

## Bet 2: connection-aware WIT contracts (#282)

`wit_gen` walks connection instances. It emits one shared interface per
cross-process connection; the producer exports it and the consumer imports it.
Today `crates/spar-codegen/src/wit_gen.rs` has no reference to connections.

- **Indicator.** None. It is listed because bet 1's chain stops at one
  component without it.
- **Acceptance.** The fixture has at least 3 processes and at least 4
  cross-process connections.
  - Generated components fuse with `meld fuse`. The tool is named on purpose:
    a weaker composer could pass where meld would not.
  - There are 0 unresolved imports, and the number of imports equals the number
    of cross-process connections counted from the instance model.
  - A value the producer sends is observed by the consumer under kiln.
  - A fixture whose endpoint types differ is rejected by `spar codegen`. The
    same fixture is first shown to pass codegen on main, so the rejection comes
    from the new code.
- **Kill.** If the shared interface needs a restructuring of `transform/wit.rs`
  (refactors are frozen in spar until 2027-02-01), ship port-to-port data
  connections only and defer request/response to v0.51.0.
- **rivet.** REQ-CODEGEN-WIT-CONN-001 exists (release v0.39.0, proposed);
  moving it to v0.50.0 is part of the decision. New:
  REQ-CODEGEN-WIT-CONN-MISMATCH-001.

## Bet 3: the proof check is required, with an axiom audit and a trust-escape ledger

The O3 indicator asks for proof-carrying repositories whose proof check, with a
kernel axiom audit and a trust-escape ledger, is a required check. The ledger
must count every assumption that narrows a harness's input range. Today spar
has:

- a committed Lean axiom ledger (verified);
- 20 source lines with `kani::assume`;
- 12 source lines matching `kani::proof`;
- Kani not installed on the self-hosted runners, so the five Kani steps in #638
  fail weekly.

- **Indicator.** O3 is unmeasured. This makes spar satisfy it: required proof
  check, axiom audit inside it, ledger inside it.
- **Acceptance.**
  - The proof check (Lean build, axiom audit, Kani harnesses, ledger) is in
    `.github/required-contexts.txt`.
  - The count of Kani harnesses executed equals the count of `#[kani::proof]`
    in source, printed side by side. A missing tool is a failure.
  - The ledger counts `kani::assume`, `kani::any_where`, unwind bounds, stubs,
    and Lean `axiom`, `sorry` and hypotheses in the audited theorems. Each count
    may only shrink.
  - Planted controls: one added `kani::assume`, one wrapped in a helper, one new
    Lean axiom. Each turns the check red.
- **Kill.** If Kani cannot be installed on the runners, the harnesses move to a
  hosted job; they are never marked skipped.
- **Not in this bet.** Splitting the scheduling proofs into their own Lake
  library (#272) serves kiln-async and moves no indicator. It stays as a
  separate item.
- **rivet.** REQ-PROOF-TRUST-ESCAPE-LEDGER-001, REQ-CI-KANI-EXECUTED-001,
  REQ-PROOF-CHECK-REQUIRED-001.

## Bet 4: spar-wasm bundle as the reference fixture

The release already builds spar for wasm32-wasip2. It would add:

- a rivet trace export;
- a witness MC/DC report on that component;
- a sigil attestation;
- verification by the same command the O2 script runs.

- **Indicator.** None of O2's numbers. O2 counts consumer repositories, and spar
  is not one. The value is that spar is the first to run witness on a component
  module, which the 2027-Q1 relay stretch needs.
- **Acceptance.**
  - The verify command runs in per-PR CI on a fixture build, not only at release
    time.
  - Removal controls: removing the trace, the MC/DC report or the attestation
    each makes it fail.
  - Tamper control: a piece with a stale digest also makes it fail.
  - The MC/DC report has more than 0 decisions and the trace has more than 0
    links, both printed.
- **Expected v0.50.0 outcome.** Verify fails because MC/DC is absent, blocked on
  witness reading components. That is reported as the state, and nothing is
  called a bundle until verify passes.
- **rivet.** REQ-RELEASE-BUNDLE-001, REQ-RELEASE-BUNDLE-NEG-001.

## Bet 5: gate inventory and one signed WASI gate

The O5 indicator counts our tools and gates that run as signed WASI components
under kiln, with wasmtime as the differential, out of all our tools and gates.

- **Acceptance.**
  - A committed inventory of spar's tools and gates, each tagged with its
    runtime (python, native, wasi-kiln).
  - A check that fails when a script or workflow gate is missing from the
    inventory. The independent count is `find tools -name '*.py'` (32 today)
    plus Rust gate binaries plus workflow gate steps.
  - At most one gate ported: tools/check-runner-guard, already std-only Rust, to
    wasm32-wasip2. It is signed and run under kiln, with wasmtime as the
    differential, and with an equivalence test against the native binary that
    includes a planted divergence.
- **Kill.** If kiln cannot give a gate argv and a read-only directory without a
  `*_GAP` switch, the number stays 0 of N, is reported as 0, and the seam is
  filed on kiln. If `plant guard` replaces the gate first, port nothing and keep
  the inventory.
- **rivet.** REQ-CI-GATE-INVENTORY-001, REQ-CI-GATE-WASI-001.

## Side item: ready work for other contributors (O4)

Publish the deferred items below as issues with acceptance criteria and a
failing-capable check each, labelled ready, with the ready time recorded.
Whether agent-authored pull requests count as people is for the O4 script to
say, not this plan.

## Proposed deferrals to v0.51.0

Each is real work that does not move an indicator this release:

- REQ-RENDER-COMPOUND-001, REQ-RENDER-GOLDEN-001, REQ-RENDER-METRICS-001
- REQ-TSN-CLI-SURFACE-001, REQ-DBC-CYCLE-TIME-CONSUMED-001
- REQ-SYSML2-FLOWS-MODES-001, REQ-FINDINGS-LAYER-001
- REQ-RELEASE-OPENVSX-001, REQ-RELEASE-VEX-001
- REQ-DEPS-ROWAN-SYNC-001, REQ-FUZZ-SOLVER-RSS-001

REQ-CI-MUTANTS-RATCHET-WALKDOWN-001 keeps its ratchet but not "to zero" in
this release.

Kept in v0.50.0 because a bet uses them:

- REQ-VERIFY-MEASURED-VALUE-001, for bet 1's budget comparison;
- REQ-CLAIM-LEDGER-001 and REQ-OSATE-ANALYSIS-ORACLE-001, for public claims
  backed by checks;
- REQ-PROOF-AXIOM-LEDGER-001, for bet 3;
- the release and verification items already implemented.

## Risks

- **Other repositories.** Bets 1, 4 and 5 depend on synth, witness, sigil and
  kiln. Each bet has a spar-only floor. Seam blocks are written before code.
- **O1 stays unmeasured after this release.** No hardware is reachable from
  spar's CI.
- **Runner state.** #502 and #638 can make every acceptance check flaky.
  Installing Kani lands first.
- **Size.** Five bets plus the kept items is more than one release has carried.
  If one goes, bet 4 goes first, since its expected outcome is a recorded
  failure.
