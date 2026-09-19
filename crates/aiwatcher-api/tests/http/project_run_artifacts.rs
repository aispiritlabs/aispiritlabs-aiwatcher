//! What a project's run produced, read by the project (ADR_0033, IAM-03 M0).
//!
//! The pair that had no twin. Until D4 that was defensible: the instance route
//! refused a project's run rather than answering an empty list, and a project
//! member still held `viewer` on the instance and could read their own bytes
//! nowhere else — so the gap was visible and small. D4 takes the instance role
//! away from a client, which turns "no twin" into "no route at all" for the one
//! thing a step's view is mostly for: the log of the pod that ran it.
//!
//! Three stores have to land on the same side for this to be an answer rather
//! than a coincidence — the workflow store the run is in, the catalog that
//! describes what it made, and the bytes themselves — and each is asked here.
use super::*;

use aiwatcher_execution::artifact::Provenance;
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    ArtifactCatalog, CatalogedArtifact, ExecutionPlan, WorkflowStore,
    plan::{
        CachePolicy, DefinitionKind, DefinitionRevision, PlanStep, RetryPolicy, RuntimeBinding,
        ScoreEvaluationSpec,
    },
};

/// One step, of a kind a project has a performer for.
fn plan() -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Evaluation,
        "nightly".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![PlanStep {
            id: "score".to_owned(),
            runtime: RuntimeBinding::ScoreEvaluation(ScoreEvaluationSpec {
                declaration: "nightly".to_owned(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        }],
        Vec::new(),
    )
}

fn started(
    key: &str,
    project: Option<aiwatcher_execution::ProjectStart>,
) -> aiwatcher_execution::StartRun {
    aiwatcher_execution::StartRun {
        identity: aiwatcher_execution::RunIdentity::Key(key.to_owned()),
        parameters: std::collections::BTreeMap::new(),
        requested_by: "a test".to_owned(),
        decided_by: aiwatcher_execution::Decider::Local,
        payloads: None,
        project,
    }
}

/// Start a run on `store` and put one artifact of it in `catalog` and `bytes`.
async fn produced(
    store: &Arc<dyn WorkflowStore>,
    catalog: &Arc<dyn ArtifactCatalog>,
    bytes: &MemoryArtifacts,
    key: &str,
    project: Option<aiwatcher_execution::ProjectStart>,
    log: &str,
) -> (String, String) {
    let handler = aiwatcher_execution::ExecutionHandler::new(Arc::clone(store));
    let run = aiwatcher_execution::Executions {
        handler: Some(&handler),
        pipelines: None,
        workflows: None,
        payloads: Default::default(),
        archive: false,
        engine: Default::default(),
        query_timeout_seconds: None,
        notify: None,
    }
    .start(plan(), started(key, project))
    .await
    .expect("a start");
    let artifact = bytes.put_bytes("pod.log", aiwatcher_core::ArtifactKind::Log, log.as_bytes());
    let digest = artifact.digest.clone();
    catalog
        .record(CatalogedArtifact {
            artifact,
            produced_by: Some(Provenance {
                execution_id: run.execution_id.clone(),
                step_id: "score".to_owned(),
                attempt: 1,
            }),
            inputs: Vec::new(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        })
        .await
        .expect("a catalog row");
    (run.execution_id.as_str().to_owned(), digest)
}

#[tokio::test]
async fn a_project_reads_its_own_runs_artifacts_and_the_instance_reads_none_of_them() {
    let mut f = IamFixture::with_registries().await;
    let store: Arc<dyn WorkflowStore> = Arc::new(MemoryWorkflowStore::new());
    f.fixture.state.executions = Some(Arc::new(aiwatcher_execution::ExecutionHandler::new(
        Arc::clone(&store),
    )));
    let catalog: Arc<dyn ArtifactCatalog> =
        Arc::new(aiwatcher_execution::artifact::memory::MemoryArtifactCatalog::new());
    f.fixture.state.catalog = Some(Arc::clone(&catalog));
    let objects = Arc::new(MemoryArtifacts::default());
    f.fixture.state.artifacts =
        Some(Arc::clone(&objects) as Arc<dyn aiwatcher_core::ports::AttemptArtifacts>);

    let owner = f.cookie("owner", Role::Admin);
    let client = f.client_cookie("a client");
    let (_, identity) = f
        .request("GET", "/api/v1/auth/me", Some(&client), Value::Null, false)
        .await;
    let subject = identity["subject"].as_str().expect("a subject").to_owned();
    let organization = f.create(&owner).await;
    let project = f.project(&owner, &organization).await;
    f.grant(&owner, &organization, &project, &subject, "viewer")
        .await;
    let scope =
        aiwatcher_iam::ProjectScope::parse(&format!("{organization}/{project}")).expect("a scope");

    // One run on each side, each with a log of its own.
    let (global, _) = produced(
        &store,
        &catalog,
        &objects,
        "global",
        None,
        "the deployment's own run",
    )
    .await;
    let bound = store.for_project(scope).expect("a project store");
    let mine = catalog.for_project(scope).expect("a project catalog");
    let my_bytes = MemoryArtifacts {
        objects: Arc::clone(&objects.objects),
        scope: Some(scope.on_the_log()),
    };
    let (ours, digest) = produced(
        &bound,
        &mine,
        &my_bytes,
        "ours",
        Some(aiwatcher_execution::ProjectStart::new(
            scope,
            aiwatcher_iam::Principal::new(&f.issuer, &subject).expect("a principal"),
        )),
        "what our pod said",
    )
    .await;

    let root = format!("/api/v1/orgs/{organization}/projects/{project}");
    let (status, produced_rows) = f
        .request(
            "GET",
            &format!("{root}/executions/{ours}/artifacts"),
            Some(&client),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{produced_rows}");
    assert_eq!(produced_rows[0]["artifact"]["digest"], digest);

    let (status, content) = f
        .request(
            "GET",
            &format!("{root}/executions/{ours}/artifacts/{digest}"),
            Some(&client),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{content}");
    assert_eq!(content["text"], "what our pod said");

    // And the three refusals that make it a boundary. The instance's own list
    // is not this caller's to read at all…
    let (status, refused) = f
        .request(
            "GET",
            &format!("/api/v1/executions/{ours}/artifacts"),
            Some(&client),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused["code"], "instance_role_required");

    // …an instance admin reading it is told the run is not there, rather than
    // that it produced nothing…
    assert_eq!(
        f.request(
            "GET",
            &format!("/api/v1/executions/{ours}/artifacts"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
    );

    // …and the project's own route does not reach back the other way.
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/executions/{global}/artifacts"),
            Some(&client),
            Value::Null,
            false,
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
    );
}
