# The query engines

A deployment runs **one** query engine behind the panel's Query tab, its recipes and a
curation chain's query step, chosen with `AIWATCHER_QUERY_ENGINE` (AW-3,
[ADR_0028](../../docs/ADR/ADR_0028_QUERY_ENGINES.md)). They live here side by side:

| directory | what it is | language | checks |
|---|---|---|---|
| [`flow/`](flow) | Flow PHP: a query is parsed and admitted, never executed ([ADR_0008](../../docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md)) | `flow-dsl` | `just query-check` |
| [`contract/`](contract) | `aiwatcher_query`: what every Python engine serves — the six `/query` routes, the catalog all three engines load ([`catalog.json`](contract/catalog.json)), the API paged into Arrow, the fork server and the child a query runs in, `strict` admission, and the conformance suite | — | `just query-contract-check` |
| [`datafusion/`](datafusion) | `query_datafusion`: DataFusion's `DataFrame` API | `datafusion-python` | the same |
| [`duckdb/`](duckdb) | `query_duckdb`: DuckDB's relational API, on a connection locked to the corpus root | `duckdb-python` | the same |

```bash
just query-install                       # the engine AIWATCHER_QUERY_ENGINE names
just query-serve                         # on :8081, against the API on :8080
just query-check                         # its own checks
just query-conformance                   # q1–q4 in its language, compared with Flow's rows
just query-conformance --record          # against Flow only: rewrite the expected rows
AIWATCHER_QUERY_ENGINE=duckdb just query-serve
```

The three Python members are one `uv` workspace with one lock; each engine is a member
of its own, so an image installs one engine's wheel and never the other's. Two things
that bit once: ruff's `src` lists each member's directory, or `import duckdb` sorts as
first-party; and pytest runs as `.venv/bin/python -m pytest` when anything else may
`uv run` at the same time.

## A query, in each language

Every engine reads data only through `read(name, **arguments)` — a catalog dataset by
name, never a path or a URL — and answers with the value of the last line. In a
pipeline transform, `df` is the rows the chain has so far.

```python
# DataFusion: col, lit and f (DataFusion's functions)
read("spans").filter(col("kind") == lit("llm")).aggregate(
    [col("model")], [f.count(col("model")).alias("spans")]
)
```

```python
# DuckDB: the five Expression classes
read("spans").filter(ColumnExpression("kind") == ConstantExpression("llm")).aggregate(
    [
        ColumnExpression("model"),
        FunctionExpression("count", ColumnExpression("model")).alias("spans"),
    ],
    "model",
)
```

There is no SQL in either, and no session or connection in a query's namespace. The
four conformance questions in [`contract/conformance/`](contract/conformance) are
worked examples of each language, beside the Flow text they answer the same way.

## Admission

`AIWATCHER_QUERY_ADMISSION` is read by the two Python engines:

- **`open`**, the default: the query is Python, run as a notebook's cell is — in a
  child of a fork server that imported the engine and never ran a query, with every
  `AIWATCHER_*` variable removed, a scratch working directory, a CPU ceiling
  (`AIWATCHER_QUERY_TIMEOUT_SECONDS`), a memory ceiling on Linux
  (`AIWATCHER_QUERY_MEMORY_MB`) and a wall clock. A ceiling reached is a 422 naming it.
  The service binds to `127.0.0.1`.
- **`strict`**: the text is parsed and admitted before anything runs — names from the
  query's namespace or assigned by it, attributes from the engine's own classes and
  modules, each engine's `DECLINED` table, and DuckDB's rule that no text reaches an
  argument it would parse as SQL. `/query/check` reports every refusal with its line
  and column.

A DuckDB session is locked to the corpus root under either: `allowed_directories`, then
`enable_external_access` off. ADR_0028 lists what `open` costs — it runs code, on a
laptop it can reach the network, on macOS a same-user process can read the service's
initial environment — and what would make it the wrong default.

## Configuration

| variable | default | what it is |
|---|---|---|
| `AIWATCHER_URL` | `http://127.0.0.1:8080` | the aiwatcher API `read()` pages |
| `AIWATCHER_QUERY_HOST`, `AIWATCHER_QUERY_PORT` | `127.0.0.1`, `8081` | where the service listens |
| `AIWATCHER_QUERY_ADMISSION` | `open` | `open` or `strict` |
| `AIWATCHER_QUERY_TIMEOUT_SECONDS` | 30 | a query's CPU ceiling; it stays above the server's `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS`, and `just query-serve`, Compose and the chart set it a minute above that |
| `AIWATCHER_QUERY_MEMORY_MB` | 4096 | a query child's address-space ceiling, Linux only |
| `AIWATCHER_QUERY_CONCURRENCY` | 4 | how many queries run at once |
| `AIWATCHER_QUERY_CATALOG` | `contract/catalog.json` | the declared catalog |
| `AIWATCHER_CORPUS_DIR` | — | the corpus root `corpus_spans` reads (`just bench-curation-generate`) |

`contract/aiwatcher_query/config.py` is the authority on each default.

## Adding an engine

An engine's package is an `Engine` and a `__main__` that calls `serve` with a reference
to it (`contract/aiwatcher_query/engine.py`): its namespace, how rows become its frame
and back, its vocabulary and `DECLINED` table, and its volatile functions. The rest of
what one touches — a `RuntimeBinding` and `RuntimeKind` in `aiwatcher-execution`, an
executor in `aiwatcher-server/src/execution/`, Build mode's emitter and the shipped
content in the panel, seed variants, a `Dockerfile.query` target, CI's `query` matrix —
is AW-3's phase 4, which added DuckDB, laid out task by task in
[its job note](../../docs/specs/AW-3-datafusion-and-duckdb-as-query-and-curation-engines/03-job.md).
