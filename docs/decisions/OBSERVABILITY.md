# The log, and everything folded from it

**The one decision underneath all of these:** there is one durable log, delivery
to it is at-least-once, and everything the panel shows about a run is a fold
over it. That is why ids are *derived* rather than generated — a redelivery has
to land on the thing it already wrote — and why retention bounds every answer in
this group. What must outlive retention is [authored](REGISTRIES.md) instead.

| ADR | Decided | Where it stands |
|---|---|---|
| [0001](../ADR/ADR_0001_EVENT_ENVELOPE.md) | The envelope carries four correlation ids, and the backend derives what a producer omits | Accepted, **amended 2026-08-28** — the derivation gained an avalanche finalizer |
| [0002](../ADR/ADR_0002_EVENT_BUS_PORT.md) | Laser is the backbone, behind a port, with adapters that work without it | Accepted. The write-ahead log is the default and `laser` is a cargo feature that is off |
| [0003](../ADR/ADR_0003_SPAN_ASSEMBLY.md) | An event is not a span | Accepted. Hundreds of events fold into a handful of spans; `llm.chunk` is counted, never stored |
| [0004](../ADR/ADR_0004_LIVE_STREAM_RESUME.md) | The live channel is the projector's own fan-out, and a reconnect closes its own gap | Accepted. Every frame carries its checkpoint as the SSE `id:`, so the browser resumes with no application code |
| [0005](../ADR/ADR_0005_TRACE_STORAGE.md) | VictoriaTraces stores spans; QuestDB is a projection to add later, if ever | Accepted, **amended 2026-09-09** — the waterfall comes from Perses, not Grafana |
| [0007](../ADR/ADR_0007_EXPLORER_DIMENSIONS.md) | Every way of slicing runs is one fold, and every list is a cursor page | Accepted. `session \| agent \| runtime \| workflow \| trace \| model \| tool` differ only in which key a run contributes |
| [0010](../ADR/ADR_0010_EVALUATION_REPORTS.md) | An evaluation report rides the event log and forms **no** span | Accepted. `forms_span` is false and the assembler returns immediately |
| [0012](../ADR/ADR_0012_WORKFLOW_GRAPH.md) | A workflow graph is declared on the log, not discovered from an orchestrator | Accepted, and load-bearing for [execution](EXECUTION.md): it is the source that is still right when the orchestrator is bypassed |

## What has moved

**Grafana became Perses (2026-09-09).** Only 0005 carries the amendment; 0004,
[0006](DEPLOYMENT.md) and [0009](DEPLOYMENT.md) carry a status line pointing at
it, because each names the viewer without deciding it. Nothing about the log
itself was reopened.

**Nothing else in this group has been amended or superseded.** 0002's port
outlived a second backend and a role split, 0003's rule survived four new event
families that had to be told not to form spans, and 0007's one fold is what
every list route still answers from.

## What would reopen one

0005 names its own trigger — a span volume or a query shape VictoriaTraces
answers badly enough to justify a second store. 0003's is the opposite of a
number: an event type that genuinely *is* a span, rather than one somebody
wanted in a waterfall. And 0001's is the only one that would be expensive — a
producer whose span key is not stable, which turns derivation from a dedup
mechanism into a source of duplicates.
