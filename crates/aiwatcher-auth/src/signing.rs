//! Short-lived values this server hands to a browser and later has to believe.
//!
//! Two things travel that way: the session, and the login state that carries a
//! PKCE verifier across the redirect to the provider and back. Both are read
//! only by this server, so neither needs a store — they need a signature, an
//! expiry, and a constant-time comparison.
//!
//! No store is the point. A session table would be a second thing to size, to
//! evict and to replicate, for data that is already bounded by its own expiry;
//! and the projector's caps are what this process's memory budget is spent on.
//! What that costs is revocation: signing out clears the cookie in the browser
//! but cannot invalidate a copy somebody took. The session lifetime is
//! therefore the revocation window, which is why it defaults to eight hours
//! rather than to a week — see `AIWATCHER_AUTH_SESSION_TTL_SECONDS`.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::hmac;
use ring::rand::SecureRandom as _;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AuthError, AuthResult};

/// A payload plus the moment it stops being believed.
#[derive(Debug, Serialize, serde::Deserialize)]
struct Sealed<T> {
    /// Unix seconds.
    exp: i64,
    /// Issued at, kept so a log can say how old a rejected value was.
    iat: i64,
    value: T,
}

/// Seals and opens values with one HMAC key.
pub struct Signer {
    key: hmac::Key,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key. This struct is held by the `Authenticator`, which is
        // held by `AppState`, which something three layers up will eventually
        // derive `Debug` on.
        f.debug_struct("Signer").finish_non_exhaustive()
    }
}

impl Signer {
    /// A signer over an operator-supplied secret.
    ///
    /// The secret is stretched by HMAC's own key derivation rather than being
    /// required to be 32 bytes: an operator who sets a passphrase gets a
    /// working deployment, and one who sets 32 random bytes gets exactly the
    /// strength they asked for.
    #[must_use]
    pub fn new(secret: &[u8]) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret),
        }
    }

    /// A signer over a fresh random secret.
    ///
    /// What an instance with no `AIWATCHER_AUTH_SESSION_SECRET` uses. Safe —
    /// 32 bytes from the system CSPRNG — and inconvenient in exactly one way,
    /// which the caller warns about: a restart invalidates every session,
    /// because the key that signed them is gone.
    pub fn ephemeral() -> AuthResult<Self> {
        Ok(Self::new(&random_bytes::<32>()?))
    }

    /// `<payload>.<signature>`, both base64url without padding.
    pub fn seal<T: Serialize>(&self, value: &T, ttl: time::Duration) -> AuthResult<String> {
        let now = time::OffsetDateTime::now_utc();
        let sealed = Sealed {
            exp: (now + ttl).unix_timestamp(),
            iat: now.unix_timestamp(),
            value,
        };
        let json = serde_json::to_vec(&sealed).map_err(|error| {
            AuthError::Configuration(format!("cannot serialise a session: {error}"))
        })?;
        let payload = B64.encode(json);
        let signature = B64.encode(hmac::sign(&self.key, payload.as_bytes()).as_ref());
        Ok(format!("{payload}.{signature}"))
    }

    /// The value back, if the signature holds and the expiry has not passed.
    ///
    /// The signature is checked *before* the payload is parsed. Parsing first
    /// would run a deserialiser over bytes an attacker chose.
    pub fn open<T: DeserializeOwned>(&self, token: &str) -> AuthResult<T> {
        let (payload, signature) = token
            .split_once('.')
            .ok_or(AuthError::Session("malformed".into()))?;
        let signature = B64
            .decode(signature)
            .map_err(|_| AuthError::Session("malformed signature".into()))?;

        hmac::verify(&self.key, payload.as_bytes(), &signature)
            .map_err(|_| AuthError::Session("signature does not match".into()))?;

        let json = B64
            .decode(payload)
            .map_err(|_| AuthError::Session("malformed payload".into()))?;
        let sealed: Sealed<T> = serde_json::from_slice(&json)
            .map_err(|_| AuthError::Session("payload is not what this server writes".into()))?;

        if sealed.exp <= time::OffsetDateTime::now_utc().unix_timestamp() {
            return Err(AuthError::Session("expired".into()));
        }
        Ok(sealed.value)
    }
}

/// `N` bytes from the system CSPRNG.
pub fn random_bytes<const N: usize>() -> AuthResult<[u8; N]> {
    let mut bytes = [0_u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| {
            AuthError::Configuration("the system random number generator failed".into())
        })?;
    Ok(bytes)
}

/// A random, URL-safe token — the `state`, the nonce, and the PKCE verifier.
///
/// 32 bytes, which is 43 base64url characters: over the 128-bit floor RFC 7636
/// sets for a verifier and under its 128-character ceiling.
pub fn random_token() -> AuthResult<String> {
    Ok(B64.encode(random_bytes::<32>()?))
}

/// Compares two byte strings without revealing where they first differ.
///
/// Used on the `state` parameter and the id token's nonce, both of which a
/// caller supplies and both of which are compared against a secret this server
/// issued. The length is allowed to leak — every value compared here is a
/// fixed-length token — and `black_box` is what stops the optimiser turning
/// the fold back into an early return.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    std::hint::black_box(difference) == 0
}

/// The S256 challenge for a PKCE verifier.
#[must_use]
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    B64.encode(digest.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
    struct Payload {
        subject: String,
    }

    fn payload() -> Payload {
        Payload {
            subject: "alice".to_owned(),
        }
    }

    #[test]
    fn a_sealed_value_comes_back_unchanged() {
        let signer = Signer::new(b"a secret");
        let token = signer
            .seal(&payload(), time::Duration::hours(1))
            .expect("seal");
        assert_eq!(signer.open::<Payload>(&token).expect("open"), payload());
    }

    #[test]
    fn a_value_signed_with_another_key_is_refused() {
        let token = Signer::new(b"one key")
            .seal(&payload(), time::Duration::hours(1))
            .expect("seal");
        assert!(Signer::new(b"another key").open::<Payload>(&token).is_err());
    }

    #[test]
    fn an_edited_payload_is_refused() {
        // The whole reason the signature is checked before the payload is
        // parsed: without it, this string reaches serde.
        let signer = Signer::new(b"a secret");
        let token = signer
            .seal(&payload(), time::Duration::hours(1))
            .expect("seal");
        let (_, signature) = token.split_once('.').expect("two parts");
        let forged = B64.encode(br#"{"exp":99999999999,"iat":0,"value":{"subject":"root"}}"#);
        assert!(
            signer
                .open::<Payload>(&format!("{forged}.{signature}"))
                .is_err()
        );
    }

    #[test]
    fn an_expired_value_is_refused_even_though_it_verifies() {
        let signer = Signer::new(b"a secret");
        let token = signer
            .seal(&payload(), time::Duration::seconds(-1))
            .expect("seal");
        let error = signer.open::<Payload>(&token).expect_err("expired");
        assert!(error.to_string().contains("expired"), "{error}");
    }

    #[test]
    fn a_pkce_challenge_is_the_rfc_7636_example() {
        // RFC 7636 appendix B, so a mismatch here is a mismatch with every
        // provider rather than with our own idea of the derivation.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn comparing_in_constant_time_still_compares() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abz"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn two_random_tokens_differ() {
        assert_ne!(random_token().expect("rng"), random_token().expect("rng"));
    }
}
