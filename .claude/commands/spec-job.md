---
description: Phase ③ — write 03-job.md, the technical approach, the decisions and the task checklist; move the card to Job.
argument-hint: "[ID]"
---

# /spec-job `[ID]`

Phase ③ . Load **`spec-flow`** for the preamble and the shared snippets.

## What this phase produces
The *how*: design decisions **with their rationale**, and a concrete task list. Each
task is about one sitting's work and names the spec requirement it implements. This
note is where implementation progress is tracked — tick tasks as they land.

## Steps
1. Preamble → project hook → resolve the spec → read `02-spec.md`.
2. Decide the approach against the repository's own rules (the hook names where they
   are). A decision that contradicts one of them is a decision to raise with the
   user, not one to make quietly.
3. Write the note:

   ```bash
   cat > "$DIR/03-job.md" <<EOF
   ---
   id: $ID
   step: job
   status: doing
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/job, branch/$BTAG, status/doing]
   ---

   \`#spec/$ID\` · \`#step/job\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ③ Job — $ID

   ## Technical approach
   <one paragraph: how this gets built>

   ## Decisions
   - **Decision:** <choice> — *because* <rationale>. Alternative rejected: <what, why>.

   ## Changes
   - **Files:** \`path/to/file\` (new|modified|deleted) — <what changes>
   - **Contract:** <routes, types, generated clients — and the command that regenerates them>
   - **Data / migrations:** <schema changes; additive first, removals a release later>

   ## Tasks
   - [ ] 1.1 <task> — implements *Requirement: <name>*
   - [ ] 1.2 <task>
   - [ ] 2.1 <task>

   ## Log
   - $NOW — job planned on \`$BRANCH\`
   EOF
   ```
4. Advance: `move_card Job` then `bump_index job doing 03-job "job planned"`.
5. Implement. As tasks land, tick their checkboxes in place and append to the log —
   the checklist is the progress record, so keep it honest rather than tidy.
6. When every task is ticked, suggest `/spec-tests $ID`.

## Notes
- A decision worth an ADR is one that would be expensive to reverse. Note it here,
  and let `/spec-deploy` graduate it to the repository's decision record — the spec
  folder is the record of this change, not of the decision.
