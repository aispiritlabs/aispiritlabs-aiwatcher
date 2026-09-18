//! What an execution writes, what it refuses, and what it does after a crash.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;

use aiwatcher_core::migration::sha256_hex;
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_iam::{
    Command, GrantWindow, Grantee, IamStore, Principal, ProjectRole, ProjectScope,
};
use aiwatcher_migration::MigrationError;
use aiwatcher_migration::authority::{
    Admission, Fixture, FixtureAuthority, IamAuthority, Offline, TargetAuthority,
};
use aiwatcher_migration::execute::{Destination, Options, execute, survey};
use aiwatcher_migration::manifest::{BlockerKind, Checkpoint, IamCheck, Manifest, Publication};
use aiwatcher_migration::plan::{Source, plan};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use support::{Faulty, contents, scope, seeded};

/// A temporary directory that goes away with the test.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aiwatcher-migration-{name}-{}",
            aiwatcher_iam::ProjectId::new().0
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn checkpoint(&self) -> std::path::PathBuf {
        self.0.join("checkpoint.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn admitting(target: ProjectScope) -> FixtureAuthority {
    let principal = Principal::new("test", "operator").unwrap();
    FixtureAuthority::new(
        Fixture {
            admits: vec![Admission {
                organization: target.organization.0.to_string(),
                project: target.project.0.to_string(),
                provider: "test".to_owned(),
                subject: "operator".to_owned(),
                role: ProjectRole::Admin,
            }],
        },
        principal,
        0,
    )
}

fn destination(scratch: &Scratch) -> Destination {
    Destination {
        label: "fs:/snapshot".to_owned(),
        checkpoint: scratch.checkpoint(),
    }
}

async fn planned(store: &Arc<dyn ObjectStore>, target: ProjectScope) -> Manifest {
    plan(store, &Source::new("fs:/snapshot"), target)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_copy_keeps_every_byte_and_reopens_under_the_project() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("bytes");
    let manifest = planned(&store, target).await;
    let before = contents(store.as_ref()).await;

    let receipt = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();

    assert!(receipt.complete);
    assert_eq!(receipt.written, manifest.body.objects.len() as u64);
    assert_eq!(receipt.publication, Publication::CreateOnly);
    assert!(
        !receipt.cutover_ready,
        "families with no adapter are still there, so this is not a cutover"
    );

    for object in &manifest.body.objects {
        let source = before.get(&object.source_key).expect("a planned source");
        let copied = store.get(&object.target_key).await.unwrap().unwrap();
        assert_eq!(&copied, source, "{} was rewritten", object.source_key);
        assert_eq!(sha256_hex(&copied), object.sha256);
        assert_eq!(
            store.get(&object.source_key).await.unwrap().as_ref(),
            Some(source),
            "the source is left exactly as it was"
        );
    }

    // The registries read their own objects back out of the project, with the
    // same version ids they were published under.
    let prompts = aiwatcher_prompts::Registry::new(
        store.clone(),
        aiwatcher_prompts::RegistryConfig::default(),
    )
    .for_project(target)
    .unwrap();
    let head = prompts
        .head(&aiwatcher_core::prompts::PromptName::parse("house.extract").unwrap())
        .await
        .unwrap()
        .expect("the prompt is in the project");
    let legacy = aiwatcher_prompts::Registry::new(
        store.clone(),
        aiwatcher_prompts::RegistryConfig::default(),
    );
    let original = legacy
        .head(&aiwatcher_core::prompts::PromptName::parse("house.extract").unwrap())
        .await
        .unwrap()
        .expect("and still in the legacy registry");
    assert_eq!(
        head.labels, original.labels,
        "labels survive; they live only in the head"
    );
    assert_eq!(head.versions, original.versions);
    let pinned = head.labels.get("production").expect("a production label");
    assert!(
        prompts
            .verified_version(&head.name, pinned)
            .await
            .unwrap()
            .is_some(),
        "the version the label names is readable in the project, under the same id"
    );

    let training = aiwatcher_training::Registry::new(store.clone(), "training")
        .for_project(target)
        .unwrap();
    let model = training.model("walls", None).await.unwrap();
    assert_eq!(model.head.name, "walls");
}

#[tokio::test]
async fn running_it_again_writes_nothing_and_is_not_a_conflict() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let first = Scratch::new("idempotent-a");
    let manifest = planned(&store, target).await;
    execute(
        &store,
        &manifest,
        &destination(&first),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    let after_first = contents(store.as_ref()).await;

    // A second run with no memory of the first: every object is already there,
    // byte for byte, which is the safe repetition this design is built on.
    let second = Scratch::new("idempotent-b");
    let receipt = execute(
        &store,
        &manifest,
        &destination(&second),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    assert_eq!(receipt.written, 0);
    assert_eq!(
        receipt.already_identical,
        manifest.body.objects.len() as u64
    );
    assert!(receipt.complete);
    assert_eq!(contents(store.as_ref()).await, after_first);
}

#[tokio::test]
async fn a_target_holding_different_bytes_stops_the_run_and_is_not_overwritten() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("conflict");
    let manifest = planned(&store, target).await;
    let occupied = &manifest.body.objects[0].target_key;
    store
        .put(occupied, b"somebody else's bytes".to_vec())
        .await
        .unwrap();

    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("a conflict stops the run");
    assert!(matches!(error, MigrationError::Conflict { .. }), "{error}");
    assert_eq!(
        store.get(occupied).await.unwrap().unwrap(),
        b"somebody else's bytes",
        "nothing is overwritten"
    );

    // And the survey said so before anybody ran it.
    let looked = survey(&store, &manifest, &admitting(target)).await.unwrap();
    assert_eq!(looked.conflicts.len(), 1);
    assert_eq!(looked.conflicts[0].target_key, *occupied);
}

#[tokio::test]
async fn a_crash_before_a_write_after_one_and_after_the_checkpoint_all_resume_exactly_once() {
    let target = scope();
    let scratch = Scratch::new("resume");
    let memory = seeded().await;
    let reference: Arc<dyn ObjectStore> = memory.clone();
    let manifest = planned(&reference, target).await;
    let total = manifest.body.objects.len();
    assert!(total > 4);

    // A store that refuses its fourth write outright: three objects are
    // copied, the fourth is not, and the checkpoint knows about three.
    let faulty: Arc<dyn ObjectStore> = Arc::new(Faulty::new(memory.clone()).failing_write(4));
    let error = execute(
        &faulty,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("the write failed");
    assert!(matches!(error, MigrationError::Store(_)), "{error}");

    let checkpoint: Checkpoint =
        serde_json::from_slice(&std::fs::read(scratch.checkpoint()).unwrap()).unwrap();
    assert_eq!(
        checkpoint.done.len(),
        3,
        "only what was read back is recorded"
    );
    assert!(checkpoint.binds_to(&manifest, "fs:/snapshot"));

    // The same run, resumed on a healthy store. Nothing is copied twice and
    // nothing is missed.
    let receipt = execute(
        &reference,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    assert_eq!(receipt.resumed, 3);
    assert_eq!(receipt.written as usize, total - 3);
    assert_eq!(receipt.already_identical, 0);
    assert!(receipt.complete);
    for object in &manifest.body.objects {
        assert!(store_has(&reference, &object.target_key, &object.sha256).await);
    }

    // A crash *after* a write and before its checkpoint: drop the last entry
    // and run again. The object is already identical, so the repetition is a
    // no-op rather than a conflict.
    let mut lagging: Checkpoint =
        serde_json::from_slice(&std::fs::read(scratch.checkpoint()).unwrap()).unwrap();
    lagging.done.truncate(lagging.done.len() - 2);
    std::fs::write(
        scratch.checkpoint(),
        serde_json::to_vec_pretty(&lagging).unwrap(),
    )
    .unwrap();
    let again = execute(
        &reference,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    assert_eq!(again.written, 0);
    assert_eq!(
        again.already_identical, 2,
        "a lagging checkpoint costs a re-read"
    );
    assert!(again.complete);
}

async fn store_has(store: &Arc<dyn ObjectStore>, key: &str, sha256: &str) -> bool {
    store
        .get(key)
        .await
        .unwrap()
        .is_some_and(|bytes| sha256_hex(&bytes) == sha256)
}

#[tokio::test]
async fn a_write_that_reported_success_and_stored_something_else_stops_the_run() {
    let memory = seeded().await;
    let target = scope();
    let scratch = Scratch::new("readback");
    let manifest = planned(&(memory.clone() as Arc<dyn ObjectStore>), target).await;
    let store: Arc<dyn ObjectStore> = Arc::new(Faulty::new(memory).corrupting_write(2));

    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("the read-back caught it");
    assert!(matches!(error, MigrationError::ReadBack { .. }), "{error}");
    let checkpoint: Checkpoint =
        serde_json::from_slice(&std::fs::read(scratch.checkpoint()).unwrap()).unwrap();
    assert_eq!(
        checkpoint.done.len(),
        1,
        "a checkpoint is written after the read-back, never before"
    );
}

#[tokio::test]
async fn a_store_that_cannot_publish_create_only_is_refused_unless_somebody_states_otherwise() {
    let memory = seeded().await;
    let target = scope();
    let manifest = planned(&(memory.clone() as Arc<dyn ObjectStore>), target).await;
    let store: Arc<dyn ObjectStore> = Arc::new(Faulty::new(memory).without_atomic_create());

    let refused = Scratch::new("no-create");
    let error = execute(
        &store,
        &manifest,
        &destination(&refused),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("read-then-write is not a silent fallback");
    assert!(
        matches!(error, MigrationError::NoAtomicCreate { .. }),
        "{error}"
    );

    let acknowledged = Scratch::new("exclusive");
    let receipt = execute(
        &store,
        &manifest,
        &destination(&acknowledged),
        &admitting(target),
        Options {
            exclusive_access: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.publication, Publication::ExclusiveAccess);
    assert!(receipt.complete);
}

#[tokio::test]
async fn a_source_that_moved_stops_the_run_before_anything_is_written() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("moved");
    let manifest = planned(&store, target).await;

    let edited = manifest
        .body
        .objects
        .iter()
        .find(|object| object.category == "versions")
        .expect("a prompt version")
        .source_key
        .clone();
    let original = store.get(&edited).await.unwrap().unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["notes"] = serde_json::Value::String("edited under the migration".to_owned());
    store
        .put(&edited, serde_json::to_vec(&changed).unwrap())
        .await
        .unwrap();

    let before = contents(store.as_ref()).await;
    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("the snapshot moved");
    assert!(
        matches!(error, MigrationError::SnapshotChanged(_)),
        "{error}"
    );
    assert_eq!(
        contents(store.as_ref()).await,
        before,
        "nothing was written"
    );
    assert!(!scratch.checkpoint().exists());
}

#[tokio::test]
async fn an_object_that_disappeared_stops_the_run() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("gone");
    let manifest = planned(&store, target).await;
    store
        .delete(&manifest.body.objects[0].source_key)
        .await
        .unwrap();
    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("an object is gone");
    assert!(
        matches!(error, MigrationError::SnapshotChanged(_)),
        "{error}"
    );
}

#[tokio::test]
async fn an_edited_manifest_is_refused_before_anything_is_read() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("edited");
    let manifest = planned(&store, target).await;

    for edit in [
        |manifest: &mut Manifest| {
            manifest.body.objects[0].target_key = "somewhere/else.json".to_owned();
        },
        |manifest: &mut Manifest| {
            manifest.body.objects[0].target_key =
                format!("{}/../escape.json", manifest.body.objects[0].target_key);
        },
        |manifest: &mut Manifest| manifest.body.blockers.clear(),
    ] {
        let mut edited = manifest.clone();
        edit(&mut edited);
        let error = execute(
            &store,
            &edited,
            &destination(&scratch),
            &admitting(target),
            Options::default(),
        )
        .await
        .expect_err("an edited manifest is not the one that was reviewed");
        assert!(matches!(error, MigrationError::ManifestEdited), "{error}");
    }

    // Even re-sealed, so the id matches its own body again, an edited mapping
    // is refused: the executor plans again and compares.
    let mut forged = manifest.clone();
    forged.body.objects[0].target_key = "somewhere/else.json".to_owned();
    let resealed = Manifest::seal(forged.body).unwrap();
    let error = execute(
        &store,
        &resealed,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("re-sealing a forged mapping does not make it the plan");
    assert!(
        matches!(error, MigrationError::SnapshotChanged(_)),
        "{error}"
    );
    assert!(!scratch.checkpoint().exists());
}

#[tokio::test]
async fn a_checkpoint_from_another_plan_target_or_store_is_refused() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("foreign-checkpoint");
    let manifest = planned(&store, target).await;
    execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();

    let other = scope();
    let other_manifest = planned(&store, other).await;
    let error = execute(
        &store,
        &other_manifest,
        &destination(&scratch),
        &admitting(other),
        Options::default(),
    )
    .await
    .expect_err("progress for one plan is not progress for another");
    assert!(
        matches!(error, MigrationError::CheckpointMismatch { .. }),
        "{error}"
    );

    let elsewhere = Destination {
        label: "fs:/a-different-bucket".to_owned(),
        checkpoint: scratch.checkpoint(),
    };
    let error = execute(
        &store,
        &manifest,
        &elsewhere,
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("progress against one store is not progress against another");
    assert!(
        matches!(error, MigrationError::CheckpointMismatch { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_damaged_or_foreign_source_refuses_the_write_entirely() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("damaged");
    store
        .put("prompts/house.extract/stray.txt", b"not ours".to_vec())
        .await
        .unwrap();
    let manifest = planned(&store, target).await;
    let before = contents(store.as_ref()).await;
    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .expect_err("a schema nobody recognises stops the run");
    assert!(matches!(error, MigrationError::Blocked(_)), "{error}");
    assert_eq!(contents(store.as_ref()).await, before);
}

#[tokio::test]
async fn an_execution_needs_iam_and_a_dry_run_may_say_it_reached_none() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("offline");
    let manifest = planned(&store, target).await;

    let offline = survey(&store, &manifest, &Offline::default())
        .await
        .unwrap();
    assert!(matches!(offline.iam, IamCheck::NotChecked { .. }));
    assert!(
        offline
            .blockers
            .iter()
            .any(|blocker| blocker.kind == BlockerKind::NonAuthoritativeIam),
        "a dry run that confirmed nothing says so"
    );

    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &Offline::default(),
        Options::default(),
    )
    .await
    .expect_err("a write needs the target confirmed");
    assert!(matches!(error, MigrationError::Authority(_)), "{error}");
    assert_eq!(
        store
            .get(&manifest.body.objects[0].target_key)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_target_iam_does_not_admit_refuses_and_a_rehearsal_says_it_is_one() {
    let memory = seeded().await;
    let store: Arc<dyn ObjectStore> = memory.clone();
    let target = scope();
    let scratch = Scratch::new("iam");
    let manifest = planned(&store, target).await;

    // The fixture admits a different project entirely.
    let elsewhere = admitting(scope());
    let error = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &elsewhere,
        Options::default(),
    )
    .await
    .expect_err("nothing confirmed this organization and project");
    assert!(matches!(error, MigrationError::Authority(_)), "{error}");

    let receipt = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    let IamCheck::Verified { authoritative, .. } = receipt.iam else {
        panic!("a write records who admitted it");
    };
    assert!(!authoritative, "a fixture is not the deployment's IAM");
    assert!(
        receipt
            .blockers
            .iter()
            .any(|blocker| blocker.kind == BlockerKind::NonAuthoritativeIam),
        "and the receipt says so for as long as it is kept"
    );
    assert!(!receipt.cutover_ready);
}

#[tokio::test]
async fn the_control_plane_itself_answers_for_a_real_organization_and_project() {
    let iam = Arc::new(aiwatcher_iam::memory::MemoryIamStore::default());
    let owner = Principal::new("oidc", "operator").unwrap();
    let organization = iam.create_organization(&owner, "Acme").await.unwrap();
    let aiwatcher_iam::Change::ProjectCreated(project) = iam
        .apply(
            organization.id,
            &owner,
            Command::CreateProject {
                name: "plans".to_owned(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("creating a project answers with the project");
    };

    let store: Arc<dyn IamStore> = iam.clone();
    let authority = IamAuthority::new(store.clone(), owner.clone(), "memory", false);
    let check = authority.admit(project.scope).await.unwrap();
    let IamCheck::Verified { role, .. } = check else {
        panic!("a live admin grant is admitted");
    };
    assert_eq!(role, ProjectRole::Admin, "a creator gets an explicit grant");

    // Somebody with no grant is refused, and the refusal does not say whether
    // the project exists.
    let stranger = Principal::new("oidc", "somebody-else").unwrap();
    let refused = IamAuthority::new(store.clone(), stranger.clone(), "memory", false)
        .admit(project.scope)
        .await
        .expect_err("no grant, no migration");
    assert!(matches!(refused, MigrationError::Authority(_)));

    // An editor grant is not enough: this writes a project's whole registry.
    // A grant names a member, so the member comes first — and membership alone
    // still confers no project access, which is what the read above proved.
    iam.apply(
        organization.id,
        &owner,
        Command::SetMember {
            principal: stranger.clone(),
            role: aiwatcher_iam::OrganizationRole::Member,
        },
    )
    .await
    .unwrap();
    iam.apply(
        organization.id,
        &owner,
        Command::Grant {
            project: project.scope.project,
            grantee: Grantee::User(stranger.clone()),
            role: ProjectRole::Editor,
            window: GrantWindow::permanent(0),
        },
    )
    .await
    .unwrap();
    let refused = IamAuthority::new(store, stranger, "memory", false)
        .admit(project.scope)
        .await
        .expect_err("an editor may not land somebody's whole registry");
    assert!(matches!(refused, MigrationError::Authority(_)));

    // And a project nobody created is refused, whoever asks.
    let nowhere = IamAuthority::new(iam, owner, "memory", false)
        .admit(ProjectScope {
            organization: organization.id,
            project: aiwatcher_iam::ProjectId::new(),
        })
        .await
        .expect_err("there is no such project");
    assert!(matches!(nowhere, MigrationError::Authority(_)));
}

#[tokio::test]
async fn an_empty_store_plans_to_nothing_and_copies_nothing() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let target = scope();
    let scratch = Scratch::new("empty");
    let manifest = planned(&store, target).await;
    assert!(manifest.body.objects.is_empty());
    let receipt = execute(
        &store,
        &manifest,
        &destination(&scratch),
        &admitting(target),
        Options::default(),
    )
    .await
    .unwrap();
    assert_eq!(receipt.written, 0);
    assert!(receipt.complete, "there was nothing to copy");
    assert!(
        !receipt.cutover_ready,
        "every family with no adapter still stands between this and a cutover"
    );
    assert!(store.list("").await.unwrap().is_empty());
}
