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

# The local authentik `just authentik-up` starts. The issuer ends in the
# *application* slug, not the provider name — the one thing that is always
# wrong the first time.
authentik_issuer := env_var_or_default("AIWATCHER_AUTH_ISSUER", "http://localhost:9000/application/o/aiwatcher/")
laser_connection := env_var_or_default("AIWATCHER_LASER_CONNECTION_STRING", "iggy:iggy@127.0.0.1:8090")

# The workflow store `just test-postgres` runs against. Port 5433, not 5432: a
# developer machine usually already has something on the default one, and a
# suite that silently connected to it would create tables in somebody's
# project database.
workflow_postgres_url := env_var_or_default("AIWATCHER_WORKFLOW_POSTGRES_URL", "postgres://aiwatcher:aiwatcher@127.0.0.1:5433/aiwatcher")

# Where a managed query step is sent: the one engine this deployment runs, and
# the only address a query step ever runs against — a plan names a binding and
# its parameters, never a host. `AIWATCHER_FLOW_URL` is its older name, read for
# one release.
query_url := env_var_or_default("AIWATCHER_QUERY_URL", env_var_or_default("AIWATCHER_FLOW_URL", "http://127.0.0.1:8081"))

# Which query engine `query-serve`, `query-install` and `query-check` mean:
# flow, datafusion or duckdb (AW-3).
query_engine := env_var_or_default("AIWATCHER_QUERY_ENGINE", "flow")

# The control plane `just run-flyte` browses. `flytectl demo start` serves one
# on :30080; a cluster's is the flyteadmin Service. There is no `flyte-up` here
# on purpose — the demo cluster is a k3s in Docker that this repo does not
# manage, and pretending to own its lifecycle would be a recipe that half works.
flyte_endpoint := env_var_or_default("AIWATCHER_FLYTE_ENDPOINT", "http://localhost:30080")
flyte_project := env_var_or_default("AIWATCHER_FLYTE_PROJECT", "flytesnacks")
flyte_domain := env_var_or_default("AIWATCHER_FLYTE_DOMAIN", "development")

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

# The registries and serving reader against a real object store. Run
# `just rustfs-up` first; this is the wire test for both SigV4 implementations.
test-rustfs:
    AIWATCHER_PROMPT_S3_ENDPOINT={{rustfs_endpoint}} \
      cargo test -p aiwatcher-prompts --test rustfs -- --ignored --test-threads=1

# `just postgres-up` first. Two files: `postgres` is what makes the third
# `WorkflowStore` adapter prove the same properties as the two that need no
# service — the suite is `aiwatcher_execution::testing`, called by all three —
# and `postgres_upgrade` proves the schema one release leaves behind is one the
# release before it can still read and write. A fresh-database test answers
# neither of those.
test-postgres:
    AIWATCHER_WORKFLOW_POSTGRES_URL={{workflow_postgres_url}} \
      cargo test -p aiwatcher-execution --features postgres,testing \
      --test postgres --test postgres_upgrade \
      -- --ignored --test-threads=1

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

# Loose object files an older build left behind in `target/*/deps`, and the one
# maintenance task on this repository that is not optional on macOS.
#
# `[profile.dev]` in Cargo.toml stops new ones being made — see the comment
# there for why a directory full of them turns a 5.5-second test suite into a
# half-hour one. This clears what a checkout built before that setting already
# has: a `.o` file is a build intermediate no `cargo` freshness check reads, so
# nothing is rebuilt by removing them.
#
# The keepers are moved out and the old directory is deleted whole, because
# unlinking eight hundred thousand files one at a time takes longer than the
# build this is meant to save.

# Clear loose object files an older build left in target/*/deps.
prune-objects:
    #!/usr/bin/env python3
    import glob, os, subprocess, time
    # `target/<profile>/deps`, and `target/<dir>/<profile>/deps` for the target
    # directories Tilt and the container build keep beside it. Globbed rather
    # than walked: one of these holds most of the files on the disk.
    roots = sorted(set(glob.glob("target/*/deps") + glob.glob("target/*/*/deps")))
    for deps in roots:
        names = os.listdir(deps)
        stale = [n for n in names if n.endswith(".o")]
        print(f"{deps}: {len(stale)} loose object files of {len(names)} entries")
        if not stale:
            continue
        trash = f"{deps}.pruning-{int(time.time())}"
        os.rename(deps, trash)
        os.mkdir(deps)
        for name in names:
            if not name.endswith(".o"):
                os.rename(os.path.join(trash, name), os.path.join(deps, name))
        subprocess.Popen(["rm", "-rf", trash], start_new_session=True)
        print(f"  kept {len(names) - len(stale)}; {trash} is being removed in the background")

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

