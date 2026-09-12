//! The catalogue is a published order, and the claims behind it are the truth.
use super::*;

fn ids(page: &aiwatcher_evaluation::DurablePage) -> Vec<String> {
    page.evaluations
        .iter()
        .map(|evidence| evidence.receipt.evaluation_id.clone())
        .collect()
}

#[tokio::test]
async fn the_catalogue_is_newest_first_paged_by_period_and_backfilled_for_older_evidence() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = registry(store.clone(), Arc::new(Source::default()));
    for (id, at) in [("first", 1000), ("second", 4600), ("third", 8200)] {
        publish(&registry, request(id, 3), "editor", at)
            .await
            .unwrap();
    }

    let page = registry
        .list(None, 200, None, None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&page), ["third", "second", "first"]);

    // A period is a bound on the key rather than a filter over a scan, which
    // is the whole reason the screen may have one at all.
    let recent = registry
        .list(None, 200, Some(5000), None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&recent), ["third", "second"]);

    let head = registry
        .list(None, 2, None, None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&head), ["third", "second"]);
    let rest = registry
        .list(head.next_cursor.as_deref(), 2, None, None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&rest), ["first"]);
    assert!(rest.next_cursor.is_none());

    // Evidence published before this instance kept a catalogue. The order is
    // derived, so losing it loses no evidence and the pass puts it back.
    for entry in store.list("evaluations/index/").await.unwrap() {
        store.delete(&entry.key).await.unwrap();
    }
    assert!(
        registry
            .list(None, 200, None, None, "viewer", 9000)
            .await
            .unwrap()
            .evaluations
            .is_empty()
    );
    for id in ["first", "second", "third"] {
        assert_eq!(
            registry
                .get(id, "viewer", 9000)
                .await
                .unwrap()
                .unwrap()
                .state,
            EvidenceState::Complete,
            "a detail read goes through the claim, not the catalogue"
        );
    }
    registry.collect_orphans(9000).await.unwrap();
    let rebuilt = registry
        .list(None, 200, None, None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&rebuilt), ["third", "second", "first"]);

    // A forgotten result keeps its row and says why: a catalogue it vanished
    // from would say it had never been published.
    assert!(registry.forget("second").await.unwrap());
    let after = registry
        .list(None, 200, None, None, "viewer", 9000)
        .await
        .unwrap();
    assert_eq!(ids(&after), ["third", "second", "first"]);
    assert_eq!(after.evaluations[1].state, EvidenceState::DeletedSource);
    assert!(after.evaluations[1].metrics.is_empty());
    assert!(after.evaluations[1].manifest.is_none());
}
