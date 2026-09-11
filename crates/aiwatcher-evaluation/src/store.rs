//! Private keys and verified immutable bytes. Nothing here trusts a caller digest.
use crate::{EvaluationError, EvaluationReceipt, EvidenceState, Result, canonical};
use aiwatcher_core::storage::ObjectStore;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Store(pub Arc<dyn ObjectStore>);

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
    pub async fn artifact<T: Serialize>(&self, id: &str, value: &T) -> Result<String> {
        let bytes = canonical(value)?;
        let digest = hash(&bytes);
        let key = format!("{}{digest}.json", content(id));
        self.0.create(&key, bytes.clone()).await?;
        if self.0.get(&key).await?.as_deref() != Some(bytes.as_slice()) {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(digest)
    }
    pub async fn verified<T: DeserializeOwned>(&self, id: &str, digest: &str) -> Result<T> {
        let bytes = self
            .0
            .get(&format!("{}{digest}.json", content(id)))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        if hash(&bytes) != digest {
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
