//! A model asked a rubric's question, and what it may and may not claim.
//!
//! Every other measurement here is a fold over bytes somebody can read again.
//! A judge's is not: it is a model call, and whether it would answer the same
//! way tomorrow depends on a provider, a revision and a configuration rather
//! than on a digest. So a judge is admitted under its own rule rather than the
//! one the source adapters keep — its configuration pinned by content, a
//! calibration set of human judgements that is part of the evidence, its
//! disagreement with those judgements stored beside the result, and the result
//! marked as something a model said at the time.
//!
//! Nothing in this module opens a socket. It builds the calls, reads the
//! replies and folds the agreement; [`JudgeModel`] is the port a deployment's
//! client implements, and the executor that holds it decides how many calls run
//! at once.

use std::collections::BTreeMap;

use aiwatcher_core::{ArtifactKind, ArtifactRef};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    AssessmentValue, EvaluationError, Result, Rubric, Rubrics, SCHEMA_VERSION, Scale, Scorecard,
    VersionReference, canonical, digest, require,
    store::{self, Store},
    text,
};

const MAX_INSTRUCTIONS: usize = 8 * 1024;
const MAX_TOKENS: u32 = 4096;
/// The human judgements one calibration set may hold.
const MAX_ITEMS: usize = 2_000;

/// How a judge is asked, beyond the rubric's own words.
///
/// Part of the evidence's context through its digest, so asking at another
/// temperature is another measurement rather than the same one again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeSettings {
    /// Said after the rubric, never instead of it: a person and a judge are
    /// given the same form, and this is what only the model is told.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default = "JudgeSettings::default_max_tokens")]
    pub max_tokens: u32,
}

impl Default for JudgeSettings {
    fn default() -> Self {
        Self {
            instructions: String::new(),
            temperature: 0.0,
            seed: None,
            max_tokens: Self::default_max_tokens(),
        }
    }
}

impl JudgeSettings {
    const fn default_max_tokens() -> u32 {
        256
    }

    fn validate(&self) -> Result<()> {
        require(
            self.instructions.len() <= MAX_INSTRUCTIONS && !self.instructions.contains('\0'),
            "run.judge.settings.instructions",
            "must be at most 8 KiB of text",
        )?;
        require(
            self.temperature.is_finite() && (0.0..=2.0).contains(&self.temperature),
            "run.judge.settings.temperature",
            "must be a temperature from 0 to 2",
        )?;
        require(
            (1..=MAX_TOKENS).contains(&self.max_tokens),
            "run.judge.settings.max_tokens",
            "must allow 1–4096 tokens",
        )
    }

    /// The reference a context pins these settings by, and the bytes behind it.
    pub(crate) fn artifact(&self) -> Result<(ArtifactRef, Vec<u8>)> {
        let bytes = canonical(self)?;
        let digest = store::hash(&bytes);
        Ok((
            ArtifactRef {
                name: "judge-settings.json".into(),
                uri: format!("evaluation://judge-settings/{digest}"),
                digest,
                size_bytes: Some(bytes.len() as u64),
                content_type: "application/json".into(),
                kind: ArtifactKind::Blob,
                schema_ref: None,
            },
            bytes,
        ))
    }
}

/// Which judge a run is measured by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgeDeclaration {
    /// The judge profile this deployment was configured with — `openai` or
    /// `llamacpp`. A profile is declared rather than detected, and a run
    /// declared for one is refused by a deployment holding the other.
    pub provider: String,
    /// The model the provider is asked for, and the revision the author pins.
    /// Nothing can check a provider served that revision, which is part of why
    /// the result is marked as a model's word rather than re-readable bytes.
    pub model: VersionReference,
    #[serde(default)]
    pub settings: JudgeSettings,
    /// A calibration set this registry took, by its name and digest.
    pub calibration: VersionReference,
}

impl JudgeDeclaration {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            matches!(self.provider.as_str(), "openai" | "llamacpp"),
            "run.judge.provider",
            "must be a judge profile this deployment implements: openai or llamacpp",
        )?;
        self.model.validate("run.judge.model")?;
        self.calibration.validate("run.judge.calibration")?;
        self.settings.validate()
    }
}

/// One person's judgement of one case, which a judge is measured against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CalibrationItem {
    pub case_id: String,
    pub repetition_id: String,
    pub rubric: VersionReference,
    pub value: AssessmentValue,
    pub author: String,
    /// The revision that stood when the set was taken. A later change of mind
    /// is a later set, never a quiet re-reading of this one.
    pub standing_id: String,
    pub revision: u32,
}

