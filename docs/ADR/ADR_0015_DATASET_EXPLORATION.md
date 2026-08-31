# ADR_0015: Dataset exploration uses slices and immutable dataset references

- **Status**: accepted
- **Date**: 2026-08-31

## Context

Dataset versions contain up to 1,000 rows and 4 MiB. Returning an entire artifact
to inspect its first rows makes the browser pay the maximum cost before the user
has asked for it. The Hugging Face Dataset Viewer provides the useful reference
shape: rows and their feature schema arrive in bounded slices, while search and
filtering happen before the slice is returned.

Datasets are also the join point between curation and evaluation. A collection
name alone is mutable; an evaluation that only records that name cannot prove
which cases it scored.

## Decision

**The viewer reads bounded slices.** `GET /api/v1/dataset-rows` accepts `name`,
an optional immutable `version`, `offset`, `limit` (maximum 100), and a
case-insensitive `search`. The object-store artifact is decoded on the server,
searched, and sliced before crossing the network. The response includes stable
original row indexes, the total and matching counts, columns, provenance, and
the next offset.

**The panel uses TanStack Table and lazy network loading.** It requests 50 rows
initially and asks for the next slice only when scrolling approaches the end.
The table schema is dynamic because curated rows are intentionally generic.
Selecting a row opens its complete JSON without widening every table cell.

**The canonical evaluation join is `dataset-name@version-sha256`.** New
evaluation producers SHOULD record this string in `eval.started.data.dataset`.
The dataset page also queries the legacy collection name so reports written
before this decision remain discoverable, but only the versioned reference is
an exact comparison boundary.

**Lineage is navigable in both directions.** A dataset version links to the
saved Data Curation recipe and exact Flow PHP pipeline. Evaluation reports link
back to the collection/version they name. The dataset's Evaluations tab lists
every retained report for the exact or legacy reference.

**Experiments reuse the same reference.** An evaluation already names its
`variant`; together `dataset@version + variant + suite` identify the quality
side of an experiment. The Experiments URL accepts the dataset and variant now.
The remaining backend work is to add `variant` as a trace dimension, which will
join quality to latency and token cost without inferring identity from model or
workflow names.

## Consequences

The first page is cheap and a 1,000-row dataset never mounts or crosses the
network all at once. Search currently scans at most the bounded 4 MiB artifact
after object-store retrieval; that is appropriate at today's cap. If artifacts
move beyond that cap, row groups and a searchable columnar index must replace
JSON artifact scans rather than increasing the endpoint limit.

Offset pagination is stable because dataset versions are immutable. It would
not be appropriate for a mutable collection.
