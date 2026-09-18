//! Storage boundary only: nothing here opens a project execution, claims an
//! attempt, checks a grant or wires a scoped catalog to a reactor.
//!
//! What it does prove is the property the boundary exists for: identical bytes,
//! an identical cache key and an identical execution id in two projects of one
//! organization, in two organizations, and in the deployment-wide namespace,
//! share no manifest, no provenance, no cache answer and no invalidation — and
//! a record swapped underneath any of them answers with a refusal rather than
//! with somebody else's data.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::Mutex;

use aiwatcher_core::ports::PortResult;
use aiwatcher_core::storage::{ObjectEntry, ObjectStore};
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_execution::artifact::layout;
use aiwatcher_execution::{
    ArtifactCatalog, CacheEntry, CatalogedArtifact, ExecutionId, ObjectArtifactCatalog, Provenance,
    StoreError,
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use async_trait::async_trait;
use time::{Duration, OffsetDateTime};

const EPOCH: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

/// The one execution id every namespace in these tests uses.
const RUN: &str = "execution-1";

/// The one cache key every namespace in these tests uses.
const KEY: &str = "step/read/plan-1";

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

/// The same bytes in every namespace: one content address, many keys.
fn address(seed: &str) -> String {
    aiwatcher_jobs::digest(seed.as_bytes())
}

fn reference(prefix: &str, kind: ArtifactKind, digest: &str) -> ArtifactRef {
    ArtifactRef::new("rows", layout::data_uri(prefix, kind, digest), digest).of_kind(kind)
}

fn made_by(attempt: u32) -> Provenance {
    Provenance {
        execution_id: ExecutionId::new(RUN),
        step_id: "read".to_owned(),
        attempt,
    }
}

fn cataloged(artifact: ArtifactRef, attempt: u32, inputs: Vec<String>) -> CatalogedArtifact {
    CatalogedArtifact {
        artifact,
        produced_by: Some(made_by(attempt)),
        inputs,
        created_at: EPOCH,
    }
}

fn entry(artifacts: Vec<ArtifactRef>, expires_at: Option<OffsetDateTime>) -> CacheEntry {
    CacheEntry {
        cache_key: KEY.to_owned(),
        artifacts,
        created_at: EPOCH,
        expires_at,
        invalidated_at: None,
    }
}

fn refused(error: &StoreError) -> bool {
    matches!(error, StoreError::OutOfScope(_))
}

/// A directory that goes away with the test.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("aiwatcher-artifact-scope-{}", ProjectId::new().0)))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One namespace under test, and the prefix its keys are spelled with.
struct Namespace {
    what: &'static str,
    prefix: String,
    catalog: ObjectArtifactCatalog,
}

fn namespaces(
    store: &Arc<dyn ObjectStore>,
    scopes: &[(&'static str, Option<ProjectScope>)],
) -> Vec<Namespace> {
    let root = ObjectArtifactCatalog::new(Arc::clone(store));
    scopes
        .iter()
        .map(|(what, scope)| Namespace {
            what,
            prefix: layout::prefix(*scope),
            catalog: match scope {
                None => root.clone(),
                Some(scope) => root.for_project(*scope).expect("a first binding"),
            },
        })
        .collect()
}

#[tokio::test]
async fn one_digest_one_key_and_one_run_id_are_four_separate_catalogs_across_a_reopen() {
    let scratch = Scratch::new();
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let mine = scope();
    let sibling = ProjectScope {
        project: ProjectId::new(),
        ..mine
    };
    let elsewhere = ProjectScope {
        organization: OrganizationId::new(),
        ..mine
    };
    let layout_of = [
        ("the deployment", None),
        ("my project", Some(mine)),
        ("a sibling project", Some(sibling)),
        ("another organization", Some(elsewhere)),
    ];
    let spaces = namespaces(&store, &layout_of);
    let rows = address("the same table");
    let source = address("what it was read from");

    for space in &spaces {
        let recorded = space
            .catalog
            .record(cataloged(
                reference(&space.prefix, ArtifactKind::Rows, &rows),
                1,
                vec![source.clone()],
            ))
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", space.what));
        assert_eq!(
            recorded.artifact.digest, rows,
            "{}: the scope is outside the content address",
            space.what
        );
        space
            .catalog
            .remember(entry(vec![recorded.artifact.clone()], None))
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", space.what));
    }

    // Every namespace answers with its own URI for one digest, one cache key
    // and one execution id.
    let mut uris = Vec::new();
    for space in &spaces {
        let described = space
            .catalog
            .by_digest(&rows, ArtifactKind::Rows)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{}: its own manifest", space.what));
        assert_eq!(described.inputs, vec![source.clone()]);
        let hit = space
            .catalog
            .cached(KEY, EPOCH)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{}: its own cache entry", space.what));
        assert_eq!(hit.artifacts, vec![described.artifact.clone()]);
        let produced = space
            .catalog
            .produced_by(&ExecutionId::new(RUN))
            .await
            .unwrap();
        assert_eq!(produced, vec![described.clone()], "{}", space.what);
        uris.push(described.artifact.uri);
    }
    let unique: std::collections::BTreeSet<_> = uris.iter().collect();
    assert_eq!(
        unique.len(),
        uris.len(),
        "one digest, four pointers: {uris:?}"
    );

    // Invalidating one leaves the others answering, and leaves every manifest
    // alone — an execution that used those artifacts is a record of what
    // happened.
    spaces[1].catalog.invalidate(KEY, EPOCH).await.unwrap();
    for (index, space) in spaces.iter().enumerate() {
        assert_eq!(
            space.catalog.cached(KEY, EPOCH).await.unwrap().is_none(),
            index == 1,
            "{}: invalidation is not shared",
            space.what
        );
        assert!(
            space
                .catalog
                .by_digest(&rows, ArtifactKind::Rows)
                .await
                .unwrap()
                .is_some(),
            "{}: invalidation marks an entry and deletes nothing",
            space.what
        );
    }

    // And all of it survives a reopen of the same directory.
    drop(spaces);
    drop(store);
    let reopened: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&scratch.0).await.unwrap());
    let spaces = namespaces(&reopened, &layout_of);
    for (index, space) in spaces.iter().enumerate() {
        assert_eq!(
            space
                .catalog
                .by_digest(&rows, ArtifactKind::Rows)
                .await
                .unwrap()
                .expect("its manifest")
                .artifact
                .uri,
            uris[index],
            "{}",
            space.what
        );
        assert_eq!(
            space.catalog.cached(KEY, EPOCH).await.unwrap().is_none(),
            index == 1,
            "{}",
            space.what
        );
    }
}

#[tokio::test]
async fn a_bound_catalog_answers_for_no_other_namespace_and_none_answers_for_it() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let mine = scope();
    let spaces = namespaces(
        &store,
        &[("the deployment", None), ("my project", Some(mine))],
    );
    let digest = address("only the deployment holds this");
    spaces[0]
        .catalog
        .record(cataloged(
            reference(&spaces[0].prefix, ArtifactKind::Rows, &digest),
            1,
            Vec::new(),
        ))
        .await
        .unwrap();
    spaces[0]
        .catalog
        .remember(entry(
            vec![reference(&spaces[0].prefix, ArtifactKind::Rows, &digest)],
            None,
        ))
        .await
        .unwrap();

    // No fallback in either direction: a project does not read what the
    // deployment holds, and the deployment does not read what a project does.
    let project = &spaces[1].catalog;
    assert!(
        project
            .by_digest(&digest, ArtifactKind::Rows)
            .await
            .unwrap()
            .is_none()
    );
    assert!(project.cached(KEY, EPOCH).await.unwrap().is_none());
    assert!(
        project
            .produced_by(&ExecutionId::new(RUN))
            .await
            .unwrap()
            .is_empty()
    );
    project.invalidate(KEY, EPOCH).await.unwrap();
    assert!(
        spaces[0]
            .catalog
            .cached(KEY, EPOCH)
            .await
            .unwrap()
            .is_some(),
        "a project invalidating its own missing key touches nothing else"
    );
}

