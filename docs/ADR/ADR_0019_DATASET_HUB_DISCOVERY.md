# ADR_0019: A dataset hub is searched for what exists and never asked what is permitted

- **Status**: accepted, amended by [ADR_0020](ADR_0020_GENERIC_VISION_ANNOTATION.md)
  — the reconciliation below is unchanged; the curated table it reconciles
  against now ships empty and is loaded from `AIWATCHER_DATASET_SOURCES`.
- **Date**: 2026-09-02

## Context

ADR_0017 gave annotations a `sources` table: a dated list of public floor-plan
corpora, each linking its original, each carrying a conservative usage verdict
somebody wrote by hand. Its module docstring states the reason it is a table
and not a client — Hugging Face, Kaggle and Roboflow Universe all mirror these
corpora, and all of them restate licences wrongly often enough that fetching
one live would be *worse* than useless, because it would arrive looking
authoritative.

That table has eight rows. The corpus a project actually needs is usually not
one of them, and finding out what exists means leaving the tool: a search on
two hubs, a licence read at each original, and a manual note of what was found.
Meanwhile the images that *are* found have to be registered one HTTP call at a
time, with the split key — the building — supplied by hand per image.

So there are two separate questions being conflated by the absence of a search:

* **What exists?** A discovery question. It has no wrong answer that costs
  anything, and a hub answers it far better than eight rows ever will.
* **What may we train on?** A permission question with an expensive wrong
  answer, which surfaces in a legal review rather than in a metric.

The first live search run against Hugging Face made the distinction concrete in
one row: `Voxel51/FloorPlanCAD` declares `cc-by-sa-4.0`. FloorPlanCAD's authors
state that the drawings are not theirs to license and the annotations are
CC BY-NC. The mirror is not lying; it is a field somebody filled in.

## Decision

Search the hubs. Never let one answer the second question.

`aiwatcher-annotations::hubs` searches Kaggle and Hugging Face and returns rows
in which the two questions are **two fields that are never merged**:

* `claimed_license` — what the hub says, verbatim, named for what it is.
* `usage` — aiwatcher's verdict, which is `unclear` for every row unless it
  matched the curated table, in which case `curated_source` names the row a
  human read.

Importing is a separate route, `POST /api/v1/annotation-imports`, taking rows a
**Flow PHP pipeline** produced. Rights are asserted once for the batch, by a
person, and default to `UsageRights::Unknown` — which a commercial export then
excludes by name, in a manifest, forever. The one licence decision aiwatcher
makes *against* the caller is refusing a commercial claim on a batch that
matched a curated research-only corpus.

Both hubs are off by default. `AIWATCHER_HUGGINGFACE_ENABLED` is a switch
rather than a credential because the dataset search is public; Kaggle needs
both halves of a credential and either alone is not one. Unconfigured is a 501
naming the variable, not an empty list — an empty search result is a claim
about the world, and a deployment that never searched must not make it.

### Why the mapping is a Flow query

Every hub lays its files out differently, and the columns a registration needs
— the image's dimensions, and the *building* it belongs to — are not in a
search result at all. A mapping written in Rust would be a `match` on hub names
that grows by one arm per corpus and is wrong for the next one.

A Flow pipeline is where it belongs: ADR_0014 already versions those scripts,
ADR_0008 already parses rather than executes them, and the panel already
renders one before it runs. Two datasets were added for it — `hub_datasets`
reads the search, `annotation_images` reads what a project already holds, so a
second run of an import can skip what the first one registered.

### Matching a mirror to a curated row

On tokens, not substrings, and the bound is not decorative. `RPLAN` is a
substring of `floorplans`, so a plain `contains` handed
`wall-constrained-floorplans-manual-only` RPLAN's licence verdict — a
permission claim invented by a coincidence of spelling. A candidate matches
when it is a whole token; only a candidate of eight characters or more may
match across separators, so `cubicasa-5k` still finds `cubicasa5k` while
`rplan` cannot find `floorplans`. Both directions of failure are safe: a miss
leaves the row `unclear`, which is where every row starts.

## Alternatives considered

**Map the hub's licence tag onto `SourceUsage`.** The simplest thing, and
exactly the failure ADR_0017 exists to prevent. A CC BY-NC corpus re-uploaded
as MIT would import as commercially clear, and nothing downstream would ever
say otherwise — the export would include it, the manifest would record it as
permitted, and the model trained on it would carry a claim nobody checked.

**Keep the table and add no search.** The status quo, and it is not neutral:
people search anyway, in a browser, and write down what they find without the
curated table in front of them. Putting the search beside the verdict is what
makes the verdict visible at the moment somebody needs it.

**Fetch the licence file from the repository itself.** Better than the tag and
still wrong. A `LICENSE` in a mirror is as much the uploader's guess as the
tag is, and the corpora that matter here — CubiCasa5K, FloorPlanCAD, ZInD —
state their terms in a paper or a request form rather than in a file.

**Download and register images in Rust.** Rejected for the reason the mapping
is a Flow query: it is one `match` arm per hub, unversioned, unreadable, and
wrong for the next corpus.

**Refuse an import whose rights are unknown.** Considered and rejected as too
strict in the wrong direction. Unknown rights are a *usable* state — an
experiment whose weights are thrown away is a legitimate thing to do — and the
export policy already handles it correctly by exclusion. Refusing here would
teach people to claim a licence in order to get past the dialog, which is the
one outcome worse than an honest `unknown`.

## Consequences

An instance that enables a hub makes outbound requests to a third party. That
is a decision somebody makes rather than inherits, which is why both hubs are
off by default and why the Kaggle credential lives only in configuration.

A search is a partial answer when one hub is down: the per-hub status carries
the failure and the other hub's results are still returned. A 502 for a
half-successful search would throw away the half that worked.

The curated table is now load-bearing in a second way. It was a signpost; it is
now also what decides whether a hub row may be imported as commercially usable.
A row added to it wrongly is worse than a row missing from it, and that
asymmetry is why `SourceUsage` errs to `Unclear` and why `verified_on` is a
required field.

Importing gives a project images with no drawings. That is the intended state —
they are there to be annotated — but a project's image count now says less than
it did about how much labelled data exists. The export's counts are the ones
that mean anything.

**What would make this wrong.** Two observations. If a hub starts publishing a
licence that is *verifiably* the original's — an SPDX identifier resolved
against a registry the corpus authors themselves control, rather than a field
an uploader typed — then the reconciliation could take it, and this ADR's
central refusal would be over-cautious. And if the curated table grows past
roughly thirty rows while token matching keeps producing wrong pairings, the
match should become explicit: a `mirrors:` list per curated row, naming the hub
ids somebody checked, rather than a heuristic over names.
