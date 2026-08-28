//! The RustFS adapter — which is to say, an S3 adapter.
//!
//! RustFS is the target because it is the object store this deployment runs:
//! a single Rust binary, no JVM, S3 on the wire. But nothing here is specific
//! to it. The protocol is S3's, so the same adapter reaches MinIO, Ceph's
//! RADOS gateway, SeaweedFS or AWS itself by changing `AIWATCHER_PROMPT_S3_ENDPOINT`,
//! and that is the point of putting it behind [`ObjectStore`] rather than
//! writing a RustFS client.
//!
//! **Path-style addressing, always.** The virtual-hosted form
//! (`https://bucket.host/key`) needs DNS for every bucket, which a container
//! called `rustfs` on a compose network does not have. Every S3
//! implementation still accepts path style; AWS's deprecation applies to new
//! buckets on the public endpoint, which is not what this is for.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode, Url};
use time::OffsetDateTime;

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};

use crate::sigv4::{self, Credentials};

const TARGET: &str = "prompt-object-store";

/// How many keys one `ListObjectsV2` asks for. The service maximum; the
/// registry pages until the listing is exhausted either way, so a smaller
/// value would only mean more round trips.
const LIST_PAGE_SIZE: &str = "1000";

#[derive(Clone, Debug)]
pub struct S3Config {
    /// Base URL of the S3 endpoint, e.g. `http://rustfs:9000`.
    pub endpoint: String,
    pub bucket: String,
    pub credentials: Credentials,
    pub timeout: Duration,
    /// Create the bucket on connect when it is missing.
    ///
    /// On by default because the first thing a fresh RustFS has is no buckets,
    /// and failing to start over that turns a one-line compose service into a
    /// manual step. A deployment whose bucket is provisioned elsewhere — and
    /// whose credentials therefore may not be allowed to create one — turns it
    /// off.
    pub create_bucket: bool,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:9000".to_owned(),
            bucket: "aiwatcher-prompts".to_owned(),
            credentials: Credentials {
                access_key_id: String::new(),
                secret_access_key: String::new(),
                session_token: None,
                region: "us-east-1".to_owned(),
            },
            timeout: Duration::from_secs(10),
            create_bucket: true,
        }
    }
}

/// An S3 bucket, reached with hand-signed requests.
pub struct S3ObjectStore {
    http: reqwest::Client,
    endpoint: Url,
    /// `host` or `host:port`, exactly as it goes into the signed `Host` header.
    host: String,
    config: S3Config,
}

impl std::fmt::Debug for S3ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3ObjectStore")
            .field("endpoint", &self.endpoint.as_str())
            .field("bucket", &self.config.bucket)
            .finish_non_exhaustive()
    }
}

impl S3ObjectStore {
    /// Build a client, and create the bucket if configured to.
    ///
    /// # Errors
    ///
    /// [`PortError::Rejected`] for an endpoint that is not a URL or a bucket
    /// the credentials may not create, and [`PortError::Unavailable`] when the
    /// endpoint cannot be reached.
    pub async fn connect(config: S3Config) -> PortResult<Self> {
        let endpoint = Url::parse(config.endpoint.trim_end_matches('/')).map_err(|error| {
            PortError::Rejected {
                target: TARGET,
                message: format!("{:?} is not a URL: {error}", config.endpoint),
            }
        })?;
        let host = match (endpoint.host_str(), endpoint.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            (None, _) => {
                return Err(PortError::Rejected {
                    target: TARGET,
                    message: format!("{:?} has no host", config.endpoint),
                });
            }
        };
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| PortError::Other {
                target: TARGET,
                source: Box::new(error),
            })?;

