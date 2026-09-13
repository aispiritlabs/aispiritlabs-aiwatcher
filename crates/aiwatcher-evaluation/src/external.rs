//! Metrics a scorer framework implements, measured by a service that runs it.
//!
//! DeepEval, Opik and the rest ship metrics that are Python a scorecard must
//! never carry, so they run in a scorer service (`services/scorers`) and this
//! module is all aiwatcher knows of them: a **catalog** the service describes
//! itself with, and one **question** per case. No framework is named here.
//!
//! **Which way is better is the adapter's word, pinned into the card.**
//! Publishing a card copies what the catalog says a metric is — release, model,
//! unit, direction — into the card version, and a run holds the service to it.
//!
//! **A metric a model graded says so**, as `measured_by` on its definition, and
//! **a reply keeps its number and never its words.** See ADR_0030's amendment
//! of 2026-09-13.
//!
//! **A graded metric may be held against people.** A card names a rubric, the
//! number at which the metric's answer passes and, on named levels, the level
//! at which a person's does; a run names a calibration set taken under that
//! rubric, asks the metric about every item and publishes how often the two
//! verdicts were the same, beside the numbers — a judge's agreement, for a
//! framework's model.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    Aggregation, AssessmentValue, CalibrationSet, EvaluationError, JudgeAgreement, MetricDirection,
    Result, Rubric, Rubrics, Scale, Scorecard, VersionReference, digest, require, text,
};

/// The contract version a catalog speaks. A service speaking another is
/// refused by name rather than half understood.
pub const SCORER_CONTRACT: u32 = 1;

/// What part of a case a metric reads.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CaseSide {
    /// What the case asked, where the card's `input_path` points.
    Input,
    /// The answer being measured, where `answer_path` points.
    Answer,
    /// What the cohort expected, where `expected_path` points.
    Expected,
}

/// The JSON a parameter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    String,
    Number,
    Integer,
    Boolean,
    StringList,
}

