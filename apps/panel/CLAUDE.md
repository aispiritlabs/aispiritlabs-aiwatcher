# The panel

How this panel is built, and what the browser may not decide. Vite, TanStack
Router and Query, Tailwind; `cd apps/panel && npm run build` is a vite build
followed by a full `tsc` project check.

## Conventions

- `apps/panel/src/api/generated` is generated. Never edit it by hand. It and
  `routeTree.gen.ts` are in `.prettierignore`: their generators emit their own
  formatting, and `just fmt` reformatting them would fight `just openapi`.
- `eslint.config.js` holds which rules this panel keeps, and every rule that is
  off is off *in writing*, with what it fires on. A rule with standing
  violations would never report the next one, so a new rule arrives with its
  call sites read rather than with a warning count. Formatting is not lint's
  business: `.prettierrc` is the authority and `just fmt` is what moves it.
- Runtime validation belongs only where codegen cannot reach — the SSE and
  WebSocket frames, in `src/shared/lib/live.ts`. Everything the generated SDK
  returns is already typed.
- Filters live in the URL, not in component state, so a link to a filtered view
  lands the reader on the same view. That includes the search boxes: the input
  holds a draft, a 250 ms debounce commits it to the search params.
- **One filter, every table and every chart** (`src/shared/lib/object-filter.ts`).
  The axes are the dimensions' own names plus the run's status — `agent`,
  `runtime`, `workflow`, `session`, `variant`, `trace`, `model`, `tool`,
  `prompt`, `status` — and translating one into a route's parameter happens
  there and nowhere else. Five rules carry it. A **filter selects runs**: every
  axis names a property a run has, directly or through its spans, which is how
  `RunFilter` already matches model, tool and prompt. A **number is counted
  over the selected runs**, and narrowed further only where the axis is about
  the thing being counted *and the read counts something smaller than a run*.
  Only `/metrics` does: with a model chosen its LLM calls and tokens are that
  model's while its tool calls are every tool call in those runs, because a tool
  call has no model. `/runs` and `/dimensions/{kind}` count nothing below the
  run — every figure on a row is a total folded when the run was ingested, the
  same number whatever the filter says — so there a model axis selects and
  narrows nothing, and the page says *that*. `queryFor` returns whichever
  sentence is true beside the request it describes, so a page cannot keep saying
  one after the route stops doing it; the span list says neither, because its
  rows are the calls the chips already name. A **view that cannot apply an axis
  names it** — the Live view's rule, now everybody's, and the three reasons are
  that the route has no parameter (only the span list still has any: five axes
  that are properties of a run rather than of a span), that the word means
  something else here (a span's `ok | error` is not a run's status) or that more
  values were chosen than the route takes; an axis is then *not sent*, because a
  narrower answer than the chips claim is the one failure a filter must not
  have. And it lives in the **URL**, so it survives a move between an area's
  views the way the window does. A bare value reads as a list of one, which is
  what keeps `?status=failed` working.
- `agents` is an area rather than a pivot, and the reason is worth keeping: an
  agent is **not an authored object** — no registry, no version, nothing
  outliving retention — so its page never 404s on a name, it says the period
  holds nothing under it. What separates the two reads it makes is the whole
  design: `by_agent` in the metrics response is keyed by the agent on each
  *span*, so it is this agent's own calls, tokens, cost and latency, while
  `by_model` and `by_tool` are folded over every span of the runs it took part
  in — a run where it hands work to another agent counts that agent's models
  there, and the cards say so. Which prompts an agent runs on is a third read,
  and the card that used to say it was **not answerable** is why the `prompt`
  dimension exists: the answer was in the spans and no fold grouped by it, so
  the only way to the list was paging every run and counting versions in the
  browser. It is the name and never the version — the registry is keyed by name
  and a version is what a prompt's own page lists — and the name is linked only
  where the registry confirms it holds one, because a span's prompt name is
  retained telemetry while the text is authored and outlives it, so the two can
  disagree in either direction.
- Routes are grouped into peer work areas — Data, Models & quality,
  Applications, Workflows and Learning — described once in
  `src/app/navigation.ts`. The desktop sidebar exposes all areas; mobile
  navigation exposes the same groups and their pages. `/` renders a neutral work
  overview. Existing object URLs and area redirects remain valid. `/account`
  exposes the current identity and read-only identity-provider groups from the
  global header. These groups are not application teams and do not imply
  project-level authorization. Active areas come from the pathname. An area
  layout route owns only shared state, such as the observability stream.
  `NavArea.carries` retains the period in Observability and the annotation
  project in Annotations.
