/-
  End-to-End Flow Latency — Monotonicity in Component Execution Times

  Reference: AADL AS5506C §12 (end-to-end flows); Feiertag et al.,
  "A Compositional Framework for End-to-End Path Delay Calculation of
  Automotive Systems", WATERS 2009.

  We model an end-to-end flow as a `List ExecTime`, where each element
  is the worst-case execution time (WCET) of one component along the path.
  The aggregate latency is the sum of WCETs plus a fixed sampling delay
  (one major frame period, also expressed as a `Nat`).

  The key property: if every per-component WCET is pointwise non-decreasing
  (i.e. the system is under higher load), the end-to-end latency cannot
  decrease. This justifies spar-analysis's latency analysis pass — it
  is sound to replace each component's WCET with a conservative upper
  bound without underestimating the flow latency.

  This justifies `compute_flow_latency` in
  `crates/spar-analysis/src/latency.rs`.
-/
import Mathlib.Tactic

namespace Spar.Scheduling.Latency

/-! ## Type aliases -/

/-- A worst-case execution time (non-negative, in picoseconds). -/
abbrev ExecTime := Nat

/-- An end-to-end flow is a sequence of component WCETs. -/
abbrev FlowPath := List ExecTime

/-- Time values (non-negative, in picoseconds). -/
abbrev Time := Nat

/-! ## Latency model -/

/-- `Latency sampling path` is the sum of WCETs along `path` plus a
    constant `sampling` delay (one major frame / hyper-period).
    `sampling` accounts for the worst-case sampling jitter introduced
    when the first component reads from an upstream periodic source. -/
def Latency (sampling : Time) (path : FlowPath) : Time :=
  sampling + path.sum

/-! ## Helper lemmas -/

/-- The sum of a list is monotone in each element: if every element of
    `c1` is ≤ the corresponding element of `c2` (pointwise), then
    `c1.sum ≤ c2.sum`. -/
theorem list_sum_le_of_pointwise {c1 c2 : FlowPath}
    (h : List.Forall₂ (· ≤ ·) c1 c2) : c1.sum ≤ c2.sum := by
  induction h with
  | nil => simp
  | cons hle _ ih =>
    simp only [List.sum_cons]
    exact Nat.add_le_add hle ih

/-- Latency is monotone in the sampling delay: increasing the sampling
    delay increases (or preserves) the total latency. -/
theorem latency_mono_sampling {s1 s2 : Time} (path : FlowPath)
    (h : s1 ≤ s2) : Latency s1 path ≤ Latency s2 path := by
  unfold Latency
  exact Nat.add_le_add_right h _

/-- Latency is monotone in the path: if every component WCET in `c1` is
    ≤ the corresponding WCET in `c2`, then `Latency s c1 ≤ Latency s c2`. -/
theorem latency_mono_path (s : Time) {c1 c2 : FlowPath}
    (h : List.Forall₂ (· ≤ ·) c1 c2) :
    Latency s c1 ≤ Latency s c2 := by
  unfold Latency
  exact Nat.add_le_add_left (list_sum_le_of_pointwise h) _

/-! ## Main theorem -/

/-- **Theorem — Latency Monotonicity.**

    If every component's WCET in path `c1` is pointwise ≤ the corresponding
    WCET in path `c2`, then the end-to-end latency under `c1` is ≤ the
    latency under `c2`, regardless of the sampling delay.

    Formally: `∀ i, c1[i] ≤ c2[i]  →  Latency s c1 ≤ Latency s c2`.

    `List.Forall₂ (· ≤ ·) c1 c2` is the standard Mathlib spelling of
    "pointwise ≤ on lists of the same length". -/
theorem latency_monotone (s : Time) (c1 c2 : FlowPath)
    (h : List.Forall₂ (· ≤ ·) c1 c2) :
    Latency s c1 ≤ Latency s c2 :=
  latency_mono_path s h

/-- Corollary: adding a component to a flow never decreases its latency
    (single-step extension). -/
theorem latency_cons_le (s : Time) (e : ExecTime) (path : FlowPath) :
    Latency s path ≤ Latency s (e :: path) := by
  unfold Latency
  simp only [List.sum_cons]
  omega

/-- Helper: `List.set i e' l` preserves all elements except position `i`. -/
theorem list_sum_set_le {path : FlowPath} {i : Nat} {e' : ExecTime}
    (hi : i < path.length) (he : path[i]'hi ≤ e') :
    path.sum ≤ (path.set i e').sum := by
  induction path generalizing i with
  | nil => exact absurd hi (by simp)
  | cons x xs ih =>
    cases i with
    | zero =>
      simp only [List.set, List.getElem_cons_zero] at he
      simp only [List.set, List.sum_cons]
      exact Nat.add_le_add_right he _
    | succ i' =>
      simp only [List.set, List.sum_cons, List.length_cons, Nat.succ_lt_succ_iff] at *
      exact Nat.add_le_add_left (ih (by omega) (by simpa using he)) _

/-- Corollary: replacing one component's WCET with a larger value
    yields a larger or equal latency. -/
theorem latency_replace_le (s : Time) (path : FlowPath)
    (i : Nat) (e' : ExecTime)
    (hi : i < path.length) (he : path[i]'hi ≤ e') :
    Latency s path ≤ Latency s (path.set i e') := by
  unfold Latency
  exact Nat.add_le_add_left (list_sum_set_le hi he) _

end Spar.Scheduling.Latency