# The panel's own tests. What they are for is narrow and worth saying: the
# generated client does not throw on a refusal, so a component that forgets to
# read the outcome runs its success path over a 403. These render a component
# against a stubbed `fetch` and assert on what a person would see.
panel-test:
    cd {{panel}} && npm run test

# ── Running locally ──────────────────────────────────────────────────────────

# The prompt registry defaults to ./.data/prompts, so this needs nothing
# running. `just run-rustfs` is the same server against the object store.

# The whole local stack from the one binary: server, log, query service.
# What a person new to this repository runs first. Everything else in this
# section is that with one piece changed.
up *ARGS:
    cargo run --features duckdb --bin aiwatcher -- up {{ARGS}}

# The same, on the local DuckDB store — the workflow history in one file rather
# than in a directory of JSON, so `aiwatcher sql` can ask it questions. Not
# while this is running: DuckDB gives a database to one writer *or* to any
# number of readers, and a read-only open against a held file is refused by
# name. `just up` and `aiwatcher down` are the pair that makes it readable.
run-duckdb:
    AIWATCHER_BUS=wal \
    AIWATCHER_WORKFLOW_STORE=duckdb \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --features duckdb --bin aiwatcher

# The storage contract, the same twenty-nine properties the memory, file and
# PostgreSQL adapters prove — against an embedded database, so unlike
# `just test-postgres` this needs nothing running.
test-duckdb:
    cargo test --features duckdb -p aiwatcher-execution --test duckdb

# Server on :8080, durable write-ahead log in ./.data.
run:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# Hugging Face's dataset search is public, so it is a switch rather than a
# credential. Kaggle needs both halves of one — export AIWATCHER_KAGGLE_USERNAME
# and AIWATCHER_KAGGLE_KEY before running this and it joins in. Neither hub is
# ever asked what a licence permits; see ADR_0019.

# The server with dataset hub search on, for the Datasets area's Discover view.
run-hubs:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_HUGGINGFACE_ENABLED=true \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# The server with the encrypted conversation archive on, keyed from ./.data.
run-conversations:
    #!/usr/bin/env bash
    # The archive is off by default and needs a key, so this generates one on
    # first use and reuses it after. That file is the only thing that can read
    # the archive: delete it and everything sealed under it is unreadable,
    # which is also how a key is destroyed on purpose. A deployment gets its
    # key from a secret manager, never from a file beside the data.
    set -euo pipefail
    mkdir -p ./.data
    key_file=./.data/conversation-key
    if [ ! -f "$key_file" ]; then
      python3 -c "import base64,os;print('dev:'+base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip('='))" > "$key_file"
      chmod 600 "$key_file"
      echo "generated a development archive key in $key_file"
    fi
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_CONVERSATION_ARCHIVE=on \
    AIWATCHER_CONVERSATION_KEYS="$(cat "$key_file")" \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# Nothing needs to be running: both suites stand a control plane up on a
# loopback socket. The second one is the end-to-end pass — a real instance built
# by `wiring::build`, served on another socket, driven over HTTP — and it is
# what covers the seams neither half can: a config field nothing wires, a rerun
# reaching a 501 the engine would have served, a correlation id minted by the
# API and dropped by the adapter.

# The engine: the adapter, then the whole stack, against a stand-in Flyte.
test-pipeline:
    cargo test -p aiwatcher-pipeline
    cargo test -p aiwatcher-server --test engine_end_to_end

# The workflow store defaults to ./.data/workflow and holds one process, so the
# two roles below are one process tree unless a database is behind them. That is
# what `just dev` and `just run` run, and it is the refusal in `Config::validate`
# rather than a lock file somebody has to interpret.

# Server on :8080 with managed query execution wired. Run `just query-serve` beside it.
run-execution:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_QUERY_URL={{query_url}} \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher

