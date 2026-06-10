/-
  Network Calculus — Piecewise-Affine (T-SPEC) Min-Plus Theorems

  Mirrors `crates/spar-network/src/curves.rs::piecewise`
  (v0.9.3 NC tightness item #1).  The Rust implementation generalises
  the single-bucket affine arrival curve to a min-of-affines family
  α(t) = min_i (σ_i + ρ_i · t), capturing T-SPEC-style multi-leaky-bucket
  traffic descriptors.  This file states the corresponding mathematical
  claims so they can be machine-checked.

  Reference: Le Boudec & Thiran, "Network Calculus", Springer 2001
  (chapter 1 §1.4 — multi-leaky-bucket arrival curves and the
  pointwise-min closure of the min-plus operators).

  Closed forms captured:

    α(t)         = min_i (σ_i + ρ_i · t)         (piecewise arrival)
    backlog      B = min_i (σ_i + ρ_i · T)        (per-bucket min, when ∀i, ρ_i ≤ R)
    delay        D = min_i (T + σ_i / R)          (per-bucket min)
    output bursts σ'_i = σ_i + ρ_i · T            (rate preserved per-bucket)
    residual     R' = R - max_i ρ_i               (conservative single-bucket lower)
                 T' = T + max_i ( σ_i / R' )

  The single-bucket spec in `MinPlus.lean` is unchanged; this file is
  a strict generalisation.  All theorems below are stated but not yet
  discharged — they are tagged `sorry -- TODO(v1.0.0)` per the project
  policy that statements are the load-bearing artefact and full
  proof-discharge is post-MVP.

  This file is **not** imported into `Proofs.lean` yet; it is an
  out-of-tree skeleton for the v1.0.0 sweep.  Once the per-bucket
  generalisations of MinPlus theorems 1-7 are discharged, this file
  joins the `Proofs.lean` import list and the v1.0.0 release pulls
  the piecewise curve into the load-bearing proof corpus.
-/

import Mathlib.Tactic
import Proofs.Network.MinPlus

namespace Spar.Network.MinPlusPwa

open Spar.Network.MinPlus (ArrivalCurve ServiceCurve scale)

/-! ## Type definition mirroring the Rust API -/

/-- Piecewise-affine arrival curve: a non-empty list of leaky buckets
    `(σ_i, ρ_i)` whose pointwise min is the arrival curve.

    Mirrors `crates/spar-network/src/curves.rs::piecewise::
    PiecewiseAffineArrivalCurve`.  In the Rust implementation the
    bucket list is canonicalised (sorted by σ ascending, deduped); we
    leave the list unconstrained here and prove the theorems modulo
    permutation. -/
structure PwaArrivalCurve where
  /-- Bucket list `(σ_i bytes, ρ_i bps)`, non-empty by hypothesis. -/
  buckets : List (Nat × Nat)
  /-- Non-emptiness witness — required so the min over buckets is
      defined.  Mirrors the `EmptyBuckets` constructor error. -/
  buckets_nonempty : buckets ≠ []
  deriving Repr

/-! ## α(t) — the curve evaluator -/

/-- α(t) for the piecewise form: minimum over per-bucket evaluations.

    `bits_to_bytes_in_window` is encoded as `(ρ * t) / scale` to match
    the single-bucket `MinPlus.lean` convention.  Causality at t = 0
    is enforced by the explicit short-circuit, matching the Rust impl
    and the single-bucket Lean spec. -/
def PwaArrivalCurve.at (α : PwaArrivalCurve) (t_ps : Nat) : Nat :=
  if t_ps = 0 then 0
  else
    -- Per-bucket readouts; min over the (non-empty) list.
    let readouts := α.buckets.map (fun (s, r) => s + (r * t_ps) / scale)
    -- `List.minimum?` returns `Option`; defaulting to 0 when the list
    -- is empty is safe because `buckets_nonempty` rules that out.
    readouts.minimum?.getD 0

/-! ## Theorem 1 — Causality at t = 0 -/

/-- Piecewise α at t = 0 is zero.  Direct from the explicit
    `if t_ps = 0` short-circuit, matching the single-bucket
    `arrival_at_zero_is_zero` in `MinPlus.lean`. -/
theorem pwa_arrival_at_zero_is_zero (α : PwaArrivalCurve) :
    α.at 0 = 0 := by
  unfold PwaArrivalCurve.at
  simp

/-! ## Theorem 2 — Monotonicity of α(t) -/

/-- α(t) is monotone non-decreasing in `t`.  Each per-bucket affine
    `σ_i + ρ_i · t / scale` is monotone in `t`; the pointwise minimum
    of monotone functions is monotone. -/
theorem pwa_arrival_at_mono (α : PwaArrivalCurve) {t1 t2 : Nat}
    (h : t1 ≤ t2) : α.at t1 ≤ α.at t2 := by
  -- TODO(v1.0.0): discharge.  Same shape as `arrival_at_mono` in
  -- `MinPlus.lean` (case-split on `t1 = 0`); the inductive step uses
  -- `Nat.div_le_div_right` per-bucket and `List.minimum?_le_iff`
  -- for the closure under min.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 3 — Per-bucket domination -/

