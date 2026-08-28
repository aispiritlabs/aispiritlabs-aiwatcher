# aiwatcher — task runner.
#
#   just            list every recipe
#   just check      everything CI runs; green here means green there

set shell := ["bash", "-euo", "pipefail", "-c"]
set dotenv-load := true

panel := "apps/panel"
contract := "contracts/openapi.json"

# The Iggy the Laser adapter is tested against. Pinned, not `latest`: the 0.9
# line is protocol-matched to the `iggy` 0.11 client under `laser_sdk` 0.3, and
# a 0.8 server accepts the connection and then never answers the login.
iggy_image := "apache/iggy:0.9.0-edge.5"
# The API version the rendered manifests are validated against. Pinned rather
# than kubeconform's default of "master", so a schema change upstream is a
# decision here and not a build that went red overnight.
kubeconform_k8s_version := "1.33.0"

# The object store the prompt registry is tested against. RustFS speaks S3, so
# `just test-rustfs` is really a test of the SigV4 signer — see
# `crates/aiwatcher-prompts/src/sigv4.rs`.
rustfs_image := "rustfs/rustfs:1.0.0-rc.3"
rustfs_endpoint := env_var_or_default("AIWATCHER_PROMPT_S3_ENDPOINT", "http://127.0.0.1:9010")
laser_connection := env_var_or_default("AIWATCHER_LASER_CONNECTION_STRING", "iggy:iggy@127.0.0.1:8090")

# Clusters Tilt is allowed to touch. A remote context is a hard stop, not a
# prompt — see the Tiltfile.
k8s_context := env_var_or_default("AIWATCHER_K8S_CONTEXT", "orbstack")
k8s_namespace := "aiwatcher"
# "base" (write-ahead log) or "laser".
k8s_overlay := env_var_or_default("AIWATCHER_K8S_OVERLAY", "base")

[private]
default:
    @just --list --unsorted

# ── Verification ─────────────────────────────────────────────────────────────

# Everything CI runs.
check:
    @./scripts/check.sh

# Format Rust and the panel.
fmt:
    cargo fmt --all
    cd {{panel}} && npx prettier --write "src/**/*.{ts,tsx,css}" --log-level warn

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -Dwarnings

# The default suite: no broker, no cluster, no network.
test:
    cargo test --workspace --all-targets

# One test by name, e.g. `just test-one two_parallel`.
test-one pattern:
    cargo test --workspace {{pattern}} -- --nocapture

# The Laser adapter against a real broker. Needs `just iggy-up` first.
test-laser:
    AIWATCHER_LASER_CONNECTION_STRING={{laser_connection}} \
      cargo test -p aiwatcher-bus --features laser --test laser_integration \
      -- --ignored --test-threads=1

# Five integration tests against a real object store. Run `just rustfs-up` first.
test-rustfs:
    AIWATCHER_PROMPT_S3_ENDPOINT={{rustfs_endpoint}} \
      cargo test -p aiwatcher-prompts --test rustfs -- --ignored --test-threads=1

audit:
    cargo deny check

# ── Building ─────────────────────────────────────────────────────────────────

build:
    cargo build --workspace

# With the real Laser backend compiled in.
build-laser:
    cargo build --workspace --features aiwatcher-server/laser

release:
    cargo build --release --bin aiwatcher

# ── Contract ─────────────────────────────────────────────────────────────────

# Regenerate the OpenAPI document and the panel's client. Commit both.
openapi:
    cargo run --quiet --bin aiwatcher-openapi -- {{contract}}
    cd {{panel}} && npm run generate:api

# Fail if the committed contract has drifted from the routes.
openapi-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run --quiet --bin aiwatcher-openapi -- /tmp/aiwatcher-openapi-check.json
    if ! diff -q {{contract}} /tmp/aiwatcher-openapi-check.json >/dev/null; then
      echo "✗ {{contract}} is stale — run 'just openapi' and commit the result"
      diff -u {{contract}} /tmp/aiwatcher-openapi-check.json | head -40
      exit 1
    fi
    echo "✓ {{contract}} is current"

panel-build:
    cd {{panel}} && npm run build

# ── Running locally ──────────────────────────────────────────────────────────

# The prompt registry defaults to ./.data/prompts, so this needs nothing
# running. `just run-rustfs` is the same server against the object store.

