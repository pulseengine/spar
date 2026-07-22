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

#eval reflectInterference
#eval reflectRtaStep
#eval reflectInterferenceJittered
#eval reflectRtaStepJittered
#eval reflectRtaStepJitteredBlocking

end Spar.Scheduling.Codegen
