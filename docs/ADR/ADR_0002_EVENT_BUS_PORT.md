# ADR_0002: Laser is the backbone, behind a port, with adapters that work without it

- **Status**: accepted
- **Date**: 2026-08-27

## Context

Laser (`laser-sdk` 0.3, over Apache Iggy) is the event backbone. It gives us
what the pipeline needs: ordering within a partition, resumable cursors,
server-managed consumer-group offsets, and at-least-once delivery.

It is also a young dependency — 0.3.0, published 2026-08-19 — that pulls roughly
360 crates and requires rustc 1.97.1. And it needs a broker running to test
against, which a unit test suite should not.

## Decision

**Nothing above `aiwatcher-bus` names Laser.** The pipeline talks to
`MessageSource`, `MessageSink` and `Checkpointer`; which log is behind them is a
wiring decision in `aiwatcher-server`.

Four adapters ship:

| Adapter | For |
|---------|-----|
| `adapters::laser` | The real `laser_sdk`. Behind the `laser` cargo feature. |
| `adapters::wal` | An append-only JSONL file. Single node, durable, no broker. **The default.** |
| `adapters::memory` | Tests and `just dev`. |
| `adapters::broker` | A generic poll/commit adapter over a four-method trait — what a Kafka or NATS backend would implement, and the one the contract test can drive without a broker. |

The `laser` feature is **off by default**. A plain `cargo build` and the whole
default test suite need neither the SDK nor a broker; `just build-laser` and
`just test-laser` turn it on. `AIWATCHER_BUS=laser` in a binary built without
the feature is a startup error, never a silent fallback to a different log.

### The envelope on the wire, not the record

The other adapters promote an `EventEnvelope` into a `RecordedEvent` at append
time, because they *are* the store and they assign the position. Under Laser the
broker assigns it, and a producer cannot know it in advance. So the topic
carries the **envelope**, and the **consumer** promotes, stamping the position
from the offset Iggy actually gave the record.

That is also what lets a Python agent publish to Laser directly rather than
through this process: a producer never has to be able to compute a position.

### One partition

A `Checkpoint` is a single ordered scalar. That is what makes `Last-Event-ID`
resume work with no client-side bookkeeping, and what lets the live tail drop
what a client already saw with one comparison.

A multi-partition log has no total order — partition 0 offset 100 and partition
1 offset 5 are not comparable — so a scalar cursor would silently skip events on
a lagging partition. `LaserConfig::partitions` therefore defaults to 1, and the
constructor warns if it is raised. Records are still keyed by run
(`run:<run_id>`), so raising it later preserves per-run ordering; what it would
*also* require is replacing the scalar checkpoint with a per-partition vector.

### Commits go through the consumer task

Offsets can only be stored through the `Consumer` that owns them, and that lives
in the subscription task. `Checkpointer::save` therefore sends the position over
a channel to that task. The point is to keep the ordering the pipeline depends
on: commit **after** the durable write, never before.

`Checkpointer::load` returns `None` deliberately — the broker resumes a consumer
group from its own committed offset, and overriding that with a stale local copy
would replay events the group already handled.

## Alternatives considered

**Depend on Laser unconditionally.** Simplest, and it makes a 0.3 dependency and
its 360 crates load-bearing for every build and every test, including the ones
that have nothing to do with the log.

**Use `iggy` directly, skipping Laser.** The client crate is real and Laser is
built on it. Rejected: Laser is what was chosen, and its `Topic`/`Cursor`/
`Consumer` layer is the part this adapter actually uses.

**Kafka or NATS JetStream.** Both are safe and both are more operational surface
than this system needs. `adapters::broker` means either can be adopted by
implementing four methods.

**Ship only the write-ahead log and defer Laser.** Rejected now that the SDK is
real and wired: deferring would leave the multi-writer story untested.

## Consequences

- The write-ahead log stays the default and is a real durable single-node log:
  it survives a restart, truncates a torn trailing record cleanly, and resumes a
  projector from a stored checkpoint. What it does not do is scale past one
  process.
- The Laser adapter's tests are `#[ignore]`d and need a broker
  (`just iggy-up && just test-laser`). Six of them, ~2 seconds against a real
  server. CI runs them against an Iggy service container; the default local
  suite does not.
- Connecting has a **15-second timeout**. The Iggy client retries an unreachable
  broker with no ceiling, and a process that hangs on startup is worse than one
  that exits: it never goes unready, so no probe fires and nothing restarts it.
- MSRV is 1.98.0 because of this dependency. Every other crate in the workspace
  would have been happy on 1.90.

### Running Iggy: three flags, each of which fails differently

`laser_sdk` 0.3 builds on the `iggy` 0.11 client. Getting a server it can talk
to took longer than writing the adapter, and none of the failures name their
cause:

| Missing | Symptom |
|---------|---------|
| `seccomp=unconfined` | `Cannot create runtime: Operation not permitted` — the runtime is io_uring, and the default Docker/Kubernetes profiles block `io_uring_setup`/`enter`/`register`. The server's own message names this one. |
| `IGGY_SYSTEM_SHARDING_CPU_ALLOCATION=<n>` | `MemoryAffinityFailed`. The default `cpu_allocation = "numa:auto"` binds each shard's memory to a NUMA node, which fails in a container VM and takes the whole server down. A fixed shard count skips the NUMA path. |
| `IGGY_ROOT_USERNAME` / `IGGY_ROOT_PASSWORD` | The server accepts the TCP connection and then closes it part-way through the login. Client-side this reads as `Failed to read VSR response header` and then an unbounded reconnect loop — it looks like a protocol version mismatch, and it is not. |

Server version still matters: an Iggy **0.8.x** server accepts the connection
and never answers the login regardless of the above. Pin **0.9.x**.

`just iggy-up` and `deploy/k8s/base/iggy.yaml` set all three, each with the
symptom in a comment beside it.

### Two live-locks the integration suite caught

Both were in this adapter, both would have reached production, and neither
shows up without a real broker:

**Unbounded redelivery.** `CommitPolicy::Disabled` is required — the pipeline
must commit only after a durable write — but under it every poll starts from the
offset last *stored* on the server. Between a read and its commit the broker
keeps returning the same records: the test measured ~300,000 redeliveries of the
same two events. The adapter now tracks its own local read position and drops
what it has already emitted. The committed offset stays deliberately behind that
position, and that gap is exactly what makes a crash replay rather than lose.

**A subscription that outlived its stream.** The task kept polling after the
caller dropped the receiver, holding its consumer-group membership, so the next
subscription to the same group joined and received nothing. It now watches for
the receiver closing and leaves the group.

Consumer initialisation is also bounded (`init_retries` plus a timeout): the
Iggy client retries forever by default, which turns a misconfiguration into a
hang instead of an error.

**What would make this wrong.** If throughput ever exceeds one Iggy partition,
the scalar checkpoint is the thing that has to change first — not the adapter.
And if the port is never used for anything but Laser and the write-ahead log,
`adapters::broker` is indirection with no second implementation to justify it
and should be deleted rather than maintained.
