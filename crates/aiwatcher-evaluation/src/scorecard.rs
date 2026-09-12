//! What an evaluation measures, declared once and versioned by its content.
//!
//! The suite list this API already serves is an aggregate of reports: a name a
//! producer sent, discovered after the fact. Nothing in it says what was
//! measured, so nothing in it can be run again. A scorecard is that missing
//! half — the scorers a case is put through, the metric each one writes, and
//! which end of each metric is better — and it is authored rather than
//! observed, so it is content-addressed the way a rubric and a prompt are.
//!
//! Two rules carry it. **A scorer is named, not written**: a scorecard holds
//! no code, only a name from the vocabulary below and the parameters that name
//! takes, so publishing one is not a way to run something on this host. And
//! **the metric definition is derived**, never authored beside the scorer: a
//! forbidden phrase somebody declared as higher-is-better would invert every
//! comparison drawn from it, and which way is better is a fact about what the
//! scorer counts rather than an opinion about it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::{
    Aggregation, EvaluationError, MetricDefinition, MetricDirection, Result, SCHEMA_VERSION,
    digest, require,
    store::{self, Store},
    text,
};

/// The scorers this deployment implements, and what each one takes.
///
/// The enum is the implementation: a name reaches a `match` arm and never a
/// callable, so the vocabulary cannot grow without code that knows what the
/// new word means.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scorer {
    /// The answer is the expected one. Two strings are compared as text under
    /// the options below; anything else is compared as JSON, so an object with
    /// its keys in another order still matches.
    ExactMatch {
        #[serde(default)]
        ignore_case: bool,
        #[serde(default)]
        trim: bool,
    },
    /// The expected answer appears somewhere in the answer.
    Contains {
        #[serde(default)]
        ignore_case: bool,
    },
    /// The answer has the shape this pattern describes. The pattern is the
    /// scorecard's rather than the case's: it asks the same question of every
    /// answer, which is why it reads no expectation.
    RegexMatch { pattern: String },
    /// The answer is a number near the expected number.
    NumericWithin { tolerance: f64 },
    /// The answer said something it must not. Counted rather than avoided, so
    /// the metric means what its name says and lower is better.
    Forbidden {
        text: String,
        #[serde(default)]
        ignore_case: bool,
    },
}

/// One scorer's answer about one case.
#[derive(Clone, Debug, PartialEq)]
pub enum Score {
    Measured(f64),
    /// Why this case has no number. Never a zero: an answer missing the field
    /// a scorer reads has not scored badly, it has not been scored.
    Unscored(String),
}

impl Scorer {
    /// Whether this scorer reads the case's expected answer at all.
    #[must_use]
    pub const fn reads_expected(&self) -> bool {
        matches!(
            self,
            Self::ExactMatch { .. } | Self::Contains { .. } | Self::NumericWithin { .. }
        )
    }

    /// Which way this scorer's metric is better, and in what unit.
    ///
    /// Every scorer here answers pass or fail about one case, so the aggregate
    /// is the fraction that passed — a rate, which is also what bounds each
    /// case's own number to nought or one. A scorer measuring a quantity would
    /// have a unit only its author knows, and the spec gains that field the day
    /// one arrives rather than inviting a guess now.
    #[must_use]
    pub const fn direction(&self) -> MetricDirection {
        match self {
            Self::Forbidden { .. } => MetricDirection::Lower,
            _ => MetricDirection::Higher,
        }
    }

    fn validate(&self, field: &str) -> Result<()> {
        match self {
            Self::RegexMatch { pattern } => {
                text(pattern, &format!("{field}.pattern"))?;
                // Refused here rather than at score time: a pattern that does
                // not compile would otherwise fail every case of a run under a
                // scorecard somebody already published.
                regex::Regex::new(pattern).map_err(|error| EvaluationError::Invalid {
                    field: format!("{field}.pattern"),
                    reason: error.to_string().replace('\n', " "),
                })?;
                Ok(())
            }
            Self::NumericWithin { tolerance } => require(
                tolerance.is_finite() && *tolerance >= 0.0,
                &format!("{field}.tolerance"),
                "must be a finite tolerance of zero or more",
            ),
            Self::Forbidden { text: phrase, .. } => text(phrase, &format!("{field}.text")),
            Self::ExactMatch { .. } | Self::Contains { .. } => Ok(()),
        }
    }

