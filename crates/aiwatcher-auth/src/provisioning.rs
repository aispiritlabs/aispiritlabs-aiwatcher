//! Where somebody who has no account goes to make one.
//!
//! An invitation offers a project to a **secret**, because nobody knows a
//! stranger's `(provider, subject)` until their provider has minted one
//! (`crates/aiwatcher-iam/README.md`). That leaves one step nothing here could
//! take: the stranger has to *have* an account with the provider first, and
//! until now somebody made it by hand.
//!
//! So aiwatcher asks the provider to open its own enrollment to one person, and
//! sends them there. Three rules carry it, and each is a thing this module does
//! **not** do.
//!
//! * **It creates no user and no membership.** What it creates is an
//!   *invitation* at the provider — one token, one use, an expiry of its own —
//!   and the person fills in their own details on the provider's page. The
//!   service account behind [`AIWATCHER_AUTH_PROVISION_TOKEN`] needs exactly
//!   that one permission, and a wider one would make aiwatcher a tool for
//!   making accounts in somebody's identity provider.
//! * **It never sees a password.** The enrollment flow is the provider's page
//!   on the provider's origin. There is nothing here to lose.
//! * **It grants nothing.** The grant is still the redemption of aiwatcher's
//!   own offer, against the verified pair the session proves, exactly as it was
//!   — so no rule about identity changes and this is one outbound adapter.
//!
//! An answer from the provider is **data**, like every other answer from
//! outside the building: bounded, timed out, and never a URL this process then
//! follows or hands to a browser as somewhere to go. What the panel is given is
//! built here from the configured base and the id that came back.
//!
//! [`AIWATCHER_AUTH_PROVISION_TOKEN`]: ProvisioningConfig::token

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AuthError, AuthResult};

/// What a deployment configures to be able to offer an account at all.
#[derive(Clone, Debug)]
pub struct ProvisioningConfig {
    /// The provider's own base URL — authentik's, not the issuer path under it.
    pub url: String,
    /// A service account token that may create invitations and nothing else.
    pub token: String,
    /// The enrollment flow's slug, as the provider spells it in its own URLs.
    pub flow: String,
    /// How long the provider's invitation stays open. Shorter than aiwatcher's
    /// own offer on purpose: this one is spent in the next few minutes, while
    /// the offer waits for somebody to get round to it.
    pub ttl: Duration,
    pub http_timeout: Duration,
}

impl ProvisioningConfig {
    /// # Errors
    ///
    /// [`AuthError::Configuration`] naming the field that is empty or not a URL.
    pub fn validate(&self) -> AuthResult<()> {
        if self.token.trim().is_empty() {
            return Err(AuthError::Configuration(
                "AIWATCHER_AUTH_PROVISION_TOKEN is empty".into(),
            ));
        }
        if self.flow.trim().is_empty() {
            return Err(AuthError::Configuration(
                "AIWATCHER_AUTH_PROVISION_FLOW is empty".into(),
            ));
        }
        url::Url::parse(self.url.trim_end_matches('/')).map_err(|error| {
            AuthError::Configuration(format!(
                "AIWATCHER_AUTH_PROVISION_URL is not a URL: {error}"
            ))
        })?;
        Ok(())
    }
}

/// Where to send somebody to make an account, and when that stops working.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Enrollment {
    /// The provider's enrollment page, carrying the one-use token.
    pub url: String,
    /// When the provider's invitation lapses, as a Unix timestamp.
    pub expires_at: i64,
}

/// The one thing this adapter asks the provider for.
#[derive(Debug)]
pub struct AccountProvisioning {
    config: ProvisioningConfig,
    http: reqwest::Client,
}

impl AccountProvisioning {
    /// # Errors
    ///
    /// [`AuthError::Configuration`] for a configuration that cannot be used.
    pub fn new(config: ProvisioningConfig) -> AuthResult<Self> {
        config.validate()?;
        let http = reqwest::Client::builder()
            .timeout(config.http_timeout)
            .build()
            .map_err(|error| {
                AuthError::Configuration(format!("cannot build an HTTP client: {error}"))
            })?;
        Ok(Self { config, http })
    }

    /// The provider's base, without a trailing slash.
    #[must_use]
    pub fn provider_url(&self) -> &str {
        self.config.url.trim_end_matches('/')
    }

    /// Open the provider's enrollment to one person.
    ///
    /// `email` is a prefill and nothing else: whoever holds the token decides
    /// who redeems, here as in the offer it came from, and a provider that
    /// ignores the hint changes nothing about who ends up with the grant.
    ///
    /// # Errors
    ///
    /// [`AuthError::Provisioning`] when the provider refused or could not be
    /// reached. Its text is the provider's, truncated, and goes to the log
    /// rather than to a browser.
    pub async fn open_enrollment(&self, email: Option<&str>) -> AuthResult<Enrollment> {
        let expires_at = time::OffsetDateTime::now_utc() + self.config.ttl;
        let flow = self.flow_id().await?;
        let mut body = serde_json::json!({
            "name": format!("aiwatcher-{}", uuid_like(expires_at)),
            "expires": expires_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|error| AuthError::Provisioning(error.to_string()))?,
            "single_use": true,
            "flow": flow,
        });
        if let Some(email) = email.map(str::trim).filter(|value| !value.is_empty()) {
            body["fixed_data"] = serde_json::json!({ "email": email });
        }
        let created: Created = self
            .post("/api/v3/stages/invitation/invitations/", &body)
            .await?;
        Ok(Enrollment {
            url: format!(
                "{}/if/flow/{}/?itoken={}",
                self.provider_url(),
                urlencode(&self.config.flow),
                urlencode(&created.pk),
            ),
            expires_at: expires_at.unix_timestamp(),
        })
    }

    /// The flow's id, which the provider's API wants where its URLs take a slug.
    async fn flow_id(&self) -> AuthResult<String> {
        let found: Flows = self
            .get(&format!(
                "/api/v3/flows/instances/?slug={}",
                urlencode(&self.config.flow)
            ))
            .await?;
        found
            .results
            .into_iter()
            .next()
            .map(|flow| flow.pk)
            .ok_or_else(|| {
                AuthError::Provisioning(format!(
                    "the identity provider has no flow named {:?} \
                     (AIWATCHER_AUTH_PROVISION_FLOW)",
                    self.config.flow
                ))
            })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> AuthResult<T> {
        self.read(self.http.get(format!("{}{path}", self.provider_url())))
            .await
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> AuthResult<T> {
        self.read(
            self.http
                .post(format!("{}{path}", self.provider_url()))
                .json(body),
        )
        .await
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> AuthResult<T> {
        let response = request
            .bearer_auth(&self.config.token)
            .send()
            .await
            .map_err(|error| AuthError::Provisioning(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| AuthError::Provisioning(error.to_string()))?;
        if !status.is_success() {
            return Err(AuthError::Provisioning(format!(
                "the identity provider answered {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AuthError::Provisioning(format!(
                "the identity provider's answer does not read: {error}"
            ))
        })
    }
}

#[derive(Deserialize)]
struct Created {
    pk: String,
}

#[derive(Deserialize)]
struct Flows {
    results: Vec<Flow>,
}

#[derive(Deserialize)]
struct Flow {
    pk: String,
}

/// A name the provider will accept, unique enough not to collide with the one
/// somebody else asked for in the same second.
fn uuid_like(at: time::OffsetDateTime) -> String {
    format!(
        "{}-{}",
        at.unix_timestamp(),
        at.nanosecond()
            .to_string()
            .chars()
            .take(6)
            .collect::<String>()
    )
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
