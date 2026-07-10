# codegen-exec-oracle

Runtime data-flow oracle for **REQ-CODEGEN-WIT-DATAPLANE-001** — the load-bearing
proof that spar's generated data-plane marshalling actually moves typed data across
the WIT ABI at runtime, not just that it compiles.

It lives **outside the main workspace** (own `[workspace]`, like `fuzz/`) so its
heavy `wasmtime` dependency never poisons `cargo build`/`cargo test --workspace`.

## What it does

1. Generates the DATAPLANE crate via `spar_codegen::generate` (from
   `test-data/codegen/dataplane.aadl`).
2. Patches the `e` thread's `compute` into an **echo** (`dout = din`,
   `rec_out = rec_in`) — this stands in for *user logic*; the code under test is
   spar's generated marshalling glue, not the echo.
3. Builds it to a `wasm32-wasip2` **component**.
4. Instantiates it under wasmtime with a host implementing the imported `p-ports`
   interface, feeds sentinel-distinct values, calls `e-compute`, and asserts the
   exact values came back out.

Run it:

```sh
cargo run --manifest-path codegen-exec-oracle/Cargo.toml --bin exec-oracle
```

Requires the `wasm32-wasip2` target (`rustup target add wasm32-wasip2`). Gated in
CI by the `codegen-oracle` job.

## The committed `wit/dp.wit`

`wasmtime::component::bindgen!` needs the WIT at **compile time**, but `generate()`
produces it at **runtime** — so the host binds against the committed
`wit/dp.wit`. A fast test in spar-codegen
(`dataplane_wit_matches_exec_oracle_fixture`) asserts `generate()` reproduces this
file byte-for-byte, so the two cannot drift.

**If that test fails**, codegen's WIT output changed; regenerate the fixture from
the DATAPLANE process's `generate_wit` output and re-commit `wit/dp.wit`.