# The same, on PostgreSQL. Run `just postgres-up` first; needs the feature built in.
run-postgres:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_WORKFLOW_STORE=postgres \
    AIWATCHER_WORKFLOW_POSTGRES_URL={{workflow_postgres_url}} \
    AIWATCHER_QUERY_URL={{query_url}} \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher --features postgres

# The two roles as two processes. Three things stop being per-process when the
# binary is split, and the start-up refuses each by name: the workflow store
# (`postgres`), the log the outbox publishes to and the projector folds
# (`laser`), and the object store one role writes a step's result into for the
# other to read (`s3`). So this needs `just postgres-up`, `just iggy-up` and
# `just rustfs-up` — which is the honest shape of a split deployment, and why
# one process holding both roles is the default.

# `aiwatcher work` alone: the outbox and the reactors, and no ingress.
run-work:
    AIWATCHER_BUS=laser \
    AIWATCHER_LASER_CONNECTION_STRING={{laser_connection}} \
    AIWATCHER_WORKFLOW_STORE=postgres \
    AIWATCHER_WORKFLOW_POSTGRES_URL={{workflow_postgres_url}} \
    AIWATCHER_PROMPT_STORE=s3 \
    AIWATCHER_PROMPT_S3_ENDPOINT={{rustfs_endpoint}} \
    AIWATCHER_PROMPT_S3_ACCESS_KEY=rustfsadmin \
    AIWATCHER_PROMPT_S3_SECRET_KEY=rustfsadmin \
    AIWATCHER_QUERY_URL={{query_url}} \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher --features postgres,laser -- work

# `aiwatcher serve` alone: the API, the read model and the object store.
run-serve:
    AIWATCHER_BUS=laser \
    AIWATCHER_LASER_CONNECTION_STRING={{laser_connection}} \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_WORKFLOW_STORE=postgres \
    AIWATCHER_WORKFLOW_POSTGRES_URL={{workflow_postgres_url}} \
    AIWATCHER_PROMPT_STORE=s3 \
    AIWATCHER_PROMPT_S3_ENDPOINT={{rustfs_endpoint}} \
    AIWATCHER_PROMPT_S3_ACCESS_KEY=rustfsadmin \
    AIWATCHER_PROMPT_S3_SECRET_KEY=rustfsadmin \
    AIWATCHER_LOG=info,aiwatcher=debug \
    cargo run --bin aiwatcher --features postgres,laser -- serve

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

# The redirect URL below comes back to the panel's dev server rather than to the
# API, because that server proxies /api — so the browser sees one origin
# exactly as it will behind an ingress. Run `just dev` beside this.
#
# The client secret is not in here. Read it once from authentik
# (Admin → Providers → aiwatcher) into .env as AIWATCHER_AUTH_CLIENT_SECRET.

# Server on :8080 as an OIDC relying party. Run `just authentik-up` first.
run-sso:
    AIWATCHER_BUS=wal \
    AIWATCHER_INGEST_ENABLED=true \
    AIWATCHER_AUTH_MODE=oidc \
    AIWATCHER_AUTH_ISSUER={{authentik_issuer}} \
    AIWATCHER_AUTH_CLIENT_ID=aiwatcher \
    AIWATCHER_AUTH_REDIRECT_URL=http://localhost:5173/api/v1/auth/callback \
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

# The seed is `scripts/seed-dev.py`, its output in ./.data/dev-seed.log. The log
# is in memory and re-seeded on every start; the registries under ./.data are
# seeded once. AIWATCHER_DEV_SEED=0 starts it empty.
#
# Server, one SDK worker, the panel — and enough seeded data to click around in.
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    # A server left on :8080 answers the health check below on this one's
    # behalf, and the seed would then land in whichever instance that is.
    if curl -fsS --max-time 1 http://127.0.0.1:8080/livez >/dev/null 2>&1; then
      echo "✗ something already answers on :8080 — stop it first (lsof -nP -iTCP:8080 -sTCP:LISTEN)" >&2
      exit 1
    fi
    # The conversation archive is on so Conversations has something to show,
    # keyed exactly as `just run-conversations` keys it.
    mkdir -p ./.data
    key_file=./.data/conversation-key
    if [ ! -f "$key_file" ]; then
      python3 -c "import base64,os;print('dev:'+base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip('='))" > "$key_file"
      chmod 600 "$key_file"
    fi
    # The whole process group, not the pids: `cargo run` does not pass a signal
    # on, and killing it alone is how an aiwatcher is left holding :8080.
    trap 'trap - EXIT INT TERM; kill 0 2>/dev/null || true' EXIT INT TERM
    AIWATCHER_BUS=memory AIWATCHER_WORKFLOW_STORE=memory AIWATCHER_INGEST_ENABLED=true \
      AIWATCHER_CONVERSATION_ARCHIVE=on AIWATCHER_CONVERSATION_KEYS="$(cat "$key_file")" \
      cargo run --bin aiwatcher &
    server=$!
    for attempt in $(seq 1 120); do
        curl -fsS http://127.0.0.1:8080/healthz >/dev/null && break
        kill -0 "$server"
        sleep 1
    done
    curl -fsS http://127.0.0.1:8080/healthz >/dev/null
    (cd sdk/python && PYTHONPATH=examples uv run aiwatcher-runtime --factory worker_workflow:build_runtime) &
    if [ "${AIWATCHER_DEV_SEED:-1}" != 0 ]; then
      ./scripts/seed-dev.py --live &
    fi
    cd {{panel}} && npm run dev

