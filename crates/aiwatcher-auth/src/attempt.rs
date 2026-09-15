//! The credential a launched pod holds: its own attempt, and nothing else
//! (ADR_0031).
//!
//! The pod launcher mints one per attempt and puts it in the pod's
//! `AIWATCHER_TOKEN`, so a template carries no aiwatcher secret at all. What it
//! opens is decided twice. Here, by what it names: one attempt and that
//! attempt's queue, as an [`Role::Editor`] that expires with the attempt's own
//! deadline. And in the API's authentication layer, by where it is presented:
//! that attempt's worker routes and ingest, and every other path refuses it
//! before a handler runs, because most read routes check no role.
//!
//! **Its key is derived, never the secret itself.** An operator who gives this
//! and the session the same secret still has two keys, so a session never opens
//! as a pod's credential and a pod's credential never opens as a session.
//!
//! The attempt rides as three plain fields rather than as the execution crate's
//! key, which would make this crate depend on the engine it authenticates for.

use std::sync::Arc;

use ring::hmac;
use serde::{Deserialize, Serialize};

use crate::error::{AuthError, AuthResult};
use crate::identity::{Credential, Identity, Role};
use crate::signing::{Signer, random_bytes};

/// What every attempt credential starts with.
///
/// Recognised by its prefix rather than by trying every bearer as an HMAC, and
/// kept away from the JWT verifier, which would otherwise be handed a value it
/// can only refuse.
pub const PREFIX: &str = "aw-attempt.";

/// What the key is derived under. The version is in it, so a change to what a
/// credential carries is a new key rather than old values read a new way.
const KEY_LABEL: &[u8] = b"aiwatcher pod attempt credential v1";

/// The one attempt a credential is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AttemptScope {
    /// The execution the attempt belongs to.
    pub execution: String,
    /// The step of that execution's pinned plan.
    pub step: String,
    /// Which attempt of that step. A retry is a new number, and so a new
    /// credential for a new pod.
    pub attempt: u32,
    /// The attempt's queue, which is the only queue this credential may claim
    /// on.
    pub queue: String,
}

impl AttemptScope {
    /// `<execution>/<step>/<attempt>`, the attempt's idempotency key.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.execution, self.step, self.attempt)
    }

    /// Whether this is the attempt a route's path names.
    #[must_use]
    pub fn names(&self, execution: &str, step: &str, attempt: u32) -> bool {
        self.execution == execution && self.step == step && self.attempt == attempt
    }

    /// The caller a credential for this attempt authenticates as.
    ///
    /// An editor, because the worker routes and ingest both ask for one; the
    /// layer in front of the API is what keeps it from being an editor
    /// anywhere else. The subject names the attempt, so an event published with
    /// a lifted credential says whose it was.
    #[must_use]
    pub fn identity(&self, expires_at: i64) -> Identity {
        Identity {
            issuer: None,
            subject: format!("attempt:{}", self.key()),
            username: None,
            name: None,
            email: None,
            groups: Vec::new(),
            roles: vec![Role::Editor],
            expires_at: Some(expires_at),
            queues: vec![self.queue.clone()],
            attempt: Some(self.clone()),
            credential: Credential::Attempt,
        }
    }
}

/// Mints and opens attempt credentials under one derived key.
///
/// One per process, shared by whatever mints and whatever checks: two would be
/// two keys, and every pod would be refused by the server that started it.
/// Cloning shares the key.
#[derive(Clone)]
pub struct AttemptCredentials {
    signer: Arc<Signer>,
    ephemeral: bool,
}

impl std::fmt::Debug for AttemptCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key. This is held by the authenticator and by the launcher,
        // and something above both derives `Debug`.
        f.debug_struct("AttemptCredentials")
            .field("ephemeral", &self.ephemeral)
            .finish_non_exhaustive()
    }
}

impl AttemptCredentials {
    /// Credentials under `AIWATCHER_POD_CREDENTIAL_SECRET`, or under a key
    /// generated now when there is none.
    ///
    /// A generated key is right for one process and wrong for two: a restart
    /// refuses every running pod's reports, and a split deployment's other role
    /// never had it. The caller warns about the first and refuses the second.
    ///
    /// # Errors
    ///
    /// [`AuthError::Configuration`] when the secret is empty, or when the
    /// system random number generator fails.
    pub fn new(secret: Option<&str>) -> AuthResult<Self> {
        let (secret, ephemeral) = match secret {
            Some(secret) if secret.trim().is_empty() => {
                return Err(AuthError::Configuration(
                    "AIWATCHER_POD_CREDENTIAL_SECRET is set and empty; unset it for a key \
                     generated at start-up, or generate one with `openssl rand -hex 32`"
                        .to_owned(),
                ));
            }
            Some(secret) => (secret.as_bytes().to_vec(), false),
            None => (random_bytes::<32>()?.to_vec(), true),
        };
        let derived = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &secret), KEY_LABEL);
        Ok(Self {
            signer: Arc::new(Signer::new(derived.as_ref())),
            ephemeral,
        })
    }

    /// Whether the key was generated at start-up rather than given.
    #[must_use]
    pub const fn is_ephemeral(&self) -> bool {
        self.ephemeral
    }

    /// A credential for one attempt, believed for `ttl` from now.
    ///
    /// # Errors
    ///
    /// [`AuthError::Configuration`] when the scope cannot be serialised.
    pub fn mint(&self, scope: &AttemptScope, ttl: time::Duration) -> AuthResult<String> {
        Ok(format!("{PREFIX}{}", self.signer.seal(scope, ttl)?))
    }

    /// The caller a credential authenticates as, if this process minted it and
    /// it has not expired.
    ///
    /// # Errors
    ///
    /// [`AuthError::Token`] for a value without the prefix, one sealed under
    /// another key, and one past its expiry.
    pub fn open(&self, token: &str) -> AuthResult<Identity> {
        let sealed = token.strip_prefix(PREFIX).ok_or_else(|| {
            AuthError::Token("an attempt credential starts with its prefix".to_owned())
        })?;
        let (scope, expires_at) = self
            .signer
            .open_with_expiry::<AttemptScope>(sealed)
            .map_err(|error| match error {
                AuthError::Session(why) => AuthError::Token(format!("attempt credential: {why}")),
                other => other,
            })?;
        Ok(scope.identity(expires_at))
    }
}

