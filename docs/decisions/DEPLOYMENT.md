# How it is installed, and who may call it

**The one decision underneath all of these:** an installation is a place that
already runs things, and this software is a guest in it. Every decision here
follows from refusing to assume otherwise — the cluster is read rather than
declared, the local context is checked rather than trusted, the identity
provider is proved to rather than asserted at, and a single-node install is
allowed to need none of it.

This is the only group whose decisions a reader of the crates never meets.

| ADR | Decided | Where it stands |
|---|---|---|
| [0006](../ADR/ADR_0006_LOCAL_K8S_WITH_TILT.md) | The test environment is Tilt on a local Kubernetes, guarded against remote clusters | Accepted; the local stack runs Perses since 2026-09-09 ([0005](OBSERVABILITY.md)) |
| [0009](../ADR/ADR_0009_INSTALL_BY_DETECTION.md) | Installation decides what to install by reading the cluster, not by flags | Accepted; what is detected beside planner is Perses, not Grafana, since 2026-09-09 |
| [0013](../ADR/ADR_0013_SINGLE_SIGN_ON.md) | aiwatcher is its own relying party, and the session is a cookie it signs | Accepted |
| [0027](../ADR/ADR_0027_LOCAL_INSTALL.md) | A local install is one binary, one database and one token | Accepted, 2026-09-09 — the newest decision in the repository |

## Why each is shaped the way it is

**0009 exists because of a failure with no error message.** A second
VictoriaMetrics installed beside an existing one splits one workload's metrics
across two stores and reports nothing — just a gap in a graph somebody notices
weeks later. So every backend is `install | external | none` rather than a
boolean, and the findings-to-values mapping lives in one place so a plain `helm
-f` reaches the same result as the planner path.

**0006 is a guard, not a workflow.** The kubeconfig on the machine that wrote it
has production contexts in it, so both the `Tiltfile` and `just tilt-up` hard-stop
on a non-local context. The guard is duplicated deliberately: a typo in a context
name must not be the only thing between a keystroke and production.

**0013 is shaped by two routes.** The panel's most important surfaces are an SSE
stream and a WebSocket, and a browser can set headers on neither — so the
authorization-code exchange runs *in this process*, the provider's tokens are
read once and dropped, and what the browser keeps is an HttpOnly cookie this
server signs. There is no session store, which means the session TTL **is** the
revocation window. The default is `none`, because a release that started refusing
requests would be an upgrade that took an installation down.

**0027 is the other end of the same range.** Everything above assumes a cluster;
0027 assumes a laptop or one box, and its point is that the difference should be
configuration rather than a second product.

## What has moved

**Grafana became Perses (2026-09-09)** and touched three of these four by status
line. Only [0005](OBSERVABILITY.md) carries the amendment, because these name the
viewer without deciding it.

## What would reopen one

0013's is the one with a real trigger: `proxy` mode trusts a header, and the
chart refuses to render it without a NetworkPolicy for exactly that reason. An
installation that needs a boundary the network cannot draw is an installation
that needs `oidc`, and the day a deployment needs neither is the day this
decision is worth re-reading.
