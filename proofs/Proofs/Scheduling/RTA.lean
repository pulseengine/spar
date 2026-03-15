/-
  Response Time Analysis (RTA) — Fixed-Point Convergence

  Reference: Joseph & Pandya, "Finding Response Times in a Real-Time
  System", The Computer Journal, 1986.

  We prove that the RTA recurrence:
    R_{n+1} = C_i + Σ_j ⌈R_n / T_j⌉ × C_j
  (where j ranges over higher-priority tasks)

  is monotonically non-decreasing and either:
    (a) converges to a fixed point R* ≤ D_i (task meets deadline), or
    (b) exceeds D_i in finite steps (task misses deadline).

  This justifies the termination of `compute_response_time` in
  crates/spar-analysis/src/scheduling.rs.
-/
import Mathlib.Tactic

namespace Spar.Scheduling.RTA

-- A task is characterized by its worst-case execution time,
-- period, and deadline (all positive naturals in picoseconds).
structure Task where
  exec : Nat      -- worst-case execution time C_i
  period : Nat    -- minimum inter-arrival time T_i
  deadline : Nat  -- relative deadline D_i
  exec_pos : exec > 0
  period_pos : period > 0
  deadline_pos : deadline > 0

-- Ceiling division: ⌈a / b⌉
def ceilDiv (a b : Nat) (_ : b > 0) : Nat :=
  (a + b - 1) / b

-- The interference from a single higher-priority task hp over
-- interval of length r.
def interference (hp : Task) (r : Nat) : Nat :=
  ceilDiv r hp.period hp.period_pos * hp.exec

-- Total interference from all higher-priority tasks (recursive).
def totalInterference : List Task → Nat → Nat
  | [], _ => 0
  | hp :: rest, r => interference hp r + totalInterference rest r

-- The RTA recurrence function: R_{n+1} = C_i + Σ ⌈R_n/T_j⌉ × C_j
def rtaStep (task : Task) (hps : List Task) (r : Nat) : Nat :=
  task.exec + totalInterference hps r

-- Key property: ceilDiv is monotone in its first argument.
theorem ceilDiv_mono {a₁ a₂ b : Nat} (hb : b > 0) (h : a₁ ≤ a₂) :
    ceilDiv a₁ b hb ≤ ceilDiv a₂ b hb := by
  unfold ceilDiv
  apply Nat.div_le_div_right
  omega

