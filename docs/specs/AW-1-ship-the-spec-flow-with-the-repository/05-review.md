---
id: AW-1
step: review
status: done
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-1, step/review, branch/main, status/done]
---

`#spec/AW-1` · `#step/review` · `#branch/main` · repo `aiwatcher`

# ⑤ Review — AW-1

**Reviewer:** Claude, in the owner's session, 2026-09-11, at the owner's request
to close the spec. **Pull request:** none — the change is on `main`, inside
`43453ed` and `b0b5fb9`, not pushed.

## Artefacts

- [x] Every requirement in `02-spec.md` has a scenario, and every scenario a row
      in `04-tests.md`
- [x] The one scenario with no test says why, and what was checked in its place
- [x] The spec moved when the behaviour did — the declared-but-absent case — with
      a log line, rather than living only in the code
- [x] `03-job.md`'s eight tasks are ticked, and each names the requirement it
      implements

## Code

- [x] `authored_of` refuses a name that is both pinned and authored **before**
      any network call (`scripts/vendor-skills.py:48-55`), so the refusal cannot
      be masked by a fetch that fails first
- [x] `--check` expects an authored directory rather than reporting it, and
      reports an authored name with nothing behind it (`:173-181`); the vendoring
      path leaves authored skills alone (`:191`)
- [x] Backward compatible: `manifest.get("authored", [])`
- [x] The READMEs state the two kinds once each, and neither restates
      `CLAUDE.md`

## Process

- [ ] **The change has no commit of its own.** It landed inside two broad
      commits whose subjects name something else, so `git log -- .claude/` is
      the only way to find it. Not repairable without rewriting published
      history; worth not repeating.
- [ ] **`just skills-check` is in neither `just check` nor CI.** The spec left
      it out on purpose, as an open question, and it is still open: until it is
      in a gate, a directory nobody declared is caught only when somebody runs
      the recipe.
- [x] Verification recorded as it came out, including the red step it did not
      cause

## Findings

1. The global `~/.claude/skills/spec-flow` and the repository's copy can drift,
   and nothing compares them. The repository's copy is the one this repository's
   hook names, so for work here that one is the rule — which is what the spec
   asked for — but a machine that edits the global one will not notice.
2. The follow-ups above are decisions, not defects in this change: adding
   `skills-check` to `scripts/check.sh` is a one-line change with a cost (a
   network call the check path does not make today, only on vendoring — the
   check itself is offline), and it belongs to whoever next touches the gate.

## Outcome

**Approved.** The requirements are met and tested as far as a checkout can test
them; the one red step of `just check` is outside this change; the two process
items are recorded for next time rather than blocking this one.

## Log
- 2026-09-11 12:20 — reviewed and approved on `main`; two process items recorded
