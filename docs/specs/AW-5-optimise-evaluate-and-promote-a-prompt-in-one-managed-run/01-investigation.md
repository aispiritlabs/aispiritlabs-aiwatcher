---
id: AW-5
step: investigation
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/investigation, branch/main, status/doing]
---

`#spec/AW-5` · `#step/investigation` · `#branch/main` · repo `aiwatcher`

# ① Investigation — AW-5

## Problem statement

Phase 15 in `docs/PIPELINE_ARCHITECTURE.md` (§28, `:1823-1831`) has two bullets
and one exit:

- an `EvaluationSuite` binding over `record_evaluation`, with planner's catalog
  gate and `ai_spirit_agent`'s DeepEval scenarios as steps;
- `AiwatcherTransport` for distributed mode (§40.6).

**Exit:** an optimise-evaluate-promote definition runs end to end, with the
verdict computed on the server as ADR_0011 requires.

The owner picked it next on 2026-09-11. Checked against three repositories, the
phase is smaller than it reads and differently shaped:

- The transport has already shipped.
- The verdict is already the server's.
- What is really missing is that the pieces do not know about each other:
  - an evaluation report does not know which run and step produced it;
  - an optimisation record does not know which reports its numbers came from;
  - a label does not know whether the version it points at was rejected;
  - a rejected candidate cannot skip the question that would promote it.

## Current state

### Distributed mode: delivered by AW-2 Phase F

`sdk/agentic/aiwatcher_agentic/runtime/distributed/aiwatcher.py:153` is
`AiwatcherTransport`, and it works like this:

- A hop is `POST /executions` of the target agent's one-step workflow, with the
  message id as the `Idempotency-Key` (`:246-270`).
- The queue is the target agent (`:216`).
- A reply goes to a mailbox, one hosted execution per reply address
  (`:437-479`).
- `just e2e-agent-transport` runs it, and lab 6's compose file runs three agents
  on it with no broker.

Two things differ from §40.6, both deliberately:

- **There is no shared history.** A hop is a run, and only replies are appended
  (`:20-27`), because nothing in a hosted execution can be claimed.
- **There is no liveness.** `AiwatcherServiceRegistry` ignores
  `max_age_seconds` (`:495-515`), because a hop to an agent with no worker waits
  in its queue.

§40.6 (`:2729-2736`) still calls the transport deferred and still names
`RedisStreamsTransport`, which no longer exists anywhere.

### Bindings

`RuntimeBinding` (`crates/aiwatcher-execution/src/plan.rs:95-122`) has these
variants: `FlowPhp`, `DataFusion` and `DuckDb` (both AW-3, uncommitted),
`Marimo`, `PublishDataset`, `PythonTask`, `HumanInput` and `ExternalWorkflow`.

`EvaluationSuite` appears only in the plan document (`:823, 1825, 2193, 2448,
2739`). The name `evaluation_suite` does appear in the code, but only as the
read-only list of suite names (`crates/aiwatcher-api/src/evaluations.rs:32`).

A registered workflow compiles each step to `PythonTask`
(`crates/aiwatcher-execution/src/definition/mod.rs:291-295`) or `HumanInput`
(`:280-290`), and the cache policy is hard-coded to `CachePolicy::Never`
(`:323`).

### Evaluation reports (ADR_0010)

- `record_evaluation` (`sdk/python/aiwatcher_sdk/__init__.py:464-525`) builds a
  fresh `Correlation(run_id=evaluation_id or new)`. It never sets
  `workflow_run_id`, even inside a task whose `TaskContext` already carries one
  (`sdk/python/aiwatcher_sdk/worker/context.py:52-59`).
- The envelope has room for that link (`crates/aiwatcher-core/src/envelope.rs:158`).
- `EvaluationSummary` (`crates/aiwatcher-projector/src/evaluations.rs:66-106`)
  has no execution field and no step field. `variant` is free text, and it is
  the only way to join a report to a prompt version (`:359`).
- `ReadModel::apply` routes every `eval.*` event away before the workflow fold
  (`crates/aiwatcher-projector/src/readmodel.rs:372-381`). A report carrying
  `workflow_run_id` therefore cannot invent an execution. It would still appear
  on that execution's live stream, which is the right place for it.
- The projection is bounded and in memory, 500 reports by default. The log is
  the record.
- ADR_0010 point 5: aiwatcher **records** evaluations and does not run them —
  "no suite runner".

### Optimisation and the verdict (ADR_0011)

