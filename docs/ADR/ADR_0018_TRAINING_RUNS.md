# ADR_0018: A training run is a record, not a trace, and it has its own module

- **Status**: accepted
- **Date**: 2026-09-01

## Context

The model ADR_0017 collects labels for has to be trained, and training is the
one part of this stack that would otherwise be watched somewhere else. Weights
& Biases and MLflow both do it well and both do it in a system that knows
nothing about the agent runs that will call the resulting model. That
separation is the problem worth solving: when a floor-plan extraction run
starts returning bad geometry, the question is which model version it is on and
what that version's held-out numbers were, and today that question crosses a
boundary.

The first design put `train.*` events on the event log beside `llm.*` and
`tool.*` — the log is already there, the SDK already publishes to it, and a
training run has a start and an end like anything else. Following it through
produced a sequence of exceptions:

* an **epoch** is not a span. Two hundred equal bars in a waterfall is a
  picture of the fact that epochs take about the same time; the useful view is
  a curve, and the nesting a span tree adds is not true anyway.
* a **step** does not belong on the log at all. A 300-image dataset at batch 4
  for 200 epochs is 15 000 steps; the same loop on a real corpus is millions.
* a **profiler session** is not a trace. `torch.profiler` on a single step
  emits tens of thousands of records.
* a **checkpoint** is not an artifact of the run in the log's sense; it is
  hundreds of megabytes the projector must never hold.

What was left riding the trace machinery was one span with no children — and a
special case in the read model's status fold, because a training run has no
`run.started` and would otherwise spin forever. That special case was the
signal: a design whose last step is an exception in somebody else's fold is a
design in the wrong place.

Three properties of a training run are simply different from an agent run's.
Its **grain** is minutes rather than milliseconds. Its **lifetime** exceeds
retention — six months later the question "which export produced this
checkpoint" still has to have an answer. And its **reader** wants a curve and a
model registry, neither of which the projector models.

## Decision

**Training is its own module, with its own store and its own API.**
`aiwatcher-training` sits beside `aiwatcher-prompts`, `aiwatcher-datasets` and
`aiwatcher-annotations` as the fourth authored artifact over the same
`ObjectStore` port, under an `annotations`-style prefix of its own. Nothing in
it touches the event log, the live hub or the span assembler. `train.*` is not
in the event catalog and `Subject::Train` does not exist.

**A run is a record that grows in place.** It opens with `POST
/api/v1/training-runs`, accumulates through `…/progress`, and closes with
`…/finish`. Three write routes, not seven, because the client buffers and
flushes one batch per epoch: epochs, sampled points, checkpoints and profiler
summaries arrive together, and one place makes a retry idempotent.

**An epoch is a point on a curve and a step is arithmetic.** The SDK aggregates
steps locally and sends one epoch record; a finer series exists
(`TrainingRun::samples`) and is rate-limited at the client and decimated at the
server. Decimation halves by dropping every second point rather than truncating
either end, because what a learning-rate series is read for is its shape, and
`sample_decimations` says the interval is no longer the one the client chose.

**A retried epoch replaces the epoch it already wrote.** A network blip during
a six-hour run must not produce a curve with two points at the same x.
Similarly, re-opening an *open* run returns it, and re-opening a *finished* one
is a 409: a run id is used once, or the second run inherits the first's curve.

**Nothing decides a trainer died.** A run with no end is `Running`, and what
the record reports instead is `last_heard_from`. This is the projector's rule
for agent runs, restated: a process killed by an OOM and one thinking for
twenty minutes are indistinguishable from here, and the panel draws the stall
line rather than the registry claiming a fact it does not have.

**A model version is the other half, and it is why this lives here.** A
version names the run that produced it and the export that run was trained on;
an agent span names a model. That is the join — from bad geometry in
production, to the checkpoint, to the labelled images — and it exists only
because both ends are in one system. Provenance is read *from the run*, never
taken from the registration, so a version cannot claim a lineage its run does
not have.

**Two rules gate a promotion, and both are ADR_0011's verdict rule for
weights.** A label is refused when the version has no held-out measurement —
the validation score is the number early stopping maximised, so promoting on it
promotes the selection — and refused when the dataset is a mutable name rather
than an immutable export reference. A version that fails either is *recorded*,
with the reason returned on the registration rather than three days later.
Moving a label needs `admin`, like a rerun and a launch: it is the write here
that changes what a service loads next.

**Weights & Biases stays available and is not the system of record.** The SDK's
bridge is one-directional and duck-typed: hand the training client a `wandb`
run as `mirror=` and the same points go to both. aiwatcher does not
reimplement sweeps; W&B does not learn about the agent runs.

## Alternatives considered

**Put `train.*` on the event log.** The first design, described above. It buys
the log's delivery guarantees for data that has one writer and no ordering
problem, and it costs an exception in every fold the events pass through.
Rejected after implementing it.

**Make each epoch a span.** The obvious mapping, and it produces a waterfall of
two hundred equal bars. Rejected.

**Use MLflow or W&B as the system of record and link out.** Less code, and it
puts the model's provenance in a system that cannot answer "which agent runs
used this checkpoint". The link exists in the other direction instead.
Rejected, without prejudice to using either alongside.

**Ingest the profiler's Chrome trace and render it.** The read model would not
survive the spans, and the result would be a worse profiler UI than the one
that ships with PyTorch. Rejected; the summary is what earns its place.

**Stream the run over SSE, like the live event channel.** An epoch is minutes,
so a five-second poll costs one request and answers the same question. The live
channel exists because a token arrives every 30 ms. Rejected until something in
training moves at that speed.

## Consequences

Training visibility is at epoch grain. A loop that diverges inside an epoch
shows up as one bad point rather than the moment it happened, and finding that
moment means turning the sampled interval down for a run. That is the trade:
the default is the grain that stays affordable on the largest corpus this is
meant to reach.

A run's whole record is one object, capped at 8 MiB — comfortably above a
10 000-epoch run with six metrics. Listing reads a small per-run summary object
instead, so a hundred runs is a hundred small reads rather than a hundred
curves. Past roughly a thousand runs that listing wants an index, which is the
same scaling limit ADR_0017 has and will want the same answer.

Because nothing is on the log, a training run is invisible to Explore, to the
dimension folds and to VictoriaTraces. That is intentional and it has a cost:
"what happened between 14:00 and 15:00" now has two places to look. The join
between them is `model`, and it is one field.

`AIWATCHER_PROMPT_STORE` now gates four registries. ADR_0014 already called the
name historical; with four it should become a general registry setting, which
is a rename with a deprecation window rather than a decision.

**What would make this wrong.** If epoch grain routinely fails to explain a
divergence — if people re-run with sampling turned up more often than not — the
default is wrong, and step-grain buffering with a retention of its own is the
answer rather than a lower default interval. If training moves onto the same
orchestrator as ADR_0016's pipelines, `workflow_run_id` should become the
primary key of the join rather than an optional field. And if a second thing
ever needs a durable, mutable, growing record on this stack, the accumulate-in-
place write here should become a shared primitive instead of being copied.
