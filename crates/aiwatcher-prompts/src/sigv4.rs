//! AWS Signature Version 4, header-based, for S3.
//!
//! Written out rather than pulled in. The alternative is `aws-sdk-s3`, which
//! brings a credential-provider chain, a retry policy, a region resolver and
//! an HTTP stack this workspace already has — several hundred crates to sign
//! four request shapes against a bucket whose endpoint is in an environment
//! variable. The signing itself is a fixed sequence of HMACs with no policy in
//! it, so it is the rare case where writing it is smaller *and* clearer than
//! depending on it.
//!
//! What it is not is a place to improvise. The canonicalisation rules below
//! are the specification's, and the tests pin them against RFC 4231's HMAC
//! vectors and against a request whose signature is computed by hand — a
//! signer that is subtly wrong fails as `403 SignatureDoesNotMatch` with no
//! indication of which of the five steps drifted.

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::macros::format_description;

/// The algorithm this signs with. There is no other.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// `x-amz-content-sha256` for a request with no body.
pub const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Credentials and the scope they are valid in.
#[derive(Clone)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Set for temporary credentials; becomes `x-amz-security-token`.
    pub session_token: Option<String>,
    /// RustFS and MinIO do not care what this says, only that both sides say
    /// the same thing. AWS does.
    pub region: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret must not reach a log through a `#[derive(Debug)]` on some
        // struct three layers up that happens to hold these.
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("region", &self.region)
            .field("session_token", &self.session_token.is_some())
            .finish()
    }
}

/// One request to sign.
#[derive(Debug)]
pub struct Request<'a> {
    pub method: &'a str,
    /// Already percent-encoded per [`encode_path`], and starting with `/`.
    pub canonical_path: &'a str,
    /// Sorted by name; values raw, encoded here.
    pub query: &'a [(String, String)],
    /// `host:port` exactly as it appears in the `Host` header.
    pub host: &'a str,
    pub payload: &'a [u8],
    pub at: OffsetDateTime,
}

/// The headers to add to a request so it is authorised.
///
/// Returned rather than applied so the caller decides what to do with them —
/// and so the whole of this module is testable without an HTTP client.
#[derive(Debug, PartialEq, Eq)]
pub struct SignedHeaders {
    pub authorization: String,
    /// `YYYYMMDD'T'HHMMSS'Z'`.
    pub amz_date: String,
    pub content_sha256: String,
    pub session_token: Option<String>,
}

/// Sign a request for the `s3` service.
#[must_use]
pub fn sign(credentials: &Credentials, request: &Request<'_>) -> SignedHeaders {
    let amz_date = format_amz_date(request.at);
    let date_stamp = &amz_date[..8];
    let content_sha256 = hex::encode(Sha256::digest(request.payload));

    // Signed headers are the minimum that binds the request: the host it is
    // going to, the moment it is valid at, and the body it carries. Signing
    // fewer would let a proxy retarget or rewrite it.
    let mut headers: Vec<(String, String)> = vec![
        ("host".to_owned(), request.host.to_owned()),
        ("x-amz-content-sha256".to_owned(), content_sha256.clone()),
        ("x-amz-date".to_owned(), amz_date.clone()),
    ];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_owned(), token.clone()));
    }
    headers.sort_by(|left, right| left.0.cmp(&right.0));

    let signed_header_names = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();

    let canonical_query = request
        .query
        .iter()
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        request.method,
        request.canonical_path,
        canonical_query,
        canonical_headers,
        signed_header_names,
        content_sha256,
    );

    let scope = format!("{date_stamp}/{}/s3/aws4_request", credentials.region);
    let string_to_sign = format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes())),
    );

    let signature = hex::encode(sign_bytes(
        &signing_key(
            &credentials.secret_access_key,
            date_stamp,
            &credentials.region,
        ),
        string_to_sign.as_bytes(),
    ));

    SignedHeaders {
        authorization: format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_header_names}, \
             Signature={signature}",
            credentials.access_key_id,
        ),
        amz_date,
        content_sha256,
        session_token: credentials.session_token.clone(),
    }
}

/// `kSigning`: four chained HMACs, each keyed by the previous one.
///
/// Deriving per date and region is what stops a captured signature from being
/// replayable against another day or another bucket.
fn signing_key(secret: &str, date_stamp: &str, region: &str) -> [u8; 32] {
    let initial = format!("AWS4{secret}");
    let date = sign_bytes(initial.as_bytes(), date_stamp.as_bytes());
    let regional = sign_bytes(&date, region.as_bytes());
    let service = sign_bytes(&regional, b"s3");
    sign_bytes(&service, b"aws4_request")
}

/// HMAC-SHA256, per RFC 2104.
///
/// Sixteen lines against a dependency whose only other user would be this
/// file. Pinned by RFC 4231's vectors in the tests below.
fn sign_bytes(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }

    let mut inner = Sha256::new();
    inner.update(padded.map(|byte| byte ^ 0x36));
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(padded.map(|byte| byte ^ 0x5c));
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Percent-encode an object key into a canonical URI path.
///
/// `/` stays a separator; everything outside the unreserved set becomes
/// `%XX` in uppercase. S3 signs the path encoded exactly **once** — the double
/// encoding other AWS services use is what makes a key containing `+` or a
/// space fail against S3 and succeed everywhere else.
#[must_use]
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Percent-encode one path segment, query key or query value.
///
/// A space is `%20`, never `+`: the canonical request is not a form body, and
/// a `+` there signs a different string than the one sent.
#[must_use]
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

