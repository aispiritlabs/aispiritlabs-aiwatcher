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
    Aggregation, AgreementInterval, AssessmentValue, CalibrationSet, EvaluationError,
    JudgeAgreement, MetricDirection, Result, Rubric, Rubrics, Scale, Scorecard, VersionReference,
    digest, require, text,
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
    /// The rubric's number at which a person's judgement passes, on a numeric
    /// rubric: at it, or on the side the rubric declared better.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_score: Option<f64>,
}

impl ExternalCalibration {
    pub(crate) fn validate(&self, field: &str) -> Result<()> {
        self.rubric.validate(&format!("{field}.rubric"))?;
        require(
            self.pass_at.is_finite(),
            &format!("{field}.pass_at"),
            "must be a finite number",
        )?;
        require(
            self.pass_score.is_none_or(f64::is_finite),
            &format!("{field}.pass_score"),
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
        let invalid = |name: &str, reason: &str| {
            Err(EvaluationError::Invalid {
                field: format!("{field}.calibration.{name}"),
                reason: reason.into(),
            })
        };
        match (&rubric.scale, &self.pass_level, self.pass_score) {
            (Scale::Ordinal { levels }, Some(level), None) => require(
                levels.iter().any(|named| named == level),
                &format!("{field}.calibration.pass_level"),
                &format!("`{level}` is not one of the rubric's levels"),
            ),
            (Scale::Ordinal { .. }, None, _) => invalid(
                "pass_level",
                "a rubric with named levels needs the level a judgement passes at",
            ),
            (Scale::Ordinal { .. }, Some(_), Some(_)) => invalid(
                "pass_score",
                "a rubric with named levels passes at a level and names no score",
            ),
            (Scale::Flag, None, None) => Ok(()),
            (Scale::Flag, Some(_), _) => invalid(
                "pass_level",
                "a yes-or-no rubric passes on its better answer and names no level",
            ),
            (Scale::Flag, None, Some(_)) => invalid(
                "pass_score",
                "a yes-or-no rubric passes on its better answer and names no score",
            ),
            (Scale::Numeric { min, max }, None, Some(score)) => require(
                (*min..=*max).contains(&score),
                &format!("{field}.calibration.pass_score"),
                &format!("lies outside the rubric's scale, {min} to {max}"),
            ),
            (Scale::Numeric { .. }, None, None) => invalid(
                "pass_score",
                "a numeric rubric needs the score a person's judgement passes at",
            ),
            (Scale::Numeric { .. }, Some(_), _) => invalid(
                "pass_level",
                "a numeric rubric has no named levels; name the score it passes at",
            ),
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
            (Scale::Numeric { .. }, AssessmentValue::Number { value }) => {
                let bar = self.pass_score?;
                let passed = match rubric.direction {
                    MetricDirection::Higher => *value >= bar,
                    MetricDirection::Lower => *value <= bar,
                    MetricDirection::None => return None,
                };
                Some(if passed { 1.0 } else { 0.0 })
            }
            _ => None,
        }
    }

    /// The metric's number and the person's, turned so that more is better on
    /// both — what a ranking of the two compares.
    fn ranked(
        declared: &ExternalDeclaration,
        rubric: &Rubric,
        said: Option<f64>,
        value: &AssessmentValue,
    ) -> Option<(f64, f64)> {
        let turned = |direction: MetricDirection, number: f64| match direction {
            MetricDirection::Higher => Some(number),
            MetricDirection::Lower => Some(-number),
            MetricDirection::None => None,
        };
        Some((
            turned(declared.direction, said?)?,
            turned(
                rubric.direction,
                crate::judge::number(&rubric.scale, value)?,
            )?,
        ))
    }
}

/// How far a calibrated framework metric agreed with its people, beyond the
/// verdicts at the card's bar.
///
/// The verdict agreement is the one a result is held to, and it says nothing
/// about a bar a little either side: 60% at 0.7 may be 90% at 0.5 or a metric
/// that ranks answers the other way round from the people. So two more
/// readings ride beside it. Neither is applied to anything. The fitted bar is
/// found on the very items it is scored on, so it flatters itself; `held_out`
/// is the check it lacks — the same fit made without each case and scored on
/// that case — and `fitted_pass_interval` says how far the bar itself moves
/// when the set's cases are drawn again. Adopting a bar is still publishing the
/// card again, and a new context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalAgreement {
    #[serde(flatten)]
    pub verdicts: JudgeAgreement,
    /// Whether the metric orders answers as the people do, needing no bar:
    /// Goodman and Kruskal's gamma over the answered items, each side turned so
    /// more is better — one when every pair both sides told apart is ordered
    /// the same way, nought when the order says nothing, minus one when it is
    /// reversed. Absent when no pair was told apart on both sides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_agreement: Option<f64>,
    /// The pairs of items `rank_agreement` was counted over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranked_pairs: Option<usize>,
    /// Where `rank_agreement` plausibly lies: 95%, from its asymptotic standard
    /// error taken through Fisher's transform so it stays inside minus one and
    /// one. Absent when every pair told apart was ordered one way, where that
    /// method has nothing to say — the pair count beside it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_interval: Option<AgreementInterval>,
    /// The bar on the metric — one of the numbers it gave on this set — whose
    /// verdicts would have matched the people's most often, over every item;
    /// the nearest to the card's own `pass_at` among equals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fitted_pass_at: Option<f64>,
    /// How often, counted as `agreement` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fitted_agreement: Option<f64>,
    /// Where the fitted bar lands when the set's cases are drawn again with
    /// replacement, as often as [`REDRAWS`] says: the middle 95% of the bars
    /// those redraws fit. Wide says this set cannot tell one bar from another.
    /// The redraws are dealt from a digest of the cases, so the same set
    /// answers the same. Absent under two cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fitted_pass_interval: Option<PassAtRange>,
    /// How a bar fitted this way does on items it was not fitted on. Absent
    /// when the set's items are all one case's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_out: Option<HeldOutBar>,
}

