# Signing in locally, and the two people you need to test a grant

The steps that actually worked on 18.09.2026, on a machine where none of this
had ever been run. Everything here is one machine, four containers and two
commands; nothing below touches a cluster, and every credential in it is a
development fixture written down on purpose.

Read [`deploy/authentik/README.md`](../deploy/authentik/README.md) for what
aiwatcher needs from an identity provider and why. This file is the shortest
path to *having* one.

## The two commands

```bash
just authentik-up   # server, worker, PostgreSQL, Redis — and the blueprint
just run-sso        # aiwatcher on :8080 as a relying party
just panel          # the dev server on :5173, which is where you sign in
```

`just authentik-up` used to end at a setup form somebody had to fill in by
hand. It no longer does: the compose file sets `AUTHENTIK_BOOTSTRAP_PASSWORD`,
`AUTHENTIK_BOOTSTRAP_TOKEN` and `AUTHENTIK_BOOTSTRAP_EMAIL`, which authentik
reads **once, on the first start against an empty database**, and applies
through its own `blueprints/system/bootstrap.yaml`. That gives you `akadmin`
with a password and a non-expiring API token in one go, so the client secret
can be read out of authentik over its API instead of copied out of its admin
UI.

The blueprint beside that compose file — provider, application and the three
groups — is applied by the worker a few seconds after the containers report
healthy. Two consequences worth knowing before you debug something that is not
broken:

- **The API answers before the blueprints are applied.** `/-/health/ready/`
  returning 200 means the server is up, not that the provider exists. A call
  with the bootstrap token immediately after it says `Token invalid/expired`;
  the same call five seconds later works.
- **Discovery is the check that the slug is right.**
  `curl http://localhost:9000/application/o/aiwatcher/.well-known/openid-configuration`
  should answer with `"issuer": "http://localhost:9000/application/o/aiwatcher/"`.
  A 404 there means the issuer names the *provider* rather than the
  *application*, which is the failure `deploy/authentik/README.md` warns about
  and the one aiwatcher refuses to start on.

## The three fixture accounts

| Account | Password | authentik groups | aiwatcher instance role |
|---|---|---|---|
| `akadmin` | `aiwatcher-dev` | authentik Admins | — (never signs into aiwatcher) |
| `teacher` | `teacher-dev` | `aiwatcher-admins` | `admin` |
| `student` | `student-dev` | none | `viewer` |

`akadmin` is authentik's superuser and stays there. The other two are the two
people a grant needs: somebody who may create an organization, and somebody who
may not see it until they are told they can.

Create them with the bootstrap token, which is what `just authentik-seed`
does:

```bash
just authentik-seed          # teacher and student, idempotent
just authentik-secret        # the provider's client secret, for .env
```

## What goes in `.env`

`just` loads `.env` into **every** recipe, so only the two secrets live there.
The mode, the issuer, the client id and the redirect URL are set inline by
`just run-sso`, which is what keeps a plain `just run` unauthenticated:

```bash
AIWATCHER_AUTH_CLIENT_SECRET=…   # just authentik-secret
AIWATCHER_AUTH_SESSION_SECRET=…  # openssl rand -hex 32
```

Without the session secret every restart signs everybody out. Without the
client secret the sign-in fails at the token exchange, and the panel says so:
*"The identity provider rejected this application."*

## Sign in at :5173, not at :8080

The redirect URI on the provider is `http://localhost:5173/api/v1/auth/callback`
because the panel's dev server proxies `/api`, so the browser sees one origin.
Going to `http://localhost:8080` and signing in there fails the redirect-URI
check — strictly, by design.

The cookie is the other half of that. `Secure` is derived from the redirect
URL's scheme, so over `http://localhost` the session cookie is not `Secure` and
is therefore stored. Point the redirect URL at an `https://` host you are not
actually serving over TLS and the sign-in loops with no error anywhere, which
is the one failure in this file that looks like nothing at all.

## The same sign-in without a browser

Two people cannot share one browser profile, and a permission matrix wants to
be re-run rather than re-clicked. `scripts/sso-session.py` drives the whole
authorization-code flow with PKCE through authentik's own flow-executor API and
hands back a session:

```bash
python3 scripts/sso-session.py teacher     # prints /api/v1/auth/me
python3 scripts/iam-permission-check.py    # the whole matrix, both people
```

The one thing that is easy to get wrong there: the flow executor takes the
flow page's query string as a single `query` parameter. Pass `next=` straight
through and the flow finishes at authentik's own user page instead of coming
back to aiwatcher's callback — with no error, because nothing failed.

## What a subject is, and where to find it

A principal is `(provider, subject)` and nothing else — not an email, not a
display name. authentik's `sub` is a hash of the user's id (its provider's
`sub_mode` is `hashed_user_id`), so it is stable and unguessable and it is
**the only thing a grant can be issued to**.

Somebody who has signed in reads theirs off `/account`, under Subject. Until
invitations exist that is how a teacher gets a student's: the student signs in
once and reads it out. The provider is the issuer URL, ending in a slash.

## Resetting

```bash
just authentik-down    # keeps the database
just authentik-reset   # wipes it, including akadmin and the bootstrap token
```

A reset is the only way to re-apply the bootstrap variables, since authentik
reads them once. It also changes every `sub`, so the grants an IAM database
still holds point at people who no longer exist — drop that database with it:

```bash
docker exec aiwatcher-postgres dropdb -U aiwatcher aiwatcher_iam
```
