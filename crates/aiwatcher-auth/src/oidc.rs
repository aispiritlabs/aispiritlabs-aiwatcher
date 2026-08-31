//! The provider, as this process sees it: a discovery document and a key set.
//!
//! Nothing here is authentik-specific. Authentik is an OpenID Connect provider
//! and is reached the way every other one is — `/.well-known/openid-configuration`
//! for the endpoints, `jwks_uri` for the keys. What *is* authentik-specific
//! lives in one place each: the default groups claim (`groups`), and the
//! forward-auth header names in [`crate::proxy`].

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::{AuthError, AuthResult};
use crate::signing::constant_time_eq;

/// What the discovery document tells us.
///
/// A subset: the fields a relying party needs, and `end_session_endpoint`,
/// which is optional in the spec and is the difference between signing out of
/// aiwatcher and signing out of authentik.
#[derive(Clone, Debug, Deserialize)]
pub struct ProviderMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
}

impl ProviderMetadata {
    /// Read `{issuer}/.well-known/openid-configuration`, retrying while the
    /// provider is unreachable.
    ///
    /// Retried rather than failed on the first attempt because in a cluster
    /// this process and the identity provider come up in whatever order the
    /// scheduler picks, and a relying party that gives up after one refused
    /// connection turns a five-second race into a CrashLoopBackOff. It does
    /// eventually fail, and that is the other half: an instance that cannot
    /// find its provider must not start serving, because the only thing it
    /// could do instead is serve without authentication.
    pub async fn discover(http: &reqwest::Client, issuer: &str, attempts: u32) -> AuthResult<Self> {
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let mut backoff = Duration::from_millis(500);
        let mut last: Option<Box<dyn std::error::Error + Send + Sync>> = None;

        for attempt in 1..=attempts.max(1) {
            match fetch_metadata(http, &url).await {
                Ok(metadata) => {
                    // The document decides the endpoints; it does not get to
                    // decide the issuer. Every token this process later
                    // accepts is checked against the configured value, so
                    // taking the document's word here would hand that choice
                    // to whoever answered this request.
                    if metadata.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
                        return Err(AuthError::IssuerMismatch {
                            expected: issuer.to_owned(),
                            found: metadata.issuer,
                        });
                    }
                    return Ok(metadata);
                }
                Err(error) => {
                    tracing::warn!(
                        %url,
                        attempt,
                        attempts,
                        error = %error,
                        "the identity provider is not answering discovery yet"
                    );
                    last = Some(error);
                    if attempt < attempts {
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(8));
                    }
                }
            }
        }

        Err(AuthError::Discovery {
            issuer: issuer.to_owned(),
            source: last.unwrap_or_else(|| "no attempts were made".into()),
        })
    }
}

async fn fetch_metadata(
    http: &reqwest::Client,
    url: &str,
) -> Result<ProviderMetadata, Box<dyn std::error::Error + Send + Sync>> {
    let response = http.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}").into());
    }
    Ok(response.json::<ProviderMetadata>().await?)
}

/// The provider's public keys, refreshed when a token names one we do not have.
///
/// Keys rotate, and a relying party that only fetched them at start-up starts
/// rejecting every token the moment they do. Refreshing on an unknown `kid` is
/// the standard answer; the rate limit is what stops a token carrying a
/// made-up `kid` from turning every request into a fetch of the provider.
pub struct JwksCache {
    http: reqwest::Client,
    uri: String,
    keys: RwLock<Arc<JwkSet>>,
    last_refresh: RwLock<Instant>,
    min_interval: Duration,
}

impl std::fmt::Debug for JwksCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwksCache")
            .field("uri", &self.uri)
            .finish_non_exhaustive()
    }
}

impl JwksCache {
    pub async fn load(http: reqwest::Client, uri: String) -> AuthResult<Self> {
        let keys = fetch_keys(&http, &uri).await?;
        Ok(Self {
            http,
            uri,
            keys: RwLock::new(Arc::new(keys)),
            last_refresh: RwLock::new(Instant::now()),
            min_interval: Duration::from_secs(60),
        })
    }

