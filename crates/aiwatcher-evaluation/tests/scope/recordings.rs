use super::*;
use aiwatcher_evaluation::{EvaluationError, EvidenceState};

#[tokio::test]
async fn project_recordings_preserve_exact_bytes_after_reopen_and_verify_local_digests() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-project-recordings-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = registry(store.clone());
    let project = scope();
    let a = legacy.for_project_authored(project).unwrap();
    let b = legacy
        .for_project_authored(ProjectScope {
            project: ProjectId::new(),
            ..project
        })
        .unwrap();
    let c = legacy
        .for_project_authored(ProjectScope {
            organization: OrganizationId::new(),
            ..project
        })
        .unwrap();
    let bytes =
        b"{\n\"answers\":[{\"case_id\":\"first\",\"answer\":1.00000000000000001}]}\n".to_vec();
    let artifact = a
        .stage_recording("answers/test", bytes.clone())
        .await
        .unwrap();
    assert_eq!(
        value(
            &a.stage_recording("answers/test", bytes.clone())
                .await
                .unwrap()
        ),
        value(&artifact)
    );
    assert_eq!(artifact.size_bytes, Some(bytes.len() as u64));
    assert_eq!(
        artifact.uri,
        format!("evaluation://recordings/{}", artifact.digest)
    );
    for other in [&b, &c, &legacy] {
        assert!(matches!(
            other.recording_bytes(&artifact.digest).await,
            Err(EvaluationError::Unavailable(EvidenceState::MissingArtifact))
        ));
        assert!(other.recording(&artifact).await.is_err());
        assert_eq!(
            value(
                &other
                    .stage_recording("answers/test", bytes.clone())
                    .await
                    .unwrap()
            ),
            value(&artifact)
        );
    }
    let reopened = registry(Arc::new(FileObjectStore::open(&dir).await.unwrap()))
        .for_project_authored(project)
        .unwrap();
    assert_eq!(
        reopened.recording_bytes(&artifact.digest).await.unwrap(),
        bytes
    );
    assert_eq!(
        reopened.recording(&artifact).await.unwrap().answers[0].case_id,
        "first"
    );
    assert!(reopened.for_project_authored(scope()).is_err());
    let key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-recordings/{}.json",
        project.organization.0, project.project.0, artifact.digest
    );
    assert_eq!(store.get(&key).await.unwrap().unwrap(), bytes);
    store.put(&key, b"{\"answers\":[]}".to_vec()).await.unwrap();
    assert!(matches!(
        reopened.recording_bytes(&artifact.digest).await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    assert!(reopened.recording(&artifact).await.is_err());
    assert_eq!(b.recording_bytes(&artifact.digest).await.unwrap(), bytes);
    assert_eq!(
        legacy.recording_bytes(&artifact.digest).await.unwrap(),
        bytes
    );
    store.delete(&key).await.unwrap();
    assert!(matches!(
        reopened.recording_bytes(&artifact.digest).await,
        Err(EvaluationError::Unavailable(EvidenceState::MissingArtifact))
    ));
    for r in [&reopened, &legacy] {
        for invalid in ["../private", "..\\private", "", "not-a-digest"] {
            assert!(matches!(
                r.recording_bytes(invalid).await,
                Err(EvaluationError::Invalid { .. })
            ));
            let mut invalid_ref = artifact.clone();
            invalid_ref.digest = invalid.to_owned();
            assert!(matches!(
                r.recording(&invalid_ref).await,
                Err(EvaluationError::Invalid { .. })
            ));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
