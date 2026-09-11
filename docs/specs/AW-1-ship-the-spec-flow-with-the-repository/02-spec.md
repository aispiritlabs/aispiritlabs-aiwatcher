---
id: AW-1
step: spec
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-1, step/spec, branch/main, status/doing]
---

`#spec/AW-1` · `#step/spec` · `#branch/main` · repo `aiwatcher`

# ② Spec — AW-1

## Proposal

**Intent:** a clone of this repository can drive its own spec flow, and the flow's
rules are reviewed in the pull requests that change them.

**In scope:** committing the `spec-flow` skill and the eight `/spec-*` commands under
`.claude/`; teaching `vendor.json` and `scripts/vendor-skills.py` the difference
between an authored skill and a pinned one; amending the two READMEs that state the
old rule.

**Out of scope:** adding `just skills-check` to `scripts/check.sh` or to CI (open
question in ① — a separate decision); removing the global copy; a `just` recipe for
the flow; adopting the flow in any other repository.

**Approach:** the manifest keeps answering *what may be in this tree*. It gains an
`authored` list; `--check` expects those directories instead of reporting them; the
vendoring path never writes to them. The commands need no mechanism at all —
`.claude/commands/` is outside the vendoring path entirely.

**Success criteria:**
- [x] A checkout with no `~/.claude/skills/spec-flow` can run `/spec-new`
      (a fresh clone carries the skill and the commands; an agent resolving
      them is the half no checkout asserts — `04-tests.md`)
- [x] `just skills-check` exits 0 with the authored skill present
- [x] `just skills` leaves the authored skill byte-identical
- [x] A directory nobody declared is still reported and still exits 1

## Spec (delta)

### ADDED Requirements

#### Requirement: The flow ships with the repository

The repository SHALL contain the `spec-flow` skill at
`.claude/skills/spec-flow/SKILL.md` and the eight `/spec-*` commands under
`.claude/commands/`, so that driving the flow needs no per-machine setup.

##### Scenario: a fresh clone drives the flow
- GIVEN a checkout on a machine with no `~/.claude/skills/spec-flow` and no
  `~/.claude/commands/spec-*.md`
- WHEN an agent working in that checkout is asked to open a new spec
- THEN the project-scoped command and skill are the ones it loads, and
  `docs/specs/<ID>-<slug>/_index.md` is written with a card under **Backlog**

##### Scenario: the repository's copy wins over the machine's
- GIVEN both `~/.claude/skills/spec-flow` and `.claude/skills/spec-flow` exist and differ
- WHEN a phase command runs in this repository
- THEN the contract it follows is the repository's, because project scope outranks
  user scope

#### Requirement: The manifest declares what this repository authored

`vendor.json` SHALL carry an `authored` array naming every skill directory this
repository owns rather than pins. `scripts/vendor-skills.py --check` MUST treat a
named directory as expected, MUST NOT report it as stray, and MUST report the two
kinds separately. `vendor()` MUST NOT write into an authored directory.

##### Scenario: an authored skill keeps the check green
- GIVEN `.claude/skills/spec-flow/` exists and `vendor.json` names `spec-flow` under `authored`
- WHEN `just skills-check` runs
- THEN it exits 0, and its summary counts the pinned skills and the authored ones apart

##### Scenario: an undeclared directory is still caught
- GIVEN a directory under `.claude/skills/` named in neither `sources` nor `authored`
- WHEN `just skills-check` runs
- THEN it exits 1 naming that directory

##### Scenario: re-vendoring does not touch what this repository wrote
- GIVEN an authored skill with local edits
- WHEN `just skills` runs
- THEN the directory is byte-identical afterwards, and no `PROVENANCE.md` is written into it

##### Scenario: an authored name with no directory behind it
- GIVEN `vendor.json` names a skill under `authored` and that directory does not exist
- WHEN `just skills-check` runs
- THEN it exits 1 naming it, and does not advise `just skills` — no refresh can
  create a directory with no upstream

##### Scenario: a name cannot be both pinned and authored
- GIVEN `vendor.json` names one directory in `sources` and in `authored`
- WHEN either `just skills` or `just skills-check` runs
- THEN it refuses, naming the directory, rather than vendoring over the authored copy

### MODIFIED Requirements

#### Requirement: What `.claude/skills/` holds

`.claude/skills/README.md` SHALL state that the tree holds two kinds of directory —
upstream skills vendored at a pinned commit, and skills this repository authored —
and SHALL name which are which and why the authored ones are not a second copy of
anything. (Previously: *"Everything here is vendored"*, and *"Nothing
project-specific"* with no exception.)

##### Scenario: a reader can tell the two apart
- GIVEN somebody reading `.claude/skills/README.md`
- WHEN they look for whether a directory may be edited in place
- THEN the README says that a vendored one may not and an authored one is the
  repository's own file, and names the authored ones

#### Requirement: How the flow is driven

`docs/specs/README.md`'s *Driving it* section SHALL say that the skill and the
commands ship with the repository. (Previously: they live in `~/.claude`, per
machine, and a checkout without them has no commands.)

##### Scenario: the hook does not describe a state that is no longer true
- GIVEN `docs/specs/README.md` after this change
- WHEN a newcomer reads how to drive the flow
- THEN it names `.claude/commands/` rather than `~/.claude/`

## Log
- 2026-09-10 01:00 — spec drafted on `main`
- 2026-09-10 01:06 — scenario added during ③: a declared-but-absent authored skill. Written while implementing 1.2 — the manifest would otherwise claim a directory nobody has to provide, which is the stray check inverted. The spec moved rather than the behaviour arriving unspecified.
