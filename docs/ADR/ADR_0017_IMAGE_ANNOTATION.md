# ADR_0017: An annotation is authored, vector-first, and split by family rather than by image

- **Status**: accepted, amended by [ADR_0020](ADR_0020_GENERIC_VISION_ANNOTATION.md)
  — every decision below still holds, but the floor-plan *vocabulary* it
  describes is no longer shipped: a project supplies its own classes, and
  `view` is free text rather than an enum.
- **Date**: 2026-09-01

## Context

The first vision model this stack has to support reads a raster floor plan and
returns geometry: walls with a centreline and a thickness, rooms as polygons,
doors and windows as segments on a wall with a hinge and a swing, plus the one
dimension line that fixes the scale. Nothing in aiwatcher could hold that.

Three facts about the material forced most of what follows.

**A mask is not the answer, so a mask cannot be the source.** A segmentation
mask cannot say which wall an opening belongs to, which way a door swings, or
which two rooms an opening connects. Those are the fields the downstream JSON
has to carry, so the annotation has to carry them too. Anything drawn as pixels
loses them at the moment it is drawn.

**Catalogue floor plans come in families.** One house is published as the plain
plan, its mirror, a garage variant and a re-drawn revision. They are four
images and one building. Split those across train and test and the test score
measures whether the network memorised a house.

**The best public data is the data that cannot ship.** CubiCasa5K is CC BY-NC,
FloorPlanCAD's annotations are CC BY-NC over drawings its authors do not own,
LIFULL HOME'S is academic-access only. They are the right pre-training corpora
and the wrong production corpora, and the difference does not show up in any
metric — it shows up in a legal review, after the model is trained.

Roboflow and CVAT supplied the product shape. Roboflow's is the workflow —
upload, label, version, export, and a version is immutable. CVAT's is the data
model — a label carries typed attributes, and a shape is a polygon, a polyline,
a rectangle, a point set or a skeleton, not a paint stroke.

## Decision

**The vector annotation is the source of truth; every raster is derived.**
An `AnnotationRevision` holds shapes, attributes and links. Masks, heatmaps,
COCO documents and YOLO label files are produced from it and never stored as
the authoritative copy. Changing the model's input representation is then a
re-export rather than a re-annotation.

**It is an authored artifact, so it lives beside prompts and datasets.**
Everything folded from the event log is bounded by retention. An annotation is
not observed and must outlive every run that used it, so it goes into the same
configured object store behind `core::prompts::ObjectStore`, under its own
`annotations/` prefix, in the crate `aiwatcher-annotations`.

**Identity is content, exactly as it is for a prompt.** A revision id is the
SHA-256 of its canonical shapes plus the schema version and image it names, so
saving the same drawing twice is one revision. An image id is the SHA-256 of
its bytes. An export id is the SHA-256 of its manifest. Nothing here mints a
UUID.

**Review state is a label on a revision, kept in the head.** A revision is
immutable, and `draft → in_review → accepted → rejected` is not. So the head
document for an image carries the review state and which revision is accepted,
the same way prompt labels live in the head rather than in the version. The
accepted revision is what an export reads; a draft is what a labeller is still
holding.

**Pre-annotation is recorded as such and never arrives accepted.** Every shape
carries `origin: human | model | import | ocr` and an optional confidence. A
model-assisted pass writes a `draft` revision a human then corrects, which is
the workflow that makes 300 floor plans affordable. What it may not do is
produce training targets nobody looked at.

**The split key is the family, not the image.** Every image declares a
`group_id` — one building, however many renderings of it exist. An export
assigns splits by hashing `group_id` with the export's salt, so the mirror and
the garage variant land on the same side by construction rather than by
someone remembering. Explicit per-group overrides exist for the test set that
has to contain a specific house.

**Usage rights are a required field, and an export enforces a policy.** Every
image carries `owned | licensed | research_only | unknown`. An export declares
`rights_policy: commercial | research | any` and *excludes*, by name, every
image that fails it rather than refusing the whole export. Mixing CubiCasa5K
into a commercial training set then takes a deliberate `research` policy, and
the manifest says so forever.