# Varied demo data into a running server — what `just dev` seeds, for `just run`.
seed-dev *args:
    ./scripts/seed-dev.py {{args}}

# Real PostgreSQL, Rust restart and SIGKILL of a Python worker; waits for the real lease.
test-worker-runtime:
    cargo build --bin aiwatcher --features postgres
    cd sdk/python && uv run python ../../scripts/check-worker-runtime.py

# Durable report redelivery across a Rust restart and one Runtime process per attempt.
test-worker-protocol:
    cargo build --bin aiwatcher --features postgres
    cd sdk/python && uv run python ../../scripts/check-worker-runtime.py --protocol

# Publish a demo run into a running server.
seed run_id="":
    ./scripts/seed-demo-run.sh {{run_id}}

# Publish a prompt plus two optimisations — one admitted, one not.
seed-prompts:
    ./scripts/seed-demo-prompts.sh

# Publish two comparable evaluation reports into a running server.
seed-evaluation:
    ./scripts/seed-demo-evaluation.sh

# Publish two executions of one declared workflow: one finished, one running.
seed-workflow stamp="":
    ./scripts/seed-demo-workflow.sh {{stamp}}

# Seed an annotation project: 24 plans, 12 families, an export, a training run.
seed-annotations *args:
    ./scripts/seed-demo-annotations.py {{args}}

# Stage a corpus in pages and import it with the queued job. Needs `just run`.
seed-import:
    python3 ./scripts/seed-staged-import.py

# Seed runs, saved Flow recipes and two immutable datasets for the curation UI.
seed-curation:
    python3 ./scripts/seed-demo-curation.py

# Record, review and export one conversation. Needs `just run-conversations`.
seed-conversations:
    ./scripts/seed-demo-conversations.py

# Move legacy data.input/data.output pairs off the log into the archive.
import-conversation conversation subject basis reference:
    ./scripts/import-conversation-turns.py {{quote(conversation)}} \
      --subject {{quote(subject)}} --basis {{quote(basis)}} --reference {{quote(reference)}}

# The whole chain against a running server: annotate, export, fit a real
# (tiny) model, register it, and check the guardrail refuses an unmeasured one.
# Fails if the loss does not fall — a green run means data moved, not that
# every call returned 200.
e2e-train:
    ./scripts/e2e-mini-train.py

# One turn of a composed graph, drawn against the shape it declared: the two
# searchers the router passed over stay `Pending`, the hand-offs are messages
# between agents, and an agent's span nests under its node though its tracer
# publishes through a client of its own. Starts a server of its own.
e2e-agent-graph:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-graph.py

# A graph's join against a running server: three worker processes, the first
# killed with SIGKILL between the second completion and the third, and the join
# fires once from the stream. Under `ai_spirit_agent`'s environment, because the
# ledger is its code — the sibling path is the coupling AW-2 exists to remove.
e2e-agent-join:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-join.py

# The same agent through an outage: three hops written while the workers cannot
# reach the server, a claim refused until they drain, and a hop whose answer was
# lost re-sent by an operator and recognised. The server is not stopped — the
# workers go through a proxy the script shuts.
e2e-agent-outbox:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-outbox.py

