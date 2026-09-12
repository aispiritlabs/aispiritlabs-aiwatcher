//! One judgement about one thing, in a form somebody declared.
//!
//! An assessment is authored: it is what a person or a judge said, and it
//! outlives the thing it is about. A trace is evicted by retention and a
//! published result is retired by its own clock; the judgement stays, which is
//! why nothing here checks that its target still exists.
//!
//! What it never carries is an expected answer. The cohort owns those — a case
//! page resolves them from the source that was admitted — so a reviewer who
//! writes "the answer should have been 4" is proposing a change to a dataset
//! rather than recording one, and that proposal goes through review before it
//! becomes anything a score is measured against.

use serde::{Deserialize, Serialize};

use crate::{
    EvaluationError, Result, SCHEMA_VERSION, digest, require,
    rubric::{AssessmentValue, Rubric, RubricHead, RubricVersion},
    store::{self, Store},
    text,
};

/// How many times a writer will re-read the current revision and try again.
///
/// Two writers on one standing assessment is one person in two tabs, or a
/// nightly judge overlapping itself. The create is atomic, so a loser sees
/// `false` rather than a lost revision.
const WRITE_ATTEMPTS: u32 = 4;
/// The newest revisions one history request returns.
const HISTORY_PAGE: u32 = 50;
const MAX_HISTORY_PAGE: u32 = 200;

/// Exactly one thing, named the way the thing itself is named.
///
/// Every variant addresses something immutable, or says which moment made it
/// one: a session is still being added to, so an assessment of "the session"
/// has to say as of when, or two people would be judging different things
/// under one address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssessmentTarget {
    Trace {
        trace_id: String,
    },
    Span {
        trace_id: String,
        span_id: String,
    },
    Session {
        session_id: String,
        as_of: i64,
    },
    Case {
        evaluation_id: String,
        case_id: String,
        repetition_id: String,
    },
}

impl AssessmentTarget {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Trace { trace_id } => text(trace_id, "target.trace_id"),
            Self::Span { trace_id, span_id } => {
                text(trace_id, "target.trace_id")?;
                text(span_id, "target.span_id")
            }
            Self::Session { session_id, as_of } => {
                text(session_id, "target.session_id")?;
                require(
                    *as_of > 0,
                    "target.as_of",
                    "a session snapshot needs the moment it was taken",
                )
            }
            Self::Case {
                evaluation_id,
                case_id,
                repetition_id,
            } => {
                text(evaluation_id, "target.evaluation_id")?;
                text(case_id, "target.case_id")?;
                text(repetition_id, "target.repetition_id")
            }
        }
    }

    /// The content address of what is being judged. Derived here rather than
    /// sent, for the reason an approval's ID is: a caller computing it would
    /// be a second answer to what one thing is.
    pub fn address(&self) -> Result<String> {
        digest(&(SCHEMA_VERSION, "evaluation.assessment.target", self))
    }
}

/// Who made the judgement — not who filed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentSource {
    #[default]
    Human,
    Judge,
}

/// One standing judgement's identity.
///
/// The author and the source are *in* it, which is the whole mechanism behind
/// "a person's score does not replace a judge's": two of them about one target
/// under one rubric are two standing assessments, and both are returned. A key
/// that stopped at target and rubric would make the second writer an editor of
/// the first.
pub fn standing_id(
    target_id: &str,
    rubric: &str,
    source: AssessmentSource,
    author: &str,
) -> Result<String> {
    digest(&(
        SCHEMA_VERSION,
        "evaluation.assessment.standing",
        target_id,
        rubric,
        source,
        author,
    ))
}

