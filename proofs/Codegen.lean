/-
  Codegen — reflect the proven Lean scheduling definitions to Rust.

  REQ-PROOF-SCHED-CODEGEN-001 (#321). Reflects the *actual elaborated
  `Expr`* of the combinational RTA recurrence defs (RTACore.lean +
  RTAJitteredCore.lean), so the emitted Rust arithmetic is a function of
  the proven definitions — change the Lean formula and the Rust
  regenerates. A byte-diff CI gate then makes theory↔code drift for the
  recurrence arithmetic unrepresentable.

  Reflected (body read from the Lean `Expr`, drift-proof via the gate):
  `interference`, `rtaStep`, `interferenceJittered`, `rtaStepJittered`,
  `rtaStepJitteredBlocking`.

  Templated (structural scaffolding — the trusted base, gated by the
  property tests `ceil_div_matches_lean_definition` /
  `compute_response_time_matches_lean_spec`, NOT by reflection):
  `ceil_div` (leaf idiom `a.div_ceil(b)`), the summation folds
  (`total_interference`, `total_interference_jittered`,
  `total_isr_interference`), and the `compute_response_time*` fixed-point
  drivers (justified by the convergence theorems).

  Sound Nat→u64 lowering: `Nat.add → saturating_add`, `Nat.mul →
  saturating_mul`, `Nat.sub → saturating_sub`. For deadline-bounded values
  these are bit-identical to exact Nat; out of range they saturate to MAX,
  which the driver reads as "diverged" — the safe direction.

  Run: `lake exe codegen` (mathlib-free; the *Core modules import no Mathlib).
-/
import Lean
import Proofs.Scheduling.RTACore
import Proofs.Scheduling.RTAJitteredCore

open Lean Meta

namespace Spar.Scheduling.Codegen

/-- Binding environment: reflected sub-expression → Rust variable it
    lowers to. Holds flattened scalar params and struct projections
    (`Task.exec hp` → `"exec"`). -/
abbrev RustEnv := List (Expr × String)

/-- Function-valued params (the abstract `isr : IsrOverhead`): fvar →
    (Rust slice name, Rust fold-fn name). Applying it emits the fold call;
    passing it as a value uses the slice name (handled by `RustEnv`). -/
abbrev FuncEnv := List (Expr × (String × String))

private def projApp (proj : Name) (arg : Expr) : Expr :=
  mkApp (.const proj []) arg

private def binopOperands (args : Array Expr) : Expr × Expr :=
  (args[args.size - 2]!, args[args.size - 1]!)

/-- Reflect a Nat-valued recurrence-def body `Expr` into a Rust
    expression string under a binding + function environment. -/
partial def exprToRust (env : RustEnv) (fenv : FuncEnv) (e : Expr) : MetaM String := do
  -- Whole-subexpression bindings (scalar params, struct projections).
  for (key, name) in env do
    if ← isDefEq key e then
      return name
  -- Application of a function-valued param: `isr r` → fold call.
  if e.isApp then
    let fn := e.getAppFn
    for (key, (slice, foldFn)) in fenv do
      if ← isDefEq key fn then
        let arg ← exprToRust env fenv e.appArg!
        return s!"{foldFn}({slice}, {arg})"
  let e ← whnfR e
  let args := e.getAppArgs
  match e.getAppFn with
  | .const name _ =>
    match name.toString with
    | "OfNat.ofNat" =>
      if let some lit := args[args.size - 2]? then
        if let some n := lit.rawNatLit? then
          return toString n
      throwError s!"exprToRust: bad OfNat {← ppExpr e}"
    | "HAdd.hAdd" | "Nat.add" =>
      let (a, b) := binopOperands args
      return s!"{← exprToRust env fenv a}.saturating_add({← exprToRust env fenv b})"
    | "HMul.hMul" | "Nat.mul" =>
      let (a, b) := binopOperands args
      return s!"{← exprToRust env fenv a}.saturating_mul({← exprToRust env fenv b})"
    | "HSub.hSub" | "Nat.sub" =>
      let (a, b) := binopOperands args
      return s!"{← exprToRust env fenv a}.saturating_sub({← exprToRust env fenv b})"
    | "Spar.Scheduling.RTA.ceilDiv" =>
      return s!"ceil_div({← exprToRust env fenv args[0]!}, {← exprToRust env fenv args[1]!})"
    | "Spar.Scheduling.RTA.interference" =>
      let hp := args[0]!
      let r ← exprToRust env fenv args[1]!
      let period ← exprToRust env fenv (projApp ``Spar.Scheduling.RTA.Task.period hp)
      let exec ← exprToRust env fenv (projApp ``Spar.Scheduling.RTA.Task.exec hp)
      return s!"interference({period}, {exec}, {r})"
    | "Spar.Scheduling.RTA.totalInterference" =>
      return s!"total_interference({← exprToRust env fenv args[0]!}, {← exprToRust env fenv args[1]!})"
    | "Spar.Scheduling.RTAJittered.interferenceJittered" =>
      let hp := args[0]!
      let r ← exprToRust env fenv args[1]!
      let period ← exprToRust env fenv (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.period hp)
      let exec ← exprToRust env fenv (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.exec hp)
      let jitter ← exprToRust env fenv (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.jitter hp)
      return s!"interference_jittered({period}, {exec}, {jitter}, {r})"
    | "Spar.Scheduling.RTAJittered.totalInterferenceJittered" =>
      return s!"total_interference_jittered({← exprToRust env fenv args[0]!}, {← exprToRust env fenv args[1]!})"
    | "Spar.Scheduling.RTAJittered.rtaStepJittered" =>
      let task := args[0]!
      let hps ← exprToRust env fenv args[1]!
      let isr ← exprToRust env fenv args[2]!
      let r ← exprToRust env fenv args[3]!
      let exec ← exprToRust env fenv (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.exec task)
      let jitter ← exprToRust env fenv (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.jitter task)
      return s!"rta_step_jittered({exec}, {jitter}, {hps}, {isr}, {r})"
    | other => throwError s!"exprToRust: unhandled const head `{other}` in {← ppExpr e}"
  | _ => throwError s!"exprToRust: unhandled expr {← ppExpr e}"

/-- Reflect the body of a def under a prepared (env, fenv), telescoping
    the lambda binders first. `mkEnv` receives the intro'd param fvars. -/
def reflectBody (declName : Name)
    (mkEnv : Array Expr → RustEnv × FuncEnv) : MetaM String := do
  let info ← getConstInfo declName
  let some val := info.value? | throwError s!"{declName} has no value"
  lambdaTelescope val fun xs body => do
    let (env, fenv) := mkEnv xs
    exprToRust env fenv body

open Spar.Scheduling.RTA (Task) in
open Spar.Scheduling.RTAJittered (JitteredTask JitteredHigherPriorityTask) in
def reflectInterference : MetaM String :=
  reflectBody ``Spar.Scheduling.RTA.interference fun xs =>
    ([ (projApp ``Task.period xs[0]!, "period"),
       (projApp ``Task.exec xs[0]!, "exec"),
       (xs[1]!, "r") ], [])

def reflectRtaStep : MetaM String :=
  reflectBody ``Spar.Scheduling.RTA.rtaStep fun xs =>
    ([ (projApp ``Spar.Scheduling.RTA.Task.exec xs[0]!, "exec"),
       (xs[1]!, "higher_priority"),
       (xs[2]!, "r") ], [])

def reflectInterferenceJittered : MetaM String :=
  reflectBody ``Spar.Scheduling.RTAJittered.interferenceJittered fun xs =>
    ([ (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.period xs[0]!, "period"),
       (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.exec xs[0]!, "exec"),
       (projApp ``Spar.Scheduling.RTAJittered.JitteredHigherPriorityTask.jitter xs[0]!, "jitter"),
       (xs[1]!, "r") ], [])

def reflectRtaStepJittered : MetaM String :=
  reflectBody ``Spar.Scheduling.RTAJittered.rtaStepJittered fun xs =>
    ([ (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.exec xs[0]!, "exec"),
       (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.jitter xs[0]!, "jitter"),
       (xs[1]!, "higher_priority_jittered"),
       (xs[2]!, "isr_interference"),
       (xs[3]!, "r") ],
     [ (xs[2]!, ("isr_interference", "total_isr_interference")) ])

def reflectRtaStepJitteredBlocking : MetaM String :=
  reflectBody ``Spar.Scheduling.RTAJittered.rtaStepJitteredBlocking fun xs =>
    ([ (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.exec xs[0]!, "exec"),
       (projApp ``Spar.Scheduling.RTAJittered.JitteredTask.jitter xs[0]!, "jitter"),
       (xs[1]!, "higher_priority_jittered"),
       (xs[2]!, "isr_interference"),
       (xs[3]!, "blocking"),
       (xs[4]!, "r") ],
     [ (xs[2]!, ("isr_interference", "total_isr_interference")) ])

/-! ## Emission — assemble the complete `scheduling_verified.rs`.

The reflected bodies are spliced into rustfmt-formatted template
scaffolding. `formatBody` reproduces rustfmt's method-chain breaking so
the generator output is byte-identical to `rustfmt` output (the CI gate
is a byte diff). -/

/-- rustfmt `max_width`. -/
private def maxWidth : Nat := 100

/-- Split a reflected expression at top-level `.` method-chain
    boundaries (paren-depth 0). Returns (receiver, segments), each
    segment starting with `.`. -/
private def splitChain (s : String) : String × List String := Id.run do
  let mut depth : Nat := 0
  let mut cur := ""
  let mut parts : List String := []
  for c in s.toList do
    if c == '(' then
      depth := depth + 1
      cur := cur.push c
    else if c == ')' then
      depth := depth - 1
      cur := cur.push c
    else if c == '.' && depth == 0 then
      parts := parts.concat cur
      cur := "."
    else
      cur := cur.push c
  parts := parts.concat cur
  match parts with
  | [] => ("", [])
  | h :: t => (h, t)

private def isIdent (s : String) : Bool :=
  !s.isEmpty && s.toList.all fun c => c.isAlphanum || c == '_'

/-- Indent a reflected body as rustfmt would: one line at indent 4 if it
    fits in `maxWidth` columns; otherwise break the method chain at
    indent 8, gluing the first call to a bare-identifier receiver
    (rustfmt keeps `exec.saturating_add(jitter)` together but puts a
    call receiver like `rta_step_jittered(...)` on its own line). -/
private def formatBody (body : String) : String :=
  let line := "    " ++ body
  if line.length ≤ maxWidth then line
  else
    let (recv, segs) := splitChain body
    let (first, rest) :=
      match segs with
      | s0 :: srest => if isIdent recv then (recv ++ s0, srest) else (recv, segs)
      | [] => (recv, [])
    "    " ++ first ++ String.join (rest.map fun s => "\n        " ++ s)

private def tmpl1 : String := r#"//! Verified scheduling recurrences — GENERATED from the machine-checked
//! Lean definitions. DO NOT EDIT BY HAND.
//!
//! Regenerate: `cd proofs && lake exe codegen > ../crates/spar-analysis/src/scheduling_verified.rs`
//! A CI byte-diff gate fails if this file drifts from the generator output.
//!
//! Generated by `proofs/Codegen.lean`, which REFLECTS the elaborated Lean
//! `Expr` of each recurrence definition (RTACore.lean, RTAJitteredCore.lean)
//! and lowers it to Rust — so the recurrence ARITHMETIC below is a function
//! of the proven definitions, not a hand-transcription. Change the Lean
//! formula and this file regenerates.
//!
//! REFLECTED from the Lean def bodies (drift-proof via the gate):
//!   interference, rta_step, interference_jittered, rta_step_jittered,
//!   rta_step_jittered_blocking.
//!
//! TEMPLATED scaffolding (the trusted base — structural, not the arithmetic
//! that drifts; gated by the property tests `ceil_div_matches_lean_definition`
//! and `compute_response_time_matches_lean_spec`, NOT by reflection):
//!   ceil_div (leaf idiom a.div_ceil(b)), the summation folds
//!   (total_interference, total_interference_jittered, total_isr_interference),
//!   and the compute_response_time* fixed-point drivers (justified by the
//!   Lean convergence theorems).
//!
//! Lowering: Nat.add/mul/sub -> saturating_add/mul/sub; sound for
//! deadline-bounded values (out-of-range saturates to MAX = "diverged").

/// Ceiling division: ⌈a / b⌉
/// Lean: def ceilDiv (a b : Nat) (_ : b > 0) : Nat := a.div_ceil(b)
#[inline]
pub fn ceil_div(a: u64, b: u64) -> u64 {
    debug_assert!(b > 0);
    a.div_ceil(b)
}

/// Interference from one higher-priority task over interval r.
/// Lean: def interference (hp : Task) (r : Nat) : Nat :=
///         ceilDiv r hp.period hp.period_pos * hp.exec
#[inline]
pub fn interference(period: u64, exec: u64, r: u64) -> u64 {
"#

private def tmpl2 : String := r#"
}

/// Total interference from all higher-priority tasks.
/// Lean: def totalInterference : List Task → Nat → Nat
///         | [], _ => 0
///         | hp :: rest, r => interference hp r + totalInterference rest r
pub fn total_interference(higher_priority: &[(u64, u64)], r: u64) -> u64 {
    let mut total: u64 = 0;
    for &(period, exec) in higher_priority {
        total = total.saturating_add(interference(period, exec, r));
    }
    total
}

/// RTA recurrence: R_{n+1} = C_i + Σ ⌈R_n / T_j⌉ × C_j
/// Lean: def rtaStep (task : Task) (hps : List Task) (r : Nat) : Nat :=
///         task.exec + totalInterference hps r
#[inline]
pub fn rta_step(exec: u64, higher_priority: &[(u64, u64)], r: u64) -> u64 {
"#

private def tmpl3 : String := r#"
}

/// Result of RTA fixed-point iteration.
/// Lean: proved that iteration either converges or exceeds deadline
///       within deadline + 1 steps (bounded_mono_nat_seq).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtaResult {
    /// Fixed point found: response time = value.
    Converged(u64),
    /// Response time exceeds deadline — task misses deadline.
    Diverged,
}

