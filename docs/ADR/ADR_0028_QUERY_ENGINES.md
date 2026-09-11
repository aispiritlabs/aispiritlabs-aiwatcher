# ADR_0028: A deployment chooses its query engine, and a typed query is admitted or runs where code runs

- **Status**: accepted; amends ADR_0008, ADR_0014 and ADR_0024
- **Date**: 2026-09-11

The change is AW-3
([`docs/specs/AW-3-…`](../specs/AW-3-datafusion-and-duckdb-as-query-and-curation-engines/02-spec.md)):
its spec is the behaviour, its job note the measurements and every change from the plan.
This is the part that would be expensive to reverse.

## Context

ADR_0008 made Flow PHP the query surface and made it safe by construction: a query is
lexed, admitted from Flow's own signatures and built through explicit dispatch, and **no
query text is ever executed**. ADR_0014 and ADR_0024 put curation on the same engine,
and ADR_0025 then made a curation something the server runs unattended — over a corpus,
not only over the read model.

That last step is where Flow's speed became the run's speed. `benchmarks/curation`
measured one aggregation over a 5 GB CSV: Flow **548.8 s**, DataFusion **1.68 s** at 280
MiB, DuckDB **2.02 s** at 677 MiB and 0.215 s at 227 MiB over the same corpus as Parquet,
Polars as fast as DataFusion with a **5.4–5.5 GiB** peak. Every one gave the same answer.
Through aiwatcher, over 1 GB, the managed step took 95.2 s on Flow, 0.85 s on DataFusion
and 0.89 s on DuckDB.

Neither fast engine has a language that can be parsed the way Flow's is. A DataFusion
query is Python calling DataFusion's `DataFrame` API; a DuckDB query is Python calling
DuckDB's relational API. Both also speak SQL, and SQL was refused as a query language
here (the user's call, 2026-09-10): a query is each engine's own Python, and a SQL string
is precisely what a DuckDB query must be kept from passing. So running one of these
queries is running Python a person typed — the thing ADR_0008 exists to avoid.

## Decision

**One engine per deployment.** `AIWATCHER_QUERY_ENGINE` is `flow` (the default),
`datafusion` or `duckdb`; the server registers that engine's executor and no other, and
anything else is a refusal at start-up naming the variable. Each engine is its own
`RuntimeKind` — `flow_php`, `datafusion`, `duckdb` — because a reactor routes a claim on
the kind without loading the plan.

**Content names the engine it was written for.** A transform, a recipe and a dataset
version carry `engine`, omitted when it is `flow`, so everything stored before keeps its
digest and every Flow plan its `plan_id`. A chain naming two engines is a chain problem;
a chain written for another engine than the deployment's is refused when it is started —
a 422 naming the block and both engines, and no run. The panel shows such content and
does not run it.

**One contract, proved.** Every engine serves the six `/query` routes with Flow's answer
— the 1 000-row cap and `truncated`, `deterministic`, `window_applied`, `digest` — over
one declared catalog, `services/query/contract/catalog.json`, which Flow loads too. A
conformance suite asks each engine the benchmark's four questions in its own language
and compares the rows with Flow's, in CI, per engine.

**The Python engines are services, and a query runs in a child.** Never in aiwatcher's
process: a Python query needs an interpreter, and a heavy query does not belong beside
the read model's memory contract. Each query runs in a child of a **fork server that
imported the engine and never ran one** — measured: a child forked after `import
datafusion` answers, one forked after the parent ran a DataFusion query panics in Tokio.

**Two admissions, and the permissive one is the default.**

- `open`: the query is Python, run as a notebook's cell is — in that child, with every
  `AIWATCHER_*` variable removed, a scratch working directory, `RLIMIT_CPU` from
  `AIWATCHER_QUERY_TIMEOUT_SECONDS` (and `RLIMIT_AS` on Linux) and a wall clock; the
  service marks itself non-dumpable on Linux and binds to `127.0.0.1`. A ceiling reached
  is a 422 naming it, because the same query reaches it on every retry.
- `strict` (`AIWATCHER_QUERY_ADMISSION=strict`): the text is parsed with `ast` and
  admitted before anything runs, ADR_0008's shape in Python — a written grammar (no
  import, definition, loop or raise), names from the query's namespace or assigned by it,
  attributes that are public members of the engine's own classes and modules, a
  `DECLINED` table per engine with a reason per name, and members whose signatures take a
  callable or a path declined by that alone. DuckDB adds a rule over a call's arguments:
  text, or anything that may hold it, reaches no argument DuckDB parses as SQL, except a
  group key of bare column names; and a `FunctionExpression` is admitted against DuckDB's
  own `duckdb_functions()`.

Under either admission a DuckDB session is **locked to the corpus root**:
`allowed_directories` is set, then `enable_external_access` is turned off, and DuckDB
refuses to change either while it runs.

## Alternatives considered

**SQL, admitted by each engine's own options.** DataFusion's `SQLOptions` can refuse DDL
and DuckDB can refuse external access, so SQL would have been one language across both.
Refused by the user in favour of each engine's Python API, and the refusal holds up in
the code: DuckDB parses a string handed to half its relation methods as SQL, and keeping
SQL out of those arguments is most of what DuckDB's strict rule is.

