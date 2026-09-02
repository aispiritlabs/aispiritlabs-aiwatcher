//! The one place in this system that downloads bytes an outside party chose.
//!
//! Everything else in this crate answers from the object store. A hub search
//! reaches out and gets back *JSON this code parses*; this reaches out and
//! gets back a file that is about to be stored, hashed, and trained on. That
//! difference is why the plan sequenced it as its own piece and called it
//! security work rather than part of the importer: an import job that is
//! wrong re-runs, and a fetcher that is wrong is a request-forgery primitive
//! with a cluster behind it.
//!
//! Seven gates, in this order, and each one is a mistake somebody else has
//! already made:
//!
//! 1. **`https` only, parsed rather than matched.** `url::Url` decides what
//!    the host is. `https://huggingface.co@evil.test/x.png` has the
//!    allowlisted name in it and is not that host, which is the same class of
//!    error as the substring match that once invented a licence claim.
//! 2. **An allowlist of hosts**, exact or dot-suffixed. A host nobody listed
//!    is refused with its name, never fetched "just to see".
//! 3. **Every resolved address is checked** before the connection is made:
//!    loopback, private, link-local, unique-local, shared-CGNAT, multicast and
//!    reserved ranges are refused. `169.254.169.254` is the cloud metadata
//!    service and a DNS name is all it takes to point an allowlisted-looking
//!    fetch at it. See [`is_public`], which is where that list lives.
//! 4. **No redirects.** An allowlisted host answering `302` to
//!    `http://169.254.169.254/` would walk straight through gates 1–3, because
//!    they ran against the address the *caller* named. A redirect is reported
//!    with its target rather than followed.
//! 5. **A byte ceiling enforced while streaming.** `Content-Length` is a claim
//!    by the server; the cap is applied to the bytes as they arrive, so a
//!    response that lies (or omits it, or is chunked) stops at the ceiling
//!    instead of at the memory limit.
//! 6. **It has to be a picture, by its own first bytes.** [`pixels::describe`]
//!    reads the header; HTML, a PDF, a zip or a truncated download is refused.
//!    This is also the decompression-bomb gate: the header states the pixel
//!    dimensions, so a 40 kB PNG declaring 60 000 × 60 000 is refused *before*
//!    anything decodes it — which is the only place refusing it is cheap.
//! 7. **The content address is verified, never accepted.** A caller that says
//!    what it expects gets that checked against the bytes; a caller that does
//!    not gets the digest computed here. Same rule as `put_blob`: a content
//!    address supplied by the caller would let two different pictures occupy
//!    one key, which is a training set whose labels belong to another image.
//!
//! What this does **not** close is the window between the address check and
//! the connection: a resolver that answers differently the second time (DNS
//! rebinding) is not defeated by checking the first answer. Closing it needs
//! a connection-time hook — reqwest can be given a resolver, but the check has
//! to happen where the socket is opened, not where the name is looked up. The
//! gate that actually holds against it is the allowlist: rebinding requires a
//! host somebody listed, and the hosts listed here are hubs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use async_trait::async_trait;

use crate::digest;
use crate::integrations::pixels;

/// What one download may weigh.
///
/// Sixteen megabytes is the registry's own blob limit, and a fetcher that
/// allowed more would be a fetcher whose successes fail one step later.
pub const MAX_FETCH_BYTES: usize = 16 * 1024 * 1024;

/// The most pixels a picture may claim.
///
/// Eighty megapixels is four times a full-frame camera and well above any
/// scanned plan. The number that matters is not the file size: a PNG of one
/// flat colour compresses to nothing and still allocates `width × height × 4`
/// in whatever decodes it next.
pub const MAX_FETCH_PIXELS: u64 = 80_000_000;

/// How long one download may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// What a fetch is allowed to do.
///
/// Deliberately a value rather than a set of constants: the importer and the
/// panel's preview want the same rules, and a deployment that mirrors a corpus
/// onto its own host needs to say so once. There is no "allow everything" —
/// [`hosts`](Self::hosts) empty refuses every address, because a fetcher whose
/// default is open is a fetcher that is open in the deployment nobody
/// configured.
#[derive(Clone, Debug)]
pub struct FetchPolicy {
    /// Exact hosts, or `.suffix` entries matching a host and its subdomains.
    pub hosts: Vec<String>,
    pub max_bytes: usize,
    pub max_pixels: u64,
    pub timeout: Duration,
    /// Off, and only a test turns it on. A loopback address is exactly what a
    /// server-side request forgery aims at, so the switch that allows it is
    /// named for what it allows rather than for the test that needs it.
    pub allow_private_networks: bool,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            max_bytes: MAX_FETCH_BYTES,
            max_pixels: MAX_FETCH_PIXELS,
            timeout: FETCH_TIMEOUT,
            allow_private_networks: false,
        }
    }
}

