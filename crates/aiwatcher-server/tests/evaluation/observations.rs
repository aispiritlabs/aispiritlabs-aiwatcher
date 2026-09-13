//! What variants were observed doing, written down as periods close and read
//! back for a window the read model no longer holds.
use super::*;
use aiwatcher_projector::readmodel::ReadModelConfig;
use aiwatcher_projector::{PeriodStore, ReadModel};
use aiwatcher_server::observations::pass;

async fn served(model: &ReadModel, run_id: &str, at: time::OffsetDateTime, position: u64) {
    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source as Producer};
    for (offset, event_type) in [(0, EventType::RunStarted), (1, EventType::RunCompleted)] {
        let when = at + time::Duration::milliseconds(offset * 250);
        let mut envelope =
            EventEnvelope::new(event_type, run_id, when, Producer::new("app", Sdk::Python));
        envelope.variant_id = Some("v1".to_owned());
        let position = position * 2 + offset.unsigned_abs();
        model
            .apply(&envelope.record(position, position, when, None))
            .await;
    }
}

#[tokio::test]
async fn a_closed_period_is_written_once_and_answers_after_the_read_model_let_its_runs_go() {
    let store = PeriodStore::new(Arc::new(MemoryObjectStore::new()));
    let model = ReadModel::new(ReadModelConfig {
        max_runs: 3,
        ..ReadModelConfig::default()
    });
    let hour = time::macros::datetime!(2026-09-13 09:00:00 UTC);
    served(&model, "early", hour - time::Duration::minutes(1), 1).await;
    for (at, minute) in [(2_u64, 5), (3, 20)] {
        served(
            &model,
            &format!("r{at}"),
            hour + time::Duration::minutes(minute),
            at,
        )
        .await;
    }
    let now = hour + time::Duration::minutes(70);
    let mut known = std::collections::BTreeSet::new();

    // The hour before is where the fold began, so only the closed hour of the
    // runs is written — and once.
    assert_eq!(
        pass(&store, &model, 3_600, now, &mut known).await.unwrap(),
        1
    );
    assert_eq!(
        pass(&store, &model, 3_600, now, &mut known).await.unwrap(),
        0
    );
    assert_eq!(
        pass(
            &store,
            &model,
            3_600,
            now,
            &mut std::collections::BTreeSet::new()
        )
        .await
        .unwrap(),
        0,
        "a writer that forgot what it wrote asks the store"
    );

    // Later runs push the hour's out of the read model.
    for at in 4_u64..=7 {
        served(
            &model,
            &format!("r{at}"),
            now + time::Duration::minutes(i64::try_from(at).unwrap()),
            at,
        )
        .await;
    }
    let from = hour.unix_timestamp();
    let periods = store
        .read(
            &["v1"],
            from,
            (now + time::Duration::minutes(10)).unix_timestamp(),
        )
        .await
        .unwrap();
    assert_eq!(periods.len(), 1);
    let observed = model
        .variant_observations(&["v1"], None, &periods, None)
        .await;
    assert_eq!(
        (observed[0].runs, observed[0].periods),
        (2 + 3, 1),
        "the written hour's two, and the three the read model still holds after it"
    );
    assert!(
        observed[0]
            .duration_ms
            .as_ref()
            .is_some_and(|summary| summary.bucketed)
    );
}
