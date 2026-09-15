# IAM control plane — first part of IAM-01

This crate implements organization membership, local teams and project grants.
It authorizes mutations of **its own IAM metadata**. The server now exposes an
optional IAM API with a transactional audit trail. The dataset, prompt,
training/model, annotation, authored evaluation, workflow definition and case
review registries and native cohort derivation now have project-scoped HTTP
adapters, described below. Legacy data, logs, other artifacts, streams, query services and execution
workers are not yet isolated.
No organization/project selector should be activated on the strength of this
crate alone.

The identity provider authenticates a person or service account. AIWatcher owns
organization/project membership; IdP groups are not copied into teams. A
principal is the exact `(provider, subject)` pair. Email, display name and
instance role are not part of its identity or project authorization.

`Authenticator::iam_principal` in `aiwatcher-auth` constructs this pair from a
verified OIDC session or bearer identity and checks its issuer and expiry. New
session cookies preserve the verified issuer; the API's `Identity.issuer` field
is additive and optional. Issuer-bound cookies are refused after switching to
a different provider, even if the session signing secret remains the same.
Older cookies can still use the legacy API, but must sign in again to obtain an
IAM principal. Session identities use the session TTL; bearer identities retain
their token expiry. Anonymous, local, static ingest, attempt and proxy identities
are not assigned a guessed provider namespace. Their eventual scoped adapters
are a separate integration step.

## Policy

| Authority | Operations |
| --- | --- |
| Organization owner | Manage all memberships; create teams/projects; manage project grants. |
| Organization admin | Manage ordinary members and teams; create projects; manage project grants. Cannot promote or change admins/owners. |
| Organization member | Membership alone provides no project access or organization administration. |
| Project admin | Manage grants for that project to existing organization members or teams. |
| Project editor | Project write role; cannot manage grants. |
| Project viewer | Project read role. |

An owner/admin has **no implicit project read/write grant**. Creating a project
atomically adds an explicit, removable admin grant to its creator. Organization
administrators can recover access by issuing a new explicit grant. Instance
administrators must not be passed through as organization/project admins; the
API separately authorizes organization creation and invokes
`create_organization` with the verified first owner.

At least one owner must remain. Removing a member removes their direct grants
and team memberships; adding the member again does not resurrect those rights.
Removing a person from a team leaves independent direct grants intact. Deleting
a team removes its grants. Teams and grants cannot reference another
organization. Project access is checked against the full
`ProjectScope { organization, project }`, including for a guessed or reused ID.

The effective project role is the maximum over all active direct/team grants.
Time is Unix seconds, with half-open intervals:

- Before `valid_from`: no contribution.
- At `valid_from`: the declared role becomes active.
- At `edit_until`: that grant becomes Viewer, while read access remains.
- At `read_until`: that grant stops contributing.
- An absent end is unlimited; specifically, absent `read_until` retains read
  access after a finite edit period ends.

Expiry/revocation of one source does not remove an independent active source.
`ProjectAccess.grants` reports each contributing grant, its ID, grantee, window
and current role, so a future workshop UI can explain remaining access. No
workshop enrollment or provisioning is implemented here. Delegating project
admin is administrative authority to issue grants, including permanent ones;
workshop participants should receive editor/viewer grants, not admin authority.

`ProjectAccess` is a snapshot with `evaluated_at`, not a bearer capability or a
permission to cache access for the SSO session. Every new operation needs a
fresh decision. Long-lived streams and jobs will need revocation/expiry checks
and shutdown semantics when the data plane is integrated.

## Storage and transactions

`MemoryIamStore` is the test/development adapter. `PostgresIamStore`, behind the
`postgres` feature, supports concurrent server processes. Both implement
`IamStore` and run the same policy inside their write lock. The clock is sampled
after the lock is acquired: a grant that expires while a write waits does not
authorize that write. Invalid commands commit no partial changes.

PostgreSQL stores one versioned JSONB aggregate per organization in
`iam_organizations`. Memberships, teams, projects and grants share one
consistency boundary. `SELECT … FOR UPDATE` serializes control-plane mutations
within one organization; unrelated organizations can write independently. A
GIN index on the authoritative membership array supports exact provider/subject
organization discovery without a second membership table that could drift.

