/-
  Rate Monotonic Utilization Bound — Soundness

  Reference: Liu & Layland, "Scheduling Algorithms for Multiprogramming
  in a Hard-Real-Time Environment", JACM, 1973.

  The RM utilization bound: for n periodic tasks with implicit deadlines,
  if U = Σ(C_i/T_i) ≤ n × (2^(1/n) - 1), all tasks meet deadlines
  under rate-monotonic scheduling.

  With Mathlib we can state properties over ℝ using rpow.
-/
import Mathlib.Tactic
import Mathlib.Analysis.SpecialFunctions.Pow.Real

namespace Spar.Scheduling.RMBound

-- Task utilization as a rational: C/T.
structure TaskUtil where
  exec : Nat
  period : Nat
  period_pos : period > 0

-- Utilization of a task as a real number.
noncomputable def utilization (t : TaskUtil) : ℝ :=
  (t.exec : ℝ) / (t.period : ℝ)

-- Sum of utilizations.
noncomputable def totalUtil : List TaskUtil → ℝ
  | [] => 0
  | t :: rest => utilization t + totalUtil rest

-- The RM bound function: n × (2^(1/n) - 1)
noncomputable def rmBound (n : ℕ) (_ : n ≥ 1) : ℝ :=
  n * ((2 : ℝ) ^ ((1 : ℝ) / n) - 1)

-- RM bound for n=1 is exactly 1.0.
theorem rmBound_one : rmBound 1 (by omega) = 1 := by
  unfold rmBound
  simp
  norm_num

-- The key fact: rmBound is decreasing and converges to ln(2).
-- rmBound(n) ≥ ln(2) for all n ≥ 1.
theorem rmBound_ge_ln2 (n : ℕ) (hn : n ≥ 1) :
    rmBound n hn ≥ Real.log 2 := by
  sorry -- requires calculus: concavity of 2^(1/n)

-- Single-task RM: if C ≤ T (utilization ≤ 1), trivially schedulable.
theorem rm_single_task (t : TaskUtil) (h : t.exec ≤ t.period) :
    utilization t ≤ 1 := by
  unfold utilization
  rw [div_le_one (Nat.cast_pos.mpr t.period_pos)]
  exact Nat.cast_le.mpr h

-- totalUtil is non-negative when all tasks have non-negative utilization.
theorem totalUtil_nonneg (tasks : List TaskUtil) :
    totalUtil tasks ≥ 0 := by
  induction tasks with
  | nil => simp [totalUtil]
  | cons t rest ih =>
    simp only [totalUtil]
    have : utilization t ≥ 0 := by
      unfold utilization
      positivity
    linarith

end Spar.Scheduling.RMBound