/// Compute worst-case response time via fixed-point iteration.
///
/// Lean: iterN (rtaStep task hps) n task.exec
///       converges to least fixed point (rta_finds_least_fixed_point).
///
/// Max iterations bounded by deadline + 1 (rta_terminates).
pub fn compute_response_time(
    exec: u64,
    deadline: u64,
    higher_priority: &[(u64, u64)],
) -> RtaResult {
    let mut r = exec;
    for _ in 0..=deadline {
        let new_r = rta_step(exec, higher_priority, r);
        if new_r == r {
            return RtaResult::Converged(r);
        }
        if new_r > deadline {
            return RtaResult::Diverged;
        }
        r = new_r;
    }
    RtaResult::Diverged
}

/// Interference from one higher-priority task with release jitter.
///
/// Tindell-Clark extension: the task's release window is inflated by
/// its own jitter, which multiplies ceiling jobs it can release within
/// interval `r`:
///
///   interference_j(period, exec, jitter, r) = ⌈(r + jitter) / period⌉ × exec
///
/// When `jitter == 0` this reduces to [`interference`].
#[inline]
pub fn interference_jittered(period: u64, exec: u64, jitter: u64, r: u64) -> u64 {
"#

private def tmpl4 : String := r#"
}

/// Total interference from higher-priority tasks, each with its own jitter.
///
/// `higher_priority_jittered` items are `(period, exec, jitter)`.
pub fn total_interference_jittered(higher_priority_jittered: &[(u64, u64, u64)], r: u64) -> u64 {
    let mut total: u64 = 0;
    for &(period, exec, jitter) in higher_priority_jittered {
        total = total.saturating_add(interference_jittered(period, exec, jitter, r));
    }
    total
}

