Rank source files in this repository by likelihood of containing a
correctness-relevant bug (AADL parsing accepts invalid models, HIR
construction drops semantic detail, solver emits infeasible
deployment, codegen produces wrong WASM), on a 1–5 scale. Output
JSON: `[{"file": "...", "rank": N, "reason": "..."}]`, sorted
descending.

Scope: `crates/spar-*/src/**/*.rs`. Exclude tests, benches.

Ranking rubric (spar-specific — AADL v2.2/v2.3 toolchain with
multi-phase pipeline):

5 (frontend correctness — bugs here = wrong semantic model downstream):
  - crates/spar-parser/**      # AADL text → AST
  - crates/spar-syntax/**      # AST → typed syntax tree
  - crates/spar-hir/**         # semantic model (intermediate rep)
  - crates/spar-hir-def/**     # HIR definitions / database queries
  - crates/spar-annex/**       # annex sub-languages (BLESS, EMV2, etc.)

4 (analysis + transform + solver):
  - crates/spar-analysis/**    # dataflow / property checks
  - crates/spar-transform/**   # HIR → optimized HIR
  - crates/spar-solver/**      # constraint solver — infeasible-deployment class
  - crates/spar-verify/**      # verification checks (proof-code drift class)

3 (codegen + rendering):
  - crates/spar-codegen/**     # any output emission
  - crates/spar-render/**      # report generation
  - crates/spar-wasm/**        # WASM output path
  - crates/spar-sysml2/**      # SysML v2 bridge

2 (base + wiring):
  - crates/spar-base-db/**     # salsa-style incremental DB
  - crates/spar-cli/**         # argv + env

1 (proof / constants):
  - crates/spar-verify-macros/**  # proof macros
  - **/verify/**

When ranking:
- Spar parses a real standard (AS-5506D / AADL v2.2/v2.3). Like any
  standard-parser, the attack surface is "accept every valid model,
  reject every invalid one." Silent acceptance of invalid AADL is a
  crown-jewel bug.
- The solver phase is the Loom/synth-equivalent: it MUST preserve
  semantics (feasibility, timing constraints). A miscalculation
  there produces a deployment that silently violates requirements.
- 180 Kani proofs exist in the codebase — coverage does not mean
  absence. Proof-code drift is a finding class here as in loom/synth.
- If a file straddles two tiers, pick the higher.
- Files you haven't seen default to rank 2.
- Do not guess rank 5 from path alone — open the file.