- `observability/live` is the one view that reads the log rather than a fold of
  it. Its filter is applied by the server — `Scope::Selection` and the repeated
  `?agent=&runtime=&workflow=&session=` parameters — because `llm.chunk` is most
  of the log by volume and narrowing in the browser would mean receiving all of
  it to discard it. What it *cannot* filter on is model and tool: those are
  span-level facts assembled from several events (ADR_0003), so the page says
  which parts of a selection it is not following rather than going quiet. Its
  feed is bounded and Pause freezes the rendering, never the subscription.
- The Query view has a **Build** mode that compiles clicked attributes into the
  deployed engine's language — Flow, DataFusion or DuckDB — and the move to
  **Write** is one-way. `query-builder.ts` generates; the engine decides
  (`services/query/flow/src/Dsl` for Flow, admission for a Python engine).
  Parsing text back into chips would be a second grammar in TypeScript for
  languages whose real ones live in the engines, and the day they disagreed
  opening a hand-written query in the builder would silently rewrite it.
- `training` is the one area that reads nothing folded from the log at all. It
  polls while a run is `running` and stops when none is — an epoch is minutes,
  so five seconds costs one request and answers the same question a live channel
  would. It draws no progress bar: nothing knows how many epochs a run intends,
  and a bar that guesses is a bar that lies.
- `prompts` is the one area that reads something other than the log, and the one
  that writes. It answers 501 rather than 404 when no store is configured, and
  `RegistryDisabled` says which variable is unset — an empty list would be a
  different problem with a different fix. `datasets` and `annotations` share
  that store and that component, because one setting decides all three.
- `alerts` is the one area that speaks first. Everything else here waits to be
  opened; this is what a deployment says out loud. Three things on it are the
  server's and are not worked out in the browser: whether a notification arrived
  is the delivery's own state and its attempt count rather than an age or a
  spinner, what a receiver saw is the payload stored beside it rather than a
  payload rebuilt from the rule, and what happens next to a failed one is the
  row the retry route answers with. Two absences are sentences and not failures:
  a 403 is a role, because a rule decides who is woken, and a 501 is a
  deployment with no object store — the same one Prompts needs, named. A channel
  nobody configured is a working state and says so, because rules are still kept
  and regressions are still found; what does not happen is anybody being told.
  It publishes no rule: a rule's trigger is a `GatePolicy` and a context id, and
  a form that composed one in the browser would be a second opinion about what a
  gate holds a result to.
- `datasets` is the one area that reads a service aiwatcher does not run. Its
  Discover view searches Kaggle and Hugging Face, and renders the mirror's
  licence claim and aiwatcher's verdict as two separate things — never one
  badge, and the rights selector is never pre-filled from a hub's word. A hub
  nobody configured renders the 501 with its variable, not an empty list.
- `conversations` is the one area that shows content, and the one where a role
  decides whether it is shown at all. Its list decrypts nothing — every badge,
  count and finding comes from the plaintext head — and a turn's words are
  fetched one at a time by an explicit click, which the API answers only for an
  `admin`. A 403 there renders as "reading content needs the admin role" rather
  than as a failure, because it is not one. It draws a progress bar for a
  running export and Training draws none, and the difference is honest: an
  export's denominator is a conversation list that was pinned when the job was
  created, while nothing knows how many epochs a run intends.
- `annotations` gained a fourth view, **Imports**, and it is the one screen here
  where the refusals come before the successes. An import of six hundred
  thousand pictures that registered four hundred thousand looks, from a success
  response, exactly like one that worked; the counts by reason and the rows
  behind them are the whole story. It draws a progress bar and Training does
  not, for the same reason Conversations does: the pages were counted when the
  batch was sealed, so the denominator is a fact.
- `data-curation` is the one area that orchestrates. Its **Pipeline** view runs
  a chain across three systems from the browser — Flow PHP, the notebook
  runtime, the Rust registry — and reports each block as it goes, including the
  three that light up together because Flow executes them as one query. Both
  engines are optional and their absence is a badge, not a failure. Its
  **Recipe** view is ADR_0014's single-script editor, still the right tool when
  the whole curation is one query.
