//! A hosted execution's words, sealed under the archive's own keys.
//!
//! An agent graph's every hop carries text, and the workflow stream carries a
//! *reference* to it rather than the text. Where that text lives is the run's
//! choice: `external` leaves it wherever the worker keeps it and this crate
//! never sees it, and `sealed` brings it here — which is what a deployment
//! under a retention obligation picks, because a reference into somebody's
//! laptop is not something an erasure can reach.
//!
//! It is in this crate rather than beside the workflow store for one reason:
//! this is where the keys are, and a second place that encrypted content would
//! be a second key layout to get right. The key path is the AEAD's associated
//! data here as it is for a turn, so a ciphertext copied from one execution to
//! another does not open.
//!
//! ## What this does not do yet
//!
//! Erasure by subject does not reach these. A turn names whose words it holds;
//! a hosted payload does not, because nothing on the wire says so — the worker
//! would have to declare it. Retention by age, and deletion with the execution,
//! do reach them. A deployment sealing under an obligation to erase by subject
//! needs that field first, and this note is what says so out loud.

use crate::store::Backend;
use crate::{Error, Result, digest};

/// Where sealed content went, and what it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedPayload {
    /// What the workflow stream records. Resolvable only through this
    /// instance, which is the difference from an `external` reference.
    pub reference: String,
    /// `sha256` of the plaintext, hex. The same address the caller computed,
    /// recomputed here rather than believed — a content address supplied by
    /// the caller would let two different payloads occupy one key.
    pub digest: String,
    pub size: usize,
}

/// The scheme a sealed reference is written under.
///
/// Named rather than a bare path so a reader of a workflow stream can tell at a
/// glance which side of the policy a run was on.
pub const SCHEME: &str = "aiwatcher://";

/// The reference one sealed payload is fetched by.
#[must_use]
pub fn reference_of(execution: &str, content_digest: &str) -> String {
    format!("{SCHEME}executions/{execution}/payloads/{content_digest}")
}

/// The execution and digest a reference names, if it is one of ours.
#[must_use]
pub fn parse_reference(reference: &str) -> Option<(&str, &str)> {
    let rest = reference
        .strip_prefix(SCHEME)?
        .strip_prefix("executions/")?;
    let (execution, tail) = rest.split_once("/payloads/")?;
    (!execution.is_empty() && !tail.is_empty() && !tail.contains('/')).then_some((execution, tail))
}

pub(crate) fn payload_key(backend: &Backend, execution: &str, content_digest: &str) -> String {
    // The execution is hashed for the reason every caller-supplied name here
    // is: it comes from a producer that never heard of this crate, and a key
    // that embedded one holding a slash would put one run's payloads inside
    // another's prefix.
    format!(
        "{}/executions/{}/payloads/{content_digest}.json",
        backend.prefix(),
        digest(execution.as_bytes())
    )
}

pub(crate) async fn seal(
    backend: &Backend,
    execution: &str,
    plaintext: &[u8],
) -> Result<SealedPayload> {
    if plaintext.is_empty() {
        return Err(Error::Invalid(
            "a sealed payload with nothing in it".to_owned(),
        ));
    }
    let content_digest = digest(plaintext);
    let key = payload_key(backend, execution, &content_digest);
    backend.seal_bytes(&key, plaintext).await?;
    Ok(SealedPayload {
        reference: reference_of(execution, &content_digest),
        digest: content_digest,
        size: plaintext.len(),
    })
}

pub(crate) async fn open(
    backend: &Backend,
    execution: &str,
    content_digest: &str,
) -> Result<Vec<u8>> {
    let key = payload_key(backend, execution, content_digest);
    backend
        .open_bytes(&key)
        .await?
        .ok_or_else(|| Error::NotFound(format!("a sealed payload {content_digest} of {execution}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_names_one_execution_and_one_payload() {
        let reference = reference_of("run-1", "a".repeat(64).as_str());
        assert_eq!(
            parse_reference(&reference),
            Some(("run-1", "a".repeat(64).as_str()))
        );
    }

    #[test]
    fn a_reference_this_instance_did_not_issue_is_not_one_of_ours() {
        // The whole point of the scheme: an `external` run's reference points
        // at the worker's own store, and reading it here would be this instance
        // claiming to hold words it never saw.
        assert!(parse_reference("file:///tmp/x.json").is_none());
        assert!(parse_reference("agentic://turn/7").is_none());
        // And a digest with a slash in it would be a path, not a name.
        assert!(parse_reference("aiwatcher://executions/run-1/payloads/a/b").is_none());
        assert!(parse_reference("aiwatcher://executions//payloads/abc").is_none());
    }
}
