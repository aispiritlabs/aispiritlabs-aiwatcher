//! Scoring answers somebody already has, rather than asking for new ones.
//!
//! A run declared here calls no model of the application under test: it names
//! the answers it reads, and every case it emits is a fold over bytes that
//! were pinned before it started. That is the whole point of the mode — a new
//! scorecard measures an unchanged recording, so the difference between two
//! results is the measurement rather than the weather.
//!
//! The declaration is addressed by its content, so the plan that runs it names
//! a digest rather than carrying a body, and two starts of the same intention
//! land on one document. Nothing here reads a clock or opens a socket.

use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    CaseMeasurement, EvaluationContext, EvaluationManifest, EvaluationOrigin, Result, ResultStatus,
    SCHEMA_VERSION, Scorecard, VariantManifest, VersionReference, digest,
    reference::artifact,
    require,
    scorecard::Score,
    store::{self, Store},
    text,
};

/// The name a published result's scorer reference carries.
pub const SCORING_ENGINE: &str = "aiwatcher.scoring";
/// The version of the scorer vocabulary that did the measuring.
///
/// Two references, because they answer two questions. The suite reference is
/// the scorecard somebody declared; this one is the code that read it — and a
/// rewritten scorer measures an unchanged declaration differently, so bump
/// this whenever an existing scorer's answer changes for some input. Two
/// results measured under different rules then compare as what they are.
pub const SCORING_VERSION: &str = "1";

#[must_use]
pub fn scoring_engine() -> VersionReference {
    VersionReference {
        name: SCORING_ENGINE.into(),
        version: SCORING_VERSION.into(),
    }
}

/// Which cases are measured, and what they are measured against.
///
/// The dataset is the variant's: a manifest requires the cohort and the thing
/// it measures to name one dataset, and a cohort that could name another would
/// be a result about two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Cohort {
    pub case_manifest: ArtifactRef,
    /// A producer's split name, not proof of independence or permission.
    pub split: String,
    pub input_schema: ArtifactRef,
    pub expectations_schema: ArtifactRef,
}

/// One saved answer, as the recording holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordedAnswer {
    pub case_id: String,
    pub answer: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

/// The shape of the artifact a scoring run reads.
#[derive(Clone, Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordedAnswers {
    pub answers: Vec<RecordedAnswer>,
}

/// What a run of saved answers measures, and what it measures it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScoringRun {
    /// The logical result this run produces. A technical retry reuses it.
    pub evaluation_id: String,
    /// An independent measurement of the same variant, not an attempt counter.
    pub repetition_id: String,
    pub variant: VariantManifest,
    pub cohort: Cohort,
    /// The card, at a concrete version. A head would let a rewrite change what
    /// a started run measures between one attempt and the next.
    pub scorecard: VersionReference,
    /// The recording. Pinned by digest, so the answers cannot change under a
    /// retry — which is what makes re-running one cheap and honest.
    pub answers: ArtifactRef,
}

impl ScoringRun {
    pub fn validate(&self) -> Result<()> {
        text(&self.evaluation_id, "run.evaluation_id")?;
        text(&self.repetition_id, "run.repetition_id")?;
        self.variant.validate()?;
        artifact(&self.cohort.case_manifest, "run.cohort.case_manifest")?;
        text(&self.cohort.split, "run.cohort.split")?;
        artifact(&self.cohort.input_schema, "run.cohort.input_schema")?;
        artifact(
            &self.cohort.expectations_schema,
            "run.cohort.expectations_schema",
        )?;
        self.scorecard.validate("run.scorecard")?;
        artifact(&self.answers, "run.answers")
    }

    /// The content address of this declaration.
    pub fn id(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, "evaluation.scoring_run", self))
    }

    /// The manifest a result of this run publishes.
    ///
    /// The metrics come from the card rather than from the caller: which way a
    /// scorer's number is better is a fact about what it counts, and a context
    /// restating it would be free to disagree with the card it names.
    pub fn manifest(
        &self,
        card: &Scorecard,
        case_count: u64,
        ran_by: Option<&StepOrigin>,
    ) -> Result<EvaluationManifest> {
        require(
            card.name == self.scorecard.name,
            "run.scorecard.name",
            "names a different card from the one resolved",
        )?;
        Ok(EvaluationManifest {
            schema_version: SCHEMA_VERSION,
            origin: EvaluationOrigin {
                evaluation_id: self.evaluation_id.clone(),
                repetition_id: self.repetition_id.clone(),
                execution_id: ran_by.map(|origin| origin.execution_id.clone()),
                step_id: ran_by.and_then(|origin| origin.step_id.clone()),
            },
            variant: self.variant.clone(),
            context: EvaluationContext {
                dataset: self.variant.dataset.clone(),
                case_manifest: self.cohort.case_manifest.clone(),
                case_count,
                split: self.cohort.split.clone(),
                suite: self.scorecard.clone(),
                scorer: scoring_engine(),
                input_schema: self.cohort.input_schema.clone(),
                expectations_schema: self.cohort.expectations_schema.clone(),
                judge: None,
                metrics: card.metrics(),
            },
        })
    }
}

