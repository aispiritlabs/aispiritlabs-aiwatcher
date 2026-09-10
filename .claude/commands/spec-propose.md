---
description: Phase ② — write 02-spec.md as an OpenSpec proposal plus a delta spec, move the card to Spec.
argument-hint: "[ID]"
---

# /spec-propose `[ID]`

Phase ② — the *spec* phase. Load **`spec-flow`** for the preamble and the shared
snippets.

## What this phase produces
A behaviour contract, not an implementation plan. The *how* belongs to `/spec-job`.

- **RFC-2119** keywords: MUST/SHALL (required), SHOULD (recommended), MAY (optional).
- Every requirement carries at least one **Scenario** — a happy path, plus the edge
  or error case where it matters — written so a test can be derived from it verbatim.
- Use the **delta** sections: ADDED for new behaviour, MODIFIED (with `Previously:`)
  for changed behaviour, REMOVED (with `Reason:`) for deletions. Brownfield-friendly:
  the spec is the change, not a restatement of the whole system.

## Steps
1. Preamble → project hook → resolve the spec → read `01-investigation.md`.
2. Draft the proposal and the delta with the user. Settle the open questions the
   investigation left; if one cannot be settled, it is out of scope, and say so.
3. Write the note:

   ```bash
   cat > "$DIR/02-spec.md" <<EOF
   ---
   id: $ID
   step: spec
   status: doing
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/spec, branch/$BTAG, status/doing]
   ---

   \`#spec/$ID\` · \`#step/spec\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # ② Spec — $ID

   ## Proposal
   **Intent:** <the value this delivers>
   **In scope:** <…>
   **Out of scope:** <…>
   **Approach:** <one paragraph of direction>
   **Success criteria:**
   - [ ] <observable outcome>

   ## Spec (delta)
   ### ADDED Requirements
   #### Requirement: <name>
   The system SHALL <behaviour>.
   ##### Scenario: <happy path>
   - GIVEN <precondition>
   - WHEN <action>
   - THEN <expected outcome>
   ##### Scenario: <edge / error case>
   - GIVEN <…>
   - WHEN <…>
   - THEN <…>

   ### MODIFIED Requirements
   #### Requirement: <name>
   The system SHALL <new behaviour>. (Previously: <old behaviour>)

   ### REMOVED Requirements
   #### Requirement: <name>
   (Reason: <why>)

   ## Log
   - $NOW — spec drafted on \`$BRANCH\`
   EOF
   ```

   Drop MODIFIED/REMOVED when the change is purely additive — empty headings read as
   sections somebody forgot to fill in.
4. Advance: `move_card Spec` then `bump_index spec doing 02-spec "spec drafted"`.
5. Confirm, count the scenarios (that is what `/spec-tests` will have to match), and
   suggest `/spec-job $ID`.
