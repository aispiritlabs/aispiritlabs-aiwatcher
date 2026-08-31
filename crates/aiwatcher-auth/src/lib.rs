//! Single sign-on, with authentik as the provider it was built against.
//!
//! aiwatcher had no notion of a person before this crate. What guarded it was
//! whatever guarded its ingress — in planner's cluster, a Traefik forward-auth
//! to authentik — which answers "may this request in" and nothing else. Every
//! run looked equally anonymous, a rerun had no requester, and anything inside
//! the cluster that could reach the Service was past the only gate there was.
//!
//! Three modes, and the middle one is the reason the other two are not enough
//! on their own:
//!
//! | `AIWATCHER_AUTH_MODE` | Who says who you are | What it costs to adopt |
//! |---|---|---|
//! | `none` (default) | nobody | nothing; unchanged behaviour |
//! | `oidc` | this process, as an OIDC relying party | an application in authentik |
//! | `proxy` | the authenticating proxy already in front | one variable, where that proxy exists |
//!
//! `none` stays the default because every other setting in this workspace has
//! a default that works with nothing running, and because a release that
//! started refusing requests would be an upgrade that took an installation
//! down.
//!
//! ## The session is a cookie, and that is a decision
//!
//! A single-page app usually holds a token in memory and sends it as a header.
//! That does not work here: the panel's two most important routes are an SSE
//! stream and a WebSocket, and browsers let neither set request headers. The
//! alternatives are a credential in a query string — which is the one place a
//! credential ends up in every access log between here and the browser — or a
//! cookie the browser attaches on its own. So the authorization-code exchange
//! happens *in this process*, the provider's tokens are read once and dropped,
//! and what the browser keeps is an HttpOnly cookie this server signed. No
//! token ever reaches JavaScript, and `EventSource` needs no special case.
//!
//! ## What this crate does not do
//!
//! It stores nothing. There is no session table, no refresh-token vault and no
//! user directory: the provider owns all three. The cost is that signing out
//! clears the browser's cookie but cannot invalidate a copy taken from it, so
//! the session lifetime *is* the revocation window — eight hours by default.
//! A deployment that needs a shorter one shortens it; a deployment that needs
//! true revocation needs a store, which is a different decision than this one.

pub mod cookie;
pub mod error;
pub mod identity;
pub mod oidc;
pub mod proxy;
pub mod signing;

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::signing::constant_time_eq;

pub use cookie::{CookieSpec, SameSite};
pub use error::{AuthError, AuthResult};
pub use identity::{Credential, Identity, NotEntitled, Role, RoleMapping, UnknownRole};
pub use oidc::{Claims, ProviderMetadata};
pub use proxy::ProxyHeaders;

/// Where identity comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// No authentication. Every caller is [`Identity::anonymous`] and every
    /// role check passes.
    #[default]
    None,
    /// This process runs the authorization-code flow against an OpenID
    /// Connect provider.
    Oidc,
    /// An authenticating reverse proxy in front of this process asserts the
    /// identity in headers. See [`proxy`] for what makes that sound.
    Proxy,
}

impl std::str::FromStr for AuthMode {
    type Err = AuthError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::None),
            // `authentik` is accepted because that is what an operator setting
            // this up is thinking about; it is an OIDC provider either way.
            "oidc" | "openid" | "authentik" | "sso" => Ok(Self::Oidc),
            "proxy" | "forward-auth" | "forwardauth" | "header" => Ok(Self::Proxy),
            other => Err(AuthError::Configuration(format!(
                "AIWATCHER_AUTH_MODE is {other:?}; expected one of none, oidc, proxy"
            ))),
        }
    }
}

impl AuthMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Oidc => "oidc",
            Self::Proxy => "proxy",
        }
    }
}