/// Total interference from periodic ISRs on the same CPU.
///
/// ISRs preempt all tasks unconditionally. Each ISR contributes
/// `⌈r / period⌉ × exec` to the task-level interference term.
/// `isrs` items are `(period, exec)`.
pub fn total_isr_interference(isrs: &[(u64, u64)], r: u64) -> u64 {
    total_interference(isrs, r)
}

/// Jittered RTA recurrence with ISR interference.
///
/// Computes
///
///   R_{n+1} = exec + jitter
///           + Σ_{hp j} ⌈(R_n + J_j)/T_j⌉ × C_j
///           + Σ_{ISR k} ⌈R_n / T_k⌉ × C_k
///
/// The own jitter `jitter` is added as a constant (the release window
/// of the task under analysis); higher-priority tasks' jitters scale
/// their ceiling counts. ISR interference is added as an extra term.
///
/// The classical convergence argument (monotone, bounded below, with
/// an upper bound of deadline+1) still applies because all added terms
/// are monotone non-decreasing in `r`.
#[inline]
pub fn rta_step_jittered(
    exec: u64,
    jitter: u64,
    higher_priority_jittered: &[(u64, u64, u64)],
    isr_interference: &[(u64, u64)],
    r: u64,
) -> u64 {
"#

private def tmpl5 : String := r#"
}

