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
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

use crate::{
    CalibrationSet, CaseMeasurement, DatasetKind, DatasetReference, EvaluationContext,
    EvaluationManifest, EvaluationOrigin, JudgeCall, JudgeConfiguration, JudgeDeclaration, Result,
    ResultStatus, Rubrics, SCHEMA_VERSION, Scorecard, VariantManifest, VersionReference, digest,
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

/// Where a run's answers are read from.
///
/// On the wire a recording is the artifact reference it always was — so a
/// declaration written before the archive was a source keeps its content
/// address — and the archive is the one word `"archive"`.
///
/// Both names carry the scoring run they belong to in the contract: a gate's
/// answers and the conversation archive are other domains' words.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
#[schema(as = ScoringAnswers)]
#[serde(untagged)]
pub enum Answers {
    /// A staged recording, pinned by the digest of its bytes.
    Recording(ArtifactRef),
    /// What the conversation archive says was answered: each case of a
    /// conversation cohort is an assistant turn, and its answer is that turn's
    /// response. Read through the archive's owner under the approval that
    /// admitted the pair, sealed like every result from that source, and never
    /// staged anywhere in the clear.
    Archive(ArchiveWord),
    /// What the variant answers now: a worker task is handed each case's
    /// input and writes an answer per case, which the run then scores. The
    /// template of C1, and the one source whose answers did not exist when the
    /// run was declared — pinned instead by the attempt that produced them.
    Generated(Generated),
}

/// Answers a worker generates, on the wire as `{"generated_by": {…}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScoringGenerated)]
#[serde(deny_unknown_fields)]
pub struct Generated {
    pub generated_by: Generation,
}

/// The worker task that generates a run's answers.
///
/// Named like any registered workflow's step — a task a worker registered and
/// the queue it claims from — and never code: the application is the worker's,
/// and aiwatcher hands it inputs and reads what it wrote. What it runs is the
/// variant's to say, so the task is told the variant manifest it is measured
/// as, and nothing the cohort expected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScoringGeneration)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    /// `name@version`, as the worker registered it.
    pub task: String,
    /// The queue a worker claims it from; its token has to name it.
    pub queue: String,
    /// Handed to the task beside the variant. Part of the declaration, so two
    /// runs told different things are two runs.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    #[schema(value_type = Object)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// What the generation step writes: one row per case it answered.
pub const GENERATED_ANSWERS: &str = "answers";
/// What the generation step also writes: one row saying what it answered with.
pub const GENERATED_WITH: &str = "generated_with";

/// What a worker's task says it generated with: the digest of each artifact
/// of the variant's it holds as bytes, as the one row of `generated_with`.
///
/// The variant pins its code and generation config by content, and the task is
/// the only party holding either — a worker built from an older commit, or
/// handed another config, answers under the variant's name with something
/// else, and the numbers publish as the variant's. So the task reports the
/// digests of what it holds and the score step holds them to the pins before
/// it reads an answer. It is the worker's word, checked for agreement and not
/// proved: a task that echoed the pins would pass, and nothing here can tell.
/// The model, prompt and workflow are references a task resolves through a
/// registry, not bytes it holds, and are not reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedWith {
    pub code: String,
    pub generation_config: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
}

impl GeneratedWith {
    /// Every place what the task generated with is not what the variant pins,
    /// each naming both digests; empty when they agree.
    #[must_use]
    pub fn disagreements(&self, variant: &VariantManifest) -> Vec<String> {
        let pinned = |artifact: &Option<ArtifactRef>| artifact.as_ref().map(|a| a.digest.clone());
        [
            (
                "code",
                Some(variant.code.digest.clone()),
                Some(self.code.clone()),
            ),
            (
                "generation_config",
                Some(variant.generation_config.digest.clone()),
                Some(self.generation_config.clone()),
            ),
            (
                "response_schema",
                pinned(&variant.response_schema),
                self.response_schema.clone(),
            ),
            ("tools", pinned(&variant.tools), self.tools.clone()),
        ]
        .into_iter()
        .filter(|(_, pinned, held)| pinned != held)
        .map(|(field, pinned, held)| {
            let said = |digest: Option<String>| digest.unwrap_or_else(|| "nothing".to_owned());
            format!(
                "{field}: the variant pins {} and the task generated with {}",
                said(pinned),
                said(held)
            )
        })
        .collect()
    }
}
/// What the cases step writes for the generation step to read.
pub const COHORT_INPUTS: &str = "cases";

/// The one word that names the archive as a run's answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScoringArchive)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveWord {
    Archive,
}

impl<'de> Deserialize<'de> for Answers {
    /// By shape rather than by trying each variant in turn: an untagged
    /// derive reports a malformed recording as "matched no variant", and the
    /// artifact's own refusal is the one that says what is wrong with it.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(word) if word == "archive" => {
                Ok(Self::Archive(ArchiveWord::Archive))
            }
            serde_json::Value::String(word) => Err(D::Error::custom(format!(
                "answers: `{word}` names no source; a recording is an artifact reference, \
                 the archive is \"archive\", and generated answers are {{\"generated_by\": …}}"
            ))),
            serde_json::Value::Object(fields) if fields.contains_key("generated_by") => {
                Generated::deserialize(serde_json::Value::Object(fields))
                    .map(Self::Generated)
                    .map_err(D::Error::custom)
            }
            value => ArtifactRef::deserialize(value)
                .map(Self::Recording)
                .map_err(D::Error::custom),
        }
    }
}

impl Answers {
    /// The staged recording, when that is where the answers are.
    #[must_use]
    pub const fn recording(&self) -> Option<&ArtifactRef> {
        match self {
            Self::Recording(artifact) => Some(artifact),
            Self::Archive(_) | Self::Generated(_) => None,
        }
    }

    /// The worker task, when a worker generates the answers.
    #[must_use]
    pub const fn generation(&self) -> Option<&Generation> {
        match self {
            Self::Generated(generated) => Some(&generated.generated_by),
            Self::Recording(_) | Self::Archive(_) => None,
        }
    }
}