/-- For every bucket `(σ_i, ρ_i)` of α and every `t > 0`,
    α(t) ≤ σ_i + ρ_i · t / scale.  This is the load-bearing
    "min ≤ each member" lemma that lets us recover the
    single-bucket bounds per bucket. -/
theorem pwa_at_le_per_bucket (α : PwaArrivalCurve) (t_ps : Nat)
    (ht : t_ps ≠ 0) (s r : Nat) (h_in : (s, r) ∈ α.buckets) :
    α.at t_ps ≤ s + (r * t_ps) / scale := by
  -- TODO(v1.0.0): discharge by `List.minimum?_le_of_mem` on the
  -- mapped readouts list.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 4 — Backlog closed form (per-bucket min)

    For piecewise α with every bucket stable (`ρ_i ≤ R`) and a
    rate-latency β, the backlog is bounded above by the **min** over
    per-bucket bounds — each bucket independently dominates α, so
    each per-bucket backlog `σ_i + ρ_i · T / scale` is a valid bound,
    and the smallest one is the tightest valid bound. -/

/-- Closed-form per-bucket backlog and the composite min.  Mirrors
    the Rust `piecewise::backlog_bound`. -/
def pwaBacklogClosedForm (α : PwaArrivalCurve) (β : ServiceCurve) : Nat :=
  let perBucket :=
    α.buckets.map (fun (s, r) => s + (r * β.latency_ps) / scale)
  perBucket.minimum?.getD 0

/-- **Theorem 4 — Piecewise backlog bound.**  For piecewise α with
    every bucket stable on β, the closed-form `pwaBacklogClosedForm`
    is a valid backlog bound: there exists `B` such that
    `α(t) ≤ β(t) + B` for every `t`. -/
theorem pwa_backlog_bound_classical
    (α : PwaArrivalCurve) (β : ServiceCurve)
    (h_stable : ∀ s r, (s, r) ∈ α.buckets → r ≤ β.rate_bps) :
    ∃ B : Nat,
        B = pwaBacklogClosedForm α β
      ∧ ∀ t : Nat, α.at t ≤ β.at t + B := by
  refine ⟨pwaBacklogClosedForm α β, rfl, ?_⟩
  intro _t
  -- TODO(v1.0.0): discharge.  Per-bucket version of the
  -- single-bucket `backlog_bound_classical` (which itself is a
  -- v1.0.0 sorry); apply `pwa_at_le_per_bucket` for each bucket and
  -- close the min.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 5 — Delay closed form (per-bucket min)

    For piecewise α with every bucket stable and `0 < R`, the delay
    is bounded above by the **min** over per-bucket delay bounds
    `T + σ_i / R`.  At any operating point one bucket binds; its
    `σ/R` term is the realised burst-drain time. -/

/-- Closed-form per-bucket delay and the composite min (rounded up
    per-bucket so the bound is never an under-estimate).  Mirrors
    the Rust `piecewise::delay_bound`. -/
def pwaDelayClosedForm (α : PwaArrivalCurve) (β : ServiceCurve) : Nat :=
  if β.rate_bps = 0 then 0
  else
    let perBucket :=
      α.buckets.map
        (fun (s, _) => β.latency_ps + (s * scale + β.rate_bps - 1) / β.rate_bps)
    perBucket.minimum?.getD 0

/-- **Theorem 5 — Piecewise delay bound.**  For piecewise α with
    every bucket stable on β and `0 < R`, the closed-form
    `pwaDelayClosedForm` is a valid delay bound: every byte arriving
    by `t` is served by `t + D`. -/
theorem pwa_delay_bound_classical
    (α : PwaArrivalCurve) (β : ServiceCurve)
    (h_stable : ∀ s r, (s, r) ∈ α.buckets → r ≤ β.rate_bps)
    (h_rate_pos : 0 < β.rate_bps) :
    ∃ D : Nat,
        D = pwaDelayClosedForm α β
      ∧ ∀ t : Nat, α.at t ≤ β.at (t + D) := by
  refine ⟨pwaDelayClosedForm α β, rfl, ?_⟩
  intro _t
  -- TODO(v1.0.0): discharge by selecting the binding bucket via
  -- `pwa_at_le_per_bucket` and reducing to the single-bucket
  -- `delay_bound_classical` (itself sorry); the per-bucket → min
  -- chase is a `List.minimum?` chase.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 6 — Output bound (per-bucket inflation)

    The output of a piecewise α through a rate-latency β is again
    piecewise: each bucket's burst grows by `ρ_i · T`, the rate is
    preserved per-bucket. -/

/-- Per-bucket output curve construction.  Mirrors the Rust
    `piecewise::output_bound`. -/
def pwaOutputClosedForm (α : PwaArrivalCurve) (β : ServiceCurve) :
    PwaArrivalCurve where
  buckets :=
    α.buckets.map (fun (s, r) => (s + (r * β.latency_ps) / scale, r))
  buckets_nonempty := by
    -- `List.map` preserves non-emptiness.
    intro h
    apply α.buckets_nonempty
    -- TODO(v1.0.0): discharge via `List.map_eq_nil` or its
    -- Mathlib alias.  Trivial structurally but needs the right
    -- lemma name.
    sorry -- TODO(v1.0.0)

