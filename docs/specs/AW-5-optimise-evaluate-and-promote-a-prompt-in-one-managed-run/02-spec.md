---
id: AW-5
step: spec
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/spec, branch/main, status/doing]
---

`#spec/AW-5` · `#step/spec` · `#branch/main` · repo `aiwatcher`

# ② Spec — AW-5

## Proposal

**Intent.** Improving a prompt should be one managed run rather than three
scripts and a hand edit:

`evaluate the baseline → optimise → evaluate the candidate on held-out cases →
record → a person promotes`

Every piece exists today, and none of them knows about the others:
- a report does not say which step produced it;
- an optimisation does not say which reports its numbers came from;
- a label will point at a candidate the server rejected.

This spec joins them and proves the whole cycle in this repository. That is
Phase 15's exit.

**In scope.**
- An evaluation report recorded from a task names its execution and step.
- An optimisation names the held-out reports its scores came from.
- The `production` label refuses a candidate the server rejected.
- `just e2e-optimise`, a deterministic run of the whole cycle with no paid call.
- The plan document: distributed mode recorded as delivered with its two
  departures from §40.6, and the §35 `EvaluationSuite` row withdrawn.

**Out of scope.**
- The server deriving held-out scores from reports (Option C). It waits for its
  own gate.
- An `EvaluationSuite` binding.
- Conditional edges in a plan.
- planner adopting this, which is planner's ticket: its runtime has to read the
  label, its script has to become steps, and its two dataset names have to
  agree.
- Model promotion, which has no training code to attach to.
- Any change to `apps/panel`, which another session is rebuilding.
- The TypeScript SDK.

**Approach.** The fold, the record and the label are all the server's, so each
join is a server-side rule, and an SDK change only carries data to it. The
evaluation stays an ordinary worker task: ADR_0010's "aiwatcher records, the
producer runs" is unchanged. The run needs no plan feature that does not exist:
- a candidate's text passes between steps as an artifact, never inline;
- the question to an admin is asked from inside the promote step, and only
  when the verdict admitted, so a rejected candidate is never put to anybody.

Nothing here touches the files AW-3 holds, except the contract. Only this
spec's hunks of the contract are committed.

**Success criteria:**
- [ ] `just e2e-optimise` passes in three variants, each against a fresh
      server:
      - admitted and approved: `production` moves;
      - admitted and kept: `production` stays;
      - rejected: no question is asked and `production` stays.
- [ ] Every report the run records is listed with its execution and step.
- [ ] The run's optimisation record names both held-out reports.
- [ ] The run's stored messages never contain the candidate's text.
- [ ] A `production` label pointed at a rejected candidate is refused by name.
- [ ] §28 Phase 15, §35 and §40.6 of the plan describe what exists.

## Spec (delta)

### ADDED Requirements

#### Requirement: a report knows the step that produced it
A worker task SHALL be able to record an evaluation through its task context.
The report SHALL carry the execution as its `workflow_run_id`, and name the
step. The evaluation's summary SHALL expose both as `execution_id` and
`step_id`.

A report recorded outside a task SHALL carry neither, and SHALL be folded
exactly as before. Unless the task names an evaluation id, the id SHALL be
derived from the step's `step_key` and the report's suite and variant, so a
retried step lands on its report rather than beside it.

##### Scenario: a report recorded by a step
- GIVEN a managed run whose `evaluate_baseline` step records an evaluation
  through its task context
- WHEN the evaluation is listed
- THEN its summary names that run's execution id and `evaluate_baseline`

##### Scenario: a report recorded by a script
- GIVEN `record_evaluation` called on a client outside any task
- WHEN the evaluation is listed
- THEN `execution_id` and `step_id` are absent, and the suite, dataset, variant
  and baseline comparison are what they were before this change

##### Scenario: a report does not become a workflow execution
- GIVEN a report carrying a `workflow_run_id` that no workflow declared
- WHEN the workflow executions are listed
- THEN no execution with that id appears because of the report

##### Scenario: a retried evaluation step
- GIVEN an evaluation step whose first attempt recorded its report and then
  lost its lease
- WHEN the second attempt records the same suite and variant
- THEN one evaluation is listed for that step, not two

#### Requirement: an optimisation names the reports behind its held-out scores
An optimisation request MAY name a `baseline_evaluation` and a
`candidate_evaluation`. The stored record SHALL keep both and return them. The
server SHALL NOT resolve them: the verdict SHALL still be computed from the
`test` scores in the request.

The existing `evaluation_id` SHALL keep its current meaning. A stored record
without the two new fields SHALL still read.

##### Scenario: an optimisation that names its reports
- GIVEN two recorded evaluations, one of the baseline and one of the candidate,
  on the held-out cases
- WHEN an optimisation is recorded naming both
- THEN the record returned, and the record read back later, name both

##### Scenario: an optimisation recorded before this change
- GIVEN a record stored with no `baseline_evaluation` and no
  `candidate_evaluation`
- WHEN it is read
- THEN it reads, with both absent

##### Scenario: a named report does not decide the verdict
- GIVEN an optimisation naming reports that do not exist
- WHEN it is recorded with held-out scores that improve the primary metric
- THEN it is admitted, because the server does not read the reports (Option C
  is out of scope)

#### Requirement: `production` never points at a rejected candidate
Moving the `production` label SHALL be refused when the target version was
stored as an optimisation's candidate and the server rejected it. The refusal
SHALL name the optimisation and the verdict's reason, and SHALL leave the label
where it was.

These SHALL NOT be refused:
- any other label;
- a version a person published;
- an admitted candidate.

##### Scenario: promoting a rejected candidate by hand
- GIVEN an optimisation whose candidate was rejected because it stopped
  interpolating `{{ page }}`