        let store = Self {
            http,
            endpoint,
            host,
            config,
        };
        if store.config.create_bucket {
            store.ensure_bucket().await?;
        }
        Ok(store)
    }

    /// `HEAD` the bucket, and `PUT` it when it is not there.
    ///
    /// A 409 is success: it means somebody else created it between the two
    /// calls, which is the expected outcome when several replicas start
    /// together.
    async fn ensure_bucket(&self) -> PortResult<()> {
        let response = self.send(Method::HEAD, "", &[], Vec::new()).await?;
        if response.status.is_success() {
            return Ok(());
        }
        if response.status != StatusCode::NOT_FOUND {
            return Err(response.into_error("checking the bucket"));
        }
        let created = self.send(Method::PUT, "", &[], Vec::new()).await?;
        if created.status.is_success() || created.status == StatusCode::CONFLICT {
            tracing::info!(bucket = %self.config.bucket, "created the prompt bucket");
            return Ok(());
        }
        Err(created.into_error("creating the bucket"))
    }

    /// Sign and send one request. `key` is appended to the bucket path.
    async fn send(
        &self,
        method: Method,
        key: &str,
        query: &[(String, String)],
        body: Vec<u8>,
    ) -> PortResult<RawResponse> {
        let path = if key.is_empty() {
            format!("/{}", self.config.bucket)
        } else {
            format!("/{}/{key}", self.config.bucket)
        };
        let canonical_path = sigv4::encode_path(&path);

        let signed = sigv4::sign(
            &self.config.credentials,
            &sigv4::Request {
                method: method.as_str(),
                canonical_path: &canonical_path,
                query,
                host: &self.host,
                payload: &body,
                at: OffsetDateTime::now_utc(),
            },
        );

        // Built from the already-canonical path so the URL that goes on the
        // wire is byte-identical to the one that was signed. Letting `Url`
        // re-encode it is exactly how a key with a space starts failing.
        let mut url = self.endpoint.clone();
        url.set_path(&canonical_path);
        if !query.is_empty() {
            url.set_query(Some(
                &query
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{}={}",
                            sigv4::encode_component(key),
                            sigv4::encode_component(value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("&"),
            ));
        }

        let mut request = self
            .http
            .request(method, url)
            .header("host", &self.host)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.content_sha256)
            .header("authorization", &signed.authorization);
        if let Some(token) = &signed.session_token {
            request = request.header("x-amz-security-token", token);
        }

        let response = request.body(body).send().await.map_err(|error| {
            // Nothing was answered, so the request may or may not have landed.
            // Retryable: every write this adapter makes is idempotent.
            PortError::Unavailable {
                target: TARGET,
                message: error.to_string(),
            }
        })?;

        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| PortError::Unavailable {
                target: TARGET,
                message: format!("reading the response body: {error}"),
            })?;
        Ok(RawResponse {
            status,
            body: body.to_vec(),
        })
    }
}

struct RawResponse {
    status: StatusCode,
    body: Vec<u8>,
}

impl RawResponse {
    /// Classify a failure the way the pipeline's retry policy needs it.
    ///
    /// Getting this backwards is the expensive mistake: a retryable error
    /// treated as fatal discards a prompt, and a fatal one treated as
    /// retryable spins. 5xx and 429 are the store's problem and will pass;
    /// a 4xx is this process's problem and will not.
    fn into_error(self, doing: &str) -> PortError {
        let message = format!(
            "{doing}: {} — {}",
            self.status,
            String::from_utf8_lossy(&self.body).trim()
        );
        if self.status.is_server_error() || self.status == StatusCode::TOO_MANY_REQUESTS {
            PortError::Unavailable {
                target: TARGET,
                message,
            }
        } else {
            PortError::Rejected {
                target: TARGET,
                message,
            }
        }
    }
}

#[async_trait]
impl ObjectStore for S3ObjectStore {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        let response = self.send(Method::PUT, key, &[], body).await?;
        if response.status.is_success() {
            Ok(())
        } else {
            Err(response.into_error(&format!("putting {key}")))
        }
    }

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        let response = self.send(Method::GET, key, &[], Vec::new()).await?;
        if response.status.is_success() {
            return Ok(Some(response.body));
        }
        // A missing key is an answer, not a failure — the registry asks for
        // heads that may not exist yet on every publish.
        if response.status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Err(response.into_error(&format!("getting {key}")))
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        let mut entries = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            // Sorted by name, because the canonical query string is and the
            // signature is over that. Building it sorted here rather than
            // sorting inside the signer keeps the two in step visibly.
            let mut query = vec![
                ("list-type".to_owned(), "2".to_owned()),
                ("max-keys".to_owned(), LIST_PAGE_SIZE.to_owned()),
                ("prefix".to_owned(), prefix.to_owned()),
            ];
            if let Some(token) = &continuation {
                query.insert(0, ("continuation-token".to_owned(), token.clone()));
            }

            let response = self.send(Method::GET, "", &query, Vec::new()).await?;
            if !response.status.is_success() {
                return Err(response.into_error(&format!("listing {prefix}")));
            }
            let xml = String::from_utf8_lossy(&response.body).into_owned();
            entries.extend(parse_listing(&xml));

            match next_token(&xml) {
                Some(token) => continuation = Some(token),
                None => break,
            }
        }
        Ok(entries)
    }

    async fn delete(&self, key: &str) -> PortResult<()> {
        let response = self.send(Method::DELETE, key, &[], Vec::new()).await?;
        // S3 answers 204 for a key that was never there, and so does this.
        if response.status.is_success() || response.status == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(response.into_error(&format!("deleting {key}")))
        }
    }
}