This is a bounded metadata store: at most 4 MiB of serialized metadata per
organization, and each mutation validates/writes the aggregate. It is suitable
as the first transactional IAM foundation, not for high-volume per-artifact
ACLs. Normalize/index further before workloads exceed that boundary. No
telemetry, resource contents, invitation tokens or secrets belong in the JSON.
Successful organization creation and commands append to `iam_audit` in the same
transaction. Failure to write the audit entry rolls back the state change too.
Each entry carries the verified actor, organization, trusted time, command and
result, with a monotonically increasing sequence within its organization. Failed
commands do not appear as successful changes. This is a mutation history, not an
audit of rejected requests or data access. No HTTP route updates/deletes entries;
it is not a tamper-evident ledger against database administrators.

Audit reads require current organization owner/admin membership, checked with
the page read under a shared lock. Pagination uses `after` (exclusive sequence,
default 0) and `limit` (1–100, default 50). Audit storage is separate from the
4 MiB metadata limit and grows with administrative mutations; retention/export
is not implemented. Migration 0002 adds the table without rewriting existing
IAM documents or inventing events for changes before audit was available.

Schema migrations use a dedicated advisory lock and `iam_schema_migrations`.
They are atomic, repeatable at startup and independent of workflow-store
migrations. A newer database/document version or broken cross-reference fails
closed. Existing documents are not rewritten on read. Released migration files
must remain immutable; future changes add a migration. Enabling the server's IAM URL runs these migrations at startup. Validation for
this milestone used only disposable test databases; no user resource data has
been migrated.

## Optional server API

`AIWATCHER_IAM_POSTGRES_URL` enables the IAM metadata control plane and project
registry authorization. Omit it to leave those endpoints disabled (501 after the authentication layer). The server
requires `AIWATCHER_AUTH_MODE=oidc` and the `aiwatcher-server/postgres` Cargo
feature; it refuses other auth modes or a missing feature instead of falling
back to memory. Build, for example, with:

```sh
rtk cargo build -p aiwatcher-cli --features aiwatcher-server/postgres
```

Configure the existing issuer/client/redirect/session settings and point the
IAM URL at a dedicated development PostgreSQL database. Startup connects with a
five-connection pool and runs additive IAM migrations. This setting is separate
from the workflow-store URL. No production configuration is changed by this
implementation. To disable the API again, remove the IAM URL and restart; keep
the tables. Older binaries that understand only schema 1 refuse schema 2 rather
than silently discarding audit semantics.

| Method and path under `/api/v1/iam` | Meaning |
| --- | --- |
| `GET /organizations` | Only organizations containing the verified principal. |
| `POST /organizations` | Instance admin creates an organization with themselves as first owner. Body: `{"name":"Example"}`. |
| `POST /organizations/{organization}/commands` | Apply a typed membership/team/project/grant command using current IAM roles. |
| `GET /organizations/{organization}/projects` | Only projects with an effective grant for this caller. |
| `GET /organizations/{organization}/projects/{project}/access` | Fresh grant decision and its sources, including expiry. |
| `GET /organizations/{organization}/audit?after=0&limit=50` | Successful mutations, organization owner/admin only. |

All mutations require `X-AIWatcher-IAM: 1` in addition to JSON and authentication.
This non-simple header prevents cross-origin form writes using a session cookie;
cross-origin JavaScript must pass the deployment's explicit CORS policy. There
is no wildcard CORS with OIDC. Responses use `Cache-Control: no-store`; an access
response is never a durable capability. OpenAPI and the generated panel client
include these operations, but no UI selector or administration view is enabled.
Unknown top-level command/creation fields are rejected, including attempts to
provide an actor or bootstrap owner. Instance role/group claims do not grant
access to another organization's metadata. Outside-scope resources return 404,
insufficient roles 403, last-owner conflicts 409 and storage failures a generic
503 without exposing database details.

Examples of command bodies (IDs and principals must already belong to the
organization; adding a member is a separate explicit operation):

```json
{"type":"create_project","name":"Workshop"}
```

```json
{"type":"set_member","principal":{"provider":"https://idp.example/oidc","subject":"stable-subject"},"role":"member"}
```

Organization creation has no invitation/email or arbitrary owner field. Instance
administrators can create organizations, but cannot read another organization
merely because of that instance role. The API derives the actor from a verified
OIDC session/bearer, never from a request body.

## Verification

The memory and PostgreSQL adapters share nine contract scenarios. The database
suite also checks reconnect/migration idempotence, concurrent owner removal,
concurrent member removal and granting, expiry while waiting for a row lock,
preservation of unknown/inconsistent documents, audit rollback on database
failure, concurrent audit ordering and the schema-1 upgrade. The API tests run
signed sessions through the real authentication middleware with a loopback OIDC
discovery/JWKS service; they do not inject a trusted caller into handlers. They
check cross-organization refusals, bootstrap, stale/foreign sessions, forged
actor fields, mutation headers, grant expiry/revocation and audit pagination.