impl FetchPolicy {
    /// The hosts the hub integration hands out addresses for.
    #[must_use]
    pub fn for_hubs() -> Self {
        Self {
            hosts: vec![
                "huggingface.co".to_owned(),
                ".huggingface.co".to_owned(),
                "cdn-lfs.huggingface.co".to_owned(),
                ".hf.co".to_owned(),
                "hf.co".to_owned(),
            ],
            ..Self::default()
        }
    }

    #[must_use]
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            match allowed.strip_prefix('.') {
                Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
                None => host == allowed,
            }
        })
    }
}

/// A picture that passed every gate.
#[derive(Clone, Debug)]
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    pub content_type: String,
    /// `sha256` of the bytes, computed here. This is the `image_id`.
    pub digest: String,
    pub width: u32,
    pub height: u32,
}

/// Where an importer gets bytes from.
///
/// A port rather than a concrete client, for the reason every port in this
/// workspace exists: the import job runs inside `aiwatcher-annotations` and
/// the thing that knows how to reach a hub is configured in the server. It
/// also keeps the job's tests free of a network — a fake that returns three
/// pictures and one refusal is four lines.
#[async_trait]
pub trait ImageSource: Send + Sync + std::fmt::Debug {
    /// The bytes behind one address.
    ///
    /// `expected` is a content address the caller already believes; when it is
    /// supplied and the bytes disagree, that is an error rather than a new
    /// version of the picture.
    async fn fetch(&self, uri: &str, expected: Option<&str>) -> Result<FetchedImage, String>;
}

/// The bounded downloader.
#[derive(Clone, Debug)]
pub struct Fetcher {
    http: reqwest::Client,
    policy: FetchPolicy,
}

