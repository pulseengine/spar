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
--
-- Proof strategy (integer arithmetic):
--   Let q₁ = l / T₁, q₂ = l / T₂ (Nat division).
--   We know q₁ * T₁ ≤ l and q₂ * T₂ ≤ l (Nat.div_mul_le_self).
--   Multiply desired inequality by T₁ * T₂ (> 0):
--     (q₁*C₁ + q₂*C₂) * (T₁*T₂)
--     = (q₁*T₁)*(C₁*T₂) + (q₂*T₂)*(C₂*T₁)
--     ≤ l*(C₁*T₂) + l*(C₂*T₁)           [using q_i*T_i ≤ l]
--     = l*(C₁*T₂ + C₂*T₁)
--     ≤ l*(T₁*T₂)                         [hypothesis]
--   Cancel T₁*T₂ to get q₁*C₁ + q₂*C₂ ≤ l.
theorem edf_two_tasks_demand (t1 t2 : Task) (l : Nat)
    (h : t1.exec * t2.period + t2.exec * t1.period ≤ t1.period * t2.period) :
    demandBound t1 l + demandBound t2 l ≤ l := by
  unfold demandBound
  set q₁ := l / t1.period
  set q₂ := l / t2.period
  set C₁ := t1.exec
  set C₂ := t2.exec
  set T₁ := t1.period
  set T₂ := t2.period
  -- Cancel the positive factor T₁ * T₂
  have hT₁ : 0 < T₁ := t1.period_pos
  have hT₂ : 0 < T₂ := t2.period_pos
  have hT : 0 < T₁ * T₂ := Nat.mul_pos hT₁ hT₂
  -- Suffices to show (q₁*C₁ + q₂*C₂) * (T₁*T₂) ≤ l * (T₁*T₂)
  suffices hsuff : (q₁ * C₁ + q₂ * C₂) * (T₁ * T₂) ≤ l * (T₁ * T₂) by
    exact Nat.le_of_mul_le_mul_right hsuff hT
  -- Use: q₁ * T₁ ≤ l and q₂ * T₂ ≤ l
  have hq₁ : q₁ * T₁ ≤ l := Nat.div_mul_le_self l T₁
  have hq₂ : q₂ * T₂ ≤ l := Nat.div_mul_le_self l T₂
  calc (q₁ * C₁ + q₂ * C₂) * (T₁ * T₂)
      = q₁ * C₁ * (T₁ * T₂) + q₂ * C₂ * (T₁ * T₂) := by ring
    _ = (q₁ * T₁) * (C₁ * T₂) + (q₂ * T₂) * (C₂ * T₁) := by ring
    _ ≤ l * (C₁ * T₂) + l * (C₂ * T₁) := by
        apply Nat.add_le_add
        · exact Nat.mul_le_mul_right _ hq₁
        · exact Nat.mul_le_mul_right _ hq₂
    _ = l * (C₁ * T₂ + C₂ * T₁) := by ring
    _ ≤ l * (T₁ * T₂) := by exact Nat.mul_le_mul_left l h

end Spar.Scheduling.EDF
