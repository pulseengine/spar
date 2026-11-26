/-
Counts the declarations lean4checker replays for every module under a
prefix (default `Proofs`), reading the same .olean parts lean4checker reads
(exported, then server and private when present; the last part subsumes the
others). Run from proofs/ as `lake env lean --run ../tools/lean4checker/CountDecls.lean Proofs`.

Prints one `<module>: <n>` line per module and a final
`modules: <m> declarations: <total>` line, and exits 1 when the total is 0:
a re-check that replayed nothing has checked nothing.
-/
import Lean
open Lean

unsafe def main (args : List String) : IO UInt32 := do
  initSearchPath (← findSysroot)
  let sp ← searchPathRef.get
  let target := (args.headD "Proofs").toName
  let mut mods : Array Name := #[]
  for path in (← SearchPath.findAllWithExt sp "olean") do
    if let some m := (← searchModuleNameOfFileName path sp) then
      let last := m.getString!
      if target.isPrefixOf m && last != "server" && last != "private" && !mods.contains m then
        mods := mods.push m
  let mut total := 0
  for m in mods.qsort (fun a b => a.toString < b.toString) do
    let f ← findOLean m
    let mut fnames := #[f]
    let s := OLeanLevel.server.adjustFileName f
    if (← s.pathExists) then
      fnames := fnames.push s
      let p := OLeanLevel.private.adjustFileName f
      if (← p.pathExists) then
        fnames := fnames.push p
    let parts ← readModuleDataParts fnames
    let some (last, _) := parts.back? | throw <| IO.userError s!"no module data for {m}"
    let n := last.constNames.size
    IO.println s!"{m}: {n}"
    total := total + n
  IO.println s!"modules: {mods.size} declarations: {total}"
  return if total == 0 then 1 else 0