/// A bar fitted without each fold of a calibration set and scored on that
/// fold, so every item is scored once by a bar that never saw it.
///
/// A fold is a case while the set holds at most [`EVERY_CASE_ITS_OWN_FOLD`]
/// cases — every bar fitted on all the others, which is the most a small set
/// can lend a fit — and one of ten dealt from a digest of the case beyond. A
/// case two people judged is one fold with both judgements either way, and the
/// same set is dealt the same way on every run. Compare it with `agreement`,
/// the card's own bar, which was never fitted on these items either;
/// `fitted_agreement` above both is the flattery a fit on everything carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HeldOutBar {
    pub items: usize,
    /// The share of items where the verdict at the bar fitted without them was
    /// the people's, over every item.
    pub agreement: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreement_interval: Option<AgreementInterval>,
    /// How many bars were fitted — one per fold holding an item. Absent from a
    /// result measured before folds were cases, which was dealt into two halves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folds: Option<usize>,
    /// The lowest and the highest of those bars. Far apart says the fitted bar
    /// is mostly this set's noise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fold_pass_range: Option<PassAtRange>,
    /// The two halves' bars, on a result measured before folds were cases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fold_pass_at: Vec<f64>,
}

/// Two bars on a metric, the lower first.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PassAtRange {
    pub low: f64,
    pub high: f64,
}

impl PassAtRange {
    fn of(bars: &[f64]) -> Option<Self> {
        let low = bars.iter().copied().min_by(f64::total_cmp)?;
        let high = bars.iter().copied().max_by(f64::total_cmp)?;
        Some(Self { low, high })
    }
}

/// While a calibration set holds at most this many cases, each is a fold.
pub const EVERY_CASE_ITS_OWN_FOLD: usize = 200;
/// How many folds a larger set is dealt into.
const DEALT_FOLDS: u64 = 10;
/// How many times a set's cases are drawn again for the fitted bar's interval.
pub const REDRAWS: usize = 1000;

/// The first eight bytes of a labelled digest, as a number.
fn digest_number(label: &str, parts: &[&str]) -> u64 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    for part in parts {
        hasher.update(b"\0");
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first)
}

/// Which fold each item's case is dealt to, and the folds that hold one.
pub(crate) fn calibration_folds(case_ids: &[&str]) -> Vec<u64> {
    let distinct: std::collections::BTreeSet<&str> = case_ids.iter().copied().collect();
    if distinct.len() <= EVERY_CASE_ITS_OWN_FOLD {
        let position: BTreeMap<&str, u64> = distinct
            .iter()
            .zip(0..)
            .map(|(case_id, at)| (*case_id, at))
            .collect();
        case_ids
            .iter()
            .map(|case_id| position.get(case_id).copied().unwrap_or_default())
            .collect()
    } else {
        case_ids
            .iter()
            .map(|case_id| digest_number("aiwatcher.calibration.fold", &[case_id]) % DEALT_FOLDS)
            .collect()
    }
}

/// A small deterministic generator for the redraws: SplitMix64, seeded from a
/// digest of the set, so a retry folds the same bytes.
struct Redraw(u64);

impl Redraw {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^= mixed >> 31;
        // A bound is a set's case count; the bias of a remainder is nothing
        // beside the spread being measured.
        usize::try_from(mixed % bound.max(1) as u64).unwrap_or_default()
    }
}

/// One item as a fit reads it: the metric's number and the person's verdict,
/// where a person gave one that passes or fails at all.
#[derive(Clone, Copy, Debug)]
struct Point {
    number: f64,
    person: Option<f64>,
    item: usize,
}

