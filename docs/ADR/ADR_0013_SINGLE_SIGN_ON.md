# ADR_0013: aiwatcher is its own relying party, and the session is a cookie it signs

- **Status**: accepted
- **Date**: 2026-08-31

## Context

Until now aiwatcher identified nobody. What guarded it was whatever guarded its
ingress — in planner's cluster, two Traefik middlewares, the second a
forward-auth to authentik's embedded outpost. That answers exactly one
question, *may this request in*, and it answers it at the edge. Three things
followed from that:

* **Anything inside the namespace was already past the gate.** The server's
  Service is reachable by every pod, and `POST /api/v1/events` is open on
  purpose, because it is how an SDK that cannot reach the broker publishes.
* **A rerun had no requester.** `POST /api/v1/workflows/{id}/rerun` is the one
  route that asks another system to run something, and the log line for it
  could only ever have said *somebody*.
* **Reading and writing were the same permission.** The prompt registry is the
  one store here that outlives retention, and everyone who could see a prompt
  could move `production` to point at a different one.

The panel makes the second constraint sharper than it looks. Its two most
important routes are an SSE stream and a WebSocket, and browsers let neither
set request headers. So the usual single-page-app shape — hold a token in
memory, send `Authorization: Bearer` — does not reach the parts of the panel
that matter without putting the credential in a query string, which is the one
place a credential ends up in every access log between the browser and here.

authentik was already in the picture, so the question was never *which
provider*. It was *what does aiwatcher do about it*.

## Decision

**aiwatcher becomes an OpenID Connect relying party, and keeps the session in
a cookie it signs itself.** `aiwatcher-auth` is a new crate between `runner`
and `projector`; `aiwatcher-api` gains one layer in front of every route and
a `Role` check inside the handlers that write.

Three modes, and the middle one exists because the other two are each wrong
somewhere:

| `AIWATCHER_AUTH_MODE` | Who establishes the identity | Trust boundary |
|---|---|---|
| `none` (default) | nobody | whatever is in front |
| `oidc` | this process, over the authorization-code flow with PKCE | the token's signature |
| `proxy` | the authenticating proxy already in front | the network in front of port 8080 |

`none` stays the default because every setting in this workspace has a default
that works with nothing running, and because a release that started answering
401 would be an upgrade that took an installation's observability away at the
moment it was needed.

Four things follow, and each is a decision of its own:

1. **The authorization-code exchange happens in this process, and the tokens
   are dropped.** What the browser keeps is an HttpOnly, `SameSite=Lax`,
   `Secure` cookie holding a signed identity. `EventSource` and `WebSocket`
   attach it with no application code, which is the whole reason for it. No
   token reaches JavaScript.

2. **There is no session store.** The cookie is self-contained and
   HMAC-SHA256-signed, with an expiry inside the signature. No table to size,
   evict or replicate, and no state that makes a second replica behave
   differently from the first. The cost is revocation, below.

3. **Roles come from groups, and the provider owns membership.**
   `viewer | editor | admin`, mapped from group names by configuration.
   aiwatcher stores nothing about people and re-reads the mapping on every
   sign-in. `admin` guards exactly one route — the rerun.

4. **A producer gets a credential the provider cannot give it.**
   `AIWATCHER_AUTH_INGEST_TOKENS` holds shared secrets, each granting `editor`
   and never `admin`. This is not a way around the provider: an agent runs in
   the cluster, reaches the Service directly, never passes the ingress that
   authenticates a browser, and cannot complete an interactive sign-in. Without
   it, turning single sign-on on would silently stop every SDK publishing over
   HTTP — which would make the feature unadoptable in the deployment it was
   built for.

The provider is contacted once at start-up for its discovery document, with a
short retry while it comes up, and then never again except to refetch the key
set when a token names a `kid` we do not hold. If it cannot be reached, the
process does not start: the only other thing it could do is serve
unauthenticated.

