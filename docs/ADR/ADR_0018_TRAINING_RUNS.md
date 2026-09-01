# ADR_0018: A training run rides the event log; an epoch is a point, a step is a count, and the profiler is not a trace

- **Status**: accepted
- **Date**: 2026-09-01

## Context

The model ADR_0017 collects labels for has to be trained, and training is the
one part of this stack that would otherwise be watched somewhere else. Weights
& Biases and MLflow both do it well and both do it in a system that knows
nothing about the agent runs that will call the resulting model. That
separation is the problem worth solving here: when a floor-plan extraction run
starts returning bad geometry, the question is which model version it is on and
what that version's held-out numbers were, and that question crosses the
boundary.

Training also has a shape nothing else on this log has. A run is minutes to
hours. A step is milliseconds and there are hundreds of thousands of them. An
epoch is the grain a human actually reads. And a profiler session produces more
spans in sixty seconds than this projector holds for a week.

## Decision

**A training run is a run.** `train.started | train.completed | train.failed`
are `Subject::Train` with the ordinary phases, so a training run appears in
Explore, in the runs list, and in the metrics fold with no new machinery, and
its `run_id` is what an agent run's `model` attribute can be traced back to.

**An epoch is a point, not a span.** `train.epoch` carries the epoch number,
its duration and its metrics as one event on the training run's span. Two
hundred identical bars in a waterfall say nothing that the loss curve does not
say better, and pairing start and end events per epoch doubles the log to draw
them. The one thing a span would add — nesting — is not true anyway: an epoch
does not contain the validation pass in any sense a trace viewer draws
correctly.

**A step is counted, never stored.** This is exactly ADR_0003's rule for
`llm.chunk`, for exactly the same reason. The SDK aggregates step metrics
locally and emits at epoch grain by default; `train.metric` exists for a
deliberately sampled series (a learning-rate schedule, a gradient norm) and the
SDK enforces a minimum interval rather than trusting a training loop not to
call it every step.

**A checkpoint is a pointer.** `train.checkpoint` carries a URI, a step, an
epoch and the metric that selected it. Weights never enter the log, the same
rule `artifact.produced` already follows and for a sharper reason: a
checkpoint is measured in hundreds of megabytes and the projector holds every
event it accepts in memory.

**The profiler is an attachment with a summary, never a span tree.**
`train.profile` carries the top operators by self time, the memory peak, the
device, and a URI for the full Chrome trace. `torch.profiler` on a single step
emits tens of thousands of records; folding them into `SpanAssembler` would
evict every real run in the read model to draw a flame graph that a profiler UI
draws better. What the event keeps is the part somebody reads in a review: what
dominated, and by how much.

**Training metrics reach VictoriaMetrics through the existing exporter, not a
second client.** A scalar time series against wall-clock is what the metrics
backend is for, and the projector already exports there. Nothing in the
training path opens its own connection to anything.

**The join to the data is the export id.** `train.started.data.dataset` carries
`project@export-sha256` from ADR_0017 and `data.schema_version` carries the
label schema. A training run that names a mutable project name is recorded and
is not reproducible, and the panel says so rather than implying otherwise —
the same distinction ADR_0015 draws between a collection name and
`name@version`.

**Weights & Biases stays available and is not the system of record here.** The
SDK's bridge is one-directional and optional: if a `wandb` run object is
handed to the training client, the same metrics are mirrored to it. aiwatcher
does not reimplement sweeps, and W&B does not learn about the agent runs.

## Alternatives considered

**Make each epoch a span, nested under the run.** It is the obvious mapping and
it produces a waterfall of two hundred equal bars, which is a picture of the
fact that epochs take about the same time. The curve is the view; a span tree
is the wrong instrument. Rejected.

**Emit every step.** A 300-image dataset at batch 4 for 200 epochs is 15 000
steps, which is survivable; the same loop on a real corpus is millions. A rule
that only works at the small size is a rule that fails during the run that
matters. Rejected, and the SDK makes the sampled path the easy one.

**Use MLflow or W&B as the system of record and link out.** It is less code and
it puts the model's provenance in a system that cannot answer "which agent runs
used this checkpoint". The link exists in the other direction instead.
Rejected, without prejudice to using either alongside.

**Ingest the profiler's Chrome trace and render it.** VictoriaTraces would take
the spans and the read model would not survive them, and the result would be a
worse profiler UI than the one that ships with PyTorch. Rejected; the summary
is what earns its place on the log.

**Store the checkpoint in the prompt/annotation object store.** It is the same
store and it is the wrong lifetime and size class. A registry that holds a
version's text in kilobytes and a checkpoint in gigabytes has one eviction
policy for two problems. Rejected.

## Consequences

Training visibility is at epoch grain. A loop that diverges inside an epoch
shows up as one bad point rather than as the moment it happened, and finding
that moment means turning the sampled `train.metric` interval down for a run.
That is a deliberate trade: the default is the grain that stays affordable on
the largest corpus this is meant to reach.

Because `train.*` rides the ordinary run machinery, a training run is bounded
by retention like everything else on the log. What survives it is the export
manifest, the checkpoint URI and the model registry entry — none of which are
folds. A deployment that wants a permanent training history has to keep the
checkpoint records, not the events.

Nothing here polls the trainer. A run that is killed by an OOM stays `Running`
until `orphan_timeout`, exactly as the guardrail about the projector never
deciding a run has died requires, and `last_event_at` is what tells a reader
the GPU stopped talking.

**What would make this wrong.** If epoch grain routinely fails to explain a
divergence — if people find themselves re-running with sampling turned up more
often than not — the default is wrong and step-grain buffering with a retention
of its own is the answer, not a lower default interval. And if training moves
onto the same orchestrator as the pipelines in ADR_0016, the launch path should
be an engine entry rather than a shell command, at which point `train.started`
should carry the execution reference the way a launched workflow does.
