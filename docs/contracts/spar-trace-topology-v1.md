# spar-trace-topology external contract, v1

Status: **proposed** — stabilises when the v0.11.0 reconciliation
engine ships. v0.10.0 publishes the property surface and the
predicate URL only; the engine and SARIF emitter land in v0.11.0,
and the signed in-toto envelope lands in v1.0.
Last update: 2026-04-28.

## Purpose

Define the interchange surface between `spar trace topology` (the
runtime/declared topology reconciler) and external integrators
(rivet variant pipelines, witness verification kits, OEM-internal
certification toolchains).

`spar trace topology` consumes the runtime artefact set an OEM
produces from a real deployment — PCAPNG captures, LLDP topology
snapshots, Qcc YANG switch configs, tc/ethtool dumps, gPTP
synchronization logs — and reconciles them against the AADL
declaration of "what should be on the wire". The output is a SARIF
2.1.0 finding stream plus a signed in-toto v1.0 attestation envelope.

The architecture mirrors the rivet ↔ spar variant binding contract
(`docs/contracts/rivet-spar-variant-v1.md`): spar owns the
deterministic check; external readers consume the certified output;
no party crosses into the certified path.

## Predicate URL

```
https://pulseengine.eu/spar-trace-topology/v1
```

This URL is the in-toto v1.0 attestation predicate type. v1 readers
MUST recognise this exact URL; v2+ readers MAY add support for
successor URLs (`/v2`, `/v3`, …). v1 readers MUST refuse predicate
bodies whose declared predicate type differs from the v1 URL — that
is the correct behaviour per the same forward-compatibility pattern
the variant contract uses.

The URL is referenced — but not yet served as a JSON Schema document
— as of v0.10.0. The machine-readable schema lands alongside v1
stabilisation as
`docs/contracts/spar-trace-topology-v1.schema.json`.

## Input artefact list

A v1 reconciliation run consumes:

| Artefact | Format | Source |
|---|---|---|
| L2 frame capture | PCAPNG | `tcpdump`, `tshark`, TAP/SPAN port |
| LLDP topology snapshot | `lldpctl -f xml` / lldpd JSON / extracted from PCAPNG | runtime LLDP exchange |
| Switch configuration | Qcc YANG via NETCONF/RESTCONF; tc / ethtool dumps as supplementary fallback | NETCONF client / runtime tooling |
| gPTP synchronization log | `ptp4l` summary / `pmc` JSON / CTF events | linuxptp / Zephyr gPTP stack |
| Build-recorded image digests | JSON sidecar | build pipeline |
| AADL declaration | AADL v2.2/v2.3 with `Spar_Identity::*` and `Spar_TSN::*` annotations | spar-parsable model |

Out of scope for v1 (v1 readers MUST refuse with a clear migration
message):

- PCAP-classic (libpcap legacy);
- BLF (Vector binary log);
- OPC-UA captures;
- deep packet inspection / payload reconstruction.

See `docs/designs/v0.10.0-trace-topology.md` §"Out-of-scope for v1"
for the rationale.

## Output

### SARIF stream

`spar trace topology` emits a SARIF 2.1.0 log on stdout (or to a
target path with `--output`). The log carries one rule per finding
kind:

| Rule id | Maps to | Level |
|---|---|---|
| `spar-trace-topology/v1/IdentityUnknown` | `ReconcileFinding::IdentityUnknown` | error |
| `spar-trace-topology/v1/TopologyMissingWiring` | `ReconcileFinding::TopologyMissingWiring` | error |
| `spar-trace-topology/v1/ConfigDrift` | `ReconcileFinding::ConfigDrift` | error |
| `spar-trace-topology/v1/GptpOutOfBudget` | `ReconcileFinding::GptpOutOfBudget` | error |
| `spar-trace-topology/v1/BinaryMismatch` | `ReconcileFinding::BinaryMismatch` | error |

`tool.driver.name = "spar-trace-topology"`; `tool.driver.version`
carries the spar release; `tool.driver.informationUri` points back
at this contract's predicate URL.

### in-toto attestation envelope