# Server on :8080, durable write-ahead log in ./.data.
run:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# Server on :8080 with the prompt registry in RustFS. Run `just rustfs-up` first.
run-rustfs:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_PROMPT_STORE=s3 \
    AIWATCHER_PROMPT_S3_ENDPOINT={{rustfs_endpoint}} \
    AIWATCHER_PROMPT_S3_ACCESS_KEY=rustfsadmin \
    AIWATCHER_PROMPT_S3_SECRET_KEY=rustfsadmin \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# Server against a local Iggy. Run `just iggy-up` first.
run-laser:
    AIWATCHER_BUS=laser \
    AIWATCHER_LASER_CONNECTION_STRING={{laser_connection}} \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher --features laser

panel:
    cd {{panel}} && npm run dev

# Server (in-memory bus) and the panel together. Nothing survives a restart.
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    AIWATCHER_BUS=memory AIWATCHER_INGEST_ENABLED=true cargo run --bin aiwatcher &
    server=$!
    trap 'kill $server 2>/dev/null || true' EXIT INT TERM
    cd {{panel}} && npm run dev

# Publish a demo run into a running server.
seed run_id="":
    ./scripts/seed-demo-run.sh {{run_id}}

# Publish a prompt plus two optimisations — one admitted, one not.
seed-prompts:
    ./scripts/seed-demo-prompts.sh

# Publish two comparable evaluation reports into a running server.
seed-evaluation:
    ./scripts/seed-demo-evaluation.sh

# ── Python SDK ───────────────────────────────────────────────────────────────

sdk_python := "sdk/python"

# Install the SDK's dev toolchain into sdk/python/.venv.
sdk-install:
    cd {{sdk_python}} && uv sync --all-groups

sdk-fmt:
    cd {{sdk_python}} && uv run ruff format .

# Format, lint, type-check and test the Python SDK.
sdk-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{sdk_python}}
    uv run ruff format --check .
    uv run ruff check .
    uv run mypy .
    uv run pytest -q

# ── Flow query service (PHP) ─────────────────────────────────────────────────
#
# Optional. The panel's Query tab talks to it directly; without it that tab says
# so and the rest of the panel is unaffected.

flow := "services/flow"

# Install the PHP dependencies.
flow-install:
    cd {{flow}} && composer install

# The query service on :8081, against the aiwatcher API on :8080.
flow-serve port="8081":
    cd {{flow}} && AIWATCHER_URL="${AIWATCHER_URL:-http://127.0.0.1:8080}" \
      php -S 127.0.0.1:{{port}} -t public

flow-test:
    cd {{flow}} && vendor/bin/phpunit

# Mago: what `cargo clippy -Dwarnings` is for the Rust crates.
flow-lint:
    cd {{flow}} && vendor/bin/mago lint --minimum-fail-level=warning

flow-fmt:
    cd {{flow}} && vendor/bin/mago format

flow-fmt-check:
    cd {{flow}} && vendor/bin/mago format --check

# Everything the PHP service has to pass. Not part of `just check`, which is
# Rust and the panel — the service is optional and PHP may not be installed.
flow-check:
    #!/usr/bin/env bash
    set -euo pipefail
    just flow-fmt-check
    just flow-lint
    just flow-test

# Run one query from the shell, e.g. `just flow-query "data_frame()->read(default)"`.
flow-query pipeline:
    #!/usr/bin/env bash
    set -euo pipefail
    python3 -c 'import json,sys;print(json.dumps({"pipeline":sys.argv[1]}))' {{quote(pipeline)}} \
      | curl -sS -X POST http://127.0.0.1:8081/flow/query -H 'content-type: application/json' -d @- \
      | python3 -m json.tool

# ── Iggy (for the Laser backend) ─────────────────────────────────────────────

# A local broker for `just run-laser` and `just test-laser`.
iggy-up:
    #!/usr/bin/env bash
    set -euo pipefail
    docker rm -f aiwatcher-iggy >/dev/null 2>&1 || true
    # Every flag below is load-bearing; Iggy fails differently without each.
    #
    #   seccomp=unconfined            its runtime is io_uring, and the default
    #                                 profile blocks io_uring_setup/enter/register
    #   SHARDING_CPU_ALLOCATION       the default "numa:auto" binds shard memory
    #                                 to a NUMA node, which fails in a container
    #                                 VM and takes the server down
    #   ROOT_USERNAME / ROOT_PASSWORD without them the server generates a random
    #                                 root password and only logs it, so every
    #                                 login is "Invalid credentials"
    docker run -d --name aiwatcher-iggy \
      --security-opt seccomp=unconfined \
      -e IGGY_TCP_ADDRESS=0.0.0.0:8090 \
      -e IGGY_SYSTEM_SHARDING_CPU_ALLOCATION=2 \
      -e IGGY_SYSTEM_SHARDING_PIN_CORES=false \
      -e IGGY_ROOT_USERNAME=iggy \
      -e IGGY_ROOT_PASSWORD=iggy \
      -p 8090:8090 {{iggy_image}}
    echo "waiting for the broker …"
    for _ in $(seq 1 30); do
      if nc -z 127.0.0.1 8090 2>/dev/null; then echo "✓ iggy on :8090"; exit 0; fi
      sleep 1
    done
    echo "✗ iggy did not come up:" >&2
    docker logs aiwatcher-iggy 2>&1 | tail -20 >&2
    exit 1

