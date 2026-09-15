# IAM control plane — first part of IAM-01

This crate implements organization membership, local teams and project grants.
It authorizes mutations of **its own IAM metadata**. The server now exposes an
optional IAM API with a transactional audit trail. The dataset registry now also
has project-scoped HTTP adapters, described below. Legacy data, logs, other
artifacts, streams, query services and execution workers are not yet isolated.
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

Entry points for the enforcement work are `aiwatcher-api/src/auth.rs` and
`iam.rs` and `state.rs`; registry modules in `aiwatcher-api/src`; `aiwatcher-projector`'s read
model/live hub; `aiwatcher-server/src/wiring.rs` and execution workers; and the
separate `services/query` and `services/ml_pipeline` runtimes. The dataset registry adapter below is the first scoped data-plane slice;
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
