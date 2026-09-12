//! Typed judgements about one thing, in a form somebody declared.
use super::*;

fn store() -> Registry {
    registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    )
}

fn ordinal(name: &str, levels: &[&str]) -> Rubric {
    Rubric {
        name: name.into(),
        question: "did the answer help?".into(),
        guidance: String::new(),
        scale: Scale::Ordinal {
            levels: levels.iter().map(|level| (*level).to_owned()).collect(),
        },
        direction: MetricDirection::Higher,
    }
}

fn level(value: &str) -> AssessmentValue {
    AssessmentValue::Level {
        value: value.into(),
    }
}

fn about() -> AssessmentTarget {
    AssessmentTarget::Case {
        evaluation_id: "candidate-run".into(),
        case_id: "case-00001".into(),
        repetition_id: "rep-1".into(),
    }
}

fn judgement(rubric: &str, value: AssessmentValue) -> AssessmentRequest {
    AssessmentRequest {
        target: about(),
        rubric: rubric.into(),
        rubric_version: None,
        value,
        source: AssessmentSource::Human,
        author: None,
        rationale: String::new(),
    }
}

#[tokio::test]
async fn a_persons_judgement_stands_beside_the_judges_rather_than_over_it() {
    let registry = store();
    registry
        .publish_rubric(
            &ordinal("helpfulness", &["bad", "fine", "good"]),
            "ada",
            100,
        )
        .await
        .unwrap();

    let mut by_judge = judgement("helpfulness", level("good"));
    by_judge.source = AssessmentSource::Judge;
    by_judge.author = Some("gpt-4o@sha256:abc".into());
    registry.assess(&by_judge, "producer", 200).await.unwrap();
    registry
        .assess(&judgement("helpfulness", level("bad")), "ada", 300)
        .await
        .unwrap();

    let page = registry.assessments(&about()).await.unwrap();
    assert_eq!(page.assessments.len(), 2, "both judgements stand");
    let said: Vec<_> = page
        .assessments
        .iter()
        .map(|assessment| {
            (
                assessment.source,
                assessment.value.clone(),
                assessment.revision,
            )
        })
        .collect();
    assert!(said.contains(&(AssessmentSource::Human, level("bad"), 1)));
    assert!(said.contains(&(AssessmentSource::Judge, level("good"), 1)));
    // And the judge's is still attributed to the judge, filed by whoever ran it.
    let judged = page
        .assessments
        .iter()
        .find(|assessment| assessment.source == AssessmentSource::Judge)
        .unwrap();
    assert_eq!(judged.author, "gpt-4o@sha256:abc");
    assert_eq!(judged.recorded_by, "producer");
}

#[tokio::test]
async fn changing_your_mind_is_another_revision_rather_than_an_edit() {
    let registry = store();
    registry
        .publish_rubric(
            &ordinal("helpfulness", &["bad", "fine", "good"]),
            "ada",
            100,
        )
        .await
        .unwrap();
    let first = registry
        .assess(&judgement("helpfulness", level("bad")), "ada", 200)
        .await
        .unwrap();
    let second = registry
        .assess(&judgement("helpfulness", level("good")), "ada", 300)
        .await
        .unwrap();

    assert_eq!((first.revision, second.revision), (1, 2));
    assert_eq!(
        first.standing_id, second.standing_id,
        "one person on one rubric about one thing is one standing judgement"
    );
    let page = registry.assessments(&about()).await.unwrap();
    assert_eq!(page.assessments.len(), 1, "the current one, not both");
    assert_eq!(page.assessments[0].value, level("good"));

    let history = registry
        .assessment_history(&first.target_id, &first.standing_id, None, None)
        .await
        .unwrap();
    let said: Vec<_> = history
        .revisions
        .iter()
        .map(|revision| (revision.revision, revision.value.clone()))
        .collect();
    assert_eq!(said, vec![(2, level("good")), (1, level("bad"))]);
}

#[tokio::test]
async fn rewriting_a_rubric_leaves_what_was_already_said_reading_as_it_did() {
    let registry = store();
    let first = registry
        .publish_rubric(
            &ordinal("helpfulness", &["bad", "fine", "good"]),
            "ada",
            100,
        )
        .await
        .unwrap();
    registry
        .assess(&judgement("helpfulness", level("good")), "ada", 200)
        .await
        .unwrap();

    // The same name, a different set of levels: a new version, and the head
    // moves to it. `good` is not even on the new scale.
    let second = registry
        .publish_rubric(
            &ordinal("helpfulness", &["unhelpful", "helpful"]),
            "ada",
            300,
        )
        .await
        .unwrap();
    assert_ne!(first.version, second.version);
    assert_eq!(
        registry
            .rubric("helpfulness", None)
            .await
            .unwrap()
            .unwrap()
            .version,
        second.version
    );

    let page = registry.assessments(&about()).await.unwrap();
    assert_eq!(
        page.assessments[0].rubric_version, first.version,
        "what was said keeps the version it was said under"
    );
    let read_back = registry
        .rubric("helpfulness", Some(&first.version))
        .await
        .unwrap()
        .expect("an earlier version stays readable");
    assert_eq!(
        read_back.rubric.scale,
        Scale::Ordinal {
            levels: vec!["bad".into(), "fine".into(), "good".into()]
        }
    );

    // And a new judgement takes the head, which no longer admits the old word.
    let refused = registry
        .assess(&judgement("helpfulness", level("good")), "ada", 400)
        .await
        .expect_err("the current scale has no `good` on it");
    assert!(refused.to_string().contains("unhelpful, helpful"));
}

