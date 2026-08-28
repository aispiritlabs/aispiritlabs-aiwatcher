#!/usr/bin/env bash
#
# Drive a running aiwatcher with a realistic workload and report its resident
# memory as retention fills. Two agents, two LLM calls, two tool calls and 24
# streamed chunks per run — the shape the read-model caps are sized against.
#
#   just run          # in one terminal
#   just load-test    # in another
set -euo pipefail

BASE="${AIWATCHER_URL:-http://127.0.0.1:8080}"
if ! curl -sf "$BASE/livez" >/dev/null; then
  echo "✗ no server at $BASE — start one with 'just run'" >&2
  exit 1
fi
exec python3 "$(dirname "$0")/load-test.py" "${1:-5000}" "${2:-24}"
