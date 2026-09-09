# Architecture

The event log is the source of truth. Everything about a run — spans, metrics,
the runs list, a workflow graph — is a fold over it, computed by pure functions.
A redelivered event lands on the span it already wrote, and a backend that learns
something new about an old event type can be pointed at the same log again.

Authored artifacts are the exception: prompts, datasets, annotations,
conversations and models are written by a person or a job, not observed, and have
to outlive the log's retention. They live in an object store behind the same
`ObjectStore` port.

```
Python / TypeScript agents
        │ events
        ▼
   durable log (write-ahead log, or Apache Iggy under the `laser` feature)
        │
        ▼
   Rust projector ─┬─► live events  ──► SSE / WebSocket ──► React panel
                   ├─► finished spans ─► VictoriaTraces
                   ├─► aggregates ─────► VictoriaMetrics
                   └─► read model ─────► REST

   object store (S3 / RustFS)   prompts · datasets · annotations ·
                                conversations · models — outside retention

   workflow store (memory | file | postgres | duckdb)
                                the plan, the decisions, the outbox
```

## Crates

In dependency order. A crate may only depend on ones above it.

| Crate | Holds |
|-------|-------|
| `aiwatcher-core` | Domain: ids, envelope, correlation, event catalog, ports. Knows nothing about a transport or a store. |
| `aiwatcher-bus` | `MessageSource` / `MessageSink` / `Checkpointer`, plus the memory, write-ahead-log, Laser and generic-broker adapters. |
| `aiwatcher-trace` | `SpanAssembler` and the OTLP/JSON exporters. |
| `aiwatcher-jobs` | What a long job over an object store *is*: state, shards, leases, retry decisions, content addressing. Rules, not records. |
| `aiwatcher-prompts` | The prompt registry: content-addressed versions, optimisation verdicts, and the S3/filesystem adapters. |
| `aiwatcher-annotations` | Vector image annotations for any vision domain. Ships no label vocabulary — the project's schema carries it. |
| `aiwatcher-conversations` | Governed conversation data: consent, retention, an encrypted archive, a human review gate, resumable exports. Off by default. |
| `aiwatcher-training` | Training runs and the model versions they produce. A promotion is refused without a held-out score. |
| `aiwatcher-datasets` | Curation recipes, dataset versions, and the block pipelines behind the curation canvas. |
| `aiwatcher-execution` | Managed execution: the compiled plan, the states, the attempts, the pure decider, the outbox. |
| `aiwatcher-runner` | The rerun dispatcher: one HTTP POST to one configured endpoint. |
| `aiwatcher-pipeline` | Pipeline engines behind a `WorkflowEngine` port. Flyte 2 over its `/api/v1/` gateway. |
| `aiwatcher-auth` | OIDC discovery, JWKS cache, authorization-code flow with PKCE, signed session cookies, group-to-role mapping. |
| `aiwatcher-projector` | The pipeline, live hub, read model, dimension and span folds, dedup, retry, dead letters. |
| `aiwatcher-api` | axum router: REST, SSE, WebSocket, OpenAPI. |
| `aiwatcher-cli` | The `aiwatcher` verbs, the profiles, and `aiwatcher api`. |
| `aiwatcher-server` | Config, wiring, graceful shutdown, and the reactors. The only crate that knows every implementation exists. |

Around them: `apps/panel` (React), `sdk/python`, `sdk/typescript`, `contracts/`
(the OpenAPI document and the envelope schema), `deploy/` and `docs/`.

Two optional services sit outside the Cargo workspace and the Rust binary does not
know they exist:

- `services/flow` — the PHP query surface behind the panel's Query tab and a
  pipeline's transforms. `just flow-check`.
- `services/ml_pipeline` — the Python notebook runtime behind a pipeline's marimo
  blocks. `just ml-pipeline-check`.

`just check` covers neither, because PHP and a Python toolchain may not be on a
machine that only touches the Rust crates. CI runs both in their own jobs.

## Log backends

The default is the built-in write-ahead log: no broker, no configuration.
Apache Iggy sits behind the `laser` cargo feature, off by default, so a plain
build needs neither the SDK nor a running broker.

```bash
just iggy-up      # Apache Iggy in Docker, with the three flags it needs
just run-laser
just test-laser   # six integration tests, ~2s against the real broker
```

Iggy needs three settings, and each one fails in a way that does not name its
cause. `just iggy-up` and the Kubernetes manifests set all three.

| Setting | What happens without it |
|---------|-------------------------|
| `seccomp=unconfined` | `Cannot create runtime: Operation not permitted`. Iggy's runtime is io_uring; default seccomp profiles block it. |
| `IGGY_SYSTEM_SHARDING_CPU_ALLOCATION` | `MemoryAffinityFailed`. The default `numa:auto` binds shard memory to a NUMA node, which fails in a container VM. |
| `IGGY_ROOT_USERNAME` / `_PASSWORD` | The server accepts the connection and closes it mid-login. The client reports a VSR header error and reconnects forever — it looks like a protocol mismatch and is not. |

Pin an Iggy **0.9.x** server. A 0.8.x one never answers the `iggy` 0.11 client's
login regardless of the above.

## Workflow store

Managed runs keep their plan, decisions and outbox outside the event log, because
a decision is not a fact about work. Four adapters prove the same storage
contract:

| `AIWATCHER_WORKFLOW_STORE` | Use for |
|---|---|
| `memory` | tests, and `just dev` |
| `file` | one process, development. Refuses a second process by name. |
| `postgres` | production. `just postgres-up`, `just test-postgres`. |
| `duckdb` | a local instance you can query while it runs: `aiwatcher sql`. |

## Authentication

`AIWATCHER_AUTH_MODE` is `none | local | oidc | proxy`, and defaults to `none` —
a release that started refusing requests would be an upgrade that took an
installation down.

- `local` — `aiwatcher up` generates one admin token in a file only its owner can
  read.
- `oidc` — aiwatcher is its own OpenID Connect relying party. The code exchange
  happens in the server and the browser keeps a cookie the server signed, because
  the panel's two most important routes are an SSE stream and a WebSocket and a
  browser can set headers on neither.
- `proxy` — the identity comes from an authenticating reverse proxy already in
  front. Needs a network boundary; the chart refuses to render it without one.

Roles are `viewer` (reads), `editor` (publishes prompt versions and events) and
`admin` (dispatches a rerun, reads conversation content). A producer cannot sign
in interactively, so it carries `AIWATCHER_TOKEN`, which grants editor and never
admin.

Setting it up: [deploy/authentik/README.md](../deploy/authentik/README.md).

## Contracts

`contracts/openapi.json` is generated from the axum routes, and the panel's
TypeScript client is generated from it. Run `just openapi` after any route change
and commit both — CI fails on a stale contract, because a stale client is a
runtime `undefined` rather than a compile error.

## Decisions

Every expensive decision has an ADR: [docs/ADR/README.md](ADR/README.md). The
section that matters in each one is what would make the decision wrong.
