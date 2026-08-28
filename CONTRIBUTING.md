# Contributing

## Before a PR

```bash
just check
```

Runs everything CI does — Rust format, clippy, tests, the OpenAPI contract
check, the panel build and typecheck, the SDK typechecks, and the repo-wide
linters. Green here means green in CI. Optional linters that are not installed
are reported as SKIP with an install hint rather than failing the run.

To run it automatically before every push:

```bash
just setup-hooks
```

Two suites are deliberately outside `just check`, because both need something
running:

```bash
just iggy-up && just test-laser              # the Laser adapter against a real broker
just tilt-ci                                 # the whole stack on a local Kubernetes
AIWATCHER_K8S_OVERLAY=laser just tilt-ci     # the same, on the Laser backend
```

`just test-laser` is worth running after any change to `adapters::laser`. Two
live-locks in that file — unbounded redelivery, and a subscription that outlived
its stream and held its consumer-group membership — were invisible to the
in-process fake and obvious within seconds against a real broker.

## Changing an API route or a type it exposes

```bash
just openapi
```

This regenerates `contracts/openapi.json` **and** the panel's TypeScript client.
Commit both. CI fails if either is stale, because a stale client is a runtime
`undefined` rather than a compile error — exactly what the codegen exists to
prevent.

## Adding an event type

See [docs/event-catalog.md](docs/event-catalog.md#adding-an-event-type). Four
steps, and the exhaustiveness tests in `catalog.rs` will tell you if you missed
a start or an end for a subject.

## Adding a log backend

Implement `MessageSource`, `MessageSink` and `Checkpointer` in
`crates/aiwatcher-bus/src/adapters/`, then add the arm to
`crates/aiwatcher-server/src/wiring.rs`.

If it needs a heavy dependency or a running server, put it behind a cargo
feature the way `laser` is: the default build and the default test suite should
need neither.

Add it to `crates/aiwatcher-bus/tests/contract.rs`. That file runs one body of
assertions against every adapter — interchangeable adapters are only
interchangeable if they behave identically, and a shared test is the only way to
keep that true.

## Writing tests

Name a test for the behaviour it pins down, as a sentence:

```rust
#[test]
fn two_parallel_llm_calls_both_parent_onto_the_agent_not_onto_each_other() { … }
```

not `test_parenting`. A failing test name should explain the bug without opening
the file.

## Touching the Kubernetes stack

`deploy/k8s/base` is the write-ahead-log stack; `deploy/k8s/laser` is an overlay
that swaps the backend. `just k8s-validate` checks both client-side and runs as
part of `just check`, so a broken kustomization never needs a cluster to catch.

Do not weaken the local-context guards in the `Tiltfile` or the justfile. This
kubeconfig has production clusters in it.

## Decisions

If a change would be expensive to reverse — a storage choice, a wire format, an
ordering guarantee — write an ADR under `docs/ADR/` using
[the template](docs/ADR/template.md). The section that matters is **what would
make this wrong**: without it nobody knows when to reopen the decision.