/// Pull the `<Contents>` entries out of a `ListObjectsV2` response.
///
/// A hand-written scan rather than an XML parser, and the reason it is safe to
/// be one is that every key in this bucket is written by
/// `Registry::key_for` out of a validated [`aiwatcher_core::PromptName`] and a
/// hex id — `[a-z0-9._/-]`, which XML never escapes. A store shared with
/// something that writes arbitrary keys would need a real parser; this one
/// would simply skip those entries, which is why the scan is written to skip
/// rather than to guess.
fn parse_listing(xml: &str) -> Vec<ObjectEntry> {
    let mut entries = Vec::new();
    for block in xml.split("<Contents>").skip(1) {
        let Some(block) = block.split("</Contents>").next() else {
            continue;
        };
        let Some(key) = tag(block, "Key") else {
            continue;
        };
        if key.contains('&') {
            tracing::debug!(%key, "skipping an object key this listing cannot decode");
            continue;
        }
        entries.push(ObjectEntry {
            key,
            size: tag(block, "Size")
                .and_then(|size| size.parse().ok())
                .unwrap_or_default(),
            last_modified: tag(block, "LastModified").and_then(|stamp| {
                OffsetDateTime::parse(&stamp, &time::format_description::well_known::Rfc3339).ok()
            }),
        });
    }
    entries
}

/// The continuation token, when the listing was truncated.
fn next_token(xml: &str) -> Option<String> {
    if tag(xml, "IsTruncated").as_deref() != Some("true") {
        return None;
    }
    tag(xml, "NextContinuationToken")
}

fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const LISTING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>aiwatcher-prompts</Name>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>prompts/planner.floor-plan/head.json</Key>
    <LastModified>2026-08-28T10:00:00.000Z</LastModified>
    <Size>412</Size>
  </Contents>
  <Contents>
    <Key>prompts/planner.floor-plan/versions/abc.json</Key>
    <LastModified>2026-08-28T10:00:01.000Z</LastModified>
    <Size>1024</Size>
  </Contents>
</ListBucketResult>"#;

    #[test]
    fn a_listing_yields_one_entry_per_object() {
        let entries = parse_listing(LISTING);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "prompts/planner.floor-plan/head.json");
        assert_eq!(entries[0].size, 412);
        assert!(entries[0].last_modified.is_some());
        assert_eq!(entries[1].size, 1024);
    }

    #[test]
    fn an_untruncated_listing_asks_for_no_second_page() {
        assert_eq!(next_token(LISTING), None);
    }

    #[test]
    fn a_truncated_listing_carries_the_token_for_the_next_one() {
        // Without this the registry silently sees only the first 1000 objects,
        // which for a prompt with a long history means a version list that is
        // missing its oldest entries and says nothing about it.
        let truncated = LISTING
            .replace(
                "<IsTruncated>false</IsTruncated>",
                "<IsTruncated>true</IsTruncated>",
            )
            .replace(
                "</ListBucketResult>",
                "<NextContinuationToken>tok-2</NextContinuationToken></ListBucketResult>",
            );
        assert_eq!(next_token(&truncated).as_deref(), Some("tok-2"));
    }

    #[test]
    fn an_escaped_key_is_skipped_rather_than_returned_wrong() {
        let hostile = LISTING.replace(
            "prompts/planner.floor-plan/head.json",
            "prompts/a&amp;b/head.json",
        );
        let entries = parse_listing(&hostile);
        assert_eq!(entries.len(), 1, "the undecodable entry is dropped");
        assert_eq!(
            entries[0].key,
            "prompts/planner.floor-plan/versions/abc.json"
        );
    }

    #[test]
    fn a_5xx_is_retryable_and_a_4xx_is_not() {
        let unavailable = RawResponse {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: b"slow down".to_vec(),
        }
        .into_error("putting k");
        assert!(unavailable.is_retryable());

        let rejected = RawResponse {
            status: StatusCode::FORBIDDEN,
            body: b"SignatureDoesNotMatch".to_vec(),
        }
        .into_error("putting k");
        assert!(!rejected.is_retryable());
        assert!(rejected.to_string().contains("SignatureDoesNotMatch"));

        // 429 is the store asking for less traffic, not refusing the request.
        assert!(
            RawResponse {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: Vec::new(),
            }
            .into_error("putting k")
            .is_retryable()
        );
    }

    #[tokio::test]
    async fn an_endpoint_that_is_not_a_url_is_refused_before_any_request() {
        let error = S3ObjectStore::connect(S3Config {
            endpoint: "not a url".to_owned(),
            create_bucket: false,
            ..S3Config::default()
        })
        .await
        .expect_err("refused");
        assert!(!error.is_retryable(), "{error}");
    }
}
