/-! Mirrors `compute_response_time_jittered` in
    `crates/spar-analysis/src/scheduling_verified.rs` (PR #147).
    The Rust implementation is checked against this spec via property tests
    in that file's `#[cfg(test)] mod tests`. -/

/-
  Jittered Response Time Analysis (RTA) — Fixed-Point Convergence

  Reference: Tindell & Clark, "Holistic schedulability analysis for
  distributed hard real-time systems", Microprocessing & Microprogramming,
  1994.  Joseph & Pandya 1986 for the un-jittered baseline.

  We extend the RTA recurrence in `RTA.lean` with three new ingredients:
    1. The task under analysis has a release jitter `J_i` added as a
       constant offset to its response window.
    2. Each higher-priority task `j` has its own release jitter `J_j`
       which inflates the ceiling count: ⌈(R + J_j) / T_j⌉ × C_j.
    3. Periodic ISR overhead enters as an extra monotone term
       `IsrOverhead : Nat → Nat`.

  Recurrence:
    R(0)   = C_i + J_i
    R(n+1) = C_i + J_i
           + Σ_j ⌈(R(n) + J_j) / T_j⌉ × C_j
           + IsrOverhead(R(n))

  When all `J_j = 0`, `J_i = 0`, and `IsrOverhead = fun _ => 0`, this
  reduces to `Spar.Scheduling.RTA.rtaStep` modulo packaging.

  This file states the theorems anchoring the Rust implementation in
  `compute_response_time_jittered`.  Convergence is established under
  the same termination argument as the un-jittered case — monotone
  non-decreasing sequence bounded by the deadline.
-/
import Mathlib.Tactic
import Proofs.Scheduling.RTA

namespace Spar.Scheduling.RTAJittered

open Spar.Scheduling.RTA (ceilDiv ceilDiv_mono iterN iterN_mono iterN_nondecreasing
  no_fp_implies_growth bounded_mono_nat_seq)

/-! ## Type definitions mirroring the Rust API -/

/-- A higher-priority task with release jitter, mirroring the Rust tuple
    `(period, exec, jitter) : (u64, u64, u64)`. -/
structure JitteredHigherPriorityTask where
  period : Nat
  exec : Nat
  jitter : Nat
  period_pos : period > 0

/-- A task under analysis, mirroring the Rust signature
    `compute_response_time_jittered(exec, deadline, jitter, …)`. -/
structure JitteredTask where
  exec : Nat
  deadline : Nat
  jitter : Nat
  exec_pos : exec > 0
  deadline_pos : deadline > 0

/-- ISR interference as an opaque monotone function `R ↦ overhead(R)`.
    The Rust side computes this from a list of `(period, exec)` tuples
    via `total_isr_interference`; we abstract over the list because the
    convergence argument only needs monotonicity. -/
abbrev IsrOverhead := Nat → Nat

/-- Constructor matching the Rust `total_isr_interference` (which is
    just `total_interference` over `(period, exec)` pairs).  Lifts a
    list of ISR specs into an `IsrOverhead` function. -/
def isrOverheadOfList (isrs : List Spar.Scheduling.RTA.Task) : IsrOverhead :=
  fun r => Spar.Scheduling.RTA.totalInterference isrs r

/-! ## Step function — the right-hand side of the jittered recurrence -/

/-- Interference from one higher-priority task with release jitter:
    `⌈(r + J_j) / T_j⌉ × C_j`.  Mirrors `interference_jittered` in
    `scheduling_verified.rs`. -/
def interferenceJittered (hp : JitteredHigherPriorityTask) (r : Nat) : Nat :=
  ceilDiv (r + hp.jitter) hp.period hp.period_pos * hp.exec

/-- Total higher-priority interference, summed over all HP tasks.
    Mirrors `total_interference_jittered`. -/
def totalInterferenceJittered : List JitteredHigherPriorityTask → Nat → Nat
  | [], _ => 0
  | hp :: rest, r => interferenceJittered hp r + totalInterferenceJittered rest r

/-- The jittered RTA recurrence step:
      R_{n+1} = C_i + J_i + Σ_j ⌈(R_n + J_j)/T_j⌉ × C_j + ISR(R_n).
    Mirrors `rta_step_jittered` in `scheduling_verified.rs`. -/
def rtaStepJittered
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (r : Nat) : Nat :=
  task.exec + task.jitter + totalInterferenceJittered hps r + isr r

/-! ## Theorem 1 — Monotonicity -/

/-- Jittered single-task interference is monotone in `r`. -/
theorem interferenceJittered_mono
    (hp : JitteredHigherPriorityTask) {r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    interferenceJittered hp r₁ ≤ interferenceJittered hp r₂ := by
  unfold interferenceJittered
  apply Nat.mul_le_mul_right
  exact ceilDiv_mono hp.period_pos (by omega)

/-- Jittered total interference is monotone in `r`. -/
theorem totalInterferenceJittered_mono
    {hps : List JitteredHigherPriorityTask} {r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    totalInterferenceJittered hps r₁ ≤ totalInterferenceJittered hps r₂ := by
  induction hps with
  | nil => simp [totalInterferenceJittered]
  | cons hp rest ih =>
    simp only [totalInterferenceJittered]
    exact Nat.add_le_add (interferenceJittered_mono hp h) ih

/-- An `IsrOverhead` function is monotone iff it is non-decreasing in `r`. -/
def IsrOverhead.Monotone (isr : IsrOverhead) : Prop :=
  ∀ r₁ r₂, r₁ ≤ r₂ → isr r₁ ≤ isr r₂

/-- The list-based `isrOverheadOfList` is always monotone — this is the
    canonical construction the Rust side uses. -/
theorem isrOverheadOfList_mono (isrs : List Spar.Scheduling.RTA.Task) :
    IsrOverhead.Monotone (isrOverheadOfList isrs) := by
  intro r₁ r₂ h
  unfold isrOverheadOfList
  exact Spar.Scheduling.RTA.totalInterference_mono h

/-- **Theorem 1 — Monotonicity.**
    `R₁ ≤ R₂` implies `rtaStepJittered task hps isr R₁ ≤ rtaStepJittered task hps isr R₂`
    whenever `isr` is itself monotone. -/
theorem rtaStep_jittered_mono
    {task : JitteredTask}
    {hps : List JitteredHigherPriorityTask}
    {isr : IsrOverhead}
    (hisr : IsrOverhead.Monotone isr)
    {r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    rtaStepJittered task hps isr r₁ ≤ rtaStepJittered task hps isr r₂ := by
  unfold rtaStepJittered
  have hI := totalInterferenceJittered_mono (hps := hps) h
  have hO := hisr r₁ r₂ h
  -- Goal: exec + jitter + tot r₁ + isr r₁ ≤ exec + jitter + tot r₂ + isr r₂.
  -- Decompose as (exec+jitter+tot r) + isr r and apply Nat.add_le_add.
  have step1 : task.exec + task.jitter + totalInterferenceJittered hps r₁
             ≤ task.exec + task.jitter + totalInterferenceJittered hps r₂ :=
    Nat.add_le_add_left hI _
  exact Nat.add_le_add step1 hO

/-! ## Theorem 2 — Degenerate case (zero jitter recovers classical) -/

/-- Bridge: an `RTA.Task` with the same period/exec inherits its
    `period_pos` proof. We use this to translate between the two
    higher-priority shapes when the jitter is zero. -/
def hpFromClassic (t : Spar.Scheduling.RTA.Task) : JitteredHigherPriorityTask :=
  { period := t.period, exec := t.exec, jitter := 0, period_pos := t.period_pos }

/-- A `JitteredTask` recovers from an `RTA.Task` when jitter is zero.
    Used only inside Theorem 2's statement. -/
def taskFromClassic (t : Spar.Scheduling.RTA.Task) : JitteredTask :=
  { exec := t.exec, deadline := t.deadline, jitter := 0,
    exec_pos := t.exec_pos, deadline_pos := t.deadline_pos }

/-- With zero jitter, `interferenceJittered` reduces to `RTA.interference`. -/
theorem interferenceJittered_zero_jitter (t : Spar.Scheduling.RTA.Task) (r : Nat) :
    interferenceJittered (hpFromClassic t) r = Spar.Scheduling.RTA.interference t r := by
  unfold interferenceJittered Spar.Scheduling.RTA.interference hpFromClassic
  simp

/-- With zero jitter on every HP task, `totalInterferenceJittered` reduces
    to `RTA.totalInterference`. -/
theorem totalInterferenceJittered_zero_jitter (ts : List Spar.Scheduling.RTA.Task) (r : Nat) :
    totalInterferenceJittered (ts.map hpFromClassic) r =
      Spar.Scheduling.RTA.totalInterference ts r := by
  induction ts with
  | nil => simp [totalInterferenceJittered, Spar.Scheduling.RTA.totalInterference]
  | cons t rest ih =>
    simp only [List.map, totalInterferenceJittered,
      Spar.Scheduling.RTA.totalInterference]
    rw [interferenceJittered_zero_jitter, ih]

/-- **Theorem 2 — Degenerate case.**
    When the task under analysis has zero jitter, every HP task has zero
    jitter, and the ISR overhead is identically zero, `rtaStepJittered`
    coincides with `rtaStep` from `RTA.lean`.  This is the non-regression
    property anchoring the Rust-side test `no_isrs_matches_classical_rta`. -/
theorem rtaStep_jittered_zero_jitter
    (t : Spar.Scheduling.RTA.Task)
    (hps : List Spar.Scheduling.RTA.Task)
    (r : Nat) :
    rtaStepJittered (taskFromClassic t) (hps.map hpFromClassic)
        (fun _ => 0) r =
      Spar.Scheduling.RTA.rtaStep t hps r := by
  unfold rtaStepJittered Spar.Scheduling.RTA.rtaStep taskFromClassic
  rw [totalInterferenceJittered_zero_jitter]
  simp

/-! ## Theorem 3 — Convergence to least fixed point -/

/-- A value `r` is a fixed point of the jittered recurrence. -/
def isFixedPointJittered
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (r : Nat) : Prop :=
  rtaStepJittered task hps isr r = r

/-- The jittered step at the initial value `C_i + J_i` is at least
    `C_i + J_i` (interference and ISR terms are non-negative). -/
theorem rtaStepJittered_ge_initial
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead) :
    rtaStepJittered task hps isr (task.exec + task.jitter)
      ≥ task.exec + task.jitter := by
  unfold rtaStepJittered; omega

/-- **Theorem 3 — Convergence to least fixed point.**

    Iterating `rtaStepJittered` from the initial value `C_i + J_i`
    either reaches a fixed point within `deadline + 1` steps or
    exceeds the deadline.  This mirrors `rta_terminates` /
    `rta_finds_least_fixed_point` in `RTA.lean` and justifies the
    bounded loop in `compute_response_time_jittered`. -/
theorem rtaJittered_finds_least_fixed_point
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (hisr : IsrOverhead.Monotone isr) :
    ∃ n : Nat, n ≤ task.deadline + 1 ∧
      (isFixedPointJittered task hps isr
          (iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter)) ∨
       iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter)
          > task.deadline) := by
  -- Mirror the un-jittered termination proof in RTA.lean: the step is
  -- monotone (Theorem 1) and at the initial point r₀ = C_i + J_i we have
  -- step(r₀) ≥ r₀, so the iterate sequence is non-decreasing.  Then by
  -- `bounded_mono_nat_seq` (a generic Nat-sequence lemma proved in
  -- `RTA.lean`) the sequence either fixes within `deadline + 1` steps
  -- or exceeds the bound.
  have hmono : ∀ a b, a ≤ b → rtaStepJittered task hps isr a
                                ≤ rtaStepJittered task hps isr b :=
    fun _ _ h => rtaStep_jittered_mono hisr h
  have hexp := rtaStepJittered_ge_initial task hps isr
  obtain ⟨n, hn, hor⟩ := bounded_mono_nat_seq hmono hexp (B := task.deadline)
  refine ⟨n, hn, ?_⟩
  -- The local `let r := …` in the goal needs the same unfold treatment
  -- as in `RTA.rta_terminates`.
  rcases hor with heq | hgt
  · left
    show isFixedPointJittered task hps isr
        (iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter))
    unfold isFixedPointJittered
    have : iterN (rtaStepJittered task hps isr) (n + 1) (task.exec + task.jitter) =
        rtaStepJittered task hps isr
          (iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter)) := rfl
    linarith
  · exact Or.inr hgt

/-- Soundness: every iterate from the canonical start `C_i + J_i` is
    bounded above by any fixed point that itself dominates `C_i + J_i`.
    Hence iteration converges to the **least** such fixed point. -/
theorem iterN_le_fixed_point_jittered
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (hisr : IsrOverhead.Monotone isr)
    (r' : Nat)
    (hfp' : isFixedPointJittered task hps isr r')
    (hge' : r' ≥ task.exec + task.jitter)
    (n : Nat) :
    iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter) ≤ r' := by
  induction n with
  | zero => exact hge'
  | succ n ih =>
    calc iterN (rtaStepJittered task hps isr) (n + 1) (task.exec + task.jitter)
        = rtaStepJittered task hps isr
            (iterN (rtaStepJittered task hps isr) n (task.exec + task.jitter)) := rfl
      _ ≤ rtaStepJittered task hps isr r' := rtaStep_jittered_mono hisr ih
      _ = r' := hfp'

end Spar.Scheduling.RTAJittered
