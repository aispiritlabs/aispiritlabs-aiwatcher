//! Signing in, and what a signed-in caller may do.
//!
//! Five routes and one layer. The layer is the part that matters: it runs in
//! front of every route in this crate, so a route added later is authenticated
//! by default rather than by remembering to say so. What it does *not* do is
//! decide whether a caller is allowed to perform an operation — that is a
//! [`Role`] check in the handler, because the answer differs per handler and a
//! table of paths in a middleware would drift from the routes it guards.
//!
//! The exceptions are listed in [`is_public`] and they are the smallest set
//! that works: the health probes, which a kubelet reaches with no credential,
//! and the sign-in routes themselves, because a login that required a session
//! could never establish one.
//!
//! See `aiwatcher_auth` for why the session is a cookie and not a header, and
//! for what `proxy` mode trusts.

use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Query, Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use aiwatcher_auth::{AuthMode, CookieSpec, Identity, PublicAuthConfig, Role};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/config", get(auth_config))
        .route("/api/v1/auth/login", get(login))
        .route("/api/v1/auth/callback", get(callback))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
}

/// Reachable without a credential.
///
/// `/api/v1/auth/me` is deliberately not here: an unauthenticated caller
/// getting a 401 from it is exactly what tells the panel to render its sign-in
/// screen, and an anonymous body would make "signed out" indistinguishable
/// from "signed in as nobody".
fn is_public(path: &str) -> bool {
    matches!(
        path,
        "/livez"
            | "/healthz"
            | "/readyz"
            | "/api/v1/auth/config"
            | "/api/v1/auth/login"
            | "/api/v1/auth/callback"
            // Public so that signing out of an already-expired session still
            // clears the cookie instead of answering 401 and leaving it set.
            | "/api/v1/auth/logout"
    )
}

/// Establish the caller's identity, or refuse the request.
///
/// Runs for every route. With no provider configured it inserts
/// [`Identity::anonymous`], which satisfies every role check — that is what
/// `AIWATCHER_AUTH_MODE=none` means, and doing it here rather than in each
/// handler keeps the disabled case from being a second code path.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(auth) = state.auth.clone() else {
        request.extensions_mut().insert(Identity::anonymous());
        return next.run(request).await;
    };

    if is_public(request.uri().path()) {
        return next.run(request).await;
    }

    let headers = request.headers().clone();
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let result = auth
        .authenticate(
            header(COOKIE.as_str()),
            header(AUTHORIZATION.as_str()),
            header,
        )
        .await;

    match result {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// Whoever is making this request.
///
/// An extractor rather than a lookup so a handler that needs it says so in its
/// signature — which is also what makes the ones that do not need it obvious.
#[derive(Clone, Debug)]
pub struct Caller(pub Identity);

impl Caller {
    /// The identity, or a 403 naming what was needed.
    pub fn require(&self, role: Role) -> ApiResult<&Identity> {
        if self.0.can(role) {
            return Ok(&self.0);
        }
        Err(ApiError::Forbidden {
            needed: role,
            held: self.0.role(),
        })
    }

    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.0
    }
}

impl<S> FromRequestParts<S> for Caller
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Identity>()
            .cloned()
            .map(Caller)
            .ok_or(ApiError::Unauthenticated)
    }
}

// ── Routes ───────────────────────────────────────────────────────────────────

