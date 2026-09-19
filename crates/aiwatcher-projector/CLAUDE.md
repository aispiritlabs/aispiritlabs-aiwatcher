# The projector: folds, periods and the journal

The rules for `aiwatcher-projector`. The read model, the dimensions
([ADR_0007](../../docs/ADR/ADR_0007_EXPLORER_DIMENSIONS.md)) and span assembly
([ADR_0003](../../docs/ADR/ADR_0003_SPAN_ASSEMBLY.md)) are in the root file,
with everything else about what the log carries; what is here is the period fold
— the one output of this crate that writes an object store — and the journal and
`asked` index beside it.

- **Never count a benchmark's runs as traffic.** A run names the variant that
  answered in it (`variant_id` on the envelope, ADR_0001 amended), and a run
  made to answer a measurement's case also names that result on its start
  (`data.evaluation_id`). The variant's observations count those apart and in no
  figure, because the measurement's own runs already are its result. The
  telemetry clients take a variant only as an argument, never from the
  environment, so a worker importing the deployed application does not report a
  benchmark as traffic. A measurement's runs are numbered all the same, per
  result and attempt, so a traces step tells a run lost in transport from one
  nobody opened.
- **Never snapshot a bounded fold into a durable record.** What a variant was
  observed doing is a projector output of its own (`period_fold`), not a copy of
  the read model: it reads each event once in log order, closes a period when
  the log's clock — `min(occurred_at, ingested_at)`, so a skewed producer closes
  nothing early — has passed it, and writes it create-only, rolled up into the
  hour and the day it lies in.
- **Never let a restart leave a gap in it.** Its state is saved as generations
  under the position it was folded through; the one furthest along is loaded,
  the projector resumes from that position when it is behind the checkpoint, and
  the fold skips what it holds — so a restart that does not replay leaves no
  gap, a replay counts nothing twice, and a process sharing the processor ID
  sets nothing back. With no state left it starts again from where the last
  period written says it was. A closed period that could not be written holds
  the checkpoint back and stays the fold's to read until it is written.
- **Never count a late run where it arrived.** A run ending in a closed period
  is held in the oldest open one by the period it ended in, never dropped and
  never counted where it arrived, and a period keeps its runs by the second they
  ended in whatever its width. A width configured anew takes over at the next
  hour, so two widths never cover one span.
- **Never read a gap in the log as a silence.** On a log that numbers every
  event, a position the log no longer holds is first looked for in the
  **journal** — a consumer of its own, on a connection of its own, in every role
  and in one of its own, keeping each stretch it read with only what the fold
  reads, for as many days as the deployment says. What no page covers is written
  down with the span of time it may have lain in, the periods it reaches say
  they are incomplete, and a window over it says how many events it may be short
  of. On any log, a number a client skipped in its own count of a run's events
  is an event the fold never read: the run is counted with what arrived, its
  period says it is incomplete, and a window counts the lost events.
- **Never let a run nobody heard of go uncounted.** A number a client skipped in
  its count of the runs it opened for a variant is a run whose start never
  arrived, counted where the next start did — and so are the runs before a
  client's first start the fold reads, and the runs a client's `client.counted`
  says it opened past the last start that arrived. Said when it closes, every
  five minutes by its own clock, and — where a transport keeps a spool, which
  holds the count as each run starts — by the next transport on that spool,
  however the client ended. A client killed with no spool leaves its last lost
  runs unsaid.
- **Never answer a window from two folds.** A window over what a variant was
  observed doing is the period fold's alone — every period it reaches into, from
  the store and from the fold's memory, counting from the window's start to the
  second — and it says where counting began; without a window the answer is the
  read model's alone. Split between the two, a late run or an evicted one was
  counted twice or not at all, and nothing in the numbers said which.
- **Never read calls asked elsewhere from the read model alone.** It holds only
  the runs its caps keep, and after a restart that does not replay, none — so a
  case asked there before was never asked, by its account. The projector's
  `asked` index keeps every witnessed call that said what it asked, a page a
  minute, in the object store, and a page not yet written holds the resume back
  as a closed period does. What it does not reach back to is named with its date
  and denies the exchange, rather than read as nothing asked.
- **Never fold a project into a fold of its own.** An event carries which
  project published it (ADR_0001 amended, ADR_0033), and **one** fold reads it,
  with the scope in the row: a run's project is the first event's and is never
  moved, a dimension row is `(project, key)`, a span carries it as two
  attributes the assembler writes when the span opens, and a period's record,
  a client's count of a measurement's runs and a witnessed call each carry it
  too. Absence is the global side, which is every row this build has written, so
  nothing existing moves. A fold per tenant would answer the same questions and
  would be the premise ADR_0033 says to revisit — that one instance serves many
  projects — rather than a dimension on the one it already has.
- **Never let a project into a key a content address already names.** A variant
  ID is a content address of a declaration's pins and an `evaluation_id` is a
  published result's, so one declaration made in two projects has one ID — and a
  map keyed by it alone sums two projects' traffic into a figure belonging to
  neither. The scope goes in the **key** (`period_fold::keyed`, the measured
  counts, the period store's `scopes/` segment) and on the record's face; a
  global key stays byte for byte what it was, so a create-only write still lands
  on the first record.
- **Never read a project's row as permission to see it.** Every one of these is
  a fact about whose a run is, and none is a decision about who may read it.
  That is a grant, asked of IAM fresh per request, in `aiwatcher-api`.
- **Never let a read of the fold not name a side.** `ReadScope` is `Global |
  Project` and it is an argument to every read that returns runs or spans, not
  an axis in `RunSelection`: the axes there are a caller's own and may be
  widened by asking for less, while a scope decides which rows are there at all
  — including for the counts a fold takes *before* it narrows anything, like a
  dimension page's ungrouped total or the metrics summary's `runs_retained`.
  Filtering after the fold reports one side's rows against the whole instance's
  totals. The default is `Global`, which fails closed: a reader that forgot to
  name a side sees what was always instance-wide and never a project's.
  `ReadModel::runs_in` is where the rows come from; a fold that reads
  `state.runs` directly is a fold that forgot.
