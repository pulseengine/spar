/-
  RTACore — the mathlib-free combinational core of the RTA theory.

  These are the pure `def`s (no tactics, no Mathlib) that the Rust
  analysis in `crates/spar-analysis/src/scheduling_verified.rs` mirrors.
  They are separated out of `RTA.lean` (which imports `Mathlib.Tactic`
  for the proofs) so a future reflection-based generator can import THIS
  file and read the real definitions while staying mathlib-free — the
  named single source of the scheduling math for issue #321
  (REQ-PROOF-SCHED-CODEGEN-001, OPEN). Today the Rust is hand-written to
  mirror these defs and bound to them by property tests; it is not yet
  generated.

  `RTA.lean` imports this module and proves the theorems (monotonicity,
  convergence) about these exact definitions, so the proofs are stated
  over the same source the Rust mirrors.

  DO NOT add `import Mathlib.*` here — keeping this module mathlib-free is
  what will let the future generator reflect over it without dragging in
  mathlib (the fast build path the generator needs).
-/

namespace Spar.Scheduling.RTA

/-- A task: worst-case execution time, period, deadline (picoseconds),
    with positivity proofs. -/
structure Task where
  exec : Nat      -- worst-case execution time C_i
  period : Nat    -- minimum inter-arrival time T_i
  deadline : Nat  -- relative deadline D_i
  exec_pos : exec > 0
  period_pos : period > 0
  deadline_pos : deadline > 0

/-- Ceiling division: ⌈a / b⌉. -/
def ceilDiv (a b : Nat) (_ : b > 0) : Nat :=
  (a + b - 1) / b

/-- The interference from a single higher-priority task hp over an
    interval of length r. -/
def interference (hp : Task) (r : Nat) : Nat :=
  ceilDiv r hp.period hp.period_pos * hp.exec

/-- Total interference from all higher-priority tasks (recursive). -/
def totalInterference : List Task → Nat → Nat
  | [], _ => 0
  | hp :: rest, r => interference hp r + totalInterference rest r

/-- The RTA recurrence step: R_{n+1} = C_i + Σ ⌈R_n/T_j⌉ × C_j. -/
def rtaStep (task : Task) (hps : List Task) (r : Nat) : Nat :=
  task.exec + totalInterference hps r

end Spar.Scheduling.RTA
