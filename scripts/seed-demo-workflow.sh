#!/usr/bin/env bash
#
# Publishes two executions of one declared workflow, so the Workflows tab has a
# graph to draw. Shaped after planner's house import — acquire → normalize →
# {analyze, thumbnail} → persist — because that is the case the feature exists
# for: one execution spread across a run per stage, joined by workflow_run_id.
#
# The two executions are deliberately different:
#
#   ...-a  finished, with a stage that failed on its first attempt and was
#          retried, and a branch that was never taken — so `pending` is visible
#          as something other than "not started yet".
#   ...-b  still running, so the live view has something to follow.
#
#   just run              # in one terminal
#   just seed-workflow    # in another
set -euo pipefail

BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"
STAMP="${1:-$(date +%s)}"
WORKFLOW="${AIWATCHER_WORKFLOW:-house-import}"
SERVICE="${AIWATCHER_SERVICE:-planner-import-service}"

if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no server at $BASE — start one with 'just run'" >&2
  exit 1
fi

now() { python3 -c 'import datetime;print(datetime.datetime.now(datetime.UTC).isoformat().replace("+00:00","Z"))'; }

seq_no=0
emit() { # emit <execution_id> <run_id> <event_type> <agent_id|-> <data-json>
  seq_no=$((seq_no + 1))
  local agent_field=''
  [[ "$4" != '-' ]] && agent_field="\"agent_id\":\"$4\","
  curl -sf -X POST "$BASE/api/v1/events" \
    -H 'content-type: application/json' \
    -d "{\"events\":[{
      \"event_id\":\"$1-$seq_no\",
      \"event_type\":\"$3\",
      \"occurred_at\":\"$(now)\",
      \"run_id\":\"$2\",
      \"workflow_id\":\"$WORKFLOW\",
      \"workflow_run_id\":\"$1\",
      $agent_field
      \"sequence\":$seq_no,
      \"source\":{\"service\":\"$SERVICE\",\"sdk\":\"python\"},
      \"data\":$5
    }]}" >/dev/null
}

# The topology. Five nodes and a fan-out, so the layout has something to lay
# out and `pending` means "the branch was not taken" rather than "too early".
TOPOLOGY='{
  "name":"House import",
  "version":"sha256:2f1a9c",
  "nodes":[
    {"id":"acquire","name":"Acquire assets","kind":"chain"},
    {"id":"normalize","name":"Normalize pages","kind":"chain"},
    {"id":"analyze","name":"Analyze floor plans","kind":"agent","agent":"floor-plan"},
    {"id":"thumbnail","name":"Render thumbnails","kind":"chain"},
    {"id":"persist","name":"Persist review","kind":"chain"}
  ],
  "edges":[
    {"from":"acquire","to":"normalize"},
    {"from":"normalize","to":"analyze"},
    {"from":"normalize","to":"thumbnail","label":"if public"},
    {"from":"analyze","to":"persist"},
    {"from":"thumbnail","to":"persist"}
  ]
}'

stage() { # stage <execution> <node> <agent|-> <outcome: ok|fail> <call_id>
  local execution="$1" node="$2" agent="$3" outcome="$4" call="$5"
  local run="run-$execution-$node"
  emit "$execution" "$run" run.started - '{}'
  emit "$execution" "$run" step.started "$agent" \
    "{\"node\":\"$node\",\"call_id\":\"$call\",\"step_type\":\"chain\"}"
  sleep 0.15
  if [[ $outcome == ok ]]; then
    emit "$execution" "$run" artifact.produced "$agent" \
      "{\"node\":\"$node\",\"name\":\"$node.json\",\"uri\":\"s3://planner-flyte/$execution/$node.json\",\"media_type\":\"application/json\",\"size_bytes\":$((RANDOM * 8))}"
    emit "$execution" "$run" step.completed "$agent" \
      "{\"node\":\"$node\",\"call_id\":\"$call\",\"step_type\":\"chain\"}"
    emit "$execution" "$run" run.completed - '{"status":"succeeded"}'
  else
    emit "$execution" "$run" step.failed "$agent" \
      "{\"node\":\"$node\",\"call_id\":\"$call\",\"error\":\"OpenCV found no walls in page 3\"}"
    emit "$execution" "$run" run.failed - '{"error":"the stage failed","status":"failed"}'
  fi
}

