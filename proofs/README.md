# Spar Lean Proofs

Machine-checked proofs of the mathematical theorems underlying spar's
scheduling analysis (`crates/spar-analysis/src/scheduling.rs`). These
proofs verify the **theory**, not the code.

## Verified theorems

| File | Theorem | Citation |
|------|---------|----------|
| `Proofs/Scheduling/RMBound.lean` | Rate-monotonic utilization bound | Liu & Layland 1973 |
| `Proofs/Scheduling/RTA.lean`     | Response-time analysis fixed-point convergence | Joseph & Pandya 1986 |
| `Proofs/Scheduling/EDF.lean`     | EDF optimality for implicit deadlines | Dertouzos 1974 |

## Building

```bash
cd proofs
lake build
```

The first build downloads Mathlib and takes 30+ minutes on a fresh
toolchain. Subsequent builds reuse `.lake/build` and finish in seconds.

## Toolchain pinning

The Lean toolchain is pinned in [`lean-toolchain`](lean-toolchain). The
current pin is:

```
leanprover/lean4:v4.29.0-rc6
```

This is the single source of truth for the Lean version. CI
(`.github/workflows/proofs.yml`) reads this file via
`leanprover/lean-action@v1`; bumping the pin is a one-line change.

The Bazel rules under `tools/bazel/rules_lean` are designed to
interoperate with `rules_lean` 4.27.0 (per issue #135 notes); when a
root `MODULE.bazel` adopts those rules it must register a toolchain
that resolves `lean` and `lake` from the same elan-managed toolchain
this directory uses.

Mathlib and other transitive deps are pinned by exact git revision in
[`lake-manifest.json`](lake-manifest.json). To update the dep set, run
`lake update` and commit both `lakefile.toml` and the regenerated
manifest.

## CI

The proofs are typechecked on every PR and main push by
[`.github/workflows/proofs.yml`](../.github/workflows/proofs.yml).
The Lake build directory (Mathlib + transitive olean output) is cached
keyed on `lean-toolchain` + `lake-manifest.json`, so warm runs only
recompile in-tree changes.

On failure, per-target lake build logs are uploaded as the
`lake-build-log` workflow artifact for forensic review.
