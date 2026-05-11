# Cutoff-theorem reasoning applied to spar's modal filtering pass

Status: **research note** — applies Lucio's DSLTrans Cutoff Theorem
(arXiv [2604.18792v2](https://arxiv.org/abs/2604.18792)) to spar's
per-System-Operational-Mode (SOM) analysis pipeline.
Last update: 2026-05-10.
Audience: spar maintainers thinking about Kani / proptest harnesses,
external readers comparing spar's verification posture against the
DSLTrans tractable-verification line.

> **Executive summary.** spar's modal filter — the helpers in
> `crates/spar-analysis/src/modal.rs` that decide whether a component
> or connection is active in a given SOM — fits the DSLTrans
> *positive-existence* fragment for the property "every element
> declared `in modes (s)` is preserved in the SOM-projection". For
> that property the cutoff bound is concrete and small: **K = 1 + C +
> E** where C is the maximum number of mode names listed in any one
> component's `in_modes` and E the same for any one connection. The
> theorem does **not** apply to (i) negative properties like "no
> inactive connection is preserved", (ii) downstream passes that
> close fixed-points (RTA recurrence, WCTT propagation, mode
> reachability), or (iii) the cartesian-product enumeration that
> builds the SOM set in the first place. The bound is therefore
> useful for verifying the filter, not the entire per-SOM pipeline.

---

## §1 What the modal filtering pass does in spar

The flattened AADL instance model in spar carries two pieces of modal
state:

- `ComponentInstance::in_modes: Vec<Name>`
  (`crates/spar-hir-def/src/instance.rs:84`).
- The list of `SystemOperationMode { name, mode_selections }` on the
  root `SystemInstance` (`crates/spar-hir-def/src/instance.rs:62`).
  Each SOM is one element of the cartesian product of every modal
  subcomponent's declared modes (AS5506 §12,
  `crates/spar-hir-def/src/instance.rs:31-43`).

The filter is three pure-function predicates in `modal.rs`:

- `is_component_active_in_som(instance, comp_idx, som)`
  (`crates/spar-analysis/src/modal.rs:42-75`).
- `is_connection_active_in_som(instance, conn_owner, conn_in_modes,
  som)` (`crates/spar-analysis/src/modal.rs:82-105`).
- `is_active_in_mode(in_modes, current_mode)` — single-mode variant
  used by analyses with no SOM context
  (`crates/spar-analysis/src/modal.rs:107-120`).

Each predicate is a constant-depth boolean expression over (a) the
element's declared `in_modes`, (b) the SOM's `mode_selections`, and
(c) case-insensitive name equality. There is no recursion, no
fixed-point, and no negation other than the early-return
`!in_modes.is_empty()` guard.

Eight `ModalAnalysis` impls consume these predicates today
(`grep "impl ModalAnalysis for" crates/spar-analysis/src`):
connectivity, binding-check, memory-budget, resource-budget,
weight-power, ARINC-653, bus-bandwidth, scheduling. The orchestrator
`AnalysisRunner::run_all_per_som`
(`crates/spar-analysis/src/lib.rs:290-311`) calls
`analyze_in_mode(instance, som)` once per SOM and prefixes every
emitted diagnostic with `[mode: <name>]`.

## §2 DSLTrans-fragment fit

Lucio's Cutoff Theorem (arXiv 2604.18792, abstract; theorem
statements in the paper's §3-§4) admits a DSLTrans fragment
characterised by: no negation in match patterns, no recursion, and
layered rules. The theorem proves that bounded model checking is
**complete** for *positive existence* and *traceability* properties
within that fragment — i.e. any counterexample is witnessed at some
finite bound K computable from the rule set.

The modal filter is not literally a DSLTrans transformation, but
viewed as a model-to-model projection
`filter : (Instance, SOM) → Instance` it satisfies the structural
preconditions:

| DSLTrans fragment requirement | Modal filter status |
|---|---|
| No negation in matchers | Match: filter uses positive predicates (`a.eq_ignore_ascii_case(b)`); the `is_empty()` guard short-circuits to *accept*, not reject. |
| No recursion | Match: each predicate is constant-depth. |
| Layered rules | Match: component-filter and connection-filter are independent passes over disjoint arenas. |
| Bounded fan-out per rule | Match: a component sees at most `\|in_modes\|` mode names and the SOM has at most one selection per parent; both are statically bounded by the input. |

The hierarchy-walk inside `is_component_active_in_som`
(`modal.rs:55-71`) reads the parent index once and scans the SOM's
`mode_selections` linearly — no recursive descent on the component
tree. That is the structural property the cutoff theorem leans on.

## §3 The positive-existence property and its bound K

**Property to verify.** Pseudo-DSLTrans notation, against a fixed SOM
`s`:

```
∀ component c ∈ instance.components :
    c.in_modes = ∅
    ∨ ∃ (parent_sel, mode_inst) ∈ s.mode_selections :
        mode_inst.owner = c.parent
        ∧ ∃ m ∈ c.in_modes : m ≈ mode_inst.name
    ⟹  c is active in projection(instance, s)
```

This is a positive existence statement: "for every element matching a
positive antecedent, the projection contains the witness". No
universally-quantified negative ("no inactive element appears") and
no recursion (the antecedent is depth-1 over the parent edge).

**The bound K.** The cutoff theorem says we can witness any
counterexample at some K derivable from per-class bounds (paper §3,
"per-class bounds"). For our property, the only unbounded inputs are
(a) the size of `in_modes`, (b) the number of `mode_selections` in
the SOM. Constructing a counterexample requires:

- one component with `in_modes ≠ ∅` (1 element);
- enough mode names in `c.in_modes` to exhibit every
  case-insensitive mismatch class against the SOM's owner mode
  (≤ C distinct names);
- enough entries in the SOM to exhibit at least one selection on a
  non-matching owner (≤ 1 entry is enough for the falsifying case).

Define

```
C  = max over components c of |c.in_modes|
E  = max over connections k of |k.in_modes|
M  = max over modal components of |modes|     (used for the SOM-set bound, see §5)
P  = total number of modal subcomponents
```

Then the cutoff for the **filter-correctness property** alone is

```
K_filter  =  1  +  max(C, E)
```

A counterexample to "active-in-SOM ⟹ preserved" or to "preserved ⟹
active-in-SOM" exists in the full model iff it exists in some
sub-instance with at most `K_filter` mode-name labels per element.
This follows because all comparisons are case-insensitive equality on
`Name`s; once we exhaust the equivalence classes of strings present
in the input, adding a (`K_filter` + 1)-th name only repeats an
existing equivalence class.

## §4 Worked estimate of K on a real spar model

Take `test-data/parser/modes_test.aadl` (the canonical multi-mode
example):

- `Modal_System.impl` (top): 3 declared modes
  (`nominal`, `degraded`, `emergency`) on the parent
  (`modes_test.aadl:9-13`).
- `normal_proc` with `in modes (nominal)` ⇒ C contribution 1.
- `backup_proc` with `in modes (degraded, emergency)` ⇒ C
  contribution 2.
- Connections `c1` ⇒ E contribution 1; `c2` ⇒ E contribution 2.

So `C = 2`, `E = 2`, **K_filter = 3**. Three labels per element
suffice to exhaustively test the filter on any instance with the same
mode-name vocabulary. A Kani harness or capped proptest that
enumerates components with `|in_modes| ∈ {0, 1, 2, 3}` against SOMs
with `|mode_selections| ∈ {0, 1}` covers every behavioural
equivalence class of the filter.

For the larger `test-data/aadl2rust/connection_in_modes.aadl` fixture
(`ConnectionInModes`,
`test-data/aadl2rust/connection_in_modes.aadl:1-25`) we have 2 modes
on the parent and 1 mode-name per connection, so `K_filter = 2`.

In practice every modal fixture in `test-data/` runs comfortably
inside K = 3. The bound is small because AADL `in modes (…)` lists
are short by convention — they enumerate semantic modes, not
arbitrary identifiers.

## §5 What this buys us — a discharge plan

With `K_filter` in hand the filter can be verified two ways:

**Option A — capped proptest.** A `proptest!` harness in
`crates/spar-analysis/src/modal.rs` under `#[cfg(test)]` that
generates `ComponentInstance` / `SystemOperationMode` pairs with
`|in_modes| ≤ K_filter` and asserts:

```
filter(instance, som).contains(c)
    ⟺  is_component_active_in_som(instance, c, som)
```

The cutoff theorem says the proptest is **complete** at K, not just
randomly sound. Today the file has only example-based tests
(`modal.rs:165-234`); promoting to property-based with the cutoff
gives qualitative coverage.

**Option B — Kani harness.** Bound the proof at K and ask Kani to
exhaust the symbolic state space. The proof effort is small because
the filter is straight-line code with no loops over arenas (the only
loop, `for &(_sel_comp, mode_inst_idx) in &som.mode_selections`,
walks a `Vec` bounded by `P`).

The recommended cargo command is

```
cargo kani -p spar-analysis --harness modal::proofs::filter_consistency
```

once a harness is added; or simply

```
cargo test -p spar-analysis modal::proptest_filter_at_cutoff
```

for the proptest path. The verification.yaml entry would be a
`type: feature` `VERIF-MODAL-CUTOFF` linking to `ARCH-ANALYSIS` and
the `STPA-REQ-017` already cited at the top of `modal.rs:1`.

## §6 What does NOT work

The theorem has clean edges. Honest restatements of each gap:

**Gap 1: negative properties.** "No inactive connection appears in
the SOM-projection" is universally negative. Lucio's theorem
(arXiv 2604.18792 §3) only covers positive existence / traceability.
For negatives we would need either (a) a separate completeness
argument or (b) reformulation as a positive property over the
complement instance. spar's filter does emit only the active subset,
so the negative property holds *by construction* — but that is a
constructive argument, not a cutoff-theorem one.

**Gap 2: cartesian-product SOM enumeration.** The set of SOMs is
`|system_operation_modes|` and is itself the cartesian product
`∏ |modes(c)|` over modal subcomponents `c`. For `P` modal
subcomponents each with `M` modes, `|SOMs| = M^P`. The filter's K is
small *per SOM*; the cost of running per-SOM analyses is **not**
bounded by K. The cutoff theorem says nothing about reducing the
number of SOMs — and shouldn't, because each SOM is a semantically
distinct point in the design space. The exponential blow-up is in
the input, not the verification.

**Gap 3: fixed-point downstream passes.** `RtaAnalysis`
(`crates/spar-analysis/src/rta.rs`) computes a response-time
recurrence `R_i^{n+1} = C_i + B_i + Σ_j ⌈R_i^n / T_j⌉·C_j`. The
fixed-point iteration is unbounded a priori (terminates only when
converged or unschedulable). `WcttAnalysis`
(`crates/spar-analysis/src/wctt.rs`) composes residual service
curves over a chain of hops — also a fold whose depth equals the
hop count. Neither pass has a per-class cutoff in the sense Lucio
uses; the convergence guarantees come from real-time scheduling
theory (Joseph-Pandya, Liu-Layland) and Network Calculus min-plus
algebra (`proofs/Proofs/Network/MinPlusPwa.lean` skeleton), not from
DSLTrans-style structural induction. **The cutoff does not compose**
through these passes.

**Gap 4: mode reachability.** `ModeReachabilityAnalysis`
(`crates/spar-analysis/src/mode_reachability.rs`) computes reachable
SOMs from an initial SOM via a labelled transition system. That is a
graph reachability problem — exactly the recursive structure the
DSLTrans fragment excludes. The Kripke-style export to NuSMV
(`mode_reachability::export_smv`,
`crates/spar-cli/src/main.rs:1208`) routes this analysis to an
external model checker by design.

**Gap 5: case-insensitive name equality as a hidden quantifier.** A
strict reading of the DSLTrans fragment requires equality predicates
to be decidable in constant time per pair. `a.eq_ignore_ascii_case(b)`
is constant-time per character pair, but if we admit arbitrarily long
identifiers the per-comparison cost is not O(1). In the spar AADL
parser identifiers are bounded by the lexer
(`crates/spar-parser/src/lexer.rs`), so practical inputs are fine,
but a formal proof would need to discharge that bound.

## §7 Open questions

- Could the same analysis apply to spar's *connection-walk* passes
  (e.g. `flow_check.rs`)? Those follow `connections[i].dst → next
  component`, which is non-recursive only if the flow graph has
  bounded depth — i.e. only on acyclic flow declarations. Worth a
  follow-up note.
- The DSLTrans CEGAR-based fragment verification (arXiv 2604.18792
  abstract) supports automatic refinement when a property does not
  fit the fragment. It is unclear whether the modal-filter Kani path
  benefits from CEGAR; the filter is small enough that the SMT
  encoding is cheap regardless.
- `is_component_active_in_som` returns `true` when the parent has no
  mode selection in the SOM (`modal.rs:73-74`). This is a
  *permissive* default and matches AS5506 semantics — but the
  bidirectional cutoff property in §3 only covers the case
  `c.parent.has_selection ⟹ active iff matches`. The default-active
  branch is trivially correct but worth flagging in the
  formalisation.

## §8 What we did not change

This document does not modify any code in `crates/spar-analysis`.
The cutoff property is asserted on the existing filter as it stands
post-merge of #140. No bugs were found during the read; the modal
helpers in `modal.rs` are tight.

---

### References

- Lucio, L. *Tractable Verification of Model Transformations: A
  Cutoff-Theorem Approach for DSLTrans.* arXiv:2604.18792v2 (2026).
  Abstract: Cutoff Theorem for positive existence / traceability;
  Z3-based implementation; 552/899 properties proved, 345
  counterexamples, 2 undecided across ATL Zoo benchmarks.
- spar source references (all under
  `/Users/r/git/pulseengine/spar/`):
  - `crates/spar-analysis/src/modal.rs:42-105` — filter predicates.
  - `crates/spar-analysis/src/lib.rs:290-311` — per-SOM driver.
  - `crates/spar-hir-def/src/instance.rs:31-85` — instance + SOM
    types.
  - `test-data/parser/modes_test.aadl:1-37` — worked-example fixture.
  - `test-data/aadl2rust/connection_in_modes.aadl:1-25` — second
    fixture for connection filter.
