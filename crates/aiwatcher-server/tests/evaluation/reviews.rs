//! Proposals on their way to becoming cases, through people.
use super::*;

fn proposal(trace: &str) -> CaseProposal {
    CaseProposal {
        dataset: "capitals".into(),
        target: AssessmentTarget::Trace {
            trace_id: trace.into(),
        },
        question: Some("What is the capital of Kenya?".into()),
        answer: Some("Mombasa".into()),
        at: None,
        note: "the user said it was wrong".into(),
        assessment: None,
        content: Some(ReviewContent::Written),
        split: None,
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
                split: None,
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

/// A case of a published result holds its own words: proposing it at the
/// position a comparison row carries reads the question its cohort asked and
/// what the variant answered, and the case's judgements can find the review.
#[tokio::test]
async fn a_result_s_case_is_proposed_in_its_own_words_and_found_from_the_case() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let mut request = request("answers-v2", 3);
    request.cases[1].actual = Some(serde_json::json!("Mombasa"));
    let receipt = publish(&registry, request, "ci", 1000).await.unwrap();
    // Where case-00001 sits, in the words the case route speaks.
    let at = registry
        .cases(
            "answers-v2",
            &receipt.version,
            None,
            Some(1),
            "reader",
            1000,
        )
        .await
        .unwrap()
        .unwrap()
        .next_cursor
        .expect("a second case");
    let target = AssessmentTarget::Case {
        evaluation_id: "answers-v2".into(),
        case_id: "case-00001".into(),
        repetition_id: "measurement-1".into(),
    };
    let from_case = |dataset: &str, at: &str| CaseProposal {
        dataset: dataset.into(),
        target: target.clone(),
        question: None,
        answer: None,
        at: Some(at.into()),
        note: "judged wrong".into(),
        assessment: Some("standing-1".into()),
        content: None,
        split: None,
    };

    let (review, created) = registry
        .propose_case(&from_case("capitals", &at), "ada", 1100)
        .await
        .unwrap();

    assert!(created);
    assert_eq!(review.question, "question 1", "what the cohort asked");
    assert_eq!(
        review.answer.as_deref(),
        Some("Mombasa"),
        "what the variant answered"
    );
    assert_eq!(review.content, ReviewContent::Measured);
    let approved = registry
        .review_case(
            "capitals",
            &review.id,
            &ReviewAction::Expect {
                expected: "Nairobi".into(),
                split: None,
            },
            "grace",
            false,
            1150,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        registry
            .review_case(
                "capitals",
                &approved.id,
                &ReviewAction::Approve,
                "grace",
                false,
                1160
            )
            .await
            .is_ok(),
        "a result's words need no admin to become a case"
    );

    registry
        .propose_case(&from_case("regressions", &at), "linus", 1200)
        .await
        .unwrap();
    let found = registry.reviews_of(&target).await.unwrap();
    assert_eq!(
        found
            .items
            .iter()
            .map(|item| (item.dataset.as_str(), item.state))
            .collect::<Vec<_>>(),
        [
            ("capitals", ReviewState::Approved),
            ("regressions", ReviewState::Proposed)
        ],
        "every review of the case, whichever dataset it joins"
    );

    // A position that is some other case is not this one's words.
    let first = format!("{}:0", receipt.version);
    let refused = registry
        .propose_case(&from_case("elsewhere", &first), "ada", 1300)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("does not point at case-00001"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_trace_holds_no_words_so_a_proposal_of_one_writes_its_question() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let mut unwritten = proposal("trace-9");
    unwritten.question = None;
    let refused = registry
        .propose_case(&unwritten, "ada", 1000)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("write the question"),
        "{refused}"
    );

    let mut unowned = proposal("trace-9");
    unowned.content = None;
    let refused = registry
        .propose_case(&unowned, "ada", 1000)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("whose words"), "{refused}");
}
