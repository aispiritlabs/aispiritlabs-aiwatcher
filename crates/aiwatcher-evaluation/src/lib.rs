//! Evaluation owns pinned variants and evidence, independently of telemetry.
//! The facade validates and freezes contracts. The registry owns durable
//! evidence, source authorization, immutable publication and erasure (ADR_0030).

mod approval;
mod assessment;
mod cohort;
mod comparison;
mod context;
mod experiment;
mod external;
mod gate;
mod judge;
mod manifest;
mod reference;
mod registry;
mod result;
mod review;
mod rubric;
mod scope;
mod scorecard;
mod scoring;
mod store;
mod traced;

pub use aiwatcher_core::Comparability;
pub use approval::{
    Approval, ApprovalBundles, ApprovalLine, ApprovalLinePage, ApprovalLineRecord, ApprovalPage,
    ApprovalRecord, PinnedMember, StagedFile, Withdrawal, approval_id, bundle_digest, line_id,
};
pub use assessment::{
    Assessment, AssessmentHistory, AssessmentPage, AssessmentRequest, AssessmentSource,
    AssessmentTarget, AssessmentTargetQuery, TargetKind, standing_id,
};
pub use cohort::{
    COHORT_CASES, COHORT_EXPECTATIONS_SCHEMA, COHORT_INPUT_SCHEMA, CohortFiles, CohortRequest,
    DerivedCohort,
};
pub use comparison::{
    CaseChange, CaseDiffPage, CaseFilter, CaseOutcome, DiffQuery, EvidenceCaseDelta,
    EvidenceComparison, EvidenceMetricDelta,
};
pub use experiment::{Experiment, ExperimentEntry, ExperimentIndex, ExperimentRow, RowComparison};
pub use gate::{
    GateCase, GateDecision, GateMetric, GatePolicy, GateSubject, GateVerdict,
    decide as gate_decision,
};
pub use registry::{
    CollectionReport, PUBLICATION_GRACE_SECONDS, Registry, RegistryConfig, SourceAuthority,
    SourceEvidence,
};
pub use result::*;
pub use review::{
    CaseProposal, ReviewAction, ReviewContent, ReviewItem, ReviewPage, ReviewState, TargetReviews,
};
pub use store::EvidenceCipher;
pub use traced::{
    CallElsewhere, GENERATION_TRACES, GenerationServed, GenerationTrace, RunsCounted, StepSeen,
    TracedAnswer, TracedCall, TracedRun, TracedTool, Witnesses, lost_or_unknown, trace_answers,
};

pub use context::{
    Aggregation, CalibrationPin, EvaluationContext, ExternalMeasure, JudgeConfiguration,
    MetricDefinition, MetricDirection,
};
pub use external::{
    CaseSide, CatalogAdapter, CatalogMetric, ExternalCalibration, ExternalCall, ExternalCase,
    ExternalDeclaration, ExternalReply, ExternalReport, ExternalScorers, KeptScore, Parameter,
    ParameterKind, RecordedCatalog, SCORER_CONTRACT, ScorerCatalog, ScorerFailure,
    external_agreement, resolve as resolve_external,
};
pub use judge::{
    AgreementInterval, CalibrationItem, CalibrationRequest, CalibrationSet, CalibrationVersion,
    JudgeAgreement, JudgeCall, JudgeDeclaration, JudgeFailure, JudgeMessage, JudgeModel,
    JudgeReply, JudgeReport, JudgeSettings, Served, ServedModel, agreement, ask, number, read,
    reply_schema, scored, served,
};
pub use manifest::{EvaluationManifest, EvaluationOrigin, PreparedEvaluation, VariantManifest};
pub use reference::{DatasetKind, DatasetReference, VersionReference};
pub use rubric::{AssessmentValue, Rubric, RubricHead, RubricPage, RubricVersion, Scale};
pub use scorecard::{
    Change as ScorecardChange, External, FieldChange, MetricChange, Rubrics, Score, Scorecard,
    ScorecardDiff, ScorecardHead, ScorecardPage, ScorecardVersion, ScorecardVersions, Scorer,
    ScorerSpec, diff as scorecard_diff,
};
pub use scoring::{
    Answers, ArchiveWord, Asked, Asking, COHORT_INPUTS, Calibrated, Cohort, CohortCases,
    DeclaredRun, ExternalAsking, ExternalQuestion, GENERATED_ANSWERS, GENERATED_WITH, Generated,
    GeneratedWith, Generation, JudgeQuestion, Judged, MAX_RUN_CONCURRENCY, MAX_RUN_TIMEOUT_SECONDS,
    MIN_RUN_TIMEOUT_SECONDS, RecordedAnswer, RecordedAnswers, RunSettings, SCORING_ENGINE,
    SCORING_VERSION, Scored, ScoringRun, ScoringRunView, Shown, StepOrigin, archived,
    external_questions, external_replies, questions, replies, score, score_spelled, score_with,
    scoring_engine, warnings,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u32 = 1;
/// Contract metadata, not result bytes. Large inputs belong in artifacts.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("evaluation ID already belongs to a different result")]
    Conflict,
    #[error("another revision of this assessment was written first")]
    Contested,
    /// Nothing admits this variant and context yet. Carries the approval that
    /// would, because that address is the one thing an operator needs next.
    #[error("no operator has admitted this pair yet: approval {0}")]
    NotAdmitted(String),
    /// The pair is admitted already, over other bundle bytes than the ones
    /// staged now. An approval is made once, so the way on is the bytes it
    /// admitted — not a second approval, and not the conflict of two results
    /// under one ID this used to be reported as.
    #[error(
        "approval {0} already admitted this pair over other bundle bytes; stage the bytes it \
         admitted"
    )]
    AdmittedOtherBytes(String),
    #[error("evaluation evidence is {0:?}")]
    Unavailable(EvidenceState),
    #[error("storage: {0}")]
    Storage(#[from] aiwatcher_core::ports::PortError),
    #[error("cannot encode evaluation metadata: {0}")]
    Encoding(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, EvaluationError>;

/// The public entry point. A prepared value can only be built after validation.
#[derive(Debug)]
pub struct Evaluation;

impl Evaluation {
    pub fn prepare(manifest: EvaluationManifest) -> Result<PreparedEvaluation> {
        manifest.validate()?;
        let bytes = canonical(&manifest)?;
        require(
            bytes.len() <= MAX_MANIFEST_BYTES,
            "manifest",
            "exceeds 256 KiB",
        )?;
        let variant_id = digest(&manifest.variant)?;
        let context_id = manifest.context.id()?;
        Ok(PreparedEvaluation::new(manifest, variant_id, context_id))
    }
}

pub(crate) fn require(ok: bool, field: &str, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EvaluationError::Invalid {
            field: field.into(),
            reason: reason.into(),
        })
    }
}

pub(crate) fn text(value: &str, field: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.trim() == value
            && value.len() <= 512
            && !value.chars().any(char::is_control),
        field,
        "must be trimmed nonblank text of at most 512 bytes without control characters",
    )
}

pub(crate) fn digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(hex::encode(Sha256::digest(canonical(value)?)))
}

// Sort every object explicitly: another workspace consumer may enable
// serde_json's preserve_order feature. SDKs use the server's returned IDs.
fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    fn sort(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.sort_keys();
                for child in map.values_mut() {
                    sort(child);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    sort(child);
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)?;
    sort(&mut value);
    Ok(serde_json::to_vec(&value)?)
}