- `data-curation` is also the one area with a *managed* path beside its ad-hoc
  one: **Run on the server** saves the revision, calls `POST /api/v1/executions`
  and the browser may close (ADR_0025) — which is why both the pipeline's name
  and the execution's id live in the search params, and why a reload comes back
  to the same canvas following the same run. Beside it is the **schedule**: when
  that definition runs unattended, with `Save and run now` for the case where it
  should also go once immediately. It renders `next_run` and never computes one.
  It follows that run with the stream that already exists —
  `openWorkflowStream`, because a managed run's facts carry the execution as
  their `workflow_run_id` — and the stream is only the *signal*: every frame
  re-reads `GET /executions/{id}`, whose projection was written in the same
  transaction as the decision. Nothing here parses a frame's payload, and the
  waterfall is a link to the Workflows view rather than a second drawing of one
  run. The ad-hoc path stays, because a preview and a block at a time are what
  an editor needs; the two empty states say which of the two ran, since "nothing
  run yet" under a card reading `completed` is a contradiction.
- `evaluation` is the one area with an operator's half beside a reader's. Its
  Approvals panel stages a bundle and admits the pair it declares, in that
  order, and it **never computes an approval ID**: that is a digest over the
  canonicalised declaration, so `POST /api/v1/evaluation-approvals/address`
  answers it and a second implementation in TypeScript would be a second answer
  to what a pair is. Withdrawal asks first, because it hides every result
  measured under that pair and is final. It is also the one area that compares
  two *published* results, and it decides neither half of that: which results
  may be compared is `?context_id=`, the catalogue narrowed by the server, and
  whether two of them are comparable is `GET
  /api/v1/evaluation-results/{id}/comparison`. There is no automatic baseline on
  that half — the folded one has one because a log fold has no other way to
  offer a pair — and a withheld delta renders as withheld rather than absent. It
  is the one place a delta may be **coloured**, because a pinned context
  declares `MetricDirection` per metric; a folded report's metric is a name a
  producer sent, so colouring one there would be guessing whether a rise is an
  improvement or a bill. Its **Scorecards** panel publishes the card a run is
  declared against — the compiled scorers, a judge's rubric, a framework's
  metric picked from the catalog the work role recorded with a field per
  parameter, and the people it is held against — and sends nothing the server
  derives: no `declared`, no direction. A card's versions open under it, newest
  first, and a new version starts from any of them; what changed between two is
  `GET /evaluation-scorecards/{name}/diff`, because what a metric was derived to
  be on each side is in no version for a browser to compare. Its **Measure**
  form is the start of a scoring run, and it derives nothing either: the variant
  is a published result's, the answers are a recording, the archive's or a
  worker's task generating them — with a baseline, a second declaration
  differing in its variant and ID alone, followed beside it and compared once
  both have published — the cohort is that result's or one the server derived
  from a dataset version (`POST /evaluation-cohorts`, with the case limit), the
  metrics, the approval and the run's identity come back from the server, and
  the declaration and the run it started live in the URL. How the run goes — its
  timeout and how many questions at once — is the declaration's `settings`,
  never the manifest's. Admitting from there stages the files the cohort pins
  beside the declaration's own manifest, so nobody writes one out. The run it
  started is followed with the shared `ManagedRunCard` — the stream as the
  signal, the run's page as the truth — and the catalogue is re-read whenever
  that run moves, without a list here of which states are endings. A judged
  result says it is a model's word and shows its agreement with people
  uncoloured, with its interval, and what the provider said served it; a metric
  a framework measured names the framework, its release and the model that
  graded it. A declaration the server warns about — a judge or a scorer service
  sent the archive's words, a metric a model graded uncalibrated — shows the
  server's own sentences, and Start and Stage-and-admit wait for somebody to
  acknowledge them — here and in the Approvals panel, which reads
  `reads_archive` and `measured_by` off the chosen manifest and works nothing
  out.