    /// The key for `kid`, refetching once if it is not held.
    pub async fn key(&self, kid: &str) -> AuthResult<Jwk> {
        if let Some(jwk) = self.keys.read().await.find(kid) {
            return Ok(jwk.clone());
        }

        {
            let last = *self.last_refresh.read().await;
            if last.elapsed() < self.min_interval {
                return Err(AuthError::Token(format!(
                    "signed with an unknown key ({kid})"
                )));
            }
        }

        tracing::info!(kid, uri = %self.uri, "refetching the provider's key set");
        let fresh = fetch_keys(&self.http, &self.uri).await?;
        let found = fresh.find(kid).cloned();
        *self.keys.write().await = Arc::new(fresh);
        *self.last_refresh.write().await = Instant::now();

        found.ok_or_else(|| AuthError::Token(format!("signed with an unknown key ({kid})")))
    }
}

async fn fetch_keys(http: &reqwest::Client, uri: &str) -> AuthResult<JwkSet> {
    let response = http
        .get(uri)
        .send()
        .await
        .map_err(|error| AuthError::Jwks(format!("{uri}: {error}")))?;
    if !response.status().is_success() {
        return Err(AuthError::Jwks(format!(
            "{uri}: HTTP {}",
            response.status()
        )));
    }
    response
        .json::<JwkSet>()
        .await
        .map_err(|error| AuthError::Jwks(format!("{uri}: {error}")))
}

/// The claims this crate reads, plus everything else the provider sent.
///
/// `extra` is not laziness: the groups claim is configurable, because a
/// deployment that already carries authorisation groups under some other name
/// should not have to duplicate them into `groups` to use aiwatcher.
#[derive(Clone, Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub exp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Claims {
    /// The named claim, as a list of group names.
    ///
    /// Three shapes are accepted because three are in the wild: a JSON array
    /// (authentik, Keycloak), one string, and a space- or comma-separated
    /// string. A claim that is present but none of those yields no groups
    /// rather than an error — the caller is then an authenticated user in no
    /// group, which the role mapping already has an answer for.
    #[must_use]
    pub fn groups(&self, claim: &str) -> Vec<String> {
        match self.extra.get(claim) {
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            Some(serde_json::Value::String(value)) => value
                .split([',', ' ', '|'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Verifies tokens against the provider's keys.
#[derive(Debug)]
pub struct TokenVerifier {
    jwks: JwksCache,
    issuer: String,
    audiences: HashSet<String>,
}

impl TokenVerifier {
    #[must_use]
    pub fn new(jwks: JwksCache, issuer: String, audiences: HashSet<String>) -> Self {
        Self {
            jwks,
            issuer,
            audiences,
        }
    }

    /// Verify a signed JWT and return its claims.
    ///
    /// `expected_nonce` is `Some` for an id token coming back from a login and
    /// `None` for a bearer token, which never had one. When it is `Some` the
    /// check is mandatory: a token with no `nonce` is refused, because the
    /// nonce is what ties the id token to the authorization request this
    /// server started.
    pub async fn verify(&self, token: &str, expected_nonce: Option<&str>) -> AuthResult<Claims> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|error| AuthError::Token(format!("unreadable header: {error}")))?;
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Token("no kid in the header".into()))?;

        let jwk = self.jwks.key(&kid).await?;
        let algorithms = algorithms_for(&jwk, header.alg)?;
        let key = DecodingKey::from_jwk(&jwk)
            .map_err(|error| AuthError::Token(format!("unusable key {kid}: {error}")))?;

        let mut validation = Validation::new(algorithms[0]);
        validation.algorithms = algorithms;
        // Every one of these is a claim whose absence has been a real
        // vulnerability in somebody's relying party. Required, not merely
        // validated-if-present.
        validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences.iter().collect::<Vec<_>>());
        validation.validate_exp = true;
        validation.validate_aud = true;
        validation.leeway = 60;

        let data = jsonwebtoken::decode::<Claims>(token, &key, &validation)
            .map_err(|error| AuthError::Token(error.to_string()))?;

        if let Some(expected) = expected_nonce {
            match data.claims.nonce.as_deref() {
                Some(actual) if constant_time_eq(actual.as_bytes(), expected.as_bytes()) => {}
                _ => {
                    return Err(AuthError::Token(
                        "the nonce does not match this sign-in".into(),
                    ));
                }
            }
        }

        Ok(data.claims)
    }
}

