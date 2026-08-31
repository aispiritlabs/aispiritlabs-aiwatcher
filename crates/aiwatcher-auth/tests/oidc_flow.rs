// See the note in aiwatcher-bus/tests: the clippy.toml allowances only reach
// `#[cfg(test)]` modules, and this is a separate crate.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The whole sign-in, against a provider that is really there.
//!
//! The unit tests in this crate check the pieces — the PKCE derivation, the
//! algorithm allowlist, the cookie attributes, the role mapping. None of them
//! proves that the pieces fit, and the failures that matter in a relying party
//! are almost all seams: a nonce that is generated and never checked, an
//! audience that is set and never enforced, a state cookie written under one
//! name and read under another.
//!
//! So this stands up an OpenID provider on a loopback socket — a discovery
//! document, a key set, and a token endpoint that mints a real RS256 id token
//! with the key in `tests/fixtures` — and drives [`Authenticator`] through it
//! exactly as the API's routes do.

use std::collections::HashMap;
use std::sync::Arc;

use aiwatcher_auth::{AuthConfig, AuthMode, Authenticator, Credential, Role, RoleMapping};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SIGNING_KEY: &str = include_str!("fixtures/signing-key.pem");
const MODULUS: &str = include_str!("fixtures/signing-key.modulus");
const KID: &str = "test-signing-key";

/// What the fake provider was asked for, so the test can assert on it.
#[derive(Debug, Default)]
struct Seen {
    /// The form fields of the token request, once it happens.
    token_request: HashMap<String, String>,
}

struct Provider {
    issuer: String,
    seen: Arc<tokio::sync::Mutex<Seen>>,
    /// The nonce to put in the next id token. `None` leaves it out entirely,
    /// which is how the "a token with no nonce is refused" case is built.
    nonce: Arc<tokio::sync::Mutex<Option<String>>>,
    audience: Arc<tokio::sync::Mutex<String>>,
    groups: Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl Provider {
    /// Start one, and return it once it is listening.
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let issuer = format!("http://127.0.0.1:{port}");
        let seen = Arc::new(tokio::sync::Mutex::new(Seen::default()));
        let nonce = Arc::new(tokio::sync::Mutex::new(None));
        let audience = Arc::new(tokio::sync::Mutex::new("aiwatcher".to_owned()));
        let groups = Arc::new(tokio::sync::Mutex::new(vec![
            "everyone".to_owned(),
            "aiwatcher-admins".to_owned(),
        ]));

        let task = Provider {
            issuer: issuer.clone(),
            seen: Arc::clone(&seen),
            nonce: Arc::clone(&nonce),
            audience: Arc::clone(&audience),
            groups: Arc::clone(&groups),
        };

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                task.serve(stream).await;
            }
        });

        Provider {
            issuer,
            seen,
            nonce,
            audience,
            groups,
        }
    }

    async fn serve(&self, mut stream: tokio::net::TcpStream) {
        let mut buffer = vec![0_u8; 8192];
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();

        // Answered on any prefix, not just the root, so a test can point the
        // relying party at a path this provider does not actually serve and
        // still get a document back — which is what the issuer-mismatch case
        // needs, and what a misconfigured deployment really hits.
        let body = if path.ends_with("/.well-known/openid-configuration") {
            serde_json::json!({
                "issuer": self.issuer,
                "authorization_endpoint": format!("{}/authorize", self.issuer),
                "token_endpoint": format!("{}/token", self.issuer),
                "jwks_uri": format!("{}/jwks", self.issuer),
                "end_session_endpoint": format!("{}/end-session", self.issuer),
            })
        } else if path.starts_with("/jwks") {
            serde_json::json!({
                "keys": [{
                    "kty": "RSA",
                    "kid": KID,
                    "use": "sig",
                    "alg": "RS256",
                    "n": MODULUS.trim(),
                    "e": "AQAB",
                }]
            })
        } else if path.starts_with("/token") {
            let form = request.rsplit("\r\n\r\n").next().unwrap_or_default();
            self.seen.lock().await.token_request = url::form_urlencoded::parse(form.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            serde_json::json!({
                "access_token": "an-access-token",
                "token_type": "Bearer",
                "id_token": self.id_token().await,
            })
        } else {
            serde_json::json!({ "error": "not_found" })
        };

        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
    }

    async fn id_token(&self) -> String {
        let mut claims = serde_json::json!({
            "iss": self.issuer,
            "sub": "5f2c-alice",
            "aud": *self.audience.lock().await,
            "exp": time::OffsetDateTime::now_utc().unix_timestamp() + 300,
            "iat": time::OffsetDateTime::now_utc().unix_timestamp(),
            "email": "alice@example.test",
            "preferred_username": "alice",
            "name": "Alice Example",
            "groups": *self.groups.lock().await,
        });
        if let Some(nonce) = self.nonce.lock().await.as_ref() {
            claims["nonce"] = serde_json::json!(nonce);
        }

        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(KID.to_owned());
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(SIGNING_KEY.as_bytes()).expect("a key"),
        )
        .expect("a token")
    }
}