#[tokio::test]
async fn a_judgement_is_refused_before_it_is_written_when_nothing_declares_its_form() {
    let registry = store();
    let refused = registry
        .assess(&judgement("helpfulness", level("good")), "ada", 100)
        .await
        .expect_err("no rubric of that name exists");
    assert!(refused.to_string().contains("no rubric of that name"));
    assert!(
        registry
            .assessments(&about())
            .await
            .unwrap()
            .assessments
            .is_empty(),
        "a refused judgement leaves nothing behind"
    );
}

#[tokio::test]
async fn a_client_cannot_file_a_judgement_under_somebody_elses_name() {
    let registry = store();
    registry
        .publish_rubric(&ordinal("helpfulness", &["bad", "good"]), "ada", 100)
        .await
        .unwrap();
    let mut impersonating = judgement("helpfulness", level("good"));
    impersonating.author = Some("grace".into());
    let refused = registry
        .assess(&impersonating, "ada", 200)
        .await
        .expect_err("a person's judgement is the session's");
    assert!(refused.to_string().contains("author"));

    // Filed without one, it is attributed to the caller and to nobody else.
    let recorded = registry
        .assess(&judgement("helpfulness", level("good")), "ada", 300)
        .await
        .unwrap();
    assert_eq!(
        (recorded.author.as_str(), recorded.recorded_by.as_str()),
        ("ada", "ada")
    );
}

#[tokio::test]
async fn one_session_judged_at_two_moments_is_two_things_rather_than_a_revision() {
    let registry = store();
    registry
        .publish_rubric(&ordinal("helpfulness", &["bad", "good"]), "ada", 100)
        .await
        .unwrap();
    let snapshot = |as_of| AssessmentTarget::Session {
        session_id: "session-7".into(),
        as_of,
    };
    for as_of in [1_000, 2_000] {
        let mut request = judgement("helpfulness", level("good"));
        request.target = snapshot(as_of);
        registry.assess(&request, "ada", as_of).await.unwrap();
    }

    let earlier = registry.assessments(&snapshot(1_000)).await.unwrap();
    let later = registry.assessments(&snapshot(2_000)).await.unwrap();
    assert_ne!(earlier.target_id, later.target_id);
    assert_eq!(earlier.assessments.len(), 1);
    assert_eq!(later.assessments.len(), 1);
    assert_eq!(
        earlier.assessments[0].revision, 1,
        "neither revised the other"
    );
}

#[tokio::test]
async fn a_judge_that_rescores_every_night_pages_its_own_history() {
    let registry = store();
    registry
        .publish_rubric(&ordinal("helpfulness", &["bad", "good"]), "ada", 100)
        .await
        .unwrap();
    let mut nightly = judgement("helpfulness", level("good"));
    nightly.source = AssessmentSource::Judge;
    nightly.author = Some("nightly".into());
    let mut about = (String::new(), String::new());
    for night in 1..=5 {
        nightly.value = level(if night % 2 == 0 { "bad" } else { "good" });
        let written = registry
            .assess(&nightly, "worker", night * 86_400)
            .await
            .unwrap();
        about = (written.target_id, written.standing_id);
    }
    let (target_id, standing) = about;

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = registry
            .assessment_history(&target_id, &standing, cursor, Some(2))
            .await
            .unwrap();
        seen.extend(page.revisions.iter().map(|revision| revision.revision));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, vec![5, 4, 3, 2, 1], "newest first, and none skipped");
}

#[tokio::test]
async fn publishing_one_form_twice_lands_on_the_version_that_is_already_there() {
    let registry = store();
    let first = registry
        .publish_rubric(&ordinal("helpfulness", &["bad", "good"]), "ada", 100)
        .await
        .unwrap();
    let again = registry
        .publish_rubric(&ordinal("helpfulness", &["bad", "good"]), "grace", 200)
        .await
        .unwrap();
    assert_eq!(first.version, again.version);
    assert_eq!(
        again.published_by, "ada",
        "the version is what was written first; publishing it again is not authorship"
    );
    assert_eq!(registry.rubrics().await.unwrap().len(), 1);
}
