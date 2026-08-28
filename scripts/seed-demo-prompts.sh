#!/usr/bin/env bash
#
# Publishes one prompt with a history worth looking at: an authored baseline
# and three optimisations — one admitted and two refused, for the two different
# reasons a candidate gets refused.
#
# The refusals are the point. The second gains 0.35 on the split the optimiser
# searched and 0.00 on the split it never saw, which is the shape of every
# overfit. The third scores better still and has quietly stopped mentioning the
# page it was supposed to be reading. Both ask to be promoted; neither is. A
# prompt page that only ever showed successes would be a page nobody needs.
#
#   just run          # in one terminal
#   just seed-prompts # in another
set -euo pipefail

BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"
NAME="${AIWATCHER_PROMPT:-planner.floor-plan}"

if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no server at $BASE — start one with 'just run'" >&2
  exit 1
fi

BASELINE='You are reading a scanned house plan.

Describe the floor plan on {{ page }} in {{ language }}. For every room give its
name and its area in square metres. Report the total area last.'

ADMITTED='You are reading a scanned house plan.

Work through {{ page }} room by room before answering. For every room give its
name and its floor area in square metres, taken from the dimension lines rather
than estimated from the drawing. Answer in {{ language }}. Report the total
area last, and say so when a room has no dimensions on the page.'

# Longer, more emphatic, and better on the split it was searched against. It
# still interpolates both variables, so nothing about it looks wrong — the
# held-out score is the only thing that says it learned the dev cases.
OVERFIT='You are a meticulous, world-class architectural analyst.

Study {{ page }} exhaustively. Enumerate every room, and for each one state the
name, the floor area in square metres to two decimal places, and the aspect
ratio. Be exhaustive. Answer in {{ language }}. Finish with the total area, the
mean room area, and a confidence score.'

# Scores better still, and no longer mentions the page at all: it learned to
# describe a plausible house rather than the one it was given. The harness fed
# it the same fixed text every time, so nothing in the score could show this.
UNGROUNDED='You are an expert architect. Describe a typical single-family house
in {{ language }}: living room, kitchen, three bedrooms, two bathrooms, with
areas in square metres and a total at the end.'

json() { python3 -c 'import json, sys; print(json.dumps(sys.argv[1]))' "$1"; }

post() { # post <path> <body>
  curl -sf -X POST "$BASE$1" -H 'content-type: application/json' -d "$2"
}

# Every reader below takes the response as an argument rather than on stdin:
# quoting a Python one-liner inside a double-quoted bash string is how these
# scripts acquire bugs that only show up in the second branch.
field() { # field <json> <python expression over `record`>
  python3 -c '
import json, sys
record = json.loads(sys.argv[1])
print(eval(sys.argv[2], {"record": record}))
' "$1" "$2"
}

registry_status=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/prompts")
if [[ $registry_status == 501 ]]; then
  echo "✗ this instance has no prompt registry (AIWATCHER_PROMPT_STORE=none)" >&2
  exit 1
fi

echo "publishing $NAME …"
published=$(post /api/v1/prompts "{
  \"name\": \"$NAME\",
  \"text\": $(json "$BASELINE"),
  \"author\": \"demo-seeder\",
  \"model\": \"qwen/qwen3-vl-235b\",
  \"notes\": \"the prompt the service shipped with\",
  \"description\": \"Extracts rooms and areas from a scanned floor plan.\",
  \"tags\": [\"planner\", \"vision\"],
  \"label\": \"production\"
}")
baseline=$(field "$published" 'record["version"]["version_id"]')
echo "  baseline ${baseline:0:12} — variables: $(field "$published" '", ".join(record["version"]["variables"])')"

echo "recording an optimisation that clears the held-out gate …"
admitted=$(post "/api/v1/prompts/$NAME/optimizations" "{
  \"algorithm\": \"deepeval/SIMBA\",
  \"baseline\": \"$baseline\",
  \"candidate_text\": $(json "$ADMITTED"),
  \"primary_metric\": \"mean_score\",
  \"dev\":  [{\"metric\": \"mean_score\", \"baseline\": 0.61, \"candidate\": 0.79},
            {\"metric\": \"min_score\",  \"baseline\": 0.40, \"candidate\": 0.58}],
  \"test\": [{\"metric\": \"mean_score\", \"baseline\": 0.60, \"candidate\": 0.67},
            {\"metric\": \"min_score\",  \"baseline\": 0.38, \"candidate\": 0.49}],
  \"dataset\": \"house-catalog@3\",
  \"iterations\": 8,
  \"duration_ms\": 1830000,
  \"report\": {\"accepted_iterations\": 3, \"candidates_evaluated\": 24},
  \"promote\": true
}")
echo "  $(field "$admitted" 'record["outcome"]') — dev +0.18, held out +0.07"

echo "recording one that only moved the split it was searching …"
rejected=$(post "/api/v1/prompts/$NAME/optimizations" "{
  \"algorithm\": \"deepeval/SIMBA\",
  \"baseline\": \"$baseline\",
  \"candidate_text\": $(json "$OVERFIT"),
  \"primary_metric\": \"mean_score\",
  \"dev\":  [{\"metric\": \"mean_score\", \"baseline\": 0.61, \"candidate\": 0.96}],
  \"test\": [{\"metric\": \"mean_score\", \"baseline\": 0.60, \"candidate\": 0.60}],
  \"dataset\": \"house-catalog@3\",
  \"iterations\": 16,
  \"duration_ms\": 3120000,
  \"promote\": true
}")
echo "  $(field "$rejected" 'record["outcome"] + " — " + record.get("reason", "")')"

echo "recording one that stopped reading the page it was given …"
ungrounded=$(post "/api/v1/prompts/$NAME/optimizations" "{
  \"algorithm\": \"deepeval/SIMBA\",
  \"baseline\": \"$baseline\",
  \"candidate_text\": $(json "$UNGROUNDED"),
  \"primary_metric\": \"mean_score\",
  \"dev\":  [{\"metric\": \"mean_score\", \"baseline\": 0.61, \"candidate\": 0.98}],
  \"test\": [{\"metric\": \"mean_score\", \"baseline\": 0.60, \"candidate\": 0.91}],
  \"dataset\": \"house-catalog@3\",
  \"iterations\": 24,
  \"duration_ms\": 4400000,
  \"promote\": true
}")
echo "  $(field "$ungrounded" 'record["outcome"] + " — " + record.get("reason", "")')"
echo "  it stopped interpolating: $(field "$ungrounded" '", ".join(record["variables_lost"]) or "nothing"')"

echo
detail=$(curl -sf "$BASE/api/v1/prompts/$NAME")
echo "✓ $NAME: $(field "$detail" '"{} versions, {} optimisations, production is {}".format(
    len(record["head"]["versions"]),
    len(record["head"]["optimizations"]),
    record["head"]["labels"]["production"][:12])')"
echo "  $BASE/api/v1/prompts/$NAME"
echo "  panel: http://localhost:5173/prompts/$NAME"
