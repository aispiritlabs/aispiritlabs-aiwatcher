---
description: Phase ④ — run the project's verification, write 04-tests.md as a matrix of spec scenario → test → result, move the card to Tests.
argument-hint: "[ID]"
---

# /spec-tests `[ID]`

Phase ④ . Load **`spec-flow`** for the preamble and the shared snippets.

## What this phase produces
Evidence that the implementation satisfies the spec — one row per **Scenario** in
`02-spec.md`, naming the test that covers it and what it actually printed.

**Never fabricate a result.** Run the command the project hook calls verification and
record its real output. A scenario that cannot pass as written is an entry under
*Issues* and a decision — fix the code, or revise the spec and say why — never a
deleted row.

## Steps
1. Preamble → project hook → resolve the spec → read `02-spec.md` for the scenarios.
2. Run what the hook names: the whole-suite command, and the extra gates it lists for
   the parts of the tree this change touched. Capture the output.
3. Write the note:

   ```bash
   cat > "$DIR/04-tests.md" <<EOF
   ---
   id: $ID
   step: tests
   status: doing
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/tests, branch/$BTAG, status/doing]
   ---

   \`#spec/$ID\` · \`#step/tests\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ④ Tests — $ID

   ## Verification matrix
   | Spec scenario | Test | Result |
   |---|---|---|
   | <Requirement › Scenario> | \`<test name / path>\` | ✅ / ❌ |

   ## Commands run
   \`\`\`
   <command> — <summary of what it printed>
   \`\`\`

   ## Issues found & resolutions
   - <issue> → <fix, or spec revision and the reason for it>

   ## Quality checks
   - [ ] The project's verification command passes
   - [ ] Every spec scenario has a test that fails without the change
   - [ ] No backward-incompatible migration in a single release

   ## Log
   - $NOW — verification run on \`$BRANCH\`
   EOF
   ```
4. Advance: `move_card Tests` then `bump_index tests doing 04-tests "verification run"`.
   If anything is red, use `status: blocked` — `bump_index tests blocked 04-tests
   "verification red: <what>"` — and stay on this column.
5. Report pass/fail to the user with the real output, and suggest `/spec-review $ID`
   when it is green.
