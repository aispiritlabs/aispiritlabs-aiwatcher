#!/usr/bin/env bash
#
# Local pre-PR verification — the single source of truth for "will CI pass?".
# Mirrors .github/workflows/ci.yml, so a green run here means a green CI run.
#
# Behaviour: runs EVERY check rather than stopping at the first failure, prints
# a summary, and exits non-zero if anything that actually ran failed. Optional
# tooling that is not installed is reported as SKIP with an install hint — CI
# remains the hard gate.
#
# What this does NOT cover, because both need something running: the Laser
# integration tests (`just iggy-up && just test-laser`) and the Kubernetes stack
# (`just tilt-ci`).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -t 1 ]]; then
  B=$'\e[1m'; RED=$'\e[31m'; GRN=$'\e[32m'; YEL=$'\e[33m'; NC=$'\e[0m'
else
  B=; RED=; GRN=; YEL=; NC=
fi

declare -a summary=()
failed=0

run() { # run "<label>" <cmd...>
  local label="$1"; shift
  printf '\n%s▶ %s%s\n' "$B" "$label" "$NC"
  if "$@"; then
    summary+=("${GRN}PASS${NC}  ${label}")
  else
    summary+=("${RED}FAIL${NC}  ${label}")
    failed=1
  fi
}

skip() { # skip "<label>" "<hint>"
  printf '\n%s⤳ SKIP %s — %s%s\n' "$YEL" "$1" "$2" "$NC"
  summary+=("${YEL}SKIP${NC}  ${1} (${2})")
}

have() { command -v "$1" >/dev/null 2>&1; }

# ── Rust ─────────────────────────────────────────────────────────────────────
run "cargo fmt"    cargo fmt --all --check
run "cargo clippy" cargo clippy --workspace --all-targets --all-features -- -Dwarnings
run "cargo test"   cargo test --workspace --all-targets

# ── Contract ─────────────────────────────────────────────────────────────────
# The panel's client is generated from contracts/openapi.json. A stale contract
# means the generated client and the Rust routes have silently diverged.
run "openapi contract is current" just openapi-check

# ── Panel ────────────────────────────────────────────────────────────────────
if [[ -d apps/panel/node_modules ]]; then
  run "panel build + typecheck" bash -c "cd apps/panel && npm run build"
else
  skip "panel build" "run 'just install' first"
fi

if [[ -d sdk/typescript/node_modules ]]; then
  run "typescript sdk typecheck" bash -c "cd sdk/typescript && npx tsc --noEmit"
else
  skip "typescript sdk typecheck" "run 'just install' first"
fi

# ── Python SDK ───────────────────────────────────────────────────────────────
# `uv` manages its own interpreter, so this needs nothing but uv on PATH.
if have uv; then
  run "python sdk" just sdk-check
else
  skip "python sdk" "https://docs.astral.sh/uv/getting-started/installation/"
fi

# ── Kubernetes manifests ─────────────────────────────────────────────────────
# Client-side only: no cluster is contacted, so this is safe to run anywhere.
if have kubectl; then
  run "k8s manifests" just k8s-validate
else
  skip "k8s manifests" "kubectl is not installed"
fi

# The install chart. Rendering plus a client-side apply --dry-run; no cluster is
# contacted for either environment.
if have helm && have kubectl; then
  run "helm chart" just chart-check
else
  skip "helm chart" "helm and kubectl are needed"
fi

if have tilt; then
  # Evaluating the Tiltfile catches a syntax error or a bad kustomize reference
  # without deploying anything.
  run "Tiltfile" bash -c 'tilt alpha tiltfile-result --context "${AIWATCHER_K8S_CONTEXT:-orbstack}" >/dev/null'
else
  skip "Tiltfile" "tilt is not installed"
fi

# ── Optional repo-wide linters ───────────────────────────────────────────────
if have typos; then run "typos" typos; else skip "typos" "cargo install typos-cli"; fi
if have taplo; then run "taplo fmt" taplo fmt --check; else skip "taplo fmt" "cargo install taplo-cli"; fi
if have cargo-deny; then
  run "cargo deny" cargo deny check
else
  skip "cargo deny" "cargo install cargo-deny"
fi

# ── Summary ──────────────────────────────────────────────────────────────────
printf '\n%s%s%s\n' "$B" "Summary" "$NC"
for line in "${summary[@]}"; do printf '  %s\n' "$line"; done

if (( failed )); then
  printf '\n%s✗ some checks failed%s\n' "$RED" "$NC"
  exit 1
fi
printf '\n%s✓ all checks passed%s\n' "$GRN" "$NC"
