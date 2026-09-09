# aiwatcher

Observability for AI agent runs. Agents publish events; a Rust backend folds them
into traces, metrics and a live view.

```
Python / TypeScript agents
        │ events
        ▼
   durable log ──► Rust projector ─┬─► live view  ──► SSE / WebSocket ──► React panel
                                   ├─► spans      ──► VictoriaTraces
                                   ├─► aggregates ──► VictoriaMetrics
                                   └─► read model ──► REST
```

The log is the source of truth. Spans are derived from it by pure functions, so a
redelivered event lands on the span it already wrote instead of a duplicate.

Authored artifacts — prompts, datasets, annotations, conversations, models — go to
an object store instead of the log, because they have to stay readable after the
run that used them has been evicted.

## Quick start

Needs Rust 1.98 (installed on demand by rustup) and Node. No broker, no Docker.

```bash
just install   # panel and TypeScript SDK dependencies
just dev       # server on :8080 (in-memory), panel on :5173
just seed      # publish a demo run into it
```

`just dev` keeps nothing across a restart. For data that survives one:

```bash
just run       # :8080, durable write-ahead log in ./.data
```

## Examples

| Where | What it shows |
|---|---|
| [examples/titanic](examples/titanic/README.md) | Raw CSV to a trained model, through the curation canvas. Runnable. |
| [instructions.md](instructions.md) | Two complete paths: agent conversation to a corpus, Hugging Face to a served model. |
| [EXAMPLES.md](EXAMPLES.md) | Every panel screen, with screenshots of real data. |

## Sending events

The contract is [`contracts/envelope.schema.json`](contracts/envelope.schema.json).
Anything that produces that JSON is a valid producer; the SDKs just make the common
case three lines.

```python
from aiwatcher_sdk import AiwatcherClient      # AIWATCHER_URL picks the transport

client = AiwatcherClient(service="research-service")
with client.run("run-123", conversation_id="conv-1") as run:
    with run.agent("researcher") as agent:
        with agent.llm(model="claude-opus-5", provider="anthropic") as call:
            call.first_token()
            call.usage(prompt_tokens=812, completion_tokens=193)
```

An evaluation is reported separately, and forms no span:

```python
client.record_evaluation(
    suite="catalog-floor-plan",
    dataset="house-catalog@3",          # what makes two reports comparable
    metrics={"mean_score": 0.88},
)
client.flush()
```

`sdk/typescript` mirrors it. With `AIWATCHER_URL` unset both drop everything, so
importing either never breaks a test. Unknown event types are stored and streamed,
never rejected. Event types: [docs/event-catalog.md](docs/event-catalog.md).

## The panel

Screens and screenshots: [EXAMPLES.md](EXAMPLES.md). Every list carries the same
time window in the URL, so a link carries the period with it.

| Area | What it answers |
|---|---|
| Runs | What ran, and what stalled. |
| Explore | One run, drilled down — pivot by session, agent, model or tool. |
| Metrics | Tokens, latency percentiles, cache hits, ranked breakdowns. |
| Workflows | The orchestration above a run, as a graph. |
| Evaluation | Suites, reports, per-case scores, and the delta on the same dataset. |
| Prompts | Versions, diffs, and what an optimiser did — with the verdict. |
| Datasets | Immutable curated versions, browsable row by row. |
| Data Curation | A canvas of blocks, or a single Flow PHP query. Both saveable, both runnable. |
| Annotations | Vector labelling, licence tracking, and COCO exports split by subject. |
| Conversations | What people said to your agents. Off by default, encrypted, erasable. |
| Training | The curve, the checkpoint, and the model registry the panel promotes from. |
| Experiments | Start training, evaluation and inference workflows. |

## The command line

One binary runs an instance and talks to one. `aiwatcher up` is the whole local
stack and needs nothing else running.

```bash
just up                          # or: cargo run --bin aiwatcher -- up
aiwatcher runs list window=1h
aiwatcher runs show id=run-7
aiwatcher dimensions kind=agent window=6h format=json
aiwatcher api post path=/api/v1/events body=@run.json
aiwatcher token show             # what an agent should present
```

Arguments are `key=value` in any order. `format=json` makes any read scriptable.
`aiwatcher api` reaches every route that has no verb. Another instance is a
profile, or a one-off `url=`:

```bash
aiwatcher profile set name=prod url=https://aiwatcher.example token=…
aiwatcher runs list profile=prod
```

`aiwatcher help commands` lists every verb with the route behind it.

## Optional backends

Everything below is off by default. Each recipe starts the dependency and a server
wired to it.

| You want | Run |
|---|---|
| Apache Iggy as the log | `just iggy-up && just run-laser` |
| S3 (RustFS) for the registries | `just rustfs-up && just run-rustfs` |
| PostgreSQL for managed runs | `just postgres-up && just run-postgres` |
| DuckDB for managed runs, queryable | `just run-duckdb`, then `aiwatcher sql` |
| Flow PHP for queries and curation | `just flow-serve` |
| marimo notebook blocks | `just ml-pipeline-serve` |
| Flyte as the pipeline engine | `just run-flyte` |
| Kaggle / Hugging Face search | `just run-hubs` |
| SSO against authentik | `just authentik-up && just run-sso` |
| Traces and metrics in Perses | `just stack-up` |
| Conversation archive | `just run-conversations` |

An unconfigured area answers `501` naming the variable that is unset, never an
empty list.

## Deploying

```bash
just detect NS         # what that cluster already runs — reads only
just install-plan      # render and diff; change nothing
just install-cluster   # apply, after asking on any non-local context
```

Installation reads the cluster rather than trusting a flag, because a second
metrics store beside an existing one splits one workload across two with no error
anywhere. Full walkthrough: [docs/INSTALL.md](docs/INSTALL.md).

## Commands

```bash
just               # every recipe, with what each one is for
just check         # everything CI runs; green here means green there
just test          # cargo test --workspace --all-targets
just test-one PAT  # one test by name, e.g. `just test-one two_parallel`
just openapi       # regenerate contracts/openapi.json and the panel's client
just seed-*        # populate one area with demo data
```

`just seed` and its friends fill the panel; `just` lists all of them.

## Docs

| Document | For |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The crates, the data flow, the backends |
| [docs/event-catalog.md](docs/event-catalog.md) | Event types, and how to add one |
| [docs/INSTALL.md](docs/INSTALL.md) | Installing into a cluster |
| [CONTRIBUTING.md](CONTRIBUTING.md) | What is expected of a change |

Built on [Apache Iggy](https://github.com/apache/iggy),
[RustFS](https://github.com/rustfs/rustfs),
[Flow PHP](https://github.com/flow-php/flow) and
[marimo](https://github.com/marimo-team/marimo).

## Licence

Proprietary and confidential. No licence is granted — see [LICENSE](LICENSE).