/// The archive's own responses, as the answers to the cases they are.
///
/// A conversation cohort's expectation for a case *is* the reviewed response
/// of that assistant turn, so the answers are read from the same place — which
/// is also why a card over them may read no expectation.
#[must_use]
pub fn archived(cohort: &BTreeMap<String, serde_json::Value>) -> Vec<RecordedAnswer> {
    cohort
        .iter()
        .map(|(case_id, response)| RecordedAnswer {
            case_id: case_id.clone(),
            answer: response.clone(),
            trace_id: None,
            span_id: None,
        })
        .collect()
}

/// The longest a scoring step may be told it has, in seconds: a day.
pub const MAX_RUN_TIMEOUT_SECONDS: u64 = 86_400;
/// The shortest, in seconds. Below a minute a judge's first reply is a race.
pub const MIN_RUN_TIMEOUT_SECONDS: u64 = 60;
/// The most questions one run may put to a judge at once, whatever a
/// deployment allows. A number past it is a mistake before it is a setting.
pub const MAX_RUN_CONCURRENCY: u32 = 64;

/// How a run is carried out, as opposed to what it measures.
///
/// Neither setting reaches the manifest: a result measured in ten minutes and
/// the same one measured in an hour are one measurement, and two contexts for
/// them would make the second incomparable with the first for no reason. They
/// are part of the declaration, because a declaration is the run and a retry
/// must read the deadline the first attempt had.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = ScoringRunSettings)]
#[serde(deny_unknown_fields)]
pub struct RunSettings {
    /// How long the step may run before it is stopped, in seconds. Absent is
    /// this deployment's default for the kind of run: fifteen minutes for a
    /// fold, an hour for one that asks a judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 60, maximum = 86400)]
    pub timeout_seconds: Option<u64>,
    /// How many questions are put to a judge at once. Absent is the
    /// deployment's `AIWATCHER_JUDGE_CONCURRENCY`, which is also the most a
    /// run may ask for: a provider's rate limit is the operator's to know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1, maximum = 64)]
    pub concurrency: Option<u32>,
}