```sh
rtk cargo test -p aiwatcher-iam -p aiwatcher-auth
rtk cargo clippy -p aiwatcher-iam -p aiwatcher-auth --all-targets --all-features -- -D warnings

# Explicitly opt in using a disposable PostgreSQL database. Tests create IAM
# tables and organization records, including intentionally invalid test records.
rtk proxy env AIWATCHER_IAM_TEST_POSTGRES_URL=postgres://user:password@127.0.0.1:5433/test \
  cargo test -p aiwatcher-iam --features postgres --test postgres -- --ignored
```

Database tests are explicitly ignored without that command, never silently
reported as passed when PostgreSQL is unavailable. The auth suite uses a local
OIDC provider with real RS256 signatures to verify issuer/subject binding and
legacy-session behavior.

## Next integration gates

1. Implement scope enforcement across registry/object-store keys, read models,
   event ingest, search/query/export, caches, SSE/WebSocket, runtimes, scheduled
   jobs, workers and their credentials. A scoped URL or panel filter alone is
   insufficient. `auth=none` remains an explicitly local mode.
2. Give existing resources ownership through a dry-run manifest and verified
   migration, retaining content hashes, immutable versions and references.
   Unassigned resources must not fall back to global access.
3. Verify cross-project reads and writes through real HTTP and service
   boundaries, plus existing streams/jobs after revocation. Only then activate
   the organization/project UI and continue invitations/workshop access.

Entry points for the enforcement work are `aiwatcher-api/src/auth.rs`,
`iam.rs`, `project_scope.rs` and `state.rs`; registry modules in `aiwatcher-api/src`; `aiwatcher-projector`'s read
model/live hub; `aiwatcher-server/src/wiring.rs` and execution workers; and the
separate `services/query` and `services/ml_pipeline` runtimes. The dataset, prompt, training, annotation, authored evaluation and workflow definition adapters below are scoped data-plane slices;
other adapters remain unscoped. This crate's tests are policy/storage tests, not a
claim that those resource boundaries are already isolated.


## First resource boundary: project dataset registry

The server also exposes the existing dataset/curation operations below
`/api/v1/orgs/{organization}/projects/{project}`:

| Resource suffix | Operations |
| --- | --- |
| `/datasets` | List or publish an immutable dataset version. |
| `/dataset-rows` | Read/search/page an exact version (existing query parameters). |
| `/dataset-samples` | Publish explicitly labelled limited output. |
| `/curations` | List or save recipes. |
| `/curation-pipelines` | List or save pipelines. |
| `/curation-pipelines/{name}/revisions/{revision}` | Read a pinned pipeline. |
| `/curation-library` | Search or save block templates in this project. |

Both route families share handlers and body schemas. The scoped family uses
verified OIDC issuer/subject and a fresh IAM decision for **each** operation.
Viewer can read; editor/admin can write, regardless of the instance role.
Organization ownership alone confers no access. Writes require
`X-AIWatcher-IAM: 1` and recheck the grant after JSON upload, before calling the
registry. A revoked or expired grant is not cached for the SSO session. Responses
use `Cache-Control: no-store`. Missing/wrong scopes return 404, insufficient
project role 403, invalid scope syntax 400, unavailable IAM 503, disabled IAM or
registry 501. No fallback to instance roles, anonymous mode or legacy data.

All four registry categories use keys under
`<registry-prefix>/scopes/<organization-uuid>/<project-uuid>/registry/`.
Scope lives outside content bytes and hashes. Identical content in two projects
has the same version ID but must be published/stored separately; knowing that
ID does not grant access. New registry instances reopen the same namespace.
Recipes, pipeline pins, sample labels and provenance keep their existing
representations. `Registry::for_project` accepts typed IDs and refuses rebinding
to a different scope. It is a storage boundary, **not an authorization API**;
trusted non-HTTP callers must make their own fresh grant check.

This is an additive stage before cutover. **Legacy endpoints still serve only
legacy data under instance authorization.** They cannot enumerate/read project
registry keys, including after disabling IAM. Unassigned legacy resources are
not visible through a project route. No resource was automatically assigned,
copied or removed. Other services and the current panel still use legacy APIs;
no scoped query, notebook execution, schedule, evaluation or lineage resolution
is enabled by saving a scoped resource. References such as `execution_id` are
stored provenance, not proof of a scoped execution or authorization to read it.