# A turn's model call names the registry version of the prompt it ran on: the
# promoted candidate, or the authored text `make registry` published when the
# builder has no registry — a reference that resolves, and never the words. The
# builder reads through `ai_spirit_agent`'s `registry`. Starts a server of its own.
e2e-agent-prompt:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-prompt.py

# One agent registered as a workflow of its own and started the way the panel
# starts one, with no graph anywhere. Its first attempt is lost after its tool
# wrote, and the retry does not write again; its spans nest under the run; its
# exchange is archived once, approved and exported as a fine-tuning row. Starts
# its own server with the archive on, so the one on :8080 is not touched.
e2e-agent-standalone:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-standalone.py

# Lab 6's three agents as separate worker processes, talking through aiwatcher
# with no broker: a hop is a run of the target agent's workflow claimed from its
# own queue, and the chat's answer comes back through a mailbox. A lost hand-off,
# an agent with no worker, a refused hop and SIGTERM on the way. It starts a
# server of its own.
e2e-agent-transport:
    "$(cd ../ai_spirit_agent && pwd)/.venv/bin/python" scripts/e2e-agent-transport.py

# It watches the `production` label: moving it reads, verifies and warms the
# new version while the old one keeps serving, and only then swaps.
# `POST /v1/rollback` puts the previous one back with nothing to rebuild. Two
# runtimes: `weights` needs nothing, `onnx` needs `aiwatcher-sdk[onnx]`, and
# anything else is refused by name rather than attempted.

# Verify the promoted package's digests, load it, and serve it on :8091.
serve-model port="8091" *args:
    ./scripts/serve-model.py --port {{quote(port)}} {{args}}

# It checks row by row that the graph computes the same function before
# claiming the vector's held-out score, and a running `just serve-model` then
# rolls forward across a runtime change.

# Re-express the promoted weight vector as an ONNX graph and move the label.
onnx-version *args:
    ./scripts/onnx-version.py {{args}}

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

# The workflow engine agents are built on (AW-2): a distribution of its own in
# sdk/agentic, with no runtime dependencies and a lock of its own.
sdk_agentic := "sdk/agentic"

agentic-install:
    cd {{sdk_agentic}} && uv sync --all-groups

# Format, lint, type-check and test the workflow engine.
agentic-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{sdk_agentic}}
    uv run ruff format --check .
    uv run ruff check .
    uv run mypy .
    uv run pytest -q

# ── Query engines: Flow PHP, DataFusion, DuckDB ──────────────────────────────
#
# Optional. The panel's Query tab talks to the engine directly; without it that
# tab says so and the rest of the panel is unaffected. A deployment runs one
# engine, named by AIWATCHER_QUERY_ENGINE (AW-3): the `query-*` recipes serve,
# install and check that one, and the `flow-*` recipes below are Flow's own.

query := "services/query"
flow := query + "/flow"

# Install the engine AIWATCHER_QUERY_ENGINE names.
query-install:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{query_engine}}" in
        flow) just flow-install ;;
        datafusion|duckdb) echo "the {{query_engine}} engine arrives in AW-3 phase 2; only flow is here yet" >&2; exit 1 ;;
        *) echo "AIWATCHER_QUERY_ENGINE is '{{query_engine}}'; the engines are flow, datafusion, duckdb" >&2; exit 1 ;;
    esac

# The engine AIWATCHER_QUERY_ENGINE names, on :8081 against the API on :8080.
query-serve port="8081":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{query_engine}}" in
        flow) just flow-serve {{port}} ;;
        datafusion|duckdb) echo "the {{query_engine}} engine arrives in AW-3 phase 2; only flow is here yet" >&2; exit 1 ;;
        *) echo "AIWATCHER_QUERY_ENGINE is '{{query_engine}}'; the engines are flow, datafusion, duckdb" >&2; exit 1 ;;
    esac

# Everything the engine AIWATCHER_QUERY_ENGINE names has to pass.
query-check:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{query_engine}}" in
        flow) just flow-check ;;
        datafusion|duckdb) echo "the {{query_engine}} engine arrives in AW-3 phase 2; only flow is here yet" >&2; exit 1 ;;
        *) echo "AIWATCHER_QUERY_ENGINE is '{{query_engine}}'; the engines are flow, datafusion, duckdb" >&2; exit 1 ;;
    esac

