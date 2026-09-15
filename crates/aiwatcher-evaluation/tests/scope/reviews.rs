use super::*;
use aiwatcher_evaluation::{CaseProposal, ReviewAction};

fn proposal() -> CaseProposal {
    serde_json::from_value(json!({"dataset":"golden/cases", "target":target(),
        "question":"reviewer question", "answer":"incorrect answer", "content":"written",
        "note":"why it matters", "assessment":"standing-reference", "split":"test"}))
    .unwrap()
}

#[tokio::test]
async fn review_files_keep_identity_history_and_target_index_after_reopen() {
    let dir = std::env::temp_dir().join(format!(
        "aiwatcher-scoped-review-{}",
        OrganizationId::new().0
    ));
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let legacy = registry(store.clone());
    let a_scope = scope();
    let a = legacy.for_project_authored(a_scope).unwrap();
    let b = legacy
        .for_project_authored(ProjectScope {
            project: ProjectId::new(),
            ..a_scope
        })
        .unwrap();
    let c = legacy.for_project_authored(scope()).unwrap();
    let proposal = proposal();
    let (first, created) = a.propose_case(&proposal, "author", 100).await.unwrap();
    assert!(created);
    for other in [&b, &c, &legacy] {
        assert!(
            other
                .reviews(&proposal.dataset)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            other
                .reviews_of(&proposal.target)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            other
                .review_case(
                    &proposal.dataset,
                    &first.id,
                    &ReviewAction::Approve,
                    "reviewer",
                    true,
                    101
                )
                .await
                .unwrap()
                .is_none()
        );
    }
    // Same serialized revisions in separate stores: the scope is not part of identity.
    for other in [&b, &legacy] {
        assert_eq!(
            other
                .propose_case(&proposal, "author", 100)
                .await
                .unwrap()
                .0,
            first
        );
    }
    for r in [&a, &b, &legacy] {
        r.review_case(
            &proposal.dataset,
            &first.id,
            &ReviewAction::Expect {
                expected: "verified answer".into(),
                split: None,
            },
            "reviewer",
            false,
            101,
        )
        .await
        .unwrap()
        .unwrap();
        r.review_case(
            &proposal.dataset,
            &first.id,
            &ReviewAction::Approve,
            "reviewer",
            false,
            102,
        )
        .await
        .unwrap()
        .unwrap();
    }
    let approved = a.reviews(&proposal.dataset).await.unwrap().items;
    let published = a
        .reviews_published(&approved, &"a".repeat(64), "publisher", 103)
        .await
        .unwrap();
    assert_eq!(published[0].revision, 4);
    let reopened = registry(Arc::new(FileObjectStore::open(&dir).await.unwrap()))
        .for_project_authored(a_scope)
        .unwrap();
    assert_eq!(
        reopened.reviews(&proposal.dataset).await.unwrap().items,
        published
    );
    assert_eq!(
        reopened.reviews_of(&proposal.target).await.unwrap().items,
        published
    );
    assert_eq!(b.reviews(&proposal.dataset).await.unwrap().items, approved);
    assert_eq!(
        legacy.reviews(&proposal.dataset).await.unwrap().items,
        approved
    );
    assert!(
        c.reviews_of(&proposal.target)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let root = format!(
        "evaluation-scopes/{}/{}/registry/",
        a_scope.organization.0, a_scope.project.0
    );
    let mut checked = 0;
    for entry in store.list(&root).await.unwrap() {
        let relative = entry.key.strip_prefix(&root).unwrap();
        if !relative.ends_with("/0000000004.json") {
            let original = store.get(relative).await.unwrap();
            assert!(original.is_some());
            assert_eq!(store.get(&entry.key).await.unwrap(), original);
            checked += 1;
        }
    }
    assert_eq!(checked, 4, "three revisions and the target index");
    for r in [&a, &legacy] {
        for id in ["../escape", "x/../../y", "..\\escape"] {
            assert!(
                r.review_case(
                    &proposal.dataset,
                    id,
                    &ReviewAction::Approve,
                    "author",
                    true,
                    104
                )
                .await
                .is_err()
            );
        }
    }
    // Missing case source never reaches the global resolver, even with a known target.
    let mut from_result = proposal.clone();
    from_result.question = None;
    from_result.at = Some("known-cursor".into());
    assert!(a.propose_case(&from_result, "author", 104).await.is_err());
    from_result.question = Some("unverified".into());
    from_result.content = Some(aiwatcher_evaluation::ReviewContent::Measured);
    assert!(a.propose_case(&from_result, "author", 104).await.is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Debug)]
struct WrongListing;
#[async_trait]
impl ObjectStore for WrongListing {
    async fn put(&self, _: &str, _: Vec<u8>) -> aiwatcher_core::ports::PortResult<()> {
        panic!("unexpected write")
    }
    async fn get(&self, _: &str) -> aiwatcher_core::ports::PortResult<Option<Vec<u8>>> {
        panic!("foreign key must be rejected before read")
    }
    async fn list(
        &self,
        _: &str,
    ) -> aiwatcher_core::ports::PortResult<Vec<aiwatcher_core::storage::ObjectEntry>> {
        Ok(vec![aiwatcher_core::storage::ObjectEntry {
            key: "evaluation-reviews/foreign/id/0000000001.json".into(),
            size: 1,
            last_modified: None,
        }])
    }
    async fn delete(&self, _: &str) -> aiwatcher_core::ports::PortResult<()> {
        panic!("unexpected delete")
    }
}

#[tokio::test]
async fn project_reviews_reject_foreign_listing_before_loading_bytes() {
    let r = registry(Arc::new(WrongListing))
        .for_project_authored(scope())
        .unwrap();
    assert!(r.reviews("golden/cases").await.is_err());
    assert!(r.reviews_of(&target()).await.is_err());
    assert!(r.propose_case(&proposal(), "author", 100).await.is_err());
}

#[tokio::test]
async fn publication_cannot_mark_a_later_review_revision_or_another_version() {
    let store = Arc::new(aiwatcher_prompts::adapters::memory::MemoryObjectStore::new());
    let legacy = registry(store);
    let scoped = legacy.for_project_authored(scope()).unwrap();
    for r in [&legacy, &scoped] {
        let proposal = proposal();
        let (first, _) = r.propose_case(&proposal, "author", 100).await.unwrap();
        let expect = ReviewAction::Expect {
            expected: "approved words".into(),
            split: None,
        };
        r.review_case(
            &proposal.dataset,
            &first.id,
            &expect,
            "reviewer",
            false,
            101,
        )
        .await
        .unwrap();
        let approved = r
            .review_case(
                &proposal.dataset,
                &first.id,
                &ReviewAction::Approve,
                "reviewer",
                false,
                102,
            )
            .await
            .unwrap()
            .unwrap();
        let changed = r
            .review_case(
                &proposal.dataset,
                &first.id,
                &ReviewAction::Expect {
                    expected: "later words".into(),
                    split: None,
                },
                "reviewer",
                false,
                103,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            r.reviews_published(&[approved], "version-a", "publisher", 104)
                .await,
            Err(aiwatcher_evaluation::EvaluationError::Contested)
        ));
        assert_eq!(
            r.reviews(&proposal.dataset).await.unwrap().items,
            vec![changed]
        );
        let approved = r
            .review_case(
                &proposal.dataset,
                &first.id,
                &ReviewAction::Approve,
                "reviewer",
                false,
                105,
            )
            .await
            .unwrap()
            .unwrap();
        let snapshot = vec![approved];
        let published = r
            .reviews_published(&snapshot, "version-b", "publisher", 106)
            .await
            .unwrap();
        assert_eq!(
            r.reviews_published(&snapshot, "version-b", "publisher", 107)
                .await
                .unwrap(),
            published
        );
        assert!(matches!(
            r.reviews_published(&snapshot, "version-c", "publisher", 108)
                .await,
            Err(aiwatcher_evaluation::EvaluationError::Contested)
        ));
    }
}