- WHEN `production` is set to that candidate
- THEN the request is refused naming the optimisation and "variables lost", and
  `production` still names the version it named before

##### Scenario: trying a rejected candidate on purpose
- GIVEN the same rejected candidate
- WHEN `staging` is set to it
- THEN `staging` moves

##### Scenario: a version a person wrote
- GIVEN a version published directly, with no optimisation behind it
- WHEN `production` is set to it
- THEN `production` moves

##### Scenario: an admitted candidate recorded without `promote`
- GIVEN an optimisation admitted with `promote: false`
- WHEN `production` is later set to its candidate
- THEN `production` moves

#### Requirement: one managed run optimises, evaluates and promotes
`just e2e-optimise` SHALL register this workflow and run it against a server it
starts:

`evaluate_baseline → optimise → evaluate_candidate → record → promote`

The workflow SHALL be deterministic and SHALL reach nothing outside aiwatcher.
- The two evaluation steps SHALL record their reports through the task context,
  on the held-out cases.
- `optimise` SHALL hand the candidate on as an artifact.
- `record` SHALL record the optimisation naming both reports, with `promote:
  false`, under an id derived from its `step_key`.
- `promote` SHALL ask an `admin` whether to promote **only** when the verdict
  admitted the candidate, and SHALL move `production` only on "promote".

##### Scenario: an admitted candidate is approved
- GIVEN a candidate that improves the held-out score and keeps every variable
- WHEN the run reaches `promote`, and an admin answers "promote"
- THEN one question was asked, the run completes, and `production` names the
  candidate

##### Scenario: an admitted candidate is kept back
- GIVEN the same candidate
- WHEN an admin answers "keep"
- THEN the run completes and `production` still names the baseline

##### Scenario: a rejected candidate is never put to anybody
- GIVEN a candidate that does not improve the held-out score
- WHEN the run reaches `promote`
- THEN no question is asked, the run completes, the record's outcome is
  `rejected`, and `production` still names the baseline

##### Scenario: an editor may not answer an admin's question
- GIVEN the question from an admitted run
- WHEN an `editor` answers it
- THEN the answer is refused, and the step still waits

##### Scenario: answering does not record the optimisation twice
- GIVEN an admitted run whose `promote` step parked to ask, and whose resumed
  attempt re-runs from the beginning
- WHEN the run completes
- THEN the prompt holds one optimisation from this run, and two evaluations are
  listed for it

##### Scenario: the candidate's text stays out of the workflow store
- GIVEN any of the three variants
- WHEN the run's stored messages and step results are read
- THEN none contains the candidate's text, and the text is readable only as
  the `optimise` step's artifact and as the version in the registry

### MODIFIED Requirements

#### Requirement: Phase 15's scope in the plan
The plan SHALL:
- describe distributed mode as delivered by AW-2 Phase F, with its two
  deliberate departures from §40.6: a hop is a run rather than a row in a
  shared history, and a claim carries no liveness;
- name this spec as Phase 15's evaluation half.

(Previously: the transport was "deferred until the hosted mode works with one
worker", with `RedisStreamsTransport` named as its model, and evaluation was an
`EvaluationSuite` binding.)

##### Scenario: the plan says what exists
- GIVEN `docs/PIPELINE_ARCHITECTURE.md` after this spec
- WHEN §28 Phase 15, §35 and §40.6 are read
- THEN no row describes a binding that does not exist,
  `RedisStreamsTransport` is not named as current, and Phase 15 points at AW-5

### REMOVED Requirements

#### Requirement: the `EvaluationSuite` binding (§35)
(Reason: as drawn, it is a `PythonTask` with a fixed shape. It adds nothing
unless the server consumes the report, which Option C does and which is gated.
It would have to pass through five files another session holds. An evaluation
is a worker task that records a report, and ADR_0010 already says aiwatcher does
not run suites. A later spec can revive it when a panel needs to render an
evaluation step as one.)

## Settled

- **No new binding, three joins and one refusal** (the owner, Option B).
- **The refusal covers `production` only** (the owner). It mirrors the model
  registry's `check_promotable`. `staging` stays free, because trying a
  rejected candidate on purpose is legitimate.
- **planner is the first real user, in its own ticket** (the owner). Nothing in
  this spec reaches into planner.
- **A task records a report explicitly**, through its context. The client is
  shared across threads, so an ambient context would stamp a report written by
  unrelated code in the same process.
- **`evaluation_id` keeps its meaning.** The two held-out references sit beside
  it, because stored records already carry it, with no stated meaning to
  reinterpret.
- **The verdict stays computed from the numbers the client sent.** Option C's
  gate: a producer found sending scores its reports do not show, or an
  optimisation that has to be judged from somebody else's reports.
- **The verdict that counts is the one the version's origin names.** A version
  is content-addressed, so a text stored first as a rejected candidate keeps
  that origin. A later optimisation that admits the same text is promoted with
  its own `promote: true`.

## Still open, for the job phase

- **The report's wire shape:** `workflow_run_id` in the metadata and the step in
  `data`, and under which key — `node`, as `step.*` facts use it, or `step_id`.
- **The refusal's status:** `409`, because the label's state conflicts with the
  record, or `422`, which the other domain refusals use. Also the new
  `RegistryError` variant's name.
- **How `promote` finds the verdict:** from the id `record` derived, or from
  `record`'s own output. The first needs nothing handed on; the second is what
  the store already keeps.

## Log
- 2026-09-11 12:02 — spec drafted on `main`: four requirements added over seventeen scenarios, one modified with one, one removed; the owner's three answers and four design points settled; three questions left to the job