# Install the PHP dependencies.
flow-install:
    cd {{flow}} && composer install

# The query service on :8081, against the aiwatcher API on :8080.
flow-serve port="8081":
    # `php -S` handles one request at a time unless told otherwise, and the
    # panel polls this service's health while a query is running — so the
    # single-worker default deadlocks the two against each other and the Query
    # tab reports the service as down mid-query.
    cd {{flow}} && AIWATCHER_URL="${AIWATCHER_URL:-http://127.0.0.1:8080}" \
      PHP_CLI_SERVER_WORKERS="${PHP_CLI_SERVER_WORKERS:-4}" \
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
      | curl -sS -X POST http://127.0.0.1:8081/query/query -H 'content-type: application/json' -d @- \
      | python3 -m json.tool

# ── ML pipeline notebook runtime (Python) ────────────────────────────────────
#
# Optional, exactly like the Flow service above. It serves the marimo notebooks
# a curation pipeline's notebook blocks run, and hosts each one as a live app
# for the block's editor. Without it, a pipeline of a source and a transform
# still runs and the panel says which service is missing. See ADR_0024.

ml_pipeline := "services/ml_pipeline"

# Install the dependencies, and the interpreter: uv fetches Python 3.14 itself.
ml-pipeline-install:
    cd {{ml_pipeline}} && uv sync --all-groups

# The notebook runtime on :8082. Binds to localhost: it runs notebook code.
ml-pipeline-serve port="8082":
    cd {{ml_pipeline}} && AIWATCHER_ML_PIPELINE_PORT={{port}} uv run python -m ml_pipeline

# The panel edits the same file, so stop this before saving from there.

# marimo's own editor for one notebook, for a change bigger than the panel's box.
ml-pipeline-edit notebook:
    cd {{ml_pipeline}} && uv run marimo edit "notebooks/{{notebook}}.py"

# Run one notebook over rows from a JSON file, without the panel or the chain.
ml-pipeline-run notebook rows:
    #!/usr/bin/env bash
    set -euo pipefail
    python3 -c 'import json,sys;print(json.dumps({"notebook":sys.argv[1],"rows":json.load(open(sys.argv[2]))}))' \
      {{quote(notebook)}} {{quote(rows)}} \
      | curl -sS -X POST http://127.0.0.1:8082/ml-pipeline/run -H 'content-type: application/json' -d @- \
      | python3 -m json.tool

# Not part of `just check`, which is Rust and the panel: this service is
# optional, like the PHP one.

# Everything the notebook runtime has to pass.
ml-pipeline-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ml_pipeline}}
    # `--locked` rather than a plain `uv run`: this recipe is what CI runs, and
    # a lock that no longer matches `pyproject.toml` should be a red build with
    # a message rather than a quiet re-resolve that tests a dependency set
    # nobody committed.
    uv run --locked ruff format --check .
    uv run --locked ruff check .
    uv run --locked mypy
    uv run --locked pytest -q

# Open http://localhost:5173/data-curation/pipeline and load an example: PII
# detection, Titanic features, or — the one that needs no notebook runtime —
# Titanic survival rates. The Recipe view beside it ships the same corpus as
# two single-script curations.

# The whole curation chain: the API with hub search on, both engines, the panel.
pii-demo:
    #!/usr/bin/env bash
    set -euo pipefail
    # Killed together, whichever one exits first: three of these are servers
    # that would otherwise be left holding ports somebody has to go and find.
    trap 'kill 0' EXIT INT TERM
    AIWATCHER_BUS=wal AIWATCHER_INGEST_ENABLED=true AIWATCHER_HUGGINGFACE_ENABLED=true \
      cargo run --bin aiwatcher &
    (cd {{flow}} && AIWATCHER_URL=http://127.0.0.1:8080 PHP_CLI_SERVER_WORKERS=4 \
      php -S 127.0.0.1:8081 -t public) &
    (cd {{ml_pipeline}} && uv run python -m ml_pipeline) &
    (cd {{panel}} && npm run dev)

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

# Its own compose file rather than a service in docker-compose.yml. That stack
# is what aiwatcher talks to about data; this is four containers, two of them
# databases, for what it talks to about people — and nobody working on span
# assembly should have to run a PostgreSQL to do it.