    fn score(&self, answer: &serde_json::Value, expected: &serde_json::Value) -> Score {
        match self {
            Self::ExactMatch { ignore_case, trim } => match (answer.as_str(), expected.as_str()) {
                (Some(answer), Some(expected)) => {
                    hit(fold(answer, *ignore_case, *trim) == fold(expected, *ignore_case, *trim))
                }
                _ => hit(answer == expected),
            },
            Self::Contains { ignore_case } => match (answer.as_str(), expected.as_str()) {
                (Some(answer), Some(expected)) => hit(fold(answer, *ignore_case, false)
                    .contains(&fold(expected, *ignore_case, false))),
                _ => Score::Unscored("this scorer compares text and one side is not".into()),
            },
            Self::RegexMatch { pattern } => match (regex::Regex::new(pattern), answer.as_str()) {
                (Ok(pattern), Some(answer)) => hit(pattern.is_match(answer)),
                (Ok(_), None) => {
                    Score::Unscored("this scorer reads text and the answer is not".into())
                }
                (Err(error), _) => Score::Unscored(error.to_string().replace('\n', " ")),
            },
            Self::NumericWithin { tolerance } => match (answer.as_f64(), expected.as_f64()) {
                (Some(answer), Some(expected)) => hit((answer - expected).abs() <= *tolerance),
                _ => Score::Unscored("this scorer compares numbers and one side is not".into()),
            },
            Self::Forbidden {
                text: phrase,
                ignore_case,
            } => match answer.as_str() {
                Some(answer) => hit(fold(answer, *ignore_case, false).contains(&fold(
                    phrase,
                    *ignore_case,
                    false,
                ))),
                None => Score::Unscored("this scorer reads text and the answer is not".into()),
            },
        }
    }
}

fn hit(held: bool) -> Score {
    Score::Measured(if held { 1.0 } else { 0.0 })
}

fn fold(value: &str, ignore_case: bool, trim: bool) -> String {
    let value = if trim { value.trim() } else { value };
    if ignore_case {
        value.to_lowercase()
    } else {
        value.to_owned()
    }
}

/// One measurement: a metric name, where to read each side, and the scorer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScorerSpec {
    /// The metric this writes. Unique within the scorecard, because a case
    /// carries one number per name and the second writer would win silently.
    pub metric: String,
    /// A JSON Pointer into the recorded answer. Empty reads the whole value,
    /// which is what a plain text answer wants.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub answer_path: String,
    /// The same, into the case's expected answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected_path: String,
    pub scorer: Scorer,
}

impl ScorerSpec {
    fn validate(&self, field: &str) -> Result<()> {
        text(&self.metric, &format!("{field}.metric"))?;
        pointer(&self.answer_path, &format!("{field}.answer_path"))?;
        pointer(&self.expected_path, &format!("{field}.expected_path"))?;
        require(
            self.expected_path.is_empty() || self.scorer.reads_expected(),
            &format!("{field}.expected_path"),
            "this scorer reads no expected answer",
        )?;
        self.scorer.validate(&format!("{field}.scorer"))
    }

    /// What this scorer measured about one case.
    #[must_use]
    pub fn measure(&self, answer: &serde_json::Value, expected: &serde_json::Value) -> Score {
        let Some(answer) = answer.pointer(&self.answer_path) else {
            return Score::Unscored(format!("the answer has nothing at {}", self.answer_path));
        };
        let Some(expected) = expected.pointer(&self.expected_path) else {
            return Score::Unscored(format!(
                "the expected answer has nothing at {}",
                self.expected_path
            ));
        };
        self.scorer.score(answer, expected)
    }

    /// The metric definition a published context carries for this scorer.
    #[must_use]
    pub fn metric(&self) -> MetricDefinition {
        MetricDefinition {
            name: self.metric.clone(),
            unit: "ratio".into(),
            direction: self.scorer.direction(),
            aggregation: Aggregation::Rate,
        }
    }
}

fn pointer(value: &str, field: &str) -> Result<()> {
    require(
        (value.is_empty() || value.starts_with('/'))
            && value.len() <= 512
            && !value.chars().any(char::is_control),
        field,
        "must be empty or a JSON Pointer beginning with a slash",
    )
}

