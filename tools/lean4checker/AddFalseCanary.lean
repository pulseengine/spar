/-
Negative control for the lean4checker step in proofs.yml. Adds a theorem
`false : False` whose value is `False` itself, through `addDeclCore` with
`doCheck := false`, so the elaborator accepts it without the kernel. lean4checker
must reject the resulting .olean ("declaration type mismatch"). Copied from
lean4checker's own Lean4CheckerTests/AddFalse.lean at the pinned commit.
Never imported by proofs/; compiled into a scratch directory by the control step.
-/
import Lean.Elab.Term

open Lean in
run_elab
  modifyEnv fun env => Id.run do
    let decl := .thmDecl { name := `false, levelParams := [], type := .const ``False [], value := .const ``False [] }
    let .ok env := env.addDeclCore (doCheck := false) 0 decl none |
      let _ : Inhabited Environment := ⟨env⟩
      unreachable!
    env
