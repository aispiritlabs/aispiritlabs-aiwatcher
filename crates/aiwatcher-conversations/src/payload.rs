//! A hosted execution's words, sealed under the archive's own keys.
//!
//! An agent graph's hops carry text, and the workflow stream carries a
//! *reference* to it. `external` leaves the text wherever the worker keeps it;
//! `sealed` brings it here, which is what a deployment under a retention
//! obligation picks — an erasure cannot reach a reference into a laptop. It
//! lives in this crate because the keys do, and the key path is the AEAD's
//! associated data as it is for a turn, so a ciphertext copied from one
//! execution to another does not open.
//!
//! A payload lives as long as its run. Each has a plaintext `PayloadHead`
//! naming the run, because the key hashes the execution and a hash does not
//! walk back. `aiwatcher-server`'s archive sweep reads the heads and erases the
//! payloads whose run the workflow store has forgotten — the join is there
//! because this crate may not name `aiwatcher-execution`. The two orderings run
//! opposite ways for one reason, that content a sweep cannot find is content
//! nothing can erase:
//!
//! - **Sealing** writes the head before the content.
//! - **Erasing** deletes the content before the head; a crash leaves a head
//!   naming nothing, which the next pass finishes.
//!
//! Erasure *by subject* does not reach these: a payload does not say whose
//! words it holds, because nothing on the wire declares it. A deployment with
//! that obligation performs that part itself.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

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

/// What a head's key ends with, and what tells one from the payload beside it.
const HEAD_SUFFIX: &str = ".head.json";

/// The plaintext beside one sealed payload: which run it belongs to.
///
/// The archive's own shape, for the archive's own reason. A turn's head is
/// plaintext so a review queue can be read without decrypting anything; this
/// one is plaintext so a payload can be *found* by the run it belongs to.
/// [`payload_key`] hashes the execution and a hash does not walk back, so
/// without this the only thing a listing could say about a bucket of sealed
/// payloads is how many there are.
///
/// It holds no content and no digest of any subject — only what a sweep has to
/// know to ask the workflow store whether that run still exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PayloadHead {
    pub(crate) execution: String,
    pub(crate) content_digest: String,
    pub(crate) size: usize,
}

/// Everything this instance has sealed, across every run.
pub(crate) fn payloads_prefix(backend: &Backend) -> String {
    format!("{}/executions/", backend.prefix())
}

/// One run's payloads and their heads.
pub(crate) fn execution_prefix(backend: &Backend, execution: &str) -> String {
    format!(
        "{}/executions/{}/payloads/",
        backend.prefix(),
        digest(execution.as_bytes())
    )
}

pub(crate) fn head_key(backend: &Backend, execution: &str, content_digest: &str) -> String {
    format!(
        "{}{content_digest}{HEAD_SUFFIX}",
        execution_prefix(backend, execution)
    )
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
    // The head before the content, which inverts the registry's rule on
    // purpose. There the ordering protects a pointer to bytes worth keeping, so
    // the object goes first; here the obligation runs the other way — content a
    // sweep cannot find is content nothing can erase — so nothing is sealed
    // before the thing that finds it exists. A crash in between leaves a head
    // naming no payload, which reads as `NotFound` and erases as a no-op.
    backend
        .write(
            &head_key(backend, execution, &content_digest),
            &PayloadHead {
                execution: execution.to_owned(),
                content_digest: content_digest.clone(),
                size: plaintext.len(),
            },
        )
        .await?;
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

/// Every run this instance holds sealed payloads for.
///
/// Read from the heads rather than from the keys, because the keys are hashes.
/// The caller is whoever can answer whether a run still exists — which is not
/// this crate: `aiwatcher-conversations` sits above `aiwatcher-execution` and
/// may not name it.
pub(crate) async fn executions(backend: &Backend) -> Result<Vec<String>> {
    let mut runs = BTreeSet::new();
    for entry in backend.list(&payloads_prefix(backend)).await? {
        if !entry.key.ends_with(HEAD_SUFFIX) {
            continue;
        }
        if let Some(head) = backend.read::<PayloadHead>(&entry.key).await? {
            runs.insert(head.execution);
        }
    }
    Ok(runs.into_iter().collect())
}

/// Remove every payload one run sealed, and the heads that found them.
///
/// Content before heads, for the reason the seal writes them the other way
/// round: a crash here leaves a head naming nothing, which the next pass
/// finishes. The reverse would leave sealed bytes no listing could attribute to
/// a run again.
///
/// Answers how many payloads went, so a repeated sweep is visibly a no-op.
pub(crate) async fn erase_execution(backend: &Backend, execution: &str) -> Result<usize> {
    let entries = backend.list(&execution_prefix(backend, execution)).await?;
    let (heads, payloads): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .map(|entry| entry.key)
        .partition(|key| key.ends_with(HEAD_SUFFIX));
    let erased = payloads.len();
    for key in payloads {
        backend.delete(&key).await?;
    }
    for key in heads {
        backend.delete(&key).await?;
    }
    Ok(erased)
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