#[tokio::test]
async fn a_bound_catalog_cannot_be_aimed_at_a_second_project() {
    let root = ObjectArtifactCatalog::new(Arc::new(MemoryObjectStore::new()));
    let mine = scope();
    assert_eq!(root.project_scope(), None);
    let project = root.for_project(mine).unwrap();
    assert_eq!(project.project_scope(), Some(mine));
    assert_eq!(
        project.for_project(mine).unwrap().project_scope(),
        Some(mine),
        "binding to the scope already held is the same catalog"
    );
    for other in [
        scope(),
        ProjectScope {
            project: ProjectId::new(),
            ..mine
        },
        ProjectScope {
            organization: OrganizationId::new(),
            ..mine
        },
    ] {
        let error = project.for_project(other).expect_err("a rebinding");
        assert!(refused(&error), "{error}");
        assert!(error.says_the_same_next_time());
    }
}

/// An object store that records what it was asked to do, and can be told to
/// answer a listing with a key from outside the prefix.
#[derive(Debug, Default)]
struct Spy {
    inner: MemoryObjectStore,
    touched: Mutex<Vec<(&'static str, String)>>,
    smuggled: Mutex<Option<ObjectEntry>>,
}

impl Spy {
    fn since(&self) -> usize {
        self.touched.lock().unwrap().len()
    }
    fn did(&self, from: usize) -> Vec<(&'static str, String)> {
        self.touched.lock().unwrap()[from..].to_vec()
    }
    fn note(&self, what: &'static str, key: &str) {
        self.touched.lock().unwrap().push((what, key.to_owned()));
    }
}

#[async_trait]
impl ObjectStore for Spy {
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.note("get", key);
        self.inner.get(key).await
    }
    async fn put(&self, key: &str, bytes: Vec<u8>) -> PortResult<()> {
        self.note("put", key);
        self.inner.put(key, bytes).await
    }
    async fn create(&self, key: &str, bytes: Vec<u8>) -> PortResult<bool> {
        self.note("create", key);
        self.inner.create(key, bytes).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.note("list", prefix);
        let mut entries = self.inner.list(prefix).await?;
        if let Some(smuggled) = self.smuggled.lock().unwrap().clone() {
            entries.push(smuggled);
        }
        Ok(entries)
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.note("delete", key);
        self.inner.delete(key).await
    }
}

