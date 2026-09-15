#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aiwatcher_core::{
    ports::PortResult,
    storage::{ObjectEntry, ObjectStore},
};
use aiwatcher_execution::{
    DefinitionError,
    definition::{DefinitionRegistry, WorkflowSpec},
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use time::OffsetDateTime;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn definition(task: &str) -> WorkflowSpec {
    serde_json::from_value(json!({"name":"house/import","version":"1","steps":[
        {"id":"acquire","task_ref":task,"queue":"local","timeout_seconds":30,"outputs":["rows"],"params":{"source":"original-source"}},
        {"id":"review","approval":{"prompt":"Publish these rows?","role":"editor","choices":["yes","no"]},"after":["acquire"]}
    ]})).unwrap()
}

#[tokio::test]
async fn project_definition_files_keep_pins_history_metadata_and_compiled_identity() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-definition-scope-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = DefinitionRegistry::new(store.clone());
    let a_scope = scope();
    let b_scope = ProjectScope {
        project: ProjectId::new(),
        ..a_scope
    };
    let c_scope = ProjectScope {
        organization: OrganizationId::new(),
        ..a_scope
    };
    let a = legacy.for_project(a_scope).unwrap();
    let b = legacy.for_project(b_scope).unwrap();
    let c = legacy.for_project(c_scope).unwrap();
    let original = definition("acquire@1");
    let now = OffsetDateTime::from_unix_timestamp(1000).unwrap();
    let saved = a
        .save(original.clone(), "original-author".into(), now)
        .await
        .unwrap();
    for other in [&b, &c, &legacy] {
        assert!(other.list().await.unwrap().is_empty());
        assert!(other.get(&original.name, None).await.unwrap().is_none());
        assert!(
            other
                .get(&original.name, Some(&saved.revision.0))
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        a.save(
            original.clone(),
            "second-author".into(),
            now + time::Duration::hours(1)
        )
        .await
        .unwrap(),
        saved
    );
    for other in [&b, &legacy] {
        let copy = other
            .save(original.clone(), "original-author".into(), now)
            .await
            .unwrap();
        assert_eq!(copy, saved);
        assert_eq!(
            copy.definition.compile().unwrap(),
            original.compile().unwrap()
        );
    }
    let next = a
        .save(
            definition("acquire@2"),
            "second-author".into(),
            now + time::Duration::hours(1),
        )
        .await
        .unwrap();
    assert_ne!(next.revision, saved.revision);
    assert_ne!(
        next.definition.compile().unwrap().plan_id,
        saved.definition.compile().unwrap().plan_id
    );
    let reopened = DefinitionRegistry::new(Arc::new(FileObjectStore::open(&dir).await.unwrap()))
        .for_project(a_scope)
        .unwrap();
    assert_eq!(
        reopened
            .get(&original.name, Some(&saved.revision.0))
            .await
            .unwrap(),
        Some(saved.clone())
    );
    assert_eq!(
        reopened.get(&original.name, None).await.unwrap(),
        Some(next.clone())
    );
    assert_eq!(reopened.list().await.unwrap(), vec![next]);
    assert_eq!(
        b.get(&original.name, None).await.unwrap(),
        Some(saved.clone())
    );
    assert_eq!(
        legacy.get(&original.name, None).await.unwrap(),
        Some(saved.clone())
    );
    assert!(a.for_project(b_scope).is_err());
    assert_eq!(
        a.for_project(a_scope).unwrap().list().await.unwrap(),
        a.list().await.unwrap()
    );
    for r in [&a, &legacy] {
        for invalid in [
            "../heads/secret",
            "..\\secret",
            "/absolute",
            &"A".repeat(64),
        ] {
            assert!(
                r.get(&original.name, Some(invalid))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
    let root = format!(
        "workflows/scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    let entries = store.list(&root).await.unwrap();
    for entry in &entries {
        if entry.key.ends_with(&format!("/{}.json", saved.revision.0)) {
            let relative = entry.key.strip_prefix(&root).unwrap();
            assert_eq!(
                store.get(&entry.key).await.unwrap(),
                store.get(&format!("workflows/{relative}")).await.unwrap()
            );
        }
    }
    // A head with bytes under the wrong revision is refused by list and detail.
    let head = entries
        .iter()
        .find(|entry| entry.key.starts_with(&format!("{root}heads/")))
        .unwrap();
    let mut corrupt = serde_json::to_value(&saved).unwrap();
    corrupt["definition"]["steps"][0]["params"] = json!({"altered":true});
    store
        .put(&head.key, serde_json::to_vec(&corrupt).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        reopened.list().await,
        Err(DefinitionError::Corrupt { .. })
    ));
    assert!(matches!(
        reopened.get(&original.name, None).await,
        Err(DefinitionError::Corrupt { .. })
    ));
    assert_eq!(legacy.get(&original.name, None).await.unwrap(), Some(saved));
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Debug)]
struct ForeignListing {
    key: String,
    reads: AtomicUsize,
}
#[async_trait]
impl ObjectStore for ForeignListing {
    async fn put(&self, _: &str, _: Vec<u8>) -> PortResult<()> {
        panic!("read-only test");
    }
    async fn get(&self, _: &str) -> PortResult<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    async fn list(&self, _: &str) -> PortResult<Vec<ObjectEntry>> {
        Ok(vec![ObjectEntry {
            key: self.key.clone(),
            size: 0,
            last_modified: None,
        }])
    }
    async fn delete(&self, _: &str) -> PortResult<()> {
        panic!("read-only test");
    }
}

#[tokio::test]
async fn a_listing_cannot_make_project_or_legacy_registry_read_a_foreign_key() {
    let a = scope();
    for bound in [None, Some(a)] {
        let root = bound.map_or_else(
            || "workflows".into(),
            |scope| {
                format!(
                    "workflows/scopes/{}/{}/registry",
                    scope.organization.0, scope.project.0
                )
            },
        );
        for key in [
            format!(
                "workflows/scopes/{}/{}/registry/heads/{}.json",
                OrganizationId::new().0,
                ProjectId::new().0,
                "a".repeat(64)
            ),
            format!("{root}/heads/../../../heads/{}.json", "a".repeat(64)),
            format!("{root}/heads/..\\head.json"),
            format!("{root}/versions/{}/{}.json", "a".repeat(64), "b".repeat(64)),
        ] {
            let store = Arc::new(ForeignListing {
                key,
                reads: AtomicUsize::new(0),
            });
            let legacy = DefinitionRegistry::new(store.clone());
            let registry = bound.map_or(legacy.clone(), |scope| legacy.for_project(scope).unwrap());
            assert!(matches!(
                registry.list().await,
                Err(DefinitionError::Corrupt { .. })
            ));
            assert_eq!(
                store.reads.load(Ordering::SeqCst),
                0,
                "reject before reading foreign bytes"
            );
        }
    }
}