impl ParameterKind {
    fn admits(self, value: &serde_json::Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.as_f64().is_some_and(f64::is_finite),
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Boolean => value.is_boolean(),
            Self::StringList => value
                .as_array()
                .is_some_and(|items| items.iter().all(serde_json::Value::is_string)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScorerParameter)]
pub struct Parameter {
    pub kind: ParameterKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// One metric, as the adapter that implements it describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScorerCatalogMetric)]
pub struct CatalogMetric {
    pub metric: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub unit: String,
    pub direction: MetricDirection,
    pub aggregation: Aggregation,
    /// The sides of a case it reads. `answer` always.
    pub reads: Vec<CaseSide>,
    /// Whether a model grades it — the adapter's `model`, then.
    #[serde(default)]
    pub model_graded: bool,
    /// The least and most a case may score, when the metric bounds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Parameter>,
}

/// One framework, as the service runs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScorerCatalogAdapter)]
pub struct CatalogAdapter {
    /// What a card names: `deepeval`, `opik`.
    pub name: String,
    /// The framework's own release, as installed. A card pins it.
    pub version: String,
    /// The model its graded metrics ask, as the service is configured. Absent
    /// when it has none, and then it offers no graded metric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<VersionReference>,
    pub metrics: Vec<CatalogMetric>,
}

/// Everything a scorer service measures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScorerCatalog {
    pub contract: u32,
    pub adapters: Vec<CatalogAdapter>,
}

impl ScorerCatalog {
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for a contract this build does not speak,
    /// or a metric whose description could not be pinned into a card.
    pub fn validate(&self) -> Result<()> {
        require(
            self.contract == SCORER_CONTRACT,
            "catalog.contract",
            &format!("this deployment speaks scorer contract {SCORER_CONTRACT}"),
        )?;
        for adapter in &self.adapters {
            text(&adapter.name, "catalog.adapters.name")?;
            text(&adapter.version, "catalog.adapters.version")?;
            for metric in &adapter.metrics {
                let field = format!("catalog.adapters.{}.{}", adapter.name, metric.metric);
                text(&metric.metric, &field)?;
                text(&metric.unit, &format!("{field}.unit"))?;
                require(
                    metric.reads.contains(&CaseSide::Answer),
                    &field,
                    "a metric reads the answer it measures",
                )?;
                require(
                    !metric.model_graded || adapter.model.is_some(),
                    &field,
                    "is graded by a model, and the adapter names none",
                )?;
                if let Some([low, high]) = metric.range {
                    require(
                        low.is_finite() && high.is_finite() && low < high,
                        &format!("{field}.range"),
                        "is two finite numbers, the least first",
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The adapter and the metric a card names, if this catalog has both.
    #[must_use]
    pub fn find(&self, adapter: &str, metric: &str) -> Option<(&CatalogAdapter, &CatalogMetric)> {
        let adapter = self.adapters.iter().find(|held| held.name == adapter)?;
        let metric = adapter.metrics.iter().find(|held| held.metric == metric)?;
        Some((adapter, metric))
    }
}

/// A catalog as the work role last read it from the service, kept where the
/// serve role — which opens no socket to a scorer — resolves a card against it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RecordedCatalog {
    pub catalog: ScorerCatalog,
    pub recorded_at: i64,
    /// The process that read it.
    pub recorded_by: String,
}

/// What a card version pins about one external metric: the catalog's word at
/// the moment the card was published, and never the author's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalDeclaration {
    /// The adapter's version the card measures with.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<VersionReference>,
    pub unit: String,
    pub direction: MetricDirection,
    pub aggregation: Aggregation,
    pub reads: Vec<CaseSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
}

impl ExternalDeclaration {
    /// What a catalog entry pins into a card.
    #[must_use]
    pub fn of(adapter: &CatalogAdapter, metric: &CatalogMetric) -> Self {
        Self {
            version: adapter.version.clone(),
            model: metric.model_graded.then(|| adapter.model.clone()).flatten(),
            unit: metric.unit.clone(),
            direction: metric.direction,
            aggregation: metric.aggregation,
            reads: metric.reads.clone(),
            range: metric.range,
        }
    }

    /// Whether this metric reads that side of a case.
    #[must_use]
    pub fn reads(&self, side: CaseSide) -> bool {
        self.reads.contains(&side)
    }
}

/// Resolve a card's external metric against a catalog: the declaration to pin,
/// or every reason it cannot be measured here.
///
/// # Errors
///
/// [`EvaluationError::Invalid`] naming what the catalog lacks, a parameter it
/// does not take, one it requires, or one of the wrong kind — and, for a card
/// that already pinned a declaration, a catalog that now says otherwise.
pub fn resolve(
    catalog: &ScorerCatalog,
    field: &str,
    adapter: &str,
    metric: &str,
    parameters: &serde_json::Map<String, serde_json::Value>,
    pinned: Option<&ExternalDeclaration>,
) -> Result<ExternalDeclaration> {
    let Some((held, described)) = catalog.find(adapter, metric) else {
        let offered: Vec<String> = catalog
            .adapters
            .iter()
            .flat_map(|held| {
                held.metrics
                    .iter()
                    .map(move |metric| format!("{}.{}", held.name, metric.metric))
            })
            .collect();
        return Err(EvaluationError::Invalid {
            field: field.into(),
            reason: format!(
                "the scorer service implements no {adapter} metric `{metric}`; it offers {}",
                if offered.is_empty() {
                    "nothing".to_owned()
                } else {
                    offered.join(", ")
                }
            ),
        });
    };
    let mut problems = Vec::new();
    for name in parameters.keys() {
        if !described.parameters.contains_key(name) {
            problems.push(format!("takes no parameter `{name}`"));
        }
    }
    for (name, parameter) in &described.parameters {
        match parameters.get(name) {
            None if parameter.required => problems.push(format!("requires `{name}`")),
            Some(value) if !parameter.kind.admits(value) => {
                problems.push(format!("`{name}` is a {:?}", parameter.kind));
            }
            _ => {}
        }
    }
    if !problems.is_empty() {
        return Err(EvaluationError::Invalid {
            field: field.into(),
            reason: format!("{adapter}.{metric} {}", problems.join("; ")),
        });
    }
    let declared = ExternalDeclaration::of(held, described);
    if let Some(pinned) = pinned {
        let described = |declaration: &ExternalDeclaration| {
            format!(
                "{adapter} {}{}",
                declaration.version,
                declaration
                    .model
                    .as_ref()
                    .map(|model| format!(" grading with {} {}", model.name, model.version))
                    .unwrap_or_default()
            )
        };
        require(
            pinned == &declared,
            field,
            &format!(
                "was pinned against {} and the scorer service now describes {}; publish the card \
                 again to measure with what the service runs",
                described(pinned),
                described(&declared)
            ),
        )?;
    }
    Ok(declared)
}

/// Where a framework metric's number and a person's judgement become the same
/// kind of answer, so that the two can be counted as agreeing or not.
///
/// A verdict on each side, because the two are on different scales: a
/// relevancy of 0.83 is not "good", but "at 0.7 or more" and "good or better"
/// are both a pass. Part of the card, so a calibration against another bar is
/// another card version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalCalibration {
    /// The rubric the people in a calibration set judged under.
    pub rubric: VersionReference,
    /// The metric's number at which an answer passes: at it, or on the side
    /// the catalog declared better.
    pub pass_at: f64,
    /// The rubric's level at which a person's judgement passes, on a rubric
    /// with named levels. A yes-or-no rubric passes on its better answer and
    /// names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_level: Option<String>,
}

impl ExternalCalibration {
    pub(crate) fn validate(&self, field: &str) -> Result<()> {
        self.rubric.validate(&format!("{field}.rubric"))?;
        require(
            self.pass_at.is_finite(),
            &format!("{field}.pass_at"),
            "must be a finite number",
        )?;
        match &self.pass_level {
            Some(level) => text(level, &format!("{field}.pass_level")),
            None => Ok(()),
        }
    }

    /// Whether this calibration can be drawn for a metric declared so, under
    /// that rubric: a direction on both sides, a bar inside the metric's
    /// range, and a level the rubric has — or a yes-or-no rubric and no level.
    pub(crate) fn check(
        &self,
        field: &str,
        declared: &ExternalDeclaration,
        rubric: &Rubric,
    ) -> Result<()> {
        require(
            declared.direction != MetricDirection::None,
            &format!("{field}.calibration"),
            "the catalog says neither end of this metric is better, so no number of it passes",
        )?;
        require(
            declared
                .range
                .is_none_or(|[low, high]| (low..=high).contains(&self.pass_at)),
            &format!("{field}.calibration.pass_at"),
            "lies outside the range the catalog declared for this metric",
        )?;
        require(
            rubric.direction != MetricDirection::None,
            &format!("{field}.calibration.rubric"),
            "says neither end of its scale is better, so no judgement under it passes",
        )?;
        match (&rubric.scale, &self.pass_level) {
            (Scale::Ordinal { levels }, Some(level)) => require(
                levels.iter().any(|named| named == level),
                &format!("{field}.calibration.pass_level"),
                &format!("`{level}` is not one of the rubric's levels"),
            ),
            (Scale::Ordinal { .. }, None) => Err(EvaluationError::Invalid {
                field: format!("{field}.calibration.pass_level"),
                reason: "a rubric with named levels needs the level a judgement passes at".into(),
            }),
            (Scale::Flag, None) => Ok(()),
            (Scale::Flag, Some(_)) => Err(EvaluationError::Invalid {
                field: format!("{field}.calibration.pass_level"),
                reason: "a yes-or-no rubric passes on its better answer and names no level".into(),
            }),
            (Scale::Numeric { .. }, _) => Err(EvaluationError::Invalid {
                field: format!("{field}.calibration.rubric"),
                reason: "a numeric rubric names no level a person's judgement passes at; \
                         calibrate against named levels or a yes-or-no rubric"
                    .into(),
            }),
        }
    }

    /// The metric's verdict on a number it gave: one for a pass, nought not.
    #[must_use]
    pub fn verdict(&self, declared: &ExternalDeclaration, value: f64) -> Option<f64> {
        let passed = match declared.direction {
            MetricDirection::Higher => value >= self.pass_at,
            MetricDirection::Lower => value <= self.pass_at,
            MetricDirection::None => return None,
        };
        Some(if passed { 1.0 } else { 0.0 })
    }

    /// A person's verdict on the same answer, under the rubric.
    #[must_use]
    pub fn judged(&self, rubric: &Rubric, value: &AssessmentValue) -> Option<f64> {
        match (&rubric.scale, value) {
            (Scale::Ordinal { .. }, _) => {
                crate::scored(rubric, Some(self.pass_level.as_deref()?), value)
            }
            (Scale::Flag, AssessmentValue::Flag { value }) => {
                let passed = match rubric.direction {
                    MetricDirection::Higher => *value,
                    MetricDirection::Lower => !*value,
                    MetricDirection::None => return None,
                };
                Some(if passed { 1.0 } else { 0.0 })
            }
            _ => None,
        }
    }
}

/// What a result whose framework metrics were calibrated carries beside its
/// numbers: the set, and how far each metric's verdicts were its people's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalReport {
    pub calibration: VersionReference,
    /// One row per calibrated metric. Its `mean_absolute_difference` is over
    /// the two verdicts, so it is the share of answered items they differed on.
    pub agreement: Vec<JudgeAgreement>,
}

/// Fold what a card's calibrated framework metrics said about a calibration
/// set into agreement per metric.
///
/// `said` is keyed by the item's position in the set and the metric, and holds
/// the number the service gave — or nothing where it gave none, which counts
/// against the metric like an item a judge declined.
#[must_use]
pub fn external_agreement(
    card: &Scorecard,
    rubrics: &Rubrics,
    calibration: &VersionReference,
    set: &CalibrationSet,
    said: &BTreeMap<(usize, String), Option<f64>>,
) -> ExternalReport {
    let mut agreement = Vec::new();
    for spec in &card.scorers {
        let Some(external) = spec.scorer.external() else {
            continue;
        };
        let (Some(calibrated), Some(declared)) = (external.calibration, external.declared) else {
            continue;
        };
        let Some(rubric) = rubrics.get(&calibrated.rubric) else {
            continue;
        };
        agreement.push(crate::judge::counted(
            &spec.metric,
            &calibrated.rubric,
            set.items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.rubric == calibrated.rubric)
                .map(|(index, item)| {
                    (
                        said.get(&(index, spec.metric.clone()))
                            .copied()
                            .flatten()
                            .and_then(|value| calibrated.verdict(declared, value)),
                        calibrated.judged(rubric, &item.value),
                    )
                }),
        ));
    }
    ExternalReport {
        calibration: calibration.clone(),
        agreement,
    }
}

/// One case put to one external metric.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalCase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    pub answer: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<serde_json::Value>,
}

