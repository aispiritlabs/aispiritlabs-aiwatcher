# Agent skills

Reference material an agent loads on demand while working in this repository.
Each directory is one skill: a `SKILL.md` whose frontmatter is always in
context, and `rules/` beneath it that is read only when the work calls for it.

Everything here is **vendored** — copied in at a pinned commit rather than
fetched when it is needed. Same reason the prompt registry is an object store
rather than a fold: what an agent read has to stay readable later, and a
network dependency in the middle of a review is a review that behaves
differently depending on the day. `vendor.json` holds the pins,
`licenses/` the upstream licences, and every skill carries a `PROVENANCE.md`
naming its repository, path and commit.

```bash
just skills          # re-vendor at the pinned commits
just skills-check    # is the tree still what the manifest says?
just skills-update   # move every pin to upstream HEAD, then re-vendor
```

`just check` does **not** cover this, for the reason `just diagrams` is not
covered either: a stale skill is a documentation problem, and wiring it into
CI would make it a build failure on a machine with no reason to care.

## What is here, and why

| Skill | Earns its place because |
|-------|-------------------------|
| `otel-instrumentation` | `aiwatcher-trace` folds events into spans and exports OTLP/JSON. Span names, span kinds, status codes and sampling are decisions this crate makes on every event. |
| `otel-semantic-conventions` | The attribute registry. `gen_ai.request.*`, `gen_ai.prompt`, `gen_ai.completion` and the `aiwatcher.prompt.*` attributes beside them are conventions, and a wrong one is a dimension nobody can group by. |
| `otel-collector` | `deploy/otel-collector.yaml` is in the path for a reason — the redaction processor. A guardrail that depends on a Collector config is worth being able to read the Collector's own rules for. |
| `otel-ottl` | That redaction is OTTL. So is anything else the Collector is asked to filter, route or transform. |
| `rust-skills` | 265 rules over the workspace's own language: ownership, error handling, async, `unsafe`, API design, and an anti-patterns category. `clippy` decides what fails; this decides what to write. |
| `tanstack-query-best-practices` | `apps/panel` is TanStack Query v5. Every list that can grow with retention is a `useInfiniteQuery`, and cache keys are what make a filtered link land on the same view. |
| `tanstack-router-best-practices` | The panel keeps its filters in the URL rather than in component state — search params, typed loaders and route grouping are exactly this skill's subject. |
| `hf-cli` | `aiwatcher-annotations`' hub integration searches Hugging Face. The CLI is how a corpus is inspected before `integrations::fetch` is pointed at it. |
| `huggingface-datasets` | The Dataset Viewer API — subsets, splits, row pagination, parquet URLs. What a staged import (ADR_0022) reads before it is staged. |

Two upstream skills were **left out** deliberately.
`tanstack-start-best-practices` and `tanstack-integration-best-practices` are
about TanStack Start, and the panel is a Vite SPA against a Rust API: advice
about server functions and SSR would be advice about a different application.
`huggingface-vision-trainer` trains on Hugging Face Jobs, and this project's
vision training is `e2e-train` against its own annotation export — the same
subject, the wrong platform.

## What is not here

**Nothing project-specific.** `CLAUDE.md` is this repository's own guide and
the ADRs under `docs/ADR/` are its reasoning; a skill restating either would be
a second copy free to disagree with the first.

**Nothing already installed globally.** The `python-*` family
(`~/.claude/skills/`) covers `sdk/python` and `services/ml_pipeline` —
architecture, async, data, errors, events, performance, quality, testing,
types — and `archify` is what `just diagrams` renders with. Those are the
machine's, not the repository's, and vendoring a copy would be two versions of
one skill with nothing saying which is current.

**Nothing for Go.** There is none in this workspace.

## Adding one

Add it to `vendor.json` and run `just skills`. A destination name must match
the skill's own `name:` frontmatter, which is why the two TanStack skills are
vendored under their `-best-practices` names. Never edit a vendored file: an
edited copy drifts from upstream with nothing to say so, and `just skills`
overwrites it. Fork it under a different name instead.