/// Every pointer a catalog should never take, whatever it is bound to.
///
/// Built against one namespace's own canonical reference, so each entry differs
/// from something legitimate by exactly the thing being refused.
fn forgeries(prefix: &str, digest: &str) -> Vec<ArtifactRef> {
    let canonical = reference(prefix, ArtifactKind::Rows, digest);
    let mut forged = vec![
        // A digest that is not a content address is not a path segment.
        reference(prefix, ArtifactKind::Rows, "../secret"),
        reference(prefix, ArtifactKind::Rows, &"AB".repeat(32)),
        reference(prefix, ArtifactKind::Rows, "ab"),
        reference(prefix, ArtifactKind::Rows, ""),
    ];
    for uri in [
        canonical.uri.replace("/rows/", "/rows/../rows/"),
        canonical.uri.replace("/rows/", "/rows/%2e%2e/rows/"),
        canonical.uri.replace("/rows/", "/rows//"),
        format!("{}?project=other", canonical.uri),
        // Another family under the same prefix: a cache entry is not bytes.
        format!("object://{}", layout::cache_entry_key(prefix, KEY)),
    ] {
        forged.push(ArtifactRef {
            uri,
            ..canonical.clone()
        });
    }
    // The kind and the digest are checked with the URI rather than beside it.
    forged.push(ArtifactRef {
        kind: ArtifactKind::Log,
        ..canonical.clone()
    });
    forged.push(ArtifactRef {
        digest: aiwatcher_jobs::digest(b"other bytes"),
        ..canonical
    });
    forged
}