/// One revision of one standing judgement. Immutable once written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Assessment {
    pub standing_id: String,
    pub target_id: String,
    pub target: AssessmentTarget,
    pub rubric: String,
    /// The concrete version, never the name of a head. A head moves; what was
    /// said was said under one set of levels.
    pub rubric_version: String,
    pub value: AssessmentValue,
    pub source: AssessmentSource,
    /// A person's subject, or the judge that answered.
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
    /// 1 for the first, and one higher for every change of mind. Nothing is
    /// overwritten, so the earlier revisions stay readable.
    pub revision: u32,
    pub recorded_at: i64,
    /// The session that wrote it down, which is not always the author: a judge
    /// answers and something else files the answer.
    pub recorded_by: String,
}

/// What a caller sends. The author is absent for a person on purpose — a client
/// that could name the reviewer could file somebody else's judgement.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssessmentRequest {
    pub target: AssessmentTarget,
    pub rubric: String,
    /// Absent means the head at the moment of writing, resolved here and
    /// recorded as the concrete version it resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric_version: Option<String>,
    pub value: AssessmentValue,
    #[serde(default)]
    pub source: AssessmentSource,
    /// Which judge answered. Required for a judge and refused for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssessmentPage {
    pub target_id: String,
    /// The current revision of every standing judgement about this target.
    pub assessments: Vec<Assessment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssessmentHistory {
    pub standing_id: String,
    /// Newest first.
    pub revisions: Vec<Assessment>,
    /// The revision to continue below, where earlier ones remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<u32>,
}

pub(crate) async fn publish_rubric(
    store: &Store,
    rubric: &Rubric,
    published_by: &str,
    now: i64,
) -> Result<RubricVersion> {
    rubric.validate()?;
    let version = rubric.version()?;
    let key = store::rubric_version(&rubric.name, &version);
    let published = RubricVersion {
        version: version.clone(),
        rubric: rubric.clone(),
        published_by: published_by.to_owned(),
        published_at: now,
    };
    // The version before the head that names it, the way a prompt is written:
    // an unindexed version is waiting to be named, and a head naming nothing
    // is a form the next writer cannot open.
    if !store.create(&key, &published).await? {
        let existing: RubricVersion = store.read(&key).await?.ok_or(
            EvaluationError::Unavailable(crate::EvidenceState::CorruptArtifact),
        )?;
        head_to(store, &existing, now).await?;
        return Ok(existing);
    }
    head_to(store, &published, now).await?;
    Ok(published)
}

async fn head_to(store: &Store, version: &RubricVersion, now: i64) -> Result<()> {
    let head = RubricHead {
        name: version.rubric.name.clone(),
        version: version.version.clone(),
        question: version.rubric.question.clone(),
        updated_at: now,
    };
    store
        .0
        .put(
            &store::rubric_head(&version.rubric.name),
            crate::canonical(&head)?,
        )
        .await?;
    Ok(())
}

pub(crate) async fn rubric_head(store: &Store, name: &str) -> Result<Option<RubricHead>> {
    store.read(&store::rubric_head(name)).await
}

pub(crate) async fn rubric_version(
    store: &Store,
    name: &str,
    version: &str,
) -> Result<Option<RubricVersion>> {
    store.read(&store::rubric_version(name, version)).await
}

pub(crate) async fn rubrics(store: &Store) -> Result<Vec<RubricHead>> {
    let mut heads = Vec::new();
    for entry in store.0.list(store::RUBRICS).await? {
        if entry.key.ends_with("/head.json")
            && let Some(head) = store.read::<RubricHead>(&entry.key).await?
        {
            heads.push(head);
        }
    }
    heads.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(heads)
}

