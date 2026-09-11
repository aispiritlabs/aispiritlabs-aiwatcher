---
id: AW-2
note: gui-review
created: 2026-09-11
tags: [spec/AW-2]
---

# AW-2 — `agentic_graph`'s GUI, reviewed against the panel

The spec puts the Gradio and PixiJS surface out of scope: *reviewed for
capability and not ported*. This is that review. Paths below are
`ai_spirit_agent/packages/agentic_graph/src/agentic_graph/` (`AG/`) and
`aiwatcher/apps/panel/src/` (`panel/`), read on 2026-09-11 — the panel as the
rebuild in progress leaves it, `features/` and `shared/` in place of
`components/` and `lib/`.

## Verdict

Nothing moves. Of the three capabilities the investigation said were worth
having, one does not exist in `agentic_graph`, one the panel already has, and
one — authoring an agent graph — is a real gap that belongs to a spec of its
own, not to a port:

- **The run overlay is not live.** The investigation read the Runner tab as
  lighting nodes as they execute. It lights *every* node `active` before a run
  and every node `done` or `error` after it (`AG/canvas.py:474-509`); nothing
  reaches the browser per node, and the dots along the edges move whether
  anything runs or not. The panel's Workflows view is the real version:
  per-node state from `step.*`, `Pending` for what a turn never reached, the
  hand-offs as `agent.message`, re-read on every frame of the SSE stream
  (`panel/features/workflows/`), and proven by `just e2e-agent-graph`.
- **The Events tab is covered.** It is `runtime.message_log` read after the run
  finished, filterable by agent and type (`AG/events_tab.py`). The panel's run
  detail and live feed are the same thing while it happens, filtered on the
  server.
- **A GUI run is already observable in aiwatcher.** The GUI calls
  `build_compiled_graph_system(graph, runtime_secrets=…)` and nothing else
  (`AG/runtime.py:163-166`), so the tracer and the declaration come from the
  process environment: with `AIWATCHER_URL` set, a turn started from the
  Gradio Runner is a declared execution in the panel's Workflows view. It never
  reaches the durable join — no `execution_id` is passed — which is right for a
  single-process preview.

## Capability by capability

| Capability | `agentic_graph` | The panel | Verdict |
|---|---|---|---|
| Author a graph: palette, drag, connect, entry node | 15 fixed blocks in four kinds, a PixiJS canvas, ports checked in the browser and again on the server (`AG/registry.py`, `AG/canvas.py`, `AG/builder.py:152-221`) | none for agents; the curation canvas is a linear chain of data blocks, and `TargetKind` has two arms, `curation_pipeline` and `workflow` | **Gap, own spec.** ADR_0024's rule applies: the panel draws, the server validates and refuses with every problem at once |
| Validate | on every change, `AG/builder.py:107-377` | a 422's `details`, rendered as they are | the server's rule, never a TypeScript copy |
| Save and load | "presets" as `graph.json` under `~/.aispiritagent/workspaces/<slug>/`, keys stripped | content-addressed definition revisions | a registered agent graph would be a `DefinitionRevision`; the preset folder stays the application's |
| Import, export, generate code | text boxes; a Python module calling the compiler (`AG/builder.py:379-483`) | — | the application's |
| Model and provider per node | free text on a provider node | — | configuration of a definition, not a screen |
| Run a turn, chat | a blocking run inside the Gradio process; "streaming" replays the finished answer one character at a time (`AG/runner_tab.py:150-164`) | `WorkflowLauncher` starts a registered workflow from raw JSON; no composer anywhere | **Gap, small.** Sending a turn to a hosted agent is `POST /api/v1/executions` against Phase E's one-step workflow |
| Live execution | not live (above) | Workflows view, per node, over SSE | **the panel's is the real one** |
| Events | after the run, `AG/events_tab.py` | run detail and live feed, server-filtered | covered |
| Traces, metrics | none — MLflow or aiwatcher | waterfall, metrics | covered |
| Prompts | none | the registry: versions, labels, verdicts, diff | covered, with one gap below |
| Sign-in, users | none: `0.0.0.0:7861`, no login (`AG/__init__.py:131`); a user picker nothing reads | OIDC or proxy, three roles | covered |

## What the panel lacks for an agent graph

Found by this review; neither side has them today. Ordered by value for cost,
and all of it panel work, which another session is rebuilding — these follow
that rebuild rather than race it.

1. **The prompt a turn ran on, in the run view.** The server writes
   `aiwatcher.prompt.name` and `aiwatcher.prompt.version_id` on every LLM span
   that carries a reference, and since this spec's prompt-version change every
   agent turn on a named prompt does. The waterfall reads one attribute,
   `aiwatcher.span.step_type` (`panel/features/observability/components/waterfall.tsx:51-56`),
   and has no attribute panel; the only way to the version is the event feed,
   which links it from the payload (`event-feed.tsx:97`). A span inspector
   with a `PromptRefLink` is small and closes the loop ADR_0011 exists for.
2. **A node that says what it was.** `NodeInspector` shows status, kind,
   attempts, duration, agents and a link to the *run*. Not the model, the
   prompt reference, the tokens, or the node's own span.
3. **Sending an agent a message.** Above: small, because Phase E made an agent
   a workflow; what is missing is a composer and a reply read from the payload
   store, never from the log.
4. **Authoring an agent graph.** Large: an arm or a `WorkflowSpec` shape for
   agent graphs, the builder's connection rules moved server-side, and the
   canvas the panel already has for pipelines generalised rather than a second
   one. A spec of its own when somebody needs graphs built by people who do not
   write Python.
5. **Gates for a graph that publishes itself.** `AnswerGate` renders only for a
   server-managed execution; a declared graph gets a 404 from the execution
   routes and shows none.

## What stays in `ai_spirit_agent`, and what is wrong with it

The GUI stays the application's, as the spec says. The review found these, for
whoever keeps it:

- **Exposed with no sign-in.** It listens on every interface
  (`AG/__init__.py:131`) and can write and `rmtree` under `~/.aispiritagent`.
  `server_name="127.0.0.1"` is the one-line fix.
- **PixiJS from a CDN at run time** (`AG/canvas.py:33`), with no integrity
  check and no offline fallback: the canvas is blank without the network.
- **The Runner's canvas is dead.** Both tabs render `build_canvas_html()`, so
  both carry `#agent-canvas-container`, and the canvas is created once
  (`AG/canvas.py:30-31`): loading a preset in the Runner draws into the
  Editor's, and the Editor's next save writes the Runner's graph.
- **A rename is lost.** The server's copy changes and the canvas's does not;
  validate, export, save and code generation all read the canvas first.
- **Smaller:** the Import button is wired to nothing (`AG/tab.py:410`); the
  Runner passes no session tokens, so only environment tokens work there; runs
  from the Editor never reach the Events tab; preset lists fill once per page
  load; `delete_workspace_action` imports `personal_assistant`, which the
  package does not depend on; the package README shows an API that does not
  exist.
- **Tested:** four tab tests with the runtime faked, and builder tests. None of
  the canvas's ~470 lines of JavaScript, the tab wiring, the Runner or `main()`.

Implementation stays behind for the investigation's reason: the panel's React
Flow canvases carry the shared visual language, and a second canvas technology
is a second place every graph decision has to be made.
