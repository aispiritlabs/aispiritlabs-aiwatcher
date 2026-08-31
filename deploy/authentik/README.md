# authentik, and what aiwatcher needs from it

aiwatcher is an OpenID Connect relying party. authentik is what it was built
and tested against, and nothing in the code is specific to it beyond two
defaults: the groups claim is `groups`, and the forward-auth header names are
authentik's. Keycloak, Okta, Entra and Auth0 work by changing the issuer.

There are two ways to connect them, and they are not the same decision.

| | `AIWATCHER_AUTH_MODE=oidc` | `AIWATCHER_AUTH_MODE=proxy` |
|---|---|---|
| Who runs the login | aiwatcher | authentik's outpost, at the ingress |
| Needs an authentik application | yes | no — the proxy provider you already have |
| Identity is | **proved** to aiwatcher | **asserted** to aiwatcher, in headers |
| Trust boundary | the token's signature | the network in front of port 8080 |
| Works without an ingress | yes | no |

Use `proxy` where a forward-auth already guards aiwatcher and you want it to
know *who* rather than only *whether* — it is one variable. Use `oidc`
everywhere else, and anywhere the namespace is not a boundary you would defend.

## oidc, by hand

Five minutes, and it is worth doing once even if you then use the blueprint,
because every field below is one aiwatcher will complain about by name.

1. **Groups.** Directory → Groups. Create `aiwatcher-admins`,
   `aiwatcher-editors` and `aiwatcher-viewers`. Membership is managed here and
   nowhere else; aiwatcher re-reads it on every sign-in.

2. **Provider.** Applications → Providers → Create → *OAuth2/OpenID Provider*.
   - Name: `aiwatcher`
   - Authorization flow: *implicit consent* — it is a first-party application
   - Client type: **Confidential**
   - Redirect URI, **strict**: `https://<your-host>/api/v1/auth/callback`
     (`http://localhost:5173/api/v1/auth/callback` for the panel's dev server,
     which proxies `/api`)
   - Scopes: `openid`, `email`, `profile`. The third is what carries `groups`.
   - Copy the **Client ID** and **Client Secret**.

3. **Application.** Applications → Applications → Create.
   - Name: `aiwatcher`, Slug: `aiwatcher`, Provider: the one above.
   - The slug is what makes the issuer URL. This is the field people get wrong.

4. **Point aiwatcher at it.**

   ```bash
   AIWATCHER_AUTH_MODE=oidc
   AIWATCHER_AUTH_ISSUER=https://<authentik-host>/application/o/aiwatcher/
   AIWATCHER_AUTH_CLIENT_ID=<client id>
   AIWATCHER_AUTH_CLIENT_SECRET=<client secret>
   AIWATCHER_AUTH_REDIRECT_URL=https://<your-host>/api/v1/auth/callback
   AIWATCHER_AUTH_SESSION_SECRET=$(openssl rand -hex 32)
   ```

   The issuer ends in the **application** slug, not the provider name, and it
   ends in a slash. aiwatcher reads
   `<issuer>/.well-known/openid-configuration` at start-up and refuses to start
   if the document names a different issuer, so a wrong value here is a clear
   failure at deploy time rather than a confusing one at sign-in.

## oidc, from the blueprint

`aiwatcher-blueprint.yaml` beside this file creates all three. Apply it by
mounting it into authentik's `/blueprints/custom` (which `just authentik-up`
does), by putting it in the ConfigMap the worker reads, or by pasting it into
Admin → Customisation → Blueprints.

It deliberately does not contain the client secret. Read it once from
Admin → Providers → aiwatcher.

## proxy, where the outpost is already there

Nothing to create. The outpost already sets `X-authentik-username`,
`X-authentik-groups`, `X-authentik-email`, `X-authentik-name` and
`X-authentik-uid` on every request it lets through; aiwatcher reads them.

```yaml
auth:
  mode: proxy
networkPolicy:
  enabled: true   # the chart refuses to render without it
```

The policy is not paperwork. In this mode a header is a claim, so any pod that
can reach port 8080 can claim to be an admin — the boundary is the network, and
the chart will not let you assert the mode without one.

Groups reach aiwatcher only if the authentik *proxy provider* sends them. In
its scope settings, make sure `profile` (or a mapping that emits `groups`) is
selected; otherwise every signed-in person is a viewer and nothing says why.

## Producers

An agent posting to `/api/v1/events` runs in the cluster, reaches the Service
directly, never passes whatever authenticates a browser, and cannot complete an
interactive sign-in. It gets a token of its own:

```bash
AIWATCHER_AUTH_INGEST_TOKENS="planner=$(openssl rand -hex 32)"
```

and the SDKs send it as `AIWATCHER_TOKEN`. Each token grants the **editor**
role and never admin: a secret sitting in an agent's environment must not be
able to ask an orchestrator to run something.

An authentik service account works too, in `oidc` mode: a client-credentials
token whose audience is aiwatcher's client id verifies against the same JWKS as
a person's, and its groups map to roles the same way.

## Roles

| Role | May |
|---|---|
| `viewer` | read runs, spans, dimensions, metrics, evaluations, prompts, the live stream |
| `editor` | that, plus publish prompt versions, move labels, record optimisations, publish events |
| `admin` | that, plus dispatch a workflow rerun |

An authenticated person in none of the mapped groups is a `viewer`, because the
provider letting them in was already a decision. `AIWATCHER_AUTH_DEFAULT_ROLE=none`
changes that to "a group is required to see anything", and
`AIWATCHER_AUTH_REQUIRED_GROUPS` refuses the sign-in outright.