- `experiments` sets the variants measured on one pinned context side by side,
  and every figure on it is the server's: `GET /api/v1/experiments` groups the
  published results by context, and `GET /api/v1/experiments/{context_id}`
  answers the rows with each compared to the `?baseline=` in the URL through the
  comparison's own rule. Two clocks, two columns: a case's latency and tokens
  are the evidence's `usage`, measured by the producer and summarised per result
  at publication, while a run's duration is the log fold's, for as long as the
  log keeps it. A variant measured twice is two rows, because a mean of two p90s
  is no p90. A third clock is what each variant was **observed** doing: the
  server's fold of the runs that named its `variant_id` and that no measurement
  made, over the window in the URL, one per variant and never folded into a
  row's numbers — a variant seen only in its own measurement reads as observed
  nowhere. It times each model call and its first token, and a window is the
  projector's period fold's — the runs that ended from the window's start on,
  where they ended, written or still held — which the column says with where
  counting began, how many runs came from written periods and reached the log
  late, what the log no longer held when the fold came to it, the events the
  runs' clients numbered that never arrived and the runs whose start never did,
  and that the percentiles are bucketed. What calls cost is the server's, at the
  deployment's price table, each call at the price in force on its day — a
  variant's observed calls and each row's cases, by the models their usage names
  — drawn with the day and the model each price was read for, the calls priced
  before any price was read, and the calls nothing priced.
- `evaluation`'s **Case review** panel is where feedback becomes a regression
  case: a proposal names the trace or case it was seen on and the dataset it
  joins, and the queue writes expected answers, approves, rejects and publishes
  the approved as a new version — every rule the server's, every refusal
  rendered as it came, a 403 on somebody else's words as the admin role it
  needs. A case's judgements show the reviews of that case, whichever dataset
  each joins, from `GET /evaluation-reviews/of-target`, and propose it by the
  position its row carries — on a comparison, or on a result's own case list,
  where every case has one: the server reads the question and the answer, so
  nothing is retyped and nothing is composed in the browser. A proposal and an
  expected answer may name the split the case joins; one naming none joins every
  split's cohort, which Measure says in words.
- `annotations` is the one area that draws. Its canvas puts an `<img>` and an
  `<svg>` in one transformed container, both sized to the image's *natural*
  pixels, so SVG user units are image coordinates and no shape ever carries a
  zoom level. Stroke widths and vertex radii divide by the zoom, or the plan
  disappears under ink as soon as somebody zooms out. The draft lives in
  component state and is saved explicitly: a revision is content-addressed, so
  autosaving every vertex drag would mint one per mouse move. The canvas
  implements no validation — the registry's 422 carries every problem, and a
  second rule set in TypeScript would drift from the first.
- `learning` is the one area built entirely on a control plane rather than on a
  store of its own, and one sentence is its whole design: **a workshop is a
  project, a participant is a grant on it, and enrolling is redeeming an
  invitation.** Nothing in the backend has the word "workshop" in it and nothing
  here asks it to — a lesson that runs Monday to Friday and stays readable
  afterwards is a `GrantWindow`, so this area writes no rule of its own and adds
  no route. Three things follow. **A phase is read from one grant, never summed
  into a person's access**: the effective role is the maximum over live grants,
  the server takes that decision per request and only about the caller, so
  `lib/enrollment.ts` reads a single row's dates and the page says in words that
  a closed workshop grant does not mean somebody lost access. The caller's own
  half is never derived at all — `ProjectAccess` carries the server's verdict
  for the project and one per source, and where a verdict exists it is the
  verdict that is drawn. **"Live" is what the first list means**: `projects`
  keeps only the projects a grant reaches *now*, so a place opening next Monday
  is absent and one that closed has left, which is why the second section is
  "Also in this organization" rather than "the ones you administer" and why a
  reader who cannot see the roster is told what the list cannot show them. And
  **a lab is mostly a reader of registries this instance already had**
  (ADR_0034): its tests are an evaluation scorecard at a version and a derived
  cohort, its marks are the evaluation results that share its `context_id`, and
  the one authored thing is the brief. So `lib/labs.ts` asks four scoped routes
  and works nothing out — the context id comes from `GET
  .../labs/{name}/measurement` the way an approval's address comes from its own
  route, each metric's direction comes from the card, and whether one result
  beat another is the comparison route this screen does not call. A lab that
  pins no tests renders the server's `unavailable` sentence, and a workshop with
  no labs says so: the nine empty slots are gone, and drawing a placeholder
  exercise would be the same fake they were drawn to avoid. Which lab is open
  **is** in the URL, and so is the participant's own copy of its notebook: that
  was the day a lab earned a route, because opening one runs code and working in
  one makes a file, and both are things somebody comes back to and sends to a
  classmate. The **notebook a lab hands out is the runtime's file, not the
  registry's**: the lab names it and pins a `sha256`, `screens/overview/notebook.tsx`
  reads *that revision* through `shared/lib/ml-pipeline.ts`, and when the head has
  moved past the pin the two are said to have drifted rather than one being
  quietly served as the other. Nothing executes because a lab was opened — the
  live app is behind a summary somebody clicks, for the reason `EditorHost::open`
  stages and stops. A participant never edits the lab's own file: one namespace
  holds every notebook this instance can run, so `Work in a copy of my own`
  mints one, and saving into the lab's would be thirty people overwriting each
  other. Writing a lab (`screens/overview/compose.tsx`) **uploads the notebook to
  the runtime first and pins what the runtime answered**, because a digest worked
  out in the browser is a second content address for a file the browser does not
  keep. The invitation and redeem cards are `shared/`,
  parameterised on their framing alone, because an administrator inviting
  somebody to a project and an instructor enrolling them in a workshop are the
  same offer.
