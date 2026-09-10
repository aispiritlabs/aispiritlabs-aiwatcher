---
id: AW-1
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-1, step/job, branch/main, status/doing]
---

`#spec/AW-1` · `#step/job` · `#branch/main` · repo `aiwatcher`

# ③ Job — AW-1

## Technical approach

Two thirds of this is copying files. The part with a decision in it is thirty lines
of `scripts/vendor-skills.py`: read `authored` from the manifest, subtract those
names from the stray set, refuse an overlap with `sources`, and report the two counts
separately so a green run says what it actually checked. The commands need no
mechanism — `.claude/commands/` is outside the vendoring path, which is why this
change touches one script and not two.

## Decisions

- **Decision:** declare authored skills in `vendor.json` — *because* the manifest is
  already the answer to "what may be in this tree", and the alternative (treat a
  directory with no `PROVENANCE.md` as authored) would silently accept the mistake
  the stray check exists to catch. Alternative rejected: sniffing for a missing
  `PROVENANCE.md`.
- **Decision:** keep the global copy in `~/.claude` — *because* `planner` and
  `ai_spirit_agent` run the same flow, and project scope means the repository's copy
  is what runs here regardless. Alternative rejected: deleting it, which would take
  the flow away from every other repository to buy a drift this note records instead.
- **Decision:** commands go to `.claude/commands/`, unmanaged — *because* nothing
  vendors them and nothing needs to. Alternative rejected: a second manifest for
  commands, which would be machinery for a directory with no upstream.
- **Decision:** `just skills-check` stays out of `scripts/check.sh` in this change —
  *because* ① left it an open question and putting it in CI is a decision about what
  CI should fail on, not about where the flow lives.

## Changes

- **Files:**
  - `.claude/skills/spec-flow/SKILL.md` (new) — copied from `~/.claude`, unmodified
  - `.claude/commands/spec-{new,investigate,propose,job,tests,review,deploy,board}.md` (new, 8)
  - `.claude/skills/vendor.json` (modified) — `authored: ["spec-flow"]`, with a comment
  - `scripts/vendor-skills.py` (modified) — read it, exclude from stray, refuse overlap, report both counts
  - `.claude/skills/README.md` (modified) — the two kinds; *Adding one* gains the authored case
  - `docs/specs/README.md` (modified) — *Driving it* now names `.claude/commands/`
- **Contract:** none. No route, no type, no generated client — `just openapi` is not
  involved.
- **Data / migrations:** none.

## Tasks

- [x] 1.1 Add `authored: ["spec-flow"]` to `.claude/skills/vendor.json` with a
      one-line comment saying what the list means — implements *Requirement: The
      manifest declares what this repository authored*
- [x] 1.2 Read `authored` in `vendor-skills.py`: subtract from the stray set, and
      print the pinned and authored counts apart in the `--check` summary — same requirement
- [x] 1.3 Refuse a name that appears in both `sources` and `authored`, in `vendor()`
      and in `--check`, naming the directory — *Scenario: a name cannot be both pinned
      and authored*
- [x] 2.1 Copy `~/.claude/skills/spec-flow/SKILL.md` into `.claude/skills/spec-flow/`
      — implements *Requirement: The flow ships with the repository*
- [x] 2.2 Copy the eight commands into `.claude/commands/` — same requirement
- [x] 3.1 Amend `.claude/skills/README.md`: the two kinds, which directories are
      authored, and what *Adding one* means for each — implements *Requirement: What
      `.claude/skills/` holds*
- [x] 3.2 Rewrite `docs/specs/README.md`'s *Driving it* section — implements
      *Requirement: How the flow is driven*
- [x] 4.1 Verify: `just skills-check` exits 0; `just skills` leaves the authored tree
      byte-identical (`git status` clean under `.claude/skills/spec-flow`); a scratch
      directory under `.claude/skills/` is still reported and still exits 1

## Log
- 2026-09-10 01:00 — job planned on `main`
- 2026-09-10 01:06 — all eight tasks landed. 1.2 grew one behaviour the spec did not have (a declared-but-absent authored skill); the scenario is in ② rather than only in the code.