-- Interference from a single task is monotone in interval length.
theorem interference_mono {hp : Task} {r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    interference hp r₁ ≤ interference hp r₂ := by
  unfold interference
  apply Nat.mul_le_mul_right
  exact ceilDiv_mono hp.period_pos h

-- Total interference is monotone.
theorem totalInterference_mono {hps : List Task} {r₁ r₂ : Nat} (h : r₁ ≤ r₂) :
    totalInterference hps r₁ ≤ totalInterference hps r₂ := by
  induction hps with
  | nil => simp [totalInterference]
  | cons hp rest ih =>
    simp only [totalInterference]
    exact Nat.add_le_add (interference_mono h) ih

-- The RTA step function is monotone.
theorem rtaStep_mono {task : Task} {hps : List Task} {r₁ r₂ : Nat}
    (h : r₁ ≤ r₂) : rtaStep task hps r₁ ≤ rtaStep task hps r₂ := by
  unfold rtaStep
  exact Nat.add_le_add_left (totalInterference_mono h) _

-- rtaStep always ≥ exec (interference is non-negative).
theorem rtaStep_ge_exec (task : Task) (hps : List Task) (r : Nat) :
    rtaStep task hps r ≥ task.exec := by
  unfold rtaStep; omega

-- If rtaStep r = r, then r is a fixed point.
def isFixedPoint (task : Task) (hps : List Task) (r : Nat) : Prop :=
  rtaStep task hps r = r

-- Iterate a function n times.
def iterN (f : Nat → Nat) : Nat → Nat → Nat
  | 0, x => x
  | n + 1, x => f (iterN f n x)

-- iterN of a monotone function preserves ordering.
theorem iterN_mono {f : Nat → Nat} (hf : ∀ a b, a ≤ b → f a ≤ f b)
    {x y : Nat} (hxy : x ≤ y) (n : Nat) : iterN f n x ≤ iterN f n y := by
  induction n with
  | zero => exact hxy
  | succ n ih => exact hf _ _ ih

-- If f is monotone and f x ≥ x, then the iterN sequence is non-decreasing.
theorem iterN_nondecreasing {f : Nat → Nat} (hf : ∀ a b, a ≤ b → f a ≤ f b)
    {x : Nat} (hfx : f x ≥ x) (n : Nat) : iterN f n x ≤ iterN f (n + 1) x := by
  induction n with
  | zero => exact hfx
  | succ n ih => exact hf _ _ ih

-- If no fixed point exists in 0..n-1, each step increases value by ≥ 1.
-- Therefore iterN f n x ≥ x + n.
theorem no_fp_implies_growth {f : Nat → Nat} (_hf : ∀ a b, a ≤ b → f a ≤ f b)
    {x : Nat} (_hfx : f x ≥ x)
    (hno : ∀ k, k < n → iterN f k x < iterN f (k + 1) x) :
    iterN f n x ≥ x + n := by
  induction n with
  | zero => simp [iterN]
  | succ n ih =>
    have ih' : iterN f n x ≥ x + n :=
      ih (fun k hk => hno k (Nat.lt_succ_of_lt hk))
    have hstep := hno n (Nat.lt_succ_iff.mpr (Nat.le_refl n))
    omega

-- CORE LEMMA: A non-decreasing Nat sequence bounded above by B
-- either reaches a fixed point or exceeds B within B + 1 steps.
theorem bounded_mono_nat_seq {f : Nat → Nat} (hf : ∀ a b, a ≤ b → f a ≤ f b)
    {x B : Nat} (hfx : f x ≥ x) :
    ∃ n : Nat, n ≤ B + 1 ∧
      (iterN f n x = iterN f (n + 1) x ∨ iterN f n x > B) := by
  -- Either some step in 0..B is a fixed point, or all are strict increases.
  by_cases h : ∃ k, k ≤ B ∧ iterN f k x = iterN f (k + 1) x
  · -- Case 1: fixed point found
    obtain ⟨k, hk, heq⟩ := h
    exact ⟨k, by omega, Or.inl heq⟩
  · -- Case 2: no fixed point in 0..B, so all steps are strict.
    push_neg at h
    -- h : ∀ k, k ≤ B → iterN f k x ≠ iterN f (k + 1) x
    -- Combined with non-decreasing, this means strict increase at each step.
    have hstrict : ∀ k, k ≤ B → iterN f k x < iterN f (k + 1) x := by
      intro k hk
      have hne := h k hk
      have hle := iterN_nondecreasing hf hfx k
      omega
    -- After B+1 strict increases: iterN f (B+1) x ≥ x + (B+1) > B
    have hgrow : iterN f (B + 1) x ≥ x + (B + 1) :=
      no_fp_implies_growth hf hfx (n := B + 1)
        (fun k hk => hstrict k (by omega))
    exact ⟨B + 1, le_refl _, Or.inr (by omega)⟩

-- RTA step at initial value: rtaStep(C_i) ≥ C_i.
theorem rtaStep_ge_initial (task : Task) (hps : List Task) :
    rtaStep task hps task.exec ≥ task.exec := by
  unfold rtaStep; omega

-- MAIN THEOREM: RTA terminates.
theorem rta_terminates (task : Task) (hps : List Task) :
    ∃ n : Nat, n ≤ task.deadline + 1 ∧
      (isFixedPoint task hps (iterN (rtaStep task hps) n task.exec) ∨
       iterN (rtaStep task hps) n task.exec > task.deadline) := by
  have hmono : ∀ a b, a ≤ b → rtaStep task hps a ≤ rtaStep task hps b :=
    fun a b h => rtaStep_mono h
  have hexp := rtaStep_ge_initial task hps
  obtain ⟨n, hn, hor⟩ := bounded_mono_nat_seq hmono hexp (B := task.deadline)
  refine ⟨n, hn, ?_⟩
  rcases hor with heq | hgt
  · left
    unfold isFixedPoint
    show rtaStep task hps (iterN (rtaStep task hps) n task.exec) =
         iterN (rtaStep task hps) n task.exec
    have : iterN (rtaStep task hps) (n + 1) task.exec =
        rtaStep task hps (iterN (rtaStep task hps) n task.exec) := rfl
    linarith
  · exact Or.inr hgt

-- Soundness: every iterate is ≤ any fixed point ≥ C_i.
-- This shows the iterate-computed response time is the LEAST fixed point.
theorem iterN_le_fixed_point (task : Task) (hps : List Task)
    (r' : Nat) (hfp' : isFixedPoint task hps r') (hge' : r' ≥ task.exec)
    (n : Nat) : iterN (rtaStep task hps) n task.exec ≤ r' := by
  induction n with
  | zero => exact hge'
  | succ n ih =>
    calc iterN (rtaStep task hps) (n + 1) task.exec
        = rtaStep task hps (iterN (rtaStep task hps) n task.exec) := rfl
      _ ≤ rtaStep task hps r' := rtaStep_mono ih
      _ = r' := hfp'

-- Corollary: if RTA converges, it finds the LEAST fixed point.
theorem rta_finds_least_fixed_point (task : Task) (hps : List Task) (n : Nat)
    (_hfp : isFixedPoint task hps (iterN (rtaStep task hps) n task.exec)) :
    ∀ r' : Nat, isFixedPoint task hps r' → r' ≥ task.exec →
      iterN (rtaStep task hps) n task.exec ≤ r' :=
  fun r' hfp' hge' => iterN_le_fixed_point task hps r' hfp' hge' n

end Spar.Scheduling.RTA