`spar trace topology` emits an in-toto v1.0 envelope alongside the
SARIF stream:

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "predicateType": "https://pulseengine.eu/spar-trace-topology/v1",
  "subject": [
    { "name": "<aadl-model>", "digest": { "sha256": "..." } }
  ],
  "predicate": {
    "spar_trace_topology_version": "1",
    "verifier": "spar-trace-topology X.Y.Z",
    "inputs": {
      "pcapng":     { "digest": { "sha256": "..." } },
      "lldp":       { "digest": { "sha256": "..." } },
      "switch_yang": { "digest": { "sha256": "..." } },
      "ptp_log":    { "digest": { "sha256": "..." } }
    },
    "report": {
      "findings": [ ... ]
    },
    "verified": <bool>
  }
}
```

`verified` is `true` iff `report.findings` is empty. v1 readers
MUST treat the envelope as authoritative *only* when the signature
verifies under a key the reader trusts.

The full JSON Schema for the predicate body lands as
`docs/contracts/spar-trace-topology-v1.schema.json` alongside v1
stabilisation. v0.10.0 forward-references this schema; the body
shape above is the canonical reference until then.

## Spar_Identity property surface (v0.10.0 published)

The reconciler reads the following properties from the AADL model:

| Property | AADL type | Applies to | Reconciles against |
|---|---|---|---|
| `Spar_Identity::MAC_Address` | `aadlstring` | device, processor | PCAPNG MAC; LLDP chassis-id |
| `Spar_Identity::VLAN_ID` | `aadlinteger 0 .. 4094` | connection, bus | 802.1Q tag; Qcc YANG |
| `Spar_Identity::Stream_Handle` | `aadlinteger` | connection | Qcc YANG stream-handle |
| `Spar_Identity::Multicast_Group` | `aadlstring` | connection | PCAPNG dest MAC; Qcc YANG |
| `Spar_Identity::LLDP_Chassis_Id` | `aadlstring` | device, processor, bus | LLDP chassis-id TLV |
| `Spar_Identity::LLDP_Port_Id` | `aadlstring` | bus access feature | LLDP port-id TLV |

These are non-standard property set entries registered with the
predefined property surface (no `with` import required), per
`crates/spar-hir-def/src/standard_properties.rs`. v1 readers MUST
treat them as the canonical declared identity.

## CLI contract (v0.11.0)

```
spar trace topology \
  --aadl spec.aadl \
  --pcapng capture.pcapng \
  --lldp lldp.xml \
  --switch-yang switches.json \
  --ptp ptp4l.log \
  --digests digests.json \
  --format sarif \
  --attestation out.intoto.jsonl
```

Exit codes:
- `0` — every check passed; `report.is_clean()`.
- `1` — at least one reconciliation finding raised.
- `2` — input parse failure (artefact missing, malformed, unsupported
  format).

v0.10.0 ships the foundation only — the CLI subcommand is not yet
wired. v0.11.0 lands the engine and the subcommand.

## Stability promise

- **v1 is published-but-not-stable until v0.11.0.** v0.10.0 ships
  the property surface (`Spar_Identity::*`) and the predicate URL;
  changes between v0.10.0 and v0.11.0 are still possible. v1 freezes
  at the v0.11.0 release; subsequent v1.x releases preserve the wire
  format.
- **v1 readers MUST refuse v2+ blobs.** Per the variant-contract
  pattern, predicate types under `https://pulseengine.eu/spar-trace-topology/v2`
  (and beyond) MUST be rejected by v1 readers — v2 may break the
  predicate body shape.
- **Adding optional fields to the predicate body is not breaking.**
  v1 readers MUST ignore predicate-body fields they do not recognise,
  to allow forward-compatible additions.
- **Adding new finding kinds is breaking** — it bumps the predicate
  URL.
- **Removing a finding kind is breaking** — same rule.
- **Renaming or retyping a `Spar_Identity::*` property is breaking**
  — same rule.

## Validation responsibilities

- spar (the verifier) is responsible for:
  - Schema validation of every input artefact before reconciling.
  - Producing per-finding SARIF entries with correct `ruleId` and
    `level` (always `error` per §"SARIF stream").
  - Producing the in-toto envelope with the canonical input-digest
    set.
  - Refusing v2+ predicate types in any envelope it ingests for
    cross-checking.
- External readers (rivet, witness, OEM) are responsible for:
  - Verifying the in-toto signature under their trust root before
    consuming the predicate body.
  - Refusing predicate types other than `/v1` in v1 mode.
  - Treating `verified: true` as the *only* certifying claim — the
    presence or absence of findings is the ground truth, not any
    summary string.

## Out of scope for v1

- Time-series reconciliation (per-window sub-findings).
- Variant-aware reconciliation (`--variant-context` integration).
- Application-layer / payload-level reconciliation.
- Sigstore key custody for air-gapped operators (deferred to a v1.0
  follow-up `--emit-unsigned` flag).

## References

- `docs/designs/v0.10.0-trace-topology.md` — full v1 design.
- `docs/contracts/rivet-spar-variant-v1.md` — sibling contract,
  same shape.
- IEEE 802.1AB / 802.1AS / 802.1Q / 802.1Qbv / 802.1Qcc.
- SARIF 2.1.0 (OASIS).
- in-toto v1.0 / sigstore.