/// Which algorithms a key may have signed with.
///
/// Derived from the key, not from the token's header, with the header used
/// only to narrow. The header is attacker-controlled: a relying party that
/// takes `alg` from it and looks the key up by `kid` can be handed an RSA
/// public key and told to treat it as an HMAC secret, which is the classic
/// JWT confusion attack. Symmetric keys are refused outright — a provider's
/// public key set has no business containing one.
fn algorithms_for(jwk: &Jwk, header: Algorithm) -> AuthResult<Vec<Algorithm>> {
    let permitted: Vec<Algorithm> = match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => vec![
            Algorithm::RS256,
            Algorithm::RS384,
            Algorithm::RS512,
            Algorithm::PS256,
            Algorithm::PS384,
            Algorithm::PS512,
        ],
        AlgorithmParameters::EllipticCurve(_) => vec![Algorithm::ES256, Algorithm::ES384],
        AlgorithmParameters::OctetKeyPair(_) => vec![Algorithm::EdDSA],
        AlgorithmParameters::OctetKey(_) => {
            return Err(AuthError::Token(
                "the provider's key set contains a symmetric key, which cannot verify a signature \
                 from anybody but ourselves"
                    .into(),
            ));
        }
    };

    if !permitted.contains(&header) {
        return Err(AuthError::Token(format!(
            "the header claims {header:?}, which this key cannot produce"
        )));
    }
    Ok(vec![header])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims_with(extra: serde_json::Value) -> Claims {
        let mut map = serde_json::Map::new();
        map.insert("groups".to_owned(), extra);
        Claims {
            sub: "alice".to_owned(),
            exp: None,
            nonce: None,
            email: None,
            preferred_username: None,
            name: None,
            extra: map,
        }
    }

    #[test]
    fn groups_arrive_as_an_array_which_is_what_authentik_sends() {
        let claims = claims_with(serde_json::json!(["aiwatcher-admins", "everyone"]));
        assert_eq!(
            claims.groups("groups"),
            vec!["aiwatcher-admins".to_owned(), "everyone".to_owned()]
        );
    }

    #[test]
    fn groups_also_arrive_as_a_separated_string() {
        let claims = claims_with(serde_json::json!("aiwatcher-admins everyone"));
        assert_eq!(claims.groups("groups").len(), 2);
        let claims = claims_with(serde_json::json!("aiwatcher-admins|everyone"));
        assert_eq!(claims.groups("groups").len(), 2);
    }

    #[test]
    fn a_missing_groups_claim_is_no_groups_rather_than_an_error() {
        // An authenticated user in no group is a case the role mapping
        // already answers; a hard error here would make a provider that was
        // never asked for the scope look like a broken deployment.
        let claims = claims_with(serde_json::json!(null));
        assert!(claims.groups("groups").is_empty());
        assert!(claims.groups("roles").is_empty());
    }

    #[test]
    fn a_symmetric_key_in_a_key_set_is_refused() {
        // The JWT confusion attack: the provider's public RSA key handed back
        // as an HMAC secret, with a header that says HS256.
        let jwk: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "oct",
            "kid": "shared",
            "k": "c2VjcmV0"
        }))
        .expect("a valid oct jwk");
        assert!(algorithms_for(&jwk, Algorithm::HS256).is_err());
    }

    #[test]
    fn a_header_algorithm_the_key_cannot_produce_is_refused() {
        let jwk: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "RSA",
            "kid": "signing",
            "n": "sXchDaQebHnPiGvyDOAT4saGEUetSyo9MKLOoWFsueri23bOdgWp4Dy1WlUzewbgBHod5pcM9H95GQRV3JDXboIRROSBigeC5yjU1hGzHHyXss8UDprecbAYxknTcQkhslANGRUZmdTOQ5qTRsLAt6BTYuyvVRdhS8exSZEy_c4gs_7svlJJQ4H9_NxsiIoLwAEk7-Q3UXERGYw_75IDrGA84-lA_-Ct4eTlXHBIY2EaV7t7LjJaynVJCpkv4LKjTTAumiGUIuQhrNhZLuF_RJLqHpM2kgWFLU7-VTdL1VbC2tejvcI2BlMkEpk1BzBZI0KQB0GaDWFLN-aEAw3vRw",
            "e": "AQAB"
        }))
        .expect("a valid RSA jwk");
        assert!(algorithms_for(&jwk, Algorithm::HS256).is_err());
        assert_eq!(
            algorithms_for(&jwk, Algorithm::RS256).expect("permitted"),
            vec![Algorithm::RS256]
        );
    }
}
