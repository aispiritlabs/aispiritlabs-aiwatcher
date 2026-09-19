//! What a lab registry has to hold, stated as the sentences it has to make
//! true. The fixtures are an in-memory object store and a scorecard, so
//! nothing here needs a bucket or a running instance.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use aiwatcher_core::{ArtifactKind, ArtifactRef, ObjectStore};
use aiwatcher_evaluation::{
    Cohort, DatasetKind, DatasetReference, Rubrics, Scorecard, VersionReference,
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_labs::{Lab, LabFilter, LabName, LabNotebook, LabTests, PublishLab, Registry};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use serde_json::json;

fn registry() -> Registry {
    Registry::new(Arc::new(MemoryObjectStore::new()), Default::default())
}

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

fn lab(name: &str, brief: &str) -> Lab {
    Lab {
        name: LabName::parse(name).unwrap(),
        title: "Answer the support questions".into(),
        brief: brief.into(),
        position: Some(3),
        notebook: None,
        tests: None,
    }
}

fn notebook(revision: char) -> LabNotebook {
    LabNotebook {
        name: "lab_03_agent".into(),
        revision: std::iter::repeat_n(revision, 64).collect(),
    }
}

fn publish(lab: Lab, label: Option<&str>) -> PublishLab {
    PublishLab {
        lab,
        notes: None,
        label: label.map(str::to_owned),
    }
}

fn tests() -> LabTests {
    LabTests {
        scorecard: VersionReference {
            name: "support-quality".into(),
            version: "b".repeat(64),
        },
        cases: "c".repeat(64),
    }
}

fn artifact(seed: char) -> ArtifactRef {
    ArtifactRef {
        name: format!("cases-{seed}"),
        kind: ArtifactKind::Rows,
        uri: format!("s3://cases/{seed}.jsonl"),
        digest: std::iter::repeat_n(seed, 64).collect(),
        size_bytes: Some(128),
        ..Default::default()
    }
}

fn cohort() -> Cohort {
    Cohort {
        case_manifest: artifact('1'),
        case_count: 12,
        split: "test".into(),
        input_schema: artifact('2'),
        expectations_schema: artifact('3'),
    }
}

fn dataset() -> DatasetReference {
    DatasetReference {
        kind: DatasetKind::Curation,
        name: "support-cases".into(),
        version: "d".repeat(64),
    }
}

fn card(scorer: serde_json::Value) -> Scorecard {
    serde_json::from_value(json!({
        "name": "support-quality",
        "scorers": [{"metric": "exact", "scorer": scorer}],
    }))
    .unwrap()
}

#[tokio::test]
async fn publishing_the_same_lab_twice_lands_on_the_version_that_is_already_there() {
    let registry = registry();
    let name = LabName::parse("lab-03").unwrap();
    let first = registry
        .publish(publish(lab("lab-03", "Build an agent."), None), None)
        .await
        .unwrap();
    assert!(first.created);
    let again = registry
        .publish(publish(lab("lab-03", "Build an agent."), None), None)
        .await
        .unwrap();
    assert!(!again.created, "the same document is one version");
    assert_eq!(again.version.version_id, first.version.version_id);
    assert_eq!(again.head.versions.len(), 1, "and one row in the index");

    let corrected = registry
        .publish(
            publish(lab("lab-03", "Build an agent, please."), None),
            None,
        )
        .await
        .unwrap();
    assert!(corrected.created);
    assert_ne!(corrected.version.version_id, first.version.version_id);
    let head = registry.head(&name).await.unwrap().unwrap();
    assert_eq!(head.versions.len(), 2);
    assert_eq!(
        head.versions.first().map(|v| v.version_id.as_str()),
        Some(corrected.version.version_id.as_str()),
        "newest first"
    );
}

#[tokio::test]
async fn an_unlabelled_publish_is_a_draft_and_the_label_is_what_a_participant_reads() {
    let registry = registry();
    let name = LabName::parse("lab-01").unwrap();
    let live = registry
        .publish(
            publish(lab("lab-01", "This week."), Some("published")),
            None,
        )
        .await
        .unwrap();
    let draft = registry
        .publish(
            publish(lab("lab-01", "Next week, still being written."), None),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        registry
            .resolve(&name, Some("published"))
            .await
            .unwrap()
            .lab
            .brief,
        "This week.",
        "a draft published after it does not move the label"
    );
    assert_eq!(
        registry.resolve(&name, None).await.unwrap().lab.brief,
        "This week.",
        "and `current` follows the label rather than the clock"
    );

    registry
        .set_label(&name, "published", &draft.version.version_id)
        .await
        .unwrap();
    assert_eq!(
        registry.resolve(&name, None).await.unwrap().version_id,
        draft.version.version_id
    );
    assert_ne!(live.version.version_id, draft.version.version_id);
}

#[tokio::test]
async fn a_label_pointing_at_a_version_this_registry_does_not_hold_is_refused() {
    let registry = registry();
    let name = LabName::parse("lab-02").unwrap();
    registry
        .publish(publish(lab("lab-02", "Read this."), None), None)
        .await
        .unwrap();
    let error = registry
        .set_label(&name, "published", &"f".repeat(64))
        .await
        .unwrap_err();
    assert!(
        matches!(error, aiwatcher_labs::LabError::UnknownVersion { .. }),
        "a label is a pointer, and one pointing at nothing is a row that 404s: {error}"
    );
    assert!(
        registry
            .set_label(&name, "published", "../secret")
            .await
            .is_err(),
        "and a version id is a digest, never a path segment a caller composes"
    );
}

#[tokio::test]
async fn labs_list_by_position_then_name_with_the_unplaced_ones_last() {
    let registry = registry();
    for (name, position) in [
        ("lab-b", Some(2)),
        ("lab-a", Some(9)),
        ("lab-c", None),
        ("lab-d", None),
    ] {
        let mut document = lab(name, "brief");
        document.position = position;
        registry
            .publish(publish(document, None), None)
            .await
            .unwrap();
    }
    let page = registry.list(&LabFilter::default()).await.unwrap();
    let order: Vec<&str> = page.labs.iter().map(|lab| lab.name.as_str()).collect();
    assert_eq!(order, ["lab-b", "lab-a", "lab-c", "lab-d"]);
    assert_eq!(page.total, 4);
    assert!(page.next_cursor.is_none());

    let page = registry
        .list(&LabFilter {
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("lab-a"));
    let rest = registry
        .list(&LabFilter {
            after: Some("lab-a".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let order: Vec<&str> = rest.labs.iter().map(|lab| lab.name.as_str()).collect();
    assert_eq!(
        order,
        ["lab-c", "lab-d"],
        "the cursor is a name, not a row number"
    );
}

#[tokio::test]
async fn a_labs_measurement_is_the_key_every_submission_will_publish_under() {
    let tests = tests();
    let card = card(json!({"kind": "exact_match"}));
    let measurement = tests
        .measurement(&dataset(), &cohort(), &card, &Rubrics::default())
        .unwrap();
    assert_eq!(measurement.context.suite, tests.scorecard);
    assert_eq!(measurement.context.case_count, 12);
    assert_eq!(measurement.context.split, "test");
    assert_eq!(
        measurement.context.metrics.first().map(|m| m.name.as_str()),
        Some("exact"),
        "the metrics come from the card, never from the lab"
    );
    assert_eq!(
        measurement.context_id,
        measurement.context.id().unwrap(),
        "and the id is the context's own"
    );

    // The same pins answer the same id: that is what makes it a join.
    let again = tests
        .measurement(&dataset(), &cohort(), &card, &Rubrics::default())
        .unwrap();
    assert_eq!(again.context_id, measurement.context_id);

    // A different card is a different measurement, even on one cohort.
    let other = LabTests {
        scorecard: VersionReference {
            name: "support-quality".into(),
            version: "e".repeat(64),
        },
        ..tests
    };
    let other = other
        .measurement(&dataset(), &cohort(), &card, &Rubrics::default())
        .unwrap();
    assert_ne!(other.context_id, measurement.context_id);
}

#[tokio::test]
async fn a_card_that_asks_a_judge_is_refused_by_name_rather_than_answered_with_a_shared_id() {
    let card = card(json!({
        "kind": "judge",
        "rubric": {"name": "helpfulness", "version": "a".repeat(64)},
    }));
    let error = tests()
        .measurement(&dataset(), &cohort(), &card, &Rubrics::default())
        .unwrap_err();
    match error {
        aiwatcher_labs::LabError::Unmeasurable { metric, reason } => {
            assert_eq!(metric, "exact");
            assert!(reason.contains("judge"), "{reason}");
        }
        other => panic!("expected the metric to be named: {other}"),
    }
}

#[tokio::test]
async fn a_project_sees_only_its_own_labs_and_identical_briefs_keep_one_version_id() {
    let store = Arc::new(MemoryObjectStore::new());
    let legacy = Registry::new(store.clone(), Default::default());
    let a_scope = scope();
    let a = legacy.for_project(a_scope).unwrap();
    let b = legacy
        .for_project(ProjectScope {
            project: ProjectId::new(),
            ..a_scope
        })
        .unwrap();
    let name = LabName::parse("lab-03").unwrap();

    let mine = a
        .publish(
            publish(lab("lab-03", "One workshop's brief."), Some("published")),
            None,
        )
        .await
        .unwrap();
    for other in [&b, &legacy] {
        assert!(other.head(&name).await.unwrap().is_none());
        assert!(
            other
                .list(&LabFilter::default())
                .await
                .unwrap()
                .labs
                .is_empty()
        );
        assert!(other.resolve(&name, None).await.is_err());
        assert!(
            other
                .set_label(&name, "published", &mine.version.version_id)
                .await
                .is_err()
        );
    }

    let theirs = b
        .publish(publish(lab("lab-03", "One workshop's brief."), None), None)
        .await
        .unwrap();
    assert_eq!(
        theirs.version.version_id, mine.version.version_id,
        "scope never enters the content hash"
    );
    assert!(theirs.created, "and the objects are separate");

    assert!(
        a.for_project(a_scope).is_ok(),
        "reopening its own scope is fine"
    );
    assert!(
        a.for_project(b.scope().unwrap()).is_err(),
        "rebinding to another project is not"
    );
    assert!(
        store
            .list(&format!(
                "labs/scopes/{}/{}/registry/",
                a_scope.organization.0, a_scope.project.0
            ))
            .await
            .unwrap()
            .len()
            >= 2
    );
}

#[tokio::test]
async fn a_lab_is_refused_before_it_is_stored_when_it_cannot_be_read_back() {
    let registry = registry();
    let mut too_long = lab("lab-04", "x");
    too_long.brief = "x".repeat(aiwatcher_labs::MAX_BRIEF_BYTES + 1);
    assert!(
        registry
            .publish(publish(too_long, None), None)
            .await
            .is_err()
    );

    let mut empty = lab("lab-04", "");
    empty.position = None;
    assert!(registry.publish(publish(empty, None), None).await.is_err());

    let mut placed_nowhere = lab("lab-04", "brief");
    placed_nowhere.position = Some(0);
    assert!(
        registry
            .publish(publish(placed_nowhere, None), None)
            .await
            .is_err()
    );

    let mut half_pinned = lab("lab-04", "brief");
    half_pinned.tests = Some(LabTests {
        cases: "not a digest".into(),
        ..tests()
    });
    assert!(
        registry
            .publish(publish(half_pinned, None), None)
            .await
            .is_err()
    );

    assert!(LabName::parse("Lab-04").is_err(), "names are slugs");
    assert!(LabName::parse("labs/../secret").is_err());
    assert!(LabName::parse("").is_err());
    assert!(
        registry
            .list(&LabFilter::default())
            .await
            .unwrap()
            .labs
            .is_empty(),
        "nothing refused was written"
    );
}

#[tokio::test]
async fn a_labs_notebook_is_part_of_what_it_is_and_a_new_pin_is_a_new_version() {
    let registry = registry();
    let name = LabName::parse("lab-03").unwrap();

    let plain = registry
        .publish(publish(lab("lab-03", "Build an agent."), None), None)
        .await
        .unwrap();
    assert_eq!(
        plain.head.versions.first().map(|v| v.has_notebook),
        Some(false),
        "a lab that hands out no notebook says so in the index"
    );

    let mut handed_out = lab("lab-03", "Build an agent.");
    handed_out.notebook = Some(notebook('a'));
    let first = registry
        .publish(publish(handed_out.clone(), None), None)
        .await
        .unwrap();
    assert!(first.created, "the same brief with a notebook is a new lab");
    assert_ne!(first.version.version_id, plain.version.version_id);
    assert_eq!(
        first.head.versions.first().map(|v| v.has_notebook),
        Some(true)
    );

    let again = registry
        .publish(publish(handed_out.clone(), None), None)
        .await
        .unwrap();
    assert!(!again.created, "and publishing it unchanged is idempotent");

    // The point of pinning: an edit to the notebook does not reach the
    // participants already on this lab until somebody publishes the new pin,
    // and when they do it is a version of its own.
    let mut edited = handed_out.clone();
    edited.notebook = Some(notebook('b'));
    let moved = registry.publish(publish(edited, None), None).await.unwrap();
    assert!(moved.created);
    assert_ne!(moved.version.version_id, first.version.version_id);

    let head = registry.head(&name).await.unwrap().unwrap();
    assert_eq!(head.versions.len(), 3);
    let stored = registry
        .version(&name, &first.version.version_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.lab.notebook.as_ref().map(|n| n.revision.as_str()),
        Some(notebook('a').revision.as_str()),
        "and an earlier version still names the source it was written against"
    );
}

#[tokio::test]
async fn a_notebook_pin_the_runtime_could_never_resolve_is_refused_by_field() {
    let registry = registry();

    for (pinned, field) in [
        (
            LabNotebook {
                name: "Lab-03".into(),
                revision: "a".repeat(64),
            },
            "notebook.name",
        ),
        (
            LabNotebook {
                name: "3_lab".into(),
                revision: "a".repeat(64),
            },
            "notebook.name",
        ),
        (
            LabNotebook {
                name: "lab_03_agent".into(),
                revision: "head".into(),
            },
            "notebook.revision",
        ),
    ] {
        let mut asked = lab("lab-03", "Build an agent.");
        asked.notebook = Some(pinned);
        let refused = registry
            .publish(publish(asked, None), None)
            .await
            .expect_err("a pin nothing could resolve is refused where the lab is written");
        assert!(
            refused.to_string().starts_with(field),
            "{refused} should name {field}"
        );
    }
}
