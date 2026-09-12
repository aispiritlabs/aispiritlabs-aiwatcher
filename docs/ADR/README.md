# Architecture decision records

One file per decision that would be expensive to reverse, written when the
decision is made rather than reconstructed afterwards. The value is in the
**Consequences** section: what this costs, and what would make it wrong.

Thirty of them is a lot to read cold. [`docs/decisions/`](../decisions/)
groups them into four readings — the log, the registries, execution,
deployment — with one line each and, unlike the table below, which ones later
ADRs amended or partly took back. Those are reading guides; **these files are
the record**, and they are what code comments cite.

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
| [0013](ADR_0013_SINGLE_SIGN_ON.md) | aiwatcher is its own relying party, and the session is a cookie it signs |
| [0014](ADR_0014_DATA_CURATION.md) | Flow executes curation; the authenticated Rust registry versions its scripts and outputs |
| [0015](ADR_0015_DATASET_EXPLORATION.md) | Dataset exploration uses slices and immutable dataset references |
| [0016](ADR_0016_PIPELINE_ENGINE.md) | The orchestrator is read for its inventory and asked to start one entry; the graph still comes from the log — **superseded** by AW-4, the engine removed |
| [0017](ADR_0017_IMAGE_ANNOTATION.md) | An annotation is authored, vector-first, and split by family rather than by image |
| [0018](ADR_0018_TRAINING_RUNS.md) | A training run is a record, not a trace, and it has its own module |
| [0019](ADR_0019_DATASET_HUB_DISCOVERY.md) | A dataset hub is searched for what exists and never asked what is permitted |
| [0020](ADR_0020_GENERIC_VISION_ANNOTATION.md) | The annotation tool ships no vocabulary; the schema carries the domain |
| [0021](ADR_0021_CONVERSATION_ARCHIVE.md) | Conversation content is an encrypted archive with its own retention, not events on the log |
| [0022](ADR_0022_STAGED_IMPORT_JOBS.md) | A long job over an object store is one primitive, and a corpus is staged before it is imported |
| [0023](ADR_0023_MODEL_PACKAGE.md) | A serving runtime is handed a declared package, and a checkpoint URI is not one |
| [0024](ADR_0024_CURATION_BLOCKS.md) | A curation is a chain of blocks, each belonging to the engine that can run it |
| [0025](ADR_0025_MANAGED_EXECUTION.md) | A managed execution is owned by the server, and the browser only asks for one |
| [0026](ADR_0026_ENGINE_AS_PRODUCER.md) | The execution engine is a producer on its own log |
| [0027](ADR_0027_LOCAL_INSTALL.md) | A local install is one binary, one database and one token |
| [0028](ADR_0028_QUERY_ENGINES.md) | A deployment chooses its query engine, and a typed query is admitted or runs where code runs |
| [0029](ADR_0029_POD_PER_STEP.md) | A step that needs a pod names an operator's template, and the pod is a worker for one attempt |

| [0030](ADR_0030_EVALUATION_EVIDENCE.md) | Evaluation owns pinned variants and durable evidence; contract first, persistence next |
| [0031](ADR_0031_POD_ATTEMPT_CREDENTIAL.md) | A pod authenticates as its attempt, with a credential the launcher mints — **proposed** |

Use [template.md](template.md) for a new one, and add a line to the reading it
belongs to in [`docs/decisions/`](../decisions/).
