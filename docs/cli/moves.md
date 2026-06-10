# `spar moves` — hypothetical-rebinding oracle

Available since v0.8.0. `spar moves` answers questions of the form
*"if I move component X to processor Y, would my system still close
its deadlines / bandwidth budgets / frozen-platform contracts?"* —
without mutating any source file.

There are two subcommands today: `verify` (single-target check) and
`enumerate` (list valid alternatives ranked by an objective).

## `spar moves verify`

Verifies a single hypothetical rebinding. Returns structured pass/fail
JSON with violations.

### Usage

```
spar moves verify \
    --root Pkg::Sys.Impl \
    --component Pkg::Sys.Impl.handler \
    --to Pkg::CPU_x86 \
    [--format json | text] \
    [--variant NAME | --variant-context PATH] \
    model.aadl
```

### Flags

| Flag | Purpose |
|---|---|
| `--root <FQN>` | Root system implementation, e.g. `Engines::Top.Battery` |
| `--component <FQN>` | Fully-qualified name of the component to (hypothetically) move. Must be Mobile (`Spar_Migration::Mobile => true`) or unannotated. |
| `--to <FQN>` | Target processor. Must be a processor / virtual processor instance reachable from the root. |
| `--format` | `text` (default; human-readable) or `json` (machine-readable, schema below). |
| `--variant <NAME>` | Implicit form: shells out to `rivet resolve --variant NAME --format spar-context-json`. Requires `rivet` on `$PATH` or `$RIVET_BIN`. |
| `--variant-context <PATH>` | Explicit form: read JSON from `PATH` (`-` for stdin). Mutually exclusive with `--variant`. |

### Exit codes

- `0` — OK, no violations.
- `1` — Analysis-error severity diagnostic produced (e.g. RTA reports a
  deadline miss after the move).
- `2` — Binding-violation detected (Frozen / Allowed_Targets).
- non-zero from `1`/`2` — argument errors (missing component, unknown
  target, etc.) report descriptive text on stderr.

### Output (JSON)

```json
{
  "ok": false,
  "component": "Engines::Top.Battery.app.bh",
  "target": "Engines::Top.Battery.cpu_slow",
  "variant": null,
  "feature_model_hash": null,
  "violations": [
    { "kind": "AnalysisError",
      "pass": "rta",
      "severity": "Error",
      "message": "thread 'bh' on processor 'cpu_slow' misses deadline: response time 1.2 ms > deadline 1 ms" }
  ],
  "diagnostics_by_pass": { ... }
}
```

When `--variant`/`--variant-context` is set, `variant` and
`feature_model_hash` carry the resolved-variant metadata.

## `spar moves enumerate`

Lists every valid hypothetical rebinding target for a component, each
with its verification status and a configurable ranking metric.

### Usage

```
spar moves enumerate \
    --root Pkg::Sys.Impl \
    --component Pkg::Sys.Impl.handler \
    [--target-filter <FQN-PREFIX>] \
    [--objective max-response | total-load | total-power | total-weight | balanced] \
    [--format json | text] \
    [--variant NAME | --variant-context PATH] \
    model.aadl
```

### Candidate-set derivation

If `Spar_Migration::Allowed_Targets` is set on the component, that list
is the candidate set. Otherwise: every processor (or virtual processor)
component reachable from the root.

`--target-filter <PREFIX>` narrows the candidate set to entries whose
fully-qualified name starts with `<PREFIX>` *after* `Allowed_Targets`
has been applied — i.e. a filter cannot bypass the platform's
`Allowed_Targets` declaration.

### Ranking objectives

| Objective | Metric (lower = better) |
|---|---|
| `max-response` (default) | Maximum thread response time on the candidate target. Negative values indicate deadline miss. |
| `total-load` | Sum of utilization (`exec / period`) of threads bound under the candidate. |
| `total-power` | Sum of `Spar_Power::Power_Budget` for components bound under the candidate. |
| `total-weight` | Sum of `Weight_Properties::Weight`. |
| `balanced` | Equal-weight composite of the four metrics above. |

Candidates are sorted: `ok=true` first, then by score ascending (lower
is better), then by FQN.

### Output (JSON)

```json
{
  "component": "Engines::Top.Battery.app.bh",
  "objective": "max-response",
  "variant": null,
  "feature_model_hash": null,
  "total": 3,
  "valid": 2,
  "candidates": [
    { "target": "Engines::Top.Battery.cpu_fast",
      "ok": true,
      "violations": [],
      "diagnostics_count": 0,
      "rank": { "max_response_ns": 800000, "total_load": 0.4, ..., "score": 800000.0 } },
    ...
  ]
}
```

### Exit codes

- `0` — at least one valid candidate found.
- `1` — argument errors (unknown component, malformed flag).
- enumeration with zero valid candidates still exits `0`, with `valid: 0`.

## Worked example

```aadl
package Engines
public
  with Spar_Migration;

  thread Brake_Handler end Brake_Handler;
  thread implementation Brake_Handler.Impl
    properties
      Spar_Migration::Mobile           => true;
      Spar_Migration::Allowed_Targets  => (reference (cpu_fast), reference (cpu_safety));
  end Brake_Handler.Impl;

  processor M4 end M4;

  system Top end Top;
  system implementation Top.Battery
    subcomponents
      cpu_fast:   processor M4;
      cpu_safety: processor M4;
      cpu_legacy: processor M4;
      app_thread: thread Brake_Handler.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu_fast)) applies to app_thread;
  end Top.Battery;
end Engines;
```

```sh
$ spar moves enumerate \
    --root Engines::Top.Battery \
    --component Engines::Top.Battery.app_thread \
    model.aadl

(variant=none) component=Engines::Top.Battery.app_thread total=2 valid=2

  ok  target                                    score
  --  ----------------------------------------  ------
  ✓   Engines::Top.Battery.cpu_fast             0.80 ms
  ✓   Engines::Top.Battery.cpu_safety           0.80 ms
```

`cpu_legacy` is not listed because it isn't in `Allowed_Targets`.

## Use with rivet variants

```sh
$ spar moves verify \
    --variant diesel-eu5 \
    --component Engines::Top.Battery.app_thread \
    --to Engines::Top.Battery.cpu_safety \
    model.aadl
```

The `--variant diesel-eu5` flag invokes `rivet resolve --variant
diesel-eu5 --format spar-context-json` and applies the resulting binding
rules before the move is verified. Only items in the variant's
resolved set participate in the analysis.

## Integration target — MCP tool surface (v0.9.0)

The `--format json` shape is the canonical machine-readable form. In
v0.9.0 it will also be exposed as MCP tools `spar.verify_move` and
`spar.enumerate_moves` so LLM agents can drive design-space exploration
with spar as the deterministic correctness oracle. The tools will be
`readOnlyHint: true` and `idempotentHint: true`; the deterministic
apply path stays CLI-exclusive (no `spar.apply_move` over MCP) so the
certification chain remains in spar's existing analysis primitives.

## See also

- [`docs/contracts/rivet-spar-variant-v1.md`](../contracts/rivet-spar-variant-v1.md) — variant context blob format.
- [`docs/designs/track-e-migration-research.md`](../designs/track-e-migration-research.md) — research backing this surface.