## Alternatives considered

**A bearer token in the browser, the usual SPA shape.** Loses SSE and the
WebSocket, which is most of the panel. The workarounds are a token in a query
string or a second authentication path for two routes; the first puts a
credential in access logs and the second is two things to get right instead of
one.

**Leave it to the ingress — keep only forward-auth.** This is `proxy` mode, and
it is offered, because where the proxy already exists it costs one variable and
buys a name on every rerun. It is not the only mode because the identity is
*asserted* rather than proved: in that mode any pod that can reach port 8080
can claim to be an admin. That is a defensible boundary in a fenced namespace
and not one anywhere else, so the chart refuses to render `proxy` without a
NetworkPolicy, and `oidc` is what a deployment that wants a real boundary uses.

**A server-side session store.** Would give true revocation. It would also add
the first piece of mutable state this process keeps that is not a bounded fold
over the log — something to size against `AIWATCHER_MAX_SPANS_TOTAL`, evict,
and eventually replicate. For an eight-hour session on a single-replica
Deployment that is a large amount of new machinery for a window that shortening
the TTL also closes.

**`jsonwebtoken` 11, or hand-rolled verification.** Version 11 needs either
`aws-lc-rs` — a C toolchain in the image — or `rust_crypto`, which pulls `rsa`,
which carries RUSTSEC-2023-0071 with no fixed release, so `just audit` would go
red over a crate used only to *verify* somebody else's signature. Version 9
verifies with `ring`, which rustls already puts in this graph. Hand-rolling RSA
verification was never on the table; hand-rolling the HMAC over the session
cookie was, and `ring` does that too.

**SAML.** authentik speaks it. Nothing else here would benefit, and it would be
a second protocol implementation for the same outcome.

## Consequences

**Signing out cannot invalidate a copy of the cookie.** The session lifetime
*is* the revocation window — eight hours by default, and the reason the default
is a working day rather than a week. Removing somebody from a group in
authentik takes effect at their next sign-in, not immediately. A deployment
that needs faster than that shortens `AIWATCHER_AUTH_SESSION_TTL_SECONDS`; one
that needs true revocation needs the store this ADR declined.

**Turning it on breaks HTTP ingest until a token exists.** Deliberately loud:
the chart's NOTES say so, the SDKs read `AIWATCHER_TOKEN`, and a 401 in an
agent's log is a variable to set. The alternative — quietly exempting
`/api/v1/events` — would leave the one write path open on an instance somebody
believed they had just secured.

**Without `AIWATCHER_AUTH_SESSION_SECRET` a restart signs everybody out.** The
server generates a key and warns. Safe, and annoying enough that the chart asks
for the Secret.

**`proxy` mode's boundary is the namespace.** Stated in the values file, in the
NOTES, and enforced only as far as a NetworkPolicy can enforce it. It is a
smaller claim than `oidc` makes and it is written down as one.

**The panel now has a state it did not have.** Everything renders behind a
gate, and a 401 from any route sends it back to the sign-in screen. On an
instance with `mode: none` the gate renders its children and nothing else
happens, which is the case that must keep working and is tested.

**What would make this wrong.** Any of:

* A second server replica becoming worth running. The read model and the live
  hub already prevent that (ADR_0004), but if they stop, a signed cookie
  scales and a session store would have had to.
* Somebody needing to revoke a session in under a minute — a compromised
  account, a compliance requirement with a number in it. That is the store,
  and it is a different ADR.
* The group mapping growing a fourth role, or needing per-workflow or
  per-prompt permissions. Three ordered roles is a model that fits in a
  sentence; the moment it does not, a table of group-to-resource grants is a
  different design and should not be reached by adding a fourth variable.
* `ring` losing its maintenance, or `jsonwebtoken` 9 going unmaintained while
  `rsa` still carries an unfixed advisory. Then the crypto backend is the
  decision to revisit, not the architecture above it.
