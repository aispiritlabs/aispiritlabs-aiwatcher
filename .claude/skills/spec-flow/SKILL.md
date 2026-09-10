---
name: spec-flow
type: reference
description: >
  Conventions and the shell contract for the repo-native spec-driven flow — every
  unit of work is a spec folder under `docs/specs/` that moves through investigation
  → spec → job → tests → review → deploy, tracked on one committed board. Auto-load
  this whenever a /spec-* command runs, or whenever the user talks about specs, the
  spec board, the phases of a change, or spec-driven development inside a git
  repository. Holds the folder layout, the frontmatter/tag schema, the
  resolve/move_card/bump_index snippets, and the project hook every repository
  writes once for itself.
---

# Spec flow

A spec-driven flow that lives **in the repository it is about**. Every unit of work
is a **spec folder**, and it moves through six phases, each its own file:

> **investigation → spec → job → tests → review → deploy**

One committed **board** (`docs/specs/BOARD.md`) tracks every spec across those
phases. Notes follow **OpenSpec** conventions: a proposal, a delta spec, RFC-2119
keywords, and `GIVEN/WHEN/THEN` scenarios a test can be derived from.

This is the sibling of the `manager-vault` skill — the same six phases, the same
note anatomy — with the Obsidian vault replaced by files under version control, and
the `obsidian` CLI replaced by `sed`, `grep` and `awk`. The difference is the point:
a spec that ships in the same commit as the code it describes is reviewed with it
and cannot drift from the branch it belongs to.

**Which one to use.** `manager-vault` when the work spans repositories, has no
repository at all, or belongs to a personal cross-project pipeline. `spec-flow` when
the work is one repository's, and the record should be reviewable in its pull
request.

---

## Where it lives

`<repo root>/docs/specs/`. The root comes from `git rev-parse --show-toplevel`;
`SPEC_FLOW_ROOT` overrides the folder for a repository that keeps its documentation
somewhere else.

## The project hook — read it first

`docs/specs/README.md` is written **once, by each repository**, and it is the only
place repo-specific facts live: the ID prefix, the command that verifies a change,
and where a decision that outlives the ticket graduates to. Read it before any
phase:

```bash
sed -n '/^## Project hook/,$p' "$SPEC_ROOT/README.md"
```

It **points at** the repository's own guide (`CLAUDE.md`, `docs/ADR/`, a `justfile`
or `Makefile`) and never restates it. A hook that copies rules out of those files is
a second copy free to disagree with the first. If the hook is missing, `/spec-board
init` writes a starting one and asks for the two facts it cannot guess.

## Layout

```
docs/specs/
├── README.md              # what this folder is + the project hook
├── BOARD.md               # THE board — one column per phase, one card per spec
└── <ID>-<slug>/
    ├── _index.md          # overview: status, branch, phase links, rolled-up log
    ├── 01-investigation.md
    ├── 02-spec.md         # OpenSpec proposal + delta spec
    ├── 03-job.md          # design decisions + task checklist
    ├── 04-tests.md        # verification matrix
    ├── 05-review.md       # review checklist + outcome
    └── 06-deploy.md       # ship checklist + archive
```

---

## Shared preamble

Every `/spec-*` command starts here. Shell state does not persist between tool
calls, so paste this into each Bash block that touches the flow:

```bash
REPO=$(git rev-parse --show-toplevel 2>/dev/null || pwd); REPO_NAME=$(basename "$REPO")
SPEC_ROOT="${SPEC_FLOW_ROOT:-$REPO/docs/specs}"
BOARD="$SPEC_ROOT/BOARD.md"
BRANCH=$(git -C "$REPO" branch --show-current 2>/dev/null || echo '-')
BTAG=$(printf '%s' "$BRANCH" | tr '/' '-')       # feature/x -> feature-x
NOW=$(date '+%Y-%m-%d %H:%M'); TODAY=$(date '+%Y-%m-%d')
```

## Resolve the spec

From an explicit ID, else from the branch the work is happening on:

```bash
ID="$1"                                          # may be empty
if [ -n "$ID" ]; then
  DIR=$(find "$SPEC_ROOT" -maxdepth 1 -type d -name "$ID-*" | head -1)
else
  HIT=$(grep -rl "^branch: $BRANCH\$" --include=_index.md "$SPEC_ROOT" 2>/dev/null | head -1)
  DIR=${HIT:+$(dirname "$HIT")}
fi
[ -n "$DIR" ] && [ -d "$DIR" ] || { echo "No spec for '${ID:-$BRANCH}' — run /spec-new first."; exit 1; }
ID=$(sed -n 's/^id: *//p'    "$DIR/_index.md" | head -1)
TITLE=$(sed -n 's/^title: *//p' "$DIR/_index.md" | head -1)
SLUG=$(basename "$DIR"); SLUG=${SLUG#"$ID-"}
```

If several `_index.md` name the same branch, ask the user which spec — never pick
one silently.

## Frontmatter & tag schema

Frontmatter is written once at creation; `step`, `status`, `updated` and the `tags:`
line are the only fields later phases rewrite.

```yaml
---
id: AW-12
title: Managed run retention window
step: spec                       # investigation|spec|job|tests|review|deploy
status: doing                    # todo|doing|blocked|done
branch: feature/retention
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-12, step/spec, branch/feature-retention, status/doing]
---
```

The first body line of a **phase note** repeats the trio as inline tags, so it
survives a grep that never reads frontmatter:

```markdown
`#spec/AW-12` · `#step/spec` · `#branch/feature-retention` · repo `aiwatcher`
```

`_index.md`'s inline line carries **no** `#step/` tag — the index's step moves with
every phase, and a tag that has to be rewritten in two places is a tag that ends up
disagreeing with itself. The moving fields live in frontmatter only.

Taxonomy: `#spec/<ID>` · `#step/<phase>` · `#branch/<sanitized>` ·
`#status/<todo|doing|blocked|done>`. They exist so that
`grep -rl "step/review" docs/specs` answers "what is waiting on a review" with no
tool at all.

## Note anatomy

Every note is a short, phase-specific body plus a trailing `## Log`. Log lines are
**appended**, never rewritten:

```bash
printf -- '- %s — %s\n' "$NOW" "spec drafted on \`$BRANCH\`" >> "$DIR/02-spec.md"
```

---

## The board

`docs/specs/BOARD.md` — plain markdown, one `##` heading per column, so it renders
on GitHub, in an editor and in Obsidian's Kanban plugin alike:

```markdown
# Spec board

## Backlog

## Investigation

## Spec

## Job

## Tests

## Review

## Deploy

## Done
```

A **card** links the spec's index, relative to the board:

```markdown
- [ ] [AW-12 · Managed run retention window](AW-12-managed-run-retention-window/_index.md)
```

`- [x]` once it lands in **Done**.

**Move a card** — remove the line wherever it is, insert it under the target
heading:

```bash
move_card() {                                    # $1 = column, $2 = " " or "x"
  local slug_dir; slug_dir=$(basename "$DIR")
  local card="- [${2:- }] [$ID · $TITLE]($slug_dir/_index.md)"
  grep -vF "]($slug_dir/_index.md)" "$BOARD" \
    | awk -v c="$card" -v h="## $1" '$0==h{print; print ""; print c; next} {print}' \
    | awk '/^$/{blank=1; next} {if (NR>1 && (blank || /^## /)) print ""; blank=0; print}' > "$BOARD.tmp"
  mv "$BOARD.tmp" "$BOARD"
}
```

The second `awk` is not tidiness. `grep -vF` takes the card's line out of the column
it was in and leaves the blank line that was under it, so without a pass that
squeezes blank runs to one and guarantees a single blank before every heading, a
card that has crossed six columns leaves a growing gap in each of them.

**Bump the index** — rewrite the moving fields, tick the phase's checkbox, append a
log line (`$1=step $2=status $3=NN-file $4=message`):

```bash
bump_index() {
  local idx="$DIR/_index.md"
  sed -E -e "s/^step: .*/step: $1/" \
         -e "s/^status: .*/status: $2/" \
         -e "s/^updated: .*/updated: $TODAY/" \
         -e "s#^tags: .*#tags: [spec/$ID, step/$1, branch/$BTAG, status/$2]#" \
         -e "s#^- \[ \] (\[[^]]*\]\($3\.md\))#- [x] \1#" "$idx" > "$idx.tmp"
  mv "$idx.tmp" "$idx"
  printf -- '- %s — %s\n' "$NOW" "$4" >> "$idx"
}
```