/// The bar, among the numbers of the items weighted in and the card's own,
/// whose verdicts matched the people's over the most weight — the card's
/// nearest among equals, the lower of two as near — and the weight matched.
///
/// `points` hold every item the metric gave a number, in ascending order of
/// it — one without a person's verdict is a bar and matches nothing — `weights` are per item, and nought leaves an
/// item out. One pass over the points, so a thousand redraws of a set cost a
/// thousand passes rather than a thousand times the square of it.
fn fitted_bar(
    points: &[Point],
    weights: &[f64],
    direction: MetricDirection,
    card: f64,
) -> (f64, f64) {
    let weight = |point: &Point| weights.get(point.item).copied().unwrap_or_default();
    let (mut ones, mut zeros) = (0.0, 0.0);
    for point in points {
        match point.person {
            Some(person) if person > 0.5 => ones += weight(point),
            Some(_) => zeros += weight(point),
            None => {}
        }
    }
    // What a bar at `bar` matches, given the weight strictly below it and the
    // weight at it or below.
    let matched = |ones_below: f64, zeros_below: f64, ones_at: f64, zeros_at: f64| match direction {
        // Passing is at the bar or above.
        MetricDirection::Higher => zeros_below + (ones - ones_below),
        // Passing is at the bar or below.
        MetricDirection::Lower => (ones_below + ones_at) + (zeros - zeros_below - zeros_at),
        MetricDirection::None => 0.0,
    };
    let mut candidates: Vec<(f64, f64)> = Vec::new();
    let (mut ones_below, mut zeros_below) = (0.0, 0.0);
    let mut card_seen = false;
    let mut at = 0;
    while at < points.len() {
        let number = points[at].number;
        let (mut ones_at, mut zeros_at, mut present) = (0.0, 0.0, false);
        while at < points.len() && points[at].number == number {
            let held = weight(&points[at]);
            present |= held > 0.0;
            match points[at].person {
                Some(person) if person > 0.5 => ones_at += held,
                Some(_) => zeros_at += held,
                None => {}
            }
            at += 1;
        }
        if !card_seen && card <= number {
            if card < number {
                candidates.push((card, matched(ones_below, zeros_below, 0.0, 0.0)));
            }
            card_seen = true;
        }
        if present || number == card {
            candidates.push((number, matched(ones_below, zeros_below, ones_at, zeros_at)));
        }
        ones_below += ones_at;
        zeros_below += zeros_at;
    }
    if !card_seen {
        candidates.push((card, matched(ones_below, zeros_below, 0.0, 0.0)));
    }
    let mut best = (card, f64::NEG_INFINITY);
    for (bar, agreeing) in candidates {
        let nearer = (bar - card).abs() < (best.0 - card).abs();
        if agreeing > best.1 + 1e-9 || ((agreeing - best.1).abs() <= 1e-9 && nearer) {
            best = (bar, agreeing);
        }
    }
    (best.0, best.1.max(0.0))
}

/// A 95% interval for Goodman and Kruskal's gamma over pairs, or `None` where
/// it is undefined: no pair told apart, or every one ordered the same way.
///
/// The asymptotic standard error is `2 / (P + Q)² · √Σᵢ (Q·Cᵢ − P·Dᵢ)²`, with
/// `P` and `Q` the concordant and discordant pairs and `Cᵢ`, `Dᵢ` the items
/// concordant and discordant with item `i` — checked against the spread of
/// gamma over fresh samples before it was written here. The interval is taken
/// on `atanh(gamma)` and turned back, which keeps it inside minus one and one.
fn gamma_interval(pairs: &[(f64, f64)]) -> Option<AgreementInterval> {
    const Z: f64 = 1.959_963_984_540_054;
    let mut concordant = vec![0.0_f64; pairs.len()];
    let mut discordant = vec![0.0_f64; pairs.len()];
    for (at, (x, y)) in pairs.iter().enumerate() {
        for (other, (other_x, other_y)) in pairs.iter().enumerate() {
            let (dx, dy) = (x - other_x, y - other_y);
            if at == other || dx.abs() <= 1e-12 || dy.abs() <= 1e-12 {
                continue;
            }
            if (dx > 0.0) == (dy > 0.0) {
                concordant[at] += 1.0;
            } else {
                discordant[at] += 1.0;
            }
        }
    }
    let same = concordant.iter().sum::<f64>() / 2.0;
    let reversed = discordant.iter().sum::<f64>() / 2.0;
    let told_apart = same + reversed;
    if told_apart <= 0.0 {
        return None;
    }
    let gamma = (same - reversed) / told_apart;
    if gamma.abs() >= 1.0 - 1e-12 {
        return None;
    }
    let spread = concordant
        .iter()
        .zip(&discordant)
        .map(|(c, d)| (reversed * c - same * d).powi(2))
        .sum::<f64>()
        .sqrt();
    let error = 2.0 / (told_apart * told_apart) * spread;
    let (centre, width) = (gamma.atanh(), Z * error / (1.0 - gamma * gamma));
    Some(AgreementInterval {
        low: (centre - width).tanh(),
        high: (centre + width).tanh(),
    })
}

