---
description: Phase ⑤ — review the diff against the spec, write 05-review.md with the checklist and the outcome, move the card to Review.
argument-hint: "[ID]"
---

# /spec-review `[ID]`

Phase ⑤ . Load **`spec-flow`** for the preamble and the shared snippets.

## What this phase produces
A recorded outcome. The **spec is the source of truth**: the review checks that the
code does what `02-spec.md` says, that the decisions in `03-job.md` were the ones
taken, and that nothing unspecified crept in. Unspecified behaviour is a finding even
when it is good — it means the spec is now wrong, and one of the two has to move.

## Steps
1. Preamble → project hook → resolve the spec → skim `02-spec.md`, `03-job.md`,
   `04-tests.md`.
2. Review the diff (`git diff $(git merge-base HEAD main)...HEAD`, or the PR). If the
   project hook names a review command or the session offers `/code-review`, run it
   and link what it found. Check the change against the repository's own guardrails —
   the hook says where they are written.
3. Write the note:

   ```bash
   cat > "$DIR/05-review.md" <<EOF
   ---
   id: $ID
   step: review
   status: doing
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/review, branch/$BTAG, status/doing]
   ---

   \`#spec/$ID\` · \`#step/review\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ⑤ Review — $ID

   **Reviewer:** <name>
   **PR:** <link>
   **Outcome:** <approved | changes-requested>

   ## Checklist
   - [ ] Every spec requirement is implemented; nothing unspecified was added
   - [ ] The decisions in \`03-job.md\` are the ones the code took
   - [ ] The repository's guardrails hold for the paths this change touched
   - [ ] Tests cover the spec scenarios and fail without the change
   - [ ] Generated contracts and clients regenerated and committed
   - [ ] No secrets; no backward-incompatible migration in one release
   - [ ] A decision worth keeping is queued for the decision record

   ## Findings
   - <finding> → <resolved how, or deferred and why>

   ## Log
   - $NOW — review recorded on \`$BRANCH\`
   EOF
   ```
4. Advance: `move_card Review` then `bump_index review doing 05-review "review recorded"`.
   On *changes-requested* use `bump_index review blocked 05-review "changes requested"`
   and stay on this column until they are answered.
5. Confirm, and suggest `/spec-deploy $ID` once approved.