/// Which execution measured this, when one did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepOrigin {
    pub execution_id: String,
    pub step_id: Option<String>,
}

/// A declaration as it was stored, and who declared it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeclaredRun {
    pub id: String,
    pub run: ScoringRun,
    pub declared_by: String,
    pub declared_at: i64,
}

/// What one pass over the recording measured.
#[derive(Clone, Debug)]
pub struct Scored {
    pub cases: Vec<CaseMeasurement>,
    pub status: ResultStatus,
}

/// Score every answer the cohort selects, and nothing else.
///
/// The cohort decides: an answer for a case it does not select is not part of
/// this measurement, and a selected case nobody answered is absent rather than
/// zero — it lands in the unscored count, where an unavailable recording reads
/// as a gap in the evidence instead of as a bad score.
#[must_use]
pub fn score(
    card: &Scorecard,
    cohort: &BTreeMap<String, serde_json::Value>,
    answers: &[RecordedAnswer],
    repetition_id: &str,
) -> Scored {
    let mut found: BTreeMap<&str, Vec<&RecordedAnswer>> = BTreeMap::new();
    for answer in answers {
        if let Some((case_id, _)) = cohort.get_key_value(&answer.case_id) {
            found.entry(case_id).or_default().push(answer);
        }
    }

    let mut cases = Vec::with_capacity(found.len());
    let mut scored = 0;
    let mut failed = 0;
    for (case_id, answers) in found {
        let expected = &cohort[case_id];
        let measured = match answers.as_slice() {
            // One publication is one repetition, so two answers to one case
            // are two measurements this result has no way to tell apart.
            [] | [_, _, ..] => Err("the recording holds more than one answer for this case".into()),
            [answer] => measure(card, answer, expected),
        };
        let answer = answers.first();
        let trace_id = answer.and_then(|answer| answer.trace_id.clone());
        cases.push(CaseMeasurement {
            case_id: case_id.to_owned(),
            repetition_id: repetition_id.to_owned(),
            actual: match &measured {
                Ok(_) => answer.map(|answer| answer.answer.clone()),
                Err(_) => None,
            },
            metrics: measured.clone().unwrap_or_default(),
            error: measured.err().map(|reason| clip(&reason)),
            // A span is only addressable through the trace that holds it, and
            // evidence naming one without the other cannot be opened.
            span_id: trace_id
                .as_ref()
                .and_then(|_| answer.and_then(|answer| answer.span_id.clone())),
            trace_id,
        });
        if cases.last().is_some_and(|case| case.error.is_some()) {
            failed += 1;
        } else {
            scored += 1;
        }
    }

    let status = if scored == 0 {
        ResultStatus::Failed
    } else if failed > 0 || cases.len() < cohort.len() {
        ResultStatus::Partial
    } else {
        ResultStatus::Succeeded
    };
    Scored { cases, status }
}

/// Every metric, or none of them.
///
/// A case that answered three of four metrics would be counted into three
/// averages with a denominator that differs per metric, and nothing in the
/// numbers would say so. A scorer that cannot read this case makes the case a
/// failure with the reason instead.
fn measure(
    card: &Scorecard,
    answer: &RecordedAnswer,
    expected: &serde_json::Value,
) -> std::result::Result<BTreeMap<String, f64>, String> {
    let mut metrics = BTreeMap::new();
    for spec in &card.scorers {
        match spec.measure(&answer.answer, expected) {
            Score::Measured(value) => {
                metrics.insert(spec.metric.clone(), value);
            }
            Score::Unscored(reason) => return Err(format!("{}: {reason}", spec.metric)),
        }
    }
    Ok(metrics)
}

/// A reason a result can store: bounded, on a character boundary, one line.
fn clip(reason: &str) -> String {
    let reason: String = reason
        .chars()
        .map(|letter| if letter.is_control() { ' ' } else { letter })
        .collect();
    let reason = reason.trim();
    match reason.char_indices().take_while(|(at, _)| *at < 512).last() {
        Some((at, letter)) if at + letter.len_utf8() < reason.len() => {
            reason[..at + letter.len_utf8()].to_owned()
        }
        Some(_) => reason.to_owned(),
        None => "unscorable".to_owned(),
    }
}

pub(crate) async fn declare(
    store: &Store,
    run: &ScoringRun,
    declared_by: &str,
    now: i64,
) -> Result<DeclaredRun> {
    run.validate()?;
    let id = run.id()?;
    let key = store::scoring_run(&id);
    let declared = DeclaredRun {
        id,
        run: run.clone(),
        declared_by: declared_by.to_owned(),
        declared_at: now,
    };
    if !store.create(&key, &declared).await? {
        return store
            .read(&key)
            .await?
            .ok_or(crate::EvaluationError::Unavailable(
                crate::EvidenceState::CorruptArtifact,
            ));
    }
    Ok(declared)
}

pub(crate) async fn declared(store: &Store, id: &str) -> Result<Option<DeclaredRun>> {
    store.read(&store::scoring_run(id)).await
}
