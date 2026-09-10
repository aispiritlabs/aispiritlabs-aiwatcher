---
id: AW-1
step: investigation
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-1, step/investigation, branch/main, status/doing]
---

`#spec/AW-1` · `#step/investigation` · `#branch/main` · repo `aiwatcher`

# ① Investigation — AW-1

## Problem statement

The spec-driven flow this repository's work now runs through lives entirely in
`~/.claude` — per machine, outside version control. What is committed is the
artefacts (`docs/specs/`) and the project hook that points at this repository's own
commands and rules. So a clone on another machine reads every spec perfectly and has
nothing to drive one with, the flow's own rules never appear in a pull request, and
onboarding is "copy these nine files from somebody's laptop".

The obvious fix — put them in `.claude/` — meets a tree whose stated rule is that
**everything in it is vendored** at a pinned commit, and a check that enforces it.
The question is not whether the flow belongs in the repository. It is what the
vendoring rule actually protects, and whether an authored skill can sit beside a
pinned one without dissolving it.

## Current state

- `~/.claude/skills/spec-flow/SKILL.md` — the contract: layout, frontmatter schema,
  the `resolve` / `move_card` / `bump_index` snippets, the rules. Not in the repository.
- `~/.claude/commands/spec-{new,investigate,propose,job,tests,review,deploy,board}.md`
  — eight commands, each of which opens by saying *load the `spec-flow` skill*. Not
  in the repository.
- [`docs/specs/README.md`](../README.md) — committed. Its *Driving it* section states
  the split honestly: the flow is per machine, a checkout without it still reads.
- [`docs/specs/BOARD.md`](../BOARD.md) — committed, eight columns, this spec its first card.
- `.claude/skills/` — ten vendored skills, `licenses/`, `vendor.json`, `README.md`;
  408 tracked files. Every skill carries a `PROVENANCE.md` naming repository, path
  and commit.
- `scripts/vendor-skills.py:31` — `NOT_VENDORED = {"vendor.json", "README.md", "licenses"}`.
  `--check` re-vendors into a scratch directory, diffs each pinned skill, and then
  reports every remaining entry of `.claude/skills/` as `not in the manifest`,
  exiting 1 (`scripts/vendor-skills.py:143-153`).
- `justfile:971` — `skills-check` is the only caller.
- `scripts/check.sh` — **does not call it**. `.github/workflows/ci.yml` — no mention
  of skills at all. The gate is local, run by a person who chooses to.
- `.claude/commands/` — **does not exist**, and nothing in the vendoring path names
  that directory. Commands are free of the collision entirely.
- `.claude/skills/README.md` — *"Everything here is **vendored**"*, and under *What is
  not here*: *"Nothing project-specific"* (a skill restating `CLAUDE.md` or the ADRs
  would be a second copy free to disagree) and *"Nothing already installed globally"*
  (two versions of one skill with nothing saying which is current).

## Constraints

- Claude Code discovers project skills only at `.claude/skills/<name>/SKILL.md` and
  project commands only at `.claude/commands/<name>.md`. There is no third location,
  so "put the skill somewhere the check does not look" is not available.
- `CLAUDE.md`'s rule against a second copy is about *reference material restating this
  repository's own reasoning*. `spec-flow` restates neither `CLAUDE.md` nor an ADR: it
  is process, and every repository-specific fact it needs is in
  `docs/specs/README.md`'s project hook, which points rather than copies.
- The *"nothing already installed globally"* rule bites hardest here, because the
  global copy is not going away: `planner` and `ai_spirit_agent` use the same flow.
- Never edit a vendored file — `just skills` overwrites it. Whatever is authored must
  be something `vendor()` never writes to.

## Options

### Option A — leave it global
- Pros: one copy, nothing to keep in sync, no change to any script or README.
- Cons: every cost in the problem statement stays. The flow's rules — what a phase
  may fabricate, what graduates to an ADR — are never reviewed, because they are not
  in a diff anywhere.

### Option B — commands in the repository, skill stays global
- Pros: `.claude/commands/` is untouched by the vendoring path, so zero change to
  `vendor-skills.py` and `just skills-check` stays green by construction.
- Cons: every command's first instruction is *load the `spec-flow` skill*, which a
  fresh clone does not have. The commands degrade to prose describing a contract
  nobody can read. Half a flow reads like a whole one, which is worse than neither.

### Option C — both in the repository, with the manifest naming what is authored
`.claude/skills/spec-flow/` and `.claude/commands/spec-*.md` committed; `vendor.json`
gains an `authored` list; `--check` treats those directories as expected rather than
stray; `.claude/skills/README.md` says the tree now holds two kinds of thing.
- Pros: a clone has the whole flow. The rules land in pull requests. The manifest
  stays the single answer to *what may be in this tree*, which is the only thing the
  stray check was ever protecting.
- Cons: two copies of `spec-flow` on this machine. Project scope wins, so the
  repository's copy is what runs here — but the global one still exists and will
  drift, and the skills README's flat claim stops being true and has to be amended.

### Option D — repository only, delete the global copy
- Pros: one copy, no drift, the strongest form of C.
- Cons: the flow stops existing in every other repository, which is the reason it was
  made global. `planner` would need its own copy, and then there are three.

## Recommendation

**Option C**, with the exemption written as a declaration rather than a sniff.

The tempting shortcut is to treat a directory with no `PROVENANCE.md` as authored.
That is worse than the list it replaces: a directory somebody dropped in by mistake
has no `PROVENANCE.md` either, and it is exactly the case the stray check exists to
catch. `vendor.json` gaining `"authored": ["spec-flow"]` keeps the manifest
answering the question it already answers, and a stray directory is still a stray
directory.

The drift between the two copies is the real cost and is worth stating plainly:
they will diverge, and the repository's copy is authoritative *for this repository*
because project scope wins. `spec-flow` is a process contract that changes rarely,
so the cost is low — but it is not zero, and the day the global copy grows a rule
this one lacks, this note is where somebody will look.

## Open questions

- [x] Should `just skills-check` join `scripts/check.sh`? **No — and this
      repository had already decided it in writing.**
      `.claude/skills/README.md:21-23`: *"`just check` does not cover this, for the
      reason `just diagrams` is not covered either: a stale skill is a documentation
      problem, and wiring it into CI would make it a build failure on a machine with
      no reason to care."* Writing this question down was reading that tree without
      reading its README. Running the check added a second reason the prose does not
      give: `--check` re-vendors before it compares, so it `git fetch`es four upstream
      repositories — and `scripts/check.sh` is deliberately the no-network,
      no-cluster mirror of CI, which is why the Iggy and Tilt suites are outside it
      too. In `check.sh` it would go red on a plane, and in CI it would go red the
      day an upstream repository moves, in a pull request about this one. If it
      should run unattended at all, it is a path-filtered job of its own — outside
      this spec.
- [ ] Does the global copy stay the source for other repositories, or does each get
      its own? Only matters when the second repository adopts the flow.

## Log
- 2026-09-10 00:59 — investigation written on `main`
- 2026-09-10 01:06 — open question answered: skills-check stays out of `check.sh`, and `.claude/skills/README.md:21-23` had already said why