impl RunSettings {
    /// Whether nothing was chosen, so the declaration's address stays the one
    /// it had before settings existed.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    fn validate(&self) -> Result<()> {
        if let Some(seconds) = self.timeout_seconds {
            require(
                (MIN_RUN_TIMEOUT_SECONDS..=MAX_RUN_TIMEOUT_SECONDS).contains(&seconds),
                "run.settings.timeout_seconds",
                "is between 60 seconds and a day",
            )?;
        }
        if let Some(count) = self.concurrency {
            require(
                (1..=MAX_RUN_CONCURRENCY).contains(&count),
                "run.settings.concurrency",
                "is between 1 and 64 questions at once",
            )?;
        }
        Ok(())
    }
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
    /// The recording, or the archive. Pinned either way — a recording by the
    /// digest of its bytes and the archive by the corpus version the variant
    /// names — so the answers cannot change under a retry, which is what makes
    /// re-running one cheap and honest.
    pub answers: Answers,
    /// The judge, when the card asks one. Absent otherwise, and absent from
    /// the content address then, so a declaration from before judges keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeDeclaration>,
    /// The calibration set the card's calibrated framework metrics are held
    /// against, when it calibrates any — taken under the rubrics the card
    /// names, and possibly the set a judge names too. Absent otherwise, and
    /// absent from the content address then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_calibration: Option<VersionReference>,
    /// The deadline and the pace. Absent when nothing was chosen, and absent
    /// from the content address then.
    #[serde(default, skip_serializing_if = "RunSettings::is_default")]
    pub settings: RunSettings,
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
        self.settings.validate()?;
        if let Some(judge) = &self.judge {
            judge.validate()?;
        }
        if let Some(calibration) = &self.external_calibration {
            calibration.validate("run.external_calibration")?;
        }

        let conversations = self.variant.dataset.kind == DatasetKind::Conversations;
        match &self.answers {
            Answers::Recording(recording) => {
                artifact(recording, "run.answers")?;
                // Answers to questions the archive holds, kept in a staged
                // object with no seal, no retention clock and no erasure.
                require(
                    !conversations,
                    "run.answers",
                    "a conversation cohort's answers are read from the archive, never staged in \
                     the clear; declare \"archive\"",
                )
            }
            Answers::Archive(_) => require(
                conversations,
                "run.answers",
                "only a conversation cohort has answers in the archive",
            ),
            Answers::Generated(generated) => {
                let generation = &generated.generated_by;
                text(&generation.task, "run.answers.generated_by.task")?;
                require(
                    generation
                        .task
                        .split_once('@')
                        .is_some_and(|(name, version)| !name.is_empty() && !version.is_empty()),
                    "run.answers.generated_by.task",
                    "names a task as `name@version`, the way a worker registered it",
                )?;
                text(&generation.queue, "run.answers.generated_by.queue")?;
                // Each case's question would reach a worker as a row in the
                // artifact store, outside the archive's seal, retention and
                // erasure — the recording's refusal, arriving from the other side.
                require(
                    !conversations,
                    "run.answers",
                    "a conversation cohort's questions are the archive's words, and a worker \
                     generating answers would be handed them outside it; score the archive's \
                     own responses instead",
                )
            }
        }
    }

    /// Whether this card can measure these answers at all.
    ///
    /// The archive's answer to a case is the response its expectation was
    /// read from, so a scorer comparing the two measures a response against
    /// itself and would report perfect agreement about nothing. A judge is
    /// declared exactly when the card asks one. Over the archive it is sent the
    /// archive's words, which is allowed and never quiet: the context says
    /// `reads_archive`, and [`warnings`] says it to whoever declares, admits or
    /// reads the run.
    pub fn check(&self, card: &Scorecard) -> Result<()> {
        require(
            card.name == self.scorecard.name,
            "run.scorecard.name",
            "names a different card from the one resolved",
        )?;
        require(
            self.settings.concurrency.is_none()
                || self.judge.is_some()
                || card.asks_a_scorer_service(),
            "run.settings.concurrency",
            "is how many questions a judge or a scorer service is asked at once, and this run \
             asks neither",
        )?;
        let asks_a_judge = !card.judges().is_empty();
        require(
            asks_a_judge == self.judge.is_some(),
            "run.judge",
            if asks_a_judge {
                "the card asks a judge, so the run declares which one and its calibration"
            } else {
                "the card asks no judge, so the run declares none"
            },
        )?;
        let calibrates = !card.external_calibrations().is_empty();
        require(
            calibrates == self.external_calibration.is_some(),
            "run.external_calibration",
            if calibrates {
                "the card holds a framework metric against people, so the run names the \
                 calibration set whose judgements it is held against"
            } else {
                "the card holds no framework metric against people, so the run names no \
                 calibration set for one"
            },
        )?;
        let archive = matches!(self.answers, Answers::Archive(_));
        if archive
            && let Some(spec) = card
                .scorers
                .iter()
                .find(|spec| spec.compares_with_expected())
        {
            return Err(crate::EvaluationError::Invalid {
                field: "run.scorecard".into(),
                reason: format!(
                    "`{}` compares an answer with its expectation, and the archive's answer is \
                     the response its expectation is; score it with scorers that read no \
                     expectation",
                    spec.metric
                ),
            });
        }
        Ok(())
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
    ///
    /// `calibration` is the set the judge names, when it names one: whether the
    /// judge is sent the archive's words depends on where that set was taken
    /// from as well as on this cohort. `external_calibration` is the set the
    /// run names for its framework metrics, for the same reason.
    pub fn manifest(
        &self,
        card: &Scorecard,
        rubrics: &Rubrics,
        calibration: Option<&CalibrationSet>,
        external_calibration: Option<&CalibrationSet>,
        ran_by: Option<&StepOrigin>,
    ) -> Result<EvaluationManifest> {
        self.check(card)?;
        require(
            self.judge.is_none() || calibration.is_some(),
            "run.judge.calibration",
            "is needed to say whether the judge is sent the conversation archive's words",
        )?;
        require(
            self.external_calibration.is_none() || external_calibration.is_some(),
            "run.external_calibration",
            "is needed to say whether the scorer service is sent the conversation archive's words",
        )?;
        let external_calibration = match (&self.external_calibration, external_calibration) {
            (Some(named), Some(set)) => Some(crate::CalibrationPin {
                calibration_dataset: DatasetReference {
                    kind: DatasetKind::Assessments,
                    name: named.name.clone(),
                    version: named.version.clone(),
                },
                reads_archive: set.from_archive,
            }),
            _ => None,
        };
        let reads_archive = self.variant.dataset.kind == DatasetKind::Conversations
            || calibration.is_some_and(|set| set.from_archive);
        let judge = match &self.judge {
            Some(judge) => Some(JudgeConfiguration {
                reads_archive,
                provider: judge.provider.clone(),
                model: judge.model.clone(),
                configuration: judge.settings.artifact()?.0,
                calibration_dataset: DatasetReference {
                    kind: DatasetKind::Assessments,
                    name: judge.calibration.name.clone(),
                    version: judge.calibration.version.clone(),
                },
            }),
            None => None,
        };
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
                judge,
                external_calibration,
                metrics: card.metrics(rubrics)?,
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
    /// What whoever declares, admits or starts this run should be told first,
    /// in words. Empty when there is nothing to say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Where the cohort's three files came from, when this deployment derived
    /// them from a dataset version it owns. An operator admitting the pair
    /// then brings only what the variant pins: the adapter derives the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohort: Option<crate::DerivedCohort>,
}

/// What a run's manifest commits people to that they should hear before it
/// runs. Written once, here, so the panel and an API caller read one sentence
/// rather than each composing their own from the fields.
#[must_use]
pub fn warnings(
    manifest: &EvaluationManifest,
    card: &Scorecard,
    calibration: Option<&CalibrationSet>,
) -> Vec<String> {
    let mut said = judge_warnings(manifest, card, calibration);
    said.extend(external_warnings(manifest, card));
    said
}

/// What a run that asks a scorer service commits people to.
///
/// Two things, said separately because either can be true without the other.
/// A metric a model graded is a model's word whose agreement with people
/// nobody measured — a rubric judge is calibrated, and a framework's metric is
/// not. And over a conversation cohort, the scorer service is sent the
/// archive's words, and a graded metric sends them on to its model's provider.
fn external_warnings(manifest: &EvaluationManifest, card: &Scorecard) -> Vec<String> {
    let mut said = Vec::new();
    let measured: Vec<&crate::ExternalMeasure> = manifest
        .context
        .metrics
        .iter()
        .filter_map(|metric| metric.measured_by.as_ref())
        .collect();
    for metric in &manifest.context.metrics {
        if let Some(measure) = &metric.measured_by
            && let Some(model) = &measure.model
        {
            let calibrated = card
                .scorers
                .iter()
                .find(|spec| spec.metric == metric.name)
                .and_then(|spec| spec.scorer.external()?.calibration);
            said.push(match (calibrated, &manifest.context.external_calibration) {
                (Some(calibrated), Some(pin)) => format!(
                    "`{}` is graded by {} {} through {} {}: a model's word, which re-reading will \
                     not reproduce. How often its verdicts are people's under {} (version {}) is \
                     measured on {} and published beside it.",
                    metric.name,
                    model.name,
                    model.version,
                    measure.adapter.name,
                    measure.adapter.version,
                    calibrated.rubric.name,
                    calibrated
                        .rubric
                        .version
                        .get(..12)
                        .unwrap_or(&calibrated.rubric.version),
                    pin.calibration_dataset.name
                ),
                _ => format!(
                    "`{}` is graded by {} {} through {} {}: a model's word, which re-reading will \
                     not reproduce and whose agreement with people nothing here measured.",
                    metric.name,
                    model.name,
                    model.version,
                    measure.adapter.name,
                    measure.adapter.version
                ),
            });
        }
    }
    if let Some(pin) = manifest
        .context
        .external_calibration
        .as_ref()
        .filter(|pin| pin.reads_archive)
    {
        said.push(format!(
            "This run sends the scorer service the conversation answers people judged in its \
             calibration set {}, and its graded metrics send them on to their model's provider. \
             What leaves the archive is outside its encryption, retention and erasure, and \
             nothing here can take it back.",
            pin.calibration_dataset.name
        ));
    }
    if manifest.context.dataset.kind == DatasetKind::Conversations && !measured.is_empty() {
        let mut sent = vec!["each assistant response this run scores"];
        if card
            .scorers
            .iter()
            .any(|spec| spec.scorer.external().is_some() && spec.input_path.is_some())
        {
            sent.push("what was said to the assistant before each one");
        }
        let graded = measured.iter().filter_map(|measure| measure.model.as_ref());
        let providers: std::collections::BTreeSet<String> = graded
            .map(|model| format!("{} {}", model.name, model.version))
            .collect();
        let adapters: std::collections::BTreeSet<&str> = measured
            .iter()
            .map(|measure| measure.adapter.name.as_str())
            .collect();
        said.push(format!(
            "This run sends words from the conversation archive to the scorer service ({}): {}.{} \
             What leaves the archive is outside its encryption, retention and erasure, and \
             nothing here can take it back.",
            adapters.into_iter().collect::<Vec<_>>().join(", "),
            sent.join("; "),
            if providers.is_empty() {
                String::new()
            } else {
                format!(
                    " Its graded metrics send them on to the provider of {}.",
                    providers.into_iter().collect::<Vec<_>>().join(", ")
                )
            }
        ));
    }
    said
}

fn judge_warnings(
    manifest: &EvaluationManifest,
    card: &Scorecard,
    calibration: Option<&CalibrationSet>,
) -> Vec<String> {
    let Some(judge) = manifest
        .context
        .judge
        .as_ref()
        .filter(|judge| judge.reads_archive)
    else {
        return Vec::new();
    };
    let mut sent = Vec::new();
    if manifest.context.dataset.kind == DatasetKind::Conversations {
        sent.push("each assistant response this run scores");
        if card
            .scorers
            .iter()
            .any(|spec| spec.scorer.rubric().is_some() && spec.input_path.is_some())
        {
            sent.push("what was said to the assistant before each one");
        }
    }
    if calibration.is_some_and(|set| set.from_archive) {
        sent.push("the conversation answers people judged in its calibration set");
    }
    vec![format!(
        "This run's judge ({} · {}) is sent words from the conversation archive: {}. The \
         provider keeps what it is sent outside the archive's encryption, retention and \
         erasure, and nothing here can take it back.",
        judge.provider,
        judge.model.name,
        sent.join("; ")
    )]
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
    score_with(card, cohort, answers, repetition_id, &Judged::new())
}

/// What a judge said about each case it was asked about, keyed by the case
/// and the metric, as the number its reply scored or why it scored none.
pub type Judged = BTreeMap<(String, String), Score>;

/// The same fold, with a judge's answers handed in.
///
/// The model was asked before this runs, so the fold still reads no clock and
/// opens no socket; a judge scorer reads the answer the map holds for its case,
/// and a case nobody asked about is unscored like any other unreadable one.
#[must_use]
pub fn score_with(
    card: &Scorecard,
    cohort: &BTreeMap<String, serde_json::Value>,
    answers: &[RecordedAnswer],
    repetition_id: &str,
    judged: &Judged,
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
            [answer] => measure(card, answer, expected, judged),
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
    judged: &Judged,
) -> std::result::Result<BTreeMap<String, f64>, String> {
    let mut metrics = BTreeMap::new();
    for spec in &card.scorers {
        let asked_elsewhere = spec.scorer.rubric().is_some() || spec.scorer.external().is_some();
        let score = match (asked_elsewhere, spec.sides(&answer.answer, expected)) {
            (_, Err(reason)) => Score::Unscored(reason),
            // A judge's reply or a scorer service's, handed in: the fold opens
            // no socket for either.
            (true, Ok(_)) => judged
                .get(&(answer.case_id.clone(), spec.metric.clone()))
                .cloned()
                .unwrap_or_else(|| spec.scorer.score(&answer.answer, expected)),
            (false, Ok(_)) => spec.measure(&answer.answer, expected),
        };
        match score {
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

/// What a question to the judge is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// A case this run measures.
    Case(String),
    /// An item of the calibration set, by its position.
    Calibration(usize),
}

/// One question a run puts to its judge.
#[derive(Clone, Debug, PartialEq)]
pub struct JudgeQuestion {
    pub about: Asked,
    pub metric: String,
    pub call: JudgeCall,
}

/// What a cohort selects, as its owner resolved it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CohortCases {
    /// What each selected case expected — the cohort, for the fold.
    pub expected: BTreeMap<String, serde_json::Value>,
    /// What each case was asked, where the owner keeps it; for a judge only.
    pub inputs: BTreeMap<String, serde_json::Value>,
}

/// What a person was shown about one case of a calibration result.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub answer: serde_json::Value,
    pub expected: serde_json::Value,
    /// Resolved only when a judge is to be shown it.
    pub input: Option<serde_json::Value>,
}

/// A calibration result's cases, keyed by case and repetition.
pub type Calibrated = BTreeMap<(String, String), Shown>;

/// The questions a run puts to its judge, and the cases it could not ask
/// about, with why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asking {
    pub questions: Vec<JudgeQuestion>,
    /// A case whose input the card points at and the cohort does not hold.
    /// Its reason reaches the fold, where it fails the case like any other
    /// unreadable one rather than as a judge nobody asked.
    pub refused: Judged,
}

