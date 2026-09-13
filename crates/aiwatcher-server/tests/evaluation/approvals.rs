//! Approval as a resource: many pairs at once, recorded, and withdrawable.
use super::*;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_server::evaluation::LocalSource;
use std::path::{Path, PathBuf};

/// One bundle per pair, in a directory named by the pair it admits. This is the
/// operator's side of the act; the record is the instance's.
async fn bundle(root: &Path, experiment: &str) -> PublishEvaluation {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures/evaluation-v1");
    let mut request = request(&format!("eval-{experiment}"), 3);
    request.manifest.variant.experiment_id = experiment.into();
    for (n, case) in request.cases.iter_mut().enumerate() {
        case.case_id = ["capital-pl", "two-plus-two", "empty"][n].into();
    }
    let prepared = Evaluation::prepare(request.manifest.clone()).unwrap();
    let directory = root.join(
        aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id()).unwrap(),
    );
    tokio::fs::create_dir_all(&directory).await.unwrap();
    for entry in std::fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            tokio::fs::copy(entry.path(), directory.join(entry.file_name()))
                .await
                .unwrap();
        }
    }
    tokio::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&request.manifest).unwrap(),
    )
    .await
    .unwrap();
    request
}

fn expected(case: &str) -> serde_json::Value {
    serde_json::json!({
        "answer": match case {
            "capital-pl" => "Warsaw",
            "two-plus-two" => "4",
            _ => "",
        }
    })
}

async fn state(registry: &Registry, id: &str, now: i64) -> EvidenceState {
    registry
        .get(id, "viewer", now)
        .await
        .unwrap()
        .unwrap()
        .state
}

