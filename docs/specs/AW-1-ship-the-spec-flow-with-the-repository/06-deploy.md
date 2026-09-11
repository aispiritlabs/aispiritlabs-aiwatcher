---
id: AW-1
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-1, step/deploy, branch/main, status/done]
---

`#spec/AW-1` · `#step/deploy` · `#branch/main` · repo `aiwatcher`

# ⑥ Deploy — AW-1

## Ship

- [x] On `main`: the manifest's `authored` key, `vendor-skills.py`, the skill
      and the eight commands under `.claude/`, both READMEs
- [ ] Pushed — **the owner's step**, with the rest of `main`
- [x] Nothing to deploy: this is tooling that ships by being in the checkout

## After the push

- A fresh clone on another machine runs `/spec-new` from the repository's copy —
  the one scenario no checkout could assert.
- CI runs nothing new: `skills-check` is still outside the gates (⑤, process).

## What graduates

The rule this change made — *a skill is either pinned and vendored, or authored
here and left alone, and the manifest names both* — lives where a reader of
`.claude/skills/` meets it: `.claude/skills/README.md`, with
`docs/specs/README.md`'s *Driving it* pointing at the flow's files. No ADR: it is
cheap to reverse, and nothing else depends on it.

## Archive — what shipped, against the spec

| Requirement | Shipped |
|---|---|
| The flow ships with the repository | yes — the skill and the commands are committed, and a fresh clone carries them |
| The manifest declares what this repository authored | yes — `authored`, with the four refusals and the untouched re-vendor tested |
| What `.claude/skills/` holds | yes — two kinds, told apart in its README |
| How the flow is driven | yes — the hook names the repository's files |

## Log
- 2026-09-11 12:20 — shipped on `main` (not pushed); the rule lives in `.claude/skills/README.md`; card to Done
