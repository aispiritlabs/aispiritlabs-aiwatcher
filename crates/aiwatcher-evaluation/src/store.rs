//! Private keys and verified immutable bytes. Nothing here trusts a caller digest.
use crate::{EvaluationError, EvidenceState, Result, canonical};
use aiwatcher_core::storage::ObjectStore;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Store(pub Arc<dyn ObjectStore>);

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

impl Store {
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
