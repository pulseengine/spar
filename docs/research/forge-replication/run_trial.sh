#!/usr/bin/env bash
# Forge-replication trial runner for spar.
#
# Usage:
#   ./run_trial.sh --task-id <1..24> --condition <C0|C1|C2|C3|C4> [--trial-id N]
#
# Reads tasks.json + descriptor (.sexp/.json/.yaml/.md) and launches a
# Claude Code headless session against the spar worktree pinned at the
# task set's target_commit. Counts navigation-class tool calls in the
# session transcript and appends a row to results.csv.
#
# This script does NOT run trials by default. It is the scaffold the
# user invokes to produce the dataset.
#
# Required tools on PATH: claude (Claude Code CLI), jq, git.
# Optional: rg.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPAR_ROOT="${SPAR_ROOT:-$(cd "$HERE/../../.." && pwd)}"
TASKS_FILE="$HERE/tasks.json"
RESULTS_CSV="$HERE/results.csv"
TRANSCRIPTS_DIR="$HERE/transcripts"

# ── Argument parsing ─────────────────────────────────────────────────
TASK_ID=""
CONDITION=""
TRIAL_ID="1"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --task-id) TASK_ID="$2"; shift 2;;
        --condition) CONDITION="$2"; shift 2;;
        --trial-id) TRIAL_ID="$2"; shift 2;;
        -h|--help)
            cat <<'EOF'
Usage: run_trial.sh --task-id <1..24> --condition <C0|C1|C2|C3|C4> [--trial-id N]

Conditions:
  C0  no descriptor (control)
  C1  with descriptor.sexp (treatment, default S-expr form)
  C2  with descriptor.json
  C3  with descriptor.yaml
  C4  with descriptor.md

Output: appends one row to results.csv:
  task_id,condition,n_tool_calls,found_correct,wall_time_seconds,trial_id,timestamp

Transcripts saved under transcripts/<task_id>-<condition>-<trial_id>.jsonl
EOF
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 2;;
    esac
done

if [[ -z "$TASK_ID" || -z "$CONDITION" ]]; then
    echo "missing --task-id or --condition" >&2
    exit 2
fi

# ── Sanity checks ────────────────────────────────────────────────────
command -v claude >/dev/null 2>&1 || { echo "claude CLI not on PATH" >&2; exit 3; }
command -v jq     >/dev/null 2>&1 || { echo "jq not on PATH" >&2; exit 3; }

# Pin to commit to avoid drift between trials.
PIN="$(jq -r .target_commit "$TASKS_FILE")"
HEAD_SHA="$(cd "$SPAR_ROOT" && git rev-parse --short HEAD)"
if [[ "$PIN" != "$HEAD_SHA" ]]; then
    echo "WARN: tasks.json pinned to $PIN but worktree is at $HEAD_SHA" >&2
    echo "      either check out $PIN or update tasks.json target_commit" >&2
fi

# ── Build prompt ─────────────────────────────────────────────────────
TASK_JSON="$(jq -c ".tasks[] | select(.id == ${TASK_ID})" "$TASKS_FILE")"
if [[ -z "$TASK_JSON" ]]; then
    echo "no task with id=$TASK_ID" >&2
    exit 2
fi
TASK_Q="$(echo "$TASK_JSON" | jq -r .question)"
TASK_GT="$(echo "$TASK_JSON" | jq -r .ground_truth)"

case "$CONDITION" in
    C0) DESCRIPTOR_FILE="";;
    C1) DESCRIPTOR_FILE="$HERE/descriptor.sexp";;
    C2) DESCRIPTOR_FILE="$HERE/descriptor.json";;
    C3) DESCRIPTOR_FILE="$HERE/descriptor.yaml";;
    C4) DESCRIPTOR_FILE="$HERE/descriptor.md";;
    *) echo "unknown condition: $CONDITION" >&2; exit 2;;
esac

PROMPT_FILE="$(mktemp -t spar-forge-prompt.XXXXXX)"
trap 'rm -f "$PROMPT_FILE"' EXIT

if [[ -n "$DESCRIPTOR_FILE" ]]; then
    if [[ ! -f "$DESCRIPTOR_FILE" ]]; then
        echo "descriptor not found: $DESCRIPTOR_FILE" >&2
        exit 4
    fi
    cat <<EOF >"$PROMPT_FILE"
Below is an architecture descriptor of the spar workspace. Use it to
locate the answer.

--- BEGIN DESCRIPTOR ---
$(cat "$DESCRIPTOR_FILE")
--- END DESCRIPTOR ---

Task: $TASK_Q

Reply with exactly one line in the form "file:line" (no extra prose).
EOF
else
    cat <<EOF >"$PROMPT_FILE"
Task: $TASK_Q

Reply with exactly one line in the form "file:line" (no extra prose).
EOF
fi

# ── Run session ──────────────────────────────────────────────────────
mkdir -p "$TRANSCRIPTS_DIR"
TRANSCRIPT="$TRANSCRIPTS_DIR/${TASK_ID}-${CONDITION}-${TRIAL_ID}.jsonl"
START_TS="$(date +%s)"

# claude --print runs one-shot headless. --output-format stream-json
# emits one JSON event per line including each tool call.
( cd "$SPAR_ROOT" && \
  claude --print --output-format stream-json --verbose \
         --dangerously-skip-permissions \
         < "$PROMPT_FILE" > "$TRANSCRIPT" ) || true

END_TS="$(date +%s)"
WALL=$((END_TS - START_TS))

# ── Score the transcript ─────────────────────────────────────────────
# Navigation-class tools: Read, Glob, Bash (with grep/rg/find/ls token),
# and Grep (if the harness exposes it directly).
N_CALLS=$(jq -s '
    [.[]
     | select(.type == "tool_use")
     | .name as $n
     | (.input.command // "") as $cmd
     | select(
         $n == "Read"
         or $n == "Glob"
         or $n == "Grep"
         or ($n == "Bash"
             and ($cmd | test("^\\s*(cd [^;]+;\\s*)?(grep|rg|find|ls)\\b")))
       )]
    | length
' < "$TRANSCRIPT" 2>/dev/null || echo "NaN")

# Extract the final assistant message and check substring match against
# ground truth. Loose match (substring) — manual spot-check needed for
# borderline cases.
LAST_TEXT=$(jq -s '
    [.[] | select(.type == "assistant") | .message.content[]? | select(.type=="text") | .text]
    | last // ""
' < "$TRANSCRIPT" 2>/dev/null || echo '""')

if echo "$LAST_TEXT" | grep -qF "$TASK_GT"; then
    CORRECT="true"
else
    CORRECT="false"
fi

# ── Append to CSV ────────────────────────────────────────────────────
if [[ ! -f "$RESULTS_CSV" ]]; then
    echo "task_id,condition,n_tool_calls,found_correct,wall_time_seconds,trial_id,timestamp" > "$RESULTS_CSV"
fi
echo "${TASK_ID},${CONDITION},${N_CALLS},${CORRECT},${WALL},${TRIAL_ID},$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    >> "$RESULTS_CSV"

echo "trial done: task=$TASK_ID cond=$CONDITION calls=$N_CALLS correct=$CORRECT wall=${WALL}s"
