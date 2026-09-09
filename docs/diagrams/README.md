# Diagrams

The typed JSON in this directory is the source; the HTML under `out/` is
generated from it and is not committed. Regenerate with:

```bash
just diagrams
```

Each file is rendered by [archify](https://github.com/tt-a1i/archify) (MIT),
installed as an agent skill at `~/.claude/skills/archify`. Nothing in the Cargo
workspace depends on it, and `just check` does not run it — a diagram going
stale is a documentation problem, not a build failure, and wiring it into CI
would make it the second one.

| Source | Answers |
|---|---|
| `aiwatcher-runtime.architecture.json` | What folds the event log, what executes work, and what is authored outside retention |
| `managed-execution.workflow.json` | How a canvas somebody edited becomes facts on the log — ADR_0025 and ADR_0026 |

## Editing one

A source is validated before it renders, and the validator checks geometry as
well as schema: crossings, label clearance, and whether the text is still
legible at 1440×900. It refuses more than it accepts at first, which is the
point.

```bash
A=~/.claude/skills/archify/bin/archify.mjs
node $A validate architecture aiwatcher-runtime.architecture.json --quality showcase --json
node $A deliver  architecture aiwatcher-runtime.architecture.json out/x.html --quality showcase --json
```

The `visualize` skill has the diagnostics translated into what they actually
mean and which lever fixes each one. Two are worth knowing here, because they
are the two that cost the most time:

- **A crossing in a `workflow` is usually a lane-order bug**, not a routing
  bug. Lanes that talk to each other belong next to each other.
- **`desktop-readability` has a different fix per type.** In an `architecture`
  a sublabel's font is fitted to its box, so shortening the text can fix it
  alone; in a `workflow` that font is fixed at 8px and the only lever is the
  viewBox — reduce every node's `width`.

## Evidence

`just diagrams` is the acceptance gate: 9/9 artifact checks and a clean
showcase composition, or it exits non-zero. Beyond that there is a browser
pass, which needs any Chromium — on this Mac, Edge:

```bash
export ARCHIFY_CHROME="/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
node ~/.claude/skills/archify/bin/archify.mjs visual-check docs/diagrams/out/<name>.html --json
```

It writes a receipt and a contact sheet beside the artifact and takes a minute
or two each. Both diagrams pass.

Containment is the check that bites, and the lever is rarely the diagram. A
page is header + chapters + drawing + cards, the drawing flexes to fill, and a
card row is as tall as its tallest card — so one bullet wrapping to a second
line costs sixteen pixels and can be the whole overflow. Measure before
rewriting anything:

```js
// in the artifact, at the target viewport, after document.fonts.ready
const li = document.querySelector('.cards li');
li.clientWidth;                       // the budget, bullet included
```

At 1440×900 that budget is 275px. The embedded viewer font makes it
deterministic; measuring it *before* the font settles gives a different and
wrong answer.

## What these are not

They are drawn from the ADRs and from this repository's own account of itself,
which means they are as current as somebody last made them. Nothing checks a
diagram against the code. Where one disagrees with a crate, the crate is right.
