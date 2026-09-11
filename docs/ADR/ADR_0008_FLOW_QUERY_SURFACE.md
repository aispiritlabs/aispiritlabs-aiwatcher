# ADR_0008: Flow PHP is a query surface over the API, parsed rather than executed

- **Status**: accepted; three amendments below, and a fourth made by ADR_0028 —
  Flow is one of three query engines, and the other two run code
- **Date**: 2026-08-28

## Context

The explorer answers the questions it was built to answer: group runs by agent,
by workflow, by session, by span. It cannot answer a question nobody anticipated
— "which tool got slower after the model change", "which workflows exceed a
token budget" — because every pivot is a route someone wrote.

Flow PHP was chosen as the engine for those questions. ADR_0007 had put it on
the batch side of a live/batch split on the reasoning that it is a batch
framework. That reasoning was untested, so it was measured. On 175 000 events —
5000 runs, the read model's retention ceiling — for one `groupBy(agent)`:

| Source | Grain | Rows read | Time | Peak memory |
|---|---|---|---|---|
| Rust read model (`/dimensions/agent`) | run | 1 500 | **~5 ms** | — |
| Flow over HTTP (`from_http_paginated`) | run | 1 500 | **210 ms** | 17 MB |
| Flow over Parquet, columns pruned | event | 175 000 | **2 047 ms** | 13 MB |
| Flow over the raw write-ahead log (NDJSON, 121 MB) | event | 175 000 | **16 390 ms** | 27 MB |

**The grain decides this, not the transport.** Flow reading run summaries over
HTTP is fast because the API has already folded 175 000 events into 1500 rows.
The same engine over raw events is slow in any format; Parquet buys 8× and does
not change the order of magnitude.

Two things follow. Flow is not a slower replacement for the explorer tree — 210
ms against 5 ms, for a control that fires on every click, is not a trade worth
making. And the columnar export ADR_0007 anticipated is not needed to make any
of this work: the cursors added for the panel (`next_cursor` in the body,
replayed as a query parameter) are exactly the shape Flow's
`http_pagination_cursor` expects, so it pages the existing routes with no
adapter code at all.

The remaining problem is that a query is text a person writes, and executing
text as PHP is remote code execution.

## Decision

**A separate PHP service, `services/flow`, that the panel calls directly.**
Not through the Rust API: aiwatcher's binary has no idea this exists, which is
what lets the service be absent while the rest of the panel is unaffected. The
Query tab has a first-class "not running" screen naming the command to start it.

**Datasets are named API routes, not a copy of the data.** `runs` (aliased
`default`), `spans`, and `events`. No export, no ingest job, no second store, and
results as current as the runs list. `events` is per-run and refuses to run
without one, because walking every run would be a request per run across the
whole retention window dressed up as one query.

**The query is parsed, never executed.** `token_get_all()` — PHP's own lexer,
which lexes and compiles nothing — turns the text into tokens; a whitelist
decides what those tokens may be; and an explicit `match` in `PipelineBuilder`
is the only place a name from the query becomes a call. `$name(...)` after a
whitelist check would probably be safe. A `match` is safe without the "probably"
and stays safe if the check is ever refactored badly.

**The whitelist also declines things that exist and work badly here.** Flow
0.43's loose comparisons fall through to an array comparison when either side is
null. Measured on three rows where one column is null:

```text
ref('op')->equals(lit('execute_tool'))     -> ['execute_tool', null]   wrong
ref('op')->notEquals(lit('execute_tool'))  -> ['chat']                 wrong
ref('op')->same(lit('execute_tool'))       -> ['execute_tool']         right
ref('op')->notSame(lit('execute_tool'))    -> ['chat', null]           right
```

Every column in every dataset here is nullable, so offering `equals` by name
would be offering a filter that quietly returns the wrong rows. It is refused
with the reason and the replacement, rather than silently missing.

**Errors teach the schema.** The catalog knows the columns, so a bad reference
is caught before anything runs and answered with the fix. `ref('agent')` on
`runs` does not say "unknown column"; it says that a run can involve several
agents, that the column is `agents`, and what to write instead.

**Limits belong to the request, not the language.** 1000 rows, 30 seconds, and
`truncated` reported rather than a full page left to be misread as complete.

**Syntax diagnostics come from Mago, after enrichment.** A query is nearly PHP,
and a real PHP parser gives better syntax errors than anything worth
hand-writing (*"Expected one of `RightParenthesis`, found `Semicolon`"*, with a
line). The gap is that a query is not *quite* PHP: `->read(default)` uses a
keyword as a dataset name, and Mago stops on it — *"Unexpected token
`Default`"* — as does PHP's own `token_get_all(…, TOKEN_PARSE)`.

