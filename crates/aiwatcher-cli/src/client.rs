//! The one place in this crate that knows a network exists.
//!
//! Every read and every write a command performs is an HTTP call against a
//! route the API already serves, and that is deliberate: the alternative — a
//! CLI that opened the store directly — would be a second reader of the read
//! model, free to disagree with the panel about what a run is, and unable to
//! answer at all for a remote instance. The escape hatch (`aiwatcher api …`)
//! and the ergonomic verbs go through this same type, so the credential, the
//! timeout and the error shape are decided once.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::config::Resolved;
use crate::{CliError, Format};

/// A client for one instance.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    /// Which profile this is, so an error can say *where* it was talking to
    /// rather than only what came back.
    pub profile: String,
}

impl Client {
    /// Build a client for a resolved profile.
    ///
    /// # Errors
    ///
    /// [`CliError::Io`] when the profile's token file cannot be read, and
    /// [`CliError::Other`] when the HTTP client itself will not build.
    pub fn new(resolved: &Resolved, timeout: Duration) -> Result<Self, CliError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("aiwatcher-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| CliError::Other(error.into()))?;
        Ok(Self {
            http,
            base: resolved.profile.url.trim_end_matches('/').to_owned(),
            token: resolved.profile.secret()?,
            profile: resolved.name.clone(),
        })
    }

    /// Where this client points.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Whether it has a credential to present.
    #[must_use]
    pub const fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// GET a path with query parameters, as JSON.
    ///
    /// # Errors
    ///
    /// [`CliError::Unreachable`] when the instance did not answer, and
    /// [`CliError::Api`] when it answered with a refusal.
    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, CliError> {
        let request = self.http.get(self.url(path)).query(query);
        self.send(request, path).await
    }

    /// POST a JSON body.
    ///
    /// # Errors
    ///
    /// As [`Self::get`].
    pub async fn post<B: Serialize>(&self, path: &str, body: &B) -> Result<Value, CliError> {
        let request = self.http.post(self.url(path)).json(body);
        self.send(request, path).await
    }

    /// PUT a JSON body.
    ///
    /// # Errors
    ///
    /// As [`Self::get`].
    pub async fn put<B: Serialize>(&self, path: &str, body: &B) -> Result<Value, CliError> {
        let request = self.http.put(self.url(path)).json(body);
        self.send(request, path).await
    }

    /// DELETE a path.
    ///
    /// # Errors
    ///
    /// As [`Self::get`].
    pub async fn delete(&self, path: &str) -> Result<Value, CliError> {
        let request = self.http.delete(self.url(path));
        self.send(request, path).await
    }

    /// One request of any method, for the passthrough.
    ///
    /// # Errors
    ///
    /// As [`Self::get`].
    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, CliError> {
        let mut request = self.http.request(method, self.url(path)).query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        self.send(request, path).await
    }

    /// Whether the instance is up, without asking it to do any work.
    ///
    /// `/readyz` rather than a route with data behind it, because "is it there"
    /// and "does it have anything to show me" are different questions and only
    /// the first one belongs in a status line.
    pub async fn ready(&self) -> bool {
        let Ok(response) = self.http.get(format!("{}/readyz", self.base)).send().await else {
            return false;
        };
        response.status().is_success()
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_owned();
        }
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    async fn send(&self, request: reqwest::RequestBuilder, path: &str) -> Result<Value, CliError> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|source| CliError::Unreachable {
                url: self.base.clone(),
                source,
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(CliError::Api {
                status: status.as_u16(),
                path: path.to_owned(),
                body,
            });
        }
        if body.trim().is_empty() {
            // A 204 is a success that carries nothing. `null` rather than an
            // error, so `confirmDone`'s question — did it work — is answered by
            // the status, exactly as it is in the panel's own client.
            return Ok(Value::Null);
        }
        serde_json::from_str(&body).map_err(|error| {
            CliError::Other(anyhow::anyhow!(
                "{path} answered something that is not JSON: {error}"
            ))
        })
    }
}

/// Print a JSON answer in the requested shape.
///
/// # Errors
///
/// [`CliError::Other`] when the value cannot be serialised, which for a value
/// that arrived as JSON means the writer failed.
pub fn emit(value: &Value, format: Format) -> Result<(), CliError> {
    match format {
        Format::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value)
                    .map_err(|error| CliError::Other(error.into()))?
            );
        }
        Format::Table => crate::output::render(value),
    }
    Ok(())
}
