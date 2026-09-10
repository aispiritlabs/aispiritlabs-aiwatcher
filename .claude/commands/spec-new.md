---
description: Open a new spec folder under docs/specs/ — index note, phase links, board card in Backlog.
argument-hint: [ID] <title>
---

# /spec-new `[ID] <title>`

Scaffolds a spec. Load the **`spec-flow`** skill for the layout, the schema and the
shared snippets. The ID is optional: `/spec-new "Managed run retention window"`
allocates the next free one from the project hook's prefix.

## Steps

1. **Preamble + hook** (one Bash block — paste the shared preamble from `spec-flow`),
   then read `sed -n '/^## Project hook/,$p' "$SPEC_ROOT/README.md"`. If
   `$SPEC_ROOT/README.md` or `$BOARD` is missing, run `/spec-board init` first.

2. **Resolve the ID.** If `$ARGUMENTS` starts with something matching
   `^[A-Z]{2,5}-[0-9]+$`, that is the ID and the rest is the title. Otherwise allocate:

   ```bash
   PREFIX=$(sed -n 's/^- \*\*ID prefix:\*\* *//p' "$SPEC_ROOT/README.md" | head -1)
   PREFIX=${PREFIX:-$(printf '%s' "$REPO_NAME" | tr -cd '[:alnum:]' | cut -c1-2 | tr '[:lower:]' '[:upper:]')}
   N=$(find "$SPEC_ROOT" -maxdepth 1 -type d -name "$PREFIX-*" \
         | sed -E "s#.*/$PREFIX-([0-9]+)-.*#\1#" | sort -n | tail -1)
   ID="$PREFIX-$(( ${N:-0} + 1 ))"
   TITLE="Managed run retention window"          # <- from the command args
   SLUG=$(printf '%s' "$TITLE" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-//;s/-$//')
   DIR="$SPEC_ROOT/$ID-$SLUG"
   ```

3. **Guard against duplicates.** If `$DIR` exists, or a folder for that ID already
   does, stop and say so — never overwrite an existing spec.

4. **Write `_index.md`** (the `<!-- spec-card -->` marker is what board sync
   enumerates; the inline tag line deliberately carries no `#step/`):

   ```bash
   mkdir -p "$DIR"
   cat > "$DIR/_index.md" <<EOF
   ---
   id: $ID
   title: $TITLE
   step: investigation
   status: todo
   branch: $BRANCH
   repo: $REPO_NAME
   created: $TODAY
   updated: $TODAY
   tags: [spec/$ID, step/investigation, branch/$BTAG, status/todo]
   ---
   <!-- spec-card -->

   \`#spec/$ID\` · \`#branch/$BTAG\` · repo \`$REPO_NAME\`

   # $ID — $TITLE

   > Branch \`$BRANCH\`

   ## Phases
   - [ ] [① Investigation](01-investigation.md)
   - [ ] [② Spec](02-spec.md)
   - [ ] [③ Job](03-job.md)
   - [ ] [④ Tests](04-tests.md)
   - [ ] [⑤ Review](05-review.md)
   - [ ] [⑥ Deploy](06-deploy.md)

   ## Summary
   <one or two lines: what this delivers, and why>

   ## Log
   - $NOW — spec opened on \`$BRANCH\`
   EOF
   ```

   Replace the `<…>` summary with a real line if the user gave any context.

5. **Add the Backlog card:**

   ```bash
   awk -v c="- [ ] [$ID · $TITLE]($ID-$SLUG/_index.md)" \
     '{print} /^## Backlog$/{print ""; print c}' "$BOARD" > "$BOARD.tmp" && mv "$BOARD.tmp" "$BOARD"
   ```

6. **Confirm**: the folder, the card, and the next step — `/spec-investigate $ID`.
   Mention the branch it recorded; if the user is on `main`, say so, because the
   flow records the branch and does not create one.

## Notes
- One spec, one folder. Phase notes are written later by their own commands.
- `_index.md` is the card's target and the rolled-up log for the whole change.