iggy-down:
    -docker rm -f aiwatcher-iggy

iggy-logs:
    docker logs -f --tail=100 aiwatcher-iggy

# ── RustFS (for the prompt registry) ─────────────────────────────────────────

# On :9010 rather than :9000, which is what a MinIO somebody already runs would
# be holding — the point of a local store is that starting it does not break
# whatever else is on the machine.

# A local object store for `just run-rustfs` and `just test-rustfs`.
rustfs-up:
    #!/usr/bin/env bash
    set -euo pipefail
    docker rm -f aiwatcher-rustfs >/dev/null 2>&1 || true
    docker run -d --name aiwatcher-rustfs \
      -e RUSTFS_ACCESS_KEY=rustfsadmin \
      -e RUSTFS_SECRET_KEY=rustfsadmin \
      -e RUSTFS_CONSOLE_ENABLE=false \
      -p 9010:9000 {{rustfs_image}}
    echo "waiting for the object store …"
    for _ in $(seq 1 30); do
      # 403 is the success condition: the S3 endpoint is up and is refusing an
      # unsigned request. A 200 would mean it is not authenticating at all.
      if [[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:9010/ || true)" == "403" ]]; then
        echo "✓ rustfs on :9010"
        exit 0
      fi
      sleep 1
    done
    echo "✗ rustfs did not come up:" >&2
    docker logs aiwatcher-rustfs 2>&1 | tail -20 >&2
    exit 1

rustfs-down:
    -docker rm -f aiwatcher-rustfs

rustfs-logs:
    docker logs -f --tail=100 aiwatcher-rustfs

# ── docker compose stack ─────────────────────────────────────────────────────

stack-up:
    docker compose -f deploy/docker-compose.yml up -d --build
    @printf '\n%s\n' \
      "  aiwatcher   http://localhost:8080" \
      "  Grafana     http://localhost:3000" \
      "  traces      http://localhost:10428" \
      "  metrics     http://localhost:8428"

stack-down:
    docker compose -f deploy/docker-compose.yml down

stack-logs:
    docker compose -f deploy/docker-compose.yml logs -f --tail=100

# ── Tilt on a local test cluster ─────────────────────────────────────────────

# The whole stack in Kubernetes, rebuilt on save. Local contexts only.
tilt-up:
    @just _assert-local-context
    AIWATCHER_K8S_OVERLAY={{k8s_overlay}} tilt up --context {{k8s_context}}

# Same, without the web UI — what CI would run.
tilt-ci:
    @just _assert-local-context
    AIWATCHER_K8S_OVERLAY={{k8s_overlay}} tilt ci --context {{k8s_context}}

tilt-down:
    @just _assert-local-context
    -AIWATCHER_K8S_OVERLAY={{k8s_overlay}} tilt down --context {{k8s_context}}
    -kubectl --context {{k8s_context}} delete namespace {{k8s_namespace}} --wait=false

# Render the manifests without touching a cluster.
k8s-render overlay=k8s_overlay:
    kubectl kustomize deploy/k8s/{{overlay}}

# Client-side validation of both overlays. No cluster contact, safe anywhere.
#
# The schemas come from kubeconform rather than from `kubectl apply
# --dry-run=client`, which is not client-side in the sense that matters here: it
# downloads the OpenAPI document from an apiserver to validate against, and
# needs a second round trip to map kinds to resources even with
# `--validate=false`. On a machine with no cluster — a CI runner — both are a
# refused connection to :8080 rather than a verdict on the manifests.
k8s-validate:
    #!/usr/bin/env bash
    set -euo pipefail
    just _assert-kubeconform
    for overlay in base laser; do
      kubectl kustomize "deploy/k8s/$overlay" \
        | kubeconform -strict -summary -kubernetes-version {{kubeconform_k8s_version}} -
      echo "✓ deploy/k8s/$overlay is valid"
    done

[private]
_assert-kubeconform:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v kubeconform >/dev/null 2>&1; then
      echo "✗ kubeconform is not installed — brew install kubeconform" >&2
      echo "  (or see https://github.com/yannh/kubeconform#installation)" >&2
      exit 1
    fi

# Refuse to run against anything that is not a known-local cluster.
#
# This kubeconfig has production EKS contexts in it. A typo in a context name
# must not be the only thing standing between `tilt up` and a production
# cluster, so the guard is a hard stop rather than a prompt.
[private]
_assert-local-context:
    #!/usr/bin/env bash
    set -euo pipefail
    context="{{k8s_context}}"
    case "$context" in
      orbstack|docker-desktop|minikube|colima|rancher-desktop|kind-*|k3d-*)
        ;;
      *)
        echo "✗ refusing: '$context' is not a known-local cluster." >&2
        echo "  Set AIWATCHER_K8S_CONTEXT to one of: orbstack, docker-desktop," >&2
        echo "  minikube, colima, rancher-desktop, kind-*, k3d-*" >&2
        exit 1
        ;;
    esac
    if ! kubectl config get-contexts -o name | grep -qx "$context"; then
      echo "✗ context '$context' is not in your kubeconfig" >&2
      exit 1
    fi
    echo "✓ targeting local cluster '$context'"