`Enrichment` closes the gap by substituting what the bareword stands for:
`read(default)` becomes `read('runs')`, a `declare(strict_types=1)` preamble is
prepended and a terminating semicolon appended, and every substitution is
recorded so a diagnostic's offset maps back to the character the person actually
typed. Measured at 10 ms per check, which is why `POST /flow/check` runs while
someone types rather than only on Run.

The split of responsibility is strict. Mago knows PHP and nothing about
aiwatcher — it cannot say whether a dataset, column or function is allowed, and
it is not a security boundary. The parser and builder are. Syntax is reported
first, because "unknown column" on top of a missing bracket is noise.

Mago also lints the service's own source, which is the PHP half of what
`cargo clippy -Dwarnings` does for the Rust crates. It found real things: error
suppression with `@`, an empty catch, missing assertion descriptions, redundant
`?->` before `??`. The complexity rules are disabled with reasons in
`mago.toml` — a recursive-descent parser and an exhaustive `match` are supposed
to be branchy, and the `match` in particular is the security boundary, so
collapsing it to satisfy a metric would mean dynamic dispatch.

## Alternatives considered

**Flow on the explorer's read path.** The original request. 210 ms per pivot
click against 5 ms, for a control that fires on every interaction, plus a PHP
process in the live path. Rejected on the measurement, and the measurement is
why the split is now defensible rather than assumed.

**Parquet export plus an ingest job, as ADR_0007 anticipated.** Would have been
built had the HTTP adapter not been raised. It is slower than HTTP for these
questions, and adds a job, a store and a staleness window. Deferred with a
condition (below) rather than cancelled.

**`eval` inside a hardened container.** Full Flow, immediately, and still remote
code execution with a fence around it. The parser costs more and is the only
version that can be exposed beyond a laptop.

**Generating the pipeline from the explorer's controls, read-only.** Safe and
much smaller, and it answers only the questions the explorer already answers.
The point is the ones it does not.

**Mago as the query's parser, replacing the hand-written one.** Tempting once
enrichment made it parse. Rejected: its `Mago\Sdk` is an extension API whose
`SourceFile` constructor is `@internal`, so a PHP caller cannot get a syntax
tree out of it — only diagnostics from the CLI. And a linter that has to be
present would make the security boundary depend on a dev dependency. It advises;
it does not decide.

## Consequences

PHP 8.3+ becomes a build and runtime dependency for one optional service. It is
outside the Cargo workspace, absent from `just check`, and has its own
`just flow-check` (format, lint, test).

Mago is a dev dependency, so `composer install --no-dev` leaves it out. That is
a supported state: `/flow/check` then reports the parser's diagnostics alone and
`/flow/healthz` says `linter: none`. Syntax errors are still caught — just with
the parser's own wording rather than a compiler front end's.

The whitelist is a maintenance surface: a Flow function nobody listed is a
function nobody can use. That is the intended failure direction, and the error
names the nearest allowed alternative to soften it.

A query only sees what the API serves — the read model's retention window — and
the panel says so above every result table. A result read as "all time" would be
wrong, and a table with no provenance invites exactly that reading.

The service has no authentication. It is bound to localhost by `just flow-serve`
and must not be exposed without one; the parser bounds what a query can *say*,
not who may ask.

**What would make this wrong.** If a question needs history older than
retention, HTTP cannot answer it at any speed and the columnar path from
ADR_0007 §6 becomes necessary — that is its condition. If typical queries start
taking seconds rather than the ~200 ms measured here, the grain being read is
probably wrong, not the engine. And if a later Flow fixes the null handling in
its loose comparisons, the declined list should shrink; the test named
`test_the_loose_comparisons_are_declined_with_the_reason` is where that will
show up.


## Amendment, 2026-09-08: four statistics that are not Flow's

Flow's aggregations are `count`, `sum`, `average`, `min`, `max`, `first`,
`last` and the two collectors. That is "how many" and "how much", and nothing
that says how a column is *distributed* — no median, no quantile, no deviation.
Half of what anybody opens a curation to find out is missing, and the gap shows
up the moment a query is pointed at something other than run summaries: a
median fare and a mean fare on the Titanic corpus are different numbers, and a
dataset version that published the mean is a dataset version whose reader
cannot tell which one it got.

