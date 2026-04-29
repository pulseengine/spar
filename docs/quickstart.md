# Quickstart

Spar is a Rust toolchain for [AADL](https://www.sae.org/standards/content/as5506d/) v2.2/v2.3 — the Architecture Analysis & Design Language used to describe embedded and safety-critical systems (threads, processes, processors, buses, ports, latency budgets). Spar parses AADL, builds an instance model, runs the analysis pipeline (timing, scheduling, port-binding, latency, modes, properties), and emits SVG/Mermaid diagrams plus JSON for downstream tools (Bazel, Lean, MCP).

In ~30 minutes you will: build spar, parse a real model, instantiate it, run analyses, and render a diagram.

## 1. Build (≈ 2 min on a modern laptop)

```sh
git clone https://github.com/pulseengine/spar.git
cd spar
cargo build --release -p spar
export PATH="$PWD/target/release:$PATH"
```

No system dependencies beyond a stable Rust toolchain.

## 2. Parse, instantiate, analyze, render

The repo ships `test-data/vehicle.aadl` (a small vehicle compute platform) and its dependency `test-data/sensor_lib.aadl`. AADL fully-qualified names follow `Package::Type.Implementation`.

```sh
# 1. Parse — checks syntax + AS5506 legality, emits diagnostics
spar parse test-data/vehicle.aadl test-data/sensor_lib.aadl

# 2. Instantiate — flattens the system rooted at an implementation
spar instance --root 'Vehicle::ECU.basic' \
  test-data/vehicle.aadl test-data/sensor_lib.aadl

# 3. Analyze — runs the analysis pipeline (timing, ports, latency, modes…)
spar analyze --root 'Vehicle::ECU.basic' \
  test-data/vehicle.aadl test-data/sensor_lib.aadl

# 4. Render — SVG of the instance hierarchy
spar render --root 'Vehicle::ECU.basic' -o vehicle.svg \
  test-data/vehicle.aadl test-data/sensor_lib.aadl
```

Diagnostics are colored and anchor each finding to the AS5506D section it enforces.

## 3. Troubleshooting

- **"unresolved implementation X" / "unknown package Y"** — you forgot to pass the file that declares package Y. Spar does not auto-resolve `with` imports from a search path; pass every `.aadl` file involved on the command line.
- **A `test-data/*.aadl` file fails to parse** — `vehicle.aadl` + `sensor_lib.aadl` are the recommended starting pair.

## 4. Where to go next, by role

**Embedded / RTA engineer** — read `docs/cli/moves.md` for the move/RTA workflow. The `Spar_Timing` property set (period, deadline, WCET, jitter, priority) is what feeds rate-monotonic and EDF schedulability.

**TSN network architect** — see `crates/spar-network/`. WCTT (network) and WCET (compute) alternate per hop in the latency analyzer; TAS + CBS + 802.1Qbu preemption are modeled but the closed-form bounds are not yet Lean-verified (see "What this is NOT").

**AI agent / MCP user** — start `spar-mcp` (stdio server) and wire it into Claude Desktop or your MCP client. Exposed tools: `spar.verify_move`, `spar.enumerate_moves`, `spar.check_chain`. See `crates/spar-mcp/`.

**Safety engineer** — `spar verify` produces a JSON evidence pack; combine with `proofs/` for the formally checked subset. Note: EMV2 analysis is currently structural-only; full annex consumption is on the v0.10 roadmap.

**SysML v2 user** — `crates/spar-sysml2/` parses SysML v2 and lowers to AADL per the SEI 2023 mapping rules.

**PLE / variant engineer** — variant filtering is driven by rivet (Shape 1: rivet owns the PLE, emits a JSON context, spar filters HIR). See `docs/contracts/rivet-spar-variant-v1.md`.

## What this is NOT

- **Not a simulator.** Spar is static analysis + model transformation. Co-simulation lives elsewhere (Renode, FMI).
- **Not a fully verified tool.** The Lean proofs at v0.9.1 cover the rate-monotonic / RTA core (RTA, RTAJittered, EDF, RMBound — all sorry-free); network-calculus closed forms in MinPlus.lean are still informational at this version.
- **Not real CTF.** `spar-insight`'s "Tier 1 CTF" is a bespoke textual subset for development; full LTTng / `babeltrace2` ingestion is a v0.9.x follow-up.

## References

- Repo: <https://github.com/pulseengine/spar>
- Standard: SAE AS5506D (AADL v2.3)
- Sample models: `test-data/vehicle.aadl`, `test-data/sensor_lib.aadl`
- CLI docs: `docs/cli/`
- VS Code extension: `vscode-spar/`
- MCP server: `crates/spar-mcp/`
- SysML v2 frontend: `crates/spar-sysml2/`
