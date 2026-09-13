//! Proposals on their way to becoming cases, through people.
use super::*;

fn proposal(trace: &str) -> CaseProposal {
    CaseProposal {
        dataset: "capitals".into(),
        target: AssessmentTarget::Trace {
            trace_id: trace.into(),
        },
        question: "What is the capital of Kenya?".into(),
        answer: Some("Mombasa".into()),
        note: "the user said it was wrong".into(),
        assessment: None,
        content: ReviewContent::Written,
    }
}

#[tokio::test]
async fn proposing_what_is_already_under_review_lands_on_that_review() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let (first, created) = registry
        .propose_case(&proposal("trace-1"), "ada", 1000)
        .await
        .unwrap();
    assert!(created);
    let expected = registry
        .review_case(
            "capitals",
            &first.id,
            &ReviewAction::Expect {
                expected: "Nairobi".into(),
            },
            "grace",
            false,
            1100,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expected.state, ReviewState::Ready);

    let (again, created) = registry
        .propose_case(&proposal("trace-1"), "linus", 1200)
        .await
        .unwrap();
    assert!(
        !created,
        "the same trace for the same dataset is the same review"
    );
    assert_eq!(again.revision, 2, "and the work already done on it stands");
    assert_eq!(again.expected.as_deref(), Some("Nairobi"));

    registry
        .propose_case(&proposal("trace-2"), "ada", 1300)
        .await
        .unwrap();
    let approved = registry
        .review_case(
            "capitals",
            &first.id,
            &ReviewAction::Approve,
            "grace",
            false,
            1400,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(approved.decided_by.as_deref(), Some("grace"));

    let page = registry.reviews("capitals").await.unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|item| (item.state, item.revision))
            .collect::<Vec<_>>(),
        [(ReviewState::Approved, 3), (ReviewState::Proposed, 1)]
    );
    let published = registry
        .reviews_published(&page.items[..1], "v2", "grace", 1500)
        .await
        .unwrap();
    assert_eq!(published[0].published_in.as_deref(), Some("v2"));
    assert!(
        registry
            .review_case(
                "capitals",
                "nobody",
                &ReviewAction::Approve,
                "grace",
                false,
                1600
            )
            .await
            .unwrap()
            .is_none()
    );
}
