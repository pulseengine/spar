# codegen-kiln-oracle

Runtime oracle for **REQ-CODEGEN-WIT-DATAPLANE-KILN-001** — a spar-generated
component executed on the [kiln](https://github.com/pulseengine/kiln) interpreter
(a *second* runtime beside the wasmtime `codegen-exec-oracle`), closing the
"spar generates → kiln fires" dogfood loop on the target substrate.

Out-of-workspace (like `codegen-exec-oracle/` and `fuzz/`) so the heavy kiln +
meld dependency trees never poison the main workspace build. git-pinned to
kiln `c6bcaab` + meld `v0.40.0`.

## Why meld is in the loop

**kiln runs CORE wasm modules, not Component Model components.** Direct component
instantiation via `from_parsed_*` is a won't-fix path
([kiln#269](https://github.com/pulseengine/kiln/issues/269)) that still panics on
HEAD ([kiln#427](https://github.com/pulseengine/kiln/issues/427), filed from this
work). The blessed path (kiln RFC #46) is:

```
spar codegen → wasm32-wasip2 component → meld fuse → CORE module → kiln
```

[meld](https://github.com/pulseengine/meld) lowers the component to a core module,
flattening the `state-pkg:p/p-ports` interface into flat core `(module, field)`
imports that kiln drives via a `HostImportHandler` with typed `Value` args (no
manual linear-memory decoding needed for scalars).

## What it proves

The full chain runs mechanically (`cargo run --bin kiln-oracle`):

1. spar codegen of `test-data/codegen/state.aadl` → multi-crate wasip2 workspace;
2. `cargo build --target wasm32-wasip2` → component;
3. `meld fuse` **in-process** (meld-core `Fuser`) → CORE module;
4. kiln `CapabilityAwareEngine` (QM) loads + instantiates, with a chained
   `HostImportHandler` routing `wasi:*` → `WasiDispatcher` and the custom AADL
   ports → host state;
5. two proofs on **one live kiln instance** (same discriminators as the wasmtime
   state-oracle):
   - **persistence** — feed `acc-in` 5 then 7 across two `a-compute` dispatches;
     assert `set-acc-out == 12` (a fresh instance / call-local yields 7);
   - **inter-thread** — feed `src-in=42`, dispatch `pr-compute` then `cn-compute`
     separately; assert `set-sink-out == 42` (the thread_local buffer surviving
     between dispatches inside the one instance is under test).

Non-vacuity: reverting the accumulator's Out port to emit its input drops
persistence to 7; the spike sweep (`acc-in ∈ {5, 9, 123456}` each round-tripping
to the same `set-acc-out`) proves the guest computes the value, not the harness.

Gated in CI by the `codegen-oracle` job.
