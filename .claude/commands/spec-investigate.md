---
description: Phase ① — explore the problem and the code, write 01-investigation.md, move the card to Investigation.
argument-hint: "[ID]"
---

# /spec-investigate `[ID]`

Phase ① . Load **`spec-flow`** for the preamble and the **resolve / move_card /
bump_index** snippets. Without an ID, the spec is resolved from the current branch.

## What this phase produces
Understanding, not solutions. Actually read the code — *Current state* cites real
files and line numbers. Lay out options with trade-offs and a recommendation; the
decision itself belongs to the spec phase.

## Steps
1. Preamble → project hook → resolve the spec.
2. Investigate: the relevant modules, the repository's own guide and decision
   records (the hook names them), prior specs under `$SPEC_ROOT`, the git history of
   the files in question.
3. Write the note:

   ```bash
   cat > "$DIR/01-investigation.md" <<EOF
   ---
   id: $ID
   step: investigation
   status: doing
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/investigation, branch/$BTAG, status/doing]
   ---

   \`#spec/$ID\` · \`#step/investigation\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ① Investigation — $ID

   ## Problem statement
   <what problem is being solved, and why now>

   ## Current state
   - \`path/to/file.rs:120\` — <current behaviour / limitation>

   ## Constraints
   - <performance / compatibility / security / the repository's own rules that apply>

   ## Options
   ### Option A — <name>
   - Pros: <…>
   - Cons: <…>
   ### Option B — <name>
   - Pros: <…>
   - Cons: <…>

   ## Recommendation
   <which option, and why>

   ## Open questions
   - [ ] <what has to be settled before the spec>

   ## Log
   - $NOW — investigation written on \`$BRANCH\`
   EOF
   ```

   Replace every `<…>`. An option with no stated cost reads as one nobody considered.
4. Advance: `move_card Investigation` then
   `bump_index investigation doing 01-investigation "investigation written"`.
5. Confirm, list the open questions, and suggest `/spec-propose $ID` once they are
   settled.