/// Every question a run's judge is asked.
///
/// One per judge metric for each case the fold will score — a case answered
/// twice is not scored, so it is not asked about either — and one per
/// calibration item under that metric's rubric, put the answer the person was
/// shown. A case whose answer has nothing where the card points is not asked
/// about: the fold says why, as it does for every other scorer. Nor is a case
/// whose input the card shows and the cohort does not hold, and that reason is
/// handed back; a calibration item in the same position is not asked either,
/// and counts against the judge's agreement like every item it did not answer.
#[must_use]
pub fn questions(
    run: &ScoringRun,
    card: &Scorecard,
    rubrics: &Rubrics,
    cohort: &CohortCases,
    answers: &[RecordedAnswer],
    calibration: &CalibrationSet,
    calibrated: &Calibrated,
) -> Asking {
    let mut asking = Asking::default();
    let Some(judge) = &run.judge else {
        return asking;
    };
    let mut once: BTreeMap<&str, Vec<&RecordedAnswer>> = BTreeMap::new();
    for answer in answers {
        if cohort.expected.contains_key(&answer.case_id) {
            once.entry(&answer.case_id).or_default().push(answer);
        }
    }
    for spec in &card.scorers {
        let Some(pinned) = spec.scorer.rubric() else {
            continue;
        };
        let Some(rubric) = rubrics.get(pinned) else {
            continue;
        };
        let mut put = |about: Asked,
                       answer: &serde_json::Value,
                       expected: &serde_json::Value,
                       input: Option<&serde_json::Value>| {
            let Ok((answer, expected)) = spec.sides(answer, expected) else {
                return;
            };
            let input = match spec.shown_input(input) {
                Ok(input) => input,
                Err(reason) => {
                    if let Asked::Case(case_id) = about {
                        asking
                            .refused
                            .insert((case_id, spec.metric.clone()), Score::Unscored(reason));
                    }
                    return;
                }
            };
            let expected = spec.compares_with_expected().then_some(expected);
            asking.questions.push(JudgeQuestion {
                about,
                metric: spec.metric.clone(),
                call: crate::ask(rubric, judge, input, answer, expected),
            });
        };
        for (case_id, answered) in &once {
            if let [answer] = answered.as_slice() {
                put(
                    Asked::Case((*case_id).to_owned()),
                    &answer.answer,
                    &cohort.expected[*case_id],
                    cohort.inputs.get(*case_id),
                );
            }
        }
        for (index, item) in calibration.items.iter().enumerate() {
            if item.rubric != *pinned {
                continue;
            }
            if let Some(shown) = calibrated.get(&(item.case_id.clone(), item.repetition_id.clone()))
            {
                put(
                    Asked::Calibration(index),
                    &shown.answer,
                    &shown.expected,
                    shown.input.as_ref(),
                );
            }
        }
    }
    asking
}

