# spar fuzz targets

`cargo-fuzz` harnesses for the parser, MILP scheduler/allocator, and codegen
pipeline. Satisfies issue [#138](https://github.com/pulseengine/spar/issues/138).

## Targets

| target                     | surface                             | requirements traced               |
|----------------------------|-------------------------------------|-----------------------------------|
| `fuzz_aadl_parse`          | `spar_syntax::parse`                | REQ-PARSE-001, REQ-PARSE-002, REQ-PARSER-001 |
| `fuzz_scheduler_solver`    | `spar_solver::milp::solve_milp`     | REQ-SOLVER-001, REQ-SOLVER-003, REQ-SOLVER-005 |
| `fuzz_codegen_roundtrip`   | `spar_codegen::generate`            | REQ-CODEGEN-001, REQ-CODEGEN-WIT, REQ-CODEGEN-RUST |

Each target asserts only **"no panic, no hang"** — `Err` returns from the
solver are legitimate (infeasible task sets), parse errors are legitimate
(malformed input), and varying codegen configs must all succeed on the
fixed fixture.

## Running locally

Requires nightly Rust and `cargo-fuzz`:

```sh
cargo install cargo-fuzz
```

From the repo root:

```sh
# Quick smoke (60 s per target, matches CI PR gate)
cargo +nightly fuzz run fuzz_aadl_parse        -- -max_total_time=60
cargo +nightly fuzz run fuzz_scheduler_solver  -- -max_total_time=60 -timeout=5
cargo +nightly fuzz run fuzz_codegen_roundtrip -- -max_total_time=60

# Extended (1 h per target, matches nightly cron job)
cargo +nightly fuzz run fuzz_aadl_parse        -- -max_total_time=3600
```

Build-only (no execution, useful for CI caching):

```sh
cargo +nightly fuzz build
```

## Time budgets

| context                | per-target wall time | notes                              |
|------------------------|----------------------|------------------------------------|
| local smoke / dev loop | 10-60 s              | quick regression gate              |
| CI `fuzz-smoke` (PR)   | 60 s                 | `-max_total_time=60`, no corpus persist |
| CI `fuzz-nightly`      | 3600 s (1 h)         | cron daily 03:00 UTC, corpus uploaded as artifact |

The `fuzz_scheduler_solver` target passes `-timeout=5` so any single MILP
call that blocks for more than five seconds is reported as a hang — this
is the "non-termination" guard the issue body calls out.

## Corpus

Corpora live under `fuzz/corpus/<target>/` and are `.gitignore`d (libfuzzer
writes and mutates them at runtime). The nightly workflow uploads the
corpus directory as a build artifact so it can be reused across runs and
seeded into the criterion benchmark's worst-case input collection.

Seed inputs can be added by dropping files into `fuzz/corpus/<target>/`
before the run.
