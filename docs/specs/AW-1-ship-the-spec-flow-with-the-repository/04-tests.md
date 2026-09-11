---
id: AW-1
step: tests
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-11
tags: [spec/AW-1, step/tests, branch/main, status/doing]
---

`#spec/AW-1` · `#step/tests` · `#branch/main` · repo `aiwatcher`

# ④ Tests — AW-1

## Verification matrix

| Spec scenario | Test | Result |
|---|---|---|
| Manifest › an authored skill keeps the check green | `just skills-check` | ✅ exit 0 — *"9 skills match their pins, 1 authored here"* (2026-09-10, again 2026-09-11) |
| Manifest › an undeclared directory is still caught | `just skills-check` with `.claude/skills/zzz-scratch/` | ✅ exit 1 — *"not in the manifest: zzz-scratch"* |
| Manifest › an authored name with no directory behind it | `--check` with `spec-flow` moved aside | ✅ exit 1 — *"authored in the manifest, absent from the tree: spec-flow"*, and no `just skills` advice |
| Manifest › a name cannot be both pinned and authored | `--check` with `rust-skills` added to `authored` | ✅ exit 1 — *"named as both pinned and authored: rust-skills"*, before any network call |
| Manifest › re-vendoring does not touch what this repository wrote | `just skills`, then `shasum` | ✅ `SKILL.md` byte-identical, no `PROVENANCE.md` written |
| Ships with the repository › a fresh clone drives the flow | `git clone` of `main` into a scratch directory, then `scripts/vendor-skills.py --check` inside it | ✅ as far as a checkout can say — the clone holds `.claude/skills/spec-flow/SKILL.md` and all eight `.claude/commands/spec-*.md`, and its check prints *"9 skills match their pins, 1 authored here"*. That an agent then resolves them is the half no checkout can assert |
| Ships with the repository › the repository's copy wins | — | ⚠️ not automated — see Issues |
| What `.claude/skills/` holds › a reader can tell the two apart | reading `.claude/skills/README.md` | ✅ two kinds named in the intro, an authored table, and *Adding one* split in two |
| How the flow is driven › the hook does not describe a state that is no longer true | reading `docs/specs/README.md` | ✅ *Driving it* names `.claude/skills/spec-flow/` and `.claude/commands/` |

## Commands run

2026-09-10:

```
just skills-check          exit 0   9 skills match their pins, 1 authored here
just skills                exit 0   9 skills vendored, 1 authored here and left alone
python3 scripts/lint-comments.py
                           exit 1   three pre-existing files, none in this diff
typos <the changed files>  exit 0
```

2026-09-11, after the fixes below:

```
git clone . <scratch> && vendor-skills.py --check
                           exit 0   9 skills match their pins, 1 authored here
just check                 exit 1   18 of 19 steps PASS; FAIL comments —
                                    crates/aiwatcher-conversations/src/payload.rs:1,
                                    a 45-line module doc, outside this diff
```

## Issues found & resolutions

- **`just check` is red on one file this change never touched.**
  `crates/aiwatcher-conversations/src/payload.rs:1` is a 45-line module
  comment, last changed in `6f7a2da`. → Not fixed here, for the reason the first
  run gave: it is somebody else's finding. Of the three files that run named, the
  panel one is gone with the panel rebuild, and
  `crates/aiwatcher-server/src/conversations.rs:188` cited a plan section where
  it could state the rule — a one-line fix, made in `ee0d538`.
- **Running `just check` in full found two things today's licence change broke,
  and one it did not.** The OpenAPI document carries the workspace licence in
  `info.license`, so the contract went stale (`670bf4e`); and `taplo` refused
  `sdk/agentic/pyproject.toml`, which was unformatted since AW-2 created it
  (`8db5d61`). → Fixed, and a second full run confirmed both.
- **The full run measured other sessions' work too.** The working tree holds
  AW-3's engine and contract changes and the panel rebuild. → Every step they
  reach passed, so the one red step is the only thing in the way; the result is
  recorded as what it is, not scoped away.
- **One scenario still has no test.** *The repository's copy wins* is about how
  an agent resolves a skill when a machine has one of the same name, which
  nothing inside a checkout can observe. → What was verified instead: the
  repository's copy is at the project-scoped path, committed, and present in a
  fresh clone.
- **The behaviour grew past the spec during ③.** → As recorded on 2026-09-10: the
  declared-but-absent scenario is in `02-spec.md` with a log line.

## Quality checks

- [ ] The project's verification command passes — **not claimed**: 18 of 19
      steps pass; the one that fails does so on a file outside this diff
- [x] Every scenario that can be tested from here has a test that fails without the change
- [x] No backward-incompatible migration — a `vendor.json` without `authored`
      still reads (`manifest.get("authored", [])`)
- [x] No secret, no network dependency added to any gate that did not have one

## Log
- 2026-09-10 01:06 — verification run on `main`
- 2026-09-11 12:20 — re-verified: a fresh clone carries the flow; full `just check` 18/19, the one failure outside this diff; the contract licence and a `taplo` format fixed on the way
