#!/usr/bin/env bash
#
# Install aiwatcher into a cluster.
#
#   ./install.sh                          into namespace "aiwatcher", environment "default"
#   ./install.sh -e planner -n planner    beside planner, on planner's k3s
#   ./install.sh -e planner -n planner --plan     render and diff; change nothing
#
# What this adds over `helmfile apply` on its own:
#
#   * a preflight, so a missing tool is a sentence with an install line rather
#     than a failure from inside helmfile
#   * the detection report, printed before anything is applied, so what the
#     install is about to reuse and what it is about to create is a thing you
#     read rather than a thing you find out
#   * a confirmation for any cluster that is not a known-local one. This
#     kubeconfig has production contexts in it, and `helmfile apply` is not a
#     command that asks.
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"
ROOT="$(cd "$DEPLOY/.." && pwd)"

# ── Defaults from .env ───────────────────────────────────────────────────────
#
# The justfile has loaded this file into every recipe from the start
# (`set dotenv-load := true`), so `just install-cluster` has always had these and
# `./install.sh` never did. Reading it here is what makes the two agree — and
# what lets a machine keep its registry, its tag and its environment in one
# gitignored file instead of in a command line nobody remembers.
#
# Parsed rather than sourced. This script applies manifests to a cluster, and
# `source` on a file of settings runs whatever is in it. A real environment
# variable still wins over the file, which is `just`'s precedence too.
load_dotenv() {
  local file="$1" line key value
  if [[ ! -f $file ]]; then
    return 0
  fi
  while IFS= read -r line || [[ -n $line ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    if [[ -z $line || $line == '#'* ]]; then
      continue
    fi
    line="${line#export }"
    if [[ $line != *=* ]]; then
      continue
    fi
    key="${line%%=*}"
    value="${line#*=}"
    key="${key%"${key##*[![:space:]]}"}"
    if [[ ! $key =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      continue
    fi
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    # One layer of matching quotes, the way dotenv does it.
    if [[ ${#value} -ge 2 && ( ( $value == \"*\" ) || ( $value == \'*\' ) ) ]]; then
      value="${value:1:${#value}-2}"
    fi
    # Already set in the real environment: leave it alone.
    if [[ -n ${!key+x} ]]; then
      continue
    fi
    export "$key=$value"
  done < "$file"
}
load_dotenv "$ROOT/.env"

# AIWATCHER_ENV is what the justfile already defaults its recipes from; honouring
# it here means `./install.sh` on its own and `just install-cluster` pick the
# same environment.
environment="${AIWATCHER_ENV:-default}"
namespace="${AIWATCHER_NAMESPACE:-}"
release="${AIWATCHER_RELEASE:-aiwatcher}"
context="${AIWATCHER_K8S_CONTEXT:-}"
plan_only=false
assume_yes="${AIWATCHER_ASSUME_YES:-false}"

usage() {
  # The header comment, up to the first line that is not one. A line range would
  # go stale the moment the comment grew.
  awk 'NR > 2 && !/^#/ { exit } NR > 2 { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  cat <<'USAGE'

Options:
  -e, --environment NAME   helmfile environment: default | planner   (default: $AIWATCHER_ENV, or default)
  -n, --namespace NAME     namespace to install into                 (default: per environment)
  -r, --release NAME       helm release name                         (default: aiwatcher)
      --context NAME       kube context                              (default: current)
      --plan               show what would change, apply nothing
  -y, --yes                do not ask before applying
  -h, --help

Environment:
  Read from ./.env first, if there is one; a real environment variable wins.
  With the image lines in there, this script needs no arguments at all.

  AIWATCHER_ENV                             environment to install (default: default)
  AIWATCHER_IMAGE, AIWATCHER_IMAGE_TAG      override the server image
                                            (tag defaults to the checkout's commit)
  AIWATCHER_PANEL_IMAGE                     override the panel image
  AIWATCHER_FLOW_IMAGE                      override the query service image
  AIWATCHER_FLOW=true                       install the optional query service
  AIWATCHER_IMAGE_PULL_SECRET               pull Secret for private images
  IMAGE_PULL_SECRET                         planner-compatible fallback
  AIWATCHER_DOMAIN                          publish an ingress on this host;
                                            otherwise detection derives one from
                                            the cluster's other ingress hosts
  AIWATCHER_VICTORIAMETRICS_URL=<url|none>  force detection's answer
  AIWATCHER_VICTORIATRACES_URL=<url|none>
  AIWATCHER_DETECT=off                      skip detection entirely
USAGE
}

while (($#)); do
  case "$1" in
    -e|--environment) environment="$2"; shift 2 ;;
    -n|--namespace)   namespace="$2"; shift 2 ;;
    -r|--release)     release="$2"; shift 2 ;;
    --context)        context="$2"; shift 2 ;;
    --plan)           plan_only=true; shift ;;
    -y|--yes)         assume_yes=true; shift ;;
    -h|--help)        usage; exit 0 ;;
    *) printf 'Unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

# The namespace defaults per environment, and detection needs it before helmfile
# reads any values file — so it is resolved here and exported, not there.
if [[ -z $namespace ]]; then
  case "$environment" in
    planner) namespace=planner ;;
    *)       namespace=aiwatcher ;;
  esac
fi

if [[ ! -f "$DEPLOY/environments/$environment.yaml" ]]; then
  printf '✗ no such environment: %s\n' "$environment" >&2
  printf '  available: %s\n' "$(cd "$DEPLOY/environments" && ls *.yaml | sed 's/\.yaml//' | tr '\n' ' ')" >&2
  exit 2
fi

# helmfile resolves the paths inside the file against the working directory, not
# against the file. Everything below therefore runs from deploy/.
cd "$DEPLOY"

if [[ -t 1 ]]; then
  B=$'\e[1m'; RED=$'\e[31m'; GRN=$'\e[32m'; YEL=$'\e[33m'; DIM=$'\e[2m'; NC=$'\e[0m'
else
  B=; RED=; GRN=; YEL=; DIM=; NC=
fi

fail() { printf '%s✗ %s%s\n' "$RED" "$1" "$NC" >&2; exit 1; }

# ── Preflight ────────────────────────────────────────────────────────────────

for command in kubectl helm helmfile python3; do
  command -v "$command" >/dev/null || fail "$command is not installed.
  kubectl   https://kubernetes.io/docs/tasks/tools/
  helm      https://helm.sh/docs/intro/install/
  helmfile  mise use -g helmfile  |  brew install helmfile"
done

# `helmfile apply` diffs before it syncs, and that diff is a helm plugin. It is
# not required here: without it we `sync` instead, which applies the same
# manifests and only gives up the "what would change" summary. Making it a hard
# requirement would put a plugin whose Helm 4 support is its own moving target
# between you and an install.
if helm plugin list 2>/dev/null | grep -q '^diff'; then
  have_diff=true
else
  have_diff=false
fi

if [[ -n $context ]]; then
  kubectl config get-contexts -o name | grep -qx "$context" \
    || fail "context '$context' is not in your kubeconfig"
else
  context="$(kubectl config current-context 2>/dev/null || true)"
  [[ -n $context ]] || fail "no current kube context, and --context was not given"
fi

server="$(kubectl --context "$context" config view --minify -o jsonpath='{.clusters[0].cluster.server}' 2>/dev/null || echo '?')"
kubectl --context "$context" cluster-info >/dev/null 2>&1 \
  || fail "cannot reach the cluster behind context '$context'"

export AIWATCHER_NAMESPACE="$namespace"
export AIWATCHER_RELEASE="$release"
export AIWATCHER_K8S_CONTEXT="$context"

# ── What is already there ────────────────────────────────────────────────────

printf '\n%saiwatcher → %s%s\n' "$B" "$context" "$NC"
printf '  %s%s%s\n' "$DIM" "$server" "$NC"
printf '  environment %s, namespace %s, release %s\n\n' "$environment" "$namespace" "$release"

report="$(mktemp -t aiwatcher-detect.XXXXXX)"
trap 'rm -f "$report"' EXIT
"$DEPLOY/scripts/detect-stack.py" --namespace "$namespace" --context "$context" > "$report"
"$DEPLOY/scripts/detect-stack.py" --namespace "$namespace" --context "$context" --format text

# Turning detection off is a decision; failing to reach the cluster is not, and
# installing on the second would risk exactly the duplicate this exists to
# avoid.
if [[ ${AIWATCHER_DETECT:-} != off ]]; then
  grep -q 'reachable: true' "$report" \
    || fail "detection could not read the cluster, so it cannot tell what is already installed.
  Installing anyway would risk a second VictoriaMetrics beside the existing one.
  Grant the token 'get,list' on pods, services and networkpolicies, or set
  AIWATCHER_DETECT=off and the *_URL variables by hand."
fi

# ── Overrides from the environment ───────────────────────────────────────────

# `A && B` rather than `if` would exit here under `set -e` the moment one of
# these variables was unset, which is the normal case.
sets=()

# CI publishes each image under the commit SHA and nothing else — see
# .github/workflows/release-images.yml — while the environment files say
# `tag: latest`, which is a tag no build ever pushes. So when a real registry has
# been named and a tag has not, the tag is this checkout's own commit: the one
# whose build produced those images.
if [[ -n ${AIWATCHER_IMAGE:-} && -z ${AIWATCHER_IMAGE_TAG:-} ]]; then
  if AIWATCHER_IMAGE_TAG="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null)"; then
    printf '  tag from this checkout: %s' "${AIWATCHER_IMAGE_TAG:0:12}"
    if [[ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]]; then
      printf ' %s(uncommitted changes are not in it)%s' "$DIM" "$NC"
    fi
    printf '\n\n'
  else
    unset AIWATCHER_IMAGE_TAG
  fi
fi

if [[ -n ${AIWATCHER_IMAGE:-} ]]; then
  sets+=(--set "image.repository=$AIWATCHER_IMAGE")
fi
if [[ -n ${AIWATCHER_IMAGE_TAG:-} ]]; then
  sets+=(--set "image.tag=$AIWATCHER_IMAGE_TAG" --set "panel.image.tag=$AIWATCHER_IMAGE_TAG" \
    --set "flow.image.tag=$AIWATCHER_IMAGE_TAG")
fi
if [[ -n ${AIWATCHER_PANEL_IMAGE:-} ]]; then
  sets+=(--set "panel.image.repository=$AIWATCHER_PANEL_IMAGE")
fi
if [[ -n ${AIWATCHER_FLOW_IMAGE:-} ]]; then
  sets+=(--set "flow.image.repository=$AIWATCHER_FLOW_IMAGE")
fi
# Naming the image is not the same as asking for the service: a build script
# that exports all three should not turn the Query tab on by itself. The chart
# defaults to off and this is the one switch.
if [[ ${AIWATCHER_FLOW:-} == "true" ]]; then
  sets+=(--set "flow.enabled=true")
fi
image_pull_secret="${AIWATCHER_IMAGE_PULL_SECRET:-${IMAGE_PULL_SECRET:-}}"
if [[ -n $image_pull_secret ]]; then
  [[ $image_pull_secret =~ ^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$ ]] \
    || fail "image pull Secret is not a valid Kubernetes name: $image_pull_secret"
  sets+=(--set "imagePullSecrets[0].name=$image_pull_secret")
fi
# Naming a host is also asking to publish on it, which is why this sets both.
# Leaving it unset is the normal path: detect-stack.py derives the host from the
# cluster's other ingresses, and the environment file decides whether to use it
# — planner's does, because the same file attaches the middlewares that
# authenticate it. Both read this variable; neither invents a second answer.
if [[ -n ${AIWATCHER_DOMAIN:-} ]]; then
  [[ $AIWATCHER_DOMAIN =~ ^[A-Za-z0-9.-]+$ ]] || fail "AIWATCHER_DOMAIN is not a hostname: $AIWATCHER_DOMAIN"
  sets+=(--set "ingress.enabled=true" --set "ingress.host=$AIWATCHER_DOMAIN")
fi

helmfile=(helmfile --file helmfile.yaml.gotmpl --environment "$environment" --kube-context "$context")

# ── Apply ────────────────────────────────────────────────────────────────────

if $plan_only; then
  if $have_diff; then
    printf '\n%s▶ what would change%s\n' "$B" "$NC"
    "${helmfile[@]}" ${sets[@]+"${sets[@]}"} diff --detailed-exitcode || true
  else
    printf '\n%s▶ what would be applied%s\n' "$B" "$NC"
    "${helmfile[@]}" ${sets[@]+"${sets[@]}"} template
    printf '\n%sThat is the full manifest, not a diff against what is running.%s\n' "$YEL" "$NC"
    printf '%sFor a diff: helm plugin install https://github.com/databus23/helm-diff%s\n' "$YEL" "$NC"
  fi
  printf '\n%sPlan only — nothing was applied.%s\n' "$YEL" "$NC"
  exit 0
fi

# A known-local cluster is a scratch cluster; anything else is somebody's.
case "$context" in
  orbstack|docker-desktop|minikube|colima|rancher-desktop|kind-*|k3d-*|k3s-*) local_cluster=true ;;
  *) local_cluster=false ;;
