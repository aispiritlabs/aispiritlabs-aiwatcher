# ADR_0011: A prompt is authored, not observed, and lives in an object store

- **Status**: accepted
- **Date**: 2026-08-28

## Context

[ADR_0010](ADR_0010_EVALUATION_REPORTS.md) gave evaluation reports somewhere to
live and stopped short of what those reports are usually *about*. Reading
`planner-mlplatform` again, the thing being evaluated is almost always a
prompt:

- `app/ai/floor_plan.py` exports `FLOOR_PLAN_SYSTEM_PROMPT`,
  `FLOOR_PLAN_PROMPT_VERSION` and `FLOOR_PLAN_PROMPT_SHA256`. The version is a
  hand-incremented integer and the hash is a `hashlib.sha256` computed at
  import, which somebody has to remember to update together.
- `app/prompt_builder.py` renders Markdown templates with `{{ variables }}` out
  of `app/prompts/`.
- `app/evaluation/prompt_optimization.py` runs DeepEval's `PromptOptimizer`
  with `SIMBA` over a frozen dev/test split, and writes the winner to a file
  under `artifacts/`.

So a prompt already has a version, a hash, declared variables and an
optimisation history — and all four live in a git checkout, a constant and a
directory of JSON. Three things follow from that, and each of them costs
something:

1. **A trace cannot say what it ran on.** A span carries `gen_ai.request.model`
   and the tokens, and the prompt reaches the model as text that the Collector
   redacts before export. Six weeks later "run X scored 0.61" is unanswerable:
   nothing in the system holds the text.
2. **An optimiser's own number is not evidence.** SIMBA selects a candidate by
   maximising a score, and then reports that score.
   `prompt_optimization.py` already knows this — `DEV_CATALOG_IDS` and
   `TEST_CATALOG_IDS` are disjoint and `split_catalog_cases` refuses to run if
   they drift. That discipline lives in one Python file, and nothing carries it
   between runs or across services.
3. **An optimiser rewrites text freely.** A candidate that scores well and has
   quietly stopped mentioning `{{ page }}` is a prompt that describes a
   plausible house instead of the one on the page. The harness fed it the same
   fixed inputs every iteration, so no score can show this.

The obvious place to put a prompt is the read model, beside runs and
evaluations. That is wrong for one reason that settles it: **everything else
here is bounded by retention.** `ReadModelConfig` sheds the oldest runs,
`EvaluationConfig` sheds the oldest reports, and the log itself is a rolling
window. A prompt has to be readable after every run that used it is gone.

## Decision

**The prompt registry is authored state in an object store, behind an
`ObjectStore` port, with three rules the server enforces.**

Concretely:

1. **A version is its text.** `PromptVersionId` is `sha256(text)`, lowercase
   hex — the same value `planner` already computes as `FLOOR_PLAN_PROMPT_SHA256`
   and the same value the Python SDK computes locally. Publishing is therefore
   idempotent: a deploy job that publishes on every start writes one version.
   This is [ADR_0001](ADR_0001_EVENT_ENVELOPE.md)'s derivation rule applied to a
   different kind of id, for the same reason.

2. **The store is S3, and the layout belongs to the registry.** The port is
   `ObjectStore` — four blob operations — rather than a domain-level
   `PromptStore`, because the key layout and the immutability rules are the
   same whether the bytes land in RustFS or in a directory, and duplicating
   them per adapter is how two adapters end up disagreeing about where a
   version lives. The layout is:

   ```text
   {prefix}/{name}/head.json                      mutable index: labels, description, summaries
   {prefix}/{name}/versions/{version_id}.json     immutable, content-addressed
   {prefix}/{name}/optimizations/{id}.json        immutable
   ```

   `head.json` holds no fact that is not also in an object it points at, except
   the labels — which are pointers somebody moved and live nowhere else.
   `Registry::rebuild` re-derives it by listing. The version object is written
   **before** the head that indexes it, which is `pipeline.rs::flush`'s
   checkpoint ordering applied to a different write.

3. **The verdict is computed by the server.** A client reports what it
   measured; `OptimizationRecord::verdict` decides. A candidate is `admitted`
   only when all of:

   - it differs from the baseline,
   - it still interpolates every variable the baseline declared, and
   - it improves the **held-out** score on one named primary metric.

   Anything else is `rejected` with a reason. `promote: true` moves the
   `production` label *if* the candidate was admitted; it never overrides the
   verdict.

The store defaults to a directory under `AIWATCHER_DATA_DIR`, so
`cargo run --bin aiwatcher` still starts a usable instance with no setup.
`AIWATCHER_PROMPT_STORE=s3` is what a deployment runs, and
`AIWATCHER_PROMPT_STORE=none` makes every `/api/v1/prompts` route answer 501 —
distinct from an empty registry, and the panel says which.

### Why RustFS