- `Registry::record_optimization` (`crates/aiwatcher-prompts/src/lib.rs:544-661`)
  stores the candidate as a version whose origin is `Optimized`. It then
  computes `OptimizationRecord::verdict` from the held-out `test` scores the
  client sent, and moves `production` only if `promote` was set and the
  candidate was admitted (`:642-645`).
- **The verdict is already the server's,** from numbers the client supplied,
  which is exactly ADR_0011 §3's wording: "A client reports what it measured".
- `OptimizationRequest.evaluation_id` (`lib.rs:220`) is one opaque string. It is
  stored and never resolved.
- The optimisation id is derived from `started_at`, which defaults to now
  (`:566-575`). A task that runs twice records twice, unless it supplies
  `optimization_id` itself.
- **`set_label` does not look at the verdict** (`lib.rs:504-525`). Any author
  can point `production` at a candidate the server rejected. ADR_0011 says
  `promote` "never overrides the verdict", and the label route overrides it
  with one PUT.
- The model registry is stricter: `check_promotable` runs on every label
  (`crates/aiwatcher-training/src/model.rs:103-122`).

### Human input inside an attempt (Phase 14, closed)

- `TaskContext.ask` (`worker/context.py:135-170`) takes `role`, `choices`,
  `timeout_seconds` and `on_timeout`, so a task can ask an `admin`.
- A resumed attempt re-runs from the beginning with the answers replayed.
- `step_key` (`:77-86`) is the same on every attempt of a step: the key a record
  of the step's outcome is filed under.
- The plan has **no conditional edges**. An `ApprovalStep` is asked whenever
  its parent completes, so a question can be put to a person about a candidate
  the server has already rejected.

### Who would use it

- **planner** (`planner-mlplatform/app/evaluation/`) has the real thing:
  - `just ml-optimize` runs DeepEval's SIMBA with a fixed dev and held-out
    split (`prompt_optimization.py:42-43, 400-421`);
  - it calls `record_optimization` with `promote=False`
    (`prompt_registry.py:173-185`);
  - `just ml-eval` publishes `record_evaluation` for the catalog gate
    (`house_catalog.py:692-708`).

  Three things stop it being this phase's first user:
  - it never moves a label, and its runtime reads the prompt from a constant,
    not a label (`app/ai/_domain/_prompting.py:21`);
  - both scripts are single processes over a temporary `JsonStore`;
  - the two dataset labels already disagree (`catalog-cases@5` and
    `house-catalog@1`).

  CI runs neither script, because both make paid calls.
- **ai_spirit_agent** has DeepEval MIPROv2 and a split through
  `aiwatcher_sdk.optimization.split_cases`. However, `optimize_and_record`
  (`packages/evaluation/src/evaluation/notes_prompt_optimization_miprov2.py:170`)
  has no callers. Every entry point writes a `.txt` file, and nothing there
  calls `record_evaluation`.
- **Model promotion** has nothing to attach to. planner's `app/training/` is an
  empty package, and ADR_0018 already refuses an unmeasured version.

## Constraints

- **Files other sessions hold.** AW-3 holds `plan.rs`, `compile.rs`,
  `cache.rs`, `facts.rs`, `context.rs`, the postgres store and
  `contracts/openapi.json`, all uncommitted. A new `RuntimeBinding` variant goes
  through the exhaustive matches in the first five. Any contract change lands
  in a file that already carries their hunks, so only mine is committed, through
  a temporary index.
- **ADR_0010 point 5.** Recording is aiwatcher's job and running a suite is the
  producer's. A binding that "runs an evaluation" has to mean that a worker
  runs it.
- **No prompt in a workflow message** (`CLAUDE.md:1259`). A candidate's text
  cannot pass between steps as inline output. It is an artifact written
  through the attempt's outputs route, or a version in the registry.
- **A resumed attempt re-runs from the beginning.** Anything a task writes
  outside the store is keyed by `step_key`, or a park writes it twice. Paid
  evaluation calls belong in steps of their own, so answering a question does
  not re-run them.
- **A bounded, in-memory projection.** A server-side rule that reads a report
  back can find it evicted. Only the log outlives the projection.

## Options

### Option A — the `EvaluationSuite` binding, as §35 draws it

A new variant and spec (suite, dataset ref, prompt version), a compiler arm, and
a worker executor that calls `record_evaluation` and returns its metrics.
- Pros:
  - Typed inputs the panel can render.
  - The server knows a step is an evaluation.
- Cons:
  - As §35 draws it, it is a `PythonTask` with a fixed shape. It adds nothing
    unless the server also consumes the report, and consuming it is Option C.
  - It goes through the five files AW-3 holds, so it waits for that session.
  - It leans on ADR_0010's "no suite runner".
  - It touches none of the four gaps above.