pub(crate) async fn assess(
    store: &Store,
    request: &AssessmentRequest,
    recorded_by: &str,
    now: i64,
) -> Result<Assessment> {
    request.target.validate()?;
    text(&request.rubric, "rubric")?;
    require(
        request.rationale.len() <= 8 * 1024 && !request.rationale.contains('\0'),
        "rationale",
        "must be at most 8 KiB of text",
    )?;
    let author = match request.source {
        AssessmentSource::Human => {
            require(
                request.author.is_none(),
                "author",
                "a person's judgement is attributed to the session that filed it",
            )?;
            recorded_by.to_owned()
        }
        AssessmentSource::Judge => {
            let author = request.author.clone().unwrap_or_default();
            text(&author, "author")?;
            author
        }
    };
    let version = match &request.rubric_version {
        Some(version) => version.clone(),
        None => {
            rubric_head(store, &request.rubric)
                .await?
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "rubric".into(),
                    reason: "no rubric of that name has been published".into(),
                })?
                .version
        }
    };
    let published = rubric_version(store, &request.rubric, &version)
        .await?
        .ok_or_else(|| EvaluationError::Invalid {
            field: "rubric_version".into(),
            reason: "this rubric has no such version".into(),
        })?;
    published.rubric.scale.admits(&request.value)?;

    let target_id = request.target.address()?;
    let standing = standing_id(&target_id, &request.rubric, request.source, &author)?;
    for _ in 0..WRITE_ATTEMPTS {
        let revision = current(store, &target_id, &standing)
            .await?
            .map_or(1, |assessment| assessment.revision + 1);
        let record = Assessment {
            standing_id: standing.clone(),
            target_id: target_id.clone(),
            target: request.target.clone(),
            rubric: request.rubric.clone(),
            rubric_version: version.clone(),
            value: request.value.clone(),
            source: request.source,
            author: author.clone(),
            rationale: request.rationale.clone(),
            revision,
            recorded_at: now,
            recorded_by: recorded_by.to_owned(),
        };
        if store
            .create(&store::assessment(&target_id, &standing, revision), &record)
            .await?
        {
            return Ok(record);
        }
    }
    Err(EvaluationError::Contested)
}

async fn current(store: &Store, target_id: &str, standing: &str) -> Result<Option<Assessment>> {
    let mut keys: Vec<String> = store
        .0
        .list(&store::assessment_standing(target_id, standing))
        .await?
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort();
    match keys.last() {
        Some(key) => store.read(key).await,
        None => Ok(None),
    }
}

/// Every standing judgement about one target, at its current revision.
///
/// This grows with how many people and judges looked at one thing rather than
/// with retention, so it answers whole: a cursor over four rows would be
/// ceremony, and the bound is the only thing worth stating.
pub(crate) async fn assessments(
    store: &Store,
    target: &AssessmentTarget,
) -> Result<AssessmentPage> {
    target.validate()?;
    let target_id = target.address()?;
    let mut keys: Vec<String> = store
        .0
        .list(&store::assessments(&target_id))
        .await?
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort();
    let mut assessments = Vec::new();
    // The keys are `…/{standing}/{revision}`, zero padded and sorted, so the
    // last key of each standing group is that judgement's current revision.
    for (index, key) in keys.iter().enumerate() {
        let group = key.rsplit_once('/').map(|(head, _)| head);
        let next = keys
            .get(index + 1)
            .and_then(|key| key.rsplit_once('/').map(|(head, _)| head));
        if group == next {
            continue;
        }
        if let Some(assessment) = store.read::<Assessment>(key).await? {
            assessments.push(assessment);
        }
    }
    assessments.sort_by(|a, b| {
        (&a.rubric, a.source as u8, &a.author).cmp(&(&b.rubric, b.source as u8, &b.author))
    });
    Ok(AssessmentPage {
        target_id,
        assessments,
    })
}

