#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aiwatcher_core::storage::ObjectStore;
use aiwatcher_evaluation::{
    AssessmentRequest, AssessmentTarget, Registry, Rubric, Scorecard, SourceAuthority,
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug)]
struct UnscopedSources;
#[async_trait]
impl SourceAuthority for UnscopedSources {
    async fn resolve(
        &self,
        _: &aiwatcher_evaluation::EvaluationManifest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::SourceEvidence> {
        panic!("authored operations must never resolve an instance source")
    }
    async fn derive_cohort(
        &self,
        _: &aiwatcher_evaluation::CohortRequest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::CohortFiles> {
        panic!("scoped registry must not retain the instance source resolver")
    }
}
fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn registry(store: Arc<dyn ObjectStore>) -> Registry {
    Registry::new(store, Arc::new(UnscopedSources), Default::default()).unwrap()
}
fn rubric(question: &str) -> Rubric {
    serde_json::from_value(json!({"name":"helpfulness", "question":question,
        "scale":{"kind":"ordinal","levels":["bad","good","great"]}, "direction":"higher"}))
    .unwrap()
}
fn card(version: &str, pass: &str) -> Scorecard {
    serde_json::from_value(
        json!({"name":"quality", "scorers":[{"metric":"helpful", "scorer":{
            "kind":"judge", "rubric":{"name":"helpfulness","version":version}, "pass_level":pass
        }}]}),
    )
    .unwrap()
}
fn target() -> AssessmentTarget {
    serde_json::from_value(json!({"kind":"case","evaluation_id":"source-result", "case_id":"case-1","repetition_id":"trial-1"})).unwrap()
}
fn assessment(version: &str, level: &str) -> AssessmentRequest {
    serde_json::from_value(
        json!({"target":target(),"rubric":"helpfulness", "rubric_version":version,
        "value":{"type":"level","value":level}, "rationale":"original rationale"}),
    )
    .unwrap()
}
fn value(item: &impl serde::Serialize) -> Value {
    serde_json::to_value(item).unwrap()
}

#[path = "scope/reviews.rs"]
mod review_scope;

#[tokio::test]
async fn authored_project_files_preserve_hashes_history_and_local_references_after_reopen() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-scoped-evaluation-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = registry(store.clone());
    let a_scope = scope();
    let b_scope = ProjectScope {
        project: ProjectId::new(),
        ..a_scope
    };
    let c_scope = ProjectScope {
        organization: OrganizationId::new(),
        ..a_scope
    };
    let a = legacy.for_project_authored(a_scope).unwrap();
    let b = legacy.for_project_authored(b_scope).unwrap();
    let c = legacy.for_project_authored(c_scope).unwrap();
    let form = a
        .publish_rubric(&rubric("original question"), "person", 100)
        .await
        .unwrap();
    let first = a
        .publish_scorecard(&card(&form.version, "good"), "person", 101)
        .await
        .unwrap();
    let said = a
        .assess(&assessment(&form.version, "good"), "person", 102)
        .await
        .unwrap();
    for other in [&b, &c, &legacy] {
        assert!(other.rubrics().await.unwrap().is_empty());
        assert!(other.scorecards().await.unwrap().is_empty());
        assert!(
            other
                .rubric("helpfulness", Some(&form.version))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            other
                .scorecard("quality", Some(&first.version))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            other
                .assessments(&target())
                .await
                .unwrap()
                .assessments
                .is_empty()
        );
        assert!(
            other
                .assessment_history(&said.target_id, &said.standing_id, None, None)
                .await
                .unwrap()
                .revisions
                .is_empty()
        );
        assert!(
            other
                .assess(&assessment(&form.version, "good"), "person", 103)
                .await
                .is_err()
        );
        assert!(
            other
                .publish_scorecard(&card(&form.version, "good"), "person", 103)
                .await
                .is_err()
        );
    }
    // The same content is explicitly published in each registry, preserving its identity.
    for other in [&b, &legacy] {
        let copied = other
            .publish_rubric(&rubric("original question"), "person", 100)
            .await
            .unwrap();
        assert_eq!(value(&copied), value(&form));
        let copied = other
            .publish_scorecard(&card(&form.version, "good"), "person", 101)
            .await
            .unwrap();
        assert_eq!(value(&copied), value(&first));
        let copied = other
            .assess(&assessment(&form.version, "good"), "person", 102)
            .await
            .unwrap();
        assert_eq!(value(&copied), value(&said));
    }
    let second = a
        .publish_scorecard(&card(&form.version, "great"), "person", 104)
        .await
        .unwrap();
    assert_ne!(first.version, second.version);
    let updated = a
        .assess(&assessment(&form.version, "great"), "person", 105)
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    a.publish_rubric(&rubric("new question"), "other-person", 106)
        .await
        .unwrap();
    let reopened = registry(Arc::new(FileObjectStore::open(&dir).await.unwrap()))
        .for_project_authored(a_scope)
        .unwrap();
    assert_eq!(
        value(
            &reopened
                .rubric("helpfulness", Some(&form.version))
                .await
                .unwrap()
                .unwrap()
        ),
        value(&form)
    );
    assert_eq!(
        value(
            &reopened
                .scorecard("quality", Some(&first.version))
                .await
                .unwrap()
                .unwrap()
        ),
        value(&first)
    );
    let versions = reopened
        .scorecard_versions("quality")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(versions.versions.len(), 2);
    assert!(
        reopened
            .scorecard_diff("quality", &first.version, &second.version)
            .await
            .unwrap()
            .is_some()
    );
    let history = reopened
        .assessment_history(&said.target_id, &said.standing_id, None, Some(1))
        .await
        .unwrap();
    assert_eq!(value(&history.revisions[0]), value(&updated));
    let older = reopened
        .assessment_history(
            &said.target_id,
            &said.standing_id,
            history.next_cursor,
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(value(&older.revisions[0]), value(&said));
    assert_eq!(
        b.scorecard_versions("quality")
            .await
            .unwrap()
            .unwrap()
            .versions
            .len(),
        1
    );
    assert_eq!(
        value(&b.assessments(&target()).await.unwrap().assessments[0]),
        value(&said)
    );
    assert!(a.for_project_authored(b_scope).is_err());
    assert!(a.for_project_authored(a_scope).is_ok());
    // Compare stored immutable bytes directly, not only reserialized API values.
    let root = format!(
        "evaluation-scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    for entry in store.list(&root).await.unwrap() {
        let relative = entry.key.strip_prefix(&root).unwrap();
        if relative.contains(&form.version)
            || relative.contains(&first.version)
            || relative.ends_with("/0000000001.json")
        {
            assert_eq!(
                store.get(&entry.key).await.unwrap(),
                store.get(relative).await.unwrap()
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn authored_scope_refuses_key_escape_external_catalog_and_instance_sources() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-scoped-evaluation-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = registry(store.clone());
    let a = legacy.for_project_authored(scope()).unwrap();
    for r in [&a, &legacy] {
        for invalid in ["../head", "x/../../y", "..\\head", "/absolute"] {
            assert!(r.rubric("form", Some(invalid)).await.is_err());
            assert!(r.scorecard("card", Some(invalid)).await.is_err());
            assert!(r.scorecard_diff("card", invalid, "missing").await.is_err());
            assert!(
                r.assessment_history(invalid, "standing", None, None)
                    .await
                    .is_err()
            );
            assert!(
                r.assessment_history("target", invalid, None, None)
                    .await
                    .is_err()
            );
        }
    }
    let request=serde_json::from_value(json!({"dataset":{"kind":"curation","name":"data","version":"a".repeat(64)},"split":"test","limit":1})).unwrap();
    assert!(a.derive_cohort(&request, "person", 100).await.is_err());
    assert!(a.scorer_catalog().await.is_err());
    let external: Scorecard = serde_json::from_value(
        json!({"name":"external", "scorers":[{"metric":"quality","scorer":{
            "kind":"external", "adapter":"example", "metric":"quality", "parameters":{}
        }}]}),
    )
    .unwrap();
    let refused = a
        .publish_scorecard(&external, "person", 100)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("external scorer catalog"),
        "{refused}"
    );
    assert!(
        store.list("").await.unwrap().is_empty(),
        "refused operations write nothing"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[path = "scope/recordings.rs"]
mod recording_scope;