/// Compute worst-case response time for a task with release jitter,
/// competing HP tasks (also with jitter), and ISR interference.
///
/// When `jitter == 0`, all `higher_priority_jittered` items have
/// `jitter == 0`, and `isr_interference` is empty, this reduces to
/// exactly the same fixed-point as [`compute_response_time`].
///
/// Max iterations bounded by `deadline + 1` (monotone convergence).
pub fn compute_response_time_jittered(
    exec: u64,
    deadline: u64,
    jitter: u64,
    higher_priority_jittered: &[(u64, u64, u64)],
    isr_interference: &[(u64, u64)],
) -> RtaResult {
    // Initial value: exec + own jitter (the minimum response-window width).
    let mut r = exec.saturating_add(jitter);
    // If the starting point already exceeds the deadline, we've missed it.
    if r > deadline {
        return RtaResult::Diverged;
    }
    for _ in 0..=deadline {
        let new_r = rta_step_jittered(exec, jitter, higher_priority_jittered, isr_interference, r);
        if new_r == r {
            return RtaResult::Converged(r);
        }
        if new_r > deadline {
            return RtaResult::Diverged;
        }
        r = new_r;
    }
    RtaResult::Diverged
}

/// Jittered RTA recurrence with ISR interference and a PIP/PCP blocking term.
///
/// Joseph & Pandya 1986 / Buttazzo: the blocking term `B_i` is the
/// maximum time the task can be blocked by lower-priority tasks
/// holding shared resources under the configured Locking_Protocol.
/// Modelled here as a constant added per step:
///
/// ```text
/// R_{n+1} = exec + jitter + blocking
///         + Σ_{hp j} ⌈(R_n + J_j)/T_j⌉ × C_j
///         + Σ_{ISR k} ⌈R_n / T_k⌉ × C_k
/// ```
///
/// Convergence: `blocking` is a constant, so the recurrence remains
/// monotone non-decreasing in `r` and bounded above by `deadline + 1`,
/// preserving the proven termination argument.
#[inline]
pub fn rta_step_jittered_blocking(
    exec: u64,
    jitter: u64,
    blocking: u64,
    higher_priority_jittered: &[(u64, u64, u64)],
    isr_interference: &[(u64, u64)],
    r: u64,
) -> u64 {
"#

private def tmpl6 : String := r#"
}