Authorization of these bounded operations occurs at request admission (and
again after upload for writes). It does not cancel an object-store operation
already admitted before revocation, nor make IAM and object storage one atomic
transaction. Stream/job cancellation and the broader cutover policy remain
separate gates. Do not describe the whole deployment as multi-tenant safe yet.

### Read-only migration inventory

Run the operator tool against an **existing filesystem snapshot** and explicit
target UUIDs. Example shape (replace all four arguments):

```sh
rtk cargo run -p aiwatcher-datasets --example migration_manifest -- \
  /absolute/path/to/object-store-snapshot datasets ORGANIZATION_UUID PROJECT_UUID
```

It prints schema-1 JSON to stdout: target scope, source prefix, object counts for
collections/recipes/pipelines/library, sorted source-to-target keys, byte sizes,
SHA-256 of original bytes, and target state `absent`, `identical` or `conflict`.
Heads and all historical revisions are included. Known structured provenance,
version and notebook references are inventoried verbatim; row contents are not
included. Scoped keys are excluded from the legacy source inventory. The tool
neither republishes nor modifies source/target bytes. A malformed JSON object or
an object disappearing during the scan fails the dry run. Repeating a dry run
against unchanged bytes returns the same manifest.

This is an **inventory**, not the migration executor or the complete cutover
manifest: it does not validate all domain references or arbitrary query-text
dependencies, resolve group mappings, confirm that target UUIDs exist in IAM,
freeze a live store, or apply/resume copies. Operators must review conflicts and
references against the intended target; references are not rewritten. A future
executor must verify the snapshot/hashes, refuse conflicts, preserve every byte,
validate references and counts, and support interrupted/idempotent execution
before a write freeze and cutover can be authorized. The CLI currently reads a
filesystem snapshot; the library uses the `ObjectStore` port.


## Second resource boundary: project prompt registry

All eight prompt operations are also served below
`/api/v1/orgs/{organization}/projects/{project}`:

| Resource suffix | Operations |
| --- | --- |
| `/prompts` | List/search/page or publish a version. |
| `/prompts/{name}` | Read the head and its current prompt text. |
| `/prompts/{name}/versions/{version_id}` | Read an exact historical version. |
| `/prompts/{name}/labels/{label}` | Move a label to a version stored in this project. |
| `/prompts/{name}/optimizations` | Record optimization against a baseline in this project. |
| `/prompts/{name}/optimizations/{optimization_id}` | Read the record and its report. |
| `/prompts/{name}/rebuild` | Rebuild the head from this project's stored objects. |

These routes and dataset routes share current OIDC/IAM admission in
`project_scope.rs`. Viewer reads; editor/admin writes, including label movement
and rebuild. Organization ownership and instance administration confer no
implicit project access. Each mutation requires `X-AIWatcher-IAM: 1`; JSON
mutations recheck the grant after receiving the body. Route responses use
`Cache-Control: no-store`. Existing legacy routes retain instance authorization.
The OpenAPI document and generated panel client expose both route families.

`aiwatcher_prompts::Registry::for_project` binds all operations to
`<prompt-prefix>/scopes/<organization-uuid>/<project-uuid>/registry/`.
It preserves the configured size/index limits and refuses rebinding to another
project. Keys cannot leave that namespace. Prompt names cannot contain path
separators, so legacy enumeration excludes these objects and an encoded name
cannot make the legacy API reach project storage. A legitimate legacy prompt
named `scopes` continues to work.

Scope is outside the stored content. Text hashes, immutable version metadata,
labels, optimization IDs, reports and literal references keep their existing
representations. Identical text published independently into two projects has
the same version ID and separate stored objects. A known hash, optimization ID
or baseline from another project does not authorize reading, promotion or
optimization. Promotion still uses the existing admission policy.

This stage does not bind the panel, SDK prompt readers, runtime caches,
execution context or evaluation services to projects. Optimization dataset and
evaluation references, and prompt parent/model metadata, remain stored claims;
they are not resolved through a global fallback or validated as project lineage.
The dataset migration inventory above still covers datasets/curation only;
a prompt inventory, verified migration executor and cutover remain pending.
Disabling IAM does not expose project prompts through the legacy routes.
The bounded-operation admission limitation described for datasets applies here
as well; this is not cancellation of an already admitted storage operation.

Verification includes the real Axum router with signed OIDC sessions and the
loopback discovery/JWKS fixture, all eight operations, cross-project and
cross-organization refusals, known hashes, labels/reports, grant intervals and
revocation, mutation headers, delayed JSON uploads and disabled IAM. File-store
tests reopen project storage and verify original version metadata and
optimization reports, unchanged hashes and isolated legacy enumeration. These
are not end-to-end tests against Authentik, PostgreSQL, S3 or runtime services.

