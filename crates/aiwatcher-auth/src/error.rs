//! One error type for everything that can go wrong establishing an identity.
//!
//! The split that matters to a caller is not which step failed but who has to
//! do something about it: [`AuthError::is_caller_fault`] separates "sign in
//! again" from "the identity provider is down", which is the difference
//! between a 401 the browser can act on and a 503 it should retry.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    /// Something about the configuration cannot work, found at start-up.
    #[error("{0}")]
    Configuration(String),

    /// The provider's discovery document could not be read.
    #[error("the identity provider at {issuer} could not be reached: {source}")]
    Discovery {
        issuer: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The discovery document names a different issuer than the one
    /// configured. A hard stop rather than a warning: the issuer is what every
    /// later token is validated against, and taking the document's word for it
    /// would mean whoever answered the discovery request chose it.
    #[error("the discovery document at {expected} declares issuer {found}")]
    IssuerMismatch { expected: String, found: String },

    #[error("the provider's key set could not be read: {0}")]
    Jwks(String),

    /// A token that will not verify. Never says why in more detail than this:
    /// the difference between a bad signature and a wrong audience is useful
    /// to an attacker and not to anybody else, so the reason goes to the log
    /// and the caller gets "invalid".
    #[error("the token is not valid: {0}")]
    Token(String),

    #[error("the session is not valid: {0}")]
    Session(String),

    /// The `state` parameter did not match the one this server issued, or the
    /// login took longer than the state cookie's lifetime. Both mean the same
    /// thing to the user — start again — and both are what stops a login
    /// somebody else began from landing in this browser.
    #[error("this sign-in could not be matched to one that started here: {0}")]
    State(&'static str),

    #[error("the provider refused the authorization code (HTTP {status}): {message}")]
    Exchange { status: u16, message: String },

    /// The provider authenticated somebody this deployment grants nothing to.
    #[error(transparent)]
    NotEntitled(#[from] crate::identity::NotEntitled),

    /// No credential at all on a request that needs one.
    #[error("authentication is required")]
    Unauthenticated,

    /// `proxy` mode, and the authenticating proxy set no identity header. This
    /// is a 401 and not an anonymous request on purpose — in proxy mode the
    /// absence of the header means the request did not come through the proxy.
    #[error(
        "no {header} header; this instance only accepts requests through its authenticating proxy"
    )]
    ProxyIdentityMissing { header: String },

    /// A `next=` that would send the browser somewhere other than this app.
    #[error("{0:?} is not a path on this application")]
    UnsafeRedirect(String),
}

impl AuthError {
    /// Whether the caller can fix this by presenting a different credential.
    ///
    /// The complement is not "our fault" but "not something the caller can act
    /// on" — a provider that is down produces the same 503 whatever the caller
    /// sends.
    #[must_use]
    pub fn is_caller_fault(&self) -> bool {
        match self {
            Self::Token(_)
            | Self::Session(_)
            | Self::State(_)
            | Self::Unauthenticated
            | Self::ProxyIdentityMissing { .. }
            | Self::UnsafeRedirect(_)
            | Self::NotEntitled(_) => true,
            Self::Configuration(_)
            | Self::Discovery { .. }
            | Self::IssuerMismatch { .. }
            | Self::Jwks(_)
            | Self::Exchange { .. } => false,
        }
    }

    /// Whether the same request might succeed shortly. Only the provider being
    /// unreachable qualifies; a refused code will be refused identically for
    /// ever.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Discovery { .. } | Self::Jwks(_))
    }
}

pub type AuthResult<T> = std::result::Result<T, AuthError>;
