# Forge-style architecture-descriptor replication protocol on spar

Status: **research protocol** — scaffolds a replication of Jin's
"Formal Architecture Descriptors" controlled experiment
(arXiv [2604.13108](https://arxiv.org/abs/2604.13108)) using spar
itself as the target codebase. The scaffold is here; the trials are
the user's job.
Last update: 2026-05-10.
Audience: spar maintainers, Forge-paper replicators, anyone trying to
quantify whether an AADL-style architecture descriptor reduces agent
navigation steps on a Rust codebase.

> **TL;DR.** spar already produces architecture descriptors (rivet
> YAML, AADL, SysML v2 round-trip) about itself. That gives us a
> rare eat-your-own-dogfood replication target: agent runs `find
> where X happens` queries against the spar repo with and without the
> existing `artifacts/architecture.yaml` + `descriptor.sexp` in
> context. We mirror Jin's protocol (N=24 tasks × 2 conditions,
> Wilcoxon signed-rank, Cohen's d) and ship the scripted runner,
> tasks file, descriptor, and an empty RESULTS template. **No
> numbers are reported here — only the protocol.**

---

## §1 Target choice and rationale

We pick **spar itself** as the target codebase.

| Candidate | LoC | Why considered | Why we rejected (if so) |
|---|---|---|---|
| `tokio-rs/mini-redis` | ~3k Rust | Tutorial-grade, very clean. | Too small; navigation already trivial, no headroom for a 33-44 % reduction to be observable. |
| `astral-sh/ruff` | ~280k Rust | Real-world Rust, well-organised. | Too large; descriptor generation cost dominates, and the "ground truth file:line" for tasks is harder to nail down without an existing architectural artifact. |
| **spar (self)** | ~120k Rust across 19 crates | Has a hand-curated `artifacts/architecture.yaml` (rivet) + an AADL fixture (`artifacts/spar.aadl` if we add one) describing its own crate-graph. Mid-size enough to make navigation cost real, small enough that ground truth is auditable. | Self-experimentation introduces a confound (the model may have been trained on this repo or similar). We accept the confound and pre-register the analysis. |

A self-referential replication is methodologically weaker than
testing on an unrelated repo, but stronger than no replication. The
core claim under test — *"agents handed a structured architecture
descriptor navigate faster than agents reading code blind"* — is
codebase-agnostic, so a single-target run can produce a usable effect
size estimate before the user commissions a multi-repo replication.

The user can swap targets by adjusting `run_trial.sh`'s
`TARGET_REPO` and supplying a different descriptor; the protocol is
target-independent.

## §2 Descriptor production

Three artefacts live under `docs/research/forge-replication/`:

1. **`descriptor.sexp`** — the canonical descriptor format. S-expression
   over the spar crate graph and the top-level types in each crate.
   Per Jin §"format comparison" (arXiv 2604.13108), S-expressions
   *"detect all structural completeness errors"* while JSON fails
   atomically and YAML silently corrupts ~50 % of errors. We make
   the S-expression form authoritative; JSON / YAML are derived if
   the user wants a format-comparison arm.
2. **`descriptor.aadl`** — an AADL stub describing spar's crate graph
   as nested `system` declarations. Not used by the trial runner
   directly, but kept for cross-checking with the rivet
   `artifacts/architecture.yaml` already in the repo.
3. **`tasks.json`** — N=24 code-localization tasks. Each task has a
   natural-language question, a ground-truth `file:line` answer, and
   a difficulty tag (`easy`, `medium`, `hard`).

The descriptor is hand-curated, not auto-generated. Auto-generation
of a Rust-source→architecture descriptor is a spar **roadmap** item
(SysML v2 emitter is currently rivet→SysML, see
`crates/spar-sysml2/src/generate.rs:153`); for the replication
protocol we accept the hand-curated form. Jin's §"artifact vs
process" used auto-generated descriptors against a Forge tool, so
this is a known deviation; the user can re-run with an
auto-generated descriptor once spar gains the emitter, and the
effect-size delta is itself an interesting reading.

## §3 Tasks

`tasks.json` carries 24 entries, balanced by difficulty (8 easy / 8
medium / 8 hard) and by spar subsystem (parser / analysis / network /
codegen / CLI / verify / docs). A representative subset:

- *Easy.* "Find where AADL `in modes (…)` lists are parsed into the
  HIR." → `crates/spar-hir-def/src/instance.rs:84` (the
  `in_modes` field on `ComponentInstance`).
- *Medium.* "Find the function that computes the WCET inflation for
  context switches." → `crates/spar-analysis/src/rta.rs` (the
  `Context_Switch_Time` integration introduced in #198, v0.9.2).
- *Hard.* "Find the predicate that decides whether a connection is
  active in a given SOM." →
  `crates/spar-analysis/src/modal.rs:82` (`is_connection_active_in_som`).

Each task is a one-liner — Jin's protocol is explicit that tasks
must be answerable from the descriptor alone if the descriptor
captures the right surface (arXiv 2604.13108 §"controlled
experiment"). Hard tasks are tasks where the answer is in a deep
sub-module that descriptor-blind navigation would have to grep its
way to.

## §4 Conditions

Minimum two arms:

- **C0 — no-descriptor (control).** Agent is given the task prompt
  and a clean working copy of the spar repo. No additional context.
- **C1 — with-descriptor (treatment).** Agent is given the task
  prompt, a clean working copy, and the `descriptor.sexp` file as
  pre-context.

Optional format arms (mirror Jin §"format comparison"):

- **C2 — JSON descriptor.** Same content as C1, JSON-serialised.
- **C3 — YAML descriptor.** Same content as C1, YAML-serialised.
- **C4 — Markdown descriptor.** Same content as C1, prose form.

We default to C0 vs C1 only; the user enables C2-C4 by adding their
descriptor variants and re-running `run_trial.sh`. Jin found
*"no significant format difference between S-expression / JSON /
YAML / Markdown"* on the navigation-step metric, but a significant
difference on error detection — so the format arms are optional and
the user's choice.

## §5 Metric

**Navigation steps.** Count of tool calls the agent emits before
producing a final answer with the correct `file:line`. Tools that
count:

- `Read` (full or partial file read);
- `Bash` calls that include `grep`, `rg`, `find`, `ls` (heuristic:
  any Bash invocation whose first token after `cd …;` is one of
  those);
- `Glob` calls.

Tools that do **not** count: `Write`, `Edit`, the final reply itself,
and meta-calls (`gh`, `cargo`). This matches Jin §"controlled
experiment", which counts only navigation-class tool calls.

The runner outputs `n_tool_calls` per trial. Wall time is recorded
as a secondary metric.

## §6 Statistical plan

Per Jin §"controlled experiment" we adopt Wilcoxon signed-rank as
the primary test, with N=24 paired observations (each task is run
once under C0 and once under C1; the pair is the unit of analysis).

For Wilcoxon signed-rank with N=24 paired observations, p<0.05
two-sided requires the signed-rank sum to lie outside the critical
region (T ≤ 81 for N=24, two-sided α=0.05; standard tables). In
practical terms this corresponds to roughly 16-20 of the 24 task
pairs showing a reduction in the same direction. The paper reports
**d=0.92** — a large effect; if our replication produces **d=0.5** (a
medium effect) we still have a publishable signal with N=24.

A power calculation against d=0.5, α=0.05 two-sided, suggests N≥27
for 80 % power on a paired t-test (Cohen 1988 tables). N=24 is
slightly under-powered for a medium effect; we either accept that or
pre-register a second batch. Either is fine — pre-registration is
the discipline.

Pre-register the analysis plan **before** the user runs trials.
Record the pre-registration in `RESULTS.md` (template included).

## §7 Trial-runner scaffold

`run_trial.sh` is a small bash driver that:

1. Reads `tasks.json` and selects task `--task-id N` (1..24).
2. Reads `--condition C0|C1|C2|...` and assembles the prompt:
   - C0: just the task question.
   - C1+: the task question prefixed by the matching descriptor
     file's content.
3. Launches a Claude Code session in headless mode (`claude
   --print` style) against the worktree, with the trial prompt.
4. Parses the session transcript for tool calls (counting only the
   navigation tools listed in §5).
5. Compares the agent's emitted `file:line` against the task's
   `ground_truth` field; sets `found_correct=true|false`.
6. Appends a row to `results.csv` with the schema
   `task_id,condition,n_tool_calls,found_correct,wall_time_seconds,trial_id,timestamp`.

The script is included as `run_trial.sh` (executable). It does not
require root, network, or any cargo work; it expects `claude`,
`jq`, and a clean `git status` on the worktree before each trial.

There is no automated harness for *correctness* of the agent's
answer — the user spot-checks `found_correct=false` rows manually,
because the ground-truth `file:line` can drift if the repo evolves
between trial batches. To minimise drift, pin all trials to a single
commit SHA.

## §8 Reporting template

`RESULTS.md` lives under `docs/research/forge-replication/` as an
empty template. Sections:

- **Pre-registration.** Date, hypothesis, primary statistical test.
- **Conditions.** Which arms were run, descriptor commit SHA, agent
  model + temperature, pinned spar SHA.
- **N.** Tasks per arm, paired or unpaired, exclusions.
- **Wilcoxon p.** Signed-rank statistic, p-value, effect direction.
- **Cohen's d.** Computed on paired differences.
- **Discussion.** What replicated, what didn't, what's a methods
  limitation.

The template explicitly does **not** include placeholder numbers.
Empty fields stay empty until the user runs trials. A
"pending replication" pointer in the v0.9.x milestone blog post
(`docs/blog/v0.9.x-milestone.md`) links here.

## §9 Honest deviations from Jin's protocol

- **Self-target.** Jin used Forge against unfamiliar repos; we use
  spar against spar. The confound is acknowledged in §1.
- **Hand-curated descriptor.** Jin's "artifact vs process" arm
  required auto-generated descriptors; we have only a hand-curated
  one until spar's Rust-source→descriptor emitter ships.
- **N=24 with 2 arms, not 4.** Jin's controlled experiment was 24×4.
  We default to 24×2 (paired); the 4-arm format comparison is
  optional and unbalanced for default runs.
- **No field study.** Jin reports 7,012 Claude Code sessions with
  52 % variance reduction. We have no comparable instrumentation;
  the field-study arm is not replicated.

These deviations weaken the replication's external validity but do
not invalidate the controlled-experiment arm. The user can close
each gap as the toolchain matures.

## §10 Artefacts shipped in this PR

Under `docs/research/forge-replication/`:

- `descriptor.sexp` — S-expression descriptor of spar's crate graph
  (the v0.9.3-tip snapshot).
- `descriptor.aadl` — AADL stub of the same surface, cross-check
  artefact.
- `tasks.json` — 24 code-localization tasks with ground-truth
  `file:line` answers.
- `run_trial.sh` — executable bash trial runner.
- `RESULTS.md` — empty results template, with pre-registration
  prompts.

Together these are the minimum scaffold the user needs to run a
Forge-style replication on spar. No trial has been run; no number is
reported.

---

### References

- Jin et al. *Formal Architecture Descriptors for AI Agent Code
  Localization.* arXiv:2604.13108 (2026). Controlled experiment
  (24 × 4, Wilcoxon p=0.009, d=0.92); artifact-vs-process (15 × 3,
  100% vs 80%, p=0.002, d=1.04); field study (7,012 sessions,
  52% variance reduction); S-expression best for error detection.
- Cohen, J. *Statistical Power Analysis for the Behavioral Sciences*
  (1988) — N≥27 tables used in §6.
- spar internal:
  `crates/spar-sysml2/src/generate.rs:153` — current SysML v2
  emitter target (rivet → SysML, not Rust → architecture).
  `artifacts/architecture.yaml:1-60` — existing rivet self-description.