/// Whether a bearer is shaped like an attempt credential, and so is answered
/// by [`AttemptCredentials::open`] and by nothing else.
#[must_use]
pub fn is_attempt_credential(presented: &str) -> bool {
    presented.starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> AttemptScope {
        AttemptScope {
            execution: "run-1".to_owned(),
            step: "analyze".to_owned(),
            attempt: 1,
            queue: "houses".to_owned(),
        }
    }

    fn given(secret: &str) -> AttemptCredentials {
        AttemptCredentials::new(Some(secret)).expect("a secret")
    }

    #[test]
    fn a_credential_opens_back_to_the_attempt_it_was_minted_for() {
        let credentials = given("one secret for both roles");
        let token = credentials
            .mint(&scope(), time::Duration::minutes(10))
            .expect("mints");
        assert!(token.starts_with(PREFIX), "{token}");

        let identity = credentials.open(&token).expect("opens");
        assert_eq!(identity.attempt, Some(scope()));
        assert_eq!(identity.subject, "attempt:run-1/analyze/1");
        assert_eq!(identity.credential, Credential::Attempt);
        assert_eq!(identity.role(), Role::Editor, "never more than an editor");
        assert!(identity.expires_at.is_some());

        // The other role of a split deployment, given the same secret, is the
        // same key.
        assert_eq!(
            given("one secret for both roles")
                .open(&token)
                .expect("opens")
                .attempt,
            Some(scope())
        );
    }

    #[test]
    fn a_credential_past_its_expiry_is_refused() {
        let credentials = given("a secret");
        let token = credentials
            .mint(&scope(), time::Duration::seconds(-1))
            .expect("mints");
        let error = credentials.open(&token).expect_err("expired");
        assert!(error.is_caller_fault());
        assert!(error.to_string().contains("expired"), "{error}");
    }

    #[test]
    fn a_credential_sealed_under_another_secret_is_refused() {
        let token = given("one secret")
            .mint(&scope(), time::Duration::minutes(10))
            .expect("mints");
        assert!(given("another secret").open(&token).is_err());
        // And a generated key is a key nobody else has.
        assert!(
            AttemptCredentials::new(None)
                .expect("generates")
                .open(&token)
                .is_err()
        );
    }

    #[test]
    fn a_session_sealed_under_the_same_secret_does_not_open_as_an_attempt() {
        // The operator gave both the same secret. The session is sealed with
        // it; the attempt key is derived from it, so the two never agree.
        let secret = "the same secret twice";
        let session = Signer::new(secret.as_bytes())
            .seal(&scope(), time::Duration::hours(1))
            .expect("seals");
        assert!(
            given(secret).open(&format!("{PREFIX}{session}")).is_err(),
            "a session must not open as a pod's credential"
        );

        // And the other way round: a pod's credential is not a session.
        let minted = given(secret)
            .mint(&scope(), time::Duration::hours(1))
            .expect("mints");
        let sealed = minted.strip_prefix(PREFIX).expect("prefixed");
        assert!(
            Signer::new(secret.as_bytes())
                .open::<Identity>(sealed)
                .is_err()
        );
    }

    #[test]
    fn a_value_without_the_prefix_is_never_opened() {
        let credentials = given("a secret");
        let token = credentials
            .mint(&scope(), time::Duration::minutes(10))
            .expect("mints");
        let bare = token.strip_prefix(PREFIX).expect("prefixed");
        assert!(!is_attempt_credential(bare));
        assert!(credentials.open(bare).is_err());
        assert!(is_attempt_credential(&token));
    }

    #[test]
    fn a_credential_may_claim_its_own_queue_and_no_other() {
        let identity = scope().identity(0);
        assert!(identity.may_claim("houses"));
        assert!(!identity.may_claim("plans"));
        assert_eq!(
            identity.claimable_queues(),
            Some(&["houses".to_owned()][..])
        );
    }

    #[test]
    fn an_empty_secret_is_refused_rather_than_used() {
        assert!(AttemptCredentials::new(Some("  ")).is_err());
        assert!(!given("a secret").is_ephemeral());
        assert!(
            AttemptCredentials::new(None)
                .expect("generates")
                .is_ephemeral()
        );
    }

    #[test]
    fn the_credentials_never_print_their_key() {
        let rendered = format!("{:?}", given("do not print me"));
        assert!(!rendered.contains("do not print me"), "{rendered}");
    }
}