/// The human judgements a judge is calibrated against, frozen.
///
/// Taken from a published result, whose answers stay readable at the version
/// named, and from the assessments people made of its cases under the rubric
/// versions asked for — so the judge is put the same answers, under the same
/// words, that the people were.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CalibrationSet {
    pub name: String,
    /// The evaluation, by its ID and the result version its cases are read at.
    pub result: VersionReference,
    pub items: Vec<CalibrationItem>,
}

impl CalibrationSet {
    pub(crate) fn version(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, "evaluation.calibration", self))
    }
}

/// What a caller asks to be frozen.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalibrationRequest {
    pub name: String,
    pub evaluation_id: String,
    /// The rubric versions whose human judgements are taken.
    #[schema(min_items = 1, max_items = 32)]
    pub rubrics: Vec<VersionReference>,
}

/// One frozen set, and who took it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CalibrationVersion {
    pub version: String,
    pub calibration: CalibrationSet,
    pub taken_by: String,
    pub taken_at: i64,
}

/// One chat message a judge is sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JudgeMessage {
    pub role: &'static str,
    pub content: String,
}

/// One question to a judge, complete. What the port sends and nothing more.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JudgeCall {
    pub model: String,
    pub messages: Vec<JudgeMessage>,
    pub temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub max_tokens: u32,
    /// The JSON Schema a reply has to satisfy, for a provider that decodes
    /// against one. Asked for rather than hoped for: a small model told "true
    /// or false" answers `"false"` in quotes often enough to fail every case it
    /// disagrees with, and a reply that is a value of another kind is refused
    /// rather than read generously.
    pub schema: serde_json::Value,
}

/// What the judge said, verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeReply {
    pub content: String,
    /// What the provider said it answered with. Kept beside the reply rather
    /// than checked against the declaration: the words a provider uses for a
    /// model are its own — an alias, a file, a dated snapshot — and a refusal
    /// on a spelling would refuse every honest provider that spells it another
    /// way.
    #[serde(default)]
    pub served: Served,
}

/// A provider's own claim about what served one reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Served {
    /// The response's `model`: what the provider says answered, which for a
    /// hosted API is often the dated snapshot a moving name resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The response's `system_fingerprint`: the backend configuration, which
    /// changes when the provider changes what serves the same model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Why no reply came back.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JudgeFailure {
    /// Nothing was decided: the provider could not be reached, timed out or
    /// said it was overloaded. Worth asking again.
    #[error("the judge could not be reached: {0}")]
    Unavailable(String),
    /// The provider refused the request as asked — a model it does not serve,
    /// a credential it does not accept. Asking again says the same thing.
    #[error("the judge refused the request: {0}")]
    Refused(String),
}

/// A deployment's judge client.
#[async_trait]
pub trait JudgeModel: Send + Sync + std::fmt::Debug {
    /// The profile this client speaks, which a declaration has to name.
    fn provider(&self) -> &str;

    async fn ask(&self, call: &JudgeCall) -> std::result::Result<JudgeReply, JudgeFailure>;
}