DONE="exec-$STAMP-a"
LIVE="exec-$STAMP-b"

echo "seeding two executions of $WORKFLOW …"

# ── The finished one ─────────────────────────────────────────────────────────
# Declared from the entrypoint's own run, which is what planner's Flyte
# controller task is: a pod of its own, beside the stages it launches.
emit "$DONE" "run-$DONE-driver" run.started - '{}'
emit "$DONE" "run-$DONE-driver" workflow.declared - "$TOPOLOGY"

stage "$DONE" acquire   importer   ok   a1
stage "$DONE" normalize importer   ok   n1

# Two agents talking: the importer hands the plan to the vision agent and gets
# an answer back. This is the edge nothing could infer from nesting.
emit "$DONE" "run-$DONE-normalize" agent.message importer \
  '{"to":"floor-plan","kind":"handoff","channel":"planner-import-data"}'

# One node that failed and was retried. Two `step.started` with different
# call ids is what makes this two attempts rather than one redelivery.
run="run-$DONE-analyze"
emit "$DONE" "$run" run.started - '{}'
emit "$DONE" "$run" step.started floor-plan '{"node":"analyze","call_id":"an1","step_type":"agent"}'
sleep 0.1
emit "$DONE" "$run" step.failed floor-plan '{"node":"analyze","call_id":"an1","error":"vision provider timed out"}'
emit "$DONE" "$run" step.started floor-plan '{"node":"analyze","call_id":"an2","step_type":"agent"}'
emit "$DONE" "$run" llm.started floor-plan '{"call_id":"c1","provider":"anthropic","model":"claude-opus-5"}'
emit "$DONE" "$run" llm.completed floor-plan \
  '{"call_id":"c1","model":"claude-opus-5","prompt_tokens":4210,"completion_tokens":880}'
emit "$DONE" "$run" artifact.produced floor-plan \
  "{\"node\":\"analyze\",\"name\":\"analysis.json\",\"uri\":\"s3://planner-flyte/$DONE/analysis.json\",\"media_type\":\"application/json\",\"size_bytes\":184320,\"digest\":\"sha256:9e2c\"}"
emit "$DONE" "$run" step.completed floor-plan '{"node":"analyze","call_id":"an2","step_type":"agent"}'
emit "$DONE" "$run" agent.message floor-plan '{"to":"importer","kind":"response"}'
emit "$DONE" "$run" run.completed - '{"status":"succeeded"}'

# `thumbnail` is never started: the branch was not taken. It stays `pending`,
# which is the thing a projection over observed events alone cannot say.
stage "$DONE" persist importer ok p1
emit "$DONE" "run-$DONE-driver" run.completed - '{"status":"succeeded"}'

# ── The one still going ──────────────────────────────────────────────────────
emit "$LIVE" "run-$LIVE-driver" run.started - '{}'
emit "$LIVE" "run-$LIVE-driver" workflow.declared - "$TOPOLOGY"
stage "$LIVE" acquire importer ok a1
run="run-$LIVE-normalize"
emit "$LIVE" "$run" run.started - '{}'
emit "$LIVE" "$run" step.started importer '{"node":"normalize","call_id":"n1","step_type":"chain"}'
emit "$LIVE" "$run" agent.message importer '{"to":"floor-plan","kind":"request","channel":"planner-import-data"}'

sleep 0.8
executions=$(curl -sf "$BASE/api/v1/workflow-executions?workflow_id=$WORKFLOW" \
  | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["executions"]))')
pending=$(curl -sf "$BASE/api/v1/workflow-executions/$DONE" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["summary"]["nodes_pending"])')

echo "✓ $seq_no events published → $executions executions, $pending stage never taken"
echo "  $BASE/api/v1/workflows/$WORKFLOW"
echo "  $BASE/api/v1/workflow-executions/$LIVE"
echo "  panel: http://localhost:5173/workflows?workflow=$WORKFLOW&execution=$LIVE"