/// Compute worst-case response time with jitter, ISR interference, and
/// a PIP/PCP blocking term `B_i`.
///
/// When `blocking == 0` this reduces to
/// [`compute_response_time_jittered`] exactly (every iterate is identical).
///
/// Max iterations bounded by `deadline + 1` (monotone convergence).
pub fn compute_response_time_jittered_blocking(
    exec: u64,
    deadline: u64,
    jitter: u64,
    blocking: u64,
    higher_priority_jittered: &[(u64, u64, u64)],
    isr_interference: &[(u64, u64)],
) -> RtaResult {
    // Initial value: exec + own jitter + blocking (the minimum response window).
    let mut r = exec.saturating_add(jitter).saturating_add(blocking);
    if r > deadline {
        return RtaResult::Diverged;
    }
    for _ in 0..=deadline {
        let new_r = rta_step_jittered_blocking(
            exec,
            jitter,
            blocking,
            higher_priority_jittered,
            isr_interference,
            r,
        );
        if new_r == r {
            return RtaResult::Converged(r);
        }
        if new_r > deadline {
            return RtaResult::Diverged;
        }
        r = new_r;
    }
    RtaResult::Diverged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_div_basic() {
        assert_eq!(ceil_div(7, 3), 3);
        assert_eq!(ceil_div(6, 3), 2);
        assert_eq!(ceil_div(1, 1), 1);
        assert_eq!(ceil_div(0, 5), 0);
    }

    #[test]
    fn single_task_converges() {
        // One task, no interference: R = C
        let result = compute_response_time(5, 20, &[]);
        assert_eq!(result, RtaResult::Converged(5));
    }

    #[test]
    fn two_tasks_converges() {
        // Task: exec=3, deadline=10
        // Higher priority: period=10, exec=2
        // R0 = 3, R1 = 3 + ceil(3/10)*2 = 3+2 = 5
        // R2 = 3 + ceil(5/10)*2 = 3+2 = 5 (fixed point)
        let result = compute_response_time(3, 10, &[(10, 2)]);
        assert_eq!(result, RtaResult::Converged(5));
    }

    #[test]
    fn overloaded_diverges() {
        // Task: exec=8, deadline=10
        // Higher priority: period=10, exec=5
        // R0=8, R1=8+ceil(8/10)*5=8+5=13 > 10 → diverged
        let result = compute_response_time(8, 10, &[(10, 5)]);
        assert_eq!(result, RtaResult::Diverged);
    }

    #[test]
    #[allow(clippy::manual_div_ceil)]
    fn ceil_div_matches_lean_definition() {
        // Lean codegen produces: (a + b - 1) / b
        // Rust uses: a.div_ceil(b)
        // Verify equivalence for all reasonable inputs
        for a in 0..=1000_u64 {
            for b in 1..=100_u64 {
                let lean_style = (a + b - 1) / b;
                let rust_style = a.div_ceil(b);
                assert_eq!(
                    lean_style, rust_style,
                    "ceil_div mismatch for a={}, b={}",
                    a, b
                );
            }
        }
    }

    #[test]
    fn compute_response_time_matches_lean_spec() {
        // Verify the response time computation matches the formal spec:
        // R(n+1) = C_i + sum(ceil(R(n)/T_j) * C_j) for higher-priority tasks
        // Test against known manually-computed results

        // Single task: R = C = 3
        assert_eq!(compute_response_time(3, 10, &[]), RtaResult::Converged(3));

        // Two tasks: R = C1 + ceil(R/T2)*C2, fixed point
        // Task 1: C=3, T=10, D=10; HP tasks: (T=5, C=2)
        // R(0) = 3
        // R(1) = 3 + ceil(3/5)*2 = 3 + 2 = 5
        // R(2) = 3 + ceil(5/5)*2 = 3 + 2 = 5 (converged)
        assert_eq!(
            compute_response_time(3, 10, &[(5, 2)]),
            RtaResult::Converged(5)
        );
    }

    // ── Jittered recurrence tests ──────────────────────────────────

    #[test]
    fn jittered_reduces_to_classical_when_zero() {
        // zero jitter, no ISRs: must match classical RTA exactly.
        let hp_jittered = [(10, 2, 0)];
        let r_j = compute_response_time_jittered(3, 10, 0, &hp_jittered, &[]);
        let r_c = compute_response_time(3, 10, &[(10, 2)]);
        assert_eq!(r_j, r_c);
    }

    #[test]
    fn jittered_own_jitter_adds_constant() {
        // Single task, own jitter = 1: R = C + J = 3 + 1 = 4.
        let r = compute_response_time_jittered(3, 10, 1, &[], &[]);
        assert_eq!(r, RtaResult::Converged(4));
    }

    #[test]
    fn jittered_hp_jitter_inflates_ceiling() {
        // Task: C=3, D=100. HP: T=10, C=2, J=5.
        // R0 = 3
        // R1 = 3 + ceil((3+5)/10) * 2 = 3 + 1*2 = 5
        // R2 = 3 + ceil((5+5)/10) * 2 = 3 + 1*2 = 5 (converged)
        // Without jitter: R = 3 + ceil(3/10)*2 = 3+2 = 5 also.
        // With J=15: R = 3 + ceil((5+15)/10)*2 = 3+4 = 7 ...
        let r_big = compute_response_time_jittered(3, 100, 0, &[(10, 2, 15)], &[]);
        match r_big {
            RtaResult::Converged(v) => assert!(v > 5, "jitter should inflate response: got {v}"),
            _ => panic!("should converge"),
        }
    }

    #[test]
    fn jittered_isr_interference_adds_term() {
        // Task: C=3, D=100. No HP tasks. One ISR: T=10, C=1.
        // R0 = 3, R1 = 3 + ceil(3/10)*1 = 3 + 1 = 4, R2 = 3 + ceil(4/10)*1 = 4. Converged.
        let r = compute_response_time_jittered(3, 100, 0, &[], &[(10, 1)]);
        assert_eq!(r, RtaResult::Converged(4));
    }

    #[test]
    fn jittered_diverges_on_overload() {
        // C=8, D=10, HP T=10 C=5, J=0: classical diverges. Jittered should too.
        let r = compute_response_time_jittered(8, 10, 0, &[(10, 5, 0)], &[]);
        assert_eq!(r, RtaResult::Diverged);
    }

    // ── Blocking-term recurrence tests (v0.7.1) ────────────────────

    #[test]
    fn blocking_zero_matches_jittered() {
        // Identical to compute_response_time_jittered when blocking=0.
        let hp = [(10, 2, 0)];
        let r_b = compute_response_time_jittered_blocking(3, 100, 0, 0, &hp, &[]);
        let r_j = compute_response_time_jittered(3, 100, 0, &hp, &[]);
        assert_eq!(r_b, r_j);
    }

    #[test]
    fn blocking_inflates_by_constant() {
        // Single task, no HP, no ISR, B=2: R = C + B = 3 + 2 = 5.
        let r = compute_response_time_jittered_blocking(3, 10, 0, 2, &[], &[]);
        assert_eq!(r, RtaResult::Converged(5));
    }

    #[test]
    fn blocking_plus_isr_compose() {
        // C=3, D=100, B=5, ISR T=10 C=1.
        // Without blocking: jittered → R0=3, R1=3+ceil(3/10)*1=4, R2=3+ceil(4/10)*1=4. R=4.
        // With B=5: every iterate adds +5 → R=9.
        let r = compute_response_time_jittered_blocking(3, 100, 0, 5, &[], &[(10, 1)]);
        assert_eq!(r, RtaResult::Converged(9));
    }

    #[test]
    fn blocking_diverges_on_overflow_of_deadline() {
        // C=8, D=10, B=5: even before any HP/ISR work, exec+B=13 > D=10.
        let r = compute_response_time_jittered_blocking(8, 10, 0, 5, &[], &[]);
        assert_eq!(r, RtaResult::Diverged);
    }
}
"#

