//! Admitting a variant is not authorizing an executor.
use super::{fixture::Fixture, project_declarations::seed, project_evidence::source, *};
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::execution::scoring::ScoreExecutor;
use serde_json::json;

#[tokio::test]
async fn project_scoring_admission_verifies_local_variant_bytes_but_grants_no_execution_authority()
{
    let fixture = Fixture::new("project-scoring-admission").await;
    let path = fixture.root.join("store");
    let store = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let owner = Arc::new(source(store.clone(), &fixture));
    let root = Registry::new(store.clone(), owner.clone(), Default::default()).unwrap();
    let scope = ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    };
    let registry = root.for_project_evidence(scope).unwrap();
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets
        .for_project(scope)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let mut run = seed(&registry, &fixture).await;
    let prompts = aiwatcher_prompts::Registry::new(store.clone(), Default::default());
    let prompt: aiwatcher_prompts::PublishRequest =
        serde_json::from_value(json!({"name":"private", "text":"Answer {{ question }}."})).unwrap();
    let version = prompts.publish(prompt.clone()).await.unwrap().version;
    run.variant.prompt = Some(VersionReference {
        name: version.name.to_string(),
        version: version.version_id.to_string(),
    });
    let declared = registry
        .declare_scoring_run(&run, "editor", 10)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    let bundle = owner.for_project(scope).unwrap();
    assert!(!view.admitted);
    assert!(registry.approve(&view.manifest, "admin", 11).await.is_err());
    bundle
        .stage(
            &view.approval_id,
            "manifest.json",
            serde_json::to_vec(&view.manifest).unwrap(),
        )
        .await
        .unwrap();
    for name in ["responses.py", "generation.json", "workflow.json"] {
        // Each pinned member is required; files on the host are no fallback.
        assert!(registry.approve(&view.manifest, "admin", 11).await.is_err());
        bundle
            .stage(
                &view.approval_id,
                name,
                tokio::fs::read(fixture.root.join(name)).await.unwrap(),
            )
            .await
            .unwrap();
    }
    // An identical global prompt is still not the project's prompt.
    assert!(registry.approve(&view.manifest, "admin", 11).await.is_err());
    prompts
        .for_project(scope)
        .unwrap()
        .publish(prompt)
        .await
        .unwrap();
    let approved = registry.approve(&view.manifest, "admin", 12).await.unwrap();
    assert!(registry.admits(&view.manifest).await.unwrap());
    assert!(
        registry
            .scoring_run_view(&declared.id)
            .await
            .unwrap()
            .unwrap()
            .admitted
    );
    assert_eq!(
        registry
            .approve(&view.manifest, "another-admin", 13)
            .await
            .unwrap(),
        approved
    );
    // Cards/metrics remain registry-owned, not a bypass enabled by the adapter.
    let mut forged = view.manifest.clone();
    forged.context.scorer.version = "other-binary".into();
    assert!(registry.approve(&forged, "admin", 14).await.is_err());
    forged = view.manifest.clone();
    forged.context.metrics[0].direction = MetricDirection::Lower;
    assert!(registry.approve(&forged, "admin", 14).await.is_err());
    for other in [
        ProjectScope {
            project: ProjectId::new(),
            ..scope
        },
        ProjectScope {
            organization: OrganizationId::new(),
            ..scope
        },
    ] {
        assert!(
            root.for_project_evidence(other)
                .unwrap()
                .approve(&view.manifest, "admin", 14)
                .await
                .is_err()
        );
    }
    assert!(root.approve(&view.manifest, "admin", 14).await.is_err());
    let reopened_store = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let reopened = Registry::new(
        reopened_store.clone(),
        Arc::new(source(reopened_store, &fixture)),
        Default::default(),
    )
    .unwrap()
    .for_project_evidence(scope)
    .unwrap();
    assert!(reopened.admits(&view.manifest).await.unwrap());
    for name in ["responses.py", "generation.json", "workflow.json"] {
        bundle
            .stage(&view.approval_id, name, b"changed".to_vec())
            .await
            .unwrap();
        assert!(reopened.admission(&view.manifest).await.is_err());
        bundle
            .stage(
                &view.approval_id,
                name,
                tokio::fs::read(fixture.root.join(name)).await.unwrap(),
            )
            .await
            .unwrap();
        assert!(reopened.admits(&view.manifest).await.unwrap());
    }
    // Even a bound registry with a fully approved pair cannot be passed to a
    // global executor as a substitute for project execution authorization.
    let (command, context) = attempt(&declared.id, &run.evaluation_id);
    let refused = ScoreExecutor::new(Arc::new(reopened.clone()))
        .execute(&command, &context)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::Policy);
    assert!(refused.message.contains("execution authority"));
    assert!(
        reopened
            .get(&run.evaluation_id, "admin", 15)
            .await
            .unwrap()
            .is_none()
    );
    reopened
        .withdraw(&view.approval_id, "admin", 16)
        .await
        .unwrap()
        .unwrap();
    assert!(!reopened.admits(&view.manifest).await.unwrap());
}
