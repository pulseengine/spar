/-
  ARINC 653 Partition Isolation — Formal Proof

  Reference: ARINC Specification 653P1-5 §3 (Partitioning),
  SAE AS5506C §14.5 (AADL ARINC653 annex).

  ARINC 653 requires that the operating system enforce strict temporal
  and spatial isolation between partitions. In the temporal domain this
  means: during a partition's allocated window in the Major Frame, only
  threads bound to *that* partition may execute.

  We model:
    * `Partition`  — an opaque identifier for a partition.
    * `Thread`     — an opaque identifier for a thread.
    * `Window`     — a time slot in the Major Frame; carries the id of
                     the partition that owns the window.
    * `PartitionSchedule` — the ordered list of (partition, window) pairs
                     that make up one Major Frame.
    * `ThreadBinding` — a function mapping each thread to its partition.
    * `Executes`   — an axiomatised predicate: `Executes t w` means
                     thread `t` runs during window `w`.

  The isolation property: a conforming ARINC 653 schedule guarantees
  that if a window is allocated to partition P, then no thread whose
  binding is different from P can execute in that window.

  We prove this from a single conformance hypothesis:
    `scheduleConformant s binding` — every window in `s` only allows
    threads bound to `w.partition` to execute.

  This justifies the `arinc653_partition_isolation` check in
  `crates/spar-analysis/src/arinc653.rs`.
-/
import Mathlib.Tactic

namespace Spar.Scheduling.Arinc653

/-! ## Type definitions -/

/-- An opaque partition identifier. -/
structure Partition where
  id : Nat
  deriving DecidableEq, Repr

/-- An opaque thread identifier. -/
structure Thread where
  id : Nat
  deriving DecidableEq, Repr

/-- A time window in the Major Frame, allocated to a specific partition. -/
structure Window where
  /-- The partition that owns this window. -/
  partition : Partition
  /-- Slot index within the Major Frame (0-based). -/
  slot : Nat
  deriving DecidableEq, Repr

/-- A Major Frame: an ordered list of (partition, window) pairs.
    The pairing is redundant (window already carries its partition) but
    mirrors the AADL `ARINC653::Module_Schedule` property structure,
    which pairs each window with a partition reference. -/
abbrev PartitionSchedule := List (Partition × Window)

/-- Maps each thread to the partition it is statically bound to.
    Corresponds to the `ARINC653::Partition_Identifier` property on
    a virtual processor / process component in the AADL model. -/
abbrev ThreadBinding := Thread → Partition

/-! ## Execution predicate -/

/-- `Executes t w` is an abstract proposition: thread `t` runs during
    window `w`. We do not model *how* the OS schedules within a window;
    we only care about *which* partition's window a thread uses. -/
-- We introduce this as a section variable so callers supply a concrete
-- model if needed; all proofs work purely from the conformance hypothesis.
variable (Executes : Thread → Window → Prop)

/-! ## Conformance hypothesis -/

/-- A schedule is conformant with a thread binding if, for every window
    present in the schedule, no thread bound to a *different* partition
    executes in that window. This is the machine-checkable form of
    ARINC 653 §3's "partitioning" requirement. -/
def scheduleConformant
    (s : PartitionSchedule)
    (binding : ThreadBinding) : Prop :=
  ∀ (t : Thread) (w : Window),
    (∃ p, (p, w) ∈ s) →
    Executes t w →
    binding t = w.partition

/-! ## Helper lemmas -/

/-- If the schedule is conformant and thread `t` executes in window `w`,
    then `t`'s binding equals `w.partition`. -/
theorem binding_eq_of_executes
    {s : PartitionSchedule}
    {binding : ThreadBinding}
    (hconf : scheduleConformant Executes s binding)
    {t : Thread} {w : Window}
    (hmem : ∃ p, (p, w) ∈ s)
    (hexec : Executes t w) :
    binding t = w.partition :=
  hconf t w hmem hexec

/-- Contrapositive: if `t`'s binding differs from `w.partition`, and `w`
    appears in the schedule, then `t` cannot execute in `w`. -/
theorem not_executes_of_binding_ne
    {s : PartitionSchedule}
    {binding : ThreadBinding}
    (hconf : scheduleConformant Executes s binding)
    {t : Thread} {w : Window}
    (hmem : ∃ p, (p, w) ∈ s)
    (hne : binding t ≠ w.partition) :
    ¬ Executes t w := by
  intro hexec
  exact hne (binding_eq_of_executes Executes hconf hmem hexec)

/-! ## Main theorem -/

/-- **Theorem — ARINC 653 Partition Isolation.**

    Within a Major Frame `s` that is conformant with binding `binding`,
    a thread `t` whose binding differs from window `w`'s owning partition
    cannot execute during `w`.

    Formally:
      `scheduleConformant s binding →`
      `w ∈ s →`
      `w.partition ≠ binding t →`
      `¬ Executes t w`

    where membership is via the second component of the schedule pairs
    (i.e. `∃ p, (p, w) ∈ s`). -/
theorem partition_isolation
    {s : PartitionSchedule}
    {binding : ThreadBinding}
    (hconf : scheduleConformant Executes s binding)
    {t : Thread} {w : Window}
    (hmem : ∃ p, (p, w) ∈ s)
    (hne : w.partition ≠ binding t) :
    ¬ Executes t w :=
  not_executes_of_binding_ne Executes hconf hmem (Ne.symm hne)

/-- Corollary: threads in *different* partitions cannot share a window.
    If thread `t1` executes in window `w` and `t2` is bound to a
    different partition than `t1`, then `t2` does not execute in `w`. -/
theorem cross_partition_exclusion
    {s : PartitionSchedule}
    {binding : ThreadBinding}
    (hconf : scheduleConformant Executes s binding)
    {t1 t2 : Thread} {w : Window}
    (hmem : ∃ p, (p, w) ∈ s)
    (hexec1 : Executes t1 w)
    (hne : binding t1 ≠ binding t2) :
    ¬ Executes t2 w := by
  have hb1 : binding t1 = w.partition :=
    binding_eq_of_executes Executes hconf hmem hexec1
  -- binding t2 ≠ w.partition: if it were equal, binding t1 = binding t2 via hb1,
  -- contradicting hne.
  have hne2 : binding t2 ≠ w.partition := fun h => hne (hb1.trans h.symm)
  exact not_executes_of_binding_ne Executes hconf hmem hne2

end Spar.Scheduling.Arinc653
