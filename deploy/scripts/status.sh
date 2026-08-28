#!/usr/bin/env bash
#
# What aiwatcher is doing in a cluster, and what it is borrowing.
#
#   ./status.sh                 namespace aiwatcher, current context
#   ./status.sh -n planner      beside planner
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
namespace="${AIWATCHER_NAMESPACE:-aiwatcher}"
release="${AIWATCHER_RELEASE:-aiwatcher}"
context="${AIWATCHER_K8S_CONTEXT:-}"

while (($#)); do
  case "$1" in
    -n|--namespace) namespace="$2"; shift 2 ;;
    -r|--release)   release="$2"; shift 2 ;;
    --context)      context="$2"; shift 2 ;;
    -h|--help)      awk 'NR > 2 && !/^#/ { exit } NR > 2 { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
  esac
done

kube=(kubectl)
# An `A && B` here would exit under `set -e` whenever no context was given.
if [[ -n $context ]]; then
  kube+=(--context "$context")
fi

if [[ -t 1 ]]; then B=$'\e[1m'; NC=$'\e[0m'; else B=; NC=; fi

printf '%s▶ workloads%s\n' "$B" "$NC"
"${kube[@]}" -n "$namespace" get deploy,pod,pvc -l app.kubernetes.io/part-of=aiwatcher 2>&1 || true

printf '\n%s▶ what this install borrowed%s\n' "$B" "$NC"
"$HERE/detect-stack.py" --namespace "$namespace" ${context:+--context "$context"} --format text

printf '\n%s▶ health%s\n' "$B" "$NC"
# Through the API rather than through the pod's status: a projector replaying a
# backlog is Running and not ready, and only /readyz says which.
for probe in livez readyz; do
  if out=$("${kube[@]}" -n "$namespace" exec "deploy/$release-server" -- \
             wget -q -O- "http://127.0.0.1:8080/$probe" 2>/dev/null); then
    printf '  /%s  %s\n' "$probe" "${out:-ok}"
  else
    printf '  /%s  unreachable\n' "$probe"
  fi
done

printf '\n%s▶ recent server log%s\n' "$B" "$NC"
"${kube[@]}" -n "$namespace" logs "deploy/$release-server" --tail=20 2>&1 || true
