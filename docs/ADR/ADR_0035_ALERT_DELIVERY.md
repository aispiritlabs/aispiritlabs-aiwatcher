# ADR_0035: A notification is named by what happened, the destination is configuration, and nothing here promises exactly-once

- **Status**: accepted
- **Date**: 2026-09-19

## Context

Everything in this system waits to be read. A managed run that gave up at four
in the morning is a row in a list somebody opens at nine; a candidate two points
worse than its baseline is a gate a pipeline asked about and nobody else did.
That is the right default for an observability tool — an instance that paged
somebody on every `step.failed` would be an instance nobody kept — and it is
the wrong default for exactly two facts, which is what this adds.

The two are **a terminal execution failure** and **a regression in a completed,
comparable evaluation**. Both are already decided here and decided once. An
execution's terminal failure is `execution.failed` on the log (ADR_0026), folded
into the workflow projection with its reason. A regression is `gate::decide`'s
word (ADR_0030): four verdicts, `regression` being the one that means *a change
somebody made*, as distinct from `incomplete` — a measurement that could not
say — and `error` — a pair that does not compare. Neither needs a new judgement,
which is why this decision is about **delivery** rather than about detection.

Three things had to be decided, and each has a failure mode that is silent.

## Decision

### A destination is configuration, never authored

A rule says *what is worth telling somebody about*. Where it goes is
`AIWATCHER_ALERT_WEBHOOK_URL`, one channel for the deployment, with an optional
bearer token and an optional HMAC signing secret beside it.

This is `aiwatcher-runner`'s rule for `AIWATCHER_WORKFLOW_RUNNER_URL`, applied a
second time for the same reason: aiwatcher runs inside a cluster, and a URL that
arrives in a request body is a request-forgery primitive posted by anything that
can publish a rule — a link-local metadata address, an internal admin port, a
service that takes a POST as a command. Making the endpoint authored data would
have bought per-rule routing and sold the whole outbound boundary for it. One
channel is less than a deployment eventually wants; it is the part that can be
widened later without taking the boundary with it.

The same line runs through what is *said about* the channel. `GET
/api/v1/system` reports that it is configured, which variable configured it and
whether it signs — and never the address, because an endpoint is not a
credential and is still reconnaissance for somebody already inside.

### A delivery is named by `sha256(rule version ‖ what happened)`

The dedup key **is** the object's name in the store, and creating it is
`ObjectStore::create` — atomic create-if-absent. Everything about repeats falls
out of that one choice:

- **A source read twice raises one alert.** The watchers re-read their sources
  after a restart, after a lost cursor, after a crash between raising and
  moving a cursor. Every one of those lands on the object the first pass wrote,
  and the caller is told `created: false`. There is no lock, nothing to expire,
  and no window in which two processes both decide.
- **Editing a rule asks the question again.** A tolerance loosened after a
  regression fired is a different question about the same result, and the new
  answer is worth having. Because a rule version is a digest of its content, the
  key moves exactly when the rule's meaning does.
- **Silencing a rule is not an edit.** `enabled` lives on the head rather than
  in a version, so switching a rule off during an incident and back on
  afterwards does not move a single key — and therefore does not re-send
  everything it already sent.
- **A first pass sends nothing.** A deployment turning alerts on has a history
  behind it. A watcher with no cursor writes one where its source is now and
  stops, which is the scheduler's decision about slots it never saw, for the
  same reason: a new channel that opens by paging somebody about last month is
  worse than no channel.

### Nothing here promises exactly-once, and the key is how it says so

A receiver that acknowledged into a socket that then broke is indistinguishable
from one that never heard. This retries, so a receiver can be told twice.

Rather than build the delivery machinery that cannot close that gap either, the
key travels **with** the payload — in the body and in an `Aiwatcher-Delivery-Key`
header — and the receiver decides. A receiver that stores the key and ignores a
repeat has the guarantee this cannot give it; one that does not, gets a repeat
it can recognise.

The retry decision itself is `aiwatcher_jobs::after_failure`, called and not
copied: three attempts for something worth coming back for, immediate failure
for a refusal, with retryability read off `PortError` exactly as every other
adapter here sets it. What this crate owns is only how long a retry waits —
thirty seconds, then five minutes — because a shard job has no opinion about
that and a receiver answering 503 is asking for a moment.

A channel down for longer than that budget **fails** the delivery rather than
holding it. The record says `failed` with the last error on it, the history
shows it, and an admin sends it again by hand through `POST
/api/v1/alert-deliveries/{key}/retry`, which starts the budget over. A queue
that waited for ever would report a healthy channel to anybody who did not look,
which is the failure this whole area exists to prevent.

## Consequences

**A rule is authored data**, so this area needs the object store the prompt
registry needs, and a deployment without one gets a 501 naming
`AIWATCHER_PROMPT_STORE` rather than an empty list. The delivery record is the
queue row and the history row at once, because a delivery's history *is* what
became of it, and the history is bounded by `AIWATCHER_ALERT_HISTORY_DAYS`. The
sweep forgets **finished** rows only: a queued notification older than the
window is one nothing has taken yet, and deleting it would lose quietly what
failing it says loudly.

**The evaluation baseline is derived, not named.** A published result is held to
the last result of the *same variant* in the *same context* — same context
because that is what makes two results comparable at all, and same variant
because a different one is a different thing being measured, so calling it a
regression would report every experiment as a problem the moment somebody ran
one. A rule that wants a pinned release baseline instead does not have one yet;
what it has is a context narrowing and a `GatePolicy`.

**The watchers read folds, so they live in the serving role.** The workflow
projection is in this process's read model and the evidence index is its object
store; a work role holds neither. That is the opposite of where the outbox
publisher sits, and it is the honest placement rather than a symmetry.

**What is not done, said as plainly as what is.** There is one channel and no
per-rule routing. There is no windowed metric rule — a period, a minimum
sample, a cooldown and an explicit answer for no data are a second kind of rule
and a scheduled evaluation, and this one is event-shaped. Nothing here
aggregates: fifty executions of one definition failing in a minute are fifty
notifications, which is the correct answer for a deployment running three
definitions and the wrong one for a deployment running three hundred.

## Alternatives considered

**A destination per rule.** Rejected above: it sells the outbound boundary for
routing. A deployment that needs two destinations today can point the one
webhook at something that fans out, which is a receiver's job and is where the
routing table belongs anyway.

**Alerting from the projector, on the event.** The projector sees
`execution.failed` as it arrives, which would be quicker. It would also put an
outbound HTTP client and a durable queue inside the consumer whose one job is to
commit a checkpoint after a durable write, and a channel having a bad day would
become backpressure on the read model. Polling a fold every thirty seconds costs
a notification half a minute and keeps the pipeline what it is.

**A second retry budget for deliveries.** Five attempts with a longer backoff
was tempting and would have been a second copy of a rule `aiwatcher-jobs` owns.
The hand retry covers the case the budget does not — an outage measured in hours
— and it carries something an automatic retry never does: somebody looked.
