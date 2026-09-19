# ADR_0034: A lab is an authored brief bound to a pinned measurement, and everything else it needs already exists

- **Status**: accepted
- **Date**: 2026-09-18

## Context

The Learning area is built on one sentence — **a workshop is a project, a
participant is a grant on it, and enrolling is redeeming an invitation** — and
nothing in the backend has the word "workshop" in it. That sentence carried the
whole access half. It carried nothing of the *content* half, and the page says
so: nine lab slots, each reading `no contract` four times over, for the brief,
the tests, the evaluation and the mark.

The obvious reading of that screen is that four things are missing and four
things have to be built. Before writing a crate, the four were held against what
this instance already has, because this repository refuses a second copy of
something it owns — and three of the four turned out to be here already, under
names that came from measuring a model rather than from teaching anybody.

**The tests are a scorecard and a cohort.** `Scorecard` is a named,
content-versioned list of 1–32 measurements, each naming the metric it writes,
JSON Pointers into the answer, the expected answer and the question, and a
scorer from a closed enum. It derives each metric's unit, direction and
aggregation itself, so an author never sends a direction. The cases are a
`Cohort`, derived from a dataset version's split by `POST
/api/v1/evaluation-cohorts` and pinned by the digest of its three files. Both
route families are already project-scoped.

**Handing work in is a recording, or answers a worker generates.** `PUT
/api/v1/evaluation-recordings/{name}` stages answers pinned by the digest of
their bytes; `Answers::Generated` names a worker task that answers each case
under the variant's own prompt and model, held to them by the traces of the runs
it made. What a participant *built* is a `VariantManifest`: model, prompt, code,
generation config, tools, workflow — every one of them a reference or a digest.
Scoped already.

**The mark is a published result.** A scoring run folds the answers against the
card and publishes an `EvaluationResult`: a number per metric per case, a
summary per metric, the usage each case cost. `GET
/api/v1/evaluation-results/{id}`, `/cases` and `/comparison` are scoped, and the
panel already refuses to decide in the browser whether a case got worse.

And the class's view of a mark needs nothing new either, which is the finding
that decided the shape of this ADR. A `context_id` is the content address of the
cohort, the split, the suite, the scorer and the metric definitions *together* —
so every result published against one cohort measured by one card shares one
context id, whoever produced it. `GET /api/v1/evaluation-results?context_id=…`
is therefore already "everybody's marks for this lab", server-ranked, with each
metric's direction coming from the card.

What is **not** here is the brief — no route in 226 holds authored instructional
text — and, less obviously, the thing that binds the other three together.
Nothing anywhere says *lab 3 of this workshop is this brief, measured by this
card on these cases*. `EvaluationContext` is that binding minus the brief, but it
is **derived at publication from a declaration**, not authored, and a declaration
needs a variant — which is the participant's work and does not exist when the
instructor writes the lab.

## Decision

**A lab is one authored document: a brief, and the measurement the work will be
held to.** It is the fifth authored registry beside prompts, datasets,
annotations, conversations and training, it takes ADR_0011's shape exactly, and
it adds no second copy of anything above.

```
Lab { name, title, position, brief, tests? }
LabTests { scorecard: VersionReference, cases: <cohort digest> }
```

Four rules carry it.

**Identity is content, and a version is `sha256` over the whole document.**
Publishing the same lab twice lands on the version that is already there, the
version object is written before the head that indexes it, and labels move
separately from publishing — so `published` is what a participant reads and a
publish with no label is the instructor drafting next week's lab while the class
is on this one. This is ADR_0011's shape with no variation.

**The tests are pinned, and pinned at a version.** A card resolved by head would
let a rewrite change what an already-issued lab measures; a lab names
`name@version`, like a scoring run does and for the same reason.

**A lab answers its own context id, and nothing else computes one.** `GET
/api/v1/labs/{name}/measurement` resolves the pinned card and cohort and returns
the `EvaluationContext` every submission to that lab will publish under, its
`context_id`, and the metrics with their directions. That is the join from a lab
to its marks, and it exists as a route because a digest over a canonicalised
document is exactly the thing a second implementation in TypeScript would get
subtly wrong — the precedent is `POST /api/v1/evaluation-approvals/address`,
which exists for that reason and no other. A lab whose tests are not pinned yet
has no measurement, and the route says so in a sentence rather than guessing
one — a 200 carrying a reason, because a lab being written is an ordinary state
and not a failed read.

**A judge is not a lab's to pin, and saying so is better than an id that means
less than it looks like it does.** A judge and a calibration set are declared
per scoring *run*, so two submissions need not have been graded by the same one
and their results would not share a context. A lab whose card asks for either is
refused by the name of the metric that asks (`LabError::Unmeasurable`), at the
moment somebody writes the lab. The same holds for a card or a cohort that is
not in this project: `LabError::Unpinned`, a 422 where the lab is written rather
than a silence where somebody submits.

**It is project-scoped from birth.** `/api/v1/orgs/{organization}/projects/{project}/labs`
is a full peer of the legacy family from the first commit: `ProjectAuthorization`
on every request, keys under `labs/scopes/<org>/<project>/registry/`, scope never
in a content hash, a registry that refuses rebinding, `X-AIWatcher-IAM: 1` and a
second grant check *after* the body arrives on every mutation, and
`Cache-Control: no-store` on every scoped response. Who may see a lab is the
grant on the project and nothing else — the workshop *is* the project, so there
is no second notion of access to invent.

## Alternatives considered

**A brief is a prompt.** The shape fits perfectly: `sha256(text)` versions,
labels, one object store, already scoped. It loses on the namespace. A prompt
name is read off a span by `PromptRef::from_data` and written as
`aiwatcher.prompt.*`; the Prompts area lists every name the registry holds; the
prompt dimension groups runs by it. A lab brief in there is a prompt as far as
every one of those is concerned, and the failure would be the `Withdrawal` one a
level up — two domains sharing a namespace, nothing reading the field until
something does. A prompt version is also `sha256(text)` and *only* that, with
nowhere to put the card and the cohort the brief is about.

**A lab is a scorecard, with the brief in its `description`.** The card already
takes 8 KiB of description, and this needs no new crate at all. It loses on the
content address: the description is part of the version, so fixing a typo in the
instructions mints a new card version and every result already published was
measured under "a different card". The tests must be able to stay still while
the text is corrected, and a scorecard cannot do that. It also holds no cohort —
the cases live in the run's declaration — and no position in a sequence.

**A lab is a naming convention over what exists** — card `lab-03`, brief as a
prompt `lab-03.brief`, in the workshop's project. No backend at all. It loses
because the browser would then be parsing a string to decide which lab it is
looking at, which is a rule in the panel about data on the server: the same
mistake as `state.is_terminal()` in TypeScript, and this repository refuses it by
name in four places.

**A lab records its submissions** — `POST /labs/{name}/submissions` naming an
evaluation id. It loses because it is a second index over a list the results
registry already answers, keyed by a digest that is already shared. A submission
that forgot to register would be missing from the lab and present in the results,
and nothing would reconcile them.

**Progress, points, a ranking and a due date.** Not built, and deliberately: no
contract in this instance carries any of them, `AreaPlaceholder` exists because a
plausible fake reads as working software, and the first bug report against a
percentage that was never real costs a day. A lab's window is already the grant's
window, which is where a due date would belong if one is ever wanted.

## Consequences

The Learning area stops being a screen with nine holes and becomes a reader of
five registries, four of which it did not need. The new code is one small crate
and one API module; the rest of the work is the panel asking questions that were
already answerable.

It costs one public function in `aiwatcher-evaluation`: `EvaluationContext::id`,
the content address of a context, which `Evaluation::prepare` already computed
inline and now calls — so there is one implementation rather than two. That is
the only file outside this work's own boundary that it touches, and it is the
right home: a context's identity belongs to the context.

It makes two gaps visible rather than creating them. `POST
/api/v1/evaluation-runs/{id}/start` has **no scoped twin**, by ADR_0033's own
rule that no project `/start` is opened until IAM-02/D; so a lab can be authored,
read and its cases fetched under a project, and the run that scores a submission
is started under instance authorization until that gate lifts. And
`/api/v1/experiments` is legacy-only, so a lab's class view is built on scoped
`evaluation-results?context_id=` rather than on the experiment rows — the same
figures, one read at a time.

It rules out a lab that is not measurable. A lab may carry a brief alone — that
is the ordinary state of an instructor halfway through writing one — but there is
no third thing it can pin instead of a card and a cohort, and "the tests are a
paragraph of prose" has nowhere to go here. A workshop whose exercises are not
machine-checkable gets a brief and four honest absences.

**What would make this wrong.** Two observations. If a lab's tests routinely
need something a scorecard cannot express — a human marking the work, or a check
that reads the participant's repository rather than their answers — then the lab
is not a pinned `EvaluationContext` and this binding is the wrong one; the
signal is instructors pinning a card with one trivially-true scorer so the slot
fills. And if two labs in one workshop ever need to share a cohort while
measuring it differently, or the same card over two cohorts, the `context_id`
join stops being one-to-one with a lab and the class view needs a key of its own
rather than a derived one.

## Amendment, 2026-09-19: a lab hands out a notebook, and does not say where marimo is

The decision above holds and gains one field. A brief is what somebody reads; it
is not what they work in, and a workshop whose exercise is "open this notebook
and make it pass" had nowhere to put the notebook.

```
Lab { name, title, position, brief, notebook?, tests? }
LabNotebook { path, revision?, command?, port? }
```

**Written twice in one day, and the second time is the one that counts.** The
first draft of this field assumed the notebook was a file uploaded into
`services/ml_pipeline` and pinned by its digest, because that was the only
notebook this repository had ever held. Put beside a real workshop — nine steps
under `labs/<track>/<lab>/steps/<NN>_<slug>/notebook.py`, each a `notebook.py`
and a `notebook_solve.py`, with a `shared/` package, a `core` package, fixtures,
a `course_manifest.toml`, its own `uv` environment and pytest cells inside the
notebooks — that assumption was simply wrong. Those notebooks cannot run in the
curation runtime, and nothing here will ever hold their bytes. What survived the
comparison is the part that was about the *lab*, and what was dropped is
everything that was about one particular place to run one.

**A lab names the notebook. Where marimo is, is not the lab's.** Those are two
questions with two different owners:

- **which notebook** is authored, versioned with the lab, and content-addressed
  with it, so moving the exercise is a new version rather than a quiet edit
  under the people already on it;
- **where it runs** is the deployment's answer (a marimo served at
  `/lab-marimo` on the panel's own origin) or the participant's own machine —
  and never a URL the lab carries. A URL out of stored data, opened by a
  browser, is the shape `AIWATCHER_WORKFLOW_RUNNER_URL` is *configuration* to
  avoid; the rule is the same one, read from the browser's side.

So the panel offers the two and takes no address from the lab. `/lab-marimo`
composes because marimo's own `--base-url` makes it emit URLs under the prefix;
the local side defaults to the participant's loopback on the port the lab's
command lands on, and the same box takes an instructor's read-only `marimo run`
address, because that is the same answer with a different host and it is typed
by the person looking at it. What that address is, is kept in that person's
browser and sent nowhere.

**The digest is present exactly when this instance holds the bytes.** For a
notebook kept in `services/ml_pipeline` it is required, and then `path` is that
runtime's name for it — `services/ml_pipeline/CLAUDE.md`'s rule, that a block
names its notebook and pins the `sha256` it was saved against, restated for the
second thing to pin one. For a file in somebody's checkout there is none, and
absence is not a weaker pin but an honest one: a digest this instance cannot
check is a promise it cannot keep, and what holds that file still is the
workshop's own version control.

**The refusals part company for the same reason.** A card and a cohort are
resolved before a lab is stored, because they are in this instance's own
registry and resolving them is a read. A notebook is not — another service's, or
another repository's — and a publish that called out to find one would put a
third party in the write path of an authored document. What is refused is a
description that could never resolve anywhere: a path that escapes upwards or is
absolute, a command that is more than one line, a privileged port, a digest that
is not one, and a digest paired with a path the notebook runtime could not name.
Everything else is reported where the lab is **read**, beside the thing that
could not be opened.

**What this does not do.** It does not hand the work in. A submission is a
staged recording or answers a worker generated, scored by a run somebody starts,
and `POST /evaluation-runs/{id}/start` still has no project twin — so a
participant's work is theirs and this instance does not yet know it exists. That
is the gap this ADR already left visible, seen from the other side.

And nothing here runs anybody's code. The panel opens a marimo that is already
running and says plainly when there is none; opening the live view is a click of
its own, for the reason `EditorHost::open` stages and stops.