**Polars as a third engine.** As fast as DataFusion and a fifth of Flow's code to write
against, but it maps the whole CSV: 5.4–5.5 GiB peak for a 5 GB file against 280 MiB. A
query engine's peak is what a deployment sizes for.

**DataFusion inside the Rust process.** Measured at 93 MiB over the 5 GB CSV, the leanest
of all — and able to run only Rust or SQL, neither of which is the language here.

**One `query` runtime with the engine as a parameter.** Simpler to add to; but a claim
filter would need the plan loaded to know whether this process can run the step.

**`strict` as the default.** It is Flow's guarantee in Python, and it refuses real Python:
a loop, a function, a notebook's idioms. `open` is the notebook runtime's posture, already
accepted by ADR_0024 for the block beside it; `strict` is there for a deployment that
wants the guarantee and will write inside it.

**An empty `__builtins__` as a second wall under `strict`.** Tried, and it aborted the
process inside pyarrow: a C extension reads builtins from the frame that called it.
Admission is the one wall in Python; DuckDB's lock is a second one it provides itself.

**A fresh interpreter per query.** It pays the import every time — 464 ms for DataFusion,
47 ms for DuckDB. The fork server pays it once.

**A hand-written allowlist of calls.** The enumeration ADR_0008's second amendment
deleted, for the reason given there: a list is a queue, and it drifts in one direction.

## Consequences

**What `open` costs**, each of them accepted rather than overlooked:

- **It runs code.** ADR_0008's sentence — no query text is executed — holds for Flow and
  for `strict`, and does not hold for `open`. The ceilings bound CPU and memory, the scrub
  removes the credentials the service was given, and the scratch directory takes relative
  writes; nothing stops an `open` query reading what the service's user may read, or
  starting a process.
- **On a laptop it can reach the network**, as a notebook can. That stands outside the
  guardrail *never fetch a byte outside `integrations::fetch`*, and was raised as such in
  AW-3's job note before it was built. In a cluster, with `networkPolicy.enabled`, the
  query pod's egress is the server's Service and DNS and nothing else.
- **On macOS a same-user process can read the service's initial environment** through
  `sysctl`. There is no `/proc` and no `PR_SET_DUMPABLE`; on Linux the non-dumpable parent
  refuses a child `/proc/1/environ`, proved in the image. A credential placed in the
  service's starting environment on a Mac is readable by an `open` query there.
- **A forked child must not touch CoreFoundation on macOS.** Asking the system for its
  proxies (`SCDynamicStoreCopyProxies`) crashed every child of some fork servers and none
  of others; the child's HTTP client takes nothing from its environment for that reason
  (`trust_env=False`), which also keeps `~/.netrc` out of it. Anything new in the child
  that asks macOS for proxies, a locale or the keychain brings the crash back.
- **DuckDB's lock holds only the session's own connection.** An `open` query that imports
  `duckdb` can open another, and its module-level connection read `/etc/passwd` when
  measured. Under `strict`, which refuses the import, the lock is a second wall.
- **`RLIMIT_AS` is Linux-only.** On macOS the wall clock is the only backstop against a
  query that allocates.

**And the rest of what this costs.** A query is written for one engine: switching a
deployment strands what was written for the other, visibly — read-only in the panel, a
422 when a managed run starts or a block's context is opened. What is already running
is stranded without a word: an attempt of the old engine still pending or retrying
when the release switches is one no process claims, because only the deployed
engine's executor is registered, and it waits until its run is cancelled — so a
release switches between runs, not during one. The seed ships a variant of each pipeline for every engine
its transforms can be written in (`titanic/pipeline-php.json` has none; its steps are
this repository's own Flow ML functions). CI runs a query job per engine plus the
contract's. The conformance questions q3 and q4 end in a limit of 100 to fit the
1 000-row answer, which is narrower than the spec's "the same rows". Through the service
over 5 GB, q2 held every process under the chart's 1 GiB: DataFusion's child 129 MiB
over CSV and about 230 over Parquet, DuckDB's 259–472 MiB over CSV and 126 over Parquet,
each service under 85 MiB.

**What would make this wrong.**

- An `open` deployment reachable by anyone but its operator — the service bound beyond
  localhost without the chart's NetworkPolicy, or a panel shared with people who should
  not run code where it runs. Then `strict` is the only acceptable admission, and it
  should become the default rather than the option.
- A `strict` refusal shown to be bypassable: text reaching a DuckDB relation method by a
  route `_may_be_text` does not follow, or an engine release adding a member that reads a
  file through a parameter nothing declines — DuckDB is pybind11, so its signatures say
  nothing and its table has to. The tests that must keep failing are the defence, and are
  cheap to extend: `test_strict_refuses_text_however_it_reaches_a_relation_method` and
  DataFusion's declined-by-signature cases.
- A deployment that needs two engines at once, or an answer over 1 000 rows. Both are
  outside this decision on purpose; the second is the next spec (Parquet between steps,
  sharded dataset versions).
- Flow becoming fast enough over a corpus that the Python engines' costs stop buying
  anything — the 548.8 s is the number to re-measure.