/// What a scorer service is asked about one case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalCall {
    pub adapter: String,
    pub metric: String,
    /// The version and model the card pinned, which the service refuses to
    /// measure under when it runs something else.
    pub declared: ExternalDeclaration,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub parameters: serde_json::Map<String, serde_json::Value>,
    pub case: ExternalCase,
}

impl ExternalCall {
    /// The digest a reply to this exact question is kept under.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the call does not encode.
    pub fn question(&self) -> Result<String> {
        digest(&(crate::SCHEMA_VERSION, "evaluation.external_question", self))
    }
}

/// What a scorer service answered about one case: a number, or the adapter's
/// own sentence about why there is none. Never a model's words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

impl ExternalReply {
    /// What this reply scores, held to what the card pinned.
    #[must_use]
    pub fn score(&self, declared: &ExternalDeclaration) -> crate::Score {
        match (self.value, &self.failed) {
            (_, Some(reason)) => crate::Score::Unscored(clip(reason)),
            (None, None) => crate::Score::Unscored("the scorer service gave no number".into()),
            (Some(value), None) if !value.is_finite() => {
                crate::Score::Unscored("the scorer service's number is not finite".into())
            }
            (Some(value), None)
                if declared
                    .range
                    .is_some_and(|[low, high]| value < low || value > high) =>
            {
                crate::Score::Unscored(format!(
                    "the scorer service answered {value}, outside the range its catalog declared"
                ))
            }
            (Some(value), None)
                if declared.aggregation == Aggregation::Rate && value != 0.0 && value != 1.0 =>
            {
                crate::Score::Unscored(format!(
                    "the metric is a rate, so a case passes or does not, and the service answered \
                     {value}"
                ))
            }
            (Some(value), None) => crate::Score::Measured(value),
        }
    }
}