# ── Installing into a cluster ────────────────────────────────────────────────
#
# Different from `just tilt-up`, which watches a scratch cluster. This installs
# into one that may already run half the stack — see docs/INSTALL.md and
# ADR_0009. Needs helmfile and the helm-diff plugin:
#
#   mise use -g helmfile
#   helm plugin install https://github.com/databus23/helm-diff

install_env := env_var_or_default("AIWATCHER_ENV", "default")

# What the target cluster already runs. Reads only; safe against anything.
detect namespace="aiwatcher":
    ./deploy/scripts/detect-stack.py --namespace {{namespace}} --format text

# Render and diff without applying.
install-plan env=install_env:
    ./deploy/scripts/install.sh --environment {{env}} --plan

# Install. Asks before touching a cluster that is not a known-local one.
install-cluster env=install_env:
    ./deploy/scripts/install.sh --environment {{env}}

# What aiwatcher is doing in a cluster, and what it borrowed.
install-status namespace="aiwatcher":
    ./deploy/scripts/status.sh --namespace {{namespace}}

uninstall-cluster env=install_env:
    ./deploy/scripts/uninstall.sh --environment {{env}}

# The three images the chart deploys.
images:
    ./deploy/scripts/build-images.sh

# Render the chart with the target cluster's answer, without touching anything.
chart-render namespace="aiwatcher" values="deploy/environments/default.yaml":
    helm template aiwatcher deploy/helm/aiwatcher --namespace {{namespace}} \
      --values {{values}} \
      --values <(./deploy/scripts/detect-stack.py -n {{namespace}} --format helm-values)

# Chart lint plus a client-side validation of both environments. No cluster
# contact for the render itself; detection reads the cluster when there is one.
chart-check:
    #!/usr/bin/env bash
    set -euo pipefail
    just _assert-kubeconform
    helm lint deploy/helm/aiwatcher
    for env in default planner; do
      ns=$([[ $env == planner ]] && echo planner || echo aiwatcher)
      helm template aiwatcher deploy/helm/aiwatcher --namespace "$ns" \
        --values "deploy/environments/$env.yaml" \
        | kubeconform -strict -summary -kubernetes-version {{kubeconform_k8s_version}} -
      echo "✓ deploy/environments/$env.yaml renders and validates"
    done

# ── Setup ────────────────────────────────────────────────────────────────────

install:
    cd {{panel}} && npm install
    cd sdk/typescript && npm install

# Run `just check` before every push.
setup-hooks:
    @git config core.hooksPath .githooks
    @echo "✓ core.hooksPath set to .githooks — 'git push' now runs 'just check' first (bypass: git push --no-verify)"

# Push a realistic workload at a running server and report resident memory.
#
# The measurement the 512 MB budget in `ReadModelConfig` is based on. Run it
# after changing any retention cap.
load-test runs="5000":
    ./scripts/load-test.sh {{runs}}

# The MLflow comparison: same workload through each tracer, measured.
#
# Needs the agent checkout (AIWATCHER_AGENT_PATH) and a running server.
# Writes the numbers in docs/mlflow-comparison.md.
bench-mlflow runs="14000":
    ./scripts/bench-mlflow.sh {{runs}}
