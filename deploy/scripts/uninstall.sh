#!/usr/bin/env bash
#
# Remove aiwatcher, and say what removing it leaves behind.
#
#   ./uninstall.sh -e planner -n planner
#
# Two things do not go away on their own, and both are deliberate:
#
#   * PersistentVolumeClaims. Helm does not delete them, because a reinstall
#     onto the same claim is usually what you want and a deleted trace store is
#     not recoverable. This prints them and offers, rather than assuming.
#   * Nothing this release borrowed. The VictoriaMetrics it pointed at belongs
#     to whoever installed it; the one NetworkPolicy aiwatcher attached to those
#     pods is removed with the release, which restores their owner's rules
#     exactly, because policies are additive.
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"

environment=default
namespace="${AIWATCHER_NAMESPACE:-}"
release="${AIWATCHER_RELEASE:-aiwatcher}"
context="${AIWATCHER_K8S_CONTEXT:-}"
delete_data=false

while (($#)); do
  case "$1" in
    -e|--environment) environment="$2"; shift 2 ;;
    -n|--namespace)   namespace="$2"; shift 2 ;;
    -r|--release)     release="$2"; shift 2 ;;
    --context)        context="$2"; shift 2 ;;
    --delete-data)    delete_data=true; shift ;;
    -h|--help)        awk 'NR > 2 && !/^#/ { exit } NR > 2 { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
  esac
done

if [[ -z $namespace ]]; then
  case "$environment" in planner) namespace=planner ;; *) namespace=aiwatcher ;; esac
fi

export AIWATCHER_NAMESPACE="$namespace" AIWATCHER_RELEASE="$release"
if [[ -n $context ]]; then
  export AIWATCHER_K8S_CONTEXT="$context"
fi

kube=(kubectl)
# An `A && B` here would exit under `set -e` whenever no context was given.
if [[ -n $context ]]; then
  kube+=(--context "$context")
fi

printf 'Removing release %s from namespace %s (context %s).\n' \
  "$release" "$namespace" "${context:-$("${kube[@]}" config current-context)}"

cd "$DEPLOY"   # helmfile resolves its paths against the working directory.
helmfile --file helmfile.yaml.gotmpl --environment "$environment" \
  ${context:+--kube-context "$context"} destroy

claims=$("${kube[@]}" -n "$namespace" get pvc -l app.kubernetes.io/part-of=aiwatcher \
  -o name 2>/dev/null || true)

if [[ -z $claims ]]; then
  printf '\nNothing left behind.\n'
  exit 0
fi

printf '\nThese volumes were kept:\n'
printf '%s\n' "$claims" | sed 's/^/  /'

if ! $delete_data; then
  printf '\nThey hold the write-ahead log and the trace store. Delete them with:\n'
  printf '  %s --delete-data %s\n' "${BASH_SOURCE[0]}" "-n $namespace"
  exit 0
fi

printf '\nDelete them? This is not recoverable. [y/N] '
read -r answer
[[ $answer == [yY] ]] || { printf 'Kept.\n'; exit 0; }
printf '%s\n' "$claims" | xargs "${kube[@]}" -n "$namespace" delete
