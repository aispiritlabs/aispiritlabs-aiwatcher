#!/usr/bin/env bash
#
# Build (and optionally push) the two images the chart deploys.
#
#   ./build-images.sh                          aiwatcher:dev, aiwatcher-panel:dev
#   REGISTRY=ghcr.io/me TAG=v0.1.0 ./build-images.sh --push
#
# Two images rather than one: a panel change should not rebuild the Rust binary,
# and a Rust change should not rebuild the panel. They are deployed together and
# versioned together, which is what TAG is for.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REGISTRY="${REGISTRY:-}"
TAG="${TAG:-dev}"
# Empty for a default build; "aiwatcher-server/laser" to compile the Laser
# backend in. A default image will refuse to start with AIWATCHER_BUS=laser.
FEATURES="${FEATURES:-}"
push=false
platform="${PLATFORM:-}"

while (($#)); do
  case "$1" in
    --push) push=true; shift ;;
    --platform) platform="$2"; shift 2 ;;
    -h|--help) awk 'NR > 2 && !/^#/ { exit } NR > 2 { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
  esac
done

prefix=""
if [[ -n $REGISTRY ]]; then
  prefix="${REGISTRY%/}/"
fi

server_image="${prefix}aiwatcher:${TAG}"
panel_image="${prefix}aiwatcher-panel:${TAG}"

args=()
if [[ -n $platform ]]; then
  args+=(--platform "$platform")
fi

printf '▶ %s\n' "$server_image"
docker build "${args[@]+"${args[@]}"}" \
  --file "$ROOT/deploy/Dockerfile" \
  --build-arg "FEATURES=$FEATURES" \
  --tag "$server_image" \
  "$ROOT"

printf '\n▶ %s\n' "$panel_image"
docker build "${args[@]+"${args[@]}"}" \
  --file "$ROOT/deploy/Dockerfile.panel" \
  --tag "$panel_image" \
  "$ROOT"

if $push; then
  [[ -n $REGISTRY ]] || { printf '✗ --push needs REGISTRY set.\n' >&2; exit 1; }
  printf '\n▶ pushing\n'
  docker push "$server_image"
  docker push "$panel_image"
fi

printf '\n✓ built\n  %s\n  %s\n' "$server_image" "$panel_image"
printf '\nInstall with them:\n  AIWATCHER_IMAGE=%saiwatcher AIWATCHER_PANEL_IMAGE=%saiwatcher-panel AIWATCHER_IMAGE_TAG=%s \\\n    deploy/scripts/install.sh\n' \
  "$prefix" "$prefix" "$TAG"