```sh
rtk cargo test -p aiwatcher-api -p aiwatcher-prompts -p aiwatcher-datasets
rtk cargo clippy -p aiwatcher-api -p aiwatcher-prompts -p aiwatcher-datasets --all-targets -- -D warnings
```


## Third resource boundary: project training and model registry

Nine operations are also served below
`/api/v1/orgs/{organization}/projects/{project}`:

| Resource suffix | Operations | Minimum project role |
| --- | --- | --- |
| `/training-runs` | List/filter runs or open a run. | Viewer / editor |
| `/training-runs/{run_id}` | Read a run, curve, checkpoints and profiles. | Viewer |
| `/training-runs/{run_id}/progress` | Append a batch of progress. | Editor |
| `/training-runs/{run_id}/finish` | Record a terminal status. | Editor |
| `/models` | List models or register a version from a local run. | Viewer / editor |
| `/models/{name}` | Read head and selected/current version metadata. | Viewer |
| `/models/{name}/labels` | Move a model label. | **Admin** |

The shared resource admission retains the operation's required role for its
post-JSON check. A surviving editor grant cannot authorize a label write after
an admin grant expires during upload. Instance roles do not substitute for
project grants; legacy writes still require instance editor, and labels still
require instance admin. Mutation headers and no-store responses match the other
project registries. Label writes retain the existing immutable-dataset-reference
and held-out-measurement checks; project admin does not bypass these checks.

`aiwatcher_training::Registry::for_project` selects
`<training-prefix>/scopes/<organization-uuid>/<project-uuid>/registry/` for all
run records, summaries, model heads and immutable versions. It refuses rebinding.
Run IDs and model names can repeat across projects. Registration reads the run
from the selected registry, never from another project or legacy storage. Scope
is outside model hashes and stored metadata; independent identical registrations
keep the same hash. Reopening the file store preserves curves, profiles, model
versions, labels and references. Unassigned legacy data stays separate, including
after disabling IAM. Traversal checks also apply to legacy keys and model-version
parameters, so a crafted old URL cannot reach project objects.

The existing model-detail contract is preserved: a known model with a missing
requested version returns its local head without `current`; it does not fall
back to another version or another project's content. A missing model/run or an
attempt to promote a missing version returns 404.

This is registry metadata isolation, not isolation of a running training process
or artifact bytes. Dataset, workflow, checkpoint/profile URI and package artifact
references retain their original representations. Dataset access and artifact
ownership are not validated by these routes, and external artifact bytes are not
fetched. Panel, SDK trainers/serving readers, machine credentials, workers, caches,
job cancellation, migration inventory/executor and cutover remain pending. No
job starts or stops when these record endpoints are called. The same bounded
storage-operation admission limitation as the other registries applies.

Tests cover all nine HTTP operations using signed OIDC sessions, same IDs across
projects/organizations, local provenance, role/header enforcement, expired and
revoked grants, admin expiry with independent editor access, delayed uploads,
legacy compatibility and disabled IAM. File-store tests reopen real namespaces,
verify model hashes and curves, and reject traversal through both route families.
These are not full E2E tests with PostgreSQL, Authentik, S3 or a GPU/runtime.


## Fourth resource boundary: project annotations and image bytes

Fourteen local operations are also served below
`/api/v1/orgs/{organization}/projects/{project}`. The IAM project in that path is
separate from the annotation collection name passed in `project` or `name` query
parameters and request bodies. Collections may retain names with slashes and
may have the same name in different IAM projects.

| Resource suffix | Operations |
| --- | --- |
| `/annotation-projects`, `/annotation-project` | List, save and inspect annotation collections. |
| `/annotation-images`, `/annotation-image` | List/filter/page, register and inspect images/revisions. |
| `/annotation-revisions`, `/annotation-reviews` | Save drawings and record review against an exact revision. |
| `/annotation-exports`, `/annotation-export`, `/annotation-export/coco` | List, build and read immutable exports and derived COCO. |
| `/annotation-blobs`, `/annotation-blobs/{image_id}` | Upload and read the actual image bytes. |

Viewer reads; editor/admin writes. All project mutations require the IAM header
and a fresh grant decision after receiving JSON **or binary bytes**. Authorship
and reviewer still come from the authenticated caller. A scoped image response
uses `Cache-Control: no-store`, overriding the legacy immutable cache policy;
its next read must pass current IAM authorization after revocation or expiry.
An admitted bounded storage operation is not retroactively cancelled.

