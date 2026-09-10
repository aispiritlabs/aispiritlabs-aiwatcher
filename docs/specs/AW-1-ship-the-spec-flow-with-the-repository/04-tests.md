---
id: AW-1
step: tests
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-1, step/tests, branch/main, status/doing]
---

`#spec/AW-1` · `#step/tests` · `#branch/main` · repo `aiwatcher`

# ④ Tests — AW-1

## Verification matrix

| Spec scenario | Test | Result |
|---|---|---|
| Manifest › an authored skill keeps the check green | `just skills-check` | ✅ exit 0 — *"9 skills match their pins, 1 authored here"* |
| Manifest › an undeclared directory is still caught | `just skills-check` with `.claude/skills/zzz-scratch/` | ✅ exit 1 — *"not in the manifest: zzz-scratch"* |
| Manifest › an authored name with no directory behind it | `--check` with `spec-flow` moved aside | ✅ exit 1 — *"authored in the manifest, absent from the tree: spec-flow"*, and no `just skills` advice |
| Manifest › a name cannot be both pinned and authored | `--check` with `rust-skills` added to `authored` | ✅ exit 1 — *"named as both pinned and authored: rust-skills"*, before any network call |
| Manifest › re-vendoring does not touch what this repository wrote | `just skills`, then `shasum` | ✅ `SKILL.md` byte-identical, no `PROVENANCE.md` written |
| Ships with the repository › a fresh clone drives the flow | — | ⚠️ not automated — see Issues |
| Ships with the repository › the repository's copy wins | — | ⚠️ not automated — see Issues |
| What `.claude/skills/` holds › a reader can tell the two apart | reading `.claude/skills/README.md` | ✅ two kinds named in the intro, an authored table, and *Adding one* split in two |
| How the flow is driven › the hook does not describe a state that is no longer true | reading `docs/specs/README.md` | ✅ *Driving it* names `.claude/skills/spec-flow/` and `.claude/commands/` |

## Commands run

```
just skills-check          exit 0   9 skills match their pins, 1 authored here
just skills                exit 0   9 skills vendored, 1 authored here and left alone
python3 scripts/lint-comments.py
                           exit 1   three pre-existing files, none in this diff — see Issues
typos <the changed files>  exit 0
```

`just check` in full was **not** run. Nothing in this diff is a crate, a panel or an
SDK file, so `cargo test --workspace`, the panel build and `just sdk-check` cannot
be about it — and the working tree holds unrelated in-progress edits (below), so a
full run would report on those.

## Issues found & resolutions

- **`just check` is red before this change.** `scripts/lint-comments.py`, one of its
  steps, exits 1 on `crates/aiwatcher-conversations/src/payload.rs:1`,
  `apps/panel/src/lib/workflow-layout.ts:3` and
  `crates/aiwatcher-server/src/conversations.rs:188`. None is in this diff. → Not
  fixed here; it is somebody else's finding and folding it into this spec would hide
  whose.
- **The working tree changed under this spec while it was being written.** At the
  start of the session `git status` held six entries; it now also holds fourteen
  modified crate and panel files and two new ones. → Concurrent work, not this
  change. It is the reason the verification above is scoped to what this diff can
  move, and the reason a full `just check` here would measure something else.
- **Two scenarios have no test.** *A fresh clone drives the flow* and *the
  repository's copy wins* are about how an agent resolves a skill, which cannot be
  asserted from inside the checkout that holds it. → What was verified instead: both
  files sit at the paths a project-scoped lookup uses
  (`.claude/skills/spec-flow/SKILL.md`, `.claude/commands/spec-*.md`) and are
  untracked-but-present in the working tree, so a commit carries them. A real check
  is a clone with `HOME` pointed at an empty directory — worth doing once, by hand,
  and out of scope here.
- **The behaviour grew past the spec during ③.** The declared-but-absent case was
  written while implementing 1.2. → The spec moved: the scenario is in `02-spec.md`
  with a log line saying when and why, rather than living only in the code.

## Quality checks

- [ ] The project's verification command passes — **not claimed**: `just check` was
      not run in full, and one of its steps is red for reasons outside this diff
- [x] Every scenario that can be tested from here has a test that fails without the change
- [x] No backward-incompatible migration — the manifest gains a key; a `vendor.json`
      without `authored` still reads (`manifest.get("authored", [])`)
- [x] No secret, no network dependency added to any gate that did not have one

## Log
- 2026-09-10 01:06 — verification run on `main`
