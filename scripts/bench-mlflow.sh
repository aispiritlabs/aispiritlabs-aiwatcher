#!/usr/bin/env bash
#
# Same agent workload through MLflow and through aiwatcher, measured.
#
# Drives the `LLMTracer` surface `agentic` actually calls, so this answers "what
# does this backend cost for the work my agent does" rather than benchmarking a
# synthetic write loop. Results are written up in docs/mlflow-comparison.md.
#
#   just run                 # in one terminal
#   just bench-mlflow        # in another
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AGENT="${AIWATCHER_AGENT_PATH:-$HOME/Projects/ai_spirit/ood-workshops/ai_spirit_agent}"
RUNS="${1:-14000}"
BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"

if [[ ! -x "$AGENT/.venv/bin/python" ]]; then
  echo "✗ no agent venv at $AGENT — set AIWATCHER_AGENT_PATH" >&2
  exit 1
fi
if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no aiwatcher at $BASE — start one with 'just run'" >&2
  exit 1
fi

db="$(mktemp -t aiwatcher-bench-XXXXXX.db)"
trap 'rm -f "$db"' EXIT

cd "$AGENT"
for mode in aiwatcher mlflow; do
  echo "### $mode"
  MLFLOW_TRACKING_URI="sqlite:///$db" AIWATCHER_URL="$BASE" BENCH_QUEUE=200000 \
    PYTHONPATH="$ROOT/sdk/python" "$AGENT/.venv/bin/python" \
    "$ROOT/scripts/bench-mlflow.py" "$mode" "$RUNS" | grep -E "RESULT|runs/s"
done
