# Installing aiwatcher

Two ways in, depending on what the target already runs.

| | |
|---|---|
| An empty namespace on a cluster of your own | `deploy/scripts/install.sh` |
| Beside something that already has half the stack | `deploy/scripts/install.sh -e planner -n planner` |

Both go through `deploy/helmfile.yaml.gotmpl`, which looks at the cluster before it
decides what to install. The rest of this document is what that means and what
is left for you.

---

## What gets installed, and what does not

The chart can install ten things. Seven of them are things a cluster may
already run:

| Component | Default | Detected? |
|---|---|---|
| aiwatcher server (projector + API) | always | — |
| panel (nginx + the React build) | always | — |
| Query engine — Flow, DataFusion or DuckDB (the Query tab, recipes, a chain's query step) | **off** | — |
| OpenTelemetry Collector | installed | detected, but **never** reused automatically |
| VictoriaTraces | installed | yes → `mode: external` |
| VictoriaMetrics | installed | yes → `mode: external` |
| RustFS (the prompt registry's store) | installed | detected, but **never** reused automatically |
| PostgreSQL (the workflow store) | **off** | detected, but **never** reused automatically |
| Chat model (llama.cpp, OpenAI-compatible) | **off** | detected, but **never** reused automatically |
| Web search (SearXNG, JSON API) | **off** | detected, but **never** reused automatically |
| Perses | never installed | yes → the datasource ConfigMap is emitted |

Detection is `deploy/scripts/detect-stack.py`. Run it on its own to see what a
cluster would give you, without installing anything:

```bash
deploy/scripts/detect-stack.py --namespace planner --format text
```

```
cluster: vps
  victoriametrics  present   http://planner-victoria-metrics.planner.svc.cluster.local:8428
                             fenced by NetworkPolicy planner-observability-ingress
  victoriatraces   absent    no pod runs a matching image
  collector        absent    no pod runs a matching image
  perses           present   http://planner-perses.planner.svc.cluster.local:8080
  postgres         present   http://planner-postgres.planner.svc.cluster.local:5432
  objectstore      present   http://planner-rustfs-svc.planner.svc.cluster.local:9000
  chatmodel        absent    no pod runs a matching image
  websearch        absent    no pod runs a matching image
```

### Why this is not just a set of flags

A second VictoriaMetrics beside an existing one is not a harmless duplicate. The
same workload's metrics end up split across two stores, and every dashboard
reading either one silently stops covering half the data. The failure has no
error message; it looks like a gap in a graph. So the default is to look, and
the flags are the override rather than the interface.

### What "present" means

A pod running a matching image, with a Service in front of it on the expected
port. Not a name — names are a convention — and not a Service on its own, which
may point at nothing. What is matched:

| | image contains | port |
|---|---|---|
| VictoriaMetrics | `victoria-metrics`, `vmsingle` | 8428 |
| VictoriaTraces | `victoria-traces`, `vtsingle` | 10428 |
| Collector | `opentelemetry-collector` | 4318 |
| Perses | `persesdev/perses` | 8080 |

Cluster-mode VictoriaMetrics (`vminsert`/`vmselect`/`vmstorage`) is deliberately
not matched: its write path is a different port and a tenant-scoped URL. Point
at it by hand with `AIWATCHER_VICTORIAMETRICS_URL`.

aiwatcher's own pods are excluded by the `app.kubernetes.io/part-of: aiwatcher`
label. Without that, the second `install.sh` would find the VictoriaTraces the
first one installed, decide it was external, and uninstall it.

### Overriding it

```bash
AIWATCHER_VICTORIAMETRICS_URL=http://vm.observability.svc.cluster.local:8428  # use this
AIWATCHER_VICTORIATRACES_URL=none                                            # there is none; install one
AIWATCHER_DETECT=off                                                         # do not look at all
```

An override is reported as `source: override` in the detection output, so what
you are looking at always says where it came from.

### The Collector is never reused automatically

It is detected and reported, and the install still creates its own. A Collector
that belongs to something else almost certainly has neither the VictoriaTraces
exporter nor the `attributes/redact` processor that drops `gen_ai.prompt` and
`gen_ai.completion` before anything is exported. Sending prompt text through a
pipeline that does not redact is not a thing to arrive at by inference.

To reuse one deliberately, having checked its config:

```yaml
collector:
  mode: external
  external:
    endpoint: http://otel-collector.observability.svc.cluster.local:4318
```

### The object store is never reused automatically either

For a different reason from the Collector's. There, reuse is unsafe; here it is
**undecidable**: nothing in the cluster says which credentials aiwatcher may
use, which bucket it may write, or whether it may create one. Detection reports
the store and prints the block you would need, with the pod selector already
filled in.

Reusing one deliberately:

```yaml
promptStore:
  mode: external
  bucket: aiwatcher-prompts
  external:
    endpoint: http://minio.storage.svc.cluster.local:9000
    createBucket: false   # true only if these credentials may create one
  credentialsSecret:
    name: minio-credentials
    accessKeyKey: MINIO_ROOT_USER
    secretKeyKey: MINIO_ROOT_PASSWORD
```

`mode: none` is also a real answer: every `/api/v1/prompts` route then answers
501 and the panel's Prompts tab says which variable is unset, rather than
showing an empty registry. What is *not* a good answer is leaving prompts on
the server's own volume in a cluster — that volume holds the write-ahead log,
which is a rolling window a retention policy may delete, and a prompt has to
outlive every run that used it.

**A NetworkPolicy for an external store is off by default and usually should
stay off.** Policies are additive, so an ingress rule attached to a store that
*no* policy currently selects narrows it from "accepts everything" to "accepts
aiwatcher only" — which would cut off whoever else was writing to that bucket.
`detect-stack.py` reports whether the store is fenced; only then does
`networkPolicy.allowEgressToExternalPromptStore: true` grant a path rather than
take three away.

---

### The two components nothing in this chart calls

`chatModel` and `webSearch` are the odd pair here. Everything else the chart
installs is something aiwatcher itself reads — the server writes prompts to the
object store, the Collector forwards to VictoriaTraces. These two are installed
*for the workloads in the namespace*: a llama.cpp server speaking the OpenAI
chat API, and a SearXNG with `formats: [json]` on, which are the two endpoints
`aiwatcher_agentic.openai_chat` and `aiwatcher_agentic.web_search` are written
against.

Both are off by default and both take the same three modes. Two consequences
follow from nothing in this release calling them:

**The endpoint goes in somebody else's values file.** `helm install` prints it;
the release that runs the agent is where it is read. There is no wiring for the
chart to do.

**The NetworkPolicy has to be told who may call them.** The server's ingest rule
admits any pod in the namespace, because publishing a span is cheap and every
workload does it. These two are not: one occupies the only decoder for the
length of a run, and the other makes requests to the public internet for
whoever reaches it. So `allowIngressFrom` is named per component, and the chart
**refuses to render** `mode: install` with `networkPolicy.enabled` and an empty
list — a model server no pod may reach is a mistake that otherwise shows up as
a timeout three layers away.

```yaml
chatModel:
  mode: install
  name: animica-agent-2b            # the alias a caller sends as `model`
  model:
    uri: https://…/model.gguf
    sha256: <64 hex characters>      # required: the fetch is decided by this
    fetchImage: { repository: ghcr.io/you/your-python-image, tag: v1 }
  allowIngressFrom:
    - podSelector: { matchLabels: { app.kubernetes.io/name: your-app } }
```

`fetchImage` has no default on purpose. The initContainer runs `python -m
aiwatcher_sdk.serving.fetch`, which streams the weights, verifies the digest on
the way past and skips the download when the volume already holds them — and
the three images this chart publishes are a Rust server, a panel and a query
service. An application that already builds a Python image with the SDK in it
names that one; the alternative was a fourth image to publish and version for
a job that is twelve lines of somebody else's container.

The same fetcher runs on a laptop. `aiwatcher_sdk.serving.llama_cpp` installs a
pinned `llama-server` build beside the weights, so a developer reproduces what
the node runs without Docker — release tag, asset name and SHA256 declared, no
call to the GitHub API, and nothing unpacked before its digest matches.

## Managed execution

Off by default. With `execution.store: none` the `/api/v1/executions` routes
answer 501 naming the variable, and the panel's Pipeline view still drives a
curation chain from the browser — which is what it did before managed runs
existed and what an editor wants anyway (ADR_0025). Turning it on gives you a
chain that runs on the server: the browser asks for a run and may then close.

```yaml
execution:
  store: postgres
  retentionDays: 90
  queryUrl: ""               # derived from query.enabled
  queryStepTimeoutSeconds: 300  # the engine's ceiling follows it, a minute above
  mlPipelineUrl: ""          # no default; see below

postgresql:
  mode: install
```

**Which store.** `file` is one directory of append-only files and holds *one
process* — it takes a lock at start-up and refuses a second by name, so it is
correct for a single replica and can never become a production store by
omission. The chart refuses it on an `emptyDir`, because there every managed run
is forgotten on a restart, including one that was still going. `postgres` is
what a deployment uses.

**Retention.** `retentionDays: 0` — the default — keeps everything. The stream
is the *explanation* of a run, and it is the one thing the event log does not
carry, so this is the only copy of it; a release that started deleting it on an
upgrade would be deleting something nothing else holds. When you do set it, set
it **longer than the event log's own retention**: the log is what redelivers a
message, and the inbox that recognises a redelivery is deleted with the stream.
The sweep runs in the work role, takes terminal executions only, and never takes
one whose facts the outbox has not yet published.

**The database is never reused automatically.** The object store's reason with a
sharper edge: what this release would do with a database it found is *create
tables in it*. Which database, under whose credentials, and whether that role
may create a schema are none of them discoverable from a matching image.
Detection reports one and prints the block you would need:

```yaml
execution:
  store: postgres
postgresql:
  mode: external
  database: aiwatcher
  username: aiwatcher          # a role that may create tables
  external:
    host: postgres.data.svc.cluster.local
    port: 5432
  credentialsSecret:
    name: aiwatcher-db
    passwordKey: password
```

The same NetworkPolicy warning applies, and harder: a database is the backend
most likely to already have other clients, so
`networkPolicy.allowEgressToExternalWorkflowStore` is worth turning on only when
`detect-stack.py` says something already fences it.

**Where the steps run.** A managed query step — `flow_php`, `datafusion` or
`duckdb`, whichever `query.engine` names — is the *server* reaching the query
engine directly, not the panel proxying a person's query — so
`query.enabled: true` is enough and the chart opens that path in the policy for
you. A `marimo` notebook step has no default and no template: that service runs
notebook code with no sandbox and no authentication, so it is a development
surface bound to localhost, and a cluster that wants notebook blocks has to name
one deliberately in `execution.mlPipelineUrl`. A process with no address for a
runtime registers no executor for it and therefore claims none of its work —
absence is a working state, not a failure.

### Splitting the API from the worker

`execution.splitRoles: true` renders a second Deployment. `serve` holds the API,
the read model and the object store; `work` holds the outbox and the reactors
and is the only role that opens a socket to the query engine or a notebook
runtime — so the pod behind the ingress stops holding those addresses and
those credentials. It has no Service, no ingress path and no probes, because it
opens no listener at all.

Off is not a lesser configuration: one process holding both halves is the
default and is what every development install is. The split costs three shared
backends, which is exactly what stops being per-process when the binary becomes
two processes — the workflow store (`execution.store: postgres`), the log
(`server.bus: laser`) and the object store (`promptStore.mode` other than
`none`). The chart refuses each by name at render time, which is the same
refusal `Config::validate` makes at start-up, arriving before the rollout
instead of during it.

```yaml
execution:
  splitRoles: true
  workers: 3
```

More than one worker is the point: an attempt is claimed by exactly one of them
under a lease, and one that dies has its lease expire and its work taken over.

### Upgrading its schema

The schema is applied at start-up, once, by whichever replica reaches it first —
there is no migration step to run and nothing to sequence by hand. What that
costs is a rule about what a migration may contain: **a release may not remove
something the release before it still names.** Workers roll rather than stop, so
for the length of a `RollingUpdate` the old process keeps serving against the
schema the new one has just changed, and an image rollback runs it against that
schema again. NULL in every row makes the *data* safe to lose and says nothing
about the query still naming the column.

So a removal is two releases: one that stops using the thing and leaves it in
place, and a later one that drops it, once no binary that names it can still be
running. Nothing here can enforce that at render time — it is a review question
about the file being added — and
`crates/aiwatcher-execution/src/store/postgres/schema.rs` states it beside the
list of files.

This release is the first half of one such pair, and needs nothing from you.
`execution_runs.started_at` and `ended_at` have never been written and are
answered from the log's fold instead (ADR_0026). Migration 0003 dropped them,
0005 puts them back, and a database that already ran 0003 — a development one
that has opened a build from this branch — gets them back when it is reopened.
They go for good in a later release.

**Rolling back.** Supported across this boundary, with no database work: the
schema an upgraded database ends at is one the previous release reads and
writes. What a rollback does not undo is the schema version itself — a version
is recorded once and skipped forever after, so a database a newer binary has
opened stays where that binary left it. Rolling back the image is the supported
direction; rolling back the schema is not, which is what the two-release rule
exists to make survivable.

`just test-postgres` is the evidence: upgrade from every earlier schema,
reopening one already migrated, the previous release's own statements against
the result, and a row written by each binary read by the other.

---

## Installing beside planner

planner runs VictoriaMetrics, VictoriaLogs, Perses and a RustFS on its k3s,
behind authentik, with a NetworkPolicy per component. aiwatcher goes into the
same namespace as a guest.

Two of those it borrows. VictoriaMetrics is derived by detection.
`planner-rustfs-svc` is not — `deploy/environments/planner.yaml` sets
`promptStore.mode: external` by hand, because the credentials Secret
(`planner-rustfs-secrets`, under planner's own `RUSTFS_ACCESS_KEY` /
`RUSTFS_SECRET_KEY` key names) is a thing a person knows and detection cannot.
`planner-web`, `planner-import-api` and `planner-mlflow` already write to that
store, which is also why no NetworkPolicy is attached to it: see above.

```bash
cd deploy
AIWATCHER_IMAGE=ghcr.io/you/aiwatcher \
AIWATCHER_PANEL_IMAGE=ghcr.io/you/aiwatcher-panel \
AIWATCHER_IMAGE_TAG=v0.1.0 \
AIWATCHER_IMAGE_PULL_SECRET=ghcr-pull \
./scripts/install.sh -e planner -n planner --plan     # look first
./scripts/install.sh -e planner -n planner            # then apply
```

For Planner, the installer also accepts its existing `IMAGE_PULL_SECRET`
variable as a fallback, so sourcing `deploy/.env.k3s` is enough for private
GHCR packages. The Secret itself must already exist in the target namespace.

What that install decides, on the cluster as it stands today:

* **VictoriaMetrics** — planner's, at `planner-victoria-metrics:8428`. Not
  installed again. OTLP metrics ingestion is on by default in vmsingle, so
  nothing changes on planner's side for the write path to work.
* **VictoriaTraces** — installed. planner has no trace store, and this is the
  one backend aiwatcher genuinely adds.
* **Perses** — planner's. The datasource ConfigMap is emitted; planner already
  mounts it (below).
* **NetworkPolicy** — planner's `planner-observability-ingress` restricts port
  8428 to its own observability components, so aiwatcher's Collector would be
  refused. The install adds one ingress rule to those pods for the Collector.

### That NetworkPolicy, in detail

NetworkPolicies are additive: a pod selected by two policies accepts the union
of what they allow. So aiwatcher can grant its Collector a path to planner's
VictoriaMetrics without editing planner's chart, and deleting the aiwatcher
release restores planner's own rules exactly.

The asymmetry that makes this safe in one direction only: a pod that **no**
policy selects accepts everything. Attaching a policy to such a pod does not
widen it — it narrows it, from "everything" to "aiwatcher's Collector only",
cutting off whoever was already talking to it. So the rule is only attached when
the target pods are **already fenced** by some ingress policy, which is what the
`fenced by NetworkPolicy …` line in the detection output is reporting. If
planner ever set `networkPolicy.enabled: false`, detection would stop reporting
the fence and the install would stop attaching the rule — correctly, because it
would no longer be needed.

The one case this cannot handle is a VictoriaMetrics in a **different**
namespace: a NetworkPolicy only applies in its own, so that rule has to be added
by whoever owns that namespace. The chart fails with that message rather than
rendering a policy that would do nothing.

### Wiring the datasource into planner's Perses

planner's Perses provisions from fixed ConfigMap mounts, not from the discovery
sidecar, so the `perses.dev/resource` label alone does not make it pick this up.
What planner does instead is keep a provisioning folder for exactly this, and
mount the ConfigMap there — `optional: true`, so a planner without aiwatcher
installed comes up unchanged. In
`deploy/helm/planner/templates/observability.yaml`:

```yaml
            - { name: aiwatcher-provisioning, mountPath: /etc/perses/provisioning-aiwatcher, readOnly: true }
```

```yaml
        - name: aiwatcher-provisioning
          configMap:
            name: aiwatcher-perses-provisioning
            optional: true
```

with `/etc/perses/provisioning-aiwatcher` listed in `provisioning.folders` in
planner's Perses config. The ConfigMap name is the contract between the two
repos: changing `perses.provisioning.tracesDatasourceName` here is free, but
changing the ConfigMap's own name means changing planner's mount in the same
breath.

The datasource is a Jaeger one: VictoriaTraces answers the Jaeger query API, so
Perses reads it with the plugin already in its image. Grafana's trace-to-metrics
link has no Perses equivalent and is gone — a span still carries
`gen_ai.request.model`, but the one click from a span to the metric filtered by
it does not exist. The planner environment installer restarts `planner-perses`
after the ConfigMap appears, so provisioning sees it without waiting out a
kubelet sync period.

### Publishing the panel

aiwatcher has no authentication of its own, so an ingress without planner's
authentik middlewares would put it on the public internet. That is why the
switch and the middlewares live in the same file: `environments/planner.yaml`
turns the ingress on and carries the same two middlewares every authenticated
planner route uses, plus the `/outpost.goauthentik.io/` route the forward-auth
callback needs — that path must skip the authentik middleware, because a route
that requires a session in order to establish one never completes. The default
environment leaves the ingress off, and a cluster with no such middleware to
attach should keep it off and reach the panel by port-forward.

The host is not in that file. Detection reads it off the cluster: planner
already publishes `planner.<domain>` and `perses.<domain>`, so aiwatcher goes
to `aiwatcher.<domain>`, and installing needs no host at all.

```bash
./scripts/install.sh -e planner -n planner
```

`just detect planner` prints what it derived, on the `domain` line. To name one
by hand instead — a different domain, or a cluster publishing under several:

```bash
AIWATCHER_DOMAIN=aiwatcher.example.com ./scripts/install.sh -e planner -n planner
```

Add a DNS record for the host first; cert-manager will not issue against a name
that does not resolve. Nothing is published on a host nobody derived and nobody
named: an environment with `ingress.enabled` and no host fails to render, rather
than installing a release that answers 404 on the host the SSO app points at.

---

## Signing in

The chart installs with `auth.mode: none`, which is what every release before
this one did: aiwatcher identifies nobody and refuses nobody, and whatever
guards the ingress is the only gate. Turning that on is two decisions.

**Which mode.** `proxy` where a forward-auth already sits in front — planner's
ingress sends every request through authentik's outpost, so aiwatcher can start
reading `X-authentik-username` and the group header for one line of values.
`oidc` everywhere else, and anywhere the namespace is not a boundary you would
defend: in `proxy` mode a header is a claim, so any pod that can reach port
8080 can claim to be an admin, and the chart refuses to render that mode
without `networkPolicy.enabled`.

**What producers do.** An agent posting to `/api/v1/events` reaches the Service
directly, never passes the ingress, and cannot complete an interactive
sign-in — so turning either mode on closes HTTP ingest until it has a token:

```bash
kubectl -n planner create secret generic aiwatcher-ingest \
  --from-literal=AIWATCHER_AUTH_INGEST_TOKENS="planner=$(openssl rand -hex 32)"
```

then set `auth.ingestTokenSecret.name: aiwatcher-ingest` and give every agent
the same value as `AIWATCHER_TOKEN`. Each token grants the editor role and
never admin. Do this **before** the release rolls, not after: the chart's NOTES
say so, and the symptom otherwise is every SDK logging a 401.

For `oidc` there is a second Secret, holding the client secret and the session
signing key:

```bash
kubectl -n planner create secret generic aiwatcher-auth \
  --from-literal=AIWATCHER_AUTH_CLIENT_SECRET=... \
  --from-literal=AIWATCHER_AUTH_SESSION_SECRET="$(openssl rand -hex 32)"
```

Without the session key the server generates one at start-up and warns, which
is safe and signs everybody out on every restart.

The authentik side — the provider, the application whose *slug* makes the
issuer URL, and the three groups — is
[deploy/authentik/README.md](../deploy/authentik/README.md), with a blueprint
beside it that creates all of them.

## The Query tab

Off by default, and a deployment without it is a supported state rather than a
broken one: the tab says the service is not running and names what starts it,
and the other three observability views do not know the difference.

```bash
helm upgrade aiwatcher deploy/helm/aiwatcher -n planner \
  -f deploy/environments/planner.yaml \
  --set query.enabled=true \
  --set query.engine=duckdb        # or flow (the default), or datafusion
```

or, through the install script:

```bash
AIWATCHER_QUERY=true AIWATCHER_QUERY_ENGINE=duckdb \
  AIWATCHER_QUERY_IMAGE=ghcr.io/you/aiwatcher-query-duckdb \
  deploy/scripts/install.sh -e planner
```

What that gets you is one of `services/query`'s engines, answering the questions
the explorer tree was not built for against the API's own routes: Flow, a Flow
DataFrame pipeline parsed and never executed (ADR_0008); or DataFusion or DuckDB,
each queried in its own Python API (ADR_0028) and measured over a 5 GB corpus at
a few seconds where Flow took minutes. One engine per release, and switching is
`query.engine` alone: the server's `AIWATCHER_QUERY_ENGINE` follows it, so a
pipeline written for another engine is shown and refused rather than run. Switch
between runs, not during one: an attempt of the old engine still pending or
retrying is one nothing claims any more, and it waits until its run is
cancelled. What matters at install time is four things.

**It is not part of the binary.** aiwatcher has no idea it exists. The panel
calls it directly, `query.enabled` only decides what the panel's nginx proxies
`/query` to (and `/flow`, for one release), and turning it off later leaves
nothing behind.

**It has no authentication of its own.** Flow's parser, and a Python engine's
`strict` admission, bound what a query can *say*, not who may ask — and under
`open` admission, the default, a DataFusion or DuckDB query is Python that runs,
in a child process with ceilings and without credentials. So the only things
that may reach it are the panel's nginx and the server's reactor, which puts it
behind whatever guards the panel's host. The chart keeps it that way: a
ClusterIP Service, no ingress path, and with `networkPolicy.enabled` a policy
that admits those two pods and an egress policy that lets the engine reach the
server and DNS and nothing else. Do not give it an Ingress of its own, and set
`query.admission: strict` where the panel is shared with people who should not
run code in the cluster.

**Its memory is the engine's.** Flow's limit is 384 MiB; DataFusion's and
DuckDB's are 1 GiB, which the 5 GB benchmark corpus stayed inside with room to
spare (`benchmarks/curation/README.md`).

**Its clock is a pair.** A managed query step runs for
`execution.queryStepTimeoutSeconds` (300 by default), and the engine's own ceiling,
`query.timeoutSeconds`, is derived a minute above it whenever managed execution is
on — so a long step is stopped by its own clock and retried, rather than refused by
the engine. With managed execution off the engine keeps its default of 30 seconds,
which is what a person at the Query tab waits on. Raise the step's for a corpus on
disk; set `query.timeoutSeconds` only to override the derivation.

**A query only sees the retention window.** It reads the read model through the
API, so a result is as current as the runs list and no older than it. The panel
says so above every table, because a partial result read as "all time" is worse
than no result.

If the engine already runs somewhere this chart does not manage, point at it
with `panel.queryUpstream` instead and leave `query.enabled` off. That field wins
over `query.enabled` when both are set. The names these values had while Flow was
the only engine — `flow.*`, `panel.flowUpstream`, `execution.flowUrl` — were read
for one release and are refused by name since, as are the server's
`AIWATCHER_FLOW_URL` and `install.sh`'s `AIWATCHER_FLOW` and `AIWATCHER_FLOW_IMAGE`.

## The pipeline engine

**Removed** on 2026-09-11 ([AW-4](specs/AW-4-retire-flyte-and-run-steps-in-pods-of-our-own/_index.md),
superseding [ADR_0016](ADR/ADR_0016_PIPELINE_ENGINE.md)). aiwatcher used to read
an external orchestrator for its registered launch plans and start one from the
panel. Its one user, planner, moved onto aiwatcher's own workflow engine, so
the engine, its routes and the panel's launcher are gone.

An installation that still configures it is refused rather than quietly
upgraded into one without it:

- a values file that still sets `engine` fails to render, naming `engine` —
  delete the block;
- a server started with `AIWATCHER_ENGINE` set to anything but `none`, or with
  `AIWATCHER_WORKFLOW_RUNNER` set to `engine` or `flyte`, refuses to start and
  says the engine was removed. `none`, and an empty variable, still start.

The `AIWATCHER_FLYTE_*` variables configure nothing now and may be left or
removed. The Workflows tab's rerun still goes to the webhook
`AIWATCHER_WORKFLOW_RUNNER=http` names. Work that used to be launched in the
orchestrator is registered as a workflow and started through
[managed execution](#managed-execution).

## Replacing MLflow

Install aiwatcher before deploying the Planner cutover. Planner no longer runs
both systems in parallel: once its workloads point at the server ingest URL,
the old tracking stack is removed in the same release. The measurements that
justified that decision remain in `docs/mlflow-comparison.md`.

What actually moves is in `planner-mlplatform`, which uses MLflow two ways:

1. `mlflow.pydantic_ai.autolog(log_traces=True)` in `app/observability.py`, for
   agent traces. This is the part aiwatcher replaces: the Python SDK
   (`sdk/python`) publishes to `POST /api/v1/events` on the server, which is on
   in both environments (`server.ingestEnabled`).

   ```
   AIWATCHER_URL=http://aiwatcher-server.planner.svc.cluster.local:8080
   ```

2. `start_run` / `log_params` / `log_metrics` / `log_dict` in
   `log_evaluation_run`, for evaluation reports. This is **not** a trace, and
   for a while it had nowhere to go. It does now:
   [ADR_0010](ADR/ADR_0010_EVALUATION_REPORTS.md) added evaluation reports as a
   first-class record that rides the same log and forms no span, and the panel's
   Evaluation area renders them.

   The swap in `app/observability.py` is the same four pieces:

   ```python
   def log_evaluation_run(*, run_name, parameters, metrics, report) -> str | None:
       client = aiwatcher_client()          # the one already built for tracing
       if client is None:
           return None
       try:
           return client.record_evaluation(
               suite=run_name,               # "dom-projekt-catalog-gate"
               dataset=f"catalog-cases@{len(CATALOG_CASES)}",
               params=parameters,
               metrics=metrics,
               report=report,
           )
       except Exception as error:           # a report must not fail the gate
           logger.warning("aiwatcher_evaluation_write_failed", error=str(error)[:500])
           return None
   ```

   Two things worth getting right rather than defaulting:

   * **`dataset` is what makes two reports comparable.** aiwatcher compares a
     report only against the previous one of the same suite *on the same
     dataset*. Passing nothing still works — an unversioned suite compares
     against itself — but the day `CATALOG_CASES` grows, a pass rate that
     "dropped" will just be a different set of cases, and nothing will say so.
   * **`report` is not redacted.** The Collector strips prompts and completions
     from spans; an evaluation forms no span, so whatever
     `house_catalog.py` puts in the report is stored verbatim. That is fine for
     scores and problem lists, and a decision worth making before it holds model
     output.

   Optionally, `client.evaluation(...)` publishes a case per catalogue project
   as it is scored, which is what fills the regression view — the cases that
   passed on the previous gate and fail on this one.

The release order is therefore: install aiwatcher, deploy the Planner cutover,
run one traced inference and one evaluation, then verify both in the panel.
Planner removes the complete old tracking surface: its Deployment and Service,
ingress, dependency, runtime configuration, authentication application, object
bucket and persistent-volume settings. No artifact store or registry from that
stack remains wired into Planner.

DeepEval stays because it is the evaluation harness, not a tracking backend.
aiwatcher records the *result* and has no opinion about how it was produced —
no scorers, no judges, no suite runner.

---

## Durable evaluation evidence

Off by default (`evaluationEvidence.enabled`). Reports that ride the event log
are bounded by its retention; the evidence behind a decision is not, so
[ADR_0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md) gives it its own store, its own
retention and its own admission. Turning it on adds no backend: the evidence
goes into the bucket the prompt registry already uses, under an `evaluations/`
prefix, so there is no new service, no new credential and no new network path
for a NetworkPolicy to open. `promptStore.mode: none` means no registry at all,
and every `/api/v1/evaluation-results` route then answers 501 naming the
variable rather than an empty list.

What it does add is a directory the server reads. A published result never
copies the source it was measured against — a conversation corpus, an
annotation export, a curated dataset all stay with their owners and keep their
own deletion rules — but the *producer's* side of a measurement (the suite, the
scorer, the input and expectation schemas, the case manifest, and a model's
package where there is one) exists nowhere else, so an operator puts it where
the server can verify it:

```yaml
evaluationEvidence:
  enabled: true
  sourceDir: /etc/aiwatcher/evaluation-approvals
  volume:
    configMap: { name: evaluation-approvals }
```

One subdirectory per approval, **named by the pair it admits**. That name is a
digest of the variant and context IDs, so nobody types it:

```bash
cargo run -q -p aiwatcher-evaluation --example prepare -- manifest.json
# → variant_id, context_id and the approval_id its directory is named after

scripts/stage-evaluation-approval.py ./approvals ./my-bundle
```

The naming is the whole point of it being a directory of approvals rather than
one bundle. A second variant is a second subdirectory: admitting it cannot hide
the first, which is what an A/B comparison needs and what a single approved
bundle could not give.

Admitting one is then an API call, and it is `admin`:

```bash
curl -X POST "$AIWATCHER/api/v1/evaluation-approvals" \
  -H 'content-type: application/json' --data-binary @manifest.json
```

`admin` rather than `editor` because an ingest token is an editor by
construction — a producer that could admit its own evidence is not an approval.
After that, every publication of that pair — from a worker, a schedule or a CI
gate — needs nothing on this host. `DELETE /api/v1/evaluation-approvals/{id}`
withdraws one: every result measured under that pair stops being readable and
no new one can be published, and nothing already published has its retention
moved in either direction. Withdrawal is final for that approval ID.

Evidence disappears three ways, and no fourth: its approval is withdrawn, its
*source* is deleted or revoked at the owner (which the sweep enforces within
the hour and a read enforces immediately), or its retention runs out —
`limits.retentionDays`, 30 by default, and a source's own retention can only
shorten it. `DELETE /api/v1/evaluation-results/{id}` is the narrow case of the
first: one measurement rather than every measurement of its pair.

### What a restore of that prefix means

`evaluations/` is one of the three stores in a cluster whose contents exist
nowhere else — the conversation archive and the execution stream are the other
two — and it is the only one whose correctness rests on an object *not* coming
back. Publication and collection race at one immutable key: whichever creates
`claim.json` first decides whether that logical ID ever publishes. Restoring
the prefix from a backup can therefore put back a claim a collector already
abandoned, or drop a tombstone that was the record of an erasure.

So restore it whole, to the state of one moment, and never merge two points in
time. Restoring `content/` without the `claim.json` beside it produces bytes
nothing points at; restoring a claim without its tombstone republishes evidence
somebody deleted. After any restore, run one collection pass — the hourly half
of the retention worker — and read `retention` on `GET
/api/v1/evaluation-results` to see it complete.

### Whether retention is running

That same `retention` field is the answer. It carries the last pass: when it
ran, how much it retired and collected, and `failures` — the number of
consecutive failed passes. A sweep that has been failing for a week otherwise
looks exactly like one that had nothing to do, which is why the field is a
durable object rather than a log line: it survives a restart and every replica
reads the same one.

---

## The install script

```
./scripts/install.sh [-e ENV] [-n NAMESPACE] [--plan] [-y]
```

### Its defaults live in .env

With a `.env` at the repo root, the script takes no arguments. Copy
`.env.example` and fill in the three image lines:

```env
AIWATCHER_ENV=planner
AIWATCHER_IMAGE=ghcr.io/<owner>/aiwatcher
AIWATCHER_PANEL_IMAGE=ghcr.io/<owner>/aiwatcher-panel
AIWATCHER_QUERY_IMAGE=ghcr.io/<owner>/aiwatcher-flow
AIWATCHER_IMAGE_PULL_SECRET=ghcr-pull
```

```bash
deploy/scripts/install.sh
```

The justfile has loaded that file into every recipe from the start
(`set dotenv-load := true`); the script reads it too, so `just install-cluster`
and a bare `./install.sh` now resolve to the same install. A variable already
exported in your shell wins over the file, and a flag wins over both. The file
is parsed rather than sourced — this script applies manifests to a cluster, and
`source` on a file of settings runs whatever is in it.

Leave `AIWATCHER_IMAGE_TAG` out. CI publishes each image under the commit SHA
and nothing else, so when a registry is named and a tag is not, the tag is the
checkout's own commit — the one whose build produced those images. The
environment files say `tag: latest`, which no build ever pushes; that is why
naming a registry without a tag used to be a pull of something that does not
exist.

### The preflight

It refuses to go further than a preflight when:

* `kubectl`, `helm`, `helmfile` or `python3` is missing — with the install line
  for each;
* the cluster cannot be read well enough to detect what is on it. Installing
  anyway would risk exactly the duplicate this whole mechanism exists to avoid.

And it asks before applying to any context that is not a known-local one
(`orbstack`, `docker-desktop`, `minikube`, `colima`, `rancher-desktop`, `kind-*`,
`k3d-*`, `k3s-*`). This kubeconfig has production contexts in it and
`helmfile apply` is not a command that asks. `-y` skips the question; nothing
skips the preflight.

`--plan` runs `helmfile diff` and applies nothing.

## Prerequisites

```bash
mise use -g helmfile        # or: brew install helmfile
```

`kubectl`, `helm` and `python3` you already have. The detection script is
standard-library Python — no `jq`, no `pip install`.

The `helm-diff` plugin is optional. With it, `--plan` shows a diff against what
is running and the install goes through `helmfile apply`. Without it, `--plan`
prints the full manifest and the install goes through `helmfile sync` — the same
manifests, no pre-flight diff. It is not a hard requirement because its Helm 4
support is its own moving target, and that should not stand between you and an
install:

```bash
helm plugin install https://github.com/databus23/helm-diff
```

## Images

```bash
REGISTRY=ghcr.io/you TAG=v0.1.0 deploy/scripts/build-images.sh --push
```

Three images: `aiwatcher` (the Rust binary), `aiwatcher-panel` (nginx plus the
built React app) and `aiwatcher-flow` (Flow, the default query engine). Separate,
so a panel change does not rebuild the Rust binary and neither rebuilds the PHP.
`--no-flow` skips the third for a deployment that will not run it. The other two
engines are targets of the same `deploy/Dockerfile.query` — `docker build -f
deploy/Dockerfile.query --target datafusion .`, or `--target duckdb` — and the
release workflow publishes all three: `aiwatcher-flow`,
`aiwatcher-query-datafusion` and `aiwatcher-query-duckdb`.

The panel's nginx config is not in its image — it comes from a ConfigMap in the
chart, because it has to name the server's Service, which is a deployment-time
fact.

## Uninstalling

```bash
deploy/scripts/uninstall.sh -e planner -n planner
```

Volumes are kept. `--delete-data` removes them, after asking. Nothing that was
borrowed is touched, and the one NetworkPolicy attached to someone else's pods
goes away with the release.

## Without helmfile

The chart is a normal Helm chart and the detection script emits chart values
directly, so this is the same decision by hand:

```bash
helm upgrade --install aiwatcher deploy/helm/aiwatcher \
  --namespace planner --create-namespace \
  --values deploy/environments/planner.yaml \
  --values <(deploy/scripts/detect-stack.py -n planner --format helm-values)
```

The mapping from "what is on the cluster" to "what to install" lives in
`--format helm-values`, in one place, so this and the helmfile cannot drift.