**The bytes are content-addressed and optional.** An image is registered with
a URI, and the registry never inlines bytes into an annotation. It will,
however, hold them: `POST /api/v1/annotation-blobs` stores an upload under
`blobs/{sha256}` in the same object store and hands back the digest *it*
computed, never the one the client claimed. That is what makes the panel a
working annotation tool rather than a viewer for images somebody else already
published, and it costs one copy of each image instead of one per export.

**An export is a manifest, not a tarball.** It lists the exact samples, their
splits, their accepted revision ids, the label schema version, the counts per
class and per split, and every exclusion with its reason. COCO is served from
it because COCO is only JSON. Rasterisation stays in Python, where numpy and
PIL already are — the same boundary as ADR_0014, where Flow executes and the
Rust registry versions.

**The public dataset catalogue is a reference table, not a client.** aiwatcher
ships a static, dated table of the floor-plan corpora — what each contains, its
size, its licence and whether that licence permits a commercial model — and
serves it for search. It does not query Hugging Face, Kaggle or Roboflow
Universe, because a mirror's declared licence is routinely wrong and a wrong
licence fetched live reads as authoritative.

## Alternatives considered

**Annotate masks and post-process into geometry.** It is the shortest path to a
first model and it throws away the fields the product needs — wall membership,
swing direction, room connectivity — at annotation time, which is the one point
where a human could have supplied them for free. Rejected.

**Adopt CVAT or Roboflow and import their exports.** Both are better annotation
tools than anything built here, and the intent is still to import from them —
which is what `origin: import` is for. What they cannot do is be the place the
model's training set is versioned, because then the split key, the rights
policy and the join to the runs that use the model live in a system aiwatcher
cannot see. The registry is the boundary; the drawing surface is replaceable.

**Store annotations on the event log like an evaluation report.** ADR_0010's
reasoning runs backwards here. A report describes something that happened and
is fairly evicted with it; an annotation is the input to a model that will
outlive fifty retention windows. Rejected for the same reason prompts are not
on the log.

**Let the export be a downloadable archive of images and masks.** It makes one
training run one download, and it makes the artifact large, opaque, and a
second copy of every image. The manifest is small, diffable between versions,
and points at bytes that already exist. Rejected.

**Split randomly per image and hope.** This is the default in every tutorial
and it is what makes floor-plan papers report numbers nobody reproduces.
Rejected explicitly, and the API has no way to express it.

## Consequences

A revision is capped at 4 MiB of canonical JSON, which is roughly 20 000 points
— far above a floor plan and far below a corpus. Bulk import of a whole public
dataset is therefore per-image and not a single request; that is the same
bounded-request decision as ADR_0014 and will need an asynchronous writer at
the same moment.

Image bytes live once, under their own digest, and every revision and export
points at that digest. A registration whose URI is external is never fetched by
the server, so a broken external URI is an image nobody can label and nothing
detects until somebody opens it. The blob path is capped at 16 MiB per upload,
which is comfortably above a 300 dpi catalogue plan and far below a scan of a
full technical drawing set.

Deriving every raster means every training run pays a preprocessing cost the
first time. That cost is cached locally by the SDK, and it buys the ability to
change stride, class collapsing or mask thickness without touching the labels.

No new configuration. `AIWATCHER_PROMPT_STORE` decides whether this registry
exists, exactly as it decides whether the dataset one does, and the annotation
prefix is `annotations/`. That is the third artifact behind a setting named
after the first, and ADR_0014 already flagged the name as historical — with
three of them it should become a general registry setting, which is a rename
with a deprecation window rather than a decision.

Search over the public catalogue is over a table a human wrote. It will go
stale, which is why every row is dated and links its original. It is a signpost
and never a permission.

**What would make this wrong.** If annotation volume passes roughly 5 000
images, per-image head documents in an object store become a listing cost that
wants an index — the same shape of failure as ADR_0015's 4 MiB scan. If a
second annotation tool becomes the place people actually draw, the registry
should keep the schema, the rights and the split and stop pretending to own the
drawing surface. And if a model ever needs a target that vector shapes cannot
express — a learned distance field, a per-pixel regression — then "the vector
is the source" has to become "the vector is *a* source", with the raster stored
beside it and a stated rule about which one wins.
