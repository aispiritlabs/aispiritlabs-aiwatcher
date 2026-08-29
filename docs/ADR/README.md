# Architecture decision records

One file per decision that would be expensive to reverse, written when the
decision is made rather than reconstructed afterwards. The value is in the
**Consequences** section: what this costs, and what would make it wrong.

| ADR | Decision |
|-----|----------|
| [0001](ADR_0001_EVENT_ENVELOPE.md) | The event envelope, and the four correlation ids |
| [0002](ADR_0002_EVENT_BUS_PORT.md) | Laser behind a port, feature-gated, with adapters that work without it |
| [0003](ADR_0003_SPAN_ASSEMBLY.md) | An event is not a span |
| [0004](ADR_0004_LIVE_STREAM_RESUME.md) | The live channel, and how a reconnect closes its gap |
| [0005](ADR_0005_TRACE_STORAGE.md) | VictoriaTraces for spans, QuestDB deferred |
| [0006](ADR_0006_LOCAL_K8S_WITH_TILT.md) | Tilt on a local Kubernetes, guarded against remote clusters |
| [0007](ADR_0007_EXPLORER_DIMENSIONS.md) | Every way of slicing runs is one fold, and every list is a cursor page |
| [0008](ADR_0008_FLOW_QUERY_SURFACE.md) | Flow PHP is a query surface over the API, parsed rather than executed |
| [0009](ADR_0009_INSTALL_BY_DETECTION.md) | Installation reads the cluster to decide what to install |
| [0010](ADR_0010_EVALUATION_REPORTS.md) | An evaluation report rides the event log and forms no span |
| [0011](ADR_0011_PROMPT_REGISTRY.md) | A prompt is authored, not observed, and lives in an object store |
| [0012](ADR_0012_WORKFLOW_GRAPH.md) | A workflow graph is declared on the log and folded like everything else |

Use [template.md](template.md) for a new one.