/// The question a judge is asked about one answer.
///
/// Every word comes from the rubric, then the run's own instructions; the
/// shape of the reply is fixed here so a reply can be read without guessing.
/// Changing these words changes what a judge measures, which is a new
/// `SCORING_VERSION`.
#[must_use]
pub fn ask(
    rubric: &Rubric,
    judge: &JudgeDeclaration,
    answer: &serde_json::Value,
    expected: Option<&serde_json::Value>,
) -> JudgeCall {
    let mut system = format!(
        "You are an evaluator. Judge the answer you are given against this rubric and nothing \
         else.\n\nQuestion: {}\n",
        rubric.question
    );
    if !rubric.guidance.is_empty() {
        system.push_str(&format!("Guidance: {}\n", rubric.guidance));
    }
    system.push_str(&match &rubric.scale {
        Scale::Numeric { min, max } => format!("The value is a number from {min} to {max}.\n"),
        Scale::Ordinal { levels } => format!(
            "The value is exactly one of these levels: {}.\n",
            levels
                .iter()
                .map(|level| format!("\"{level}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Scale::Flag => "The value is true or false.\n".to_owned(),
    });
    if !judge.settings.instructions.is_empty() {
        system.push_str(&format!("\n{}\n", judge.settings.instructions));
    }
    system.push_str("\nReply with only a JSON object of the form {\"value\": ...}.");
    let mut user = format!("Answer:\n{}", rendered(answer));
    if let Some(expected) = expected {
        user.push_str(&format!("\n\nExpected answer:\n{}", rendered(expected)));
    }
    JudgeCall {
        schema: reply_schema(&rubric.scale),
        model: judge.model.name.clone(),
        messages: vec![
            JudgeMessage {
                role: "system",
                content: system,
            },
            JudgeMessage {
                role: "user",
                content: user,
            },
        ],
        temperature: judge.settings.temperature,
        seed: judge.settings.seed,
        max_tokens: judge.settings.max_tokens,
    }
}

/// The one shape a reply may take on this scale.
#[must_use]
pub fn reply_schema(scale: &Scale) -> serde_json::Value {
    let value = match scale {
        Scale::Numeric { .. } => serde_json::json!({"type": "number"}),
        Scale::Ordinal { levels } => serde_json::json!({"type": "string", "enum": levels}),
        Scale::Flag => serde_json::json!({"type": "boolean"}),
    };
    serde_json::json!({
        "type": "object",
        "properties": {"value": value},
        "required": ["value"],
        "additionalProperties": false,
    })
}

fn rendered(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

/// A reply, read as a value on the rubric's scale and the number it scores.
///
/// # Errors
///
/// Why the reply is not an answer on this scale — in words that never quote
/// it, because a reply can repeat the answer it was shown.
pub fn read(
    rubric: &Rubric,
    reply: &JudgeReply,
) -> std::result::Result<(AssessmentValue, f64), String> {
    let content = reply.content.trim();
    let object = match (content.find('{'), content.rfind('}')) {
        (Some(open), Some(close)) if open < close => &content[open..=close],
        _ => return Err("the judge's reply holds no JSON object".into()),
    };
    let parsed: serde_json::Value = serde_json::from_str(object)
        .map_err(|_| "the judge's reply is not a JSON object".to_owned())?;
    let said = parsed
        .get("value")
        .ok_or_else(|| "the judge's reply has no value".to_owned())?;
    let value = match &rubric.scale {
        Scale::Numeric { .. } => said.as_f64().map(|value| AssessmentValue::Number { value }),
        Scale::Ordinal { .. } => said.as_str().map(|value| AssessmentValue::Level {
            value: value.to_owned(),
        }),
        Scale::Flag => said.as_bool().map(|value| AssessmentValue::Flag { value }),
    }
    .ok_or_else(|| {
        "the judge answered with a value of another kind than this rubric's".to_owned()
    })?;
    rubric
        .scale
        .admits(&value)
        .map_err(|_| "the judge answered outside this rubric's scale".to_owned())?;
    let number = number(&rubric.scale, &value)
        .ok_or_else(|| "the judge answered outside this rubric's scale".to_owned())?;
    Ok((value, number))
}

/// The number a value on a scale scores: itself, its position among the
/// levels, or one and nought.
#[must_use]
pub fn number(scale: &Scale, value: &AssessmentValue) -> Option<f64> {
    match (scale, value) {
        (Scale::Numeric { .. }, AssessmentValue::Number { value }) => Some(*value),
        (Scale::Ordinal { levels }, AssessmentValue::Level { value }) => levels
            .iter()
            .position(|level| level == value)
            .map(|at| at as f64),
        (Scale::Flag, AssessmentValue::Flag { value }) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// How far a judge agreed with the people it was calibrated against, for one
/// metric.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JudgeAgreement {
    pub metric: String,
    pub rubric: VersionReference,
    /// The human judgements the set holds under this rubric.
    pub items: usize,
    /// How many of them the judge answered on the rubric's scale. The rest
    /// were asked and are part of the disagreement, not missing from it.
    pub answered: usize,
    /// The fraction of the set where the judge said what the person said.
    /// Over every item rather than over the answered ones, so a judge that
    /// declines the hard cases does not agree its way to a better number.
    pub agreement: f64,
    /// The mean distance between the two over the answered items, in the
    /// metric's unit. Absent when the judge answered none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_absolute_difference: Option<f64>,
}

/// One thing a provider said it served, and how many of the run's replies
/// said so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServedModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub replies: usize,
}

/// What a judge-scored result carries beside its numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JudgeReport {
    pub calibration: VersionReference,
    pub agreement: Vec<JudgeAgreement>,
    /// What the provider said served the replies, over every question the run
    /// asked. The declared model and revision are the author's word; this is
    /// the provider's, and more than one row means the run's answers did not
    /// all come from one thing. Empty when no reply named anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served: Vec<ServedModel>,
}

/// Fold what a judge said about the calibration set into agreement per metric.
///
/// `said` is keyed by the item's position in the set and the metric, and holds
/// the number the judge's reply scored — or nothing where it was not one.
#[must_use]
pub fn agreement(
    card: &Scorecard,
    rubrics: &Rubrics,
    calibration: &VersionReference,
    set: &CalibrationSet,
    said: &BTreeMap<(usize, String), Option<f64>>,
) -> JudgeReport {
    let mut agreement = Vec::new();
    for spec in &card.scorers {
        let (Some(pinned), Some(rubric)) = (
            spec.scorer.rubric(),
            spec.scorer.rubric().and_then(|pinned| rubrics.get(pinned)),
        ) else {
            continue;
        };
        let mut items = 0;
        let mut matched = 0;
        let mut distances = Vec::new();
        for (index, item) in set.items.iter().enumerate() {
            if item.rubric != *pinned {
                continue;
            }
            items += 1;
            let (Some(Some(judge)), Some(person)) = (
                said.get(&(index, spec.metric.clone())),
                number(&rubric.scale, &item.value),
            ) else {
                continue;
            };
            let distance = (judge - person).abs();
            if distance < 1e-9 {
                matched += 1;
            }
            distances.push(distance);
        }
        agreement.push(JudgeAgreement {
            metric: spec.metric.clone(),
            rubric: pinned.clone(),
            items,
            answered: distances.len(),
            agreement: if items == 0 {
                0.0
            } else {
                f64::from(matched) / items as f64
            },
            mean_absolute_difference: (!distances.is_empty())
                .then(|| distances.iter().sum::<f64>() / distances.len() as f64),
        });
    }
    JudgeReport {
        calibration: calibration.clone(),
        agreement,
        served: Vec::new(),
    }
}

/// Which served models a set of replies names, counted, in a stable order —
/// the report is part of the bytes a result's version is the address of.
#[must_use]
pub fn served<'a>(replies: impl IntoIterator<Item = &'a JudgeReply>) -> Vec<ServedModel> {
    let mut counted: BTreeMap<&Served, usize> = BTreeMap::new();
    for reply in replies {
        *counted.entry(&reply.served).or_default() += 1;
    }
    counted
        .into_iter()
        .filter(|(served, _)| served.model.is_some() || served.fingerprint.is_some())
        .map(|(served, replies)| ServedModel {
            model: served.model.clone(),
            fingerprint: served.fingerprint.clone(),
            replies,
        })
        .collect()
}

/// The address of one question, over every byte the port would send.
fn question(call: &JudgeCall) -> Result<String> {
    digest(&(SCHEMA_VERSION, "evaluation.judge_question", call))
}

/// The reply this run's judge already gave to exactly this question.
pub(crate) async fn remembered(
    store: &Store,
    declaration: &str,
    call: &JudgeCall,
) -> Result<Option<JudgeReply>> {
    text(declaration, "run")?;
    store
        .read(&store::judge_reply(declaration, &question(call)?))
        .await
}

/// Keep a reply before it is used, and hand back the one that stands.
///
/// The first write wins: two attempts that both asked keep one answer, and the
/// fold reads the kept one, so the result either of them publishes is the same
/// bytes.
pub(crate) async fn remember(
    store: &Store,
    declaration: &str,
    call: &JudgeCall,
    reply: JudgeReply,
) -> Result<JudgeReply> {
    text(declaration, "run")?;
    let key = store::judge_reply(declaration, &question(call)?);
    if store.create(&key, &reply).await? {
        return Ok(reply);
    }
    store.read(&key).await?.ok_or(EvaluationError::Unavailable(
        crate::EvidenceState::MissingArtifact,
    ))
}

pub(crate) async fn keep_settings(store: &Store, settings: &JudgeSettings) -> Result<ArtifactRef> {
    let (reference, bytes) = settings.artifact()?;
    store
        .0
        .create(&store::judge_settings(&reference.digest), bytes)
        .await?;
    Ok(reference)
}

/// The settings a context pinned, re-verified against the digest it pinned.
pub(crate) async fn settings(store: &Store, pinned: &ArtifactRef) -> Result<Option<JudgeSettings>> {
    let Some(bytes) = store.0.get(&store::judge_settings(&pinned.digest)).await? else {
        return Ok(None);
    };
    if store::hash(&bytes) != pinned.digest {
        return Err(EvaluationError::Unavailable(
            crate::EvidenceState::CorruptArtifact,
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| EvaluationError::Unavailable(crate::EvidenceState::CorruptArtifact))
}

pub(crate) async fn keep_calibration(
    store: &Store,
    set: CalibrationSet,
    taken_by: &str,
    now: i64,
) -> Result<CalibrationVersion> {
    text(&set.name, "calibration.name")?;
    require(
        !set.items.is_empty(),
        "calibration",
        "holds no human judgement under the rubrics asked for; a judge calibrated against \
         nothing is refused rather than defaulted",
    )?;
    require(
        set.items.len() <= MAX_ITEMS,
        "calibration",
        "holds more than 2000 judgements",
    )?;
    let version = set.version()?;
    let key = store::calibration(&version);
    let taken = CalibrationVersion {
        version,
        calibration: set,
        taken_by: taken_by.to_owned(),
        taken_at: now,
    };
    if !store.create(&key, &taken).await? {
        return store.read(&key).await?.ok_or(EvaluationError::Unavailable(
            crate::EvidenceState::CorruptArtifact,
        ));
    }
    Ok(taken)
}

pub(crate) async fn calibration(
    store: &Store,
    version: &str,
) -> Result<Option<CalibrationVersion>> {
    text(version, "calibration.version")?;
    let Some(taken) = store
        .read::<CalibrationVersion>(&store::calibration(version))
        .await?
    else {
        return Ok(None);
    };
    if taken.calibration.version()? != version {
        return Err(EvaluationError::Unavailable(
            crate::EvidenceState::CorruptArtifact,
        ));
    }
    Ok(Some(taken))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetricDirection, Scorer, ScorerSpec};
    use serde_json::json;

    fn rubric(scale: Scale) -> Rubric {
        Rubric {
            name: "helpful".into(),
            question: "Does the answer help?".into(),
            guidance: "Help means it answers what was asked.".into(),
            scale,
            direction: MetricDirection::Higher,
        }
    }

    fn judge() -> JudgeDeclaration {
        JudgeDeclaration {
            provider: "llamacpp".into(),
            model: VersionReference {
                name: "gemma".into(),
                version: "q4".into(),
            },
            settings: JudgeSettings {
                instructions: "Be strict.".into(),
                seed: Some(7),
                ..JudgeSettings::default()
            },
            calibration: VersionReference {
                name: "people".into(),
                version: "c".repeat(64),
            },
        }
    }

    fn reply(content: &str) -> JudgeReply {
        JudgeReply {
            content: content.into(),
            served: Served::default(),
        }
    }

    #[test]
    fn a_judge_is_given_the_rubrics_words_and_then_the_runs_and_asked_for_one_shape() {
        let call = ask(
            &rubric(Scale::Ordinal {
                levels: vec!["bad".into(), "good".into()],
            }),
            &judge(),
            &json!("Warsaw"),
            Some(&json!({"city": "Warsaw"})),
        );
        let system = &call.messages[0].content;
        assert!(
            system.contains("Question: Does the answer help?"),
            "{system}"
        );
        assert!(system.contains("Guidance: Help means"), "{system}");
        assert!(system.contains("\"bad\", \"good\""), "{system}");
        assert!(
            system.find("Be strict.") > system.find("Guidance"),
            "the run's words come after the form's: {system}"
        );
        assert_eq!(
            call.messages[1].content,
            "Answer:\nWarsaw\n\nExpected answer:\n{\"city\":\"Warsaw\"}"
        );
        assert_eq!((call.model.as_str(), call.seed), ("gemma", Some(7)));
        assert_eq!(
            call.schema["properties"]["value"]["enum"],
            serde_json::json!(["bad", "good"]),
            "a reply is decoded against the levels, not hoped to name one"
        );
    }

    #[test]
    fn a_reply_is_a_value_on_the_scale_or_a_reason_that_does_not_quote_it() {
        let levels = rubric(Scale::Ordinal {
            levels: vec!["bad".into(), "fine".into(), "good".into()],
        });
        assert_eq!(
            read(&levels, &reply("```json\n{\"value\": \"fine\"}\n```")).unwrap(),
            (
                AssessmentValue::Level {
                    value: "fine".into()
                },
                1.0
            )
        );
        let bounded = rubric(Scale::Numeric { min: 1.0, max: 5.0 });
        assert_eq!(read(&bounded, &reply("{\"value\": 4}")).unwrap().1, 4.0);
        for (scale, said) in [
            (&bounded, "{\"value\": 9}"),
            (&bounded, "{\"value\": \"four SECRET\"}"),
            (&levels, "{\"value\": \"SECRET\"}"),
            (&levels, "the answer mentions SECRET"),
        ] {
            let refused = read(scale, &reply(said)).unwrap_err();
            assert!(!refused.contains("SECRET"), "{refused}");
        }
        let flag = rubric(Scale::Flag);
        assert_eq!(read(&flag, &reply("{\"value\": false}")).unwrap().1, 0.0);
    }

    #[test]
    fn agreement_counts_what_the_judge_declined_against_it() {
        let pinned = VersionReference {
            name: "helpful".into(),
            version: "r1".into(),
        };
        let flag = rubric(Scale::Flag);
        let card = Scorecard {
            name: "judged".into(),
            description: String::new(),
            scorers: vec![ScorerSpec {
                metric: "helpful".into(),
                answer_path: String::new(),
                expected_path: String::new(),
                scorer: Scorer::Judge {
                    rubric: pinned.clone(),
                },
            }],
        };
        let item = |case: &str, value: bool| CalibrationItem {
            case_id: case.into(),
            repetition_id: "r".into(),
            rubric: pinned.clone(),
            value: AssessmentValue::Flag { value },
            author: "ada".into(),
            standing_id: "s".into(),
            revision: 1,
        };
        let set = CalibrationSet {
            name: "people".into(),
            result: VersionReference {
                name: "baseline".into(),
                version: "v".into(),
            },
            items: vec![
                item("a", true),
                item("b", false),
                item("c", true),
                item("d", true),
            ],
        };
        let said = BTreeMap::from([
            ((0, "helpful".to_owned()), Some(1.0)),
            ((1, "helpful".to_owned()), Some(1.0)),
            ((2, "helpful".to_owned()), None),
            ((3, "helpful".to_owned()), Some(1.0)),
        ]);
        let calibration = VersionReference {
            name: "people".into(),
            version: set.version().unwrap(),
        };
        let report = agreement(
            &card,
            &Rubrics::default().with(&pinned, flag),
            &calibration,
            &set,
            &said,
        );
        assert_eq!(report.calibration, calibration);
        let helpful = &report.agreement[0];
        assert_eq!((helpful.items, helpful.answered), (4, 3));
        assert_eq!(
            helpful.agreement, 0.5,
            "two of four: the one it declined is not agreement"
        );
        assert!((helpful.mean_absolute_difference.unwrap() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn served_models_are_counted_in_one_order_and_a_reply_that_named_nothing_is_not_a_row() {
        let by = |model: &str, fingerprint: &str| JudgeReply {
            content: String::new(),
            served: Served {
                model: Some(model.into()),
                fingerprint: Some(fingerprint.into()),
            },
        };
        let replies = [
            by("gpt-4o-2024-08-06", "fp_b"),
            reply("{}"),
            by("gpt-4o-2024-08-06", "fp_a"),
            by("gpt-4o-2024-08-06", "fp_b"),
        ];
        let counted = served(&replies);
        assert_eq!(
            counted
                .iter()
                .map(|row| (row.fingerprint.as_deref(), row.replies))
                .collect::<Vec<_>>(),
            vec![(Some("fp_a"), 1), (Some("fp_b"), 2)],
            "two backends answered one run, and the order does not depend on which came first"
        );
        let mut reversed = replies.clone();
        reversed.reverse();
        assert_eq!(served(&reversed), counted);
    }

    #[test]
    fn settings_are_pinned_by_the_digest_of_what_they_say() {
        let (first, _) = JudgeSettings::default().artifact().unwrap();
        let (again, _) = JudgeSettings::default().artifact().unwrap();
        let (warmer, _) = JudgeSettings {
            temperature: 0.7,
            ..JudgeSettings::default()
        }
        .artifact()
        .unwrap();
        assert_eq!(first, again);
        assert_ne!(first.digest, warmer.digest);
        let mut refused = judge();
        refused.provider = "somewhere".into();
        assert!(refused.validate().is_err());
    }
}
