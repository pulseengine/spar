/-
  EDF Optimality for Implicit-Deadline Systems

  Reference: Dertouzos, "Control Robotics: The Procedural Control of
  Physical Processes", IFIP Congress, 1974.

  Theorem: For a set of periodic tasks with implicit deadlines
  (D_i = T_i) on a single preemptive processor:
    U = Σ(C_i/T_i) ≤ 1  ⟺  EDF-schedulable

  This is both sufficient AND necessary (unlike the RM bound which
  is only sufficient). Our scheduling.rs uses this to report EDF
  feasibility alongside RM analysis.
-/
import Mathlib.Tactic

namespace Spar.Scheduling.EDF

-- Task model.
structure Task where
  exec : Nat
  period : Nat
  exec_pos : exec > 0
  period_pos : period > 0
  exec_le_period : exec ≤ period  -- U_i ≤ 1 per task

-- Demand Bound Function: ⌊L / T_i⌋ × C_i
def demandBound (t : Task) (l : Nat) : Nat :=
  (l / t.period) * t.exec

-- Total demand from all tasks.
def totalDemand : List Task → Nat → Nat
  | [], _ => 0
  | t :: rest, l => demandBound t l + totalDemand rest l

-- Single-task demand ≤ interval length when C ≤ T.
theorem demand_le_interval (t : Task) (l : Nat) :
    demandBound t l ≤ l := by
  unfold demandBound
  calc (l / t.period) * t.exec
      ≤ (l / t.period) * t.period := Nat.mul_le_mul_left _ t.exec_le_period
    _ ≤ l := Nat.div_mul_le_self l t.period

-- Total demand ≤ n × L.
theorem totalDemand_le_nL (tasks : List Task) (l : Nat) :
    totalDemand tasks l ≤ tasks.length * l := by
  induction tasks with
  | nil => simp [totalDemand]
  | cons t rest ih =>
    simp only [totalDemand, List.length_cons]
    have h1 := demand_le_interval t l
    linarith

-- Demand bound is monotone in interval length.
theorem demandBound_mono {t : Task} {l₁ l₂ : Nat} (h : l₁ ≤ l₂) :
    demandBound t l₁ ≤ demandBound t l₂ := by
  unfold demandBound
  exact Nat.mul_le_mul_right _ (Nat.div_le_div_right h)

-- Total demand is monotone in interval length.
theorem totalDemand_mono {tasks : List Task} {l₁ l₂ : Nat} (h : l₁ ≤ l₂) :
    totalDemand tasks l₁ ≤ totalDemand tasks l₂ := by
  induction tasks with
  | nil => simp [totalDemand]
  | cons t rest ih =>
    simp only [totalDemand]
    exact Nat.add_le_add (demandBound_mono h) ih

-- Demand at period boundaries: dbf(i, k×T_i) = k × C_i.
theorem demandBound_at_period (t : Task) (k : Nat) :
    demandBound t (k * t.period) = k * t.exec := by
  unfold demandBound
  rw [Nat.mul_div_cancel k t.period_pos]

-- For two tasks with U₁ + U₂ ≤ 1 (cross-multiplied):
-- C₁×T₂ + C₂×T₁ ≤ T₁×T₂ → total demand ≤ L for all L.
theorem edf_two_tasks_demand (t1 t2 : Task) (l : Nat)
    (h : t1.exec * t2.period + t2.exec * t1.period ≤ t1.period * t2.period) :
    demandBound t1 l + demandBound t2 l ≤ l := by
  unfold demandBound
  -- ⌊L/T₁⌋×C₁ + ⌊L/T₂⌋×C₂
  -- ≤ (L/T₁)×C₁ + (L/T₂)×C₂  (floor ≤ real value)
  -- ≤ L × (C₁/T₁ + C₂/T₂)
  -- ≤ L × 1 = L
  -- In integer arithmetic, use: ⌊L/T⌋ × C ≤ L × C / T
  -- and then L × C₁ / T₁ + L × C₂ / T₂ ≤ L
  sorry -- needs rational/real arithmetic or careful integer bounding

end Spar.Scheduling.EDF