It is one Rust binary, ~200 MB of image, no JVM and no external metadata
service, and it speaks S3. The S3 part is what matters: the adapter is
`aiwatcher-prompts::adapters::s3`, and it reaches MinIO, Ceph's RADOS gateway,
SeaweedFS or AWS by changing one environment variable. planner's k3s already
runs a RustFS that `planner-web`, `planner-import-api` and `planner-mlflow`
write to, and `deploy/environments/planner.yaml` points the registry at it
rather than installing a second one.

### Why the signer is written out

`aws-sdk-s3` brings a credential-provider chain, a retry policy, a region
resolver and an HTTP stack this workspace already has — several hundred crates
to sign four request shapes against an endpoint that is in an environment
variable. `sigv4.rs` is a fixed sequence of HMACs with no policy in it, pinned
by RFC 4231's vectors, and verified against a real RustFS by
`crates/aiwatcher-prompts/tests/rustfs.rs`. That last part is not optional: a
self-consistent signer and a correct one are indistinguishable without a server
that also implements SigV4.

## Alternatives considered

**Prompts as events on the durable log.** The consistent answer — everything
else here is a fold — and it fails on the one requirement that made the
registry necessary. The log has retention; the fold that read it would have to
be unbounded, in memory, forever. A separate never-evicted stream would be an
object store with extra steps.

**Prompts in git, with aiwatcher reading a checkout.** Where planner keeps them
today, and where prompts written by a person genuinely belong. It does not
survive the optimiser: SIMBA produces a candidate at 03:00 in a CI job, and a
registry that requires a commit to record one either blocks the run or invents
a bot committer. The two are not exclusive — a service can publish its
checked-in prompt on start-up, which is exactly what `publish` being idempotent
is for.

**A relational table.** A prompt with a version, a label and a history is a
textbook three-table schema, and adding Postgres to a project whose whole
storage story is "a log plus two time-series stores" is a larger decision than
the feature. It also does not solve the requirement any better: the reason the
data outlives retention is that nothing evicts it, not that it is in rows.

**A domain-level `PromptStore` port.** Considered and rejected above: it would
put the key layout in every adapter.

**Letting the client report the outcome.** Simplest, and it makes the registry
a filing cabinet. The whole value of a held-out gate is that something other
than the optimiser applies it.

**Blocking a rejected candidate from being stored.** Tempting, and wrong: a
rejected experiment is a result, and losing it means running it again. The
candidate is stored and listed; what it does not get is the label.

**Comparing an optimisation against the newest version rather than a named
baseline.** Would let two concurrent optimisations each claim the other's
output as their starting point. `baseline` is required, and must already be in
the registry.

## Consequences

**Listing is one `GET` per prompt.** An object store has no query, and a global
index would be a second thing to keep consistent with the objects that are
already the truth. Sixteen requests run concurrently, which keeps a
hundred-prompt registry inside one round trip's worth of latency. A registry of
ten thousand prompts wants a different design, not a larger constant.

**The head can lose a concurrent write.** Two publishes landing together can
leave one out of the index. It is a display problem rather than data loss — the
objects are the truth — and `POST /api/v1/prompts/{name}/rebuild` repairs it.
An object store with no compare-and-swap cannot do better without a lock
service, which would be a larger dependency than the problem.

**The registry has no authentication of its own.** Neither does the rest of the
API; it sits behind whatever guards the ingress, which for planner is
authentik. The difference is that this surface *writes* something durable and
irreplaceable, so an unauthenticated aiwatcher is now worse than it was.

**Prompt text is not redacted.** The Collector strips `gen_ai.prompt` and
`gen_ai.completion` from spans; a prompt in the registry is the whole point of
the registry and is stored verbatim. A producer that puts secrets, keys or
personal data into prompt text is putting them in an object store that keeps
them forever, and no retention policy in this system will remove them.

**The variable rule is syntax-specific.** It reads `{{ name }}`, which is what
`planner`'s `PromptBuilder` enforces. A prompt using f-strings, Jinja
expressions or a chat template declares no variables, and the variable-loss bar
does not apply to it — it degrades to the score gate alone rather than to a
false positive. Rust and Python implement the same scanner, and they are tested
against the same cases, because a disagreement between them would reject a
candidate for losing a variable nobody thought it had.

**One primary metric decides.** A gate with several thresholds is a gate
somebody tunes until it opens. Every other metric is reported and promotes
nothing.

**What would make this wrong.**

- A registry where `list` is the slow path — enough prompts that one `GET` each
  stops being acceptable. The number to watch is the p95 of
  `GET /api/v1/prompts`; past a second, the head index has to become a real
  index or the store has to become a database.
- Two writers routinely losing head entries, seen as `rebuild` being run as
  maintenance rather than after an incident. That is the point at which the
  object store needs conditional writes (S3 now has `If-None-Match`; RustFS
  1.0's support for it is worth re-checking) or a different store.
- A team that wants `production` to move automatically on an admitted
  candidate. The split between recording and promoting is deliberate, and if it
  is being worked around with a script that reads the record and calls
  `set_label`, the split is in the wrong place.
- Prompts that are not text: a structured chat template where flattening the
  messages into one string loses something a reader needs. The SDK's DeepEval
  bridge already flattens, and says so.