- `/account` is the one area outside `navigation.ts`, and it gained a second
  view: **Organizations & projects**, which is where a permission is tested and
  a lesson is shared. Three rules hold it. There is **no organization switcher
  in the header** — a selector that scoped the whole panel would announce a
  multi-tenancy the data plane cannot keep, so the selection lives in this
  page's URL and means "the thing I am administering". **An access answer is
  never cached**: `staleTime: 0`, because `ProjectAccess` carries `evaluated_at`
  and is a decision rather than a capability, and a revoked grant must not go on
  being true because react-query still had it. And **the four reads answer two
  different questions and are never mixed**: `projects` and `access` are about
  the caller, `roster` and a project's `grants` are about the organization, so a
  role in the roster is what somebody was *given* and only an access answer says
  what anybody may do now. The grants list shows lapsed rows exactly as issued —
  filtering by the clock in the browser would hide the row somebody opened it to
  find, and would be a second copy of the policy besides. What *is* derived from
  a server answer is which cards to draw: the roster is owner-and-admin only, so
  its 403 is the server saying this caller does not administer the organization.
  **An invitation token is pasted, never linked.** The redeem control is a field
  on this page rather than a `/invite/{token}` route, because a token in a URL
  is a secret in browser history, in a referrer header and in whatever chat it
  was pasted into — and one click instead of one paste does not buy that. It
  also keeps the secret out of this panel's URL contract, where every other
  parameter is something somebody may safely share. The token itself is drawn
  once, in the response that created it; nothing re-reads it, because nothing
  can.
- Any list that can grow with retention is a `useInfiniteQuery` feeding
  `VirtualList` (`src/shared/components/virtual-list.tsx`). A `.map` over a full
  response is only correct for a list with a fixed ceiling.
- **A second period is the one before this one, and its end is the server's**
  (`src/shared/components/period-compare.tsx`). `?compare=previous` on Metrics
  and on an agent's page reads the same route twice on the same filter, and the
  baseline ends one second before the window the *first answer reported*
  started — never `now - window` worked out here, because a relative window is
  resolved against the server's clock and a browser running a few minutes fast
  would ask for a period overlapping the one beside it, invisibly. One second,
  because both ends of a window are inclusive. The pair is relative like the
  window itself: the link means "the period before whatever this opens on", and
  pinning a baseline to a fixed instant is a different question and would want
  a control of its own. "All" has no period before it, so the toggle is
  disabled and says so rather than disappearing. **A change is never coloured
  here** — the one place the panel colours a delta is a pinned evaluation
  context that declares a direction per metric, and nothing declares that about
  a run count or a bill. A rate moves in *points*, an absent baseline reads as
  "nothing reported before" rather than as nought, and a baseline of zero gets
  no percentage, because nothing is not a denominator.
- Every list that can grow with retention also carries the time window
  (`src/shared/components/time-range.tsx`), in the URL as `window` seconds and
  served by the API as `window_seconds`. One control, one preset list, one
  default across every tab: a period that means the last hour in Explore and
  something else in Metrics is a control people re-read before every click. It
  carries across the observability sub-navigation and nothing else does — having
  narrowed to fifteen minutes, "now the metrics for it" is the next question.
  One list defaults differently and says why: Evaluation opens on everything,
  because half its rows are kept on purpose *because* they outlive the log's
  retention, and a day would hide what that half is for.
