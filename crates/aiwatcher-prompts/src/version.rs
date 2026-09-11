//! Verify an immutable prompt through its owner, independently of the index.
use aiwatcher_core::prompts::{PromptName, PromptVersion, PromptVersionId, variables_of};

use crate::{Registry, RegistryError, Result};

pub(crate) async fn read(
    registry: &Registry,
    name: &PromptName,
    version: &PromptVersionId,
) -> Result<Option<PromptVersion>> {
    let key = registry.version_key(name, version);
    let Some(bytes) = registry.store.get(&key).await? else {
        return Ok(None);
    };
    let corrupt = |reason| RegistryError::Integrity {
        key: key.clone(),
        reason,
    };
    // JSON may escape each text byte into six characters. The remaining
    // allowance bounds catalogue metadata on this verified read path.
    let limit = registry
        .config
        .max_text_bytes
        .saturating_mul(6)
        .saturating_add(256 * 1024);
    if bytes.len() > limit {
        return Err(corrupt("encoded version exceeds the read budget"));
    }
    let snapshot: PromptVersion =
        serde_json::from_slice(&bytes).map_err(|source| RegistryError::Corrupt {
            key: key.clone(),
            source,
        })?;
    if snapshot.name != *name
        || snapshot.version_id != *version
        || snapshot.text.trim().is_empty()
        || snapshot.text.len() > registry.config.max_text_bytes
        || PromptVersionId::of(&snapshot.text) != *version
        || snapshot.variables != variables_of(&snapshot.text)
    {
        return Err(corrupt(
            "name, text or variables disagree with the pinned version",
        ));
    }
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PublishRequest, RegistryConfig, adapters::memory::MemoryObjectStore};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn request(text: &str) -> PublishRequest {
        serde_json::from_value(
            json!({"name": "pinned-prompt", "text": text, "label": "production"}),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn verified_text_outlives_labels_index_eviction_and_loss_of_the_derived_head() {
        let registry = Registry::new(
            Arc::new(MemoryObjectStore::new()),
            RegistryConfig {
                max_versions_indexed: 1,
                ..RegistryConfig::default()
            },
        );
        let first = registry
            .publish(request("Answer {{ question }} dokładnie.\n"))
            .await
            .unwrap();
        let second = registry
            .publish(request("Changed prompt {{ question }}"))
            .await
            .unwrap();
        let (name, pin) = (&first.version.name, &first.version.version_id);
        assert_eq!(registry.resolve(name, None).await.unwrap(), second.version);
        assert_eq!(second.head.versions.len(), 1);
        assert_eq!(
            registry.verified_version(name, pin).await.unwrap(),
            Some(first.version.clone())
        );
        registry
            .store
            .delete(&registry.head_key(name))
            .await
            .unwrap();
        assert_eq!(
            registry.verified_version(name, pin).await.unwrap(),
            Some(first.version.clone())
        );
        registry
            .store
            .delete(&registry.version_key(name, pin))
            .await
            .unwrap();
        assert_eq!(registry.verified_version(name, pin).await.unwrap(), None);
    }

    #[tokio::test]
    async fn verified_reads_reject_tampered_content_but_do_not_hash_catalogue_metadata() {
        let registry = Registry::new(
            Arc::new(MemoryObjectStore::new()),
            RegistryConfig::default(),
        );
        let first = registry
            .publish(request("Answer {{ question }}"))
            .await
            .unwrap();
        let (name, pin) = (&first.version.name, &first.version.version_id);
        let key = registry.version_key(name, pin);
        let original = serde_json::to_value(&first.version).unwrap();
        for (field, value) in [
            ("text", json!("Forged {{ question }}")),
            ("name", json!("other-prompt")),
            ("version_id", json!("0".repeat(64))),
            ("variables", json!(["other"])),
        ] {
            let mut forged = original.clone();
            forged[field] = value;
            registry
                .store
                .put(&key, serde_json::to_vec(&forged).unwrap())
                .await
                .unwrap();
            assert!(
                matches!(
                    registry.verified_version(name, pin).await,
                    Err(RegistryError::Integrity { .. })
                ),
                "{field}"
            );
        }
        let mut metadata = original;
        metadata["notes"] = json!("catalogue change");
        metadata["model"] = json!("a hint, not verified model evidence");
        registry
            .store
            .put(&key, serde_json::to_vec(&metadata).unwrap())
            .await
            .unwrap();
        assert_eq!(
            registry
                .verified_version(name, pin)
                .await
                .unwrap()
                .unwrap()
                .text,
            first.version.text
        );
        registry
            .store
            .put(&key, b"broken json".to_vec())
            .await
            .unwrap();
        assert!(matches!(
            registry.verified_version(name, pin).await,
            Err(RegistryError::Corrupt { .. })
        ));
    }

    #[tokio::test]
    async fn verified_read_budgets_allow_json_escaping_and_reject_oversized_snapshots() {
        let registry = Registry::new(
            Arc::new(MemoryObjectStore::new()),
            RegistryConfig {
                max_text_bytes: 8,
                ..RegistryConfig::default()
            },
        );
        let first = registry
            .publish(request("a\u{0001}\u{0002}\u{0003}"))
            .await
            .unwrap();
        let (name, pin) = (&first.version.name, &first.version.version_id);
        let key = registry.version_key(name, pin);
        assert!(
            registry
                .verified_version(name, pin)
                .await
                .unwrap()
                .is_some()
        );
        let mut oversized: Value = serde_json::to_value(&first.version).unwrap();
        oversized["notes"] = json!("x".repeat(256 * 1024 + 48));
        registry
            .store
            .put(&key, serde_json::to_vec(&oversized).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            registry.verified_version(name, pin).await,
            Err(RegistryError::Integrity { .. })
        ));
        let longer = PromptVersion::new(
            name.clone(),
            "a prompt too long",
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
        registry
            .store
            .put(
                &registry.version_key(name, &longer.version_id),
                serde_json::to_vec(&longer).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            registry.verified_version(name, &longer.version_id).await,
            Err(RegistryError::Integrity { .. })
        ));
    }
}
