# Examples

- [Titanic from scratch](titanic/README.md): raw data, thematic curation blocks,
  portable flow with PHP and Python code, native PHP FlowAI encoders, and a validation baseline.
- [Titanic krok po kroku — 12 zrzutów ekranu](titanic/WALKTHROUGH.md).
- [Własny notebook z komórkami PHP/Python i wynikami po każdym kroku](titanic/NOTEBOOK.md).

Examples are executable integration exercises. The portable `.flow.json` file
can be imported in **Data curation → Pipeline → Import flow** and exported after
editing with **Export flow with code**.

## Service seed and public solutions

[`seed.json`](seed.json) is imported when the API service is initialized, before
it accepts requests. These are ordinary saved pipelines and public block
solutions: the seed calls the same registry validation and save operations as
the HTTP API. The pipeline panel contains no example or block-template catalogue.

- Open a seeded flow from **Saved pipelines**, in either canvas or notebook view.
- Open **Public solutions library** to search titles, descriptions, tags and block
  kinds. Search and pagination run on `GET /api/v1/curation-library`.
- Inspect code and settings, then **Add copy**. Python solutions copy the exact
  pinned source into a new notebook; editing the copy leaves the shared source alone.
- Select a block and use **Share selected block → Publish solution** to create
  an entry through `POST /api/v1/curation-library`. Editor permission is required.
  “Public” means visible to readers of this installation, not an Internet marketplace.

Restarting skips existing pipeline names and solution IDs, preserving user edits.
Import preflight rejects an invalid document before writing any entry; an interrupted
store write can be resumed by restarting. Run initial provisioning before allowing
concurrent authors: the object-store abstraction has no conditional create operation.

`AIWATCHER_SEED_FILE` selects a seed file; `none` disables it. Local CLI startup
defaults to `examples/seed.json` relative to the working directory. The Docker image
ships the file at `/usr/local/share/aiwatcher/seed.json` and configures that path.
Programmatic `Config::default()` has no seed; set `seed_file` when constructing a
service in an embedding or test. A configured missing or invalid file fails startup.
With the dataset registry disabled, there is no destination and nothing is imported.

The seed pins notebook files shipped with `services/ml_pipeline/notebooks`.
That service archives their revisions on startup. Run matching versions of both
services; the API does not need the Python service online to import definitions.
For transferring code to another installation, use the portable `.flow.json` bundles.

Author flow definitions in `*/pipeline*.json` and library entries in [`blocks.json`](blocks.json).
Rebuild after changing definitions or notebook code:

```sh
python3 examples/build_seed.py
python3 examples/titanic/build_bundle.py
```

Both scripts support `--check`, exercised in CI. Rebuilding changes the seed for
new names/IDs and fresh installations; it does not migrate existing user content.