/// What the judge's replies score, for the fold and for the agreement.
///
/// `replies` is in the order of the questions asked. A reply that is not a
/// value on the rubric's scale scores nothing: the case is a failure with the
/// reason, and a calibration item it answered that way counts against the
/// judge. A case that could not be asked about carries its own reason.
#[must_use]
pub fn replies(
    run: &ScoringRun,
    card: &Scorecard,
    rubrics: &Rubrics,
    calibration: &CalibrationSet,
    asking: &Asking,
    replies: &[crate::JudgeReply],
) -> (Judged, Option<crate::JudgeReport>) {
    let Some(judge) = &run.judge else {
        return (Judged::new(), None);
    };
    let mut judged = asking.refused.clone();
    let mut said = BTreeMap::new();
    for (question, reply) in asking.questions.iter().zip(replies) {
        let spec = card
            .scorers
            .iter()
            .find(|spec| spec.metric == question.metric);
        let rubric = spec
            .and_then(|spec| spec.scorer.rubric())
            .and_then(|pinned| rubrics.get(pinned));
        let pass_level = spec.and_then(|spec| spec.scorer.pass_level());
        let read = rubric.map_or_else(
            || Err("this metric's rubric was not resolved".to_owned()),
            |rubric| {
                crate::read(rubric, reply).and_then(|(value, _)| {
                    crate::scored(rubric, pass_level, &value)
                        .map(|number| (value, number))
                        .ok_or_else(|| "the judge answered outside this rubric's scale".to_owned())
                })
            },
        );
        match &question.about {
            Asked::Case(case_id) => {
                judged.insert(
                    (case_id.clone(), question.metric.clone()),
                    match read {
                        Ok((_, number)) => Score::Measured(number),
                        Err(reason) => Score::Unscored(reason),
                    },
                );
            }
            Asked::Calibration(index) => {
                said.insert(
                    (*index, question.metric.clone()),
                    read.ok().map(|(_, number)| number),
                );
            }
        }
    }
    let mut report = crate::agreement(card, rubrics, &judge.calibration, calibration, &said);
    report.served = crate::served(replies);
    (judged, Some(report))
}

/// One case a run puts to a scorer service.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalQuestion {
    pub about: Asked,
    pub metric: String,
    pub call: crate::ExternalCall,
}

/// The questions a run puts to its scorer service, and the cases it could not
/// ask about, with why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalAsking {
    pub questions: Vec<ExternalQuestion>,
    pub refused: Judged,
}