- An area that exists in the navigation before it exists in the backend renders
  `AreaPlaceholder`, which names what is missing. Never mock data to fill a
  screen — a plausible fake reads as working software.
- `system` is the one area that describes the **deployment** rather than
  anything it holds, and every word on it is the server's: `GET
  /api/v1/system` answers each capability's state, the variables that decide
  it, the values that are not secrets and one sentence on what this instance
  does without it. Three things follow. It **derives nothing** — a page that
  worked out "no object store, so Prompts is off" would be a second copy of
  `AppState`'s conditions in TypeScript, and the day they disagreed this screen
  would be the confident one. A row with a variable and **no value** is the
  contract working: that a thing is configured is not a secret and its value
  often is, so an address or a credential is reported as its variable's name,
  and the issuer is the server's own exception rather than this screen's. And
  it **writes nothing**: changing a setting is an environment variable and a
  restart, so a form here would promise a change nothing durable explains. Its
  read needs the `admin` role and its 403 is drawn as a sentence — an inventory
  of the deployment is an operator's question, so being told no is an answer
  rather than a failure. The link stays in the sidebar for everybody, because a
  link that vanished by role makes "is there a System area" a question the
  panel cannot answer.
- `src/shared/components/ui/primitives.tsx` holds the shadcn-style primitives in
  use (button, badge, card, stat, id chip). Radix is still not a dependency, and
  the line that said it would arrive with the first dialog is settled the other
  way: the first dialog is the command panel, and it took
  **`@floating-ui/react`**. What a centred palette needs is the half of that
  library which is not about coordinates — a portal, an overlay that locks
  scroll, a focus trap that returns focus, and dismissal on Escape and outside
  press that unbinds again — and pulling a component kit in for one of those
  would decide the panel's whole widget vocabulary as a side effect. TanStack
  Form still goes in with the first form, which will be the WebSocket control
  path (cancel a run, approve a tool call).
- The **command panel** (`src/app/command-panel.tsx`, ⌘K) is in `app/` rather
  than in `shared/` because it has to know both the navigation and where each
  feature's filters live, and `shared` may depend on neither — the architecture
  check enforces that. Two rules carry it. **A command is a route and a search
  object, and running one is a navigation**: nothing there mutates, the back
  button undoes it, and that is only possible because every filter in this panel
  already lives in the URL. And **it is matched, never parsed** — typing selects
  from a list `app/commands.ts` authors, the way a shell completes a command
  rather than interpreting a sentence, because a model choosing the route would
  make navigation a network call that can be wrong, and "it went somewhere else
  this time" is a bad property for the thing somebody presses to get unlost.
  Going to a page is *derived* from `app/navigation.ts`, so an area added to the
  sidebar is reachable without a second list; only the filtered commands are
  written out, because the cross product of every parameter and value is
  thousands of rows and almost none of them is a question anybody has.
  `commands.test.ts` holds every command's search against the **route's own zod
  schema**, which is the one drift that is otherwise invisible: zod strips a
  parameter the route does not declare, so the command navigates, the page
  renders, and the filter silently does nothing. It caught two on the way in.

## Refusals

- **Never let the generated client's default decide whether a call worked.** It
  does not throw: a 403 comes back as `{ data: undefined, error }` and the
  promise *resolves*, so a mutation returning the SDK call runs `onSuccess` over
  a refusal. Every call goes through `src/shared/lib/result.ts`, whose readers
  are named for what absence means on that route: `answerOf`, `answerOrNone`,
  `confirmDone` — the last because a successful DELETE is a 204, which the
  client turns into `{}`.
- **Never draw a failed read as an empty state.** 404 is the server saying there
  is no such thing; a 501, a 503 or an expired session is the server saying
  nothing usable. `answerOrNone` splits them, and what follows an empty state
  must not follow a failure: the run card offers *Forget it* only when the run
  is really gone, and the schedule form disables Save while the read has failed.
- **Never read a body from aiwatcher without checking the status first.** In
  `services/query/flow` the pipeline is lazy, so by the time `array_get(__body,
  'rows')` runs there is no status to branch on and a 501 arrives as `Path
  "rows" does not exists`. `CheckedClient` throws at the seam, and a permanent
  answer is relayed as a 4xx so a managed step reads it as `UserCode`. The
  Python engines' pager reads the status before the body for the same reason.
- **Never decide in the panel which commands a run would accept.**
  `allowed_run_actions` rides back with the thing it describes — `GET
  /executions/{id}` and every command route return a `RunView`.
  `state.is_terminal()` in TypeScript is a second copy of `decide`'s
  preconditions in another language.
- **Never put a service's address in a context's actions.** `allowed` says
  *which* actions apply, because only the server knows the state; where they
  live is the panel's routing. An action whose route does not exist is not
  listed.
- **Never let the panel reconstruct a block's context.** The pinned plan, the
  parents' artifacts and the attempt a staging key is named after are all
  somewhere the browser is not, so a canvas that guessed would guess from the
  draft on screen. `ContextSnapshot` carries the plan's own `RuntimeBinding`
  rather than a second description of it.
- **Never work out in the panel which block became which step.** `GET
  /executions/{id}/blocks` answers it from the pinned plan, and
  `RuntimeBinding::blocks` is the one place that knows which specs carry one.
  The interesting case is the one a browser would get wrong: the compiler folds
  a source and every transform behind it into a single Flow query, so three
  boxes light from one `step.started`. The mapping is immutable, so it is asked
  once per run.
- **Never draw a run's outcome on a canvas that is not what it compiled.**
  `followsTheRun` compares the run's `definition_revision` with the revision the
  draft was last loaded or saved at, and `undefined` reads as drift. A false
  drift costs a line of prose; the other direction claims an outcome for a block
  that never ran. No managed run at all is `undefined`, and an edited draft is
  the ordinary state of working rather than a warning.
- **Never keep a managed run's id out of the URL.** A run held in `useState` is
  a run a reload loses, and the way back to an old run is `GET
  /api/v1/workflow-executions`, which folds the log — a list over the inline
  projection is the second read path ADR_0026 forbids.
- **Never re-implement a pipeline's rules in the panel.** `aiwatcher-datasets`
  decides whether blocks form a runnable chain and returns every problem as
  `details` on a 422; the canvas renders those lines. `orderOf` is a *traversal*
  — the order, or `null` — and never an explanation of a refusal.
- **Never validate a drawing in two places.** The annotation registry refuses an
  invalid revision and reports *every* problem at once as `details` on a 422;
  the panel renders exactly those lines and implements no rules of its own.
- **Never sum a window into somebody else's access.** A grant's dates may be
  read one row at a time — that is what a workshop's timetable is — but the
  effective role is a maximum over every live grant, the server takes it per
  request, and there is no route that asks it about anybody but the caller. So
  Learning draws a phase per grant and never a verdict per person, hides no
  lapsed row, and gates no control on what it worked out. A page that added the
  rows up would be a second copy of `GrantWindow::role_at` with less to go on
  than the first.
- **Never decide in the browser whether a case got worse.** The pinned context
  declares which way each metric is better, so `regressed` is a fact about a
  declaration and `mixed` is a state a single verdict would hide. The panel
  renders the change the server sent, including a row its own filter would have
  dropped.
- **Never let the query builder refuse anything.** It generates text in the
  deployed engine's language and the engine decides whether that text runs. It
  follows that the builder must *drop* an attribute the chosen grain cannot
  express rather than emit a column that is not there: a refusal for a chip the
  builder offered reads as the reader's mistake. Its shapes are pinned per
  engine against the real engine.
- **Never let a lineage reference be a link before it resolves.**
  `shared/components/lineage-reference.tsx` holds the joins out of one area's
  records into another's, and each one is admitted before it is drawn: a
  dataset reference is asked of both registries because the spelling alone is
  ambiguous, and a model version is admitted as 64 hex and then held against
  the registry's own model names. That guard is not pedantry —
  `aiwatcher.model.version` carries the registry's digest from the serving
  profile and the provider's served model *name* from a gateway, and only the
  first is a version of anything. The prompt half of the same idea is
  `PromptRefLink` in `prompt-bits.tsx`, beside the registry rules it enforces.
  A link that 404s is worse than a fact somebody has to look up.
- **Never filter a live stream in the browser.** `Scope::Selection` narrows
  `/api/v1/events/stream` server-side, which is why `LiveEvent` carries
  `agent_id` and `service` at all: `llm.chunk` is most of the log by volume, and
  a subscriber watching two agents cannot be resolved to run ids when it
  connects, because the interesting run is usually the one that starts next. An
  event with no value for a filtered dimension is not a match, or narrowing to
  one agent would put its whole run back in the stream.
