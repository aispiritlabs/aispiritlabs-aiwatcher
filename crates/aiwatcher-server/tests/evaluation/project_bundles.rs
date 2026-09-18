use super::*;
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::evaluation::LocalSource;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn prefix(scope: ProjectScope, id: &str) -> String {
    format!(
        "evaluation-scopes/{}/{}/bundles/{id}/",
        scope.organization.0, scope.project.0
    )
}

#[tokio::test]
async fn project_bundle_files_are_isolated_and_reopen_without_directory_or_global_fallback() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-project-bundles-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(dir.join("store")).await.unwrap());
    tokio::fs::write(dir.join("manifest.json"), b"host declaration")
        .await
        .unwrap();
    let root = LocalSource::new(Some(dir.to_str().unwrap().into())).with_bundles(store.clone());
    let a_scope = scope();
    let b_scope = ProjectScope {
        project: ProjectId::new(),
        ..a_scope
    };
    let c_scope = ProjectScope {
        organization: OrganizationId::new(),
        ..a_scope
    };
    let a = root.for_project(a_scope).unwrap();
    let b = root.for_project(b_scope).unwrap();
    let c = root.for_project(c_scope).unwrap();
    let id = "a".repeat(64);
    let other_id = "b".repeat(64);
    assert_eq!(
        root.member(&id, "manifest.json").await.unwrap(),
        Some(b"host declaration".to_vec())
    );
    for bound in [&a, &b, &c] {
        assert!(bound.member(&id, "manifest.json").await.unwrap().is_none());
        assert!(bound.staged(&id).await.unwrap().is_empty());
    }
    let files = [
        ("manifest.json", b"{\n  \"source\": \"A\"\n}".to_vec()),
        ("model-artifacts/weights.bin", vec![0, 1, 2, 255]),
        ("scorer.py", b"# exact bytes\n".to_vec()),
    ];
    for (name, bytes) in &files {
        a.stage(&id, name, bytes.clone()).await.unwrap();
    }
    for bound in [&b, &c] {
        assert!(bound.staged(&id).await.unwrap().is_empty());
        assert_eq!(bound.discard(&id).await.unwrap(), 0);
    }
    assert!(root.staged(&id).await.unwrap().is_empty());
    assert!(a.staged(&other_id).await.unwrap().is_empty());
    for (bound, label) in [(&b, "B"), (&c, "C")] {
        bound
            .stage(&id, "manifest.json", label.as_bytes().to_vec())
            .await
            .unwrap();
    }
    root.stage(&id, "manifest.json", b"global".to_vec())
        .await
        .unwrap();
    a.stage(&other_id, "manifest.json", b"other pair".to_vec())
        .await
        .unwrap();
    let reopened = LocalSource::new(None)
        .with_bundles(Arc::new(
            FileObjectStore::open(dir.join("store")).await.unwrap(),
        ))
        .for_project(a_scope)
        .unwrap();
    assert_eq!(reopened.staged(&id).await.unwrap().len(), 3);
    for (name, bytes) in &files {
        assert_eq!(
            reopened.member(&id, name).await.unwrap().as_ref(),
            Some(bytes)
        );
        assert_eq!(
            store
                .get(&format!("{}{name}", prefix(a_scope, &id)))
                .await
                .unwrap()
                .as_ref(),
            Some(bytes)
        );
    }
    assert!(a.for_project(b_scope).is_err());
    assert!(a.for_project(a_scope).is_ok());
    assert_eq!(a.discard(&id).await.unwrap(), 3);
    assert_eq!(a.discard(&id).await.unwrap(), 0);
    assert!(a.member(&id, "manifest.json").await.unwrap().is_none());
    assert_eq!(
        a.member(&other_id, "manifest.json").await.unwrap().unwrap(),
        b"other pair"
    );
    assert_eq!(b.member(&id, "manifest.json").await.unwrap().unwrap(), b"B");
    assert_eq!(c.member(&id, "manifest.json").await.unwrap().unwrap(), b"C");
    assert_eq!(
        root.member(&id, "manifest.json").await.unwrap().unwrap(),
        b"global"
    );
    assert!(
        LocalSource::new(Some(dir.to_str().unwrap().into()))
            .for_project(a_scope)
            .is_err()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Debug)]
struct BadListing {
    malformed_member: bool,
}
#[async_trait]
impl ObjectStore for BadListing {
    async fn put(&self, _: &str, _: Vec<u8>) -> PortResult<()> {
        panic!("no writes")
    }
    async fn get(&self, _: &str) -> PortResult<Option<Vec<u8>>> {
        panic!("no reads")
    }
    async fn delete(&self, _: &str) -> PortResult<()> {
        panic!("validate the entire listing before any delete")
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        Ok(vec![
            ObjectEntry {
                key: format!("{prefix}manifest.json"),
                size: 1,
                last_modified: None,
            },
            ObjectEntry {
                key: if self.malformed_member {
                    format!("{prefix}model-artifacts/../foreign")
                } else {
                    "evaluation-scopes/foreign/secret".into()
                },
                size: 1,
                last_modified: None,
            },
        ])
    }
}
#[tokio::test]
async fn bundle_lists_reject_foreign_keys_and_bad_members_before_any_deletion() {
    for malformed_member in [false, true] {
        let root = LocalSource::new(None).with_bundles(Arc::new(BadListing { malformed_member }));
        let bound = root.for_project(scope()).unwrap();
        for adapter in [&root as &dyn ApprovalBundles, bound.as_ref()] {
            assert!(adapter.staged(&"a".repeat(64)).await.is_err());
            assert!(adapter.discard(&"a".repeat(64)).await.is_err());
            for id in ["", "..", "../foreign", "x\\foreign", "not-a-digest"] {
                assert!(adapter.stage(id, "manifest.json", vec![]).await.is_err());
                assert!(adapter.staged(id).await.is_err());
                assert!(adapter.discard(id).await.is_err());
                assert!(adapter.member(id, "manifest.json").await.is_err());
            }
            for name in [
                "../foreign",
                "model-artifacts/../secret",
                "model-artifacts/",
                "hidden/.file",
                ".hidden",
                "x\0y",
                "a\\b",
            ] {
                assert!(adapter.stage(&"a".repeat(64), name, vec![]).await.is_err());
                assert!(adapter.member(&"a".repeat(64), name).await.is_err());
            }
        }
    }
}
