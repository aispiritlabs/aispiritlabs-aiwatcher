# Who runs work, who decides, and where the facts go

**The one decision underneath all of these:** a thing that executes work and a
thing that decides what to execute are different, and only the second belongs to
this server. Read in date order these six ADRs are one argument arriving in
stages — a query surface, then a chain, then an owner, then a producer — and
each stage names what would make it wrong. Two of them were made wrong on
schedule.

| ADR | Decided | Where it stands |
|---|---|---|
| [0008](../ADR/ADR_0008_FLOW_QUERY_SURFACE.md) | Flow PHP is a query surface over the API, parsed rather than executed | Accepted, **three amendments** |
| [0014](../ADR/ADR_0014_DATA_CURATION.md) | Flow executes curation; the Rust registry versions its scripts and outputs | Accepted; **browser-mediated persistence superseded by [0025](../ADR/ADR_0025_MANAGED_EXECUTION.md)** for managed runs |
| [0016](../ADR/ADR_0016_PIPELINE_ENGINE.md) | The orchestrator is read for its inventory and asked to start one entry; the graph still comes from the log | Accepted, unchanged |
| [0024](../ADR/ADR_0024_CURATION_BLOCKS.md) | A curation is a chain of blocks, each belonging to the engine that can run it | Accepted; **the execution half superseded by 0025**, the blocks stand |
| [0025](../ADR/ADR_0025_MANAGED_EXECUTION.md) | A managed execution is owned by the server, and the browser only asks for one | Accepted |
| [0026](../ADR/ADR_0026_ENGINE_AS_PRODUCER.md) | The execution engine is a producer on its own log | Accepted |

## The arc, in the order it happened

**0008 drew the line at dispatch.** A name from a query may select a key; it may
never become a callable. There is no `eval` in `services/flow` and no name from
a query ever reaches a method.

**0014 said a curation is one script**, versioned by an authenticated registry —
and that is still right whenever the whole curation *is* one query.

**0024 said it often is not.** Detecting personal data needs something that
reads text, and a query composes values without reaching arbitrary code. So a
chain: source → transform → notebook → view, each block belonging to the engine
that can run it.

**0025 took the browser out of it.** 0024's own Consequences named the three
things that would make "the chain is driven from the browser" wrong —
unattended, scheduled, or a corpus too large for one session — and all three
arrived at once. A definition compiles to an immutable `ExecutionPlan`, a run
pins one, `decide` is pure, and one workflow input is six writes in one
transaction.

**0026 put the facts back on the log.** The question 0016 had deferred, decided
in favour: `execution.*` joins the catalog, a started plan publishes
`workflow.declared`, an attempt publishes `step.*`. So the [workflow
fold](OBSERVABILITY.md), the waterfall and the live stream draw a managed run
with **no second read path** — and the store keeps the *why*, never the facts.

## Where the boundary actually sits

The amendments to 0008 are worth reading as a group, because they moved a line
this repository had drawn in the wrong place. What guarded the query surface was
a hand-written list of 37 admitted function names, and Flow ships 239 — so the
things this language was said to be unable to do (arithmetic, a join, a group's
median beside a row, window functions) were missing from *the list*, not from
Flow. `Dsl\Registry` derives admission from return type and refuses any
parameter that takes a callable; the rule about dispatch is untouched, and
adding a name by hand now means the rule did not cover it.

What remains outside is what has no vocabulary in a query language: a model, a
scanner. That is what a notebook block is for, and it is why 0024 exists at all.

## What would reopen one

0016's is a consumer that needs one launch API for local and engine work —
nothing has needed it, and planner removed its orchestrator entirely. 0024's is
a fourth engine, which would test whether "each block belongs to the engine that
can run it" is a principle or a description of three cases. 0025's own trigger
has already fired once and produced 0026; the next would be an execution whose
decisions genuinely cannot live in this process, which is what the hosted mode
in `ExecutionMode` is reserved for.