### Option B — evaluation stays an ordinary worker task; add three joins and one refusal

1. **A report knows its step.** `record_evaluation` called inside a task stamps
   `workflow_run_id` (the execution) and the step, and the evaluation fold keeps
   both on `EvaluationSummary`.
2. **An optimisation knows its reports.** `OptimizationRequest` names the
   baseline's and the candidate's held-out reports, and the record keeps them.
   The scores are still what the client sent.
3. **A candidate knows its verdict.** The label route refuses to point
   `production` at a version the server rejected as an optimisation's
   candidate. A version a person wrote stays free to label.
4. **Proof: `just e2e-optimise`.** It registers a deterministic, no-cost
   definition:

   `evaluate(baseline) → optimise → evaluate(candidate, held-out) → record → promote`

   - The candidate passes between steps as an artifact.
   - `record` supplies `optimization_id` from `step_key`.
   - `promote` asks an `admin` through `TaskContext.ask` **only when the verdict
     admitted**, and moves the label on "promote".
   - A second run whose candidate is rejected finishes without a question, and
     the label does not move.

- Pros:
  - Touches nothing AW-3 holds except the contract.
  - Mid-attempt `ask` removes the need for conditional edges.
  - Every piece already exists and the phase only joins them.
  - The exit is proven in this repository, with no paid call and no second
    repository.
- Cons:
  - The server still believes the numbers the client sent.
  - No step is marked as "an evaluation", and the panel sees an ordinary task.
  - A contract change against a file with AW-3's hunks in it.

### Option C — B, and the server derives the held-out scores from the reports

`record_optimization` reads the two named reports and takes `test` from them,
refusing a request whose submitted scores disagree.
- Pros:
  - A client can no longer claim numbers its own reports do not show. That is
    ADR_0011's rule taken one level deeper.
- Cons:
  - The prompt registry would have to read the event log, or the bounded
    projection. A report older than the projection's cap would make a valid
    optimisation unrecordable.
  - It is a new read path from `aiwatcher-prompts` into the log. No producer
    has been caught sending numbers that disagree with its reports.

## Recommendation

**Option B.** The four gaps are what stands between "the pieces exist" and
"one run does it", and B closes all four without a runtime:

- Distributed mode is closed in the document: the two departures from §40.6
  are recorded as settled, and the stale text goes.
- The §35 `EvaluationSuite` row is withdrawn, with the reason. Option A is what
  a later spec reaches for when a panel needs to render an evaluation step as
  one.
- **Option C waits for its own gate:** a producer found sending numbers its
  reports do not show, or an optimisation that has to be judged from somebody
  else's reports.
- planner adopting it is planner's ticket. It needs a label-reading runtime, a
  split monolith and one dataset name, and none of those can be done from here.

## Open questions

- [x] **B or A?** Is a panel that renders an evaluation step as a plain task
      acceptable for now? B, then A later. — *The owner: B.*
- [x] **The label refusal.** Refuse only a *rejected optimised candidate* for
      `production`, and leave a version a person wrote free to label? Or
      refuse every label on it? The second would also block `staging`, the
      label somebody uses to try a rejected candidate on purpose. Recommend:
      `production` only, which mirrors `check_promotable`. — *The owner:
      `production` only.*
- [x] **How a report finds its step.** The client can pick it up implicitly
      inside a task, or `TaskContext` can offer an explicit
      `record_evaluation`. Recommend the explicit form: the client is shared
      across threads, and an ambient context would stamp a report written by
      unrelated code in the same process. — *Settled as recommended: the
      explicit form.*
- [x] **The existing `evaluation_id` on `OptimizationRequest`.** Keep it as it
      is and add the two held-out references beside it, or read it as the
      candidate's? Recommend keeping it: stored records carry it with no stated
      meaning, and reinterpreting it changes what they say. — *Settled as
      recommended: kept.*
- [x] **First real user.** planner's SIMBA, as its own ticket in planner after
      this lands? Or nobody until a second optimiser asks? Recommend planner,
      because it already records both halves. — *The owner: planner, as its own
      ticket.*

## Log
- 2026-09-11 12:02 — investigation written on `main`: distributed mode is already delivered, the verdict is already the server's, four joins are missing; Option B recommended, C gated, A withdrawn from §35
- 2026-09-11 12:08 — the owner answered: Option B, the refusal on `production` only, planner as its own ticket; the two technical questions settled as recommended
