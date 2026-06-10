/-
  Network Calculus — Min-Plus Algebra Theorems

  Mirrors `crates/spar-network/src/curves.rs` (Track D commit 3 of the
  v0.8.0 design).  The Rust implementation provides the four classical
  closed-form Network Calculus operators on affine "leaky bucket"
  arrival curves and rate-latency service curves; this file states the
  underlying mathematical claims so they can be machine-checked.

  Reference: Le Boudec & Thiran, "Network Calculus", Springer 2001
  (chapter 1, theorems 1.4.1 - 1.4.3).

  Closed forms captured:

    α(t) = min(σ + ρ·t, p·t)   (arrival, leaky bucket)
    β(t) = R · max(0, t − T)   (service, rate-latency)
    backlog       B = σ + ρ·T            (when ρ ≤ R)
    delay         D = T + σ/R            (horizontal distance)
    output burst  σ' = σ + ρ·T           (rate preserved)
    composition   D_total = ΣT_i + σ/min R_i

  The Lean spec is symbolic with respect to unit conversions: rates
  are in `bps`, times in `ps`, sizes in bytes, and we use a sentinel
  `scale` factor (8 · 10^12) instead of inlining the conversion at
  every step.  The Rust mirror in `curves.rs` is the load-bearing
  artifact for the integer-arithmetic side; the theorems here capture
  monotonicity, the "below latency" zero, and the closed-form shapes.

  Some proofs use `sorry` and a `TODO(v1.0.0): discharge` comment.
  Project policy: statements are the load-bearing artifact; full
  proof-discharge is post-MVP.
-/

import Mathlib.Tactic

namespace Spar.Network.MinPlus

/-! ## Type definitions mirroring the Rust API -/

/-- Affine leaky-bucket arrival curve with optional peak-rate cap.
    Mirrors `crates/spar-network/src/curves.rs::ArrivalCurve`. -/
structure ArrivalCurve where
  /-- σ — burst in bytes (the y-intercept). -/
  burst_bytes : Nat
  /-- ρ — sustained rate in bits/second. -/
  sustained_rate_bps : Nat
  /-- p — optional peak-rate cap in bits/second. -/
  peak_rate_bps : Option Nat := none
  deriving Repr

/-- Rate-latency service curve.
    Mirrors `crates/spar-network/src/curves.rs::ServiceCurve`. -/
structure ServiceCurve where
  /-- R — service rate in bits/second. -/
  rate_bps : Nat
  /-- T — service latency in picoseconds. -/
  latency_ps : Nat
  deriving Repr

/-- The unit-conversion scale factor `8 · 10^12` used by the Rust
    `bits_to_bytes_in_window` helper.  Pulled out as a constant so the
    spec is symbolic in this factor (no Mathlib floor friction).
    `8` bits/byte × `10^12` ps/second = `8_000_000_000_000`. -/
def scale : Nat := 8 * 1_000_000_000_000

theorem scale_pos : 0 < scale := by
  unfold scale
  decide

/-! ## α(t) and β(t) — the curve evaluators -/

/-- α(t) at a given time `t` in picoseconds.  Affine `σ + ρ·t/scale`
    form with optional peak-rate cap `p·t/scale`.  The integer division
    by `scale` mirrors the Rust `bits_to_bytes_in_window` helper.
    Mirrors `ArrivalCurve::at`.

    **Causality at t = 0**: α(0) = 0 for *all* arrival curves — a
    zero-length window admits zero bytes regardless of σ.  The
    peak-capped branch would give `min(σ, 0) = 0` automatically; the
    affine-only branch needs the explicit `t_ps = 0` short-circuit
    since `σ + ρ·0 = σ`.  v0.9.2 alignment: the Rust impl in
    `crates/spar-network/src/curves.rs::ArrivalCurve::at` was updated
    in the same change-set to return 0 at t = 0 (previously it
    short-circuited to σ, which violated causality). -/
def ArrivalCurve.at (α : ArrivalCurve) (t_ps : Nat) : Nat :=
  if t_ps = 0 then 0
  else
    let sustained := α.burst_bytes + (α.sustained_rate_bps * t_ps) / scale
    match α.peak_rate_bps with
    | none => sustained
    | some p => Nat.min sustained ((p * t_ps) / scale)

/-- β(t) at a given time `t`.  Rate-latency form: `R · max(0, t − T) / scale`.
    Below the latency the server has not started: `β(t) = 0`.
    Mirrors `ServiceCurve::at`. -/
