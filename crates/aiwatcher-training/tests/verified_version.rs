//! Verified reads preserve historical pins and detect changed identity material.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_training::{Error, Registry};
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn exact_identity_survives_index_and_run_loss_but_not_changed_content() {
    let store = Arc::new(MemoryObjectStore::new());
    let owner = Registry::new(store.clone(), "training");
    owner
        .start(
            serde_json::from_value(json!({
                "run_id": "run", "model": "model", "dataset": "data@abc"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let registered = owner
        .register_model(
            serde_json::from_value(json!({
                "name": "model", "run_id": "run", "checkpoint_uri": "s3://models/old",
                "metrics": {"test": {"accuracy": 0.5}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let pin = &registered.version.version;
    // This literal was produced by the pre-existing registration identity.
    assert_eq!(
        pin,
        "2d599a29559f8211e44efc35e63445275855be0d5a45b09c39bf51df402cd2d9"
    );
    let objects = store.list("training/").await.unwrap();
    let key = objects
        .iter()
        .find(|entry| entry.key.contains("/versions/"))
        .unwrap()
        .key
        .clone();
    for entry in &objects {
        if entry.key != key {
            store.delete(&entry.key).await.unwrap();
        }
    }
    let bytes = store.get(&key).await.unwrap().unwrap();
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        owner.verified_version("model", pin).await.unwrap().version,
        *pin
    );
    for (field, value) in [
        ("name", json!("other")),
        ("version", json!("0".repeat(64))),
        ("run_id", json!("other")),
        ("dataset", json!("other@abc")),
        ("checkpoint_uri", json!("file:///other")),
        ("metrics", json!({"test": {"accuracy": 0.6}})),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        store
            .put(&key, serde_json::to_vec(&changed).unwrap())
            .await
            .unwrap();
        assert!(
            matches!(
                owner.verified_version("model", pin).await,
                Err(Error::Corrupt { .. })
            ),
            "{field}"
        );
    }
    let mut metadata = original;
    metadata["notes"] = json!("New notes");
    metadata["framework"] = json!("metadata");
    store
        .put(&key, serde_json::to_vec(&metadata).unwrap())
        .await
        .unwrap();
    owner.verified_version("model", pin).await.unwrap();
    store.put(&key, b"invalid json".to_vec()).await.unwrap();
    assert!(matches!(
        owner.verified_version("model", pin).await,
        Err(Error::Corrupt { .. })
    ));
    store.put(&key, vec![b' '; 1024 * 1024 + 1]).await.unwrap();
    assert!(matches!(
        owner.verified_version("model", pin).await,
        Err(Error::TooLarge { .. })
    ));
    store.delete(&key).await.unwrap();
    assert!(matches!(
        owner.verified_version("model", pin).await,
        Err(Error::NotFound(_))
    ));
    for alias in ["latest", "production", "../head", &"AB".repeat(32)] {
        assert!(matches!(
            owner.verified_version("model", alias).await,
            Err(Error::Invalid(_))
        ));
    }
    assert!(matches!(
        owner.verified_version("../other", pin).await,
        Err(Error::Invalid(_))
    ));
}