/// Every question a run's external metrics are asked.
///
/// One per external metric for each case the fold will score, as the judge's
/// questions are: a case answered twice is not asked about, a case whose
/// answer has nothing where the card points is left to the fold to explain,
/// and a case the metric cannot be asked about — it reads the question and
/// the card shows none — carries its reason back. A metric the card has not
/// pinned a declaration for is asked nothing, and the fold says so.
///
/// A calibrated metric is also asked about every item of `calibration` under
/// its rubric, put the answer the person was shown; an item it cannot be
/// asked about is not asked, and counts against its agreement.
#[must_use]
pub fn external_questions(
    card: &Scorecard,
    cohort: &CohortCases,
    answers: &[RecordedAnswer],
    calibration: Option<(&CalibrationSet, &Calibrated)>,
) -> ExternalAsking {
    let mut asking = ExternalAsking::default();
    let mut once: BTreeMap<&str, Vec<&RecordedAnswer>> = BTreeMap::new();
    for answer in answers {
        if cohort.expected.contains_key(&answer.case_id) {
            once.entry(&answer.case_id).or_default().push(answer);
        }
    }
    for spec in &card.scorers {
        let Some(external) = spec.scorer.external() else {
            continue;
        };
        let Some(declared) = external.declared else {
            continue;
        };
        let mut put = |about: Asked,
                       input: Option<&serde_json::Value>,
                       answer: &serde_json::Value,
                       expected: &serde_json::Value| {
            if spec.sides(answer, expected).is_err() {
                return;
            }
            match spec.external_case(declared, input, answer, expected) {
                Ok(case) => asking.questions.push(ExternalQuestion {
                    about,
                    metric: spec.metric.clone(),
                    call: crate::ExternalCall {
                        adapter: external.adapter.to_owned(),
                        metric: external.metric.to_owned(),
                        declared: declared.clone(),
                        parameters: external.parameters.clone(),
                        case,
                    },
                }),
                Err(reason) => {
                    if let Asked::Case(case_id) = about {
                        asking
                            .refused
                            .insert((case_id, spec.metric.clone()), Score::Unscored(reason));
                    }
                }
            }
        };
        for (case_id, answered) in &once {
            let [answer] = answered.as_slice() else {
                continue;
            };
            put(
                Asked::Case((*case_id).to_owned()),
                cohort.inputs.get(*case_id),
                &answer.answer,
                &cohort.expected[*case_id],
            );
        }
        let (Some((set, calibrated)), Some(calibrating)) = (calibration, external.calibration)
        else {
            continue;
        };
        for (index, item) in set.items.iter().enumerate() {
            if item.rubric != calibrating.rubric {
                continue;
            }
            if let Some(shown) = calibrated.get(&(item.case_id.clone(), item.repetition_id.clone()))
            {
                put(
                    Asked::Calibration(index),
                    shown.input.as_ref(),
                    &shown.answer,
                    &shown.expected,
                );
            }
        }
    }
    asking
}