A phase's advance step is then two lines: `move_card Spec` and
`bump_index spec doing 02-spec "spec drafted on \`$BRANCH\`"`. `/spec-deploy` ends
with `move_card Done x` and `bump_index deploy done 06-deploy "shipped"`.

Write with a temporary file and `mv`, never `sed -i` — BSD and GNU `sed` disagree
about `-i`'s argument, and a flow that only works on one of them is a flow that
breaks on somebody else's machine. For the same reason no snippet globs a path that
may match nothing: `bash` passes the pattern through and `zsh` fails the command, so
enumeration is `find` or `grep -r --include`, never `"$SPEC_ROOT"/*/_index.md`.

**Board sync** (authoritative rebuild, repairs drift): every `_index.md` carries a
`<!-- spec-card -->` marker. Enumerate with
`grep -rl 'spec-card' --include=_index.md "$SPEC_ROOT"`, read each one's `id`, `title`,
`step` and `status`, and place its card in **Done** if `status: done`, else in the
column matching `step`. A spec with no phase note yet stays in **Backlog**.

## Phase → file → column

| Phase | File | Column | Command |
|---|---|---|---|
| investigation | `01-investigation.md` | Investigation | `/spec-investigate` |
| spec | `02-spec.md` | Spec | `/spec-propose` |
| job | `03-job.md` | Job | `/spec-job` |
| tests | `04-tests.md` | Tests | `/spec-tests` |
| review | `05-review.md` | Review | `/spec-review` |
| deploy | `06-deploy.md` | Deploy → Done | `/spec-deploy` |

The phase is called *spec* and its command is `/spec-propose`, because `/spec-spec`
reads as a typo. The file keeps the phase's name.

## The command family

| Command | Does | Board |
|---|---|---|
| `/spec-new [ID] <title>` | folder + `_index.md` + card | → Backlog |
| `/spec-investigate [ID]` | `01-investigation.md` | → Investigation |
| `/spec-propose [ID]` | `02-spec.md` (OpenSpec) | → Spec |
| `/spec-job [ID]` | `03-job.md` (design + tasks) | → Job |
| `/spec-tests [ID]` | `04-tests.md` (verification) | → Tests |
| `/spec-review [ID]` | `05-review.md` (review) | → Review |
| `/spec-deploy [ID]` | `06-deploy.md` (ship + archive) | → Deploy → Done |
| `/spec-board [init\|sync\|show]` | scaffold, rebuild, print | rebuild |

Each phase command: paste the preamble → read the project hook → resolve the spec →
read the phase before it → write its note from the embedded template, filled with
real content → advance the board and the index.

---

## Rules

- **One spec is one folder**, and a phase file is written by its own command and by
  nothing else. Re-running a phase overwrites its own note and nothing beside it.
- **Never rewrite a `## Log`.** Append. The log is what says when something was
  decided, and a rewritten one cannot.
- **Never fabricate a result.** `04-tests.md` records what the project hook's
  verification command actually printed. A scenario that cannot pass as written is
  an entry under *Issues* and a decision to make — never a deleted row.
- **Every requirement gets a scenario** a test can be derived from. A requirement
  with no scenario is a sentence nobody can check.
- **A decision that outlives the change graduates.** The spec folder is the record
  of *this* change; the ADR (or whatever the project hook names) is the record of the
  *decision*. Move the reasoning there and link it — two copies are free to
  disagree, and the day they do, nobody knows which is current.
- **The artefacts are committed with the code they describe.** A spec folder on a
  branch nobody merged is a proposal that was not taken, which is worth keeping.
- **The flow does not touch git history.** It records which branch the work happens
  on; it never creates a branch, a commit, a tag or a PR unless the user asks.
- **The hook names the commands; the phases run them.** `/spec-tests` runs whatever
  the hook calls verification, and reports its real output. It does not invent a
  test command from the file tree.