/// What one standing judgement said, newest first.
///
/// Addressed by the two IDs the server issued rather than by the target again:
/// a caller holds both from the listing, and a judge that re-scores nightly
/// writes a revision a night, so this pages — the one part of an assessment
/// that grows without a person doing anything.
pub(crate) async fn history(
    store: &Store,
    target_id: &str,
    standing: &str,
    before: Option<u32>,
    limit: Option<u32>,
) -> Result<AssessmentHistory> {
    text(target_id, "target_id")?;
    text(standing, "standing_id")?;
    let limit = limit.unwrap_or(HISTORY_PAGE);
    require(
        (1..=MAX_HISTORY_PAGE).contains(&limit),
        "limit",
        "must be between 1 and 200",
    )?;
    let mut keys: Vec<String> = store
        .0
        .list(&store::assessment_standing(target_id, standing))
        .await?
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort();
    keys.reverse();
    let mut revisions = Vec::new();
    let mut next_cursor = None;
    for key in keys {
        let Some(assessment) = store.read::<Assessment>(&key).await? else {
            continue;
        };
        if before.is_some_and(|before| assessment.revision >= before) {
            continue;
        }
        if revisions.len() as u32 == limit {
            next_cursor = Some(assessment.revision + 1);
            break;
        }
        revisions.push(assessment);
    }
    Ok(AssessmentHistory {
        standing_id: standing.to_owned(),
        revisions,
        next_cursor,
    })
}

/// Which of the four a query names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Trace,
    Span,
    Session,
    Case,
}

/// A target as a URL carries it.
///
/// The flat form lives beside the target it builds rather than beside the
/// handler, because which fields a span needs is a fact about a span. Written
/// out in the route it would be a second description of one thing, and the day
/// a fifth kind arrives one of the two would be the one somebody forgot.
///
/// A field the named kind does not use is refused rather than ignored: a trace
/// query carrying a `span_id` is somebody who believes they narrowed to a span.
#[derive(Clone, Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AssessmentTargetQuery {
    pub kind: TargetKind,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub session_id: Option<String>,
    pub as_of: Option<i64>,
    pub evaluation_id: Option<String>,
    pub case_id: Option<String>,
    pub repetition_id: Option<String>,
}

impl AssessmentTargetQuery {
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] naming the field that is missing, or the
    /// one that belongs to a different kind of target.
    pub fn target(&self) -> Result<AssessmentTarget> {
        let present = [
            ("trace_id", self.trace_id.is_some()),
            ("span_id", self.span_id.is_some()),
            ("session_id", self.session_id.is_some()),
            ("as_of", self.as_of.is_some()),
            ("evaluation_id", self.evaluation_id.is_some()),
            ("case_id", self.case_id.is_some()),
            ("repetition_id", self.repetition_id.is_some()),
        ];
        let used: &[&str] = match self.kind {
            TargetKind::Trace => &["trace_id"],
            TargetKind::Span => &["trace_id", "span_id"],
            TargetKind::Session => &["session_id", "as_of"],
            TargetKind::Case => &["evaluation_id", "case_id", "repetition_id"],
        };
        if let Some((spare, _)) = present
            .iter()
            .find(|(field, present)| *present && !used.contains(field))
        {
            return Err(EvaluationError::Invalid {
                field: (*spare).into(),
                reason: "belongs to a different kind of target than the one named".into(),
            });
        }
        let named = |value: &Option<String>, field: &str| {
            value.clone().ok_or_else(|| EvaluationError::Invalid {
                field: field.into(),
                reason: "this kind of target needs it".into(),
            })
        };
        let target = match self.kind {
            TargetKind::Trace => AssessmentTarget::Trace {
                trace_id: named(&self.trace_id, "trace_id")?,
            },
            TargetKind::Span => AssessmentTarget::Span {
                trace_id: named(&self.trace_id, "trace_id")?,
                span_id: named(&self.span_id, "span_id")?,
            },
            TargetKind::Session => AssessmentTarget::Session {
                session_id: named(&self.session_id, "session_id")?,
                as_of: self.as_of.ok_or_else(|| EvaluationError::Invalid {
                    field: "as_of".into(),
                    reason: "a session snapshot needs the moment it was taken".into(),
                })?,
            },
            TargetKind::Case => AssessmentTarget::Case {
                evaluation_id: named(&self.evaluation_id, "evaluation_id")?,
                case_id: named(&self.case_id, "case_id")?,
                repetition_id: named(&self.repetition_id, "repetition_id")?,
            },
        };
        target.validate()?;
        Ok(target)
    }
}
