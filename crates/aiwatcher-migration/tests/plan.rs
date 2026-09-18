//! What a dry run says, and what it refuses to say.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;

use aiwatcher_core::migration::ReferenceKind;
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_migration::manifest::{BlockerKind, FamilyState, Severity};
use aiwatcher_migration::plan::{Source, plan};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use support::{contents, scope, seeded};

#[tokio::test]
async fn a_dry_run_writes_nothing_and_reaches_the_same_manifest_twice() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let before = contents(store.as_ref()).await;
    let target = scope();
    let source = Source::new("fs:/snapshot");

    let first = plan(&store, &source, target).await.unwrap();
    let second = plan(&store, &source, target).await.unwrap();

    assert_eq!(first, second, "one snapshot plans to one manifest");
    assert!(first.identity_holds().unwrap());
    assert_eq!(
        contents(store.as_ref()).await,
        before,
        "a dry run stores nothing, not even a probe"
    );
    assert!(
        first.body.objects.len() >= 12,
        "four families were seeded, {} objects planned",
        first.body.objects.len()
    );
}

#[tokio::test]
async fn the_manifest_moves_with_the_snapshot_and_with_the_configuration() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let base = plan(&store, &Source::new("fs:/snapshot"), target)
        .await
        .unwrap();

    let elsewhere = plan(&store, &Source::new("fs:/another-copy"), target)
        .await
        .unwrap();
    assert_ne!(
        base.manifest_id, elsewhere.manifest_id,
        "which store this was planned against is part of what was reviewed"
    );

    let other_project = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();
    assert_ne!(
        base.manifest_id, other_project.manifest_id,
        "the target is part of the plan"
    );

    let renamed = plan(
        &store,
        &Source::new("fs:/snapshot").with_prefix("prompts", "elsewhere"),
        target,
    )
    .await
    .unwrap();
    assert_ne!(base.manifest_id, renamed.manifest_id);
    assert!(
        matches!(renamed.body.families["prompts"].state, FamilyState::Empty),
        "a prefix nothing was written under is empty, and says so"
    );
    assert!(
        renamed
            .body
            .blockers
            .iter()
            .any(|blocker| blocker.kind == BlockerKind::UnknownPrefix
                && blocker.subject == "prompts"),
        "the real prompts are now under a prefix no family owns, which is not nothing"
    );
}

#[tokio::test]
async fn an_empty_family_a_blocked_one_and_one_with_no_adapter_read_differently() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    store
        .put("conversations/turns/one.json", b"sealed".to_vec())
        .await
        .unwrap();
    store
        .put("evaluations/index/one.json", b"{}".to_vec())
        .await
        .unwrap();
    let manifest = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();

    assert!(matches!(
        manifest.body.families["prompts"].state,
        FamilyState::Empty
    ));
    let conversations = &manifest.body.families["conversations"].state;
    assert!(
        matches!(conversations, FamilyState::Blocked { objects: 1, .. }),
        "the archive is blocked and counted, never empty: {conversations:?}"
    );
    let evaluations = &manifest.body.families["evaluations"].state;
    assert!(
        matches!(evaluations, FamilyState::Unsupported { objects: 1, .. }),
        "no adapter is not a count of zero: {evaluations:?}"
    );
    assert!(manifest.body.objects.is_empty(), "nothing is planned");
    assert!(
        !manifest.covers_every_family(),
        "a migration of this store is not a migration of the application"
    );
    for kind in [BlockerKind::BlockedFamily, BlockerKind::UnsupportedFamily] {
        assert!(
            manifest
                .body
                .blockers
                .iter()
                .any(|blocker| blocker.kind == kind),
            "{kind:?} is named"
        );
    }
}

#[tokio::test]
async fn a_foreign_object_under_a_known_prefix_stops_the_run_rather_than_being_walked_past() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    store
        .put("prompts/house.extract/notes.txt", b"a stray file".to_vec())
        .await
        .unwrap();
    let manifest = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();
    let state = &manifest.body.families["prompts"].state;
    assert!(
        matches!(state, FamilyState::Foreign { total: 1, .. }),
        "{state:?}"
    );
    let blocker = manifest
        .refuses_execution()
        .into_iter()
        .find(|blocker| blocker.kind == BlockerKind::ForeignObject)
        .expect("a foreign object refuses execution");
    assert_eq!(blocker.severity, Severity::RefusesExecution);
    assert!(
        blocker.detail.contains("not this registry's own documents"),
        "{}",
        blocker.detail
    );
}