The backend namespace is
`<annotation-prefix>/scopes/<organization-uuid>/<project-uuid>/registry/`.
It contains collection schemas, image heads, immutable revisions and exports,
and **image blobs plus content-type sidecars**. Deduplication shares a blob
between annotation collections only within that IAM project. A known digest
cannot read a blob from another IAM project or legacy storage. Independently
uploading the same bytes produces the same digest and a new stored copy.
`Registry::for_project` refuses rebinding, and all backend reads/writes/listings
check storage keys. It is a storage adapter, not an authorization service.

Local registration of an `aiwatcher://blob/<digest>` URI requires the matching
image ID and blob bytes in the same project. External URIs remain literal
metadata and are not fetched or approved by registration. The blob URI itself
keeps its existing representation: clients must resolve it against their
selected IAM project rather than the legacy blob route. The current panel and
SDKs do not yet perform this scoped resolution.

Schema, revision and export identities remain unchanged. Stored authors,
rights, review pins, splits and references retain their original values;
COCO is derived from that registry's pinned revisions. A review in one IAM
project does not alter another, and a known foreign revision/export cannot be
read or accepted. The verified COCO library read uses the same scoped backend,
but consumers such as training/evaluation still need their own scope wiring.

`/annotation-imports` (synchronous remote hydration), the source catalogue,
staged-import APIs and workers remain on their existing legacy paths. No scoped
alias to these external operations is exposed. Import jobs need scoped
credentials, grant rechecks during work and cancellation semantics before they
can join this API. Binding the registry's storage does not authorize an import
worker or make its configured image source project-aware. Existing migration
inventory still covers datasets/curation only; annotation inventory, copying
and cutover are pending. No user data has been migrated or deployed.

Verification covers all fourteen operations through signed OIDC sessions and
the real router, separate organizations/projects with the same collection name,
known hashes and pins, binary contents and no-store responses, role/header
checks, timed/revoked grants, delayed JSON and binary uploads and disabled IAM.
File-store tests reopen scopes, compare immutable hashes and original revision
and manifest contents, read actual image bytes and verify derived COCO. No full
E2E with Authentik, PostgreSQL, S3, remote imports or runtime workers was run.


## Fifth resource boundary: project forms, assessments and scorecards

Eleven authored evaluation operations are also served below
`/api/v1/orgs/{organization}/projects/{project}`:

- `GET/POST /evaluation-rubrics`, `GET /evaluation-rubrics/{name}` with an optional version;
- `GET/POST /evaluation-assessments`, `GET /evaluation-assessments/{target_id}/{standing_id}` with history pagination;
- `GET/POST /evaluation-scorecards`, `GET /evaluation-scorecards/{name}`, its `/versions` and `/diff`.

They share the original handlers and schemas. A current project viewer can
read; editor/admin can publish and assess. Instance roles and organization
ownership do not replace a project grant. Scoped writes require
`X-AIWatcher-IAM: 1` and repeat authorization after reading JSON. Authorship is
derived from the authenticated session, including `recorded_by` for judge
assessments. Scoped responses use `Cache-Control: no-store`.

`Registry::for_project_authored` binds a typed scope to
`evaluation-scopes/<organization-uuid>/<project-uuid>/registry/` in the
configured evaluation object store. The existing `evaluation-rubrics/`,
`evaluation-assessments/` and `evaluation-scorecards/` keys are nested there.
A bound registry cannot be rebound to another project. The store adapter
preserves atomic creation, checks keys and returned list entries, and returns
relative keys to domain code. Legacy routes retain their original namespaces;
key traversal is rejected on both route families.

Forms and cards preserve their content hashes, publication metadata and
historical versions. Assessment target IDs, standing IDs and revision numbers
are also unchanged. Identical IDs in two projects refer to separate objects;
one project's new revision does not modify another. Assessments and judged
cards resolve their rubric pins only inside their own project. The same form
must be published independently before the same pin can be used elsewhere.
A missing history remains an empty page, as in the existing API.

This binding deliberately admits only those three authored storage families.
It replaces the instance source authority with a refusing authority and does
not inherit its evidence cipher or content-reader capability. Publishing a
card that needs an external scorer catalog is explicitly rejected until that
catalog has a project policy; built-in and rubric-based judge declarations
are supported. A declaration does not start a judge or an evaluation.

