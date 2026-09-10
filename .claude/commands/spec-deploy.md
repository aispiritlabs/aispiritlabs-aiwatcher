---
description: Phase ⑥ — write 06-deploy.md, graduate the lasting decisions, archive into the index, move the card to Done.
argument-hint: "[ID]"
---

# /spec-deploy `[ID]`

Phase ⑥ , the last one. Load **`spec-flow`** for the preamble and the shared snippets.

## What this phase produces
A ship record, and the **archive**: what the delta actually delivered, folded into
`_index.md` so the index reads as the durable summary; the spec marked `done`; the
card in **Done**.

This is also where a decision **graduates**. If `03-job.md` holds a decision that
would be expensive to reverse, write it into the repository's decision record — the
project hook names the folder and the template — and link it from here. Do not copy
the reasoning into both: the decision record is the copy that lasts.

## Steps
1. Preamble → project hook → resolve the spec.
2. Ship it the way the repository does (the hook names the release path). Record what
   actually happened, including what did not.
3. Write the note:

   ```bash
   cat > "$DIR/06-deploy.md" <<EOF
   ---
   id: $ID
   step: deploy
   status: done
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/deploy, branch/$BTAG, status/done]
   ---

   \`#spec/$ID\` · \`#step/deploy\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ⑥ Deploy — $ID

   ## Shipped
   - [ ] Merged: <PR / commit>
   - [ ] Released: <tag / environment>
   - [ ] Rollout / flag: <state, if any>

   ## Post-deploy checks
   - [ ] Behaviour matches the spec where it now runs
   - [ ] No regression in the signals this change could move

   ## Graduated decisions
   - <decision> → <link to the decision record>, or *none — nothing here outlives the change*

   ## Archive
   - ADDED: <requirements now live>
   - MODIFIED: <…>
   - REMOVED: <…>

   ## Log
   - $NOW — shipped from \`$BRANCH\`
   EOF
   ```
4. **Archive into the index** — append the summary so `_index.md` answers "what did
   this change do" without opening six files:

   ```bash
   printf -- '\n## Shipped\n- %s — see [⑥ Deploy](06-deploy.md)\n' "$TODAY" >> "$DIR/_index.md"
   ```
5. Advance: `move_card Done x` then `bump_index deploy done 06-deploy "shipped"`.
6. Confirm the spec is closed, name the graduated decisions, and offer
   `/spec-board sync` if anything looks out of place.