impl Fetcher {
    /// # Errors
    /// When the HTTP client cannot be built, which is a TLS backend problem
    /// rather than a configuration one.
    pub fn new(policy: FetchPolicy) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            // Gate 4. The default is to follow up to ten, which would run
            // every earlier gate against an address that is then not the one
            // connected to.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(policy.timeout)
            .build()?;
        Ok(Self { http, policy })
    }

    #[must_use]
    pub const fn policy(&self) -> &FetchPolicy {
        &self.policy
    }

    /// Download one picture, with an optional bearer token for a gated
    /// repository.
    ///
    /// # Errors
    /// One sentence per gate, naming what was refused. A caller turns these
    /// into a rejected row rather than a failed batch: one unreachable image
    /// in six hundred is a row to report, not an import to abandon.
    pub async fn fetch_image(
        &self,
        uri: &str,
        expected: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<FetchedImage, String> {
        let url = self.check_address(uri).await?;

        let mut request = self.http.get(url).timeout(self.policy.timeout);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("{uri} did not download: {error}"))?;

        let status = response.status();
        if status.is_redirection() {
            let target = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("somewhere it did not name");
            return Err(format!(
                "{uri} answered {status} redirecting to {target}; a redirect is not followed \
                 because every address check would have run against the address you named rather \
                 than the one that would be connected to"
            ));
        }
        if !status.is_success() {
            // An expired signature is the failure worth naming: hub asset
            // URLs last hours, so a batch previewed yesterday and imported
            // today fails here rather than anywhere that would explain itself.
            return Err(format!(
                "{uri} answered {status}; a hub asset URL expires within hours of being listed"
            ));
        }

        let declared = response.content_length();
        if declared.is_some_and(|length| length > self.policy.max_bytes as u64) {
            return Err(format!(
                "{uri} declares {} bytes; the limit is {}",
                declared.unwrap_or_default(),
                self.policy.max_bytes
            ));
        }

        let header_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();

        let bytes = self.read_bounded(response, uri).await?;
        self.accept(bytes, uri, expected, header_type)
    }

    /// Gates 6 and 7, over bytes that are already here.
    ///
    /// Separate from the download because a hub row can carry the picture
    /// *inline*, as base64 in a `binary` column, and bytes that never crossed
    /// the network still have to be a picture of a sane size and still have to
    /// hash to what the caller claimed.
    ///
    /// # Errors
    /// When the bytes are not a readable picture, claim more pixels than the
    /// policy allows, or do not hash to `expected`.
    pub fn accept(
        &self,
        bytes: Vec<u8>,
        what: &str,
        expected: Option<&str>,
        header_type: String,
    ) -> Result<FetchedImage, String> {
        let found = pixels::describe(&bytes).ok_or_else(|| {
            format!(
                "{what} is not a picture this can read; it starts {}",
                preview(&bytes)
            )
        })?;
        let claimed = u64::from(found.width) * u64::from(found.height);
        if found.width == 0 || found.height == 0 {
            return Err(format!("{what} declares a zero dimension"));
        }
        if claimed > self.policy.max_pixels {
            return Err(format!(
                "{what} declares {}×{} = {claimed} pixels; the limit is {}. A small file can \
                 still allocate four bytes per pixel in whatever decodes it next",
                found.width, found.height, self.policy.max_pixels
            ));
        }

        // The bytes outrank the header. Hugging Face serves its cached assets
        // as `binary/octet-stream`, and a browser handed that for something it
        // is about to draw is being told less than the first four bytes
        // already say.
        let content_type = if header_type.starts_with("image/") {
            header_type
        } else {
            found.content_type.to_owned()
        };

        let digest = digest(&bytes);
        if let Some(expected) = expected
            && expected != digest
        {
            return Err(format!(
                "{what} hashes to {digest} and the row said {expected}; the bytes at that address \
                 are not the bytes that row is about"
            ));
        }

        Ok(FetchedImage {
            bytes,
            content_type,
            digest,
            width: found.width,
            height: found.height,
        })
    }

    /// Gates 1, 2 and 3.
    async fn check_address(&self, uri: &str) -> Result<reqwest::Url, String> {
        let url =
            reqwest::Url::parse(uri).map_err(|error| format!("{uri} is not a URL: {error}"))?;
        if url.scheme() != "https" {
            return Err(format!(
                "{uri} is {}; only https is fetched, because a plaintext download of something \
                 about to be trained on is a picture anyone on the path may choose",
                url.scheme()
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!(
                "{uri} carries credentials in its authority; that is how an allowlist is walked \
                 past, because the host is what follows the '@'"
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| format!("{uri} names no host"))?
            .to_owned();
        if !self.policy.allows_host(&host) {
            return Err(format!(
                "{host} is not a host this instance may fetch from. Only a hub's own asset host \
                 is downloaded: \"fetch this address for me\" from inside a cluster is a request \
                 to reach that cluster's network on the caller's behalf"
            ));
        }
        if self.policy.allow_private_networks {
            return Ok(url);
        }

        let port = url.port_or_known_default().unwrap_or(443);
        let addresses: Vec<IpAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|error| format!("{host} did not resolve: {error}"))?
            .map(|socket| socket.ip())
            .collect();
        if addresses.is_empty() {
            return Err(format!("{host} resolved to no address"));
        }
        for address in &addresses {
            if !is_public(*address) {
                return Err(format!(
                    "{host} resolves to {address}, which is not a public address. A name that \
                     points inside the network is how a fetch of somebody else's picture becomes \
                     a read of this cluster's own services"
                ));
            }
        }
        Ok(url)
    }

    /// Gate 5: the cap applied to the bytes rather than to what was promised.
    async fn read_bounded(
        &self,
        mut response: reqwest::Response,
        uri: &str,
    ) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("{uri} did not download: {error}"))?
        {
            if body.len() + chunk.len() > self.policy.max_bytes {
                return Err(format!(
                    "{uri} is over {} bytes; the download stopped there rather than finding out \
                     how much larger it is",
                    self.policy.max_bytes
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// Whether an address is one the internet routes to somebody else.
///
/// The list is the point of the function, so it is spelled out rather than
/// composed from standard-library predicates — several of the ones that would
/// cover it (`Ipv4Addr::is_shared`, `Ipv6Addr::is_unique_local`) are still
/// unstable, and a check that is *nearly* complete is one that reads as
/// complete.
#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            // `::ffff:169.254.169.254` is the metadata service wearing a hat.
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    !(address.is_loopback()          // 127/8
        || address.is_private()      // 10/8, 172.16/12, 192.168/16
        || address.is_link_local()   // 169.254/16 — the cloud metadata service
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || address.is_unspecified()
        || a == 0
        || (a == 100 && (64..128).contains(&b))  // 100.64/10, carrier NAT
        || (a == 192 && b == 0)                  // 192.0.0/24, IETF protocol assignments
        || (a == 198 && (18..20).contains(&b))   // 198.18/15, benchmarking
        || a >= 240) // 240/4, reserved
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let first = address.segments()[0];
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (first & 0xfe00) == 0xfc00   // fc00::/7, unique local
        || (first & 0xffc0) == 0xfe80) // fe80::/10, link local
}

/// The first bytes of something that was not a picture, for the message.
///
/// Bounded and escaped: an error carrying an unbounded slice of a downloaded
/// file is a log line an outside party writes.
fn preview(bytes: &[u8]) -> String {
    let head: String = bytes
        .iter()
        .take(16)
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect();
    format!("'{head}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetcher() -> Fetcher {
        Fetcher::new(FetchPolicy::for_hubs()).expect("a client builds")
    }

    #[tokio::test]
    async fn only_https_is_fetched() {
        let error = fetcher()
            .check_address("http://huggingface.co/x.png")
            .await
            .expect_err("plaintext is refused");
        assert!(error.contains("only https"), "{error}");
    }

    #[tokio::test]
    async fn an_allowlisted_name_in_the_userinfo_is_not_the_host() {
        let error = fetcher()
            .check_address("https://huggingface.co@evil.test/x.png")
            .await
            .expect_err("credentials in the authority are refused");
        assert!(error.contains("credentials in its authority"), "{error}");
    }

    #[tokio::test]
    async fn a_host_nobody_listed_is_refused_by_name() {
        let error = fetcher()
            .check_address("https://evil.test/x.png")
            .await
            .expect_err("an unlisted host is refused");
        assert!(error.contains("evil.test is not a host"), "{error}");
    }

    #[test]
    fn a_subdomain_of_an_allowlisted_host_is_allowed_and_a_lookalike_is_not() {
        let policy = FetchPolicy::for_hubs();
        assert!(policy.allows_host("cdn-lfs-us-1.huggingface.co"));
        assert!(policy.allows_host("HuggingFace.co"));
        assert!(!policy.allows_host("huggingface.co.evil.test"));
        assert!(!policy.allows_host("nothuggingface.co"));
    }

    #[test]
    fn the_cloud_metadata_service_is_not_a_public_address() {
        assert!(!is_public("169.254.169.254".parse().expect("an address")));
        assert!(!is_public("127.0.0.1".parse().expect("an address")));
        assert!(!is_public("10.4.0.7".parse().expect("an address")));
        assert!(!is_public("172.20.1.1".parse().expect("an address")));
        assert!(!is_public("192.168.1.1".parse().expect("an address")));
        assert!(!is_public("100.64.0.1".parse().expect("an address")));
        assert!(!is_public("::1".parse().expect("an address")));
        assert!(!is_public("fd00::1".parse().expect("an address")));
        assert!(!is_public("fe80::1".parse().expect("an address")));
        assert!(!is_public(
            "::ffff:169.254.169.254".parse().expect("an address")
        ));
        assert!(is_public("93.184.216.34".parse().expect("an address")));
        assert!(is_public("2606:2800:220:1::1".parse().expect("an address")));
    }

    #[test]
    fn a_picture_claiming_more_pixels_than_it_could_hold_is_refused() {
        // A PNG header declaring 60 000 × 60 000 in 33 bytes: 3.6 gigapixels,
        // 14 GB once something allocates four bytes per pixel.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&60_000u32.to_be_bytes());
        bytes.extend_from_slice(&60_000u32.to_be_bytes());

        let error = fetcher()
            .accept(bytes, "a.png", None, "image/png".to_owned())
            .expect_err("a bomb is refused");
        assert!(error.contains("the limit is"), "{error}");
    }

    #[test]
    fn html_served_as_an_image_is_refused_with_what_it_actually_was() {
        let error = fetcher()
            .accept(
                b"<!doctype html><title>Sign in".to_vec(),
                "a.png",
                None,
                "image/png".to_owned(),
            )
            .expect_err("a login page is not a picture");
        assert!(error.contains("not a picture"), "{error}");
        assert!(error.contains("<!doctype html>"), "{error}");
    }

    #[test]
    fn bytes_that_do_not_hash_to_what_the_row_claimed_are_refused() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(&8u32.to_be_bytes());

        let error = fetcher()
            .accept(
                bytes.clone(),
                "a.png",
                Some(&"ab".repeat(32)),
                "image/png".to_owned(),
            )
            .expect_err("a mismatched content address is refused");
        assert!(
            error.contains("are not the bytes that row is about"),
            "{error}"
        );

        let accepted = fetcher()
            .accept(
                bytes.clone(),
                "a.png",
                Some(&digest(&bytes)),
                "image/png".to_owned(),
            )
            .expect("the right digest passes");
        assert_eq!(accepted.width, 8);
        assert_eq!(accepted.content_type, "image/png");
    }
}