Assessment targets remain literal references: recording a judgement does not
check that the referenced result/trace/span exists, reveal its content or
prove access to it. Result publication, evidence reads, approvals, reviews,
cohorts, recordings, calibrations, scorer catalogs, runtime and retention
workers still need their own integration. No aliases for those HTTP routes
are added by this slice. Request admission does not cancel an object-store
operation already admitted. Panel and SDK callers still use legacy routes;
organization/project UI remains disabled. No migration or deployment was run.

Validation uses the real router with signed OIDC sessions and a local
discovery/JWKS endpoint, plus disposable filesystem storage with reopening,
raw immutable-byte comparison and history pagination. These are integration
checks of this boundary, not full E2E with Authentik, PostgreSQL, S3 or workers.


## Sixth resource boundary: project workflow definitions

Three authored workflow operations also live below
`/api/v1/orgs/{organization}/projects/{project}`:

- `GET /workflow-definitions` lists current definitions;
- `GET /workflow-definitions/{name}` reads the current or requested revision;
- `POST /workflow-definitions` validates and saves a version.

The same handlers serve both families. Scoped reads require a current project
viewer grant; publication requires editor/admin, `X-AIWatcher-IAM: 1` and
another authorization check after JSON arrives. Instance roles do not replace
project grants. `registered_by` comes from the authenticated caller. Scoped
responses use `Cache-Control: no-store`.

`DefinitionRegistry::for_project` binds the object store to
`workflows/scopes/<organization-uuid>/<project-uuid>/registry/`, with `heads/`
and `versions/` underneath. It cannot be rebound to another project. Legacy
heads and versions keep their original `workflows/heads/` and
`workflows/versions/` keys. A name is hashed, including names containing
slashes; a revision remains a lowercase SHA-256 digest. Invalid revision
strings continue to answer as missing, without reading any object.

The scope does not enter the definition or its digest. Historical definitions,
registration metadata, task pins, parameters, approval gates and compiled plan
identities are preserved. Repeated publication returns the original saved
version; publishing identical content independently in another project gives
it the same revision. Changing one project's head leaves the others alone.
Listing checks each returned address before fetching bytes, then checks the
head's name and revision against its contents, on both route families.

Graph compilation and deployment pod-template validation still run before a
save. A project grant cannot override the allowed images or resource limits.
The templates remain deployment configuration; this slice adds no project
permission to launch a pod or claim a task queue.

Saving a definition neither starts execution nor creates a schedule. Existing
legacy starts and schedule saves cannot resolve a definition held only in a
project, and a pinned legacy start cannot substitute a same-named project
revision. No scoped execution or schedule aliases are added. Tasks, queues,
parameters, approval roles and resource references remain literal definition
data; runtime authorization, caches, credentials and revocation of running
work still need their own integration. Direct registry access is storage
binding, not an authorization API, and request admission does not cancel an
object-store operation already admitted.

Tests exercise the real router with signed OIDC sessions, current grants and
actual legacy start/schedule handlers. Filesystem tests reopen the registry,
compare immutable bytes and compiled identities, and reject corrupt heads.
A deliberately incorrect listing adapter verifies that foreign or traversal
keys are refused before their bytes are read. No migration or deployment was
performed; panel/SDK callers and organization/project UI still await cutover.


## Seventh resource boundary: project case reviews and dataset publication

Five operations are served below
`/api/v1/orgs/{organization}/projects/{project}`:

| Resource suffix | Operations |
| --- | --- |
| `/evaluation-reviews` | List one dataset's queue or propose a case. |
| `/evaluation-reviews/of-target` | List this project's proposals for a target. |
| `/evaluation-reviews/{id}/actions` | Write the expected answer, approve or reject. |
| `/evaluation-reviews/publish` | Append approved cases to this project's dataset. |

Viewer reads; editor/admin proposes, writes expectations, rejects, approves
written content and publishes approved cases. Approving `observed` words requires
**project admin**, checked after the JSON body arrives. An instance admin with
only project editor access cannot grant that approval; an expired project admin
grant cannot be replaced by a surviving editor grant. All mutations require
`X-AIWatcher-IAM: 1`; project responses use `Cache-Control: no-store`.

`Registry::for_project_authored` now includes `evaluation-reviews/` and
`evaluation-review-targets/` under the same
`evaluation-scopes/<org>/<project>/registry/` boundary as forms and assessments.
Review IDs, revision bytes, authors, target/assessment references and split labels
retain their representations. Dataset names are hashed; review IDs cannot escape
a key component. The adapter rejects foreign listing entries before reading them.
Known review IDs and dataset versions do not authorize access in another project.

