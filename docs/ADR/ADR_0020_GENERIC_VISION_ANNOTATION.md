# ADR_0020: The annotation tool ships no vocabulary, and the schema carries the domain

- **Status**: accepted
- **Date**: 2026-09-02
- **Amends**: [ADR_0017](ADR_0017_IMAGE_ANNOTATION.md), [ADR_0019](ADR_0019_DATASET_HUB_DISCOVERY.md)

## Context

ADR_0017 built the annotation registry to solve a floor-plan problem, and it
solved it: a segmentation mask cannot say which wall an opening sits on, so the
vector shape became the source and every raster a derivation. Every decision in
it still holds.

What also happened is that the domain leaked into the tool. By the time the
training path was finished, aiwatcher itself contained:

* a shipped label vocabulary — walls, spaces, doors, windows, passages, stairs,
  columns — served from `/api/v1/annotation-presets`;
* a `ViewType` enum whose variants were `FloorPlan`, `Section`, `Elevation`,
  `SitePlan`, with an export that hardcoded "not a floor plan" as an exclusion
  reason;
* four hundred lines of floor-plan corpora in `sources`;
* an SDK rasteriser that matched on the string `"wall"` and produced two grids
  named `structure` and `openings`.

None of that is wrong for floor plans and all of it is wrong for anything else.
A team annotating weld defects, retinal scans or satellite tiles would find a
tool that already knows what a door is, cannot be told otherwise without a
schema change in somebody else's repository, and ships a corpus table about
buildings.

The deeper problem is that the leak was invisible from inside. The floor-plan
vocabulary looked like a helpful default rather than a constraint, right up
until the first non-plan corpus.

## Decision

**aiwatcher ships no vocabulary. The project's label schema carries the domain,
and every mechanism reads it.** Four changes:

**The preset goes.** `floor_plan_classes()` and
`/api/v1/annotation-presets` are removed. A project supplies its classes. The
panel offers a *shape* to edit — one filled class, one stroked class, one
`ignore` class — which demonstrates what a schema can say without asserting
what this corpus is of.

**`view` is free text.** It was an enum of drawing types; it is now a string
alongside `level`, and an export names the views it wants rather than
hardcoding one. A corpus with a single kind of picture never sets it.

**`LabelClass` gains a `layer`.** This is the one addition, and it is the
generic form of the problem the two-headed model was solving. Some classes
*overlay* others and must not erase them — an opening in a wall, a defect on a
component, a marking on a road — and one grid can only represent the overlay by
deleting what is underneath. Classes on one layer share a grid and paint in
declaration order; classes on different layers never contend. A schema with one
layer gets one grid and never thinks about it.

**The corpus table ships empty and loads from configuration.**
`AIWATCHER_DATASET_SOURCES` names a JSON catalogue. The types and the
reconciliation stay, because ADR_0019's guardrail is generic and is the whole
point of searching a hub; the rows go, because which corpora exist and what
their licences permit is a question about one field.

The SDK rasteriser follows from all four: it takes the schema, paints by
declared geometry, uses each class's own `ignore` flag, and produces one grid
per layer. It knows no class name.

## Alternatives considered

**Keep the preset as a "starting point".** The framing that let the leak
happen. A default vocabulary is not neutral — it decides what the first hour of
labelling produces, it is what the panel renders, and it is what every example
in the documentation shows. A tool that ships one is a tool for that domain
with an escape hatch, not a generic tool.

**Parameterise the rasteriser's class list instead of reading the schema.**
Smaller, and it moves the problem: the caller then has to re-derive the layer
split, the ignore set and the paint order from a schema that already states all
three. Two descriptions of one vocabulary is one that can disagree.

**Keep `ViewType` as generic enough.** A section and an elevation are not
floor-plan-specific *ideas*, so this was tempting. It is still a closed list of
one field's drawing types, and a corpus of photographs and diagrams has no
member of it.

**Move the whole annotation module into planner.** The maximally clean split,
and it throws away the reason any of this is in aiwatcher: a model version
names the export it was trained on, an agent span names a model, and that join
only exists because both ends are in one system (ADR_0018).

**Delete `sources` outright.** Considered under ADR_0019's own terms and
rejected there for the same reason it is rejected here: the mechanism is what
makes hub search safer than reading a mirror, and it is domain-neutral. Only
the rows were domain content.

## Consequences

Starting a project now costs a vocabulary decision that used to be free. That
is the intended trade and it is not free either way: the cost of the preset was
paid by every project that was not about buildings, silently.

The default catalogue being empty means every hub result is `unclear` until an
instance loads a table. This is a safe default rather than a degraded one —
`unclear` rights make a commercial export exclude the images by name — but a
deployment that wants the guardrail to do anything has to write its table.

`/api/v1/annotation-presets` is gone from the contract, so the generated client
loses a method. That is a breaking change for anything calling it, which today
is the panel and nothing else.

The `layer` field is the one piece of new surface, and it is the one that could
be wrong. It assumes overlays are *layered* rather than arbitrarily nested — a
class that overlays an overlay needs a third layer, and a vocabulary where
which-overlays-what depends on the instance rather than the class cannot be
expressed at all. Neither case has appeared; both would need a different model.

**What would make this wrong.** If every real corpus turns out to want the same
handful of geometric roles — a filled thing, a stroked thing, an overlay — then
the vocabulary was not the domain after all and shipping a *structural* preset
under neutral names would save everybody the same hour. Watch what the second
and third corpora declare: three schemas that differ only in class names would
say the preset should come back, with the names as configuration.