/// The form itself. Everything here is part of the version: two scorecards
/// that measure different things under one name are two scorecards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Scorecard {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[schema(min_items = 1, max_items = 32)]
    pub scorers: Vec<ScorerSpec>,
}

impl Scorecard {
    pub(crate) fn validate(&self) -> Result<()> {
        text(&self.name, "scorecard.name")?;
        require(
            !self.name.contains('/'),
            "scorecard.name",
            "must not contain a path separator: the name is how a run asks for the card",
        )?;
        require(
            self.description.len() <= 8 * 1024 && !self.description.contains('\0'),
            "scorecard.description",
            "must be at most 8 KiB of text",
        )?;
        require(
            (1..=32).contains(&self.scorers.len()),
            "scorecard.scorers",
            "requires 1–32 scorers",
        )?;
        let mut names = BTreeSet::new();
        for (index, spec) in self.scorers.iter().enumerate() {
            spec.validate(&format!("scorecard.scorers[{index}]"))?;
            require(
                names.insert(&spec.metric),
                "scorecard.scorers",
                "metric names must be unique",
            )?;
        }
        Ok(())
    }

    /// The content address of this card. Publishing the same measurements
    /// twice lands on the version that is already there.
    pub(crate) fn version(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, "evaluation.scorecard", self))
    }

    /// The metrics a result measured under this card declares.
    #[must_use]
    pub fn metrics(&self) -> Vec<MetricDefinition> {
        self.scorers.iter().map(ScorerSpec::metric).collect()
    }
}

/// One immutable version, and who put it there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScorecardVersion {
    pub version: String,
    /// Nested rather than flattened: the card denies unknown fields, and a
    /// flattened struct that does reports every field beside it as one.
    pub scorecard: Scorecard,
    pub published_by: String,
    pub published_at: i64,
}

/// Which version a caller that named no version gets. Derived from the
/// versions, which are the truth — and moved by publishing, never edited.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScorecardHead {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub metrics: Vec<MetricDefinition>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScorecardPage {
    pub scorecards: Vec<ScorecardHead>,
}

pub(crate) async fn publish(
    store: &Store,
    scorecard: &Scorecard,
    published_by: &str,
    now: i64,
) -> Result<ScorecardVersion> {
    scorecard.validate()?;
    let version = scorecard.version()?;
    let key = store::scorecard_version(&scorecard.name, &version);
    let published = ScorecardVersion {
        version,
        scorecard: scorecard.clone(),
        published_by: published_by.to_owned(),
        published_at: now,
    };
    // The version before the head that names it, the way a prompt is written:
    // an unindexed version is waiting to be named, and a head naming nothing
    // is a card the next run cannot open.
    if !store.create(&key, &published).await? {
        let existing: ScorecardVersion = store.read(&key).await?.ok_or(
            EvaluationError::Unavailable(crate::EvidenceState::CorruptArtifact),
        )?;
        head_to(store, &existing, now).await?;
        return Ok(existing);
    }
    head_to(store, &published, now).await?;
    Ok(published)
}

async fn head_to(store: &Store, version: &ScorecardVersion, now: i64) -> Result<()> {
    let head = ScorecardHead {
        name: version.scorecard.name.clone(),
        version: version.version.clone(),
        description: version.scorecard.description.clone(),
        metrics: version.scorecard.metrics(),
        updated_at: now,
    };
    store
        .0
        .put(
            &store::scorecard_head(&version.scorecard.name),
            crate::canonical(&head)?,
        )
        .await?;
    Ok(())
}

pub(crate) async fn head(store: &Store, name: &str) -> Result<Option<ScorecardHead>> {
    store.read(&store::scorecard_head(name)).await
}

pub(crate) async fn version(
    store: &Store,
    name: &str,
    version: &str,
) -> Result<Option<ScorecardVersion>> {
    store.read(&store::scorecard_version(name, version)).await
}

