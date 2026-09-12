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

use aiwatcher_core::{ArtifactKind, ArtifactRef};
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

impl EvaluationContext {
    /// Whether this context was measured by the scorer vocabulary compiled
    /// into aiwatcher, rather than by code a producer ran.
    ///
    /// Asked by name only. A version this deployment does not implement is
    /// still one of ours, and the refusal that follows says which version this
    /// binary scores with — rather than asking an adapter for a file nobody
    /// could ever stage.
    #[must_use]
    pub fn scored_here(&self) -> bool {
        self.scorer.name == SCORING_ENGINE
    }
}

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
    /// How many cases this cohort selects. Declared rather than counted from
    /// the manifest's bytes, because it is part of what an operator admits: a
    /// source that later resolves another number is a different cohort under
    /// an admitted pair's name.
    #[schema(minimum = 1)]
    pub case_count: u64,
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
        require(
            self.cohort.case_count > 0,
            "run.cohort.case_count",
            "requires at least one selected case",
        )?;
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
                case_count: self.cohort.case_count,
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

/// A declaration, with what an operator needs before it may run.
///
/// The manifest is served rather than left to be assembled, for the reason an
/// approval's address is: its metrics are derived from the card, and a person
/// writing them out by hand to admit the pair would be a second answer to what
/// this run measures. The execution that will run it is not in it yet, and
/// does not need to be — an approval admits a variant and a context, never a
/// particular run of them.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ScoringRunView {
    pub declaration: DeclaredRun,
    pub manifest: EvaluationManifest,
    /// The approval that admits this pair, whether or not it exists yet.
    pub approval_id: String,
    /// Whether a publication of this manifest would pass the operator's gate
    /// now. A fact at the moment of reading: a withdrawal changes it.
    pub admitted: bool,
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

/// Keep a recording, and hand back the reference a declaration names it by.
///
/// The digest is of the bytes that arrived and never of anything a caller
/// claimed — the prompt registry's rule, for the same reason: a content address
/// somebody else supplied lets two different recordings occupy one key, and a
/// run would then measure answers it was never pointed at. Staging the same
/// bytes twice is the same reference.
pub(crate) async fn stage(store: &Store, name: &str, bytes: Vec<u8>) -> Result<ArtifactRef> {
    text(name, "recording.name")?;
    let held: RecordedAnswers =
        serde_json::from_slice(&bytes).map_err(|error| crate::EvaluationError::Invalid {
            field: "recording".into(),
            reason: format!("does not hold recorded answers: {error}"),
        })?;
    for (index, answer) in held.answers.iter().enumerate() {
        text(
            &answer.case_id,
            &format!("recording.answers[{index}].case_id"),
        )?;
    }
    let digest = store::hash(&bytes);
    let size = bytes.len() as u64;
    store.0.create(&store::recording(&digest), bytes).await?;
    Ok(ArtifactRef {
        name: name.to_owned(),
        // Resolvable only through this deployment's evaluation store, which is
        // what the digest beside it says: the bytes are the address.
        uri: format!("evaluation://recordings/{digest}"),
        digest,
        size_bytes: Some(size),
        content_type: "application/json".to_owned(),
        kind: ArtifactKind::Blob,
        schema_ref: None,
    })
}

/// The answers a declaration named, re-verified against the digest it named.
pub(crate) async fn recorded(store: &Store, answers: &ArtifactRef) -> Result<RecordedAnswers> {
    let bytes = store
        .0
        .get(&store::recording(&answers.digest))
        .await?
        .ok_or(crate::EvaluationError::Unavailable(
            crate::EvidenceState::MissingArtifact,
        ))?;
    if store::hash(&bytes) != answers.digest {
        return Err(crate::EvaluationError::Unavailable(
            crate::EvidenceState::CorruptArtifact,
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| crate::EvaluationError::Unavailable(crate::EvidenceState::CorruptArtifact))
}
