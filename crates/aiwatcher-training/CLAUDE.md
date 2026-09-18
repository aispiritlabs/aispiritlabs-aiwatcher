# Training runs, the model registry and serving

The rules for `aiwatcher-training` and for `aiwatcher_sdk.serving`, the hardened
profile that reads a promotion back out and serves what it names. Read
[ADR_0018](../../docs/ADR/ADR_0018_TRAINING_RUNS.md) and
[ADR_0023](../../docs/ADR/ADR_0023_MODEL_PACKAGE.md).

- **Never put a training run on the event log.** An epoch is not a span, a step
  does not belong on the log at all, and a profiler session is not a trace —
  which left one span with no children and an exception in the read model's
  status fold. Training has its own module, store and routes; `train.*` is not
  in the event catalog, and adding it back means re-reading ADR_0018.
- **Never send a training step anywhere.** The SDK counts and averages steps
  locally and emits one epoch record: 15 000 steps for a small run, millions on
  a real corpus. The finer series that does exist is rate-limited at the client
  and decimated at the server.
- **Never append a retried epoch.** `progress` replaces an epoch index it
  already holds, or a network blip produces a curve with two points at one x,
  which reads as training that went backwards.
- **Never reuse a training run id.** Re-opening an *open* run returns it, so a
  retried start loses nothing; re-opening a *finished* one is a 409, because the
  second run would inherit the first's curve.
- **Never store a profiler trace or a checkpoint's weights.** `ProfileRecord` is
  the top operators and a URI; `CheckpointRecord` is a URI and what selected it.
- **Never let a model version claim provenance its run does not have.**
  `register_model` reads the dataset, framework and code *from the run it names*
  and ignores what the request said.
- **Never promote a model on a validation score.** It is the number early
  stopping maximised, so promoting on it promotes the selection.
  `check_promotable` also refuses a mutable dataset name, and the two refusals
  are deliberately different sentences: one invites a held-out evaluation, the
  other an export. A version that fails either is still recorded, with the
  reason.
- **Never fit a model against an empty split.** The middle one is the trap: with
  no validation images every epoch scores zero, epoch 0 wins by default, the
  checkpoint is selected arbitrarily, and the run still reports a validation
  number the registry accepts as a held-out measurement. A metric over nothing
  is worse than a missing one.
- **Never let the training registry decide a trainer died.** A run with no end
  is `Running` and `last_heard_from` is what it reports instead.

- **Never register a model package whose artifacts carry no digest** (ADR_0023).
  An address is not an identity: `s3://models/latest.pt` is different bytes
  tomorrow. A *half* package — a declared runtime with an undigested artifact —
  is refused rather than accepted, because it reads as provenance and is not.
- **Never sniff a model's runtime.** A loader chosen by looking at the file is a
  loader chosen by whoever wrote the file. `Runtime` is declared, `Unspecified`
  is refused, and `Runtime::executes_packaged_code` is what a host answers
  *before* it opens anything — a package that runs its own code is never loaded
  in the API process, which holds the object store's credentials.
- **Never trust a declared shape an artifact could be asked about.** An ONNX
  graph carries its own names, element types and shapes, so
  `serving.runtimes.onnx` cross-checks the package's `inputs` and `outputs` and
  refuses a disagreement naming both sides. A wrong shape is not a typo: it
  means the package describes a *different model*, so its held-out score, its
  lineage and its label order belong to something else. The same settles
  `classes` — `n` classes over a width-`n` head, two over a binary one — because
  nothing at load can tell a mislabelled head from a mistrained one.
- **Never let a serving profile discover at the first request what it could
  refuse at load.** `instances` is one rank-2 tensor with a free batch axis, so
  two inputs, an image tensor, a string input or a pinned batch dimension are
  refused *by name*, each naming the profile it would need; `runtime_version` is
  compared before the bytes are read. "It loaded and every request 500s" is the
  outcome these gates turn into a deployment decision, and by then the previous
  version is gone.
- **Never make `preprocessing` executable.** It is what the trainer did in its
  own words, reported on `/v1/model` and applied by nothing; the caller holds
  the raw input, so the caller must already have done it. `entry_point` is the
  opposite and *is* acted on — a name in this package, and a value naming
  neither an artifact nor a URI's last segment is a refusal rather than a guess.
- **Never put an inference's inputs or outputs on the event log.** The profile
  reports `run.started → llm.started → llm.completed` carrying model, version,
  label, traffic, rows, latency and outcome — a primary or shadow invocation is
  a model call and joins the same traces and model dimension. What it never
  carries is what was said; a runtime that wants to retain that writes turns to
  the conversation archive.
- **Never let a broken new label remove a ready old version.** The rollout is
  two-phase: download, verify and warm the candidate while the current version
  keeps serving, and swap only if all three succeed. The previous version stays
  loaded, so a rollback needs no fetch — and the version being left is pinned
  out, because a rollback the next poll undoes is not one.
