//! Verified snapshots for consumers that must prove an immutable source pin.
use crate::{
    DatasetSummary, DatasetVersion, MAX_ARTIFACT_BYTES, MAX_ITEMS, Registry, RegistryError, Result,
    content_identity, digest, validate_name,
};

pub(crate) async fn read(registry: &Registry, name: &str, version: &str) -> Result<DatasetVersion> {
    validate_name(name, "dataset")?;
    if version.len() != 64
        || !version
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(RegistryError::Invalid(
            "an exact dataset digest is required".into(),
        ));
    }
    let missing = || RegistryError::NotFound(format!("dataset {name} version {version}"));
    let head: DatasetSummary = registry
        .read_json(&registry.dataset_head_key(name))
        .await?
        .ok_or_else(missing)?;
    if !head.versions.iter().any(|item| item.version == version) {
        return Err(missing());
    }
    let key = registry.dataset_version_key(name, version);
    let corrupt = |message: &str| RegistryError::Corrupt {
        key: key.clone(),
        message: message.into(),
    };
    let bytes = registry.store.get(&key).await?.ok_or_else(missing)?;
    // Published content has a 4 MiB limit. This verified read reserves another
    // 256 KiB for catalogue metadata before deserializing the snapshot.
    if bytes.len() > MAX_ARTIFACT_BYTES + 256 * 1024 {
        return Err(corrupt("version exceeds the published artifact budget"));
    }
    let artifact: DatasetVersion =
        serde_json::from_slice(&bytes).map_err(|error| corrupt(&error.to_string()))?;
    let identity = content_identity(
        &artifact.pipeline,
        artifact.engine,
        &artifact.summary.columns,
        &artifact.items,
        &artifact.source,
        artifact.window_seconds,
    )?;
    if head.name != name
        || artifact.name != name
        || artifact.summary.version != version
        || artifact.summary.row_count != artifact.items.len()
        || artifact.items.len() > MAX_ITEMS
        || identity.len() > MAX_ARTIFACT_BYTES
        || digest(&identity) != version
    {
        return Err(corrupt(
            "version content does not match its published identity",
        ));
    }
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PublishDatasetRequest, QueryEngine};
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use std::sync::Arc;

    fn request(engine: QueryEngine) -> PublishDatasetRequest {
        serde_json::from_value(serde_json::json!({
            "name": "verified", "pipeline": "rows", "engine": engine,
            "columns": ["answer"], "items": [{"answer": "original"}], "source": "test"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn exact_versions_survive_head_changes_and_preserve_each_engines_identity() {
        for engine in QueryEngine::ALL {
            let registry = Registry::new(Arc::new(MemoryObjectStore::new()), "datasets");
            let first = registry.publish(request(engine)).await.unwrap();
            let pin = &first.dataset.latest.version;
            let mut next = request(engine);
            next.items[0].insert("answer".into(), "new head".into());
            registry.publish(next).await.unwrap();
            let found = registry.verified_version("verified", pin).await.unwrap();
            assert_eq!(found.items[0]["answer"], "original");
            assert_eq!(found.engine, engine);
            for alias in ["latest", "production", "../head", "", &"A".repeat(64)] {
                assert!(matches!(
                    registry.verified_version("verified", alias).await,
                    Err(RegistryError::Invalid(_))
                ));
            }
            // Catalogue fields are deliberately outside the version digest.
            let key = registry.dataset_version_key("verified", pin);
            let mut metadata = found;
            metadata.description = "updated catalogue".into();
            registry.write_json(&key, &metadata).await.unwrap();
            assert!(registry.verified_version("verified", pin).await.is_ok());
            registry.store.delete(&key).await.unwrap();
            assert!(matches!(
                registry.verified_version("verified", pin).await,
                Err(RegistryError::NotFound(_))
            ));
        }
    }

    #[tokio::test]
    async fn forged_rows_and_identity_fields_are_detected_without_trusting_the_head() {
        let registry = Registry::new(Arc::new(MemoryObjectStore::new()), "datasets");
        let first = registry.publish(request(QueryEngine::Flow)).await.unwrap();
        let pin = &first.dataset.latest.version;
        let key = registry.dataset_version_key("verified", pin);
        let original: serde_json::Value = registry.read_json(&key).await.unwrap().unwrap();
        for (pointer, replacement) in [
            ("/items/0/answer", serde_json::json!("tampered")),
            ("/pipeline", serde_json::json!("changed")),
            ("/engine", serde_json::json!("duckdb")),
            ("/columns/0", serde_json::json!("other")),
            ("/source", serde_json::json!("other")),
            ("/name", serde_json::json!("other")),
            ("/version", serde_json::json!("0".repeat(64))),
            ("/row_count", serde_json::json!(99)),
        ] {
            let mut changed = original.clone();
            if pointer == "/engine" {
                changed["engine"] = replacement;
            } else {
                *changed.pointer_mut(pointer).unwrap() = replacement;
            }
            registry.write_json(&key, &changed).await.unwrap();
            assert!(
                matches!(
                    registry.verified_version("verified", pin).await,
                    Err(RegistryError::Corrupt { .. })
                ),
                "{pointer}"
            );
        }
        registry.write_json(&key, &original).await.unwrap();
        registry
            .store
            .delete(&registry.dataset_head_key("verified"))
            .await
            .unwrap();
        assert!(matches!(
            registry.verified_version("verified", pin).await,
            Err(RegistryError::NotFound(_))
        ));
    }
}