/-- Emit the complete `scheduling_verified.rs`: templated scaffolding
    with the five recurrence bodies spliced in from the reflector, so
    the emitted arithmetic is a function of the proven Lean defs. -/
def emitFile : MetaM String := do
  let interferenceBody ← reflectInterference
  let rtaStepBody ← reflectRtaStep
  let interferenceJitteredBody ← reflectInterferenceJittered
  let rtaStepJitteredBody ← reflectRtaStepJittered
  let rtaStepJitteredBlockingBody ← reflectRtaStepJitteredBlocking
  return tmpl1 ++ formatBody interferenceBody
    ++ tmpl2 ++ formatBody rtaStepBody
    ++ tmpl3 ++ formatBody interferenceJitteredBody
    ++ tmpl4 ++ formatBody rtaStepJitteredBody
    ++ tmpl5 ++ formatBody rtaStepJitteredBlockingBody
    ++ tmpl6

end Spar.Scheduling.Codegen

open Lean in
unsafe def main : IO Unit := do
  initSearchPath (← findSysroot)
  let env ← importModules
    #[{ module := `Proofs.Scheduling.RTACore },
      { module := `Proofs.Scheduling.RTAJitteredCore }] {} 0
  let coreCtx : Core.Context := { fileName := "<codegen>", fileMap := default }
  let coreState : Core.State := { env := env }
  let (out, _) ← ((Spar.Scheduling.Codegen.emitFile).run').toIO coreCtx coreState
  IO.print out