fn clip(reason: &str) -> String {
    let line: String = reason
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    format!("the scorer service: {line}")
}

/// Why a scorer service gave no reply at all.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ScorerFailure {
    /// It could not be reached, or asked to come back. Worth another attempt.
    #[error("the scorer service is unavailable: {0}")]
    Unavailable(String),
    /// It answered and refused. The same next time.
    #[error("the scorer service refused: {0}")]
    Refused(String),
}

/// A scorer service, behind the address this deployment configured.
#[async_trait]
pub trait ExternalScorers: Send + Sync + std::fmt::Debug {
    /// Everything it measures, as it describes itself now.
    async fn catalog(&self) -> std::result::Result<ScorerCatalog, ScorerFailure>;

    /// One case, under one metric.
    async fn score(&self, call: &ExternalCall)
    -> std::result::Result<ExternalReply, ScorerFailure>;
}

/// A reply kept under a declaration: the number, or the failure, and never
/// the case it was about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptScore {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

impl From<&ExternalReply> for KeptScore {
    fn from(reply: &ExternalReply) -> Self {
        Self {
            value: reply.value,
            failed: reply.failed.clone(),
        }
    }
}

impl From<KeptScore> for ExternalReply {
    fn from(kept: KeptScore) -> Self {
        Self {
            value: kept.value,
            failed: kept.failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> ScorerCatalog {
        serde_json::from_value(json!({
            "contract": 1,
            "adapters": [{
                "name": "deepeval", "version": "4.2.2",
                "model": {"name": "gemma-4-e2b", "version": "ud-q4-k-xl"},
                "metrics": [
                    {"metric": "answer_relevancy", "unit": "score", "direction": "higher",
                     "aggregation": "mean", "reads": ["input", "answer"], "model_graded": true,
                     "range": [0.0, 1.0],
                     "parameters": {"threshold": {"kind": "number"}}},
                    {"metric": "exact_match", "unit": "ratio", "direction": "higher",
                     "aggregation": "rate", "reads": ["answer", "expected"]}
                ]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn a_card_pins_what_the_catalog_said_and_a_changed_catalog_is_named_rather_than_obeyed() {
        let catalog = catalog();
        catalog.validate().unwrap();
        let params = serde_json::Map::from_iter([("threshold".into(), json!(0.7))]);
        let declared = resolve(
            &catalog,
            "scorer",
            "deepeval",
            "answer_relevancy",
            &params,
            None,
        )
        .unwrap();
        assert_eq!(declared.version, "4.2.2");
        assert_eq!(declared.model.as_ref().unwrap().name, "gemma-4-e2b");
        assert_eq!(declared.direction, MetricDirection::Higher);

        let mut upgraded = catalog;
        upgraded.adapters[0].version = "4.3.0".into();
        let refused = resolve(
            &upgraded,
            "scorer",
            "deepeval",
            "answer_relevancy",
            &params,
            Some(&declared),
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("4.2.2") && refused.contains("4.3.0"),
            "{refused}"
        );
    }

    #[test]
    fn a_metric_or_a_parameter_the_catalog_does_not_describe_is_refused_with_every_problem() {
        let catalog = catalog();
        let unknown = resolve(
            &catalog,
            "scorer",
            "deepeval",
            "vibes",
            &serde_json::Map::new(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(unknown.contains("deepeval.exact_match"), "{unknown}");

        let params = serde_json::Map::from_iter([
            ("threshold".into(), json!("high")),
            ("verbose".into(), json!(true)),
        ]);
        let wrong = resolve(
            &catalog,
            "scorer",
            "deepeval",
            "answer_relevancy",
            &params,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            wrong.contains("`verbose`") && wrong.contains("`threshold` is a Number"),
            "{wrong}"
        );
    }

    #[test]
    fn a_reply_scores_only_what_the_pinned_metric_can_be() {
        let rate =
            ExternalDeclaration::of(&catalog().adapters[0], &catalog().adapters[0].metrics[1]);
        let bounded =
            ExternalDeclaration::of(&catalog().adapters[0], &catalog().adapters[0].metrics[0]);
        let reply = |value: f64| ExternalReply {
            value: Some(value),
            failed: None,
        };
        assert_eq!(reply(1.0).score(&rate), crate::Score::Measured(1.0));
        assert!(matches!(reply(0.5).score(&rate), crate::Score::Unscored(_)));
        assert_eq!(reply(0.5).score(&bounded), crate::Score::Measured(0.5));
        assert!(matches!(
            reply(1.5).score(&bounded),
            crate::Score::Unscored(_)
        ));
        assert!(matches!(
            reply(f64::NAN).score(&bounded),
            crate::Score::Unscored(_)
        ));
        let failed = ExternalReply {
            value: None,
            failed: Some("MetricError\nno context given".into()),
        };
        assert_eq!(
            failed.score(&bounded),
            crate::Score::Unscored("the scorer service: MetricErrorno context given".into())
        );
    }

    fn graded(direction: MetricDirection) -> ExternalDeclaration {
        ExternalDeclaration {
            version: "4.2.2".into(),
            model: None,
            unit: "score".into(),
            direction,
            aggregation: Aggregation::Mean,
            reads: vec![CaseSide::Answer],
            range: Some([0.0, 1.0]),
        }
    }

    fn rubric(scale: Scale, direction: MetricDirection) -> Rubric {
        Rubric {
            name: "helpful".into(),
            question: "Does it help?".into(),
            guidance: String::new(),
            scale,
            direction,
        }
    }

    fn calibration(pass_at: f64, pass_level: Option<&str>) -> ExternalCalibration {
        ExternalCalibration {
            rubric: VersionReference {
                name: "helpful".into(),
                version: "r1".into(),
            },
            pass_at,
            pass_level: pass_level.map(str::to_owned),
        }
    }

    #[test]
    fn a_calibration_is_drawn_only_where_both_sides_can_pass() {
        let levels = rubric(
            Scale::Ordinal {
                levels: vec!["poor".into(), "fair".into(), "good".into()],
            },
            MetricDirection::Higher,
        );
        let flag = rubric(Scale::Flag, MetricDirection::Higher);
        let higher = graded(MetricDirection::Higher);

        assert!(calibration(0.7, Some("good")).check("s", &higher, &levels).is_ok());
        assert!(calibration(0.7, None).check("s", &higher, &flag).is_ok());
        for (refused, declared, rubric, why) in [
            (calibration(0.7, None), &higher, &levels, "pass_level"),
            (calibration(0.7, Some("great")), &higher, &levels, "great"),
            (calibration(0.7, Some("good")), &higher, &flag, "names no level"),
            (calibration(1.5, None), &higher, &flag, "range"),
            (
                calibration(0.7, None),
                &graded(MetricDirection::None),
                &flag,
                "neither end",
            ),
            (
                calibration(0.7, None),
                &higher,
                &rubric(Scale::Numeric { min: 1.0, max: 5.0 }, MetricDirection::Higher),
                "numeric",
            ),
        ] {
            let reason = refused.check("s", declared, rubric).unwrap_err().to_string();
            assert!(reason.contains(why), "{why}: {reason}");
        }
    }

    #[test]
    fn a_verdict_is_a_pass_on_the_better_side_of_the_bar_on_both_sides() {
        let bar = calibration(0.7, Some("fair"));
        assert_eq!(bar.verdict(&graded(MetricDirection::Higher), 0.7), Some(1.0));
        assert_eq!(bar.verdict(&graded(MetricDirection::Higher), 0.69), Some(0.0));
        assert_eq!(bar.verdict(&graded(MetricDirection::Lower), 0.2), Some(1.0));

        let fewer_is_better = rubric(
            Scale::Ordinal {
                levels: vec!["none".into(), "fair".into(), "many".into()],
            },
            MetricDirection::Lower,
        );
        let level = |value: &str| AssessmentValue::Level {
            value: value.into(),
        };
        assert_eq!(bar.judged(&fewer_is_better, &level("none")), Some(1.0));
        assert_eq!(bar.judged(&fewer_is_better, &level("many")), Some(0.0));

        let harmful = rubric(Scale::Flag, MetricDirection::Lower);
        let yes = AssessmentValue::Flag { value: true };
        assert_eq!(
            calibration(0.7, None).judged(&harmful, &yes),
            Some(0.0),
            "a yes on a rubric where yes is worse is a fail"
        );
    }
}
