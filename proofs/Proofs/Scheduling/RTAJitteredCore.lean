/-
  RTAJitteredCore — the mathlib-free combinational core of the jittered +
  blocking RTA recurrence (Tindell & Clark).

  Like RTACore relative to RTA.lean: these are the pure `def`s (no tactics,
  no Mathlib) that `scheduling_verified.rs` computes, split out of
  `RTAJittered.lean` / `ArincSupply.lean` (which import `Mathlib.Tactic`
  for the proofs) so the reflection-based `Codegen.lean` can read the real
  definitions while staying mathlib-free (issue #321,
  REQ-PROOF-SCHED-CODEGEN-001). `RTAJittered.lean` and `ArincSupply.lean`
  import this module and prove their theorems about these exact defs.

  DO NOT add `import Mathlib.*` here.
-/
import Proofs.Scheduling.RTACore

namespace Spar.Scheduling.RTAJittered

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

/-- Interference from one higher-priority task with release jitter:
    `⌈(r + J_j) / T_j⌉ × C_j`.  Mirrors `interference_jittered`. -/
def interferenceJittered (hp : JitteredHigherPriorityTask) (r : Nat) : Nat :=
  Spar.Scheduling.RTA.ceilDiv (r + hp.jitter) hp.period hp.period_pos * hp.exec

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

/-- The blocking-augmented step: the jittered step plus a constant
    blocking term `B`.  Mirrors `rta_step_jittered_blocking`. -/
def rtaStepJitteredBlocking
    (task : JitteredTask)
    (hps : List JitteredHigherPriorityTask)
    (isr : IsrOverhead)
    (blocking : Nat)
    (r : Nat) : Nat :=
  rtaStepJittered task hps isr r + blocking

end Spar.Scheduling.RTAJittered