Publication uses both the evaluation and dataset registries admitted for the
same route scope. It reads only that project's prior dataset version and approved
review snapshots, writes the next dataset version, then marks those snapshots
with the stored version. A new dataset is created when that project has none;
no global or neighboring dataset is used as a fallback. Existing non-case rows
are still refused. Editing an approved review removes approval as before.
Marking publication now refuses a changed review revision or a different already
published version, including on legacy routes, instead of attributing later
words to an earlier dataset snapshot.

**This is not a transaction across the two registries.** A crash, storage error
or concurrent review edit after the dataset write can leave a dataset version
without all publication marks; a conflicting review revision returns 409 and
is not overwritten. Concurrent dataset publications still use the existing
registry head semantics. Durable coordination/recovery of this workflow remains
a later step. The normal bounded-operation IAM admission limitation still applies:
revocation does not cancel storage work already admitted.

Project proposals currently require explicit `written` or `observed` words.
Automatic case-text extraction and caller-supplied `measured` classification are
refused until evidence/source resolution is project-aware. Target and assessment
IDs remain literal provenance references; this stage does not resolve them or
prove access to the trace/result. The global source resolver is never reused.
Legacy source-backed proposals retain their existing behavior.

The generated contract/client expose the routes; the panel and SDK workflows
still use legacy routes. No organization/project UI was activated, data migrated,
worker enabled or deployment changed. The dataset-only migration inventory does
not yet cover review revisions or target indexes.

Verification covers all five HTTP operations with signed OIDC sessions,
project/organization/legacy isolation, publication into an existing local dataset,
known-version denial, mutation headers, current role/expiry/revocation, delayed
editor/admin uploads, disabled IAM and unavailable sources. File-store tests
reopen the registry and compare original revision/index bytes; a hostile listing
adapter proves rejection before foreign reads. A regression checks publication
conflicts after concurrent review edits and idempotent marking of one version.
These are not full Authentik/PostgreSQL/S3 or worker end-to-end tests.


## Eighth resource boundary: project cohorts from native datasets

Two operations are available below
`/api/v1/orgs/{organization}/projects/{project}`:

| Resource suffix | Operations |
| --- | --- |
| `/evaluation-cohorts` | Derive pinned cases from a curation version or annotation export. |
| `/evaluation-cohorts/{cases}` | Read the saved derivation metadata by case digest. |

Viewer reads; editor/admin derives. Derivation requires `X-AIWatcher-IAM: 1` and
a current project write grant checked again after receiving JSON. Project
responses use `Cache-Control: no-store`; an instance role never substitutes for
project access. Derivation metadata is stored under
`evaluation-scopes/<org>/<project>/registry/evaluation-cohorts/`.
The case digest, canonical file bytes, source versions, split/limit semantics,
first author and derivation time retain their existing representations. Identical
source data stored independently in two projects yields the same pins and separate
metadata objects. A known source version or digest does not grant cross-project
or legacy access.

`SourceAuthority::for_project_cohorts` refuses by default. An adapter must opt in
by binding its native owners. The server's `LocalSource` builds a dedicated
cohort-only adapter over project-bound dataset and annotation registries. It does
not carry the instance's directory, staged bundles, conversation archive, prompt
or model owners. It refuses rebinding to another project and cannot resolve a
full evidence manifest. No new server configuration is required: existing wiring
already supplies the native owners to the root `LocalSource`.

Each derivation verifies the exact source version again, even if this cohort's
metadata is already stored. Moving a head does not change old pins; missing or
corrupt source data is refused without searching another project. Annotation
exports retain their existing verified image/blob checks. Curation derivation
retains the selected split, first-N limit and count of rows without a split.
GET returns historical metadata, not fresh proof that all source bytes still
exist. It does not fetch case files from the returned `aiwatcher://` references;
those are pins interpreted with the same project context.

Project conversation corpora remain unavailable, including to project/instance
admins. This stage does not expose scoped result publication, approval bundles,
recordings, scoring declarations, execution, retention workers or automatic case
extraction in review. The panel and SDK still use legacy paths; selectors remain
inactive and no data migration or deployment was performed. The admission check
does not cancel an already admitted source read or object-store write after a
revocation. The current migration inventory does not yet include cohort metadata.

HTTP tests exercise signed OIDC sessions, source-version and metadata isolation,
roles, headers, intervals, revocation, delayed body upload, disabled IAM and the
unsupported-factory refusal. Server tests use the real native owner adapters,
file-store reopen and raw metadata comparison, historical versions, split/limit
pins, corrupt/deleted curation bytes, annotation export/blob checks and rejection
of global source capabilities. They are not a full Authentik/PostgreSQL/S3 or
worker end-to-end environment.