pub(crate) async fn all(store: &Store) -> Result<Vec<ScorecardHead>> {
    let mut heads = Vec::new();
    for entry in store.0.list(store::SCORECARDS).await? {
        if entry.key.ends_with("/head.json")
            && let Some(head) = store.read::<ScorecardHead>(&entry.key).await?
        {
            heads.push(head);
        }
    }
    heads.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(heads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(metric: &str, scorer: Scorer) -> ScorerSpec {
        ScorerSpec {
            metric: metric.into(),
            answer_path: String::new(),
            expected_path: String::new(),
            scorer,
        }
    }

    fn card(scorers: Vec<ScorerSpec>) -> Scorecard {
        Scorecard {
            name: "answer-quality".into(),
            description: String::new(),
            scorers,
        }
    }

    #[test]
    fn what_a_scorer_counts_decides_which_way_is_better_rather_than_its_author() {
        let card = card(vec![
            spec(
                "exact",
                Scorer::ExactMatch {
                    ignore_case: false,
                    trim: false,
                },
            ),
            spec(
                "leaked",
                Scorer::Forbidden {
                    text: "ssn".into(),
                    ignore_case: true,
                },
            ),
        ]);
        let metrics = card.metrics();
        assert_eq!(metrics[0].direction, MetricDirection::Higher);
        assert_eq!(
            metrics[1].direction,
            MetricDirection::Lower,
            "a forbidden phrase is counted, so more of it is worse"
        );
        assert!(metrics.iter().all(|metric| metric.unit == "ratio"));
    }

    #[test]
    fn a_scorer_that_reads_no_expectation_refuses_a_path_into_one() {
        let mut spec = spec(
            "formatted",
            Scorer::RegexMatch {
                pattern: "^[0-9]+$".into(),
            },
        );
        assert!(card(vec![spec.clone()]).validate().is_ok());
        spec.expected_path = "/answer".into();
        let refused = card(vec![spec])
            .validate()
            .expect_err("a pattern reads the answer and nothing else");
        let said = refused.to_string();
        assert!(said.contains("expected_path"), "{said}");
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_refused_before_a_run_reads_it() {
        let refused = card(vec![spec(
            "formatted",
            Scorer::RegexMatch {
                pattern: "([0-9]+".into(),
            },
        )])
        .validate()
        .expect_err("an unclosed group is not a pattern");
        assert!(refused.to_string().contains("scorers[0].scorer.pattern"));
    }

    #[test]
    fn an_answer_without_the_field_a_scorer_reads_is_unscored_rather_than_zero() {
        let spec = ScorerSpec {
            metric: "exact".into(),
            answer_path: "/text".into(),
            expected_path: String::new(),
            scorer: Scorer::ExactMatch {
                ignore_case: false,
                trim: false,
            },
        };
        assert_eq!(
            spec.measure(&json!({"text": "four"}), &json!("four")),
            Score::Measured(1.0)
        );
        let Score::Unscored(reason) = spec.measure(&json!({"answer": "four"}), &json!("four"))
        else {
            panic!("an answer with no /text has not scored badly, it has not been scored");
        };
        assert!(reason.contains("/text"), "{reason}");
    }

    #[test]
    fn two_objects_match_whatever_order_their_keys_arrived_in() {
        let spec = spec(
            "exact",
            Scorer::ExactMatch {
                ignore_case: false,
                trim: false,
            },
        );
        assert_eq!(
            spec.measure(&json!({"a": 1, "b": 2}), &json!({"b": 2, "a": 1})),
            Score::Measured(1.0)
        );
    }

    #[test]
    fn measuring_something_else_under_one_name_is_another_version() {
        let first = card(vec![spec("hit", Scorer::Contains { ignore_case: false })])
            .version()
            .expect("a scorecard addresses");
        let relaxed = card(vec![spec("hit", Scorer::Contains { ignore_case: true })])
            .version()
            .expect("a scorecard addresses");
        assert_ne!(
            first, relaxed,
            "ignoring case admits answers the first version refused"
        );
    }

    #[test]
    fn one_metric_written_by_two_scorers_is_refused_when_it_is_published() {
        let refused = card(vec![
            spec("hit", Scorer::Contains { ignore_case: false }),
            spec("hit", Scorer::Contains { ignore_case: true }),
        ])
        .validate()
        .expect_err("a case carries one number per name");
        assert!(refused.to_string().contains("unique"));
    }
}
