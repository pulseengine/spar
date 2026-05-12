# Forge-replication results — spar

Status: **template, pending trials.**
Last update: 2026-05-10.

This file is the reporting template for a Forge-style controlled
experiment on spar. The protocol is documented in
[../forge-replication-protocol.md](../forge-replication-protocol.md).
The scaffolding (descriptor, tasks, runner) is shipped under
`docs/research/forge-replication/`. **No trials have been run.** Fill
in each section after the user runs `run_trial.sh` across all tasks
and conditions.

---

## Pre-registration

- Date pre-registered: _pending_
- Hypothesis (H1): Conditioning a Claude Code agent with the
  `descriptor.sexp` (Condition C1) reduces the number of
  navigation-class tool calls relative to the no-descriptor control
  (Condition C0) when locating a known `file:line` in the spar repo.
- Null (H0): No difference in tool-call count between C0 and C1.
- Primary test: Wilcoxon signed-rank, two-sided, α = 0.05.
- Secondary metrics: `found_correct` (binary), wall time (seconds).
- Direction: one-tailed expectation that C1 has fewer steps; we still
  report two-sided p as primary.

## Conditions

- Agent: _pending_ (record exact model + temperature, e.g. Claude
  Sonnet 4.6 / 4.7, temp=0).
- spar commit pin (`tasks.json::target_commit`): `bad85e6`
  (v0.9.3-tip).
- Descriptor format(s) tested:
  - C0 — no descriptor.
  - C1 — `descriptor.sexp`.
  - Optional: C2/C3/C4 — JSON / YAML / Markdown variants.
- Number of trials per (task, condition) cell: _pending_ (default 1).

## N

- Tasks: 24 (per `tasks.json`).
- Conditions: _pending_ (default C0 + C1).
- Total observations: _pending_.
- Exclusions: _pending_ (record any task where ground truth proved
  ambiguous or the agent crashed).

## Wilcoxon p

- Test statistic T: _pending_.
- Sample size N: _pending_.
- p-value (two-sided): _pending_.
- Effect direction: _pending_.

## Cohen's d

- Paired-difference mean: _pending_.
- Paired-difference SD: _pending_.
- d = _pending_.
- Interpretation: _pending_ (Cohen 1988: small=0.2, medium=0.5,
  large=0.8; Jin paper reports d=0.92 on the analogous arm).

## Accuracy

- C0 mean `found_correct`: _pending_.
- C1 mean `found_correct`: _pending_.
- Difference (% points): _pending_.

## Discussion

_pending — fill in after results are in_

Suggested topics:

- Did the effect direction match Jin's published 33–44 % reduction?
- Was the effect concentrated in `hard` tasks (the descriptor
  presumably matters most where deep navigation would otherwise be
  required)?
- Were there tasks where C1 was *worse* than C0? If so, why — was
  the descriptor's claim for that surface inaccurate or stale?
- Threats to validity (self-target confound; possible training-set
  contamination of the model on spar; hand-curated descriptor).
- Recommended next replication batch: different repo, different
  agent, auto-generated descriptor.

## Raw data

The trial runner writes `results.csv` in this directory with one row
per trial. Transcripts (one per trial) live under `transcripts/`.
Neither is committed by default — re-running trials regenerates them.
The user should commit a frozen `results.csv` + `transcripts/`
tarball when reporting numbers.
