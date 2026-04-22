You are emitting a new entry to spar's safety artifact store.
Consult `safety/stpa/` for the existing shape. Spar's safety
directory uses several analysis files (`analysis.yaml`,
`validation.yaml`, `security.yaml`, `solver-analysis.yaml`) — pick
the one whose category best fits the finding, or the equivalent of
loss-scenarios if it exists.

Input:
- Confirmed bug report (below)
- Chosen `UCA-N` from the validator
---
{{confirmed_report}}
UCA: {{uca_id}}
---

Rules:
1. Read `safety/stpa/` first. Match the existing field shape
   exactly. Do not invent fields.
2. Grouping invariant: new entries are siblings of existing ones
   under the same UCA.
3. In the prose, reference the Kani harness and the PoC test by
   fully-qualified Rust path. Cite the AS-5506D section the bug
   violates (e.g., "AADL v2.2 §11.2.1 feature-group mapping").
4. If this is proof-code drift, say so explicitly — drift requires
   re-verification or code reversion, not a primitive fix.
5. Optional: classify the finding under `category` with one of:
   parser-over-acceptance, parser-under-acceptance, hir-drift,
   analysis-unsoundness, solver-miscount, proof-drift,
   codegen-divergence. Use the existing schema if present.
6. Set `status: draft`. Deployments built on spar output must not
   consume draft findings until a human promotes them.

Emit ONLY the artifact YAML block, nothing else.