# A local authentik for `just run-sso`: server, worker, PostgreSQL and Redis.
authentik-up:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f deploy/docker-compose.authentik.yml up -d
    echo "waiting for authentik …"
    for _ in $(seq 1 60); do
      if curl -sf http://localhost:9000/-/health/ready/ >/dev/null 2>&1; then
        echo "✓ authentik is up on http://localhost:9000"
        echo
        echo "  1. finish the first-run setup: http://localhost:9000/if/flow/initial-setup/"
        echo "  2. the blueprint has already created the provider, the application"
        echo "     and the three groups — read the client secret from"
        echo "     Admin → Providers → aiwatcher into .env"
        echo "  3. put yourself in aiwatcher-admins, then: just run-sso"
        exit 0
      fi
      sleep 2
    done
    echo "✗ authentik did not come up; docker compose -f deploy/docker-compose.authentik.yml logs" >&2
    exit 1

# Stop it, keeping its database.
authentik-down:
    docker compose -f deploy/docker-compose.authentik.yml down

# Wipe it, including the admin account and the groups.
authentik-reset:
    docker compose -f deploy/docker-compose.authentik.yml down -v

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

# A local PostgreSQL for `just test-postgres` and for the workflow store.
postgres-up:
    #!/usr/bin/env bash
    set -euo pipefail
    docker rm -f aiwatcher-postgres >/dev/null 2>&1 || true
    docker run -d --name aiwatcher-postgres \
      -e POSTGRES_USER=aiwatcher \
      -e POSTGRES_PASSWORD=aiwatcher \
      -e POSTGRES_DB=aiwatcher \
      -p 5433:5432 postgres:17-alpine
    echo "waiting for the workflow store …"
    for _ in $(seq 1 30); do
      if docker exec aiwatcher-postgres pg_isready -U aiwatcher -q 2>/dev/null; then
        echo "✓ postgres on :5433"
        exit 0
      fi
      sleep 1
    done
    echo "✗ postgres did not come up:" >&2
    docker logs aiwatcher-postgres 2>&1 | tail -20 >&2
    exit 1

postgres-down:
    -docker rm -f aiwatcher-postgres

# Wipe it. The schema is applied on connect, so this is how a run starts clean.
postgres-reset: postgres-down postgres-up

rustfs-logs:
    docker logs -f --tail=100 aiwatcher-rustfs

# ── docker compose stack ─────────────────────────────────────────────────────

stack-up:
    docker compose -f deploy/docker-compose.yml up -d --build
    @printf '\n%s\n' \
      "  aiwatcher   http://localhost:8080" \
      "  Perses      http://localhost:3000" \
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
      # The planner environment publishes an ingress whose host detection reads
      # off the cluster, and this render deliberately reads no cluster. A
      # placeholder stands in so the ingress is validated here rather than
      # skipped; the chart refuses to render an empty host.
      host=()
      if [[ $env == planner ]]; then host=(--set ingress.host=aiwatcher.example.test); fi
      helm template aiwatcher deploy/helm/aiwatcher --namespace "$ns" \
        --values "deploy/environments/$env.yaml" ${host[@]+"${host[@]}"} \
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
    @git config commit.template .gitmessage
    @echo "✓ core.hooksPath set to .githooks — 'git push' runs 'just check', 'git commit' checks the subject (bypass: --no-verify)"
    @echo "✓ commit.template set to .gitmessage"

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

# ── Curation benchmark: Flow PHP and Polars over one corpus ─────────────────
#
# `benchmarks/curation`: a synthetic spans corpus of a chosen size, the same
# four queries in both engines, and every answer checked against the other
# before a time is reported. Flow is one PHP process streaming at tens of MB/s,
# so 10GB of Flow is over an hour — measure it at 1GB and scale, or leave the
# 10GB run to itself. Results land in benchmarks/curation/results/.

bench := "benchmarks/curation"

# Write the corpus both engines read: CSV of the given size, the same rows as Parquet.
bench-curation-generate size="1GB":
    cd {{bench}} && uv run python generate.py --size {{size}}

# Both engines, every query, each in its own process, e.g. `just bench-curation 10GB --engines polars`.
bench-curation size="1GB" *args:
    cd {{bench}} && composer install --no-interaction --quiet && uv run python bench.py run --size {{size}} {{args}}

