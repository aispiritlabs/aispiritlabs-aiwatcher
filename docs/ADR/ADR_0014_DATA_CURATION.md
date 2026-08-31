# ADR_0014: Flow executes curation; the Rust registry versions its scripts and outputs

- **Status**: accepted
- **Date**: 2026-08-31

## Context

The Flow query surface in ADR_0008 could validate and run a transformation, but
it deliberately kept no state. That was sufficient for questions over retained
runs and insufficient for datasets: a useful curation must be reviewable before
it writes, repeatable later, and pinned to the exact output an evaluation used.

Production runs are the natural source. A session groups their conversation and
an agent dimension selects one or many participants. The retained summaries do
not contain prompt or completion bodies by design, so promotion must keep source
run/session/trace identifiers and allow deliberate event fields rather than
quietly promising content the collector removed.

The Langfuse dataset model supplied the product constraints: production traces
can become cases, field mapping is explicit, expected output may be added later,
and dataset changes create reproducible versions.

## Decision

**Flow PHP remains the execution engine.** `/flow/check` validates without
reading data, `/flow/simulate` executes with a 25-row cap and no write, and
`/flow/query` is the full bounded execution. Curation adds `dropDuplicates`,
`rename`, `all`, and `any` to the parsed whitelist; text is still parsed and
resolved through explicit dispatch, never evaluated as PHP.

**The recipe may pin its relative period at the read boundary.**
`read(default, period: '24h')` overrides the panel's fallback window and the
effective seconds are stored with the dataset version. Durations accept minutes,
hours, days or weeks, while `period: 'all'` reads the whole retained window.
Applying the period at the HTTP read avoids fetching all history only to filter
it inside Flow.

**The browser coordinates execution and persistence.** It sends the script to
Flow, receives the rows, and publishes that exact script plus those exact rows
to the Rust API. A truncated execution is refused as a dataset version rather
than saved as if it were complete.

**The authenticated Rust API owns writes.** `POST /api/v1/curations` stores a
content-addressed recipe revision and `POST /api/v1/datasets` stores a
content-addressed dataset version. Both require `editor`. The PHP service stays
stateless and does not gain a second authentication or storage implementation.

**Datasets share the configured authored object store with prompts under their
own key prefix.** File storage works locally and the same S3-compatible store
works in production. Dataset and recipe names may contain slash-separated
folders, but keys use SHA-256 identifiers so a name is never interpreted as a
filesystem path.

## Alternatives considered

**Write artifacts from the PHP service.** It would make execution one request,
but turns an optional stateless query service into a durable authenticated
writer and makes every replica coordinate a filesystem or S3 client. Rejected.

**Keep recipes in local storage and download JSON results.** Useful for a demo,
but not a shared source of truth and not something an evaluation can name
reliably. Rejected.

**Store only a generated query, then re-run it when evaluating.** Retention and
live production state would change underneath the name. A script is provenance;
the immutable rows are the dataset. Rejected.

## Consequences

A dataset version is capped at 1,000 rows and 4 MiB, matching the request and
Flow result boundaries. Larger batch datasets need an asynchronous object-store
writer rather than larger HTTP bodies. The first version is generic rows: schema
enforcement and item-level labelling can be added without changing the
content-addressed version rule.

Recipes and datasets are unavailable when `AIWATCHER_PROMPT_STORE=none`, because
that setting currently controls the shared authored object store. The name is
historical and should become a general registry setting if another authored
artifact joins it.

**What would make this wrong.** If routine curated datasets exceed 1,000 rows or
4 MiB, or curation becomes scheduled rather than interactive, browser-mediated
publishing is the wrong transport. Flow should then write a staged artifact and
ask the Rust registry to commit its manifest, while preserving the same recipe
and content identities.
