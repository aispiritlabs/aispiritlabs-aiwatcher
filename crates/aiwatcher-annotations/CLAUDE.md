# Annotations, hubs and anything fetched

The rules for `aiwatcher-annotations` and the rasteriser in
`aiwatcher_sdk.integrations.vision`. Read
[ADR_0017](../../docs/ADR/ADR_0017_IMAGE_ANNOTATION.md),
[ADR_0020](../../docs/ADR/ADR_0020_GENERIC_VISION_ANNOTATION.md),
[ADR_0019](../../docs/ADR/ADR_0019_DATASET_HUB_DISCOVERY.md) and
[ADR_0022](../../docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md). Import jobs obey the
root file's one ordering rule.

- **Never ship a label vocabulary.** The project's schema *is* the domain — its
  classes, their geometry, which are `ignore`, which `layer` each paints into —
  and a shipped preset decides what the first hour of labelling produces
  (ADR_0020).
- **Never let a class erase what it overlays.** That is what `LabelClass::layer`
  is for: classes on one layer share an integer grid and paint in declaration
  order; classes on different layers never contend, and a model reads one head
  per layer. An opening in a wall, a defect on a component, a marking on a road:
  one grid could only draw the overlay by deleting what it sits in.
- **Never let the rasteriser know a class name.**
  `aiwatcher_sdk.integrations.vision` is driven by the schema it is handed:
  geometry decides fill or stroke, `ignore` decides exclusion, declaration order
  decides a contested pixel, `layer` decides the grid. It also checks the schema
  against the export's pinned `schema_version` — rasterising against a reordered
  vocabulary permutes every label, every metric stays finite, and nothing says
  so.
- **Never make a raster the source of an annotation.** Mask, heatmap and COCO
  document are derived from the vector shapes and regenerated on demand; an
  edited mask beside its vector is two sources of truth with nothing able to say
  which is right (ADR_0017). The rasteriser's one ordering decision is rooms
  first, walls last — reversed, every wall between two rooms has a hole where
  they meet.
- **Never split an annotation corpus by image.** The key is `group_id`, the
  subject: one building as plan, mirror, garage variant and redraw is four
  images and one observation, and splitting them apart makes the test score a
  measurement of memorisation. `export::split_for` hashes the family and the
  salt and *only* those, so adding an image never re-deals an existing family.
  There is no API that assigns a split per image.
- **Never derive an import's `group_id` from the file name.** A per-file key
  silently turns the family split back into a per-image one. The route cannot
  prevent it, so it reports it: a batch whose every row is its own family comes
  back with a warning on a response that succeeded.
- **Never let an image's usage rights be optional.** `UsageRights` has no
  default and `RightsPolicy` defaults to `commercial`, so the strict answer is
  the free one. An export *excludes by name* rather than refusing, so the
  manifest records what it left out and why, for ever.
- **Never let a model's proposal become a training target on its own.** Every
  shape carries `origin: human | model | import | ocr`, an export defaults to
  `require_human_review`, and a revision that is entirely machine output is
  excluded with the reason.
- **Never take a content address from the client.** `put_blob` hashes the bytes
  it received and ignores what the caller claimed; a caller-supplied address
  lets two different images occupy one key, which is a training set whose labels
  belong to a different picture. `AnnotationRegistry.fetch_image` verifies it
  again on the way out.
- **Never rename an annotation class in place.** The schema is versioned by the
  content of its class list and a revision names the version it was drawn
  against, so changing the classes excludes every earlier revision from the next
  export *by name* — the loud failure, and the correct one.

- **Never fetch a dataset licence from a mirror.** `sources` is a dated table a
  human wrote and an instance loaded (`AIWATCHER_DATASET_SOURCES`), every row
  links its original, and this build ships **no rows** — an empty table is a
  working state, and every hub result stays `unclear`. Hugging Face, Kaggle and
  Roboflow restate licences wrongly often enough that a live answer would be
  worse than none, because it would arrive looking authoritative.
- **Never let a hub's licence field become a usage verdict.** `hubs::reconcile`
  starts every row at `SourceUsage::Unclear` and only a match against
  `sources::catalog` moves it; the mirror's words survive verbatim in
  `claimed_license`, named for what they are, and the two are never merged into
  one badge. The first live search returned `Voxel51/FloorPlanCAD` declaring
  `cc-by-sa-4.0` for a corpus whose authors say the drawings are not theirs to
  license.
- **Never match a corpus name by substring.** `RPLAN` is a substring of
  `floorplans`, and a plain `contains` handed
  `wall-constrained-floorplans-manual-only` RPLAN's verdict — a permission claim
  invented by a coincidence of spelling. Whole-token match, cross-separator only
  from eight characters up. A miss is safe; a wrong match is a licence claim.
- **Never let an import assert rights the curated table contradicts.**
  `import::check_rights` refuses a commercial claim on a batch that matched a
  research-only corpus, and only that. Everything else is the caller's
  assertion, recorded as `UsageRights::Unknown`, which a commercial export
  excludes by name for ever — refusing outright would teach people to claim a
  licence to get past the dialog.
- **Never fetch a byte outside `integrations::fetch`.** It is the only place
  that downloads content an outside party chose, and it carries seven gates:
  https with the host *parsed* rather than matched, an allowlist, a
  public-address check on every resolved address, no redirects, a streaming byte
  ceiling, a header-only pixel ceiling and a verified content address. Both
  import routes go through the same `ImageSource` port. The gate easiest to
  under-rate is the redirect: an allowlisted host answering `302 →
  http://169.254.169.254/` walks past every check that ran against the address
  the caller named.
- **Never take an import's version from the batch id.** It is `sha256(batch
  content digest ‖ dry-run flag ‖ every result shard digest)`, so two people who
  staged the same rows on the same terms reach the same reference.
- **Never let an interrupted import publish a manifest.** A cancelled or failed
  job has no version and no index entry; the images it registered stay
  registered, because an image id is the content address of its bytes and
  re-running writes the same ones.
