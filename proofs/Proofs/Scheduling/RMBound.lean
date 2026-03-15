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
--
-- Proof: By the convexity inequality exp(x) ≥ 1 + x (Mathlib: add_one_le_exp),
-- we have 2^(1/n) = exp(ln2/n) ≥ 1 + ln2/n, so
-- n*(2^(1/n) - 1) ≥ n*(ln2/n) = ln2.
theorem rmBound_ge_ln2 (n : ℕ) (hn : n ≥ 1) :
    rmBound n hn ≥ Real.log 2 := by
  unfold rmBound
  have hn_pos : (0 : ℝ) < (n : ℝ) := Nat.cast_pos.mpr (by omega)
  have hn_ne : (n : ℝ) ≠ 0 := ne_of_gt hn_pos
  -- Step 1: Rewrite 2^(1/n) = exp(log 2 · (1/n))
  have h2pos : (0 : ℝ) < 2 := by norm_num
  rw [show (2 : ℝ) ^ ((1 : ℝ) / (n : ℝ)) = Real.exp (Real.log 2 * ((1 : ℝ) / (n : ℝ)))
    from Real.rpow_def_of_pos h2pos _]
  -- Step 2: exp(x) ≥ 1 + x gives exp(log 2 / n) - 1 ≥ log 2 / n
  set x := Real.log 2 * (1 / (n : ℝ))
  have h_exp_bound := Real.add_one_le_exp x
  -- h_exp_bound : x + 1 ≤ Real.exp x
  -- so exp(x) - 1 ≥ x
  -- Step 3: n * (exp(x) - 1) ≥ n * x = log 2
  have h_nx : (n : ℝ) * x = Real.log 2 := by
    simp only [x]; field_simp
  -- Goal: ↑n * (exp x - 1) ≥ Real.log 2
  -- = ↑n * (exp x - 1) ≥ ↑n * x   [since n*x = log 2]
  rw [← h_nx]
  -- Goal: ↑n * (exp x - 1) ≥ ↑n * x
  -- Since ≥ is ≤ reversed, this is: ↑n * x ≤ ↑n * (exp x - 1)
  -- From h_exp_bound: x + 1 ≤ exp x, so x ≤ exp x - 1.
  -- Multiply both sides by ↑n ≥ 0.
  exact mul_le_mul_of_nonneg_left (by linarith) (le_of_lt hn_pos)

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