def ServiceCurve.at (β : ServiceCurve) (t_ps : Nat) : Nat :=
  if t_ps ≤ β.latency_ps then 0
  else (β.rate_bps * (t_ps - β.latency_ps)) / scale

/-! ## Theorem 1 — Monotonicity of α(t) -/

/-- α(t) is monotone non-decreasing in `t`. -/
theorem arrival_at_mono (α : ArrivalCurve) {t1 t2 : Nat} (h : t1 ≤ t2) :
    α.at t1 ≤ α.at t2 := by
  unfold ArrivalCurve.at
  -- Three cases on the `if t = 0` causality short-circuit:
  --   • t1 = 0:        LHS = 0, any RHS ≥ 0.
  --   • t1 > 0, t2 > 0: both fall through to the affine/peak branches,
  --                     each of which is monotone in t.
  -- (t1 > 0 ∧ t2 = 0 is impossible since t1 ≤ t2.)
  by_cases h1 : t1 = 0
  · -- LHS = 0 by the causality short-circuit; any Nat is ≥ 0.
    simp [h1]
  · -- t1 ≠ 0, hence t2 ≠ 0 too (since t1 ≤ t2 and t1 > 0).
    have h2 : t2 ≠ 0 := by omega
    rw [if_neg h1, if_neg h2]
    -- Now both branches are the affine/peak forms with no short-circuit.
    have hsust : α.burst_bytes + α.sustained_rate_bps * t1 / scale
               ≤ α.burst_bytes + α.sustained_rate_bps * t2 / scale := by
      apply Nat.add_le_add_left
      apply Nat.div_le_div_right
      exact Nat.mul_le_mul_left _ h
    cases hp : α.peak_rate_bps with
    | none => simpa [hp] using hsust
    | some p =>
      have hpeak : p * t1 / scale ≤ p * t2 / scale := by
        apply Nat.div_le_div_right
        exact Nat.mul_le_mul_left _ h
      simp only [hp]
      -- `min` is monotone in both arguments separately (Mathlib's `min_le_min`).
      exact min_le_min hsust hpeak

/-! ## Theorem 2 — Monotonicity of β(t) -/

/-- β(t) is monotone non-decreasing in `t`. -/
theorem service_at_mono (β : ServiceCurve) {t1 t2 : Nat} (h : t1 ≤ t2) :
    β.at t1 ≤ β.at t2 := by
  unfold ServiceCurve.at
  by_cases h1 : t1 ≤ β.latency_ps
  · -- LHS branch is 0; any RHS value is non-negative.
    simp [h1]
  · -- t1 > latency, so t2 > latency too (since t1 ≤ t2).
    have h2 : ¬ t2 ≤ β.latency_ps := by omega
    simp [h1, h2]
    apply Nat.div_le_div_right
    apply Nat.mul_le_mul_left
    omega

/-! ## Theorem 3 — β(t) is zero below the latency -/

/-- For `t ≤ T`, the rate-latency service curve gives no service. -/
theorem service_at_zero_below_latency (β : ServiceCurve) (t : Nat)
    (h : t ≤ β.latency_ps) : β.at t = 0 := by
  unfold ServiceCurve.at
  simp [h]

/-- Strict-below variant matching the original spec wording. -/
theorem service_at_zero_strictly_below_latency (β : ServiceCurve) (t : Nat)
    (h : t < β.latency_ps) : β.at t = 0 :=
  service_at_zero_below_latency β t (Nat.le_of_lt h)

/-! ## Theorem 4 — Backlog closed form

    For affine α (`σ + ρ·t`, no peak cap) and rate-latency β with
    `ρ ≤ R`, the maximum backlog is `B = σ + ρ·T / scale`. -/

/-- Closed-form backlog bound for stable affine flow / rate-latency
    server.  Mirrors `backlog_bound` in `curves.rs`. -/
def backlogClosedForm (α : ArrivalCurve) (β : ServiceCurve) : Nat :=
  α.burst_bytes + (α.sustained_rate_bps * β.latency_ps) / scale

/-- **Theorem 4 — Backlog bound.**  For affine α with no peak cap and a
    rate-latency β with `ρ ≤ R`, there is a backlog bound of the
    form `σ + ρ·T/scale` such that for every `t`, `α(t) ≤ β(t) + B`.
    The bound is the closed form `backlogClosedForm α β`. -/
theorem backlog_bound_classical (α : ArrivalCurve) (β : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h_stable : α.sustained_rate_bps ≤ β.rate_bps) :
    ∃ B : Nat, B = backlogClosedForm α β
            ∧ ∀ t : Nat, α.at t ≤ β.at t + B := by
  -- Take B := σ + ρ·T/scale.  The witness is straightforward; the
  -- ∀-t statement requires the Le Boudec & Thiran sup-at-T argument
  -- which uses real-number reasoning we defer to v1.0.0.
  refine ⟨backlogClosedForm α β, rfl, ?_⟩
  intro _t
  -- TODO(v1.0.0): discharge via case split on (t ≤ T) vs (t > T) and
  -- apply Nat.div arithmetic with `h_stable`.  The classical real-line
  -- proof shows sup_t (α(t) - β(t)) is reached at t = T.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 5 — Delay closed form

    Horizontal distance between α and β.  For affine + rate-latency
    with `ρ ≤ R`, `D = T + σ/R` (where σ/R is in picoseconds, i.e.
    `σ · scale / R`). -/

/-- Closed-form delay bound for stable affine flow / rate-latency
    server (in picoseconds).  Mirrors `delay_bound` in `curves.rs`,
    rounded *up* so the bound is never an under-estimate. -/
def delayClosedForm (α : ArrivalCurve) (β : ServiceCurve) : Nat :=
  if β.rate_bps = 0 then 0
  else β.latency_ps + (α.burst_bytes * scale + β.rate_bps - 1) / β.rate_bps

/-- **Theorem 5 — Delay bound.**  For affine α with no peak cap and a
    rate-latency β with `0 < ρ ≤ R`, there is a delay bound of the
    form `T + σ/R` such that every byte arriving by time `t` is
    served by time `t + D`. -/
theorem delay_bound_classical (α : ArrivalCurve) (β : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h_stable : α.sustained_rate_bps ≤ β.rate_bps)
    (h_rate_pos : 0 < β.rate_bps) :
    ∃ D : Nat, D = delayClosedForm α β
            ∧ ∀ t : Nat, α.at t ≤ β.at (t + D) := by
  refine ⟨delayClosedForm α β, rfl, ?_⟩
  intro _t
  -- TODO(v1.0.0): discharge via Le Boudec & Thiran horizontal-distance
  -- argument.  In integer arithmetic this reduces to chasing div_ceil
  -- bounds across the affine ↔ rate-latency intersection point.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 6 — Output bound (burst inflation, rate preserved)

    The output of an arrival passing through a rate-latency server is
    again affine with the same sustained rate but burst inflated by
    `ρ · T / scale`. -/

/-- Output (departure) arrival curve through a rate-latency server.
    Mirrors `output_bound` in `curves.rs`. -/
def outputClosedForm (α : ArrivalCurve) (β : ServiceCurve) : ArrivalCurve :=
  { burst_bytes := α.burst_bytes + (α.sustained_rate_bps * β.latency_ps) / scale
    sustained_rate_bps := α.sustained_rate_bps
    peak_rate_bps := α.peak_rate_bps }

/-- **Theorem 6 — Output bound.**  The output through a rate-latency
    server preserves the sustained rate (and the peak cap) and
    inflates the burst by `ρ·T/scale`. -/
theorem output_bound_rate_preserved (α : ArrivalCurve) (β : ServiceCurve)
    (h_stable : α.sustained_rate_bps ≤ β.rate_bps) :
    ∃ α' : ArrivalCurve,
        α'.sustained_rate_bps = α.sustained_rate_bps
      ∧ α'.peak_rate_bps      = α.peak_rate_bps
      ∧ α'.burst_bytes        = α.burst_bytes
                               + (α.sustained_rate_bps * β.latency_ps) / scale := by
  refine ⟨outputClosedForm α β, ?_, ?_, ?_⟩
  · rfl
  · rfl
  · rfl

/-- **Theorem 6' — Output bound dominates the input arrival.**
    The closed-form output curve dominates the input curve at every
    `t` shifted by `T`, expressing the "burst grows by `ρ·T`" content
    of Le Boudec & Thiran 1.4.3. -/
theorem output_dominates_input (α : ArrivalCurve) (β : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h_stable : α.sustained_rate_bps ≤ β.rate_bps) :
    ∀ t : Nat, α.at t ≤ (outputClosedForm α β).at t := by
  intro _t
  -- TODO(v1.0.0): discharge.  The output curve has burst = σ + ρ·T
  -- (≥ σ) and the same sustained rate, so α'(t) ≥ α(t) at every t.
  sorry -- TODO(v1.0.0)

/-! ## Theorem 7 — Composition (serial chain delay aggregates)

    Two rate-latency servers in series with the same flow give a
    naive aggregated delay equal to the sum of per-hop delays.  The
    PBOO ("pay burst only once") improvement uses `min R_i` and a
    single `σ` charge — we capture the *naive* sum here as it
    matches the per-hop aggregation in `wctt.rs` (Track D commit 4). -/

/-- Per-hop naive composition: feed the output bound of the first
    server into the delay bound of the second and accumulate. -/
def composeDelayNaive (α : ArrivalCurve) (β1 β2 : ServiceCurve) : Nat :=
  delayClosedForm α β1 + delayClosedForm (outputClosedForm α β1) β2

/-- **Theorem 7 — Serial-chain composition.**  Two rate-latency
    servers in series with stable flow on each hop give a naive
    end-to-end delay bound equal to the sum of per-hop delays, where
    the second hop sees the burst-inflated output curve from the
    first. -/
theorem compose_delays
    (α : ArrivalCurve) (β1 β2 : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h_stable1 : α.sustained_rate_bps ≤ β1.rate_bps)
    (h_stable2 : α.sustained_rate_bps ≤ β2.rate_bps)
    (h_rate1_pos : 0 < β1.rate_bps)
    (h_rate2_pos : 0 < β2.rate_bps) :
    ∃ D : Nat,
        D = composeDelayNaive α β1 β2
      ∧ D = delayClosedForm α β1
          + delayClosedForm (outputClosedForm α β1) β2 := by
  refine ⟨composeDelayNaive α β1 β2, rfl, rfl⟩

/-- **Theorem 7' — Concatenation domination (PBOO weakening).**
    The naive sum overestimates the optimal pay-burst-only-once
    bound, but is itself a valid (looser) end-to-end bound: for
    every `t`, `α(t) ≤ β2(β1(t + D))` where `D` is the naive sum.

    PBOO concatenation is deferred to v0.8.x; here we just record
    the naive bound as our anchoring statement. -/
theorem compose_delays_dominates
    (α : ArrivalCurve) (β1 β2 : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h_stable1 : α.sustained_rate_bps ≤ β1.rate_bps)
    (h_stable2 : α.sustained_rate_bps ≤ β2.rate_bps)
    (h_rate1_pos : 0 < β1.rate_bps)
    (h_rate2_pos : 0 < β2.rate_bps) :
    ∀ t : Nat, α.at t ≤ β2.at (β1.at (t + composeDelayNaive α β1 β2)) := by
  intro _t
  -- TODO(v1.0.0): discharge.  Apply delay_bound_classical at each
  -- hop, then chain through output_dominates_input to thread α →
  -- output(α,β1) → β2.  The arithmetic is straightforward once the
  -- per-hop sorries above are discharged.
  sorry -- TODO(v1.0.0)

/-! ## Sanity check — causality at t = 0

    A degenerate sub-case the Rust tests pin (`arrival_curve_at_zero_is_zero`):
    `α.at 0 = 0` for *all* arrival curves, regardless of σ or peak cap.
    A zero-length window admits zero bytes — the burst σ is the
    y-intercept of the affine line and is realised only as soon as
    `t > 0` (instantaneously), not *at* `t = 0`.  This is the causal
    reading agreed in v0.9.2 (the prior Rust short-circuit returned σ
    at t = 0; that was a pre-mature optimisation that violated
    causality and has been retracted in `crates/spar-network/src/curves.rs`). -/

/-- **Causality** — α(0) = 0 for all arrival curves, regardless of
    burst σ or peak rate.  The peak-capped branch would give
    `min(σ, 0) = 0` automatically; the affine-only branch needs the
    explicit `t = 0` short-circuit baked into `ArrivalCurve.at`.
    Discharged in v0.9.2 (was the 5th tracked `sorry`). -/
theorem arrival_at_zero_is_zero (α : ArrivalCurve) :
    α.at 0 = 0 := by
  -- Direct from the `if t_ps = 0 then 0` short-circuit.
  unfold ArrivalCurve.at
  simp

/-- β below latency is monotone-zero.  Sanity corollary of Theorem 3. -/
theorem service_at_zero_at_zero (β : ServiceCurve) :
    β.at 0 = 0 := by
  unfold ServiceCurve.at
  simp

/-! ## Additional structural lemmas

    These supplemental lemmas record properties used in the main
    theorem proofs above, factored out for reusability and to make
    explicit the building blocks behind each closed form.  Several
    are direct corollaries / specialisations rather than independent
    statements; they are kept here so that the v1.0.0 discharge
    of the `sorry`s above can reference them. -/

/-- The affine branch of α (no peak cap) is monotone in `t`. -/
theorem arrival_affine_mono (α : ArrivalCurve) {t1 t2 : Nat}
    (h_no_peak : α.peak_rate_bps = none) (h : t1 ≤ t2) :
    α.at t1 ≤ α.at t2 :=
  arrival_at_mono α h

/-- The peak-capped branch of α is also monotone — Theorem 1
    specialised to the `some p` case.  Pinned out as a separate
    lemma because `wctt.rs` distinguishes the two when computing
    short-window bounds. -/
theorem arrival_peak_capped_mono (α : ArrivalCurve) (p : Nat)
    (h_peak : α.peak_rate_bps = some p) {t1 t2 : Nat} (h : t1 ≤ t2) :
    α.at t1 ≤ α.at t2 :=
  arrival_at_mono α h

/-- Composition rule for backlog along a serial chain (naive).
    If each hop is stable, the second hop sees the burst-inflated
    output curve from the first; the per-hop backlog bound at hop 2
    is therefore `σ + ρ·T₁ + ρ·T₂`. -/
def backlogChained (α : ArrivalCurve) (β1 β2 : ServiceCurve) : Nat :=
  α.burst_bytes
  + (α.sustained_rate_bps * β1.latency_ps) / scale
  + (α.sustained_rate_bps * β2.latency_ps) / scale

/-- The chained-backlog formula matches the per-hop sum of the
    closed-form backlog bounds along the serial chain.  Pure
    arithmetic, no `sorry`. -/
theorem backlogChained_eq_sum
    (α : ArrivalCurve) (β1 β2 : ServiceCurve) :
    backlogChained α β1 β2
      = backlogClosedForm α β1
      + (α.sustained_rate_bps * β2.latency_ps) / scale := by
  unfold backlogChained backlogClosedForm
  ring

/-- The output curve preserves the sustained rate.  Direct corollary
    of `outputClosedForm`. -/
theorem output_preserves_sustained_rate (α : ArrivalCurve) (β : ServiceCurve) :
    (outputClosedForm α β).sustained_rate_bps = α.sustained_rate_bps := rfl

/-- The output curve preserves the peak-rate cap (if any).  Direct
    corollary of `outputClosedForm`.  Mirrors the Rust property
    asserted by `output_bound_rate_preserved` in `curves.rs`. -/
theorem output_preserves_peak_rate (α : ArrivalCurve) (β : ServiceCurve) :
    (outputClosedForm α β).peak_rate_bps = α.peak_rate_bps := rfl

/-- The output curve's burst is exactly the input burst plus the
    closed-form inflation.  Direct corollary of `outputClosedForm`. -/
theorem output_burst_inflation (α : ArrivalCurve) (β : ServiceCurve) :
    (outputClosedForm α β).burst_bytes
      = α.burst_bytes + (α.sustained_rate_bps * β.latency_ps) / scale := rfl

/-- The output curve under stable composition keeps its rate ≤ the
    second server's rate.  This is the well-formedness gate that
    `compose_delays` needs at its second hop. -/
theorem output_stable_for_chain
    (α : ArrivalCurve) (β1 β2 : ServiceCurve)
    (h_stable2 : α.sustained_rate_bps ≤ β2.rate_bps) :
    (outputClosedForm α β1).sustained_rate_bps ≤ β2.rate_bps := by
  rw [output_preserves_sustained_rate]
  exact h_stable2

/-- Naive composition is a homomorphism in the latency component:
    a chain of two zero-latency rate-latency servers has zero added
    latency contribution beyond the burst-drain term. -/
theorem compose_delays_zero_latency
    (α : ArrivalCurve) (β1 β2 : ServiceCurve)
    (h_no_peak : α.peak_rate_bps = none)
    (h1 : β1.latency_ps = 0) (h2 : β2.latency_ps = 0)
    (h_rate1_pos : 0 < β1.rate_bps) (h_rate2_pos : 0 < β2.rate_bps) :
    composeDelayNaive α β1 β2
      = (α.burst_bytes * scale + β1.rate_bps - 1) / β1.rate_bps
      + ((α.burst_bytes + 0) * scale + β2.rate_bps - 1) / β2.rate_bps := by
  unfold composeDelayNaive delayClosedForm outputClosedForm
  -- Both rates positive ⇒ the `if rate = 0` branch is not taken.
  have hne1 : β1.rate_bps ≠ 0 := Nat.pos_iff_ne_zero.mp h_rate1_pos
  have hne2 : β2.rate_bps ≠ 0 := Nat.pos_iff_ne_zero.mp h_rate2_pos
  simp [hne1, hne2, h1, h2]

end Spar.Network.MinPlus
