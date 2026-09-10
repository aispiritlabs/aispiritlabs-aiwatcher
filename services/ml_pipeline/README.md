# `ml_pipeline` — the notebook runtime

The service behind the **marimo** block in the panel's curation pipeline
(`Data Curation → Pipeline`). It does two things with one file:

* **serves** a notebook as a live app, for the iframe the panel opens when you
  click the block — real `mo.ui` widgets, real Python, reactive re-execution;
* **runs** the same notebook as a step, over the rows the previous block
  produced, and hands what it defines to the next one.

Both see the same rows. That is the whole design: a detector built against the
rows it will run on is a detector whose first real run is not its first test.

```
Hugging Face ──► Flow PHP ──► marimo ──► View
                   rows        rows       a dataset version
```

## Running it

```bash
just ml-pipeline-install   # uv sync --all-groups
just ml-pipeline-serve     # http://127.0.0.1:8082
just ml-pipeline-check     # ruff format --check, ruff check, mypy, pytest
just ml-pipeline-edit pii_detection   # marimo's own editor, for a big change
```

The panel proxies `/ml-pipeline` to it in development
(`apps/panel/vite.config.ts`). Without it running, the Pipeline view says so and
every other block still works — the same posture as `services/query/flow` (ADR_0008).

Python 3.14, `uv`-managed. `orjson` carries every staged row and every log line;
`structlog` writes the console when a person is watching and JSON when nothing
is.

## Writing a block

A block is a marimo notebook with **two conventional names** and no harness:

```python
@app.cell
def _(Block):
    _block = Block.for_notebook(__file__)  # only when nothing is injected
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(rows):
    output = [row for row in rows if row["keep"]]
    return (output,)
```

When the chain runs, `ml_pipeline.step` calls marimo's own
[`App.run(defs={"rows": …, "params": …})`][run]: marimo uses those values
*instead of executing the cell that would define them*, re-runs everything
downstream, and hands back the definitions — `output` among them. Open the same
notebook in the panel and nothing is injected, so that first cell runs and reads
the rows the last run staged for it.

[run]: https://docs.marimo.io/api/app/#marimo.App.run

Two rules follow from marimo replacing a **whole cell** rather than one name:

1. the cell defining `rows` and `params` defines *nothing else* — put the
   imports it needs in a cell of their own, and underscore what only it uses;
2. seed every `mo.ui` element's default from `params`, and one file is both the
   step and the place you build the step.

Get the first one wrong and the run says so, in those words.

## The two blocks that ship

`pii_detection` scans a text column for the shapes of personal data, counts what
it finds and hands on the masked text. It is the one that answers "why a
notebook at all": nothing in a query language reads prose, and the alternative
to a regular expression here is a model.

`titanic_features` fills a missing age from its status group's median and sizes
a family. It used to be the second half of that argument, and is not any more:
the query surface stopped being a hand-written list, so a window function and
`->plus(...)` do the same work in one query (`titanic/features`, in the Recipe
view). It stays as a worked example of the block contract — and as the reminder
that "the query cannot do this" is worth checking before a chain grows a second
engine.

Both are imported by the service seed and loadable from `Data Curation → Pipeline → Saved pipelines`. Public block solutions are searchable in **Public solutions library**; its catalogue is served by the API.

## The head moves; a revision does not

A notebook is one editable file, and a saved pipeline block pins the `sha256` of
the source it was saved against. Every save also writes that exact source into
`.revisions/<name>/<sha256>.py`, and a managed run **names its pin** — so
editing a notebook changes what runs next and changes nothing about what already
ran. `POST /run` with no `code_revision` gets the head, which is the editor's
own path and the unsaved code somebody is looking at.

That directory is the only durable thing this service holds.
`AIWATCHER_ML_PIPELINE_REVISIONS` moves it; deleting it deletes the provenance
of every execution that pinned one of those sources, which no later run can
reconstruct. At start-up the service writes whatever heads it finds into the
history, so an upgrade keeps what is there — and only what is there.

## What it is not

There is no authentication and no sandbox: it runs notebook code in this process
(the live app) and in child processes (a run). It binds to localhost. Never
expose it on a public interface.

## Layout

| File | Holds |
|------|-------|
| `block.py` | what a notebook imports — the fallback for `rows` and `params` |
| `step.py` | one run: import the notebook, `App.run(defs=…)`, read `output` |
| `runner.py` | the process that happens in — bounded, timed, isolated |
| `staging.py` | the rows a block reads, on disk, under a name both sides compute |
| `notebooks.py` | the notebook files and their history: list, read, pin, and the two checks before a write |
| `service.py` | the seven control routes, with marimo's app host under them |
| `log.py` | console for a person, JSON for a collector |
| `notebooks/` | the notebooks themselves: `pii_detection.py` and `titanic_features.py` |
| `.revisions/` | one exact source per digest, kept forever — **the one directory here that must survive** |
| `.data/` | staged rows and outputs; scratch, safe to delete |

## FlowAI and portable examples

The runtime also ships the `flowai` Python package: row-oriented OneHotEncoder
and LabelEncoder adapters with fit/transform separation and JSON fitted state.
The panel exposes thematic templates such as Missing values and Visualization.
See [Titanic from scratch](../../examples/titanic/README.md) for the full chain,
local CSV runner, dependency requirements, and `.flow.json` import/export.

`uv sync --locked` installs the numeric and plotting dependencies used by these
blocks. FlowAI is included in this service's wheel alongside `ml_pipeline`.