#[tokio::test]
async fn a_project_catalog_takes_its_own_pointers_and_nothing_else() {
    let spy = Arc::new(Spy::default());
    let store = Arc::clone(&spy) as Arc<dyn ObjectStore>;
    let mine = scope();
    let theirs = scope();
    let project = ObjectArtifactCatalog::new(Arc::clone(&store))
        .for_project(mine)
        .unwrap();
    let prefix = layout::prefix(Some(mine));
    let digest = address("bytes");

    let mut refusals = forgeries(&prefix, &digest);
    // Every other namespace, in both directions.
    refusals.push(reference(
        &layout::prefix(Some(theirs)),
        ArtifactKind::Rows,
        &digest,
    ));
    refusals.push(reference(
        &layout::prefix(None),
        ArtifactKind::Rows,
        &digest,
    ));
    // And a pointer nothing in this deployment addressed at all.
    refusals.push(
        ArtifactRef::new("rows", "file:///tmp/rows.json", digest.clone())
            .of_kind(ArtifactKind::Rows),
    );

    for artifact in refusals {
        let before = spy.since();
        let error = project
            .record(cataloged(artifact.clone(), 1, Vec::new()))
            .await
            .expect_err(&artifact.uri);
        assert!(refused(&error), "{}: {error}", artifact.uri);
        let error = project
            .remember(entry(vec![artifact.clone()], None))
            .await
            .expect_err(&artifact.uri);
        assert!(refused(&error), "{}: {error}", artifact.uri);
        assert_eq!(
            spy.did(before),
            Vec::new(),
            "a refusal did nobody else's I/O for {}",
            artifact.uri
        );
    }

    // Its own, so the rule is a boundary rather than a wall.
    let canonical = reference(&prefix, ArtifactKind::Rows, &digest);
    project
        .record(cataloged(canonical.clone(), 1, Vec::new()))
        .await
        .unwrap();
    project
        .remember(entry(vec![canonical], None))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_deployment_wide_catalog_refuses_a_pointer_into_a_project_and_keeps_the_rest() {
    // It has always recorded pointers it did not mint — an older build's
    // `s3://`, a step handing on a `file://` — and narrowing that here would
    // drop rows rather than isolate anything. What it must refuse is a pointer
    // into a project, which is the crossing the byte store already closed from
    // the other side, and a digest that could not have been a key.
    let spy = Arc::new(Spy::default());
    let store = Arc::clone(&spy) as Arc<dyn ObjectStore>;
    let catalog = ObjectArtifactCatalog::new(Arc::clone(&store));
    let mine = scope();
    let digest = address("bytes");

    for artifact in [
        reference(&layout::prefix(Some(mine)), ArtifactKind::Rows, &digest),
        reference(&layout::prefix(Some(scope())), ArtifactKind::Log, &digest),
        // Under the scoped area and naming no project: not this deployment's
        // either, and never resolved as if it were.
        ArtifactRef::new(
            "rows",
            format!("object://{}/scopes/nobody/rows/data", layout::PREFIX),
            digest.clone(),
        )
        .of_kind(ArtifactKind::Rows),
        reference(&layout::prefix(None), ArtifactKind::Rows, "../secret"),
        ArtifactRef::new("rows", "s3://bucket/rows.json", "ab").of_kind(ArtifactKind::Rows),
    ] {
        let before = spy.since();
        let error = catalog
            .record(cataloged(artifact.clone(), 1, Vec::new()))
            .await
            .expect_err(&artifact.uri);
        assert!(refused(&error), "{}: {error}", artifact.uri);
        let error = catalog
            .remember(entry(vec![artifact.clone()], None))
            .await
            .expect_err(&artifact.uri);
        assert!(refused(&error), "{}: {error}", artifact.uri);
        assert_eq!(
            spy.did(before),
            Vec::new(),
            "a refusal did nobody else's I/O for {}",
            artifact.uri
        );
    }

    for artifact in [
        reference(&layout::prefix(None), ArtifactKind::Rows, &digest),
        ArtifactRef::new("rows", "s3://bucket/rows.json", digest.clone())
            .of_kind(ArtifactKind::Rows),
        ArtifactRef::new("rows", "file:///mnt/shared/rows.json", digest.clone())
            .of_kind(ArtifactKind::Log),
    ] {
        catalog
            .record(cataloged(artifact.clone(), 1, Vec::new()))
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", artifact.uri));
    }
}

#[tokio::test]
async fn a_record_swapped_underneath_answers_with_a_refusal_and_never_with_it() {
    let spy = Arc::new(Spy::default());
    let store = Arc::clone(&spy) as Arc<dyn ObjectStore>;
    let mine = scope();
    let theirs = scope();
    let project = ObjectArtifactCatalog::new(Arc::clone(&store))
        .for_project(mine)
        .unwrap();
    let prefix = layout::prefix(Some(mine));
    let digest = address("bytes");
    let foreign = reference(&layout::prefix(Some(theirs)), ArtifactKind::Rows, &digest);
    let global = reference(&layout::prefix(None), ArtifactKind::Rows, &digest);

    // A manifest that points out of this namespace, and one that describes a
    // different artifact than the key it is filed under.
    for tampered in [
        cataloged(foreign.clone(), 1, Vec::new()),
        cataloged(global.clone(), 1, Vec::new()),
        cataloged(
            reference(&prefix, ArtifactKind::Rows, &address("other bytes")),
            1,
            Vec::new(),
        ),
    ] {
        store
            .put(
                &layout::manifest_key(&prefix, ArtifactKind::Rows, &digest),
                serde_json::to_vec(&tampered).unwrap(),
            )
            .await
            .unwrap();
        let before = spy.since();
        let error = project
            .by_digest(&digest, ArtifactKind::Rows)
            .await
            .expect_err("a swapped manifest");
        assert!(refused(&error), "{error}");
        assert!(
            spy.did(before).iter().all(|(what, _)| *what == "get"),
            "a refusal reads and never writes"
        );
        // And `record` will not take it as "already recorded" either.
        let error = project
            .record(cataloged(
                reference(&prefix, ArtifactKind::Rows, &digest),
                1,
                Vec::new(),
            ))
            .await
            .expect_err("a swapped manifest on the idempotent path");
        assert!(refused(&error), "{error}");
    }

    // A cache entry that answers for another key, and one that points out.
    let key_path = layout::cache_entry_key(&prefix, KEY);
    for tampered in [
        CacheEntry {
            cache_key: "somebody else's step".to_owned(),
            ..entry(vec![reference(&prefix, ArtifactKind::Rows, &digest)], None)
        },
        entry(vec![foreign.clone()], None),
        entry(vec![global.clone()], None),
    ] {
        store
            .put(&key_path, serde_json::to_vec(&tampered).unwrap())
            .await
            .unwrap();
        let error = project
            .cached(KEY, EPOCH)
            .await
            .expect_err("a swapped entry");
        assert!(refused(&error), "{error}");
        let before = spy.since();
        let error = project
            .invalidate(KEY, EPOCH)
            .await
            .expect_err("a swapped entry");
        assert!(refused(&error), "{error}");
        assert!(
            spy.did(before).iter().all(|(what, _)| *what == "get"),
            "an entry this namespace may not answer with is not one it rewrites"
        );
    }

    // Unreadable JSON stays a miss, as the retry contract has always said.
    store.put(&key_path, b"{not json".to_vec()).await.unwrap();
    assert!(project.cached(KEY, EPOCH).await.unwrap().is_none());
    project.invalidate(KEY, EPOCH).await.unwrap();
}

#[tokio::test]
async fn a_lineage_listing_is_checked_rather_than_followed() {
    let spy = Arc::new(Spy::default());
    let store = Arc::clone(&spy) as Arc<dyn ObjectStore>;
    let mine = scope();
    let project = ObjectArtifactCatalog::new(Arc::clone(&store))
        .for_project(mine)
        .unwrap();
    let prefix = layout::prefix(Some(mine));
    let rows = address("rows");
    project
        .record(cataloged(
            reference(&prefix, ArtifactKind::Rows, &rows),
            1,
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(
        project
            .produced_by(&ExecutionId::new(RUN))
            .await
            .unwrap()
            .len(),
        1
    );

    // A pointer filed under this run that names a digest it is not named by.
    let other = address("another run's rows");
    store
        .put(
            &layout::lineage_key(&prefix, RUN, &rows),
            serde_json::json!({"digest": other, "kind": "rows"})
                .to_string()
                .into_bytes(),
        )
        .await
        .unwrap();
    let error = project
        .produced_by(&ExecutionId::new(RUN))
        .await
        .expect_err("a pointer that does not describe its own key");
    assert!(refused(&error), "{error}");

    // A pointer whose digest could never have been a key at all.
    store
        .put(
            &layout::lineage_key(&prefix, RUN, &rows),
            serde_json::json!({"digest": "../secret", "kind": "rows"})
                .to_string()
                .into_bytes(),
        )
        .await
        .unwrap();
    let error = project
        .produced_by(&ExecutionId::new(RUN))
        .await
        .expect_err("a digest that is not a content address");
    assert!(refused(&error), "{error}");
    store
        .delete(&layout::lineage_key(&prefix, RUN, &rows))
        .await
        .unwrap();

    // A manifest whose provenance names another execution, reached through a
    // pointer filed under this one.
    let borrowed = address("somebody else's rows");
    store
        .put(
            &layout::manifest_key(&prefix, ArtifactKind::Rows, &borrowed),
            serde_json::to_vec(&CatalogedArtifact {
                produced_by: Some(Provenance {
                    execution_id: ExecutionId::new("execution-2"),
                    step_id: "read".to_owned(),
                    attempt: 1,
                }),
                ..cataloged(
                    reference(&prefix, ArtifactKind::Rows, &borrowed),
                    1,
                    Vec::new(),
                )
            })
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .put(
            &layout::lineage_key(&prefix, RUN, &borrowed),
            serde_json::json!({"digest": borrowed, "kind": "rows"})
                .to_string()
                .into_bytes(),
        )
        .await
        .unwrap();
    let error = project
        .produced_by(&ExecutionId::new(RUN))
        .await
        .expect_err("another execution's artifact");
    assert!(refused(&error), "{error}");
    store
        .delete(&layout::lineage_key(&prefix, RUN, &borrowed))
        .await
        .unwrap();
    store
        .delete(&layout::manifest_key(
            &prefix,
            ArtifactKind::Rows,
            &borrowed,
        ))
        .await
        .unwrap();

    // And a listing that answers outside the prefix it was asked about is a
    // store's bug rather than a row to follow.
    *spy.smuggled.lock().unwrap() = Some(ObjectEntry {
        key: layout::lineage_key(&layout::prefix(None), RUN, &rows),
        size: 1,
        last_modified: None,
    });
    let error = project
        .produced_by(&ExecutionId::new(RUN))
        .await
        .expect_err("a key from outside the prefix");
    assert!(refused(&error), "{error}");
}

#[tokio::test]
async fn isolation_leaves_idempotency_expiry_and_the_no_delete_rule_where_they_were() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let mine = scope();
    let project = ObjectArtifactCatalog::new(Arc::clone(&store))
        .for_project(mine)
        .unwrap();
    let prefix = layout::prefix(Some(mine));
    let rows = reference(&prefix, ArtifactKind::Rows, &address("rows"));

    // The attempt that wrote the bytes keeps them.
    project
        .record(cataloged(rows.clone(), 1, Vec::new()))
        .await
        .unwrap();
    let again = project
        .record(cataloged(rows.clone(), 2, vec![address("something else")]))
        .await
        .unwrap();
    assert_eq!(again.produced_by.expect("provenance").attempt, 1);
    assert!(again.inputs.is_empty(), "and so does what it was made from");
    assert_eq!(
        project
            .produced_by(&ExecutionId::new(RUN))
            .await
            .unwrap()
            .len(),
        1,
        "a deterministic rerun is one artifact and not two"
    );

    // An expired entry and a missing one are the same answer, and neither
    // deletes anything.
    project
        .remember(entry(vec![rows.clone()], Some(EPOCH + Duration::hours(1))))
        .await
        .unwrap();
    assert!(
        project
            .cached(KEY, EPOCH + Duration::minutes(30))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        project
            .cached(KEY, EPOCH + Duration::hours(2))
            .await
            .unwrap()
            .is_none()
    );
    project.invalidate(KEY, EPOCH).await.unwrap();
    assert!(project.cached(KEY, EPOCH).await.unwrap().is_none());
    assert!(
        store
            .get(&layout::cache_entry_key(&prefix, KEY))
            .await
            .unwrap()
            .is_some(),
        "invalidation marks the entry rather than deleting it"
    );
    assert!(
        project
            .by_digest(&rows.digest, ArtifactKind::Rows)
            .await
            .unwrap()
            .is_some(),
        "and leaves what it named alone"
    );
    assert_eq!(
        store
            .list(&layout::prefix(None))
            .await
            .unwrap()
            .iter()
            .filter(|entry| entry.key.starts_with(&layout::prefix(Some(mine))))
            .count(),
        3,
        "the manifest, its lineage pointer and the cache entry are all still there"
    );
}
