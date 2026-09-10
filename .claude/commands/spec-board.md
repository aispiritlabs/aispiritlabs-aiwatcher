---
description: Manage the spec board — init scaffolds docs/specs and the project hook, sync rebuilds every card from the indexes, show prints it.
argument-hint: "[init|sync|show]"
---

# /spec-board `[init|sync|show]`

Manages the one board that tracks every spec in this repository. Load **`spec-flow`**
for the layout and the card format. Sub-command from the argument; default `show`.

Always start with the shared preamble.

## init — scaffold the folder, the board and the project hook

Idempotent, and it **never** overwrites a board that already has cards.

1. `mkdir -p "$SPEC_ROOT"`.
2. If `$BOARD` does not exist, write the eight columns:

   ```bash
   cat > "$BOARD" <<'EOF'
   # Spec board

   Every spec in this repository, one card per folder. `/spec-board sync` rebuilds it
   from the indexes; the phase commands move one card each.

   ## Backlog

   ## Investigation

   ## Spec

   ## Job

   ## Tests

   ## Review

   ## Deploy

   ## Done
   EOF
   ```

3. If `$SPEC_ROOT/README.md` does not exist, write it: a short paragraph on what the
   folder is, then a `## Project hook` section holding the facts the flow cannot
   guess. Ask the user for anything the repository does not answer plainly:

   - **ID prefix** — two to five capitals, e.g. `AW`.
   - **Verification** — the command that says a change is good (`just check`,
     `make test`, `npm test`), plus the extra gates that only apply to part of the tree.
   - **Decision record** — where a decision that outlives the change graduates to
     (`docs/ADR/` and its template), or *none* for a repository that keeps none.
   - **Guardrails** — the file a review reads before it approves (`CLAUDE.md`,
     `CONTRIBUTING.md`).
   - **Contract regeneration** — the command that regenerates anything generated,
     if the repository has one.

   The hook **points at** those files; it never restates them.

4. Report what it created and what was already there.

## sync — rebuild every card from the indexes

Authoritative repair; do not hand-move lines here.

1. Enumerate: `grep -rl 'spec-card' --include=_index.md "$SPEC_ROOT"`.
2. For each, read `id`, `title`, `step`, `status` from the frontmatter.
3. Bucket: **Done** when `status: done`, otherwise the capitalised `step`. A spec
   whose only file is `_index.md` stays in **Backlog**.
4. Rewrite `$BOARD` whole, keeping the heading and the intro paragraph, with cards
   under their columns — `- [x]` in Done, `- [ ] ` everywhere else.
5. Report what moved ("AW-12 Spec→Job, AW-7 Review→Done") rather than printing the
   whole board.

## show — print it

`cat "$BOARD"`, then summarise: which specs sit where, and which are `blocked` (read
`status:` from each index — the board's columns say the phase, not the health).
