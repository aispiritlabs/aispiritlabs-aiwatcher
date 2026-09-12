# Prompt: a command panel that lands on the page with the filter applied

Copy everything below the line into another project. It is written to be read
by an agent or a developer who does **not** know this repository — it names the
decisions and the traps, not aiwatcher's files. Where it says *your project*,
substitute.

This was built here as `apps/panel/src/app/{commands.ts,command-panel.tsx}`;
that implementation is the reference if you want to read one.

---

## What to build

A ⌘K command panel (a "command palette") for a web application's UI. Typing a
phrase — `show traces for live agents`, `runs that failed`, `create a new
dataset` — takes the reader to the right page **with the right filters already
applied**, the way a CLI gets you somewhere in one line instead of four clicks.

Open it with ⌘K / Ctrl-K **and** a visible button in the header. A palette
nobody is told about is a palette nobody opens, and the button is where the
shortcut gets learnt — put the `⌘K` hint inside it.

## The precondition — check this first, before writing anything

**Every filter and view selection must already live in the URL** (query/search
params), not in component state.

That is what makes the whole feature possible: a command becomes a route plus a
search object, running one is an ordinary navigation, the back button undoes
it, and nothing in the palette has to drive a page's internals.

If your project keeps filters in component state, **stop and say so**. Moving
them into the URL is the real task and it is worth doing on its own merits — a
filtered view somebody can send to somebody else. Building a palette on top of
component state means the palette has to reach into pages and set their state,
which is a different, much worse feature. Do not start there.

## The five decisions that matter

**1. A command is `{ to, search }` and nothing else.**

```ts
interface Command {
  id: string;        // stable across label renames
  label: string;     // what is read and what is scored
  group: string;     // the area, drawn beside the label so two "Runs" differ
  hint?: string;     // one line: what the reader will be looking at
  keywords?: string[]; // words somebody types that are not in the label
  to: string;              // the route
  search?: Record<string, unknown>; // the filters that make it specific
}
```

Running one calls the router's `navigate`. Nothing mutates. Nothing here holds
state a reload would lose.

Send the **whole** search object, never a patch — including an empty one for a
command that names no filter. A command is a whole question; merging would
leave whichever filters the reader happened to be looking at applied to a
different one.

**2. Matched, never parsed. Do not put a model in the navigation path.**

Typing selects from a list you author, the way a shell completes a command.
Sending the text to an LLM to pick the route makes navigation a network call
that can be wrong, and "it went somewhere else this time" is a bad property for
the thing somebody presses to get unlost. It is also slower than the thing it
replaces.

Use **subsequence** matching, not substring: the query's characters appearing
in order, scored so that word-starts and adjacent runs win. That is what lets
`stfla` reach "Show traces for live agents" and makes it feel like a CLI rather
than a menu. Score the **label**; let `group` and `keywords` only *admit* a
command, at a discount — scoring them equally lets a command with a long
keyword list beat the one whose name is being typed.

**3. Derive "go to page X" from whatever already describes your navigation.**

If your project has a single description of its nav (a sidebar config, a route
manifest), generate one command per page from it. A hand-written second list is
free to disagree with the sidebar the day someone adds a page — and a palette
is exactly where that goes unnoticed.

**Author only the filtered commands by hand.** The cross product of every
parameter and every value is thousands of rows and almost none of them is a
question anybody has. Write the ones people actually arrive with: *what is
running now*, *what is waiting on me*, *what failed*, *what did this cost*.

**4. Every command must correspond to an affordance that really exists.**

If there is no "create a dataset" dialog, do not write a command that implies
one. Send it to the page where the thing is actually done and say so in the
`hint`. A command that navigates somewhere and leaves the reader to hunt is the
empty state with extra steps; one that promises an action the app cannot
perform is a plausible fake, which reads as working software and is worse than
its absence.

**5. Use a focus/interaction library for the modal, not a component kit.**

A centred palette needs no anchor, so this is not about positioning. What it
needs is: a **portal** (out of the header's stacking context), an **overlay
that locks background scroll**, a **focus trap that returns focus** to whatever
opened it, and **dismissal on Escape and outside press** that unbinds again on
close. Writing those four by hand is where dialogs go subtly wrong.

`@floating-ui/react` provides exactly those (`FloatingPortal`,
`FloatingOverlay`, `FloatingFocusManager`, `useDismiss`, `useRole`,
`useInteractions`) without deciding your whole widget vocabulary. Pulling in a
full component kit for one dialog is a bigger decision than it looks.

## The test that makes this safe — do not skip it

**Hold every command's `search` against the target route's own schema**
(zod, valibot, whatever declares your search params), and fail if a key is
dropped or its value comes back changed.

This is the one drift that is otherwise **invisible at runtime**: a schema
strips a parameter it does not declare, so the command navigates, the page
renders, and the filter silently does nothing. Nothing throws. Nobody notices
until someone trusts a filtered view that was never filtered.

It caught two real bugs here, both of which had passed code review and unit
tests:

- A `status` filter on a live-stream view that the view **cannot honour** —
  status was assembled from several events, so no single event carried it. The
  page said so on screen; the command set it anyway. The fix was to set *no*
  filter: that view is already the thing being asked for, and no filter is the
  honest answer rather than a missing one.
- A `status` filter on a view whose schema **had no such parameter at all**. It
  was stripped in silence. The fix was to point the command at the view that
  does declare it.

Also assert: every command id is unique; every `to` is a route that exists;
every page in the navigation has a command; and the schema map in the test
covers every page, so nothing is silently unchecked.

## Behaviour to get right

- **A fresh query every time it opens.** Reopening onto the last query makes
  the first keystroke a correction rather than a search.
- **The highlight resets when the query changes.** Narrowing to two rows must
  not leave the selection pointing at the seventh.
- **Pointer and keyboard share one highlight,** so Enter runs what the mouse is
  over. Set the active index on `pointermove`.
- **Arrow keys wrap;** support `Ctrl-N` / `Ctrl-P` too if your audience is
  terminal-shaped.
- **Enter with no matches does nothing** — it must not navigate to a stale
  highlight.
- **The empty state names what was typed.** `Nothing here matches "<query>"` —
  what somebody typed is the one thing that makes "nothing" readable.
- **Keep the active row in view** with `scrollIntoView({ block: 'nearest' })`,
  which scrolls the list and not the page.

## Accessibility

`role="listbox"` on the list and `role="option"` on the rows; `aria-selected`
on the active one; `aria-activedescendant` on the input pointing at the active
row's id, so the input keeps focus while the selection moves. Label the input.
Let the focus manager return focus on close.

## Where the code goes

The palette needs to know **both** the navigation **and** where each feature's
filters live. In a layered codebase that usually means it cannot live in a
shared/ui layer that is forbidden from importing features — put it in the
application/composition layer. Check your project's own boundary rules
(lint rule, dependency-cruiser, custom script) before choosing the directory;
if a check exists, it will tell you.

## Verify it in a browser before calling it done

Unit tests will not catch a filter the page ignores. Run the app, open the
panel, type each phrase, and confirm on screen that the page **applied** the
filter — not merely that the URL changed. Both bugs above were found this way
and only this way.

## Acceptance

- ⌘K and a header button both open it; Escape and outside-press close it.
- Typing a phrase from the examples lands on the right page with the filter
  visibly applied.
- An initialism (`stfla`) reaches the command whose words those are.
- Every page in the navigation is reachable without a hand-written entry.
- A command that sets a parameter its route does not declare **fails a test**.
- No model call in the navigation path.