/// `YYYYMMDD'T'HHMMSS'Z'`, which is the only timestamp format SigV4 accepts.
#[must_use]
pub fn format_amz_date(at: OffsetDateTime) -> String {
    let description = format_description!("[year][month][day]T[hour repr:24][minute][second]Z");
    at.to_offset(time::UtcOffset::UTC)
        .format(description)
        // Formatting a valid `OffsetDateTime` with a static description cannot
        // fail; falling back to the epoch keeps the signature deterministic
        // and wrong rather than taking the process down over telemetry.
        .unwrap_or_else(|_| "19700101T000000Z".to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn hmac_sha256_matches_the_rfc_4231_vectors() {
        // Test case 1: a 20-byte key of 0x0b.
        assert_eq!(
            hex::encode(sign_bytes(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Test case 2: a key shorter than the block, a longer message.
        assert_eq!(
            hex::encode(sign_bytes(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a 131-byte key, which is hashed down to 32 first. The
        // branch that is easiest to get wrong and hardest to notice, because
        // every key this signer builds itself is short.
        assert_eq!(
            hex::encode(sign_bytes(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_space_in_a_key_is_encoded_as_the_specification_requires() {
        // `+` here would sign a different string than the one sent, and the
        // failure is a 403 with no clue in it.
        assert_eq!(encode_component("a b"), "a%20b");
        assert_eq!(encode_component("a+b"), "a%2Bb");
        assert_eq!(encode_component("a/b"), "a%2Fb");
        assert_eq!(encode_component("-_.~"), "-_.~");
        assert_eq!(encode_component("ą"), "%C4%85");
    }

    #[test]
    fn a_path_keeps_its_separators_and_encodes_everything_else() {
        assert_eq!(
            encode_path("/bucket/prompts/planner.floor-plan/head.json"),
            "/bucket/prompts/planner.floor-plan/head.json"
        );
        assert_eq!(encode_path("/bucket/a b/c"), "/bucket/a%20b/c");
    }

    fn fixture() -> Credentials {
        Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_owned(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned(),
            session_token: None,
            region: "us-east-1".to_owned(),
        }
    }

    /// The reference chain, recomputed here from the specification's own steps.
    ///
    /// This does not prove the signature is the one AWS would produce — only a
    /// real endpoint does that, which is what `tests/rustfs.rs` is for. What it
    /// proves is that `sign` composes the five steps in the documented order,
    /// so a later edit that reorders the canonical request or drops a header
    /// fails here instead of at a bucket.
    #[test]
    fn the_signature_is_the_documented_chain_of_the_documented_strings() {
        let credentials = fixture();
        let at = datetime!(2013-05-24 00:00:00 UTC);
        let signed = sign(
            &credentials,
            &Request {
                method: "GET",
                canonical_path: "/examplebucket/test.txt",
                query: &[],
                host: "s3.amazonaws.com",
                payload: b"",
                at,
            },
        );

        let canonical_request = concat!(
            "GET\n",
            "/examplebucket/test.txt\n",
            "\n",
            "host:s3.amazonaws.com\n",
            "x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
            "x-amz-date:20130524T000000Z\n",
            "\n",
            "host;x-amz-content-sha256;x-amz-date\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let expected = hex::encode(sign_bytes(
            &signing_key(&credentials.secret_access_key, "20130524", "us-east-1"),
            string_to_sign.as_bytes(),
        ));

        assert!(
            signed
                .authorization
                .ends_with(&format!("Signature={expected}")),
            "{}",
            signed.authorization
        );
        assert!(signed.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="
        ));
        assert_eq!(signed.content_sha256, EMPTY_PAYLOAD_SHA256);
        assert_eq!(signed.amz_date, "20130524T000000Z");
    }

    #[test]
    fn a_session_token_is_signed_rather_than_merely_sent() {
        // Sending it unsigned is accepted by some implementations and rejected
        // by AWS, which is the worst combination: it works in development.
        let credentials = Credentials {
            session_token: Some("token".to_owned()),
            ..fixture()
        };
        let signed = sign(
            &credentials,
            &Request {
                method: "GET",
                canonical_path: "/b/k",
                query: &[],
                host: "example",
                payload: b"",
                at: datetime!(2013-05-24 00:00:00 UTC),
            },
        );
        assert!(
            signed.authorization.contains(
                "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
            )
        );
        assert_eq!(signed.session_token.as_deref(), Some("token"));
    }

    #[test]
    fn the_payload_hash_covers_the_body() {
        let credentials = fixture();
        let at = datetime!(2024-01-02 03:04:05 UTC);
        let put = |payload: &'static [u8]| {
            sign(
                &credentials,
                &Request {
                    method: "PUT",
                    canonical_path: "/b/k",
                    query: &[],
                    host: "example",
                    payload,
                    at,
                },
            )
        };
        assert_ne!(put(b"one"), put(b"two"));
    }

    #[test]
    fn the_date_is_formatted_the_only_way_sigv4_accepts() {
        assert_eq!(
            format_amz_date(datetime!(2024-12-31 23:59:59 UTC)),
            "20241231T235959Z"
        );
        // A non-UTC instant is converted rather than stamped with its own
        // offset; SigV4 has no offset field and would read the digits as UTC.
        assert_eq!(
            format_amz_date(datetime!(2024-12-31 23:59:59 +02:00)),
            "20241231T215959Z"
        );
    }
}