/// Everything an [`Authenticator`] needs.
#[derive(Clone)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// The provider's issuer URL. For authentik this is
    /// `https://<host>/application/o/<application-slug>/` — the *application*
    /// slug, not the provider name, which is the single most common thing to
    /// get wrong here.
    pub issuer: String,
    pub client_id: String,
    /// `None` makes this a public client, which is legitimate with PKCE. An
    /// authentik OAuth2 provider is confidential by default and hands one out.
    pub client_secret: Option<String>,
    /// What a token's `aud` must contain. Defaults to `client_id`, which is
    /// what authentik puts there.
    pub audiences: Vec<String>,
    pub scopes: Vec<String>,
    /// The absolute URL the provider sends the browser back to. Must match the
    /// redirect URI registered on the provider byte for byte.
    pub redirect_url: String,
    /// Where to send a browser after signing out, absolute. `None` leaves the
    /// user on the provider's own signed-out page.
    pub post_logout_url: Option<String>,
    pub groups_claim: String,
    pub roles: RoleMapping,
    pub session_ttl: Duration,
    pub session_secret: Option<String>,
    pub cookie_name: String,
    pub cookie_secure: bool,
    pub cookie_domain: Option<String>,
    /// Accept `Authorization: Bearer` as well as the session cookie. What a
    /// machine client uses — an authentik service account, or anything holding
    /// a client-credentials token for this audience.
    pub allow_bearer: bool,
    /// Shared secrets that authenticate a producer, each granting
    /// [`Role::Editor`] and nothing more.
    ///
    /// Not a shortcut around the provider — the thing the provider cannot do.
    /// An agent publishing to `/api/v1/events` runs in the cluster, reaches
    /// the Service directly and never passes the ingress that authenticates a
    /// browser; it also cannot complete an interactive sign-in. Without this,
    /// turning single sign-on on would silently stop every SDK that publishes
    /// over HTTP, which is the whole integration path for a Python or
    /// TypeScript agent.
    ///
    /// Editor deliberately, never admin: a shared secret in an agent's
    /// environment must not be able to ask an orchestrator to run something.
    pub ingest_tokens: Vec<IngestToken>,
    /// Ask the provider's `userinfo` endpoint for groups when the id token
    /// carries none. On by default: an authentik application whose scope
    /// mapping was never added returns an id token with no `groups`, and the
    /// symptom — everybody is a viewer — looks nothing like its cause.
    pub userinfo_fallback: bool,
    pub proxy_headers: ProxyHeaders,
    /// What to call the provider on the sign-in button.
    pub provider_name: String,
    pub http_timeout: Duration,
    pub discovery_attempts: u32,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            issuer: String::new(),
            client_id: String::new(),
            client_secret: None,
            audiences: Vec::new(),
            scopes: vec![
                "openid".to_owned(),
                "profile".to_owned(),
                "email".to_owned(),
            ],
            redirect_url: String::new(),
            post_logout_url: None,
            groups_claim: "groups".to_owned(),
            roles: RoleMapping::default(),
            // Eight hours: a working day, and the window in which a stolen
            // cookie stays useful. See the note on revocation in the module
            // docs — this number is that trade-off, not a guess.
            session_ttl: Duration::from_secs(8 * 60 * 60),
            session_secret: None,
            cookie_name: "aiwatcher_session".to_owned(),
            cookie_secure: true,
            cookie_domain: None,
            allow_bearer: true,
            ingest_tokens: Vec::new(),
            userinfo_fallback: true,
            proxy_headers: ProxyHeaders::default(),
            provider_name: "authentik".to_owned(),
            http_timeout: Duration::from_secs(10),
            discovery_attempts: 5,
        }
    }
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the client secret and never the session secret. This struct is
        // reachable from `AppState`, and something above it derives `Debug`.
        f.debug_struct("AuthConfig")
            .field("mode", &self.mode)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.is_some())
            .field("session_secret", &self.session_secret.is_some())
            .field("redirect_url", &self.redirect_url)
            .field("scopes", &self.scopes)
            .field("groups_claim", &self.groups_claim)
            .field("allow_bearer", &self.allow_bearer)
            .field("ingest_tokens", &self.ingest_tokens.len())
            .finish_non_exhaustive()
    }
}

