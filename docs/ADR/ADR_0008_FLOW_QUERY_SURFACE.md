# ADR_0008: Flow PHP is a query surface over the API, parsed rather than executed

- **Status**: accepted
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
