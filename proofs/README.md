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

The workflow also runs a post-build "fail on sorry" gate (a `grep`
over `proofs/Proofs/`) that turns CI red when any line is a bare
`sorry`. The previous gate relied on `lake build` itself failing on
`sorry`, which only happens with explicit `warningAsError`
configuration that the project does not currently set — so green CI
was decorative. The post-build grep makes the gate honest.

## Known sorrys (tracked in COMPLIANCE.md)

The Network Calculus closed-form bounds in
`proofs/Proofs/Network/MinPlus.lean` carry **four** unsorried theorems
(was five in v0.9.1; the `arrival_at_zero` mismatch was reconciled in
v0.9.2 — see below). They are listed below at file:line with one-line
context. They are tracked as `TODO(v1.0.0)` in `COMPLIANCE.md`, and
the post-build "fail on sorry" gate in `.github/workflows/proofs.yml`
turns CI red until they are discharged. Discharging is scoped to a
separate v0.10 PR — the math is non-trivial Le Boudec & Thiran ch. 1
closed-form reasoning in min-plus algebra.

| File:line | Theorem | One-line context |
|-----------|---------|-------------------|
| `Proofs/Network/MinPlus.lean:189` | `backlog_bound_classical` | Closed-form backlog `B = σ + ρ·T` for affine α (no peak cap) and stable rate-latency β; needs case split `t ≤ T` vs `t > T` plus `Nat.div` arithmetic with `h_stable`. The classical real-line proof shows `sup_t (α(t) − β(t))` is reached at `t = T`. |
| `Proofs/Network/MinPlus.lean:219` | `delay_bound_classical` | Closed-form delay `D = T + σ/R` (Le Boudec & Thiran horizontal-distance argument). In integer arithmetic this reduces to chasing `div_ceil` bounds across the affine ↔ rate-latency intersection point. |
| `Proofs/Network/MinPlus.lean:260` | `output_dominates_input` | The closed-form output curve dominates the input curve at every `t`: burst inflates by `ρ·T`, sustained rate preserved, so `α'(t) ≥ α(t)`. Le Boudec & Thiran 1.4.3. |
| `Proofs/Network/MinPlus.lean:313` | `compose_delays_dominates` | Naive serial-chain composition: `α(t) ≤ β2(β1(t + D_naive))`. Chains `delay_bound_classical` per hop through `output_dominates_input`; trivial *after* the per-hop sorrys above are discharged. |

### Discharged in v0.9.2

- `arrival_at_zero_is_zero` (was `arrival_at_zero_is_burst` at MinPlus.lean:318): the v0.9.1 spec/impl mismatch (Lean gave `min(σ, 0) = 0` while Rust short-circuited to `σ`) was reconciled by aligning **toward the Lean spec** (causality: a zero-length window admits zero bytes, regardless of σ). The Rust `ArrivalCurve::at` no longer short-circuits to σ at `t = 0`; the Lean spec adds a matching `if t = 0 then 0` short-circuit so the affine-no-peak branch is also 0; the proof discharges via `simp`. Theorem renamed `arrival_at_zero_is_zero` and the Rust unit test renamed `arrival_curve_at_zero_is_zero`.

The Lean tree is **load-bearing** for `RTAJittered` / `RTA` / `EDF` /
`RMBound` (Liu & Layland 1973, Joseph & Pandya 1986, Dertouzos 1974
proofs in `Proofs/Scheduling/*` are fully discharged, no `sorry`s).
The Lean tree is **informational** for the Network Calculus bounds at
v0.9.1: the Rust `crates/spar-network/src/curves.rs` is the
load-bearing artifact for the integer-arithmetic side, validated by
unit tests against published worked examples; the Lean theorems
encode the spec but await formal discharge.