/// What a scorer service's replies score, for the fold and for the agreement.
///
/// `replies` is in the order of the questions asked, and each is held to what
/// the card pinned about its metric: a number outside the declared range, or a
/// rate that is neither nought nor one, scores nothing — and about a
/// calibration item, counts against the metric's agreement.
#[must_use]
pub fn external_replies(
    run: &ScoringRun,
    card: &Scorecard,
    rubrics: &Rubrics,
    calibration: Option<&CalibrationSet>,
    asking: &ExternalAsking,
    replies: &[crate::ExternalReply],
) -> (Judged, Option<crate::ExternalReport>) {
    let mut judged = asking.refused.clone();
    let mut said = BTreeMap::new();
    for (question, reply) in asking.questions.iter().zip(replies) {
        let score = reply.score(&question.call.declared);
        match &question.about {
            Asked::Case(case_id) => {
                judged.insert((case_id.clone(), question.metric.clone()), score);
            }
            Asked::Calibration(index) => {
                said.insert(
                    (*index, question.metric.clone()),
                    match score {
                        Score::Measured(value) => Some(value),
                        Score::Unscored(_) => None,
                    },
                );
            }
        }
    }
    let report = match (&run.external_calibration, calibration) {
        (Some(named), Some(set)) => {
            Some(crate::external_agreement(card, rubrics, named, set, &said))
        }
        _ => None,
    };
    (judged, report)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Scorer, ScorerSpec};
    use serde_json::json;

    fn run(kind: DatasetKind, answers: Answers) -> ScoringRun {
        let manifest: EvaluationManifest = serde_json::from_str(include_str!(
            "../../../contracts/fixtures/evaluation-v1/manifest.json"
        ))
        .expect("the contract fixture parses");
        let mut variant = manifest.variant;
        variant.dataset.kind = kind;
        ScoringRun {
            evaluation_id: "scored".into(),
            repetition_id: "measurement-1".into(),
            variant,
            cohort: Cohort {
                case_manifest: manifest.context.case_manifest.clone(),
                case_count: 2,
                split: "test".into(),
                input_schema: manifest.context.input_schema,
                expectations_schema: manifest.context.expectations_schema,
            },
            scorecard: VersionReference {
                name: "answer-quality".into(),
                version: "a".repeat(64),
            },
            answers,
            judge: None,
            external_calibration: None,
            settings: RunSettings::default(),
        }
    }

    fn card(scorer: Scorer) -> Scorecard {
        Scorecard {
            name: "answer-quality".into(),
            description: String::new(),
            scorers: vec![ScorerSpec {
                metric: "measured".into(),
                answer_path: "/answer".into(),
                expected_path: String::new(),
                input_path: None,
                scorer,
            }],
        }
    }

    fn recording() -> ArtifactRef {
        ArtifactRef {
            name: "answers.json".into(),
            uri: format!("evaluation://recordings/{}", "b".repeat(64)),
            digest: "b".repeat(64),
            size_bytes: Some(10),
            content_type: "application/json".into(),
            kind: ArtifactKind::Blob,
            schema_ref: None,
        }
    }

    #[test]
    fn a_recording_is_written_as_the_reference_it_always_was_so_its_address_does_not_move() {
        let declared = run(DatasetKind::Curation, Answers::Recording(recording()));
        let wire = serde_json::to_value(&declared).expect("a declaration encodes");
        assert_eq!(
            wire["answers"],
            serde_json::to_value(recording()).expect("a reference encodes"),
            "a declaration from before the archive was a source keeps its content address"
        );
        let read: ScoringRun = serde_json::from_value(wire).expect("and reads back");
        assert_eq!(read.id().unwrap(), declared.id().unwrap());

        let archive: Answers = serde_json::from_value(json!("archive")).expect("one word");
        assert_eq!(archive, Answers::Archive(ArchiveWord::Archive));
        let refused = serde_json::from_value::<Answers>(json!("elsewhere")).unwrap_err();
        assert!(refused.to_string().contains("elsewhere"), "{refused}");
        let malformed = serde_json::from_value::<Answers>(json!({"name": "x"})).unwrap_err();
        assert!(
            malformed.to_string().contains("missing field"),
            "the artifact's own refusal, not \"matched no variant\": {malformed}"
        );
    }

    #[test]
    fn generated_answers_name_a_worker_task_and_are_refused_over_the_archive() {
        let generated = |task: &str| {
            Answers::Generated(Generated {
                generated_by: Generation {
                    task: task.into(),
                    queue: "evaluation".into(),
                    params: serde_json::Map::new(),
                },
            })
        };
        let declared = run(DatasetKind::Curation, generated("support-bot.answer@3"));
        assert!(declared.validate().is_ok());
        let wire = serde_json::to_value(&declared).unwrap();
        assert_eq!(
            wire["answers"],
            json!({"generated_by": {"task": "support-bot.answer@3", "queue": "evaluation"}})
        );
        let read: ScoringRun = serde_json::from_value(wire).expect("and reads back");
        assert_eq!(read, declared);

        let unpinned = run(DatasetKind::Curation, generated("support-bot.answer"))
            .validate()
            .unwrap_err()
            .to_string();
        assert!(unpinned.contains("name@version"), "{unpinned}");
        let archive = run(
            DatasetKind::Conversations,
            generated("support-bot.answer@3"),
        )
        .validate()
        .unwrap_err()
        .to_string();
        assert!(archive.contains("archive's words"), "{archive}");
        let stray = serde_json::from_value::<Answers>(
            json!({"generated_by": {"task": "a@1", "queue": "q", "code": "print(1)"}}),
        )
        .unwrap_err();
        assert!(stray.to_string().contains("unknown field"), "{stray}");
    }

    #[test]
    fn what_a_task_generated_with_is_held_to_every_artifact_the_variant_pins() {
        let declared = run(
            DatasetKind::Curation,
            Answers::Archive(ArchiveWord::Archive),
        );
        let variant = declared.variant;
        let agreeing = GeneratedWith {
            code: variant.code.digest.clone(),
            generation_config: variant.generation_config.digest.clone(),
            response_schema: None,
            tools: None,
        };
        assert!(agreeing.disagreements(&variant).is_empty());

        let older = GeneratedWith {
            code: "0".repeat(64),
            tools: Some("1".repeat(64)),
            ..agreeing
        };
        let said = older.disagreements(&variant);
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(
            said[0].starts_with("code: the variant pins ") && said[0].ends_with(&"0".repeat(64)),
            "{said:?}"
        );
        assert_eq!(
            said[1],
            format!(
                "tools: the variant pins nothing and the task generated with {}",
                "1".repeat(64)
            )
        );
        let stray = serde_json::from_value::<GeneratedWith>(
            json!({"code": "a", "generation_config": "b", "model": "c"}),
        )
        .unwrap_err();
        assert!(stray.to_string().contains("unknown field"), "{stray}");
    }

    fn external(reads: Vec<crate::CaseSide>, graded: bool) -> Scorer {
        Scorer::External {
            adapter: "deepeval".into(),
            metric: "answer_relevancy".into(),
            parameters: serde_json::Map::new(),
            calibration: None,
            declared: Some(crate::ExternalDeclaration {
                version: "4.2.2".into(),
                model: graded.then(|| VersionReference {
                    name: "gemma-4-e2b".into(),
                    version: "q4".into(),
                }),
                unit: "score".into(),
                direction: crate::MetricDirection::Higher,
                aggregation: crate::Aggregation::Mean,
                reads,
                range: Some([0.0, 1.0]),
            }),
        }
    }

    #[test]
    fn a_framework_metric_over_the_archive_is_told_to_everybody_and_one_reading_the_expectation_is_refused()
     {
        use crate::CaseSide::{Answer, Expected, Input};
        let archived = run(
            DatasetKind::Conversations,
            Answers::Archive(ArchiveWord::Archive),
        );
        let mut graded = card(external(vec![Input, Answer], true));
        graded.scorers[0].input_path = Some("/question".into());
        let manifest = archived
            .manifest(&graded, &Rubrics::default(), None, None, None)
            .expect("a graded metric that reads the question and the answer is declared");
        assert!(
            manifest.context.metrics[0]
                .measured_by
                .as_ref()
                .is_some_and(|measure| measure.model.is_some())
        );
        let said = warnings(&manifest, &graded, None);
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].contains("`measured` is graded by gemma-4-e2b q4 through deepeval 4.2.2"));
        assert!(
            said[1].contains("to the scorer service (deepeval)")
                && said[1].contains("what was said to the assistant")
                && said[1].contains("provider of gemma-4-e2b q4"),
            "{}",
            said[1]
        );

        // A heuristic over a curation cohort sends nothing anywhere worth a word.
        let plain = card(external(vec![Answer], false));
        let quiet = run(DatasetKind::Curation, Answers::Recording(recording()))
            .manifest(&plain, &Rubrics::default(), None, None, None)
            .unwrap();
        assert!(warnings(&quiet, &plain, None).is_empty());

        // Over the archive, the expectation is the answer being measured.
        let comparing = card(external(vec![Answer, Expected], false));
        let refused = archived.check(&comparing).unwrap_err().to_string();
        assert!(refused.contains("`measured`"), "{refused}");
    }

    #[test]
    fn settings_nobody_chose_leave_a_declarations_address_where_it_was() {
        let declared = run(DatasetKind::Curation, Answers::Recording(recording()));
        let wire = serde_json::to_value(&declared).expect("a declaration encodes");
        assert!(wire.get("settings").is_none(), "{wire}");

        let mut paced = declared.clone();
        paced.settings.timeout_seconds = Some(600);
        assert!(paced.validate().is_ok());
        assert_ne!(
            paced.id().unwrap(),
            declared.id().unwrap(),
            "a run with its own deadline is its own declaration: a retry reads that deadline"
        );
        // And the same measurement: nothing about the manifest moves.
        let card = card(Scorer::ExactMatch {
            ignore_case: false,
            trim: false,
        });
        let mut card = card;
        card.scorers[0].expected_path = "/answer".into();
        assert_eq!(
            paced
                .manifest(&card, &Rubrics::default(), None, None, None)
                .unwrap(),
            declared
                .manifest(&card, &Rubrics::default(), None, None, None)
                .unwrap()
        );
    }

    #[test]
    fn a_setting_outside_its_bounds_or_with_nothing_to_pace_is_refused_by_name() {
        let mut quick = run(DatasetKind::Curation, Answers::Recording(recording()));
        quick.settings.timeout_seconds = Some(5);
        let refused = quick.validate().unwrap_err().to_string();
        assert!(refused.contains("timeout_seconds"), "{refused}");

        let mut crowded = run(DatasetKind::Curation, Answers::Recording(recording()));
        crowded.settings.concurrency = Some(4);
        let refused = crowded
            .check(&card(Scorer::Forbidden {
                text: "ssn".into(),
                ignore_case: false,
            }))
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("concurrency") && refused.contains("asks neither"),
            "{refused}"
        );
    }

    #[test]
    fn a_conversation_cohort_is_answered_from_the_archive_and_nothing_else_is() {
        assert!(
            run(
                DatasetKind::Conversations,
                Answers::Archive(ArchiveWord::Archive)
            )
            .validate()
            .is_ok()
        );
        let staged = run(DatasetKind::Conversations, Answers::Recording(recording()))
            .validate()
            .unwrap_err();
        assert!(staged.to_string().contains("archive"), "{staged}");
        let elsewhere = run(
            DatasetKind::Curation,
            Answers::Archive(ArchiveWord::Archive),
        )
        .validate()
        .unwrap_err();
        assert!(
            elsewhere.to_string().contains("conversation"),
            "{elsewhere}"
        );
    }

    #[test]
    fn a_judge_over_the_archive_is_allowed_and_its_context_and_its_warning_both_say_so() {
        let pinned = VersionReference {
            name: "helpful".into(),
            version: "r".repeat(64),
        };
        let rubrics = Rubrics::default().with(
            &pinned,
            crate::Rubric {
                name: "helpful".into(),
                question: "Does it help?".into(),
                guidance: String::new(),
                scale: crate::Scale::Flag,
                direction: crate::MetricDirection::Higher,
            },
        );
        let mut judged = card(Scorer::Judge {
            rubric: pinned,
            pass_level: None,
        });
        judged.scorers[0].input_path = Some("/question".into());
        let declare = |kind, answers| {
            let mut declared = run(kind, answers);
            declared.judge = Some(JudgeDeclaration {
                provider: "llamacpp".into(),
                model: VersionReference {
                    name: "gemma".into(),
                    version: "q4".into(),
                },
                settings: crate::JudgeSettings::default(),
                calibration: VersionReference {
                    name: "people".into(),
                    version: "c".repeat(64),
                },
            });
            declared
        };
        let set = |from_archive| CalibrationSet {
            name: "people".into(),
            result: VersionReference {
                name: "earlier".into(),
                version: "v".into(),
            },
            items: Vec::new(),
            from_archive,
        };
        let reads = |manifest: &EvaluationManifest| {
            manifest
                .context
                .judge
                .as_ref()
                .is_some_and(|judge| judge.reads_archive)
        };

        let archive = declare(
            DatasetKind::Conversations,
            Answers::Archive(ArchiveWord::Archive),
        );
        assert!(archive.check(&judged).is_ok());
        let manifest = archive
            .manifest(&judged, &rubrics, Some(&set(false)), None, None)
            .unwrap();
        assert!(reads(&manifest));
        let said = warnings(&manifest, &judged, Some(&set(false)));
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("each assistant response"), "{said:?}");
        assert!(
            said[0].contains("what was said to the assistant"),
            "{said:?}"
        );
        assert!(said[0].contains("llamacpp"), "{said:?}");
        assert!(
            archive
                .manifest(&judged, &rubrics, None, None, None)
                .is_err(),
            "whether a judge reads the archive is not guessed without its calibration set"
        );

        let recording = declare(DatasetKind::Curation, Answers::Recording(recording()));
        let quiet = recording
            .manifest(&judged, &rubrics, Some(&set(false)), None, None)
            .unwrap();
        assert!(!reads(&quiet));
        assert!(warnings(&quiet, &judged, Some(&set(false))).is_empty());
        assert!(
            !serde_json::to_string(&quiet)
                .unwrap()
                .contains("reads_archive"),
            "a context whose judge reads nothing of the archive keeps its bytes"
        );
        let calibrated = recording
            .manifest(&judged, &rubrics, Some(&set(true)), None, None)
            .unwrap();
        assert!(reads(&calibrated));
        let said = warnings(&calibrated, &judged, Some(&set(true)));
        assert!(said[0].contains("calibration set"), "{said:?}");
        assert!(!said[0].contains("each assistant response"), "{said:?}");
    }

    #[test]
    fn the_archive_is_never_measured_against_the_response_its_expectation_is() {
        let declared = run(
            DatasetKind::Conversations,
            Answers::Archive(ArchiveWord::Archive),
        );
        let refused = declared
            .check(&card(Scorer::ExactMatch {
                ignore_case: false,
                trim: false,
            }))
            .unwrap_err();
        assert!(refused.to_string().contains("measured"), "{refused}");
        assert!(
            declared
                .check(&card(Scorer::Forbidden {
                    text: "pesel".into(),
                    ignore_case: true,
                }))
                .is_ok()
        );
        let answers = archived(&BTreeMap::from([(
            "turn-1".to_owned(),
            json!({"answer": "hello"}),
        )]));
        assert_eq!(answers[0].case_id, "turn-1");
        assert_eq!(answers[0].answer, json!({"answer": "hello"}));
    }
}