impl AuthConfig {
    /// Refuse a configuration that cannot work, before anything starts
    /// listening.
    ///
    /// The alternative is an instance that starts, answers every sign-in with
    /// a redirect to a provider that refuses it, and says why only in the
    /// provider's log.
    pub fn validate(&self) -> AuthResult<()> {
        match self.mode {
            AuthMode::None | AuthMode::Proxy => Ok(()),
            AuthMode::Oidc => {
                for (name, value) in [
                    ("AIWATCHER_AUTH_ISSUER", &self.issuer),
                    ("AIWATCHER_AUTH_CLIENT_ID", &self.client_id),
                    ("AIWATCHER_AUTH_REDIRECT_URL", &self.redirect_url),
                ] {
                    if value.trim().is_empty() {
                        return Err(AuthError::Configuration(format!(
                            "{name} is required when AIWATCHER_AUTH_MODE=oidc"
                        )));
                    }
                }
                if !self.redirect_url.starts_with("http://")
                    && !self.redirect_url.starts_with("https://")
                {
                    return Err(AuthError::Configuration(format!(
                        "AIWATCHER_AUTH_REDIRECT_URL is {:?}; the provider needs an absolute URL, \
                         and it must match the redirect URI registered on the application",
                        self.redirect_url
                    )));
                }
                if !self.scopes.iter().any(|scope| scope == "openid") {
                    return Err(AuthError::Configuration(
                        "AIWATCHER_AUTH_SCOPES must include `openid`; without it the provider \
                         returns no id token and there is nothing to establish an identity from"
                            .to_owned(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn audience_set(&self) -> HashSet<String> {
        if self.audiences.is_empty() {
            std::iter::once(self.client_id.clone()).collect()
        } else {
            self.audiences.iter().cloned().collect()
        }
    }
}

/// A shared secret a producer presents, and the name it is known by.
///
/// The label is what reaches a log line. The secret never does — which is the
/// reason the two are separate fields rather than one opaque string.
#[derive(Clone)]
pub struct IngestToken {
    pub label: String,
    pub secret: String,
}

impl std::fmt::Debug for IngestToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestToken")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl std::str::FromStr for IngestToken {
    type Err = AuthError;

    /// `name=secret`, or a bare secret that takes the label `ingest`.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (label, secret) = match raw.split_once('=') {
            Some((label, secret)) => (label.trim(), secret.trim()),
            None => ("ingest", raw.trim()),
        };

        // A guessable shared secret is a worse hole than no authentication,
        // because it looks like authentication. Refused at start-up, where the
        // person who chose it is still watching.
        if secret.chars().count() < MIN_TOKEN_LENGTH {
            return Err(AuthError::Configuration(format!(
                "the ingest token {label:?} is shorter than {MIN_TOKEN_LENGTH} characters; \
                 generate one with `openssl rand -hex 32`"
            )));
        }
        Ok(Self {
            label: label.to_owned(),
            secret: secret.to_owned(),
        })
    }
}

/// Short enough to guess is short enough to refuse.
const MIN_TOKEN_LENGTH: usize = 24;

/// Under the ~4 KB every browser enforces, with room for the attributes and
/// for whatever else is set on this origin.
const MAX_SESSION_COOKIE_BYTES: usize = 3_400;

/// What the panel is told before anybody has signed in.
///
/// Public on purpose, and it is the only route reachable unauthenticated
/// besides the health probes: a panel that cannot ask "is there a login here,
/// and what is it called" has to guess, and guessing wrong means either a
/// sign-in screen on an instance with no provider or an endless 401 loop on
/// one that has.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct PublicAuthConfig {
    /// `false` when `AIWATCHER_AUTH_MODE=none`. The panel renders no sign-in
    /// screen at all in that case.
    pub enabled: bool,
    pub mode: AuthMode,
    /// What to put on the button — "authentik" by default.
    pub provider: String,
    /// Where to send the browser to sign in. `None` in `proxy` mode, where
    /// signing in is something that already happened before the request
    /// arrived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logout_url: Option<String>,
    /// Shown in the panel's own diagnostics, so "which authentik is this
    /// pointing at" is answerable without a shell on the pod.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}

/// The half of a login that has to survive the round trip to the provider.
///
/// Sealed into a short-lived cookie rather than kept in memory: it must
/// survive a restart mid-login, and in a deployment with more than one replica
/// the callback need not land on the instance that started the flow.
#[derive(Debug, Serialize, Deserialize)]
struct LoginState {
    state: String,
    verifier: String,
    nonce: String,
    next: String,
}

/// A sign-in about to start.
#[derive(Debug)]
pub struct LoginRedirect {
    /// The provider's authorization endpoint, with everything on the query
    /// string. Send the browser here.
    pub url: String,
    /// Set this alongside the redirect.
    pub cookie: CookieSpec,
}

/// A sign-in that finished.
#[derive(Debug)]
pub struct SignedIn {
    pub identity: Identity,
    /// A path on this application. Already checked — see [`safe_next`].
    pub next: String,
    pub session: CookieSpec,
    /// Clears the login-state cookie, which has done its job.
    pub state_cleanup: CookieSpec,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

/// The thing the API holds: everything needed to turn a request into an
/// [`Identity`].
pub struct Authenticator {
    config: AuthConfig,
    http: reqwest::Client,
    signer: signing::Signer,
    /// `None` in `proxy` mode, where no provider is contacted at all.
    provider: Option<ProviderMetadata>,
    verifier: Option<oidc::TokenVerifier>,
}

impl std::fmt::Debug for Authenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authenticator")
            .field("mode", &self.config.mode)
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

impl Authenticator {
    /// Build one, contacting the provider in `oidc` mode.
    ///
    /// Returns `Ok(None)` for `AuthMode::None` so the caller has one branch
    /// rather than two: the absence of an authenticator *is* the disabled
    /// state, the same way `None` prompts mean no registry.
    pub async fn connect(config: AuthConfig) -> AuthResult<Option<Self>> {
        config.validate()?;
        if config.mode == AuthMode::None {
            return Ok(None);
        }

        let http = reqwest::Client::builder()
            .timeout(config.http_timeout)
            .build()
            .map_err(|error| {
                AuthError::Configuration(format!("cannot build an HTTP client: {error}"))
            })?;

        let signer = match config.session_secret.as_deref() {
            Some(secret) => signing::Signer::new(secret.as_bytes()),
            None => {
                // Only worth saying in the mode that issues sessions. `proxy`
                // never sets a cookie — the proxy's headers are the identity
                // on every request — so the same warning there would be an
                // operator chasing a variable that changes nothing.
                if config.mode == AuthMode::Oidc {
                    tracing::warn!(
                        "AIWATCHER_AUTH_SESSION_SECRET is unset; sessions are signed with a key \
                         generated at start-up, so a restart signs everybody out"
                    );
                }
                signing::Signer::ephemeral()?
            }
        };

        let (provider, verifier) = match config.mode {
            AuthMode::Proxy | AuthMode::None => (None, None),
            AuthMode::Oidc => {
                let provider =
                    ProviderMetadata::discover(&http, &config.issuer, config.discovery_attempts)
                        .await?;
                tracing::info!(
                    issuer = %provider.issuer,
                    client_id = %config.client_id,
                    bearer = config.allow_bearer,
                    "signed in against the identity provider's discovery document"
                );
                let jwks = oidc::JwksCache::load(http.clone(), provider.jwks_uri.clone()).await?;
                let verifier =
                    oidc::TokenVerifier::new(jwks, provider.issuer.clone(), config.audience_set());
                (Some(provider), Some(verifier))
            }
        };

        Ok(Some(Self {
            config,
            http,
            signer,
            provider,
            verifier,
        }))
    }

    #[must_use]
    pub fn mode(&self) -> AuthMode {
        self.config.mode
    }

    #[must_use]
    pub fn cookie_name(&self) -> &str {
        &self.config.cookie_name
    }

    #[must_use]
    pub fn proxy_headers(&self) -> &ProxyHeaders {
        &self.config.proxy_headers
    }

    #[must_use]
    pub fn public_config(&self) -> PublicAuthConfig {
        PublicAuthConfig {
            enabled: true,
            mode: self.config.mode,
            provider: self.config.provider_name.clone(),
            login_url: match self.config.mode {
                AuthMode::Oidc => Some("/api/v1/auth/login".to_owned()),
                _ => None,
            },
            logout_url: match self.config.mode {
                AuthMode::Oidc => Some("/api/v1/auth/logout".to_owned()),
                _ => None,
            },
            issuer: (!self.config.issuer.is_empty()).then(|| self.config.issuer.clone()),
        }
    }

    /// What `/api/v1/auth/config` answers on an instance with no provider.
    #[must_use]
    pub fn disabled_config() -> PublicAuthConfig {
        PublicAuthConfig {
            enabled: false,
            mode: AuthMode::None,
            provider: String::new(),
            login_url: None,
            logout_url: None,
            issuer: None,
        }
    }

    // ── Establishing an identity ─────────────────────────────────────────────

    /// Turn a request's credentials into an identity.
    ///
    /// One entry point rather than three, so the precedence is written once:
    /// in `proxy` mode only the headers are consulted, and in `oidc` mode the
    /// session cookie wins over a bearer token. A browser that holds both is
    /// a browser whose cookie is the more recent statement.
    pub async fn authenticate<'a>(
        &self,
        cookie_header: Option<&str>,
        authorization: Option<&str>,
        header: impl Fn(&str) -> Option<&'a str>,
    ) -> AuthResult<Identity> {
        if self.config.mode == AuthMode::Proxy {
            let missing = match proxy::identity_from(
                &self.config.proxy_headers,
                &self.config.roles,
                header,
            ) {
                Ok(identity) => return Ok(identity),
                // Not a refusal yet: a producer inside the cluster reaches
                // the Service directly and never passes the proxy, so it
                // presents a token instead of headers it cannot have.
                Err(error @ AuthError::ProxyIdentityMissing { .. }) => error,
                Err(other) => return Err(other),
            };
            return self
                .identity_from_ingest_token(authorization)
                .ok_or(missing);
        }

        if let Some(session) =
            cookie_header.and_then(|raw| cookie::read(raw, &self.config.cookie_name))
        {
            return self.signer.open::<Identity>(session);
        }

        // Before the JWT path, and cheaper: a static token is a string
        // comparison, and one that matches is not going to also be a token the
        // provider issued.
        if let Some(identity) = self.identity_from_ingest_token(authorization) {
            return Ok(identity);
        }

        if self.config.allow_bearer
            && let Some(token) = authorization.and_then(bearer_token)
        {
            return self.identity_from_bearer(token).await;
        }

        Err(AuthError::Unauthenticated)
    }

    /// A producer's shared secret, if it is one of the configured ones.
    ///
    /// Compared in constant time against every candidate rather than looked up
    /// in a map: the comparison is the check, and a hash map lookup on a
    /// secret is a timing oracle for its prefix.
    fn identity_from_ingest_token(&self, authorization: Option<&str>) -> Option<Identity> {
        let presented = authorization.and_then(bearer_token)?;
        let matched = self.config.ingest_tokens.iter().find(|candidate| {
            constant_time_eq(candidate.secret.as_bytes(), presented.as_bytes())
        })?;

        Some(Identity {
            subject: format!("token:{}", matched.label),
            username: Some(matched.label.clone()),
            name: None,
            email: None,
            groups: Vec::new(),
            // Never from the role mapping, and never admin. A secret sitting
            // in an agent's environment must not be able to ask an
            // orchestrator to run something.
            roles: vec![Role::Editor],
            expires_at: None,
            credential: Credential::Token,
        })
    }

    /// A machine caller's token, verified the same way an id token is.
    async fn identity_from_bearer(&self, token: &str) -> AuthResult<Identity> {
        let verifier = self
            .verifier
            .as_ref()
            .ok_or_else(|| AuthError::Configuration("no token verifier is configured".into()))?;
        let claims = verifier.verify(token, None).await?;
        self.identity_from_claims(claims, Credential::Bearer, None)
    }

    fn identity_from_claims(
        &self,
        claims: Claims,
        credential: Credential,
        extra_groups: Option<Vec<String>>,
    ) -> AuthResult<Identity> {
        let groups = extra_groups.unwrap_or_else(|| claims.groups(&self.config.groups_claim));
        let roles = self.config.roles.resolve(&claims.sub, &groups)?;
        Ok(Identity {
            subject: claims.sub,
            username: claims.preferred_username,
            name: claims.name,
            email: claims.email,
            groups,
            roles,
            expires_at: claims.exp,
            credential,
        })
    }

    // ── The login flow ───────────────────────────────────────────────────────

    /// Begin a sign-in.
    ///
    /// PKCE even though this is usually a confidential client. The code
    /// travels back through the user's browser either way, and S256 is what
    /// stops one intercepted there from being redeemable by anybody but the
    /// process that started the flow.
    pub fn begin_login(&self, next: Option<&str>) -> AuthResult<LoginRedirect> {
        let provider = self
            .provider
            .as_ref()
            .ok_or_else(|| AuthError::Configuration("this instance has no OIDC provider".into()))?;

        let state = signing::random_token()?;
        let verifier = signing::random_token()?;
        let nonce = signing::random_token()?;
        let next = safe_next(next)?;

        let mut url = url::Url::parse(&provider.authorization_endpoint).map_err(|error| {
            AuthError::Configuration(format!("the authorization endpoint is not a URL: {error}"))
        })?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_url)
            .append_pair("scope", &self.config.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &signing::pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");

        let sealed = self.signer.seal(
            &LoginState {
                state,
                verifier,
                nonce,
                next,
            },
            // Ten minutes: long enough to type a password and answer a second
            // factor, short enough that an abandoned tab cannot be resumed
            // tomorrow.
            time::Duration::minutes(10),
        )?;

        Ok(LoginRedirect {
            url: url.to_string(),
            cookie: self
                .cookie(format!("{}_login", self.config.cookie_name), sealed)
                .with_max_age(600),
        })
    }

    /// Finish a sign-in: check the state, redeem the code, verify the id token.
    pub async fn complete_login(
        &self,
        code: &str,
        state_param: &str,
        state_cookie: Option<&str>,
    ) -> AuthResult<SignedIn> {
        let provider = self
            .provider
            .as_ref()
            .ok_or_else(|| AuthError::Configuration("this instance has no OIDC provider".into()))?;
        let verifier = self
            .verifier
            .as_ref()
            .ok_or_else(|| AuthError::Configuration("no token verifier is configured".into()))?;

        let sealed = state_cookie
            .and_then(|raw| cookie::read(raw, &format!("{}_login", self.config.cookie_name)))
            .ok_or(AuthError::State("no sign-in was started in this browser"))?;
        let login: LoginState = self
            .signer
            .open(sealed)
            .map_err(|_| AuthError::State("the sign-in expired or was not started here"))?;

        if !constant_time_eq(login.state.as_bytes(), state_param.as_bytes()) {
            return Err(AuthError::State("the state parameter does not match"));
        }

        let tokens = self.redeem(provider, code, &login.verifier).await?;
        let id_token = tokens
            .id_token
            .ok_or_else(|| AuthError::Token("the provider returned no id token".into()))?;
        let claims = verifier.verify(&id_token, Some(&login.nonce)).await?;

        // Groups from the id token when they are there, and from userinfo when
        // they are not. The second case is an application whose scope mapping
        // was never added, whose symptom is that everybody is a viewer.
        let mut groups = claims.groups(&self.config.groups_claim);
        if groups.is_empty()
            && self.config.userinfo_fallback
            && let Some(found) = self
                .groups_from_userinfo(provider, tokens.access_token.as_deref())
                .await
        {
            tracing::debug!(
                count = found.len(),
                "the id token carried no groups; took them from userinfo"
            );
            groups = found;
        }

        let identity = self.identity_from_claims(claims, Credential::Session, Some(groups))?;
        let ttl = time::Duration::try_from(self.config.session_ttl)
            .unwrap_or_else(|_| time::Duration::hours(8));
        let session = self.seal_session(&identity, ttl)?;

        tracing::info!(
            subject = identity.log_subject(),
            role = %identity.role(),
            "signed in"
        );

        Ok(SignedIn {
            session: self
                .cookie(self.config.cookie_name.clone(), session)
                .with_max_age(ttl.whole_seconds()),
            next: login.next,
            state_cleanup: self
                .cookie(format!("{}_login", self.config.cookie_name), String::new())
                .removal(),
            identity,
        })
    }

    /// The session cookie's value, small enough that a browser will keep it.
    ///
    /// A cookie over roughly 4 KB is not rejected, it is *dropped* — silently,
    /// by the browser — and the symptom is a sign-in that succeeds and lands
    /// back on the sign-in screen, for one person, the one in ninety groups.
    /// So the group list is trimmed when it does not fit. Nothing is lost that
    /// a decision depends on: the roles were resolved from those groups before
    /// this is called, and what remains is a display detail.
    fn seal_session(&self, identity: &Identity, ttl: time::Duration) -> AuthResult<String> {
        let sealed = self.signer.seal(identity, ttl)?;
        if sealed.len() <= MAX_SESSION_COOKIE_BYTES {
            return Ok(sealed);
        }

        tracing::info!(
            subject = identity.log_subject(),
            groups = identity.groups.len(),
            bytes = sealed.len(),
            "the session cookie is too large for a browser to keep; storing the roles without \
             the group list they were resolved from"
        );
        self.signer.seal(
            &Identity {
                groups: Vec::new(),
                ..identity.clone()
            },
            ttl,
        )
    }

    /// Exchange the code for tokens.
    async fn redeem(
        &self,
        provider: &ProviderMetadata,
        code: &str,
        verifier: &str,
    ) -> AuthResult<TokenResponse> {
        // Encoded here rather than through reqwest's `form` helper, which
        // would mean turning on a workspace-wide feature for the one request
        // in this workspace that posts a form.
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", code)
            .append_pair("redirect_uri", &self.config.redirect_url)
            .append_pair("client_id", &self.config.client_id)
            .append_pair("code_verifier", verifier)
            .finish();

        let mut request = self
            .http
            .post(&provider.token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body);
        if let Some(secret) = &self.config.client_secret {
            // `client_secret_basic`, which is what authentik's OAuth2 provider
            // expects by default. A secret in the form body is the fallback
            // some providers need and the one that ends up in more logs.
            request = request.basic_auth(&self.config.client_id, Some(secret));
        }

        let response = request.send().await.map_err(|error| AuthError::Exchange {
            status: 0,
            message: error.to_string(),
        })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(AuthError::Exchange {
                status: status.as_u16(),
                // The provider's own error, verbatim: `invalid_client` and
                // `redirect_uri_mismatch` are the two things that go wrong
                // here, and both are unrecognisable when paraphrased.
                message: body.chars().take(500).collect(),
            });
        }

        serde_json::from_str(&body).map_err(|error| AuthError::Exchange {
            status: status.as_u16(),
            message: format!("the token response is not JSON this understands: {error}"),
        })
    }

    /// Groups from `userinfo`, or `None` if it cannot be read.
    ///
    /// Never an error: this is a fallback, and failing a sign-in because an
    /// optional lookup did not answer would be worse than the missing groups
    /// it is trying to fill in.
    async fn groups_from_userinfo(
        &self,
        provider: &ProviderMetadata,
        access_token: Option<&str>,
    ) -> Option<Vec<String>> {
        let (endpoint, token) = (provider.userinfo_endpoint.as_ref()?, access_token?);
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let claims = response.json::<Claims>().await.ok()?;
        let groups = claims.groups(&self.config.groups_claim);
        (!groups.is_empty()).then_some(groups)
    }

    /// Sign out: the cookie to clear, and where to send the browser.
    ///
    /// Two halves because they answer different questions. Clearing the cookie
    /// ends the session *here*; the provider's `end_session_endpoint` ends it
    /// *there*, and a sign-out that only did the first would put the user back
    /// in with one click on the sign-in button, which reads as a sign-out that
    /// did not work.
    #[must_use]
    pub fn logout(&self) -> (CookieSpec, Option<String>) {
        let cleared = self
            .cookie(self.config.cookie_name.clone(), String::new())
            .removal();

        let endpoint = self
            .provider
            .as_ref()
            .and_then(|provider| provider.end_session_endpoint.clone());

        let url = endpoint.map(|endpoint| match &self.config.post_logout_url {
            Some(next) => {
                let mut url = match url::Url::parse(&endpoint) {
                    Ok(url) => url,
                    Err(_) => return endpoint.clone(),
                };
                url.query_pairs_mut()
                    .append_pair("post_logout_redirect_uri", next)
                    .append_pair("client_id", &self.config.client_id);
                url.to_string()
            }
            None => endpoint,
        });

        (cleared, url)
    }

    fn cookie(&self, name: String, value: String) -> CookieSpec {
        CookieSpec::new(name, value)
            .with_secure(self.config.cookie_secure)
            .with_domain(self.config.cookie_domain.clone())
    }
}

/// The `Bearer` value out of an `Authorization` header.
#[must_use]
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

/// Where a login is allowed to land.
///
/// The `next=` parameter is chosen by whoever built the link, which on a login
/// route means anybody: an open redirect on a sign-in endpoint is the standard
/// way to make a phishing link that starts on the real host. So only a path on
/// this application is accepted — one leading slash, no second one (which the
/// browser reads as a host), no backslash (which some parsers read as one),
/// and no control characters.
pub fn safe_next(raw: Option<&str>) -> AuthResult<String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok("/".to_owned());
    };
    let looks_like_a_host = raw.starts_with("//") || raw.starts_with("/\\");
    let is_a_path = raw.starts_with('/');
    let is_clean = !raw.contains('\\') && !raw.chars().any(char::is_control);

    if !is_a_path || looks_like_a_host || !is_clean {
        return Err(AuthError::UnsafeRedirect(raw.to_owned()));
    }
    Ok(raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_accept_the_spellings_people_actually_write() {
        for (raw, expected) in [
            ("none", AuthMode::None),
            ("off", AuthMode::None),
            ("oidc", AuthMode::Oidc),
            // What somebody setting this up is actually thinking about.
            ("authentik", AuthMode::Oidc),
            ("SSO", AuthMode::Oidc),
            ("proxy", AuthMode::Proxy),
            ("forward-auth", AuthMode::Proxy),
        ] {
            assert_eq!(raw.parse::<AuthMode>().expect(raw), expected, "{raw}");
        }
        assert!("saml".parse::<AuthMode>().is_err());
    }

    #[test]
    fn the_default_is_no_authentication_at_all() {
        // Every other setting in this workspace has a default that works with
        // nothing running, and a release that started refusing requests would
        // be an upgrade that took an installation down.
        let config = AuthConfig::default();
        assert_eq!(config.mode, AuthMode::None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn an_incomplete_oidc_configuration_is_refused_before_anything_listens() {
        let complete = AuthConfig {
            mode: AuthMode::Oidc,
            issuer: "https://auth.example.test/application/o/aiwatcher/".to_owned(),
            client_id: "aiwatcher".to_owned(),
            redirect_url: "https://aiwatcher.example.test/api/v1/auth/callback".to_owned(),
            ..AuthConfig::default()
        };
        assert!(complete.validate().is_ok());

        for incomplete in [
            AuthConfig {
                issuer: String::new(),
                ..complete.clone()
            },
            AuthConfig {
                client_id: String::new(),
                ..complete.clone()
            },
            AuthConfig {
                redirect_url: String::new(),
                ..complete.clone()
            },
            AuthConfig {
                // Relative, which the provider cannot redirect to.
                redirect_url: "/api/v1/auth/callback".to_owned(),
                ..complete.clone()
            },
            AuthConfig {
                scopes: vec!["profile".to_owned()],
                ..complete.clone()
            },
        ] {
            assert!(
                incomplete.validate().is_err(),
                "{:?} must not start",
                incomplete.redirect_url
            );
        }
    }

    #[test]
    fn the_audience_defaults_to_the_client_id() {
        let config = AuthConfig {
            client_id: "aiwatcher".to_owned(),
            ..AuthConfig::default()
        };
        assert!(config.audience_set().contains("aiwatcher"));
    }

    #[test]
    fn a_bearer_header_yields_its_token_and_nothing_else_does() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[test]
    fn a_login_may_only_return_to_a_path_on_this_application() {
        // An open redirect on a sign-in route is how a phishing link gets to
        // start on the real host.
        assert_eq!(
            safe_next(Some("/observability/runs")).expect("ok"),
            "/observability/runs"
        );
        assert_eq!(safe_next(None).expect("ok"), "/");
        assert_eq!(safe_next(Some("  ")).expect("ok"), "/");
        for hostile in [
            "//evil.test/",
            "https://evil.test/",
            "/\\evil.test",
            "/runs\\..\\x",
            "/runs\nSet-Cookie: x=1",
        ] {
            assert!(safe_next(Some(hostile)).is_err(), "{hostile}");
        }
    }

    #[test]
    fn an_ingest_token_is_read_with_or_without_a_label() {
        let labelled: IngestToken = "agents=0123456789abcdef0123456789".parse().expect("valid");
        assert_eq!(labelled.label, "agents");
        let bare: IngestToken = "0123456789abcdef0123456789".parse().expect("valid");
        assert_eq!(bare.label, "ingest");
    }

    #[test]
    fn a_guessable_ingest_token_is_refused() {
        // Worse than no authentication, because it looks like authentication.
        assert!("agents=hunter2".parse::<IngestToken>().is_err());
        assert!("short".parse::<IngestToken>().is_err());
    }

    #[test]
    fn an_ingest_token_never_prints_itself() {
        // It is reachable from `AppState`, and something above that derives
        // `Debug`.
        let token: IngestToken = "agents=0123456789abcdef0123456789".parse().expect("valid");
        let rendered = format!("{token:?}");
        assert!(rendered.contains("agents"));
        assert!(!rendered.contains("0123456789"), "{rendered}");
    }

    #[test]
    fn a_disabled_instance_still_answers_the_config_route() {
        // The panel has to be able to ask; guessing wrong means either a
        // sign-in screen on an instance with no provider, or a 401 loop.
        let config = Authenticator::disabled_config();
        assert!(!config.enabled);
        assert!(config.login_url.is_none());
    }
}
