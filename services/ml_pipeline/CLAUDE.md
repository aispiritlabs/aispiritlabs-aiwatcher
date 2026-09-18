# The notebook runtime

The rules for `services/ml_pipeline`, the Python 3.14 runtime behind a
pipeline's marimo blocks. Read
[ADR_0024](../../docs/ADR/ADR_0024_CURATION_BLOCKS.md). It runs notebook code
with no sandbox and has no authentication, which is why the first rule below is
where it binds.

- **Never run a notebook a plan did not pin.** A managed step names its
  `code_revision` and the runtime resolves *that* source from
  `.revisions/<name>/ <sha256>.py`, never falling back to the head — running
  something else under a pinned run's name produces rows that look exactly like
  a successful run. Checked twice: a GET before anything executes, recomputing
  the digest from the stored bytes, and a comparison after against what the
  subprocess imported. `UserCode`, so neither is retried; a cache *hit*
  re-checks nothing, because the key holds the pinned revision.
- **Never make an edit strand the runs that came before it.** Reading the
  *head's* digest and refusing a run whose pin no longer matched protected
  provenance by making every earlier execution unrepeatable. The history
  replaces it, under ADR_0011's two orderings: the revision before the head, and
  `keep_current` at start-up keeping what a directory holds *now*. `.revisions`
  is that service's one durable thing; `.data` beside it is scratch.
- **Never let a staged file be keyed by the notebook alone.** Two pipelines
  using one notebook would overwrite each other's rows. A run stages under its
  context and *then* points `latest` at it — that order, because the live app
  knows only a notebook's name — and the context is hashed into a directory name
  rather than sanitised.
- **Never let a notebook's injected cell define anything else.**
  `App.run(defs=)` replaces a whole cell, so the cell binding `rows` and
  `params` binds nothing downstream needs: imports go in a cell of their own and
  what only that cell uses is underscored. `ml_pipeline.step` catches marimo's
  own message and answers it.
- **Never run a notebook on the event loop.** `run_notebook` is a blocking
  `subprocess.run`; from an `async` handler it held every other request for the
  length of the run (657 ms of a 704 ms run). `GET
  /ml-pipeline/executions/{key}` has to be answerable *while* a notebook runs,
  and marimo's live app is served by the same process.
- **Never run a notebook to fill its editor.** `EditorHost::open` stages and
  stops — executing would run somebody's code because they clicked "open", and
  overwrite the output of the run being looked at. What opens is the notebook's
  **head**, with the session naming the revision that ran and the code read
  beside it by digest.
- **Never issue a token to a service that cannot check one.** The notebook
  runtime has no authentication, so a signed token presented to it is ceremony
  rather than a boundary. The gate is the route that mints the session —
  `Editor`, because staging replaces what everybody looking at that notebook's
  live app is shown — and aiwatcher reads the rows from its own object store
  rather than telling the runtime where they are. A boundary drawn where nothing
  enforces it reads as protection.
- **Never expose the notebook runtime.** It runs notebook code with no sandbox
  and has no authentication; it binds to `127.0.0.1` and is a development
  surface.
- **Never let a runtime be asked only about what it stored.** A receipt is
  durable and says what an attempt produced; only the runtime knows whether it
  is *still* executing a key, which after a timeout is the question. The
  notebook runtime is asked **best effort**, because failing to reach a
  fifteen-minute memory must not hide the durable answer.
- **Never report a number for a disk this process cannot see.** A notebook's
  staged rows live in `services/ml_pipeline`'s own scratch directory, so the
  staging figure is `GET /ml-pipeline/staging` rather than a zero beside the
  artifact totals. The artifact half is this binary's, hourly, and a count
  rather than an opinion.
- **Never hold a notebook's source in the block.** That file is what marimo
  serves, what `ml_pipeline.step` imports and what a test reads; the block names
  it and pins the `sha256` it was saved against, and the panel says when the two
  have drifted. A copy in the registry would be a second source of truth for a
  file that has to stay runnable on its own — and the *history* is not that
  copy, because a revision is named by the digest of its own bytes. The head
  answers "what runs next"; a revision answers "what ran".
