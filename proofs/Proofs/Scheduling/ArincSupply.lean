/-
  ARINC-653 Partition-Supply-Derived Inner RTA — Lean floor
  (REQ-RTA-ARINC-SUPPLY-001, issue #331)

  Two independent obligations for the supply-derived analysis in
  `crates/spar-analysis/src/arinc_supply.rs`:

  (1) **Constant non-preemptive blocking folds into the recurrence.**
      The cooperative blocking term `B_i` is a pure additive constant, so
      `rtaStepJitteredBlocking task hps isr B r
         = C_i + (J_i + B) + Σ_j ⌈(r+J_j)/T_j⌉·C_j + ISR(r)` — i.e. `B`
      folds into the release jitter. Monotonicity and least-fixed-point
      convergence therefore transfer from `RTAJittered.lean` by the SAME
      `bounded_mono_nat_seq` argument (mirrored here for the blocking
      step). This lifts, for the cooperative case, the no-blocking
      assumption `rta.rs` documented. Mirrors
      `compute_response_time_jittered_blocking`.

  (2) **The linear supply lower bound is monotone, and passing the
      demand-vs-supply test is sound UNDER an explicit supply guarantee.**
      `lsbf_Γ(t) = ⌊Θ·(t − 2(Π−Θ))/Π⌋` (0 below the blackout) is proven
      monotone. That the OS actually delivers supply ≥ `lsbf_Γ` is the
      partition scheduler's obligation — carried here as an explicit
      HYPOTHESIS (`SupplyGuarantee`), NOT proven by spar. What IS proven:
      given that hypothesis, passing the linear test `rbf(t) ≤ lsbf_Γ(t)`
      implies real demand never exceeds real supply at `t`. The content is
      the honest PROVEN-vs-ASSUMED separation (the #294 lesson).

  0 sorry / 0 admit / 0 axiom.
-/

import Mathlib.Tactic
import Proofs.Scheduling.RTA
import Proofs.Scheduling.RTAJittered

namespace Spar.Scheduling.ArincSupply

open Spar.Scheduling.RTA (iterN bounded_mono_nat_seq)
open Spar.Scheduling.RTAJittered

/-! ## Part 1 — Constant blocking folds into the jittered recurrence -/

/-- The blocking-augmented step: the jittered recurrence plus a constant
    blocking term `B`. Mirrors `rta_step_jittered_blocking` in
    `scheduling_verified.rs`. -/
def rtaStepJitteredBlocking
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (blocking : Nat)
    (r : Nat) : Nat :=
  rtaStepJittered task hps isr r + blocking

/-- **Fold-in.** The blocking step is exactly the jittered step with the
    blocking constant absorbed into the release jitter: `B` and `J_i` are
    both pure additive constants. This is why the existing `RTAJittered`
    theorems cover the blocking case. -/
theorem rtaStepJitteredBlocking_fold
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (blocking r : Nat) :
    rtaStepJitteredBlocking task hps isr blocking r
      = task.exec + (task.jitter + blocking)
        + RTAJittered.totalInterferenceJittered hps r + isr r := by
  unfold rtaStepJitteredBlocking rtaStepJittered
  omega

/-- The blocking step is monotone in `r` (adding a constant preserves the
    monotonicity of the jittered step). -/
theorem rtaStepJitteredBlocking_mono
    {task : JitteredTask}
    {hps : List JitteredHigherPriorityTask}
    {isr : IsrOverhead}
    (hisr : IsrOverhead.Monotone isr)
    {blocking r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    rtaStepJitteredBlocking task hps isr blocking r₁
      ≤ rtaStepJitteredBlocking task hps isr blocking r₂ := by
  unfold rtaStepJitteredBlocking
  exact Nat.add_le_add_right (rtaStep_jittered_mono hisr h) blocking

/-- The blocking step at the initial value `C_i + J_i + B` is at least
    `C_i + J_i + B`. -/
theorem rtaStepJitteredBlocking_ge_initial
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (blocking : Nat) :
    rtaStepJitteredBlocking task hps isr blocking (task.exec + task.jitter + blocking)
      ≥ task.exec + task.jitter + blocking := by
  unfold rtaStepJitteredBlocking rtaStepJittered
  omega

/-- **Convergence with blocking.** Iterating `rtaStepJitteredBlocking`
    from `C_i + J_i + B` either reaches a fixed point within
    `deadline + 1` steps or exceeds the deadline — the same termination
    guarantee as the un-blocked case, justifying the bounded loop in
    `compute_response_time_jittered_blocking`. -/
theorem rtaBlocking_finds_least_fixed_point
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (blocking : Nat)
    (hisr : IsrOverhead.Monotone isr) :
    ∃ n : Nat, n ≤ task.deadline + 1 ∧
      (rtaStepJitteredBlocking task hps isr blocking
          (iterN (rtaStepJitteredBlocking task hps isr blocking) n
            (task.exec + task.jitter + blocking))
        = iterN (rtaStepJitteredBlocking task hps isr blocking) n
            (task.exec + task.jitter + blocking)
       ∨ iterN (rtaStepJitteredBlocking task hps isr blocking) n
            (task.exec + task.jitter + blocking) > task.deadline) := by
  have hmono : ∀ a b, a ≤ b →
      rtaStepJitteredBlocking task hps isr blocking a
        ≤ rtaStepJitteredBlocking task hps isr blocking b :=
    fun _ _ h => rtaStepJitteredBlocking_mono hisr h
  have hexp := rtaStepJitteredBlocking_ge_initial task hps isr blocking
  obtain ⟨n, hn, hor⟩ := bounded_mono_nat_seq hmono hexp (B := task.deadline)
  refine ⟨n, hn, ?_⟩
  rcases hor with heq | hgt
  · left
    have hstep : iterN (rtaStepJitteredBlocking task hps isr blocking) (n + 1)
          (task.exec + task.jitter + blocking) =
        rtaStepJitteredBlocking task hps isr blocking
          (iterN (rtaStepJitteredBlocking task hps isr blocking) n
            (task.exec + task.jitter + blocking)) := rfl
    linarith
  · exact Or.inr hgt

/-! ## Part 2 — Linear supply lower bound: monotone + conditional soundness -/

/-- Worst-case contiguous blackout of the periodic resource `Γ = (Π, Θ)`:
    `2(Π − Θ)`. Mirrors `derived_jitter` in `arinc_supply.rs`. -/
def blackout (Pi Theta : Nat) : Nat := 2 * (Pi - Theta)

/-- Linear supply lower bound `lsbf_Γ(t) = ⌊Θ·(t − 2(Π−Θ))/Π⌋`, `0` below
    the blackout. Integer floor (conservative). Mirrors `lsbf` in
    `arinc_supply.rs`. -/
def lsbf (Pi Theta t : Nat) : Nat :=
  if t ≤ blackout Pi Theta then 0 else Theta * (t - blackout Pi Theta) / Pi

/-- `lsbf` is zero up to and including the blackout. -/
theorem lsbf_zero_below_blackout (Pi Theta t : Nat) (h : t ≤ blackout Pi Theta) :
    lsbf Pi Theta t = 0 := by
  unfold lsbf
  rw [if_pos h]

/-- **`lsbf` is monotone in `t`** — more elapsed time never lowers the
    guaranteed supply. -/
theorem lsbf_mono (Pi Theta : Nat) {t₁ t₂ : Nat} (h : t₁ ≤ t₂) :
    lsbf Pi Theta t₁ ≤ lsbf Pi Theta t₂ := by
  unfold lsbf
  by_cases h1 : t₁ ≤ blackout Pi Theta
  · rw [if_pos h1]
    exact Nat.zero_le _
  · have h2 : ¬ t₂ ≤ blackout Pi Theta := by omega
    rw [if_neg h1, if_neg h2]
    gcongr
    omega

/-- The ASSUMED supply contract: the partition scheduler delivers at least
    the linear supply lower bound in every interval of length `t`. This is
    the OS's (gust's) obligation and a HYPOTHESIS of the analysis — spar
    does NOT prove it. -/
def SupplyGuarantee (sbf : Nat → Nat) (Pi Theta : Nat) : Prop :=
  ∀ t, lsbf Pi Theta t ≤ sbf t

/-- **Fallback soundness (conditional).** IF the resource honours the
    supply guarantee, THEN passing the linear demand-vs-supply test
    `rbf(t) ≤ lsbf_Γ(t)` implies the real demand never exceeds the real
    supply at `t` — the inner task is schedulable at that `t`.

    The proof is pure transitivity; its VALUE is the explicit separation
    of what is PROVEN (this implication + `lsbf` monotonicity) from what
    is ASSUMED (`SupplyGuarantee` — the OS contract). Emitting a bound
    without stating the assumption would over-claim (the #294 lesson). -/
theorem fallback_sound_of_supply_guarantee
    {sbf : Nat → Nat} {Pi Theta : Nat}
    (hsupply : SupplyGuarantee sbf Pi Theta)
    {rbf_t t : Nat}
    (htest : rbf_t ≤ lsbf Pi Theta t) :
    rbf_t ≤ sbf t :=
  le_trans htest (hsupply t)

end Spar.Scheduling.ArincSupply