# The same comparison inside aiwatcher, as a managed curation pipeline.
#
# API, Flow service, notebook runtime and panel, wired so a managed run reaches
# both engines and the corpus. Open `curation/flow-vs-polars` in Data curation
# → Pipeline and press Run on the server, or `just bench-curation-run` from
# another terminal; the two steps' times are in the Workflows waterfall. Both
# limits are raised together, the query service's above the step's: PHP
# stopping first is a 500, which a reactor reads as an outage and retries.
bench-curation-serve size="1GB":
    #!/usr/bin/env bash
    set -euo pipefail
    corpus="$(pwd)/{{bench}}/.data/{{size}}"
    if [ ! -f "$corpus/manifest.json" ]; then
        echo "no corpus at $corpus — run: just bench-curation-generate {{size}}" >&2
        exit 1
    fi
    (cd {{flow}} && composer install --no-interaction --quiet)
    # Beside a `just dev` that already holds 8080 and 5173, move these two; set
    # AIWATCHER_DATA_DIR too, so a benchmark run lands in a store of its own.
    api_port="${AIWATCHER_BENCH_API_PORT:-8080}"
    panel_port="${AIWATCHER_BENCH_PANEL_PORT:-5173}"
    trap 'kill 0' EXIT INT TERM
    AIWATCHER_BUS=wal AIWATCHER_INGEST_ENABLED=true AIWATCHER_LISTEN="127.0.0.1:$api_port" \
      AIWATCHER_QUERY_URL={{query_url}} AIWATCHER_ML_PIPELINE_URL=http://127.0.0.1:8082 \
      AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS=7200 \
      cargo run --bin aiwatcher &
    # JIT on, as the standalone benchmark measures it: a quarter faster, and
    # AArch64 refuses a buffer above 128M.
    (cd {{flow}} && AIWATCHER_URL="http://127.0.0.1:$api_port" AIWATCHER_CORPUS_DIR="$corpus" \
      AIWATCHER_QUERY_TIMEOUT_SECONDS=7500 PHP_CLI_SERVER_WORKERS=4 \
      php -d memory_limit=2G -d opcache.enable=1 -d opcache.enable_cli=1 \
        -d opcache.jit=tracing -d opcache.jit_buffer_size=128M \
        -S 127.0.0.1:8081 -t public) &
    (cd {{ml_pipeline}} && AIWATCHER_CORPUS_DIR="$corpus" AIWATCHER_ML_PIPELINE_TIMEOUT=900 \
      uv run python -m ml_pipeline) &
    (cd {{panel}} && AIWATCHER_API_URL="http://127.0.0.1:$api_port" \
      npm run dev -- --port "$panel_port" --strictPort)

# Start `curation/flow-vs-polars` on a running `bench-curation-serve`, wait, and print both steps' times.
bench-curation-run *args:
    cd {{bench}} && uv run python in_aiwatcher.py {{args}}

# Render docs/diagrams/*.json into docs/diagrams/out/ with the archify skill.
#
# Not part of `just check`. The renderer is an agent skill rather than a
# workspace dependency, and a stale diagram is a documentation problem — wiring
# it into CI would make it a build failure instead, on a machine that has no
# reason to have the skill installed.
diagrams:
    #!/usr/bin/env bash
    set -euo pipefail
    archify="$HOME/.claude/skills/archify/bin/archify.mjs"
    if [ ! -f "$archify" ]; then
        echo "archify is not installed at $archify" >&2
        echo "  npx skills add tt-a1i/archify -g" >&2
        exit 1
    fi
    mkdir -p docs/diagrams/out
    for source in docs/diagrams/*.json; do
        name="$(basename "$source" .json)"
        node "$archify" deliver "${name##*.}" "$source" "docs/diagrams/out/$name.html" --quality showcase
    done

# Not part of `just check`, for the same reason `just diagrams` is not: a stale
# skill is a documentation problem, and wiring it into CI would make it a build
# failure on a machine with no reason to care. `just skills-check` is there for
# a machine that does.
#
# Re-vendor the agent skills in .claude/skills at their pinned commits.
skills:
    ./scripts/vendor-skills.py

# Is .claude/skills still what .claude/skills/vendor.json says it is?
skills-check:
    ./scripts/vendor-skills.py --check

# Move every pin to upstream HEAD and vendor it. The diff is the review.
skills-update:
    ./scripts/vendor-skills.py --update
    ./scripts/vendor-skills.py
