# rivet ↔ spar variant binding contract, v1

Status: **proposed** — stabilizes once both sides implement.
Last update: 2026-04-23.

## Purpose

Define the interchange format and CLI contract by which `rivet` (the
source of truth for feature models and variant configurations) communicates
a resolved variant to `spar` (which filters its HIR against it before
running analyses).

**Architecture:** rivet owns the entire product-line model — feature
model, constraints, variant definitions, bindings, SAT resolution. spar
consumes a resolved context blob and restricts its HIR accordingly. spar
does **not** parse rivet artifacts; spar does **not** solve feature
constraints.

AADL/SysML v2 source files stay variant-agnostic — the same file compiles
in every variant. Binding decisions live outside the source.

## The variant context blob

Rivet emits a single JSON document per resolved variant. spar reads it
via stdin or a file path.

### Example

```json
{
  "rivet_spar_context_version": "1",
  "variant": "diesel-eu5",
  "features": [
    "engine_diesel",
    "emissions_eu5",
    "platform_zephyr_v3",
    "target_cortex_m4"
  ],
  "bindings": [
    { "artifact": "spec/engines/diesel.aadl",   "requires": ["engine_diesel"] },
    { "artifact": "spec/engines/electric.aadl", "requires": ["engine_electric"] },
    { "symbol":   "Engines::Engine.Diesel",     "requires": ["engine_diesel"] }
  ],
  "feature_model_hash": "sha256:abc123...",
  "resolved_at": "2026-04-23T12:00:00Z",
  "generated_by": "rivet 0.3.x"
}
```

### Fields

| Field | Required | Type | Meaning |
|---|---|---|---|
| `rivet_spar_context_version` | yes | string | Contract version. v1 readers MUST accept `"1"`; MAY reject other values. |
| `variant` | yes | string | Name of the resolved variant — matches a `variants/<name>.yaml` in rivet. |
| `features` | yes | `string[]` | Flat list of feature names active in this variant. Order-insensitive. Duplicates MUST NOT appear. |
| `bindings` | yes | `Binding[]` | See below. May be empty if the project has no variant-specific artifacts. |
| `feature_model_hash` | yes | string | Stable hash of the feature model that produced this resolution. spar uses it as a salsa cache key. |
| `resolved_at` | yes | string (RFC 3339) | Timestamp of resolution. For audit trails only. |
| `generated_by` | yes | string | Emitter tool + version. For diagnostics only. |

### Binding shape

A binding is either file-scoped (`artifact`) or symbol-scoped (`symbol`),
never both in the same entry:

```
Binding = { "artifact": string, "requires": string[] }
        | { "symbol":   string, "requires": string[] }
```

- `artifact` — path to a source file, relative to the project root.
- `symbol` — fully-qualified AADL name, shape `Package::Type` or
  `Package::Type.Implementation`.
- `requires` — list of feature names that MUST all be present in
  `features` for the bound item(s) to be included. Empty list means "no
  feature requirement" — equivalent to no binding at all.

## Binding resolution semantics

For each HIR item, spar checks every binding in the context:

1. Determine whether the binding *matches* the item:
   - `artifact` binding matches iff the item's source file equals the
     binding's `artifact` path (after normalization).
   - `symbol` binding matches iff the item's fully-qualified name equals
     the binding's `symbol`, **or** the item is declared textually
     inside the body of a matching symbol (connections, properties,
     mode specifications, subcomponents).
2. Collect all matching bindings.
3. The item is **kept** iff for every matching binding,
   `binding.requires ⊆ features`.
4. An item with zero matching bindings is kept unconditionally — it's
   variant-independent infrastructure.

### Why intersection, not union

Multiple bindings matching the same item — e.g. a file-scoped binding
and a symbol-scoped binding both targeting the same type — are treated
as conjunctive: **all** required-feature sets must be satisfied. This is
the conservative choice: adding a stricter binding can only remove the
item, never reintroduce it.

A future `v2` contract may expose a `mode` field per binding to select
union semantics explicitly. v1 is intersection-only.

### Symbol granularity (contents included)

A binding on `Engines::Engine.Diesel` applies to:

- the `type` or `implementation` declaration itself,
- every subcomponent, connection, property assignment, mode, and flow
  spec textually nested inside its body.

It does **not** apply to classifiers that merely `extends` the bound
symbol. Inheritance is orthogonal to variant binding.

## CLI contract

### Explicit form (auditor-friendly, CI-friendly)

```
rivet resolve --variant diesel-eu5 --format spar-context-json > ctx.json
spar check --variant-context ctx.json spec.aadl
```

spar MUST accept `--variant-context <path>` where `<path>` is a file, or
`-` for stdin. spar MUST validate `rivet_spar_context_version` and
produce a clear error for unknown versions.

### Implicit form (developer-friendly)

```
spar check --variant diesel-eu5 spec.aadl
```

When `--variant` is passed without `--variant-context`, spar invokes
rivet internally:

1. Locate rivet: `$RIVET_BIN` if set, otherwise first `rivet` on `$PATH`.
2. Invoke `rivet resolve --variant <name> --format spar-context-json`.
3. Consume the emitter's stdout as if passed via `--variant-context`.
4. If rivet is not discoverable, spar errors with a message pointing
   to this contract doc and suggesting the explicit form.

### Matrix mode

```
spar variants matrix spec.aadl
```

spar enumerates variants via `rivet variants list --format json`
(emitting a list of variant names), then runs the equivalent of
`spar check --variant <each> spec.aadl` per variant. Per-variant
diagnostics are written under `target/spar/variants/<name>/`;
a top-level coverage table is written to
`target/spar/variants/matrix.json`.

### Rivet CLI surface spar depends on

To honor this contract, rivet MUST provide:

| Command | Output | Required |
|---|---|---|
| `rivet resolve --variant <name> --format spar-context-json` | The blob above, on stdout. | yes |
| `rivet variants list --format json` | `{"variants": ["<name>", ...]}` on stdout. | yes (for matrix mode) |
| Non-zero exit on resolution failure (unknown variant, unsat constraints) | with a human-readable error on stderr | yes |

No other spar command invokes rivet. spar never parses
`feature-model.yaml` or `variants/<name>.yaml` directly.

## Compatibility and versioning

- **v1 is stable.** Fields listed above will not be removed or have their
  meaning changed in v1 emitters or readers.
- **v2+ may add fields.** Readers MUST ignore unknown fields they do not
  understand, to allow forward-compatible extensions.
- **v2+ that changes semantics** MUST bump `rivet_spar_context_version`
  and be announced as a breaking change. spar v1 readers will refuse
  v2 blobs that declare their version as `"2"`; that is correct behavior.
- Adding new required fields is a breaking change.

## Validation responsibilities

- Rivet is responsible for:
  - Feature model well-formedness and constraint satisfaction.
  - Ensuring every `bindings[*].requires` entry references a feature
    name that exists in the feature model. spar does not cross-check.
  - Detecting and reporting unknown variant names before resolution.
- spar is responsible for:
  - Schema validation of the incoming context blob.
  - Reporting when a binding's `artifact` path does not resolve to a
    loaded source file. (Warning, not error: the file may simply not be
    part of this analysis run.)
  - Reporting when a `symbol` binding matches no declared symbol.
    (Warning.)
  - Producing per-variant diagnostics that include the `variant` name
    for traceability.

## Out of scope for v1

- Per-mode variant semantics (AADL mode ↔ variant interaction).
- Runtime variant switching (v1 variants are resolved at analysis
  time, not deployment time).
- Property-value overrides by variant (e.g. "in `diesel-eu5`, WCET of
  Handler.brake is 300µs"). If needed, add as v2 via a new
  `overrides` field.
- Nested variant contexts (variant-of-variant).

## Open questions, tracked for v1 finalization

None blocking. Two items deferred to v2 if needed:

1. Union-semantics bindings (`mode: "union"`). Not needed for v1 use cases.
2. Property overrides (see Out-of-scope).

## References

- This doc's machine-readable JSON Schema will land alongside v1
  stabilization as `docs/contracts/rivet-spar-variant-v1.schema.json`.
- Companion rivet documentation lives in the rivet repo under
  `docs/contracts/` (mirrored).
