# Worker protocol: gaps closed on 2026-09-09

This review follows the working definition → Rust execution → Python worker →
aiwatcher graph path. It covers reliability needed before adding process or pod
provisioning; it does not claim that a Kubernetes controller or hosted decider
has been delivered.

| Gap | Implemented correction | Evidence |
| --- | --- | --- |
| A committed report with a lost HTTP reply could only look like a lost lease. | A matching durable outcome is acknowledged from workflow history, including after lease retirement/restart. Conflicting outcomes receive 409; pinned queue authorization still applies. No second receipt store. | HTTP tests cover success/failure redelivery, conflicts, queue isolation, unchanged history, retired heartbeat and concurrent deliveries. PostgreSQL protocol gate restarts Rust after commit and discards the reply. |
| One process could not request a particular attempt. | `ClaimFilter.attempt` narrows the existing atomic claim, including SQL selection before locking. Capabilities, delay and lease checks remain active. | One shared property runs on memory, file and PostgreSQL: an absent, incompatible or already-held target never claims a neighbour. SDK rejects a mismatched assignment. |
| A successful function could omit a declared artifact and strand its downstream consumer. | Assignments list required outputs; SDK reports omissions as validation failures. Rust independently checks declared names/kinds, duplicates and stored objects before settlement. | SDK omitted-output test; API missing-output and fabricated-object tests. |
| A settlement response could claim success without matching the recorded result. | API checks committed history after settlement; SDK checks both attempt identity and outcome. Conflicting reports are surfaced instead of swallowed as generic lease loss. | Durable-outcome tests and malformed-settlement tests. |
| The graph inferred workflow success from one completed child, or failure from an attempt awaiting retry. | Managed `execution.*` events own the traversal status and terminal time. Child telemetry still supplies node/span details. External workflows keep their existing run-based inference. | Projector regression covers a pending successor, transient child failure, explicit failure/resume/completion and late child telemetry. |

The application entry point remains Runtime:

```bash
aiwatcher-runtime --factory worker_workflow:build_runtime \
  --pool local --attempt execution-id/persist/2
```

It builds the application's services, executes one attempt, joins it and closes
resources. It never starts the other configured slots or updates a definition
head. A task failure is a durable report and exit 1; success is exit 0.

The real PostgreSQL protocol gate reported two deliveries, one accepted acquire
completion, Runtime CLI exit codes `[1, 0]`, two graph nodes, three step spans
and two correctly parented agent spans. Run it with `just test-worker-protocol`;
the separate worker-death gate remains `just test-worker-runtime`.

Report acknowledgement is based on outputs/control result or failure class and
message. It does not reapply diagnostics/cache hints, and history retention also
bounds acknowledgement availability. Task side effects still require domain
idempotency. Claims retain a single-delivery policy: an ambiguous claim reply
recovers through lease expiry. Store reads for receipts currently load the
execution stream; paging/indexing that history remains a performance follow-up.

Next integration boundaries remain Planner's artifact-parity gate, hosted
decider transactions/timers/joins and infrastructure provisioning/autoscaling.

The panel also removes nested buttons in the pinned-notebook source control: copying
a revision and opening source are now separate controls. All 43 panel tests pass
without the previous invalid-HTML warning. SDK checks pass with 282 tests; all
12 PostgreSQL storage/upgrade tests pass, including the new exact-claim property.

Final validation: `cargo test --workspace --all-features` passed with 939 tests
and 24 ignored tests across 54 suites. The new concurrent-report regression also
passed separately. Workspace Clippy (`--all-targets --all-features`, warnings
denied), Rust formatting, SDK Ruff/mypy, panel production build and TypeScript
checks, and generated OpenAPI/client consistency checks all passed. The real
PostgreSQL protocol gate passed against the updated Rust server.
