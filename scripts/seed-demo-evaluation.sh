#!/usr/bin/env bash
#
# Publishes two evaluation reports of the same suite on the same dataset, so
# the Evaluation page has the thing worth looking at: a comparison. The second
# run scores higher on the mean and still regresses a case that passed before,
# which is the case the metric alone hides.
#
#   just run              # in one terminal
#   just seed-evaluation  # in another
set -euo pipefail

BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"
SUITE="${AIWATCHER_SUITE:-catalog-floor-plan}"
DATASET="${AIWATCHER_DATASET:-house-catalog@3}"
SERVICE="${AIWATCHER_SERVICE:-demo-seeder}"

if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no server at $BASE — start one with 'just run'" >&2
  exit 1
fi

now() { python3 -c 'import datetime;print(datetime.datetime.now(datetime.UTC).isoformat().replace("+00:00","Z"))'; }

seq_no=0
emit() { # emit <evaluation_id> <event_type> <data-json>
  seq_no=$((seq_no + 1))
  curl -sf -X POST "$BASE/api/v1/events" \
    -H 'content-type: application/json' \
    -d "{\"events\":[{
      \"event_id\":\"$1-$seq_no\",
      \"event_type\":\"$2\",
      \"occurred_at\":\"$(now)\",
      \"run_id\":\"$1\",
      \"sequence\":$seq_no,
      \"source\":{\"service\":\"$SERVICE\",\"sdk\":\"python\"},
      \"data\":$3
    }]}" >/dev/null
}

# report <id> <variant> <mean> <K-127 passed> <K-127 score>
report() {
  local id="$1" variant="$2" mean="$3" first_passed="$4" first_score="$5"
  emit "$id" eval.started \
    "{\"suite\":\"$SUITE\",\"dataset\":\"$DATASET\",\"variant\":\"$variant\",
      \"params\":{\"model\":\"gpt-5-mini\",\"threshold\":\"0.90\",\"prompt\":\"$variant\"}}"
  emit "$id" eval.case \
    "{\"case_id\":\"K-127\",\"passed\":$first_passed,\"score\":$first_score,
      \"reason\":\"catalog numbers, declared areas and openings against the contract\"}"
  emit "$id" eval.case '{"case_id":"K-204","passed":true,"score":0.97,"reason":"all catalogue requirements met"}'
  emit "$id" eval.case '{"case_id":"K-311","passed":false,"score":0.62,"reason":"pole K-311, okna 2/3"}'
  emit "$id" eval.completed \
    "{\"metrics\":{\"mean_score\":$mean,\"cost_usd\":0.42},
      \"report\":{\"scorer\":\"catalog-contract-v2\",\"notes\":\"deterministic OCR fidelity plus validated topology\"}}"
}

echo "seeding two reports of $SUITE on $DATASET …"
report "eval-$(date +%s)-baseline" "floor-plan-v2" 0.85 true 0.94
sleep 0.5
report "eval-$(date +%s)-candidate" "floor-plan-v3" 0.88 false 0.71

sleep 0.8
suites=$(curl -sf "$BASE/api/v1/evaluation-suites" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["suites"]))')
echo "✓ $seq_no events published → $suites suite(s)"
echo "  $BASE/api/v1/evaluations"
echo "  panel: http://localhost:5173/evaluation"