**Decision: `median`, `stddev`, `variance` and `percentile` join the whitelist,
implemented here over [hi-folks/statistics][lib] (MIT, no dependencies of its
own, the Python `statistics` module's definitions, PHPStan level 8).** They are
`Statistics\Statistic`, an `AggregatingFunction` Flow runs like its own, and
`Dsl\PipelineBuilder` constructs them in the same explicit `match` every other
name goes through. Writing the quantile interpolation by hand was the
alternative and is how you get a wrong number that looks right.

Three things this deliberately does not do.

**It does not widen [`Dsl\Registry`].** That class admits Flow's *own*
namespace by return type, which is a rule about a vocabulary somebody else
maintains and adds to. These four are ours and are listed by name in
`Whitelist::AGGREGATIONS`, exactly as `aggregate()`'s other arms are. The
boundary is untouched: a name from a query still selects a branch and never
becomes a callable.

**It does not answer zero for a group it has no answer for.** Every one of
these is undefined below some number of values — one for a median, two for the
rest — and below it the column is `null`. Flow's own `average()` answers 0 for
an empty group; that is its choice and the wrong one to copy here, because a
curation is read months later by somebody who was not there when it ran.

**It does not make a statistic available per row.** An aggregation answers one
row per group, and there is no join in this language to put that answer back
beside the rows it was taken over. `withEntry('typical_age', median(ref('age')))`
is refused *by name*, saying where a statistic belongs — because the mistake it
invites is the useful one to answer: filling a missing age from its group's
median is exactly what the `titanic_features` notebook block does after the
query, and why ADR_0024's chain has more than one engine in it.

[lib]: https://github.com/Hi-Folks/statistics


## Amendment, 2026-09-08 (second): the whitelist is deleted; admission is derived

The decision above says "a whitelist decides what those tokens may be", and by
the time the statistics landed that sentence had stopped being true. `Registry`
— added earlier to stop the vocabulary being an enumeration — derives admission
from Flow's own signatures: an admitted return-type namespace, and no parameter
that accepts a callable. `Whitelist::isValueFunction` consulted **both**, with
`||`, so the effective vocabulary had been `Registry`'s for as long as it had
existed. Of the 24 names the two lists still held, 23 were already admitted by
signature and one — `identical`, which returns a *join* comparison — could not
be used in any position this surface has.

A list that decides nothing and reads, to whoever opens the file, like the
boundary is worse than no list. `Dsl\Whitelist` is deleted.

**What decides what a query may name.** `Registry`, alone, for everything Flow
offers; plus two enums for the two vocabularies that are not Flow's and are
their own implementation — `Dsl\Sink` (the three loaders that mean "give the
rows back") and `Statistics\Descriptive` (`median`, `stddev`, `variance`,
`percentile`). A case is a `match` arm somebody wrote, so neither can grow by
accident.

**What decides where a name may appear.** The type the name turned out to
build, not a second list of names. `aggregate()` takes what is an
`AggregatingFunction`; `withEntry()` takes a `ScalarFunction` and refuses an
aggregation *by saying what it is*; `write()` takes a `Sink`. The old
`aggregate() takes aggregations: count, sum, …` recited a list; the message now
says `lower() is not an aggregation: it answers a value per row, so it belongs
in withEntry() or filter()`.

**What is still a written list, and why.** The pipeline steps and the methods
chainable on a value (`Parser::STEPS`, `Parser::REFERENCE_METHODS`). Neither is
a vocabulary question: a step is a `DataFrame` method this service implements
with a `match` arm, and a chained method is the one place a name from a query
would have to reach a method — which is exactly the dispatch ADR_0008 refuses.
That is also what keeps `->over(window()->partitionBy(…))` closed: Flow has
window functions, and they are not reachable from here.

**Nothing widened.** The admitted set lost `identical` and gained nothing;
`to_csv`, `from_array`, `files`, `call` and `to_callable` are refused by
`Registry` exactly as before, and `ParserRejectionTest` now proves each
category by name rather than by their absence from a list. What changed is that
there is one place to read to find out what a query may say.


## Amendment, 2026-09-08 (third): three surfaces, all derived; join and windows open

Deleting the whitelist left three questions being answered three ways: what a
query may *name* was derived (`Registry`), but which *steps* a pipeline could
take and which *methods* a value supported were still two hand-written lists —
fourteen steps and twenty methods. Those two lists were the reason this query
language was described, in this repository and in its panel, as having no
arithmetic, no join and no window functions. None of that was true of Flow.
`DataFrame` has 53 public methods including `join`, `crossJoin` and `offset`;
an `EntryReference` has over a hundred and thirty fluent methods including
`plus`, `minus`, `divide`, `regexReplace` and `dateFormat`; an aggregation has
`over()`. Every one of them was reachable in the engine and unreachable here
because nobody had typed its name.

**Decision: the same signature rule decides all three surfaces.**
[`Admission`] holds it once — a call is refused when any parameter accepts a
**callable**, a **Loader**, an **Extractor**, a **Path**, a **Filesystem**, a
**Transformer**, a **SaveMode** or Flow's own evaluation machinery — and three
thin classes apply it: `Registry` for functions (return type in Flow's value
namespaces), [`Frame`] for steps (returns a frame), [`Values`] for methods (on
the class of the object the query actually built). On Flow 0.43 that is 108
functions, 25 steps and per-value method sets, all derived.

**Dispatch is now a `ReflectionMethod` looked up in one of those maps, invoked
against the frame or the value in hand.** That is a real change from "an
explicit `match` is the only place a name becomes a call", and the reasoning is
that the property which mattered was never the `match` — it was the *reachable
set*. A `match` gave that set by hand and got it wrong in the safe direction
for a year; the rule gives it by type and keeps giving it when Flow adds a
method. What is still true, and is the sentence to keep: **a global call by
string appears nowhere**, and every name is resolved against something looked
up first.

**What this opens.** `join(<a whole query>, on: join_on(identical(…)), type:
'left')` — the right-hand side is a nested query read through the same catalog
and the same window, because joining what another query worked out is the case
that matters. `median(ref('age'))->over(window()->partitionBy(ref('status')))`
— a group's answer beside every row. `ref('sib_sp')->plus(ref('parch'))` —
arithmetic. Together they made the Titanic chain's notebook block unnecessary:
the whole feature engineering is one query, and `ADR_0024`'s seam is where it
always should have been, at "the query language has no vocabulary for this at
all" rather than at "our list is short".

**What this does not open.** `write` and `load` take a `Loader`; `map` and
`forEach` take a `callable`; `transform` takes a `Transformer`;
`filterPartitions` takes a `Path\Filter`; `saveMode` takes a `SaveMode`. All
refused by the rule rather than by their absence, which is what
`ParserRejectionTest` now proves one category at a time. `cache(?string $id)`
is the one refusal a signature cannot make — a nullable string is not a path as
far as a type is concerned, and it writes files under a name the query chose —
so it is declined by name with the reason, the way `equals` is.

**One coercion, and only one.** Some of Flow's parameters take a pure enum;
`Rounding` on `divide()` is the one that bites, and without a rounding mode a
division throws from inside Brick\Math the moment it is not exact. `::` is not
part of this language and never will be, so a written string is matched against
the enum's case names — `->divide(ref('n'), lit(2), 'half_up')` — from the
parameter's own declared type.

**What would make this wrong.** If a Flow release adds a method that reaches
outside the process through a parameter type none of the refusals name, the
rule admits it silently. That is the cost of deriving, and the mitigation is
that the refusals are about *categories* rather than classes: a new sink still
takes a `Loader`, a new source still takes an `Extractor`. The test that would
catch it is `stepsTheAdmissionRuleRefuses`, which is a list of things that must
keep failing — and, unlike the whitelist, it is cheap to extend and costs
nothing when it is right.


## Amendment, 2026-09-11: one of three engines (ADR_0028)

Flow is no longer *the* query surface. It is the default of three that a deployment
chooses between with `AIWATCHER_QUERY_ENGINE`, beside DataFusion and DuckDB, and
everything this ADR decided about Flow stands: a Flow query is lexed, admitted from
Flow's own signatures and built through explicit dispatch, and it is never executed.

What changes is the sentence around it. *No query text is executed* is now true of Flow,
and of the two Python engines under `strict` admission — a query parsed with `ast` and
admitted from the engine's own vocabulary before it runs, this ADR's shape in another
language. It is false of them under `open`, the default, where a query is Python run as
a notebook's cell is: in a child process, with ceilings and without credentials, on a
service bound to localhost. [ADR_0028](ADR_0028_QUERY_ENGINES.md) states what that costs
and what would make it wrong.

Three things moved with it. The routes are `/query/*`, and Flow serves `/flow/*` beside
them for one release. The service lives at `services/query/flow`. And the declarative
half of the catalog — names, routes, columns, parameters, windows — is
`services/query/contract/catalog.json`, which every engine loads, so the datasets a
query can name are one list in one file rather than one per language.
