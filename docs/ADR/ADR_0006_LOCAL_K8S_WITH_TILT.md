# ADR_0006: The test environment is Tilt on a local Kubernetes, guarded against remote clusters

- **Status**: accepted
- **Date**: 2026-08-27

## Context

`just stack-up` (docker compose) proves the components talk to each other. It
does not prove the things that only exist in Kubernetes: Services resolving by
name, readiness gating a rollout, a ConfigMap the Collector actually reads,
Iggy behind a StatefulSet, and the seccomp profile that Iggy's io_uring runtime
needs. Those are exactly the parts that break on first deploy.

There is a second reason. The Laser adapter cannot be exercised by the default
test suite — it needs a broker. A cluster that already runs Iggy is the natural
place to run it.

## Decision

`Tiltfile` plus `deploy/k8s/` (a kustomization) stand the whole stack up on a
local cluster: Iggy, VictoriaTraces, VictoriaMetrics, the OpenTelemetry
Collector, Grafana, and aiwatcher on the `laser` feature.

**The image is built in a container**, using the same `deploy/Dockerfile` as a
release. The manifests are copied before the sources, so a source-only change
reuses the dependency layer and recompiles only the six workspace crates.

The first attempt built on the host and wrapped the binary in a one-`COPY`
image, to reuse the local `target/` cache. That is meaningfully faster and it
does not work on macOS: the result is a Mach-O binary and the pod dies with
`Cannot run macOS (Mach-O) executable in Docker: Exec format error`. Making it
work would mean requiring `cross` or `cargo-zigbuild`, which is a setup step
this should not impose. On a Linux workstation the host build would be the
better trade; it is not portable enough to be the default.

**Two independent guards stop this reaching a remote cluster.** The kubeconfig
on a machine like this one has production EKS contexts in it, and Tilt applies
to whatever context is current:

1. The `Tiltfile` calls `fail()` at load time unless the context is on an
   explicit local allowlist (`orbstack`, `docker-desktop`, `minikube`, `colima`,
   `rancher-desktop`, `kind-*`, `k3d-*`, `k3s-*`), and only then narrows Tilt's
   own permission with `allow_k8s_contexts`.
2. `just tilt-up` runs the same check first, so a mistake is caught before Tilt
   starts at all.

Both are hard stops, not prompts. A typo in a context name must not be the only
thing between a keystroke and a production cluster.

`just k8s-validate` renders and validates the manifests **client-side**, so it
runs in CI and in `just check` without any cluster.

## Alternatives considered

**docker compose only.** Already there and still useful, but it cannot exercise
probes, Services, or the pod security context — and the seccomp requirement is
one of the two things that make Iggy hard to deploy.

**Skaffold, or plain `kubectl apply` in a script.** Both work. Tilt was chosen
for the file-watching rebuild loop and for the resource graph that makes
`resource_deps` (aiwatcher waits for the Collector, and for Iggy on the Laser
overlay) declarative.

**A cluster created on demand — `ctlptl` with kind or k3d.** Cleaner isolation,
and another tool to install. This machine already has a local cluster; the
allowlist covers the common creators, so adopting one later is a context-name
change.

**Trusting Tilt's own default context allowlist.** Tilt does block unknown
contexts by default, but the failure is a Tilt-shaped error at deploy time
rather than a message that says which context to switch to and why. Given what
is in this kubeconfig, an explicit guard with an explicit message is worth the
twenty lines.

## Consequences

- The stack needs `AIWATCHER_K8S_CONTEXT` to name a local cluster; the default
  is `orbstack`. Anything else is refused with a message naming the accepted
  values.
- `replicas: 1` on aiwatcher is load-bearing, not a placeholder: a second
  replica would have its own live hub and its own read model, so which pod a
  browser reached would decide what it saw. The Deployment uses `Recreate` for
  the same reason — two projectors in one consumer group would both consume
  during a rolling overlap.
- The namespace is labelled `pod-security.kubernetes.io/enforce: privileged`,
  because Iggy needs an unconfined seccomp profile. On a shared cluster this
  would be a dedicated namespace with a targeted exception instead.
- Everything uses `emptyDir`. A restart starts from an empty log, which is right
  for a test stack and wrong for anything else.
- Grafana runs with anonymous admin access. Local only.
- The stack is split into `deploy/k8s/base` (write-ahead log) and
  `deploy/k8s/laser` (an overlay switching the backend), selected with
  `AIWATCHER_K8S_OVERLAY`. The base is the default so the stack comes up green
  on any machine; the Laser overlay additionally needs a broker its client can
  log in to, which is still open — see [ADR_0002](ADR_0002_EVENT_BUS_PORT.md).
- Iggy needs `IGGY_SYSTEM_SHARDING_CPU_ALLOCATION` set to a fixed shard count.
  Its default, `numa:auto`, binds shard memory to a NUMA node and fails outright
  inside a container VM, taking the server down with `MemoryAffinityFailed`.
- The first `tilt up` compiles ~400 crates in Docker and takes minutes. After
  that the dependency layer is cached and a source change rebuilds in well under
  a minute.

**What would make this wrong.** If the manifests here start drifting from what
actually gets deployed — because production grows a Helm chart, say — then this
stops being a test of the real wiring and becomes a second thing to maintain. At
that point the local stack should render from the same source as production
rather than duplicating it.