/// Whether this instance has a login, and what to call it.
///
/// The one route besides the health probes that answers before anybody has
/// signed in. Without it the panel would have to guess, and guessing wrong
/// means either a sign-in screen on an instance with no provider or an endless
/// 401 loop on one that has.
#[utoipa::path(
    get,
    path = "/api/v1/auth/config",
    responses((status = 200, body = PublicAuthConfig)),
    tag = "auth",
)]
pub async fn auth_config(State(state): State<AppState>) -> Json<PublicAuthConfig> {
    Json(match &state.auth {
        Some(auth) => auth.public_config(),
        None => aiwatcher_auth::Authenticator::disabled_config(),
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct LoginQuery {
    /// Where to land after signing in. A path on this application; anything
    /// else is refused rather than followed — see `aiwatcher_auth::safe_next`.
    #[serde(default)]
    pub next: Option<String>,
}

/// Start a sign-in: a redirect to the provider.
#[utoipa::path(
    get,
    path = "/api/v1/auth/login",
    params(LoginQuery),
    responses(
        (status = 303, description = "Redirect to the identity provider"),
        (status = 501, description = "This instance has no identity provider"),
    ),
    tag = "auth",
)]
pub async fn login(
    State(state): State<AppState>,
    query: Result<Query<LoginQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let auth = provider(&state)?;
    let next = query.ok().and_then(|Query(query)| query.next);
    let redirect = auth.begin_login(next.as_deref())?;
    Ok(redirect_with(&redirect.url, &[redirect.cookie]))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// What the provider sends instead of a code when it refused —
    /// `access_denied` when a user cancelled, most often.
    #[serde(default)]
    pub error: Option<String>,
}

/// Where the provider sends the browser back to.
///
/// Every failure here ends in a redirect rather than a JSON error, because
/// this is a top-level navigation: whatever this returns is what the person
/// sees in their browser. The reason travels as `sign_in_error`, which the
/// panel reads and renders on its sign-in screen.
#[utoipa::path(
    get,
    path = "/api/v1/auth/callback",
    params(CallbackQuery),
    responses((status = 303, description = "Redirect back into the application")),
    tag = "auth",
)]
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<CallbackQuery>, QueryRejection>,
) -> Response {
    let Ok(auth) = provider(&state) else {
        return sign_in_error("not_configured", &[]);
    };
    let Ok(Query(query)) = query else {
        return sign_in_error("malformed_callback", &[]);
    };

    if let Some(error) = query.error {
        tracing::info!(%error, "the identity provider refused a sign-in");
        return sign_in_error(&error, &[]);
    }

    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return sign_in_error("missing_code", &[]);
    };

    let cookies = headers.get(COOKIE).and_then(|value| value.to_str().ok());
    match auth.complete_login(&code, &returned_state, cookies).await {
        Ok(signed_in) => redirect_with(
            &signed_in.next,
            &[signed_in.session, signed_in.state_cleanup],
        ),
        Err(error) => {
            // The detail goes to the log and a code goes to the browser. The
            // difference between a bad signature and a wrong audience is
            // useful to whoever is attacking and to nobody else.
            tracing::warn!(%error, "a sign-in could not be completed");
            sign_in_error(sign_in_error_code(&error), &[])
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LoggedOut {
    /// Where to send the browser to end the session at the provider too.
    /// `None` when the provider declares no `end_session_endpoint`, in which
    /// case clearing the cookie is all a sign-out can do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_url: Option<String>,
}

/// Sign out.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    responses((status = 200, body = LoggedOut)),
    tag = "auth",
)]
pub async fn logout(State(state): State<AppState>) -> Response {
    let Some(auth) = state.auth.as_ref() else {
        return Json(LoggedOut { redirect_url: None }).into_response();
    };
    let (cleared, redirect_url) = auth.logout();
    let mut response = Json(LoggedOut { redirect_url }).into_response();
    set_cookies(&mut response, &[cleared]);
    response
}

/// The current caller.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    responses(
        (status = 200, body = Identity),
        (status = 401, description = "Not signed in"),
    ),
    tag = "auth",
)]
pub async fn me(caller: Caller) -> Json<Identity> {
    Json(caller.0)
}

// ── Plumbing ─────────────────────────────────────────────────────────────────

/// The authenticator, or a 501 saying this instance has no login.
///
/// 501 rather than 404, for the same reason the prompt registry answers 501:
/// the route exists in the contract and this deployment wired nothing behind
/// it. A caller can tell "you may not" from "nobody can here".
fn provider(state: &AppState) -> ApiResult<std::sync::Arc<aiwatcher_auth::Authenticator>> {
    match state.auth.as_ref() {
        Some(auth) if auth.mode() == AuthMode::Oidc => Ok(std::sync::Arc::clone(auth)),
        _ => Err(ApiError::AuthDisabled),
    }
}

fn redirect_with(location: &str, cookies: &[CookieSpec]) -> Response {
    let mut response = Redirect::to(location).into_response();
    set_cookies(&mut response, cookies);
    response
}

fn set_cookies(response: &mut Response, cookies: &[CookieSpec]) {
    for cookie in cookies {
        match HeaderValue::from_str(&cookie.to_string()) {
            Ok(value) => {
                response.headers_mut().append(SET_COOKIE, value);
            }
            Err(error) => tracing::error!(%error, "a cookie could not be rendered as a header"),
        }
    }
}

/// Back into the application, carrying why the sign-in did not happen.
fn sign_in_error(code: &str, cookies: &[CookieSpec]) -> Response {
    let encoded: String = code
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .take(64)
        .collect();
    redirect_with(&format!("/?sign_in_error={encoded}"), cookies)
}

fn sign_in_error_code(error: &aiwatcher_auth::AuthError) -> &'static str {
    use aiwatcher_auth::AuthError;
    match error {
        AuthError::State(_) => "state_mismatch",
        AuthError::NotEntitled(_) => "not_entitled",
        AuthError::Exchange { .. } => "exchange_failed",
        AuthError::Token(_) => "invalid_token",
        _ => "sign_in_failed",
    }
}
