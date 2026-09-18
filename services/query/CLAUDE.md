# Query engines

The rules for `services/query` — `flow` (PHP) and the `contract`, `datafusion`
and `duckdb` Python workspace. Read
[ADR_0008](../../docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md) and
[ADR_0028](../../docs/ADR/ADR_0028_QUERY_ENGINES.md). None of this is in the
Cargo workspace and `just check` does not cover it; CI runs each engine's own
job.

- **Never let query text reach a callable.** In `services/query/flow` a name
  selects a `match` branch, is never called by string, and there is no `eval`
  anywhere in the service. `tests/Dsl/ParserRejectionTest.php` is the list of
  things that must keep failing; adding to it is cheap and is the point.
  `$name(...)` on a global appears nowhere and never may — a `ReflectionMethod`
  taken from `Frame` or `Values` and invoked against the frame or value in hand
  is *not* that: the reachable set is fixed, derived from types, and every
  member reshapes rows.
- **Never answer "what may a query reach" with a list.** `Dsl\Admission` refuses
  any call with a parameter accepting a callable, a `Loader`, an `Extractor`, a
  `Path`, a `Filesystem`, a `Transformer`, a `SaveMode` or Flow's evaluation
  machinery, and `Registry` (functions), `Frame` (steps) and `Values` (methods
  on the value in hand) apply it — including which position a name may appear
  in, decided by what the builder constructed rather than by a second list of
  names. The lists this replaced were why this language was said to have no
  arithmetic, no join and no window functions, none of which was ever true of
  Flow. A name added by hand means the rule did not cover it, which is a reason
  to look at the rule. What stays written is `Parser::STEPS` and
  `Parser::REFERENCE_METHODS`, and the `match` arms for steps needing what a
  signature cannot know — the catalog, the window, which columns exist
  afterwards.
- **Never let a refusal a signature cannot make go unnamed.** `cache(?string
  $id)` passes every type rule and writes files under a name the query chose; it
  is in `Frame::DECLINED` with the reason, as `equals` is in
  `Registry::DECLINED`. A wider type rule would refuse innocent strings
  everywhere else.
- **Never let a query name something that opens a source or writes a sink.** The
  catalog decides what may be read and `write()` decides where rows go, so
  `Loader`, `Extractor` and `Filesystem` are outside the admitted return types —
  `to_csv('/etc/…')` in a service with no authentication is a file write whose
  name looks as harmless as any other.
- **Never decide admission by loading a class.** `class_exists()` autoloads, and
  `Flow\ETL\Function\Uuid` throws at load without `ramsey/uuid`. The return type
  is matched as a **string**, or the service fails to start over an optional
  dependency of a function nobody called.
- **Never make a query write `::`.** Some Flow parameters take a pure enum —
  `Rounding` on `divide()` — so a written string is matched against the enum's
  case names from the parameter's declared type. That is the *only* coercion in
  the builder, and it is derived rather than listed.
- **Never offer Flow's loose comparisons.** In Flow 0.43 `equals` matches null
  against anything and `notEquals` drops nulls, and every column in every
  dataset is nullable. Refused *with the reason*, because "unknown function"
  sends somebody looking for a typo.
- **Never let a statistic answer zero for a group it has no answer for.**
  `median`, `stddev`, `variance` and `percentile` are this service's own
  aggregations over hi-folks/statistics, and each is undefined below some number
  of values — one for a median, two for the rest — below which the column is
  **null**. Flow's `average()` answers 0 for an empty group, which is the wrong
  choice to copy: a dataset version is read months later by somebody who was not
  there, and `0` is a claim nobody made. They are admitted by
  `Statistics\Descriptive::tryFrom` rather than by `Registry`, because that
  class's rule is about Flow's namespace and these are ours.
- **Never refuse a windowed statistic the way a bare one is refused.** An
  aggregation on its own answers one row per group, so `withEntry('typical_age',
  median(ref('age')))` is refused — and the message names *both* ways out,
  because `->over(window()->partitionBy(…))` answers it beside every row.
  `PipelineBuilder::isWindowed` asks the value rather than the name:
  `WindowFunction::window()` throws when there is no OVER clause, which is
  Flow's way of saying "not windowed".
- **Never give a transform a second representation.** `BlockSpec::Transform` is
  Flow DSL text and stays text. A structured model would be the hand-written
  function list `Dsl\Registry` replaced, and structure *beside* the text is
  worse than either: two authored representations free to drift, with no rule
  saying which is the truth. A second query engine reads `FlowSourceRef` and
  re-authors the transforms.
- **Never make the security boundary depend on Mago.** It is a dev dependency
  and may be absent: it reports syntax, `src/Dsl` decides what runs. `just
  query-check` is the service's own gate, and CI's `query` job runs it.
- **Never expose a query engine without authentication.** None has any. Flow's
  parser and a Python engine's `strict` admission bound what a query can *say*,
  not who may ask, and under `open` a query is code. `just query-serve` binds to
  localhost and the chart's NetworkPolicy admits the panel and the server and
  nothing else (ADR_0028).
- **Never run a query in a process that has run one.** A Python engine's fork
  server imports the engine and runs nothing; every query, `strict` ones too,
  runs in a child. Measured: a child forked after `import datafusion` answers,
  one forked after the parent *ran* a query panics in Tokio's I/O driver. For
  the same reason a DuckDB connection is made in `open()`, in the child, and its
  function catalog is read lazily.
- **Never let a forked query child ask macOS for anything.** Asking the system
  for its proxies calls CoreFoundation in a process forked and never exec'd, and
  it crashed every child of some fork servers and none of others — which reads
  as a flaky engine. The child's HTTP client takes nothing from its environment
  (`trust_env=False`), which also keeps `~/.netrc` out of it.
- **Never hand a DuckDB relation text under `strict`.** A string handed to an
  expression is a column name or a constant; one handed to a relation method is
  parsed as SQL — `project("x + 1")` adds one, and SQL can name a file. So
  `strict` refuses anything that may be text in any relation method's arguments,
  except `set_alias`, a join's kind and a group key of bare column names. Under
  either admission the session is locked beneath that: `allowed_directories` is
  the corpus root and `enable_external_access` is off.
- **Never run a plan on an engine it was not written for.** A transform's text
  belongs to one language, so a chain naming another engine than the
  deployment's is refused when it is started — a 422 naming the block and both
  engines, and no run — rather than sent to an engine that reads it as a syntax
  error somebody takes for their own. The panel shows such content and does not
  run it.
- **Never remember a result the query engine says is not deterministic.**
  `now()`, `uuid_v4()` and `random_string()` are honest work in the Query tab
  and a wrong cache entry in a managed step. Every engine answers
  `deterministic` beside `window_applied`: false for a corpus read and for a
  volatile call — DuckDB's from the stability `duckdb_functions()` reports,
  DataFusion's from a declared set, because its binding says nothing about
  volatility.