/// Goodman and Kruskal's gamma over pairs: of the pairs of items both sides
/// told apart, the share ordered the same way less the share ordered the other
/// way — and how many such pairs there were. `None` when there were none.
///
/// Gamma rather than a tau, because a person's side is coarse by design: a
/// yes-or-no rubric ties half of every pair, and a tau corrected for ties still
/// stops short of one for a metric that put every yes above every no. Gamma
/// reads that as one — what it is — and says over how many pairs.
fn gamma(pairs: &[(f64, f64)]) -> Option<(f64, usize)> {
    let (mut same, mut reversed) = (0_usize, 0_usize);
    for (at, (x, y)) in pairs.iter().enumerate() {
        for (other_x, other_y) in &pairs[at + 1..] {
            let (dx, dy) = (x - other_x, y - other_y);
            if dx.abs() <= 1e-12 || dy.abs() <= 1e-12 {
                continue;
            }
            if (dx > 0.0) == (dy > 0.0) {
                same += 1;
            } else {
                reversed += 1;
            }
        }
    }
    let told_apart = same + reversed;
    (told_apart > 0).then(|| {
        (
            (same as f64 - reversed as f64) / told_apart as f64,
            told_apart,
        )
    })
}

/// What a result whose framework metrics were calibrated carries beside its
/// numbers: the set, and how far each metric's verdicts were its people's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalReport {
    pub calibration: VersionReference,
    /// One row per calibrated metric. Its `mean_absolute_difference` is over
    /// the two verdicts, so it is the share of answered items they differed on.
    pub agreement: Vec<ExternalAgreement>,
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
        let items: Vec<(Option<f64>, &AssessmentValue)> = set
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.rubric == calibrated.rubric)
            .map(|(index, item)| {
                (
                    said.get(&(index, spec.metric.clone())).copied().flatten(),
                    &item.value,
                )
            })
            .collect();
        let case_ids: Vec<&str> = set
            .items
            .iter()
            .filter(|item| item.rubric == calibrated.rubric)
            .map(|item| item.case_id.as_str())
            .collect();
        let at = |bar: &ExternalCalibration| {
            crate::judge::counted(
                &spec.metric,
                &calibrated.rubric,
                items.iter().map(|(number, value)| {
                    (
                        number.and_then(|number| bar.verdict(declared, number)),
                        bar.judged(rubric, value),
                    )
                }),
            )
        };
        let verdicts = at(calibrated);
        let ranked: Vec<(f64, f64)> = items
            .iter()
            .filter_map(|(number, value)| {
                ExternalCalibration::ranked(declared, rubric, *number, value)
            })
            .collect();
        // Whether the verdicts at a bar match the people's on the items chosen.
        let hits = |pass_at: f64, chosen: &dyn Fn(usize) -> bool| {
            let bar = ExternalCalibration {
                pass_at,
                ..calibrated.clone()
            };
            items
                .iter()
                .enumerate()
                .filter(|(at, _)| chosen(*at))
                .filter(|(_, (number, value))| {
                    matches!(
                        (
                            number.and_then(|number| bar.verdict(declared, number)),
                            bar.judged(rubric, value),
                        ),
                        (Some(metric), Some(person)) if (metric - person).abs() < 1e-9
                    )
                })
                .count()
        };
        let mut points: Vec<Point> = items
            .iter()
            .enumerate()
            .filter_map(|(item, (number, value))| {
                Some(Point {
                    number: (*number)?,
                    person: calibrated.judged(rubric, value),
                    item,
                })
            })
            .collect();
        points.sort_by(|one, other| one.number.total_cmp(&other.number));
        let fit =
            |weights: &[f64]| fitted_bar(&points, weights, declared.direction, calibrated.pass_at);
        let fitted = (!items.is_empty()).then(|| {
            let (bar, matched) = fit(&vec![1.0; items.len()]);
            (bar, matched / items.len() as f64)
        });
        let folds = calibration_folds(&case_ids);
        let occupied: std::collections::BTreeSet<u64> = folds.iter().copied().collect();
        let held_out = (occupied.len() >= 2).then(|| {
            let mut matched = 0;
            let mut bars = Vec::with_capacity(occupied.len());
            for fold in &occupied {
                let without: Vec<f64> = folds
                    .iter()
                    .map(|dealt| if dealt == fold { 0.0 } else { 1.0 })
                    .collect();
                let (bar, _) = fit(&without);
                matched += hits(bar, &|at| folds.get(at) == Some(fold));
                bars.push(bar);
            }
            HeldOutBar {
                items: items.len(),
                agreement: matched as f64 / items.len() as f64,
                agreement_interval: AgreementInterval::wilson(matched, items.len()),
                folds: Some(bars.len()),
                fold_pass_range: PassAtRange::of(&bars),
                fold_pass_at: Vec::new(),
            }
        });
        // The cases drawn again with replacement, each draw weighting a case's
        // items by how often it was drawn.
        let fitted_pass_interval = {
            let distinct: Vec<&str> = {
                let mut distinct = case_ids.clone();
                distinct.sort_unstable();
                distinct.dedup();
                distinct
            };
            (distinct.len() >= 2).then(|| {
                let of_case: Vec<Vec<usize>> = distinct
                    .iter()
                    .map(|case_id| {
                        case_ids
                            .iter()
                            .enumerate()
                            .filter(|(_, own)| *own == case_id)
                            .map(|(item, _)| item)
                            .collect()
                    })
                    .collect();
                let mut seed_parts = vec![spec.metric.as_str()];
                seed_parts.extend(distinct.iter().copied());
                let mut redraw = Redraw(digest_number("aiwatcher.calibration.redraw", &seed_parts));
                let mut bars: Vec<f64> = (0..REDRAWS)
                    .map(|_| {
                        let mut weights = vec![0.0; items.len()];
                        for _ in 0..distinct.len() {
                            for item in &of_case[redraw.below(distinct.len())] {
                                weights[*item] += 1.0;
                            }
                        }
                        fit(&weights).0
                    })
                    .collect();
                bars.sort_by(f64::total_cmp);
                let rank = |share: f64| {
                    let at = ((share * REDRAWS as f64).ceil() as usize).max(1) - 1;
                    bars[at.min(bars.len() - 1)]
                };
                PassAtRange {
                    low: rank(0.025),
                    high: rank(0.975),
                }
            })
        };
        let ranking = gamma(&ranked);
        agreement.push(ExternalAgreement {
            rank_agreement: ranking.map(|(gamma, _)| gamma),
            ranked_pairs: ranking.map(|(_, pairs)| pairs),
            rank_interval: gamma_interval(&ranked),
            fitted_pass_at: fitted.map(|(bar, _)| bar),
            fitted_agreement: fitted.map(|(_, share)| share),
            fitted_pass_interval,
            held_out,
            verdicts,
        });
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
            pass_score: None,
        }
    }

    fn scored_at(pass_at: f64, pass_score: f64) -> ExternalCalibration {
        ExternalCalibration {
            pass_score: Some(pass_score),
            ..calibration(pass_at, None)
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
        let numeric = rubric(
            Scale::Numeric { min: 1.0, max: 5.0 },
            MetricDirection::Higher,
        );
        let higher = graded(MetricDirection::Higher);

        assert!(
            calibration(0.7, Some("good"))
                .check("s", &higher, &levels)
                .is_ok()
        );
        assert!(scored_at(0.7, 4.0).check("s", &higher, &numeric).is_ok());
        assert!(calibration(0.7, None).check("s", &higher, &flag).is_ok());
        for (refused, declared, rubric, why) in [
            (calibration(0.7, None), &higher, &levels, "pass_level"),
            (calibration(0.7, Some("great")), &higher, &levels, "great"),
            (
                calibration(0.7, Some("good")),
                &higher,
                &flag,
                "names no level",
            ),
            (calibration(1.5, None), &higher, &flag, "range"),
            (
                calibration(0.7, None),
                &graded(MetricDirection::None),
                &flag,
                "neither end",
            ),
            (calibration(0.7, None), &higher, &numeric, "needs the score"),
            (
                scored_at(0.7, 7.0),
                &higher,
                &numeric,
                "outside the rubric's scale",
            ),
            (scored_at(0.7, 3.0), &higher, &flag, "names no score"),
            (
                ExternalCalibration {
                    pass_level: Some("good".into()),
                    ..scored_at(0.7, 3.0)
                },
                &higher,
                &levels,
                "names no score",
            ),
        ] {
            let reason = refused
                .check("s", declared, rubric)
                .unwrap_err()
                .to_string();
            assert!(reason.contains(why), "{why}: {reason}");
        }
    }

    #[test]
    fn a_verdict_is_a_pass_on_the_better_side_of_the_bar_on_both_sides() {
        let bar = calibration(0.7, Some("fair"));
        assert_eq!(
            bar.verdict(&graded(MetricDirection::Higher), 0.7),
            Some(1.0)
        );
        assert_eq!(
            bar.verdict(&graded(MetricDirection::Higher), 0.69),
            Some(0.0)
        );
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

        let out_of_five = rubric(
            Scale::Numeric { min: 1.0, max: 5.0 },
            MetricDirection::Higher,
        );
        let number = |value: f64| AssessmentValue::Number { value };
        assert_eq!(
            scored_at(0.7, 4.0).judged(&out_of_five, &number(4.0)),
            Some(1.0)
        );
        assert_eq!(
            scored_at(0.7, 4.0).judged(&out_of_five, &number(3.5)),
            Some(0.0)
        );
    }

    #[test]
    fn a_metric_held_against_scores_out_of_five_says_how_it_ranks_them_and_which_bar_they_support()
    {
        let rubric_ref = VersionReference {
            name: "helpful".into(),
            version: "r1".into(),
        };
        let card: Scorecard = serde_json::from_value(json!({
            "name": "calibrated",
            "scorers": [{
                "metric": "relevancy", "answer_path": "/text",
                "scorer": {
                    "kind": "external", "adapter": "deepeval", "metric": "answer_relevancy",
                    "declared": serde_json::to_value(graded(MetricDirection::Higher)).unwrap(),
                    "calibration": {"rubric": rubric_ref, "pass_at": 0.9, "pass_score": 4.0}
                }
            }]
        }))
        .unwrap();
        let rubrics = Rubrics::default().with(
            &rubric_ref,
            rubric(
                Scale::Numeric { min: 1.0, max: 5.0 },
                MetricDirection::Higher,
            ),
        );
        // The people score two answers 5 and 4 and two 2 and 1; the metric
        // orders all four as they do, and its card's bar of 0.9 passes one.
        let people = [5.0, 4.0, 2.0, 1.0];
        let numbers = [0.95, 0.7, 0.4, 0.2];
        let set = CalibrationSet {
            name: "people".into(),
            result: rubric_ref.clone(),
            items: people
                .iter()
                .enumerate()
                .map(|(at, score)| crate::CalibrationItem {
                    case_id: format!("case-{at}"),
                    repetition_id: "measurement-1".into(),
                    rubric: rubric_ref.clone(),
                    value: AssessmentValue::Number { value: *score },
                    author: "ada".into(),
                    standing_id: format!("s{at}"),
                    revision: 1,
                })
                .collect(),
            from_archive: false,
        };
        let said = numbers
            .iter()
            .enumerate()
            .map(|(at, number)| ((at, "relevancy".to_owned()), Some(*number)))
            .collect();

        let report = external_agreement(&card, &rubrics, &rubric_ref, &set, &said);
        let relevancy = &report.agreement[0];
        assert_eq!(
            relevancy.verdicts.agreement, 0.75,
            "0.7 failed a person's 4"
        );
        assert_eq!(
            (relevancy.rank_agreement, relevancy.ranked_pairs),
            (Some(1.0), Some(6))
        );
        assert_eq!(
            (relevancy.fitted_pass_at, relevancy.fitted_agreement),
            (Some(0.7), Some(1.0)),
            "at 0.7 its verdicts are the people's on every item"
        );
    }

    #[test]
    fn a_ranking_is_one_for_the_people_s_order_minus_one_for_its_reverse_and_counts_its_pairs() {
        let same = [(0.1, 1.0), (0.5, 2.0), (0.9, 3.0)];
        let reversed = [(0.1, 3.0), (0.5, 2.0), (0.9, 1.0)];
        assert_eq!(gamma(&same), Some((1.0, 3)));
        assert_eq!(gamma(&reversed), Some((-1.0, 3)));
        assert_eq!(
            gamma(&[(0.1, 1.0), (0.9, 1.0)]),
            None,
            "people gave one answer"
        );
        assert_eq!(gamma(&[(0.1, 1.0)]), None);
        // A yes-or-no rubric ties half the pairs. Every yes above every no is
        // the people's order, and reads as one over the four pairs told apart.
        let flagged = [(0.2, 0.0), (0.4, 0.0), (0.6, 1.0), (0.8, 1.0)];
        assert_eq!(gamma(&flagged), Some((1.0, 4)));
        let muddled = [(0.2, 0.0), (0.7, 0.0), (0.6, 1.0), (0.8, 1.0)];
        assert_eq!(gamma(&muddled), Some((0.5, 4)));
    }

    #[test]
    fn a_ranking_s_interval_is_its_standard_error_through_fisher_s_transform_and_absent_at_one() {
        // Ten pairs the people's way round and two the other, on a coarse side.
        let pairs = [
            (0.2, 0.0),
            (0.7, 0.0),
            (0.6, 1.0),
            (0.8, 1.0),
            (0.3, 0.0),
            (0.9, 1.0),
            (0.5, 1.0),
        ];
        let interval = gamma_interval(&pairs).expect("a gamma short of one has an interval");
        assert!(
            (interval.low - -0.310_586_715_315_065_6).abs() < 1e-9,
            "{interval:?}"
        );
        assert!(
            (interval.high - 0.958_784_503_377_518_1).abs() < 1e-9,
            "{interval:?}"
        );
        assert_eq!(
            gamma_interval(&[(0.1, 1.0), (0.5, 2.0), (0.9, 3.0)]),
            None,
            "every pair one way round"
        );
        assert_eq!(gamma_interval(&[(0.1, 1.0), (0.9, 1.0)]), None);
    }

    /// A card, its rubric, a set of yes-or-no judgements one per case, and
    /// what the metric said about each.
    type Flagged = (
        Scorecard,
        Rubrics,
        VersionReference,
        CalibrationSet,
        BTreeMap<(usize, String), Option<f64>>,
    );

    fn flagged(judged: &[(String, bool, f64)]) -> Flagged {
        let rubric_ref = VersionReference {
            name: "correct".into(),
            version: "r1".into(),
        };
        let card: Scorecard = serde_json::from_value(json!({
            "name": "calibrated",
            "scorers": [{
                "metric": "relevancy", "answer_path": "/text",
                "scorer": {
                    "kind": "external", "adapter": "deepeval", "metric": "answer_relevancy",
                    "declared": serde_json::to_value(graded(MetricDirection::Higher)).unwrap(),
                    "calibration": {"rubric": rubric_ref, "pass_at": 0.9}
                }
            }]
        }))
        .unwrap();
        let rubrics =
            Rubrics::default().with(&rubric_ref, rubric(Scale::Flag, MetricDirection::Higher));
        let set = CalibrationSet {
            name: "people".into(),
            result: rubric_ref.clone(),
            items: judged
                .iter()
                .map(|(case_id, value, _)| crate::CalibrationItem {
                    case_id: case_id.clone(),
                    repetition_id: "measurement-1".into(),
                    rubric: rubric_ref.clone(),
                    value: AssessmentValue::Flag { value: *value },
                    author: "ada".into(),
                    standing_id: format!("s-{case_id}"),
                    revision: 1,
                })
                .collect(),
            from_archive: false,
        };
        let said = judged
            .iter()
            .enumerate()
            .map(|(at, (_, _, number))| ((at, "relevancy".to_owned()), Some(*number)))
            .collect();
        (card, rubrics, rubric_ref, set, said)
    }

    #[test]
    fn a_bar_every_other_case_supports_holds_on_the_case_left_out() {
        let judged: Vec<(String, bool, f64)> = [
            (true, 0.8),
            (true, 0.7),
            (false, 0.3),
            (false, 0.2),
            (true, 0.8),
            (true, 0.7),
            (false, 0.3),
            (false, 0.2),
        ]
        .into_iter()
        .enumerate()
        .map(|(at, (value, number))| (format!("case-{at}"), value, number))
        .collect();
        let (card, rubrics, rubric_ref, set, said) = flagged(&judged);

        let report = external_agreement(&card, &rubrics, &rubric_ref, &set, &said);
        let row = &report.agreement[0];

        assert_eq!(
            row.verdicts.agreement, 0.5,
            "the card's 0.9 fails every yes"
        );
        assert_eq!(
            (row.fitted_pass_at, row.fitted_agreement),
            (Some(0.7), Some(1.0))
        );
        let held_out = row.held_out.as_ref().expect("eight cases, eight folds");
        assert_eq!((held_out.items, held_out.agreement), (8, 1.0));
        assert_eq!(held_out.folds, Some(8), "a case is a fold");
        assert_eq!(
            held_out.fold_pass_range,
            Some(PassAtRange {
                low: 0.7,
                high: 0.7
            })
        );
        assert!(held_out.agreement_interval.is_some());
    }

    #[test]
    fn a_bar_fitted_on_everything_flatters_itself_and_the_case_left_out_says_by_how_much() {
        // Leaving out the yes at 0.5 fits 0.8 on the rest, which fails it.
        let judged = vec![
            ("a".to_owned(), true, 0.8),
            ("b".to_owned(), false, 0.2),
            ("c".to_owned(), true, 0.5),
            ("d".to_owned(), false, 0.4),
        ];
        let (card, rubrics, rubric_ref, set, said) = flagged(&judged);

        let report = external_agreement(&card, &rubrics, &rubric_ref, &set, &said);
        let row = &report.agreement[0];

        assert_eq!(
            (row.fitted_pass_at, row.fitted_agreement),
            (Some(0.5), Some(1.0))
        );
        let held_out = row.held_out.as_ref().expect("four folds");
        assert_eq!(held_out.agreement, 0.75, "{held_out:?}");
        assert_eq!(
            held_out.fold_pass_range,
            Some(PassAtRange {
                low: 0.5,
                high: 0.8
            })
        );
    }

    #[test]
    fn a_set_of_one_case_has_nothing_to_hold_out_and_nothing_to_draw_again() {
        let judged = vec![
            ("only".to_owned(), true, 0.8),
            ("only".to_owned(), false, 0.3),
        ];
        let (card, rubrics, rubric_ref, set, said) = flagged(&judged);

        let report = external_agreement(&card, &rubrics, &rubric_ref, &set, &said);

        assert!(report.agreement[0].fitted_pass_at.is_some());
        assert!(report.agreement[0].held_out.is_none());
        assert!(report.agreement[0].fitted_pass_interval.is_none());
    }

    #[test]
    fn the_fitted_bar_s_interval_is_the_same_on_every_run_and_wider_where_people_disagree() {
        let clear: Vec<(String, bool, f64)> = (0..20)
            .map(|at| {
                let yes = at % 2 == 0;
                (format!("case-{at}"), yes, if yes { 0.8 } else { 0.2 })
            })
            .collect();
        let muddled: Vec<(String, bool, f64)> = (0..20)
            .map(|at| (format!("case-{at}"), at % 3 == 0, f64::from(at) / 20.0))
            .collect();
        let interval = |judged: &[(String, bool, f64)]| {
            let (card, rubrics, rubric_ref, set, said) = flagged(judged);
            external_agreement(&card, &rubrics, &rubric_ref, &set, &said).agreement[0]
                .fitted_pass_interval
                .expect("twenty cases")
        };

        let first = interval(&clear);
        assert_eq!(first, interval(&clear), "dealt from the set, not a clock");
        assert!(first.low <= 0.8 && first.high >= 0.2, "{first:?}");
        let wide = interval(&muddled);
        assert!(
            wide.high - wide.low > first.high - first.low || (first.high - first.low).abs() < 1e-9,
            "{wide:?} against {first:?}"
        );
    }

    #[test]
    fn a_large_set_is_dealt_into_ten_folds_by_its_cases() {
        let judged: Vec<(String, bool, f64)> = (0..(EVERY_CASE_ITS_OWN_FOLD + 50))
            .map(|at| {
                let yes = at % 2 == 0;
                (format!("case-{at}"), yes, if yes { 0.8 } else { 0.2 })
            })
            .collect();
        let (card, rubrics, rubric_ref, set, said) = flagged(&judged);

        let report = external_agreement(&card, &rubrics, &rubric_ref, &set, &said);

        let held_out = report.agreement[0].held_out.as_ref().expect("folds");
        assert_eq!((held_out.folds, held_out.agreement), (Some(10), 1.0));
    }

    #[test]
    fn one_pass_over_the_numbers_fits_the_bar_a_look_at_every_bar_would() {
        // Against the obvious fit: every candidate bar, every item.
        let naive = |points: &[Point], weights: &[f64], direction: MetricDirection, card: f64| {
            let mut bars: Vec<f64> = points
                .iter()
                .filter(|point| weights[point.item] > 0.0)
                .map(|point| point.number)
                .collect();
            bars.push(card);
            bars.sort_by(f64::total_cmp);
            bars.dedup();
            bars.into_iter()
                .map(|bar| {
                    let agreeing: f64 = points
                        .iter()
                        .filter_map(|point| {
                            let passes = match direction {
                                MetricDirection::Higher => point.number >= bar,
                                MetricDirection::Lower => point.number <= bar,
                                MetricDirection::None => return None,
                            };
                            ((point.person? > 0.5) == passes).then_some(weights[point.item])
                        })
                        // A fold from nought: a float sum of nothing is minus
                        // nought, which orders below nought.
                        .fold(0.0, |sum, weight| sum + weight);
                    (bar, agreeing)
                })
                .min_by(|(one, agreeing), (other, agreeing_other)| {
                    agreeing_other
                        .total_cmp(agreeing)
                        .then((one - card).abs().total_cmp(&(other - card).abs()))
                })
                .expect("the card is a bar")
        };
        let mut redraw = Redraw(7);
        for round in 0..300 {
            let count = 1 + redraw.below(12);
            let mut points: Vec<Point> = (0..count)
                .map(|item| Point {
                    number: redraw.below(6) as f64 / 5.0,
                    person: match redraw.below(3) {
                        0 => None,
                        1 => Some(0.0),
                        _ => Some(1.0),
                    },
                    item,
                })
                .collect();
            points.sort_by(|one, other| one.number.total_cmp(&other.number));
            let weights: Vec<f64> = (0..count).map(|_| redraw.below(3) as f64).collect();
            let card = redraw.below(6) as f64 / 5.0 + if round % 4 == 0 { 0.1 } else { 0.0 };
            for direction in [MetricDirection::Higher, MetricDirection::Lower] {
                assert_eq!(
                    fitted_bar(&points, &weights, direction, card),
                    naive(&points, &weights, direction, card),
                    "round {round}, {direction:?}, card {card}, {points:?}, {weights:?}"
                );
            }
        }
    }
}