/-- **Theorem 6 — Piecewise output bound.**  The output curve has
    the same number of buckets as the input, with each bucket's
    burst inflated by `ρ_i · β.latency_ps / scale` and the rate
    preserved.  The output dominates the input pointwise (matches
    the single-bucket `output_dominates_input`). -/
theorem pwa_output_dominates_input
    (α : PwaArrivalCurve) (β : ServiceCurve)
    (h_stable : ∀ s r, (s, r) ∈ α.buckets → r ≤ β.rate_bps) :
    ∀ t : Nat, α.at t ≤ (pwaOutputClosedForm α β).at t := by
  intro _t
  -- TODO(v1.0.0): discharge.  Each per-bucket inflation pointwise
  -- dominates the original bucket (σ' ≥ σ, same ρ), so the min over
  -- inflated readouts is ≥ the min over original readouts.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 7 — Residual service (conservative single-bucket lower)

    A piecewise competing α can lower to a single rate-latency
    residual service curve via the **conservative** rule
    `R' = R - max ρ_i, T' = T + max_i (σ_i / R')`.  This is sound
    but loose; tightening to a piecewise residual service curve is
    an extension we do not state here. -/

/-- Maximum bucket rate of a piecewise curve.  Required to be
    less than `β.rate_bps` for the residual to be servable. -/
def PwaArrivalCurve.maxRho (α : PwaArrivalCurve) : Nat :=
  (α.buckets.map (fun (_, r) => r)).maximum?.getD 0

/-- Maximum bucket burst of a piecewise curve. -/
def PwaArrivalCurve.maxSigma (α : PwaArrivalCurve) : Nat :=
  (α.buckets.map (fun (s, _) => s)).maximum?.getD 0

/-- Closed-form conservative single-bucket residual.  Mirrors the
    Rust `piecewise::residual_service`. -/
def pwaResidualClosedForm (α : PwaArrivalCurve) (β : ServiceCurve) :
    Option ServiceCurve :=
  let maxRho := α.maxRho
  if maxRho ≥ β.rate_bps then none
  else
    let resRate := β.rate_bps - maxRho
    -- `max_i σ_i / resRate` rounded up; we use the same div_ceil
    -- shape as the single-bucket `delayClosedForm`.
    let resLatency :=
      β.latency_ps + (α.maxSigma * scale + resRate - 1) / resRate
    some { rate_bps := resRate, latency_ps := resLatency }

/-- **Theorem 7 — Piecewise residual service (conservative lower).**
    For piecewise α with `max ρ_i < R`, the closed-form residual
    `pwaResidualClosedForm` is a valid (sound but possibly loose)
    rate-latency lower bound on the actual piecewise residual. -/
theorem pwa_residual_service_classical
    (α : PwaArrivalCurve) (β : ServiceCurve)
    (h_servable : α.maxRho < β.rate_bps) :
    ∃ β' : ServiceCurve,
        pwaResidualClosedForm α β = some β'
      ∧ β'.rate_bps = β.rate_bps - α.maxRho := by
  -- The closed form by definition picks `R - maxRho` as the residual
  -- rate; the soundness statement (β'(t) ≤ β(t) - α(t)) is the
  -- v1.0.0 follow-up.
  -- TODO(v1.0.0): discharge.  Construct the witness from the
  -- definition of `pwaResidualClosedForm`; the rate equality is by
  -- inspection.  The full soundness statement is the deferred
  -- piece.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 8 — Single-bucket embedding

    The single-bucket `ArrivalCurve` (no peak cap) embeds into the
    piecewise form via a one-bucket list, and the readouts agree
    pointwise.  This is the v0.9.3 `From<ArrivalCurve>` round-trip
    contract from the Rust side, lifted to the Lean spec. -/

/-- Embed a single-bucket affine curve (no peak cap) as a one-bucket
    piecewise curve.  Mirrors `From<ArrivalCurve>` for the
    no-peak case. -/
def PwaArrivalCurve.ofAffine (α : ArrivalCurve)
    (_h_no_peak : α.peak_rate_bps = none) : PwaArrivalCurve where
  buckets := [(α.burst_bytes, α.sustained_rate_bps)]
  buckets_nonempty := by simp

/-- **Theorem 8 — Single-bucket embedding agrees on readouts.**
    Embedding a no-peak affine curve into the piecewise form
    preserves α(t) at every t. -/
theorem pwa_of_affine_at_eq
    (α : ArrivalCurve) (h_no_peak : α.peak_rate_bps = none) (t : Nat) :
    (PwaArrivalCurve.ofAffine α h_no_peak).at t = α.at t := by
  -- TODO(v1.0.0): discharge.  Both sides are the same expression
  -- after unfolding the singleton-list min and the affine-only
  -- branch of `ArrivalCurve.at`.
  sorry -- TODO(v1.0.0)

end Spar.Network.MinPlusPwa