#[tokio::test]
async fn two_variants_of_one_suite_are_readable_at_once_and_a_third_disturbs_neither() {
    let root = std::env::temp_dir().join(format!("aiwatcher-approvals-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = Registry::new(
        store.clone(),
        Arc::new(LocalSource::new(Some(root.to_str().unwrap().into()))),
        RegistryConfig::default(),
    )
    .unwrap();

    let mut receipts = Vec::new();
    for experiment in ["baseline", "candidate"] {
        let mut request = bundle(&root, experiment).await;
        for case in &mut request.cases {
            case.actual = Some(expected(&case.case_id));
        }
        registry
            .approve(&request.manifest, "operator", 100)
            .await
            .unwrap();
        receipts.push(registry.publish(request, "producer", 100).await.unwrap());
    }
    let (baseline, candidate) = (&receipts[0], &receipts[1]);
    assert_ne!(baseline.variant_id, candidate.variant_id);
    assert_eq!(
        baseline.context_id, candidate.context_id,
        "one suite on one cohort: only the variant differs"
    );
    for receipt in &receipts {
        assert_eq!(
            state(&registry, &receipt.evaluation_id, 200).await,
            EvidenceState::Complete,
            "swapping one approved bundle in used to hide the other"
        );
    }

    // A third approval is a third directory and a third record. Nothing about
    // the two already published moves — not their state, not their deadline.
    let mut third = bundle(&root, "third").await;
    for case in &mut third.cases {
        case.actual = Some(expected(&case.case_id));
    }
    registry
        .approve(&third.manifest, "operator", 300)
        .await
        .unwrap();
    registry.publish(third, "producer", 300).await.unwrap();
    for receipt in &receipts {
        let detail = registry
            .get(&receipt.evaluation_id, "viewer", 400)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.state, EvidenceState::Complete);
        assert_eq!(&detail.receipt, receipt);
    }
    assert_eq!(registry.approvals().await.unwrap().len(), 3);

    // A producer publishes another repetition of an admitted pair with nothing
    // placed on this host and nobody approving again.
    let mut repeated = bundle(&root, "baseline").await;
    repeated.manifest.origin.evaluation_id = "eval-baseline-again".into();
    for case in &mut repeated.cases {
        case.actual = Some(expected(&case.case_id));
    }
    tokio::fs::remove_file(root.join("does-not-exist"))
        .await
        .ok();
    registry.publish(repeated, "worker", 500).await.unwrap();
    assert_eq!(
        state(&registry, "eval-baseline-again", 500).await,
        EvidenceState::Complete
    );

    // Withdrawal hides every result measured under that pair and refuses the
    // next one, and moves no deadline in either direction.
    let approval =
        aiwatcher_evaluation::approval_id(&candidate.variant_id, &candidate.context_id).unwrap();
    let withdrawn = registry
        .withdraw(&approval, "operator", 600)
        .await
        .unwrap()
        .unwrap();
    assert!(!withdrawn.admits());
    assert_eq!(withdrawn.record.approved_by, "operator");
    let hidden = registry
        .get(&candidate.evaluation_id, "viewer", 700)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hidden.state, EvidenceState::Forbidden);
    assert!(hidden.manifest.is_none() && hidden.metrics.is_empty());
    assert_eq!(
        hidden.receipt.expires_at, candidate.expires_at,
        "withdrawal hides; retention is the clock it must not touch"
    );
    assert_eq!(
        state(&registry, &baseline.evaluation_id, 700).await,
        EvidenceState::Complete,
        "withdrawing one pair says nothing about another"
    );
    let mut refused = bundle(&root, "candidate").await;
    refused.manifest.origin.evaluation_id = "eval-candidate-again".into();
    assert!(matches!(
        registry.publish(refused.clone(), "producer", 800).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(
        registry
            .approve(&refused.manifest, "operator", 800)
            .await
            .is_err(),
        "a withdrawn pair is not admitted again under the same ID"
    );
    assert!(
        registry
            .withdraw("not-an-approval", "operator", 900)
            .await
            .unwrap()
            .is_none()
    );

    // Forgetting one result is the third way evidence goes, and the narrowest.
    assert!(registry.forget(&baseline.evaluation_id).await.unwrap());
    assert_eq!(
        state(&registry, &baseline.evaluation_id, 900).await,
        EvidenceState::DeletedSource
    );
    assert_eq!(
        state(&registry, "eval-baseline-again", 900).await,
        EvidenceState::Complete,
        "one measurement, not every measurement of its pair"
    );
    assert!(!registry.forget("never-published").await.unwrap());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn an_admitted_pair_refuses_a_bundle_that_changed_underneath_it() {
    let root =
        std::env::temp_dir().join(format!("aiwatcher-approval-drift-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = Registry::new(
        store,
        Arc::new(LocalSource::new(Some(root.to_str().unwrap().into()))),
        RegistryConfig::default(),
    )
    .unwrap();
    let mut request = bundle(&root, "drift").await;
    for case in &mut request.cases {
        case.actual = Some(expected(&case.case_id));
    }
    registry
        .approve(&request.manifest, "operator", 100)
        .await
        .unwrap();
    // A pair nobody admitted is refused, and says so rather than reporting a
    // source somebody deleted: no earlier evidence ever pointed at it. It names
    // the approval, because admitting it is the one thing to do next.
    let mut unapproved = request.clone();
    unapproved.manifest.variant.experiment_id = "never-admitted".into();
    unapproved.manifest.origin.evaluation_id = "eval-never-admitted".into();
    let prepared = aiwatcher_evaluation::Evaluation::prepare(unapproved.manifest.clone()).unwrap();
    let expected =
        aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id()).unwrap();
    assert!(matches!(
        registry.publish(unapproved, "producer", 100).await,
        Err(EvaluationError::NotAdmitted(named)) if named == expected
    ));
    registry
        .publish(request.clone(), "producer", 100)
        .await
        .unwrap();
    tokio::fs::remove_dir_all(root).await.unwrap();
}

/// A source whose digests the test sets: what an adapter reports as the bytes a
/// bundle adds, and what it would have reported before it digested only those.
#[derive(Debug, Default)]
struct Digests(std::sync::Mutex<(Option<String>, Option<String>)>);

impl Digests {
    fn report(&self, current: Option<&str>, earlier: Option<&str>) {
        *self.0.lock().unwrap() = (current.map(Into::into), earlier.map(Into::into));
    }
}

#[async_trait]
impl SourceAuthority for Digests {
    async fn resolve(&self, manifest: &EvaluationManifest, _: &str) -> Result<SourceEvidence> {
        let (bundle_digest, earlier_bundle_digest) = self.0.lock().unwrap().clone();
        Ok(SourceEvidence {
            expected: (0..manifest.context.case_count)
                .map(|n| (format!("case-{n:05}"), serde_json::json!({"answer": ""})))
                .collect(),
            bundle_digest,
            earlier_bundle_digest,
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn an_approval_recorded_over_a_whole_declaration_still_admits_until_its_bytes_change() {
    let source = Arc::new(Digests::default());
    let registry = Registry::new(
        Arc::new(MemoryObjectStore::new()),
        source.clone(),
        RegistryConfig::default(),
    )
    .unwrap();
    let request = request("admitted-before", 2);

    // Admitted by the adapter as it was: the digest of the whole declaration.
    source.report(Some("whole-declaration"), None);
    let approval = registry
        .approve(&request.manifest, "operator", 100)
        .await
        .unwrap();
    registry
        .publish(request.clone(), "producer", 100)
        .await
        .unwrap();

    // The adapter now digests only what a bundle adds, and adds nothing here;
    // the bytes that approval covered are still the ones staged.
    source.report(None, Some("whole-declaration"));
    assert_eq!(
        state(&registry, "admitted-before", 101).await,
        EvidenceState::Complete
    );
    registry
        .approve(&request.manifest, "operator", 101)
        .await
        .expect("admitting it again is the approval it already has");

    // Those bytes changed: the pair stops reading, and admitting it again says
    // which approval holds it rather than reporting two results under one ID.
    source.report(None, Some("another-declaration"));
    assert_eq!(
        state(&registry, "admitted-before", 102).await,
        EvidenceState::Forbidden
    );
    let refused = registry
        .approve(&request.manifest, "operator", 102)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, EvaluationError::AdmittedOtherBytes(named) if *named == approval.record.approval_id),
        "{refused}"
    );
    assert!(refused.to_string().contains("stage the bytes it admitted"));
}

/// What a line stages for a variant naming a model and a workflow: the package
/// as the training registry holds it, and the bytes nobody here holds by the
/// digests their owners pinned.
#[tokio::test]
async fn a_model_and_a_workflow_imply_the_members_a_line_stages_and_where_their_bytes_come_from() {
    use aiwatcher_evaluation::{ApprovalBundles, PinnedMember};
    let training = Arc::new(aiwatcher_training::Registry::new(
        Arc::new(MemoryObjectStore::new()),
        "training",
    ));
    training
        .start(
            serde_json::from_value(serde_json::json!({
                "run_id": "run", "model": "capitals", "dataset": "capitals@abc"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let weights = "ab".repeat(32);
    let registered = training
        .register_model(
            serde_json::from_value(serde_json::json!({
                "name": "capitals", "run_id": "run", "checkpoint_uri": "s3://models/capitals",
                "package": {
                    "runtime": "weights",
                    "artifacts": [{
                        "name": "weights", "uri": "s3://models/capitals.bin", "digest": weights,
                        "size_bytes": 12, "content_type": "", "kind": "model"
                    }]
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let source = LocalSource::new(None).with_training(training);
    let mut variant = request("line", 1).manifest.variant;
    variant.model = Some(VersionReference {
        name: "capitals".into(),
        version: registered.version.version.clone(),
    });

    let members = source.pinned_members(&variant).await.unwrap();

    let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "model-package.json",
            "model-artifacts/weights",
            "workflow.json"
        ]
    );
    let package: aiwatcher_training::ModelPackage =
        serde_json::from_slice(members[0].bytes.as_ref().expect("derived from its owner")).unwrap();
    assert_eq!(package.artifacts[0].digest, weights);
    assert_eq!(
        members[1],
        PinnedMember {
            name: "model-artifacts/weights".into(),
            digest: weights,
            size_bytes: Some(12),
            bytes: None,
        },
        "weights are nobody's here to derive: a pipeline sends them by digest"
    );
    assert_eq!(
        members[2].digest,
        variant.workflow.as_ref().unwrap().version,
        "a workflow's declaration, by the digest the variant pins"
    );
}