async fn authenticator(provider: &Provider) -> Authenticator {
    Authenticator::connect(AuthConfig {
        mode: AuthMode::Oidc,
        issuer: provider.issuer.clone(),
        client_id: "aiwatcher".to_owned(),
        client_secret: Some("a-client-secret".to_owned()),
        redirect_url: "http://localhost:5173/api/v1/auth/callback".to_owned(),
        session_secret: Some("a-session-signing-key".to_owned()),
        roles: RoleMapping::default(),
        ..AuthConfig::default()
    })
    .await
    .expect("discovery succeeds against a provider that is up")
    .expect("oidc mode produces an authenticator")
}

/// Pull one query parameter out of the authorize URL.
fn parameter(url: &str, name: &str) -> String {
    url::Url::parse(url)
        .expect("a url")
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| panic!("{name} is not on the authorize URL: {url}"))
}

/// What a browser sends back: the `Cookie` header holding what was set.
fn as_cookie_header(cookie: &aiwatcher_auth::CookieSpec) -> String {
    format!("{}={}", cookie.name, cookie.value)
}

#[tokio::test]
async fn a_sign_in_ends_in_a_session_carrying_the_roles_the_groups_map_to() {
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;

    // 1. The redirect out. Everything the provider needs is on the query
    //    string, and the half that has to come back is sealed in a cookie.
    let redirect = auth
        .begin_login(Some("/observability/runs"))
        .expect("a redirect");
    assert_eq!(parameter(&redirect.url, "response_type"), "code");
    assert_eq!(parameter(&redirect.url, "client_id"), "aiwatcher");
    assert_eq!(parameter(&redirect.url, "code_challenge_method"), "S256");
    assert!(
        redirect.cookie.http_only,
        "the state must not reach JavaScript"
    );

    let state = parameter(&redirect.url, "state");
    let nonce = parameter(&redirect.url, "nonce");
    *provider.nonce.lock().await = Some(nonce.clone());

    // 2. The redirect back. The provider hands over a code; the verifier comes
    //    out of the cookie, which is the whole reason PKCE survives the round
    //    trip through somebody else's website.
    let signed_in = auth
        .complete_login(
            "an-authorization-code",
            &state,
            Some(&as_cookie_header(&redirect.cookie)),
        )
        .await
        .expect("the sign-in completes");

    assert_eq!(signed_in.identity.subject, "5f2c-alice");
    assert_eq!(signed_in.identity.username.as_deref(), Some("alice"));
    assert_eq!(
        signed_in.identity.email.as_deref(),
        Some("alice@example.test")
    );
    assert!(
        signed_in.identity.can(Role::Admin),
        "aiwatcher-admins maps to admin"
    );
    assert_eq!(signed_in.identity.credential, Credential::Session);
    assert_eq!(
        signed_in.next, "/observability/runs",
        "the page they were on before signing in"
    );

    // The code was redeemed with the verifier matching the challenge sent, and
    // the challenge is not the verifier.
    let seen = provider.seen.lock().await;
    let verifier = seen.token_request.get("code_verifier").expect("a verifier");
    assert_eq!(
        aiwatcher_auth::signing::pkce_challenge(verifier),
        parameter(&redirect.url, "code_challenge")
    );
    assert_eq!(
        seen.token_request.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    drop(seen);

    // 3. The session cookie is what every later request carries, and it comes
    //    back as the same identity.
    assert!(signed_in.session.http_only);
    let restored = auth
        .authenticate(Some(&as_cookie_header(&signed_in.session)), None, |_| None)
        .await
        .expect("the session is accepted");
    assert_eq!(restored.subject, "5f2c-alice");
    assert!(restored.can(Role::Admin));

    // And the state cookie is cleared, having done its job.
    assert_eq!(signed_in.state_cleanup.max_age, Some(0));
}

#[tokio::test]
async fn a_callback_whose_state_does_not_match_is_refused() {
    // The check that stops a sign-in somebody else started from landing in
    // this browser.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let redirect = auth.begin_login(None).expect("a redirect");
    *provider.nonce.lock().await = Some(parameter(&redirect.url, "nonce"));

    let error = auth
        .complete_login(
            "a-code",
            "a-state-nobody-issued",
            Some(&as_cookie_header(&redirect.cookie)),
        )
        .await
        .expect_err("refused");
    assert!(
        matches!(error, aiwatcher_auth::AuthError::State(_)),
        "{error}"
    );
}

#[tokio::test]
async fn a_callback_with_no_state_cookie_at_all_is_refused() {
    // What a link somebody was sent looks like: a callback arriving in a
    // browser that never started a sign-in.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let redirect = auth.begin_login(None).expect("a redirect");

    let error = auth
        .complete_login("a-code", &parameter(&redirect.url, "state"), None)
        .await
        .expect_err("refused");
    assert!(
        matches!(error, aiwatcher_auth::AuthError::State(_)),
        "{error}"
    );
}

#[tokio::test]
async fn an_id_token_without_the_nonce_that_was_asked_for_is_refused() {
    // The nonce is what ties the id token to *this* authorization request. A
    // relying party that generates one and never checks it has written the
    // parameter and not the protection.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let redirect = auth.begin_login(None).expect("a redirect");
    *provider.nonce.lock().await = None;

    let error = auth
        .complete_login(
            "a-code",
            &parameter(&redirect.url, "state"),
            Some(&as_cookie_header(&redirect.cookie)),
        )
        .await
        .expect_err("refused");
    assert!(
        matches!(error, aiwatcher_auth::AuthError::Token(_)),
        "{error}"
    );
}

#[tokio::test]
async fn an_id_token_minted_for_another_audience_is_refused() {
    // A token from the same provider, signed by the same key, issued to a
    // different application. Accepting it would make every other application
    // in the same authentik a way in here.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let redirect = auth.begin_login(None).expect("a redirect");
    *provider.nonce.lock().await = Some(parameter(&redirect.url, "nonce"));
    *provider.audience.lock().await = "some-other-application".to_owned();

    let error = auth
        .complete_login(
            "a-code",
            &parameter(&redirect.url, "state"),
            Some(&as_cookie_header(&redirect.cookie)),
        )
        .await
        .expect_err("refused");
    assert!(
        matches!(error, aiwatcher_auth::AuthError::Token(_)),
        "{error}"
    );
}

#[tokio::test]
async fn a_machine_client_may_present_the_same_token_as_a_bearer() {
    // What a client-credentials token from an authentik service account looks
    // like: no session, no cookie, the same signature and the same audience.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let token = provider.id_token().await;

    let identity = auth
        .authenticate(None, Some(&format!("Bearer {token}")), |_| None)
        .await
        .expect("the bearer is accepted");
    assert_eq!(identity.subject, "5f2c-alice");
    assert_eq!(identity.credential, Credential::Bearer);
    assert!(identity.can(Role::Admin));
}

#[tokio::test]
async fn a_discovery_document_naming_another_issuer_stops_the_start_up() {
    // Every token accepted afterwards is validated against the issuer, so
    // taking the document's word for it would hand that choice to whoever
    // answered this request.
    let provider = Provider::start().await;
    let error = Authenticator::connect(AuthConfig {
        mode: AuthMode::Oidc,
        // Reachable, and not what the document declares.
        issuer: format!("{}/somewhere-else", provider.issuer),
        client_id: "aiwatcher".to_owned(),
        redirect_url: "http://localhost:5173/api/v1/auth/callback".to_owned(),
        discovery_attempts: 1,
        ..AuthConfig::default()
    })
    .await
    .expect_err("refused");
    assert!(
        matches!(error, aiwatcher_auth::AuthError::IssuerMismatch { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn signing_out_clears_the_cookie_and_says_where_to_go_next() {
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;
    let (cleared, redirect) = auth.logout();

    assert_eq!(cleared.max_age, Some(0), "the browser has to drop it");
    assert!(cleared.value.is_empty());
    assert!(
        redirect
            .expect("the provider declares one")
            .contains("/end-session"),
        "clearing our own cookie would put them back in with one click"
    );
}

#[tokio::test]
async fn a_user_in_too_many_groups_still_gets_a_cookie_a_browser_will_keep() {
    // A cookie over roughly 4 KB is dropped by the browser rather than
    // rejected by anything, so the symptom is one person whose sign-in
    // succeeds and lands straight back on the sign-in screen. The roles were
    // resolved before the cookie was written, so trimming the group list costs
    // a display detail and nothing a decision depends on.
    let provider = Provider::start().await;
    let auth = authenticator(&provider).await;

    let mut groups: Vec<String> = (0..400)
        .map(|index| format!("planner-team-{index}"))
        .collect();
    groups.push("aiwatcher-admins".to_owned());
    *provider.groups.lock().await = groups;

    let redirect = auth.begin_login(None).expect("a redirect");
    *provider.nonce.lock().await = Some(parameter(&redirect.url, "nonce"));

    let signed_in = auth
        .complete_login(
            "a-code",
            &parameter(&redirect.url, "state"),
            Some(&as_cookie_header(&redirect.cookie)),
        )
        .await
        .expect("the sign-in completes");

    assert!(
        signed_in.session.value.len() < 4096,
        "a browser would drop this one: {} bytes",
        signed_in.session.value.len()
    );
    assert!(
        signed_in.identity.can(Role::Admin),
        "the role survives even though the group it came from was trimmed"
    );

    let restored = auth
        .authenticate(Some(&as_cookie_header(&signed_in.session)), None, |_| None)
        .await
        .expect("the trimmed session is still a valid one");
    assert!(restored.can(Role::Admin));
}