#[tokio::test]
async fn a_document_of_the_wrong_shape_is_damaged_and_named() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    store
        .put(
            "training/runs/aaaa/record.json",
            br#"{"this":"is not a training run"}"#.to_vec(),
        )
        .await
        .unwrap();
    let manifest = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();
    let FamilyState::Damaged { key, .. } = &manifest.body.families["training"].state else {
        panic!(
            "a foreign document is damaged: {:?}",
            manifest.body.families["training"].state
        );
    };
    assert_eq!(key, "training/runs/aaaa/record.json");
    let blocker = manifest
        .refuses_execution()
        .into_iter()
        .find(|blocker| blocker.kind == BlockerKind::DamagedObject)
        .expect("a document of the wrong shape is damaged, not merely foreign");
    assert_eq!(blocker.subject, "training/runs/aaaa/record.json");
}

#[tokio::test]
async fn references_are_counted_and_named_and_never_rewritten() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let manifest = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();

    assert!(manifest.body.references.internal > 0);
    assert!(
        manifest.body.references.internal_missing.is_empty(),
        "a consistent seed has nothing dangling: {:?}",
        manifest.body.references.internal_missing
    );
    assert!(
        manifest
            .body
            .references
            .foreign
            .keys()
            .any(|owner| owner.contains("aiwatcher-evaluation")
                || owner.contains("aiwatcher-annotations")),
        "an optimisation's evaluation id and a run's dataset name their owners: {:?}",
        manifest.body.references.foreign
    );
    assert!(
        manifest
            .body
            .references
            .opaque
            .keys()
            .any(|reason| reason.contains("not analysed")),
        "query text is reported as unanalysed rather than parsed: {:?}",
        manifest.body.references.opaque
    );

    let label = manifest
        .body
        .objects
        .iter()
        .find(|object| object.source_key == "prompts/house.extract/head.json")
        .expect("the prompt head is planned");
    let production = label
        .references
        .get("/labels/production")
        .expect("a label lives only in the head, so it is named");
    assert!(matches!(production.kind, ReferenceKind::Internal { .. }));
    assert!(
        manifest.identity_holds().unwrap(),
        "the manifest is named by exactly what it says"
    );
}

#[tokio::test]
async fn a_dangling_internal_reference_is_named_and_blocks_a_cutover_without_blocking_the_copy() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    // A model head keeps its label while the version it names disappears —
    // exactly what a half-restored bucket looks like.
    let version = store
        .list("training/models/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.contains("/versions/"))
        .expect("a model version was seeded");
    store.delete(&version.key).await.unwrap();

    let manifest = plan(&store, &Source::new("fs:/snapshot"), scope())
        .await
        .unwrap();
    let missing = &manifest.body.references.internal_missing;
    assert!(!missing.is_empty(), "the head still names it");
    assert!(missing.iter().all(|entry| entry.names == version.key));
    assert!(
        manifest.refuses_execution().is_empty(),
        "a source that was already inconsistent is still copyable, faithfully"
    );
    assert!(
        manifest
            .body
            .blockers
            .iter()
            .any(|blocker| blocker.kind == BlockerKind::DanglingReference
                && blocker.severity == Severity::RefusesCutover)
    );
}

#[tokio::test]
async fn already_scoped_keys_are_never_a_source() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let first = plan(&store, &Source::new("fs:/snapshot"), target)
        .await
        .unwrap();
    // Somebody's earlier copy, sitting in the destination.
    for object in &first.body.objects {
        let bytes = store.get(&object.source_key).await.unwrap().unwrap();
        store.put(&object.target_key, bytes).await.unwrap();
    }
    let second = plan(&store, &Source::new("fs:/snapshot"), target)
        .await
        .unwrap();
    assert_eq!(
        first.manifest_id, second.manifest_id,
        "a half-finished copy must not change the plan, or no resume could bind to it"
    );
    assert_eq!(first.body.objects, second.body.objects);
}
