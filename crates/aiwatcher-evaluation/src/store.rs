//! Private keys and verified immutable bytes. Nothing here trusts a caller digest.
use crate::{EvaluationError, EvaluationReceipt, EvidenceState, Result, canonical};
use aiwatcher_core::storage::ObjectStore;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Store(
    pub Arc<dyn ObjectStore>,
    pub Option<Arc<dyn EvidenceCipher>>,
);

/// Deployment encryption, without a dependency on another domain's storage.
/// Implementations authenticate the full object path and never log plaintext.
pub trait EvidenceCipher: Send + Sync + std::fmt::Debug {
    fn seal(&self, path: &str, plaintext: &[u8]) -> Result<serde_json::Value>;
    fn open(&self, path: &str, envelope: &serde_json::Value) -> Result<Vec<u8>>;
}

#[derive(Serialize, Deserialize)]
struct Protected {
    evaluation_sealed_v1: serde_json::Value,
}

/// An intent contains no source material. Its first deadline is never renewed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub evaluation_id: String,
    pub expires_at: i64,
}

/// Old receipt objects remain readable and keep their exact serialized shape.
/// Collection competes with publication at this SAME immutable key, not at a
/// separate tombstone that could race the final commit.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Claim {
    Committed(EvaluationReceipt),
    Abandoned { abandoned: Pending },
}
impl Claim {
    pub fn id(&self) -> &str {
        match self {
            Self::Committed(receipt) => &receipt.evaluation_id,
            Self::Abandoned { abandoned } => &abandoned.evaluation_id,
        }
    }
    pub fn receipt(self) -> Result<EvaluationReceipt> {
        match self {
            Self::Committed(receipt) => Ok(receipt),
            Self::Abandoned { .. } => Err(EvaluationError::Unavailable(EvidenceState::Expired)),
        }
    }
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(crate) fn root(id: &str) -> String {
    format!("evaluations/{}/", hash(id.as_bytes()))
}
pub(crate) fn claim(id: &str) -> String {
    format!("{}claim.json", root(id))
}
pub(crate) fn tombstone(id: &str) -> String {
    format!("{}tombstone.json", root(id))
}
pub(crate) fn content(id: &str) -> String {
    format!("{}content/", root(id))
}
pub(crate) fn pending(id: &str) -> String {
    format!("{}pending.json", root(id))
}

/// The catalogue, in published order.
///
/// Derived, the way a prompt's head is derived: the claims are the truth and
/// every detail read still goes through them. What this adds is an order an
/// object store can serve — a result's own key is the hash of its ID, so
/// "newest first" was a scan of every object under `evaluations/` to answer.
pub(crate) const INDEX: &str = "evaluations/index/";
/// Newest first, because a key's order is the only order a listing has. The
/// complement of the second makes an ascending listing a descending clock, and
/// the hash keeps a producer's own ID out of a key.
pub(crate) fn indexed(committed_at: i64, id: &str) -> String {
    format!(
        "{INDEX}{:019}-{}",
        i64::MAX.saturating_sub(committed_at),
        hash(id.as_bytes())
    )
}
/// The bound an ascending listing stops at for evidence published since then.
pub(crate) fn indexed_since(committed_at: i64) -> String {
    format!("{INDEX}{:019}-~", i64::MAX.saturating_sub(committed_at))
}

/// Approvals live beside the evidence and never under an evaluation ID: one
/// approval admits every repetition of its pair, and outlives all of them.
pub(crate) const APPROVALS: &str = "evaluations/approvals/";
/// Outside every prefix a scan filters on, and deliberately overwritten rather
/// than versioned: it is the last pass, not a history of passes.
pub(crate) const RETENTION: &str = "evaluations/retention.json";

/// Rubrics and assessments are authored rather than measured, and they live
/// outside `evaluations/` for a cost reason as much as an ownership one: the
/// sweep and the collection pass each list that whole prefix to filter it by
/// suffix, so a hundred thousand judgements there would be a hundred thousand
/// keys every pass walks past.
pub(crate) const RUBRICS: &str = "evaluation-rubrics/";
pub(crate) const ASSESSMENTS: &str = "evaluation-assessments/";
/// A scorecard is authored too, and it is read at the start of every run
/// rather than measured, so it keeps the same distance from the sweep.
pub(crate) const SCORECARDS: &str = "evaluation-scorecards/";
/// The name is hashed for the same reason an evaluation ID is: it is a
/// producer's text, and a key is not the place to find out it held a slash.
pub(crate) fn rubric_head(name: &str) -> String {
    format!("{RUBRICS}{}/head.json", hash(name.as_bytes()))
}
pub(crate) fn rubric_version(name: &str, version: &str) -> String {
    format!("{RUBRICS}{}/versions/{version}.json", hash(name.as_bytes()))
}
pub(crate) fn scorecard_head(name: &str) -> String {
    format!("{SCORECARDS}{}/head.json", hash(name.as_bytes()))
}
pub(crate) fn scorecard_version(name: &str, version: &str) -> String {
    format!(
        "{SCORECARDS}{}/versions/{version}.json",
        hash(name.as_bytes())
    )
}
pub(crate) fn assessments(target_id: &str) -> String {
    format!("{ASSESSMENTS}{target_id}/")
}
pub(crate) fn assessment_standing(target_id: &str, standing_id: &str) -> String {
    format!("{ASSESSMENTS}{target_id}/{standing_id}/")
}
/// Zero padded, so a listing's order is the revision order and the last key of
/// a standing judgement's group is its current revision.
pub(crate) fn assessment(target_id: &str, standing_id: &str, revision: u32) -> String {
    format!(
        "{}{revision:010}.json",
        assessment_standing(target_id, standing_id)
    )
}
pub(crate) fn approval(id: &str) -> String {
    format!("{APPROVALS}{id}/record.json")
}
pub(crate) fn withdrawal(id: &str) -> String {
    format!("{APPROVALS}{id}/withdrawn.json")
}

impl Store {
    pub async fn begin(&self, intent: Pending, now: i64) -> Result<()> {
        let id = &intent.evaluation_id;
        self.create(&pending(id), &intent).await?;
        let stored: Pending = self
            .read(&pending(id))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        if stored.evaluation_id != *id {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        if stored.expires_at <= now {
            self.create(&claim(id), &Claim::Abandoned { abandoned: stored })
                .await?;
        }
        if let Some(value) = self.read::<Claim>(&claim(id)).await? {
            value.receipt()?;
        }
        Ok(())
    }
    pub async fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.0
            .get(key)
            .await?
            .map(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|_| EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
            })
            .transpose()
    }
    pub async fn create<T: Serialize>(&self, key: &str, value: &T) -> Result<bool> {
        Ok(self.0.create(key, canonical(value)?).await?)
    }
    pub async fn artifact<T: Serialize>(
        &self,
        id: &str,
        value: &T,
        protected: bool,
    ) -> Result<String> {
        let bytes = canonical(value)?;
        let digest = hash(&bytes);
        let key = format!("{}{digest}.json", content(id));
        let stored = if protected {
            let cipher = self
                .1
                .as_ref()
                .ok_or(EvaluationError::Unavailable(EvidenceState::Forbidden))?;
            canonical(&Protected {
                evaluation_sealed_v1: cipher.seal(&key, &bytes)?,
            })?
        } else {
            bytes.clone()
        };
        self.0.create(&key, stored).await?;
        let (found, sealed) = self.open(id, &digest).await?;
        if found != bytes || (protected && !sealed) {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(digest)
    }
    async fn open(&self, id: &str, digest: &str) -> Result<(Vec<u8>, bool)> {
        let key = format!("{}{digest}.json", content(id));
        let bytes = self
            .0
            .get(&key)
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        // A serde struct also accepts a one-element JSON sequence. Detect only
        // the named OBJECT field, or an old one-row shard looks like a seal.
        let envelope = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value.get("evaluation_sealed_v1").cloned());
        let (bytes, sealed) = if let Some(envelope) = envelope {
            let cipher = self
                .1
                .as_ref()
                .ok_or(EvaluationError::Unavailable(EvidenceState::Forbidden))?;
            (cipher.open(&key, &envelope)?, true)
        } else {
            (bytes, false)
        };
        if hash(&bytes) != digest {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        Ok((bytes, sealed))
    }
    pub async fn verified<T: DeserializeOwned>(&self, id: &str, digest: &str) -> Result<T> {
        self.verified_protected(id, digest, false).await
    }
    /// The object and whether it arrived sealed. Metadata names the dataset its
    /// own protection depends on, so reading it twice to learn that is the one
    /// full re-read a summary can never avoid by asking in a better order.
    pub async fn opened<T: DeserializeOwned>(&self, id: &str, digest: &str) -> Result<(T, bool)> {
        let (bytes, sealed) = self.open(id, digest).await?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| EvaluationError::Unavailable(EvidenceState::CorruptArtifact))?;
        Ok((value, sealed))
    }
    pub async fn verified_protected<T: DeserializeOwned>(
        &self,
        id: &str,
        digest: &str,
        protected: bool,
    ) -> Result<T> {
        let (bytes, sealed) = self.open(id, digest).await?;
        if protected && !sealed {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    }
    pub async fn erase(&self, id: &str) -> Result<()> {
        for entry in self.0.list(&content(id)).await? {
            self.0.delete(&entry.key).await?;
        }
        Ok(())
    }
}
