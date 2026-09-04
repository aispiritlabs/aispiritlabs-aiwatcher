# ADR_0024: A curation is a chain of blocks, each belonging to the engine that can run it

- **Status**: accepted; the execution half superseded by ADR_0025
- **Date**: 2026-09-04

The block vocabulary, the chain validation and the content-addressed revision
stand. "The chain is driven from the browser" no longer holds for a managed
run: ADR_0025 gives the server the sequencing, the retries and the publication,
and leaves the browser the ad-hoc path it names below.

## Context

ADR_0014 settled how a curation is executed and kept: Flow PHP transforms, the
Rust registry versions the script and the rows it produced. That is the right
shape while the whole curation *is* one query.

The demo that broke it is personal-data detection. The corpus is on Hugging
Face, so the rows come from `hub_rows` and Flow reads them perfectly well. Then
something has to look at the text — find what is in it, mask it, count what it
found — and Flow is deliberately unable to do that. The query language is a
whitelist over Flow's DSL, parsed and never executed (ADR_0008), and widening
it until it can express a detector would be building an interpreter behind a
security boundary that exists precisely to stop that.

So the work has to happen in Python. And the person writing it does not want a
batch job: they want to see the rows, move a threshold, look at what changed,
and only then run it over everything. That is a notebook, and it is the one
part of this system that had no home.

Three further facts shaped the answer:

* **The three engines are already separate.** Flow PHP is an optional service
  the panel talks to directly and the aiwatcher binary does not know exists. The
  registry is Rust. A notebook runtime is a third thing, optional in the same
  way.
* **The steps are not interchangeable.** A Flow step reads its rows by naming a
  dataset in the query service's catalog. There is no way to hand it a list of
  rows a notebook produced.
* **A dataset version already records how it was made**, and the Flow script
  alone would stop being that record the moment a notebook ran after it.

## Decision

A curation may also be a **chain of blocks**, saved in the dataset registry
beside the recipes as a content-addressed revision. Four kinds:

| Block | Engine | What it holds |
|-------|--------|---------------|
| `source` | Flow PHP | a dataset in the query service's catalog, and the arguments that catalog declares |
| `transform` | Flow PHP | steps appended to the `read()` |
| `notebook` | `services/ml_pipeline` | the name of a marimo notebook, its settings, and the revision it was saved against |
| `view` | Rust registry | the dataset an immutable version is published to |

**The shape is a chain**, validated in `aiwatcher-datasets`: one head,
one next per block, everything reached, a source first, nothing after the view
— and every `transform` before the first `notebook`, because a Flow step cannot
be handed rows. A refusal is a 422 carrying *every* problem, and the canvas
renders those lines and implements no rules of its own, exactly as the
annotation canvas does not re-implement the shape validator.

**The chain is driven from the browser.** Every source and transform block
compiles to one Flow query — Flow executes one pipeline, so three boxes light up
together and the panel says so. Its rows go to the first notebook block, that
notebook's rows to the next, and the view publishes what is left.

**A notebook is one file doing two jobs.** `ml_pipeline.step` runs it through
marimo's own `App.run(defs={"rows": …, "params": …})`, which uses those values
instead of executing the cell that would define them and hands back the
definitions — `output` among them. The same notebook is served as a live app in
an iframe inside the block's editor, where nothing is injected, so that cell
runs and reads the rows the last run staged for it. One file, no harness, and the
widgets are being moved against the rows the block will actually run on.

**The notebook source lives in the notebook directory, not in the block.** That
file is what marimo serves, what a run imports and what a test reads. The block
names it and pins the `sha256` of the source it was saved against, and the panel
says when the two have drifted.

**A published version records the chain.** `produced_by` is
`<pipeline name>@<revision>`, and publishing saves the pipeline first so the
reference is to something. It is provenance, not identity: a block dragged
across the canvas is a new pipeline revision and the same rows, and a dataset
version per canvas tidy-up would be a version history about layout.

## Alternatives considered

**Execute the chain in Rust.** The API would hold a Flow client and a notebook
client, and one POST would run everything. It loses the property ADR_0008 was
built for — that the aiwatcher binary does not know the optional services exist
— and it makes the server a workflow engine, which ADR_0016 says to delegate.
The browser is also the only place that can *show* the chain running block by
block, which is most of the value.

**Run the notebook in the API process.** Refused for the reason
`Runtime::executes_packaged_code` exists: that process holds the object store's
credentials and every registry behind them, and a notebook is arbitrary code
somebody is halfway through writing.

**A general DAG rather than a chain.** A block with two parents has to be told
how to combine them and a block with two children runs twice; neither has an
answer that is right more often than it is wrong. A chain refuses the shapes it
cannot run rather than guessing at them, and joins are what the Flow block is
for.

**A harness in the notebook** — `block.get_rows()` at the top and
`block.write_output(rows)` at the bottom, over a file the runner passes in
environment variables. This was built first and then removed: `App.run(defs=)`
is marimo's own API for exactly this, and it deletes the output half of the
contract. The cost is that marimo replaces a *whole cell*, so the cell defining
`rows` and `params` may define nothing else — a constraint the step catches and
explains, because marimo's own message names the missing definitions without
saying what to do about them.

**The block holds the notebook's code.** Then a saved pipeline is
self-contained. It also means the file marimo serves is a materialisation of
something else, two things can disagree about what the notebook is, and the
notebook stops being testable on its own. Pinning the revision keeps the
provenance without the second copy.

**Jupyter and papermill.** A marimo notebook is a Python file: it is diffable,
importable, lintable and runnable as a script, it has a real dependency graph
rather than execution order, and it serves itself as an interactive app. The
interactive half is the whole reason the notebook block exists, and papermill
does not have one.

## Consequences

The panel now depends on two optional services for one screen, and says which
one is missing rather than failing — a chain of a source and a transform runs
with the notebook runtime switched off.

**A chain does not survive a closed tab.** There is no scheduled or unattended
run: the browser is the thing sequencing the blocks. Repeatable, unattended
execution is what the pipeline engine is for (ADR_0016), and a curation that
needs it should be registered there.

**The notebook runtime runs unsandboxed code on localhost.** It hosts notebooks
in its own process and runs them in child processes with a timeout; it has no
authentication and binds to `127.0.0.1`. It is a development surface. Putting it
in a cluster needs a sandbox and an identity, and that is a different decision.

Row counts are bounded twice — the Flow preview cap and the notebook runtime's
own ceiling, which is the dataset registry's — so a block that hands on more
than can be saved says so where it happens rather than at the end.

**What would make this wrong.** Somebody needing a chain to run unattended, on a
schedule, or over a corpus too large for one browser session. At that point the
chain stops being an editing surface and becomes a job: the machine in
`aiwatcher-jobs` (ADR_0022) and the engine in ADR_0016 already exist, and the
block definitions saved here are exactly what such a job would read. The other
signal is a second notebook runtime — a second language, or a hosted one — which
would mean `notebook` needs the `Runtime`-style declaration ADR_0023 gives a
model package, instead of being one service's notion of a notebook.
