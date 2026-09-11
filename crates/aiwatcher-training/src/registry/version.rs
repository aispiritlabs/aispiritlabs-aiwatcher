//! Exact model identity reads, independent of the mutable model index.
use super::Registry;
use crate::{Error, ModelVersion, Result, validate_slug};

impl Registry {
    /// Read an exact version and verify its historical content identity.
    ///
    /// The identity covers provenance, scores and ordered artifact digests.
    /// It does not authenticate runtime, artifact locations or other metadata,
    /// nor fetch artifact bytes. Callers must approve those and verify bytes
    /// separately. The derived head and the current run are not prerequisites.
    pub async fn verified_version(&self, name: &str, version: &str) -> Result<ModelVersion> {
        validate_slug(name, "a model name")?;
        if version.len() != 64
            || !version
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid(
                "a model version must be a lowercase sha256 digest".into(),
            ));
        }
        let key = self.model_version_key(name, version);
        let body = self
            .store
            .get(&key)
            .await?
            .ok_or_else(|| Error::NotFound(format!("the version {version} of {name}")))?;
        if body.len() > 1024 * 1024 {
            return Err(Error::TooLarge {
                what: "the model version record",
                size: body.len(),
                limit: 1024 * 1024,
            });
        }
        let corrupt = |message: String| Error::Corrupt {
            key: key.clone(),
            message,
        };
        let found: ModelVersion =
            serde_json::from_slice(&body).map_err(|e| corrupt(e.to_string()))?;
        if let Some(package) = &found.package {
            package.validate().map_err(|e| corrupt(e.to_string()))?;
        }
        let identity = crate::model::version_id(
            &found.name,
            &found.run_id,
            &found.checkpoint_uri,
            &found.dataset,
            &found.metrics,
            found.package.as_ref(),
        )
        .map_err(|e| corrupt(e.to_string()))?;
        if found.name != name || found.version != version || identity != version {
            return Err(corrupt(
                "model content does not match the pinned identity".into(),
            ));
        }
        Ok(found)
    }
}
