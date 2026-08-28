// `clippy.toml`'s `allow-expect-in-tests` only reaches `#[cfg(test)]` modules,
// not files under `tests/`. An assertion that panics is the point here.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The registry against a real RustFS.
//!
//! Everything in `src` is tested against the in-memory store, which proves the
//! rules and proves nothing about the wire. The signer is the reason this file
//! exists: a canonical request that is wrong in any of five places comes back
//! as `403 SignatureDoesNotMatch` with no indication of which, and no unit
//! test can tell a self-consistent signer from a correct one. Only a server
//! that also implements SigV4 can.
//!
//! Ignored by default and run by `just test-rustfs`, which starts the
//! container first — the same arrangement as the Laser tests, for the same
//! reason: `just check` must stay runnable with no daemons.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_core::prompts::{
    ObjectStore, OptimizationOutcome, PRODUCTION_LABEL, PromptName, PromptVersionId, Score,
};
use aiwatcher_prompts::adapters::s3::{S3Config, S3ObjectStore};
use aiwatcher_prompts::sigv4::Credentials;
use aiwatcher_prompts::{
    OptimizationRequest, PromptFilter, PublishRequest, Registry, RegistryConfig,
};

const BASELINE: &str = "Describe the floor plan on {{ page }} in {{ language }}.";
const CANDIDATE: &str = "Read {{ page }} closely; describe every room in {{ language }}.";

fn endpoint() -> String {
    std::env::var("AIWATCHER_PROMPT_S3_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:9010".to_owned())
}

/// A bucket per test, so the tests are order-independent and can run together.
async fn store(bucket: &str) -> S3ObjectStore {
    S3ObjectStore::connect(S3Config {
        endpoint: endpoint(),
        bucket: bucket.to_owned(),
        credentials: Credentials {
            access_key_id: std::env::var("AIWATCHER_PROMPT_S3_ACCESS_KEY")
                .unwrap_or_else(|_| "rustfsadmin".to_owned()),
            secret_access_key: std::env::var("AIWATCHER_PROMPT_S3_SECRET_KEY")
                .unwrap_or_else(|_| "rustfsadmin".to_owned()),
            session_token: None,
            region: "us-east-1".to_owned(),
        },
        timeout: Duration::from_secs(10),
        create_bucket: true,
    })
    .await
    .expect("rustfs is reachable — run `just rustfs-up`")
}

#[tokio::test]
#[ignore = "needs a RustFS; run `just test-rustfs`"]
async fn a_signed_request_is_accepted_and_objects_round_trip() {
    let store = store("aiwatcher-test-roundtrip").await;

    // The whole point of the file: if the signature were wrong, this is a 403.
    store
        .put("prompts/a/head.json", b"{\"ok\":true}".to_vec())
        .await
        .expect("put is signed correctly");
    assert_eq!(
        store.get("prompts/a/head.json").await.unwrap(),
        Some(b"{\"ok\":true}".to_vec())
    );

    // A key that was never written reads as absent, not as an error — the
    // registry asks for a head that may not exist on every publish.
    assert_eq!(store.get("prompts/a/missing.json").await.unwrap(), None);

    let keys: Vec<String> = store
        .list("prompts/a/")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    assert_eq!(keys, ["prompts/a/head.json"]);

    store.delete("prompts/a/head.json").await.unwrap();
    assert_eq!(store.get("prompts/a/head.json").await.unwrap(), None);
    // And deleting it again is not an error.
    store.delete("prompts/a/head.json").await.unwrap();
}

#[tokio::test]
#[ignore = "needs a RustFS; run `just test-rustfs`"]
async fn a_listing_that_needs_several_pages_returns_every_key() {
    // 1000 is the page size the adapter asks for, so this is the boundary
    // where a missing continuation token silently truncates the version list.
    let store = store("aiwatcher-test-paging").await;
    for index in 0..1_050 {
        store
            .put(&format!("paged/{index:05}.json"), b"{}".to_vec())
            .await
            .unwrap();
    }
    assert_eq!(store.list("paged/").await.unwrap().len(), 1_050);
}

#[tokio::test]
#[ignore = "needs a RustFS; run `just test-rustfs`"]
async fn a_size_and_a_timestamp_survive_the_listing() {
    let store = store("aiwatcher-test-metadata").await;
    store.put("sized/one.json", vec![b'x'; 41]).await.unwrap();
    let entry = store
        .list("sized/")
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("one object");
    assert_eq!(entry.size, 41);
    assert!(
        entry.last_modified.is_some(),
        "RustFS reports LastModified and the adapter has to parse it"
    );
}

