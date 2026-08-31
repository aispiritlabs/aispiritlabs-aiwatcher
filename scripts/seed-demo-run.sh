#!/usr/bin/env bash
#
# Publishes one realistic run into a running server, so the panel has something
# to show. Includes a streamed LLM call with chunk events — the case that makes
# the "an event is not a span" rule visible: ~40 events in, 4 spans out.
#
#   make run        # in one terminal
#   make seed       # in another
set -euo pipefail

BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"
RUN_ID="${1:-run-$(date +%s)}"
CONVERSATION="${AIWATCHER_CONVERSATION:-conv-demo}"
# A conversation groups runs by who is talking; a workflow groups them by what
# is being executed. The explorer pivots on both, so the demo carries both.
WORKFLOW="${AIWATCHER_WORKFLOW:-research-summary}"
# `source.service` is what the `runtime` pivot groups by.
SERVICE="${AIWATCHER_SERVICE:-demo-seeder}"
AGENT="${AIWATCHER_AGENT:-research-agent}"

if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no server at $BASE — start one with 'make run'" >&2
  exit 1
fi

now() { python3 -c 'import datetime;print(datetime.datetime.now(datetime.UTC).isoformat().replace("+00:00","Z"))'; }

seq_no=0
emit() { # emit <event_type> <agent_id|-> <data-json>
  seq_no=$((seq_no + 1))
  local agent_field=''
  [[ "$2" != '-' ]] && agent_field="\"agent_id\":\"$2\","
  curl -sf -X POST "$BASE/api/v1/events" \
    -H 'content-type: application/json' \
    -d "{\"events\":[{
      \"event_id\":\"$RUN_ID-$seq_no\",
      \"event_type\":\"$1\",
      \"occurred_at\":\"$(now)\",
      \"run_id\":\"$RUN_ID\",
      \"conversation_id\":\"$CONVERSATION\",
      \"workflow_id\":\"$WORKFLOW\",
      $agent_field
      \"sequence\":$seq_no,
      \"source\":{\"service\":\"$SERVICE\",\"sdk\":\"python\"},
      \"data\":$3
    }]}" >/dev/null
}

echo "seeding $RUN_ID …"

emit run.started        - '{}'
emit agent.started      "$AGENT" '{}'
emit llm.started        "$AGENT" '{"call_id":"c1","provider":"anthropic","model":"claude-opus-5"}'
sleep 0.2
emit llm.first_token    "$AGENT" '{"call_id":"c1","provider":"anthropic","model":"claude-opus-5"}'
for i in $(seq 1 24); do
  emit llm.chunk        "$AGENT" "{\"call_id\":\"c1\",\"text\":\"token-$i \"}"
done
emit llm.completed      "$AGENT" '{"call_id":"c1","provider":"anthropic","model":"claude-opus-5","prompt_tokens":812,"completion_tokens":193,"cached_tokens":400,"finish_reason":"stop"}'

emit tool.started       "$AGENT" '{"call_id":"t1","tool_name":"web_search"}'
sleep 0.3
emit tool.completed     "$AGENT" '{"call_id":"t1","tool_name":"web_search","results":7}'

emit llm.started        "$AGENT" '{"call_id":"c2","provider":"anthropic","model":"claude-opus-5"}'
sleep 0.2
emit llm.completed      "$AGENT" '{"call_id":"c2","provider":"anthropic","model":"claude-opus-5","prompt_tokens":1420,"completion_tokens":88,"cached_tokens":812,"finish_reason":"stop"}'

emit agent.completed    "$AGENT" '{}'
emit run.completed      - '{"status":"succeeded"}'

sleep 0.8
spans=$(curl -sf "$BASE/api/v1/runs/$RUN_ID" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["spans"]))')
echo "✓ $seq_no events published → $spans spans"
echo "  $BASE/api/v1/runs/$RUN_ID"
echo "  panel: http://localhost:5173/runs/$RUN_ID"