esac

if ! $local_cluster && [[ $assume_yes != true ]]; then
  printf '\n%s"%s" is not a known-local cluster.%s\n' "$YEL" "$context" "$NC"
  printf 'Apply to %s, namespace %s? [y/N] ' "$server" "$namespace"
  read -r answer
  [[ $answer == [yY] ]] || { printf 'Nothing was applied.\n'; exit 0; }
fi

printf '\n%s▶ applying%s\n' "$B" "$NC"
if $have_diff; then
  "${helmfile[@]}" ${sets[@]+"${sets[@]}"} apply
else
  # Same manifests, no pre-flight diff. `apply` would fail here rather than
  # fall back on its own.
  "${helmfile[@]}" ${sets[@]+"${sets[@]}"} sync
fi

# Planner's Grafana init container copies the optional datasource ConfigMap at
# pod startup. On a first install that ConfigMap appears after the existing pod
# started, so restart it once to run provisioning with the new source. This
# touches only the selected planner environment and only when it exists.
if [[ $environment == planner ]] \
  && kubectl --context "$context" -n "$namespace" get deployment/planner-grafana >/dev/null 2>&1 \
  && kubectl --context "$context" -n "$namespace" get configmap/aiwatcher-grafana-datasources >/dev/null 2>&1; then
  printf '\n%s▶ reloading planner Grafana datasource provisioning%s\n' "$B" "$NC"
  kubectl --context "$context" -n "$namespace" rollout restart deployment/planner-grafana
  kubectl --context "$context" -n "$namespace" rollout status deployment/planner-grafana --timeout=10m
fi

printf '\n%s✓ aiwatcher is installed%s\n' "$GRN" "$NC"
kubectl --context "$context" -n "$namespace" get deploy,svc -l app.kubernetes.io/part-of=aiwatcher
printf '\n  panel:  kubectl -n %s port-forward svc/%s-panel 8080:80\n' "$namespace" "$release"
printf '  logs:   kubectl -n %s logs -f deploy/%s-server\n' "$namespace" "$release"
printf '  status: %s/scripts/status.sh -n %s\n' "$DEPLOY" "$namespace"
