//! The one channel: an HTTP POST to one configured endpoint.
//!
//! `aiwatcher-runner`'s shape, for `aiwatcher-runner`'s reason. The endpoint
//! is a variable rather than a field in a request body, so nothing that can
//! publish an alert rule can aim this process's outbound POSTs at a metadata
//! service. It lives here with the other outbound clients because a client
//! that holds a socket and a credential belongs beside the loop that uses it.
//!
//! What the receiver gets, beside the JSON body:
//!
//! ```text
//! Aiwatcher-Delivery-Key: <the dedup key, which is also the body's>
//! Aiwatcher-Signature:    sha256=<hmac of the exact bytes>   (when signed)
//! Authorization:          Bearer <token>                     (when set)
//! ```
//!
//! The signature is over the bytes as sent, so a receiver verifies before
//! parsing. Both extra headers are there to make the receiver's job possible
//! rather than to make a promise: retries are real, and the key is how a
//! receiver turns at-least-once into what it needs.

use std::time::Duration;

use async_trait::async_trait;

use aiwatcher_alerts::{AlertChannel, AlertPayload, ChannelDescription};
use aiwatcher_core::ports::{PortError, PortResult};

const TARGET: &str = "alert-channel";

/// The variable an operator reads when something here is wrong.
pub const URL_VARIABLE: &str = "AIWATCHER_ALERT_WEBHOOK_URL";

#[derive(Clone)]
pub struct WebhookConfig {
    pub endpoint: String,
    /// Sent as `Authorization: Bearer …` when present.
    pub token: Option<String>,
    /// Signs the body with HMAC-SHA256 when present.
    pub secret: Option<String>,
    pub timeout: Duration,
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither the token nor the secret may reach a log through a derived
        // `Debug` three layers up — and neither may the endpoint, which is
        // reconnaissance for somebody already inside.
        f.debug_struct("WebhookConfig")
            .field("endpoint", &"<set>")
            .field("token", &self.token.is_some())
            .field("secret", &self.secret.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

pub struct WebhookChannel {
    http: reqwest::Client,
    config: WebhookConfig,
}

impl std::fmt::Debug for WebhookChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookChannel").finish_non_exhaustive()
    }
}

impl WebhookChannel {
    /// # Errors
    ///
    /// [`PortError::Other`] only if the HTTP client cannot be built — a TLS
    /// backend problem, not a reachability one. The endpoint is not contacted
    /// here: a channel that refused to start because the receiver was down
    /// would take aiwatcher down with whatever it was meant to warn about.
    pub fn new(config: WebhookConfig) -> PortResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| PortError::Other {
                target: TARGET,
                source: Box::new(error),
            })?;
        Ok(Self { http, config })
    }

    fn sign(&self, body: &[u8]) -> Option<String> {
        let secret = self.config.secret.as_ref()?;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        Some(format!(
            "sha256={}",
            hex::encode(ring::hmac::sign(&key, body).as_ref())
        ))
    }
}

#[async_trait]
impl AlertChannel for WebhookChannel {
    async fn deliver(&self, payload: &AlertPayload) -> PortResult<()> {
        let body = serde_json::to_vec(payload).map_err(|error| PortError::Rejected {
            target: TARGET,
            message: format!("this notification cannot be written as JSON: {error}"),
        })?;

        let mut request = self
            .http
            .post(&self.config.endpoint)
            .header("content-type", "application/json")
            .header("Aiwatcher-Delivery-Key", &payload.dedup_key);
        if let Some(signature) = self.sign(&body) {
            request = request.header("Aiwatcher-Signature", signature);
        }
        if let Some(token) = &self.config.token {
            request = request.bearer_auth(token);
        }

        let response = request.body(body).send().await.map_err(|error| {
            // Nothing answered, or the answer did not arrive. Ambiguous by
            // nature: the receiver may have taken it, which is why the payload
            // carries a key rather than this promising anything.
            PortError::Unavailable {
                target: TARGET,
                message: format!("the alert channel could not be reached: {error}"),
            }
        })?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // A receiver that understood and said no will say no again; one that is
        // overloaded or behind a proxy having a moment is worth coming back
        // for. The queue believes this line, so it is the one worth arguing
        // about: 408 and 429 are asks to come back, and every 5xx is too.
        let message = format!("the alert channel answered {status}");
        if status.is_server_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            Err(PortError::Unavailable {
                target: TARGET,
                message,
            })
        } else {
            Err(PortError::Rejected {
                target: TARGET,
                message,
            })
        }
    }

    fn describe(&self) -> ChannelDescription {
        ChannelDescription {
            kind: "webhook",
            variable: URL_VARIABLE,
            signed: self.config.secret.is_some(),
        }
    }
}
