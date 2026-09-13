//! What an evaluation measures, declared before anything runs it.
use super::*;

fn store() -> Registry {
    registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    )
}

fn spec(metric: &str, scorer: Scorer) -> ScorerSpec {
    ScorerSpec {
        metric: metric.into(),
        answer_path: String::new(),
        expected_path: String::new(),
        input_path: None,
        scorer,
    }
}

fn exactly() -> Scorer {
    Scorer::ExactMatch {
        ignore_case: false,
        trim: false,
    }
}

fn card(scorers: Vec<ScorerSpec>) -> Scorecard {
    Scorecard {
        name: "answer-quality".into(),
        description: "what a saved answer is put through".into(),
        scorers,
    }
}

#[tokio::test]
async fn declaring_the_same_measurements_again_lands_on_the_version_that_is_there() {
    let registry = store();
    let first = registry
        .publish_scorecard(&card(vec![spec("exact", exactly())]), "ada", 1_700_000_000)
        .await
        .expect("a scorecard publishes");
    let again = registry
        .publish_scorecard(
            &card(vec![spec("exact", exactly())]),
            "grace",
            1_700_000_900,
        )
        .await
        .expect("the same card publishes");

    assert_eq!(first.version, again.version);
    assert_eq!(
        again.published_by, "ada",
        "the card that is there is the one that stands, not the one just sent"
    );
    assert_eq!(again.published_at, first.published_at);
}

#[tokio::test]
async fn rewriting_a_card_leaves_a_run_that_named_the_old_one_reading_as_it_did() {
    let registry = store();
    let first = registry
        .publish_scorecard(&card(vec![spec("exact", exactly())]), "ada", 1_700_000_000)
        .await
        .expect("a scorecard publishes");

    let relaxed = registry
        .publish_scorecard(
            &card(vec![spec(
                "exact",
                Scorer::ExactMatch {
                    ignore_case: true,
                    trim: true,
                },
            )]),
            "ada",
            1_700_003_600,
        )
        .await
        .expect("a rewritten scorecard publishes");
    assert_ne!(relaxed.version, first.version);

    let head = registry
        .scorecard("answer-quality", None)
        .await
        .expect("the head reads")
        .expect("a card was published");
    assert_eq!(head.version, relaxed.version, "the head moved");

    let pinned = registry
        .scorecard("answer-quality", Some(&first.version))
        .await
        .expect("the version reads")
        .expect("the earlier version is still there");
    assert_eq!(
        pinned.scorecard.scorers[0].scorer,
        exactly(),
        "what a published result was measured under does not move under it"
    );
}

#[tokio::test]
async fn a_card_that_would_fail_every_case_is_refused_before_a_run_reads_it() {
    let registry = store();
    let refused = registry
        .publish_scorecard(
            &card(vec![spec(
                "formatted",
                Scorer::RegexMatch {
                    pattern: "(unclosed".into(),
                },
            )]),
            "ada",
            1_700_000_000,
        )
        .await
        .expect_err("an unclosed group is not a pattern");
    assert!(
        refused.to_string().contains("pattern"),
        "the refusal names the field: {refused}"
    );

    assert!(
        registry
            .scorecard("answer-quality", None)
            .await
            .expect("the head reads")
            .is_none(),
        "a refused card leaves no head behind"
    );
}

#[tokio::test]
async fn the_list_says_what_each_card_measures_without_opening_it() {
    let registry = store();
    registry
        .publish_scorecard(
            &card(vec![
                spec("exact", exactly()),
                spec(
                    "leaked",
                    Scorer::Forbidden {
                        text: "ssn".into(),
                        ignore_case: true,
                    },
                ),
            ]),
            "ada",
            1_700_000_000,
        )
        .await
        .expect("a scorecard publishes");

    let cards = registry.scorecards().await.expect("the list reads");
    assert_eq!(cards.len(), 1);
    let metrics = &cards[0].metrics;
    assert_eq!(metrics[0].name, "exact");
    assert_eq!(metrics[0].direction, MetricDirection::Higher);
    assert_eq!(
        metrics[1].direction,
        MetricDirection::Lower,
        "a forbidden phrase is counted, so more of it is worse"
    );
}

#[tokio::test]
async fn a_card_nobody_declared_is_absent_rather_than_empty() {
    assert!(
        store()
            .scorecard("answer-quality", None)
            .await
            .expect("the head reads")
            .is_none()
    );
}

#[tokio::test]
async fn a_card_s_versions_read_newest_first_and_their_diff_names_what_a_metric_became() {
    let registry = store();
    let first = registry
        .publish_scorecard(
            &card(vec![
                spec("exact", exactly()),
                spec(
                    "leaked",
                    Scorer::Forbidden {
                        text: "ssn".into(),
                        ignore_case: true,
                    },
                ),
            ]),
            "ada",
            1_700_000_000,
        )
        .await
        .unwrap();
    let second = registry
        .publish_scorecard(
            &card(vec![
                spec(
                    "exact",
                    Scorer::ExactMatch {
                        ignore_case: true,
                        trim: false,
                    },
                ),
                spec(
                    "off_by",
                    Scorer::AbsoluteError {
                        unit: "minutes".into(),
                    },
                ),
            ]),
            "grace",
            1_700_003_600,
        )
        .await
        .unwrap();

    let versions = registry
        .scorecard_versions("answer-quality")
        .await
        .unwrap()
        .expect("a published card has versions");
    assert_eq!(
        versions
            .versions
            .iter()
            .map(|version| version.version.as_str())
            .collect::<Vec<_>>(),
        [second.version.as_str(), first.version.as_str()]
    );
    assert!(
        registry
            .scorecard_versions("nobody-published")
            .await
            .unwrap()
            .is_none()
    );

    let diff = registry
        .scorecard_diff("answer-quality", &first.version, &second.version)
        .await
        .unwrap()
        .expect("both are versions of the card");
    let changes: Vec<(&str, ScorecardChange)> = diff
        .metrics
        .iter()
        .map(|change| (change.metric.as_str(), change.change))
        .collect();
    assert_eq!(
        changes,
        [
            ("exact", ScorecardChange::Changed),
            ("off_by", ScorecardChange::Added),
            ("leaked", ScorecardChange::Removed),
        ]
    );
    assert_eq!(
        diff.metrics[0].fields,
        [FieldChange {
            path: "/scorer/ignore_case".into(),
            before: Some(serde_json::json!(false)),
            after: Some(serde_json::json!(true)),
        }]
    );
    let off_by = diff.metrics[1].after.as_ref().expect("derived");
    assert_eq!(
        (off_by.unit.as_str(), off_by.direction),
        ("minutes", MetricDirection::Lower),
        "what the added metric is, derived rather than left to the reader"
    );
    assert_eq!(
        diff.metrics[2]
            .before
            .as_ref()
            .map(|before| before.direction),
        Some(MetricDirection::Lower)
    );
    assert!(
        registry
            .scorecard_diff("answer-quality", &first.version, &"0".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
}