#[tokio::test]
#[ignore = "needs a RustFS; run `just test-rustfs`"]
async fn the_registry_runs_against_a_bucket_exactly_as_it_does_in_memory() {
    let registry = Registry::new(
        Arc::new(store("aiwatcher-test-registry").await),
        RegistryConfig::default(),
    );
    let name = PromptName::parse("planner.floor-plan").unwrap();

    let published = registry
        .publish(PublishRequest {
            name: name.clone(),
            text: BASELINE.to_owned(),
            author: Some("integration".to_owned()),
            notes: None,
            model: Some("qwen/qwen3-vl-235b".to_owned()),
            parent: None,
            metadata: std::collections::BTreeMap::new(),
            description: Some("Floor plan extraction".to_owned()),
            tags: Some(vec!["planner".to_owned()]),
            label: Some(PRODUCTION_LABEL.to_owned()),
        })
        .await
        .expect("published");
    assert_eq!(published.version.version_id, PromptVersionId::of(BASELINE));
    assert_eq!(published.version.variables, vec!["language", "page"]);

    let record = registry
        .record_optimization(
            &name,
            OptimizationRequest {
                optimization_id: None,
                algorithm: "deepeval/SIMBA".to_owned(),
                baseline: PromptVersionId::of(BASELINE),
                candidate_text: CANDIDATE.to_owned(),
                primary_metric: "mean_score".to_owned(),
                dev: vec![Score {
                    metric: "mean_score".to_owned(),
                    baseline: Some(0.61),
                    candidate: Some(0.79),
                }],
                test: vec![Score {
                    metric: "mean_score".to_owned(),
                    baseline: Some(0.60),
                    candidate: Some(0.67),
                }],
                dataset: Some("catalog@1".to_owned()),
                evaluation_id: Some("eval-integration".to_owned()),
                started_at: None,
                duration_ms: Some(1_800_000),
                iterations: Some(8),
                report: Some(serde_json::json!({ "accepted_iterations": 3 })),
                promote: true,
            },
        )
        .await
        .expect("recorded");
    assert_eq!(record.outcome, OptimizationOutcome::Admitted);
    assert_eq!(record.reason, None);

    // Read back through fresh requests: everything above went over the wire.
    let head = registry.head(&name).await.unwrap().expect("stored");
    assert_eq!(head.versions.len(), 2);
    assert_eq!(head.optimizations.len(), 1);
    assert_eq!(
        head.labels.get(PRODUCTION_LABEL),
        Some(&PromptVersionId::of(CANDIDATE)),
        "promote:true on an admitted candidate moves production"
    );
    assert_eq!(registry.resolve(&name, None).await.unwrap().text, CANDIDATE);

    let stored = registry
        .optimization(&name, &record.optimization_id)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(
        stored.report,
        Some(serde_json::json!({ "accepted_iterations": 3 }))
    );

    let page = registry.list(&PromptFilter::default()).await.unwrap();
    assert!(
        page.prompts.iter().any(|summary| summary.name == name),
        "the prompt is listed by walking the bucket"
    );

    // And the index can be re-derived from the objects alone.
    let rebuilt = registry.rebuild(&name).await.unwrap();
    assert_eq!(rebuilt.versions.len(), 2);
    assert_eq!(rebuilt.optimizations.len(), 1);
    assert_eq!(
        rebuilt.labels.get(PRODUCTION_LABEL),
        Some(&PromptVersionId::of(CANDIDATE))
    );
}

#[tokio::test]
#[ignore = "needs a RustFS; run `just test-rustfs`"]
async fn wrong_credentials_are_refused_without_a_retry_loop() {
    // The classification matters: a signature the store rejects will be
    // rejected identically forever, and treating it as retryable spins.
    let store = S3ObjectStore::connect(S3Config {
        endpoint: endpoint(),
        bucket: "aiwatcher-test-roundtrip".to_owned(),
        credentials: Credentials {
            access_key_id: "rustfsadmin".to_owned(),
            secret_access_key: "definitely-not-the-secret".to_owned(),
            session_token: None,
            region: "us-east-1".to_owned(),
        },
        timeout: Duration::from_secs(10),
        create_bucket: false,
    })
    .await
    .expect("connecting does not authenticate");

    let error = store
        .put("prompts/a/head.json", b"{}".to_vec())
        .await
        .expect_err("refused");
    assert!(!error.is_retryable(), "{error}");
}
