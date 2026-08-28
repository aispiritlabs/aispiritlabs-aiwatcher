//! The prompt registry's domain: a named prompt, its immutable versions, and
//! what an optimisation did to it.
//!
//! A prompt is the one artifact in this system that is **authored** rather
//! than observed. Everything else here is a fold over the log — runs, spans,
//! evaluations — and everything else is therefore bounded by retention. A
//! prompt is not: the version a run used has to still be readable long after
//! that run has been evicted, or the trace says "model x, score 0.61" and
//! nothing can say what was asked. So the registry lives in an object store,
//! not in the read model, and it is the only part of aiwatcher that keeps
//! something forever.
//!
//! Three rules carry the meaning:
//!
//! * **A version is its text.** [`PromptVersionId`] is `sha256(text)`, the same
//!   reason trace and span ids are derived rather than generated
//!   (`ADR_0001`): publishing the same text twice lands on the version that is
//!   already there instead of writing a second one. A producer that computes
//!   the hash itself — as `planner` already does — arrives at the same id.
//! * **An optimisation's verdict is computed, not supplied.** A client reports
//!   what it measured; [`OptimizationRecord::verdict`] decides whether that
//!   counts as an improvement. See [`OptimizationOutcome`].
//! * **A candidate that drops a variable is not a candidate.** An optimiser
//!   rewrites prompt text freely and can delete a `{{ placeholder }}` while
//!   scoring better on a harness that fed it fixed inputs. Promoting that
//!   prompt ships one that never interpolates its input. [`variables_of`] is
//!   what makes the loss visible, and it is a hard bar on admission.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ports::{PortError, PortResult};

/// The label a deployment reads to answer "which version is live".
pub const PRODUCTION_LABEL: &str = "production";

/// How long a prompt name may be. Long enough for `planner-floor-plan-system`,
/// short enough that it is an identifier rather than a sentence.
pub const MAX_NAME_LENGTH: usize = 128;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum PromptError {
    #[error(
        "{value:?} is not a valid prompt name: use lowercase letters, digits, '.', '_' and '-', \
         starting with a letter or digit, at most {MAX_NAME_LENGTH} characters"
    )]
    InvalidName { value: String },

    #[error("a prompt version cannot be empty")]
    EmptyText,

    #[error("{value:?} is not a version id: expected 64 lowercase hex characters")]
    InvalidVersionId { value: String },
}

/// A prompt's identity.
///
/// Deliberately without `/`. A name is a path segment in
/// `/api/v1/prompts/{name}/versions/{version_id}`, and a name that can contain
/// a separator makes that route ambiguous with every route below it. Namespace
/// with `-` or `.` instead: `planner.floor-plan.system`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, utoipa::ToSchema)]
#[serde(transparent)]
#[schema(value_type = String, example = "planner.floor-plan.system")]
pub struct PromptName(String);

impl PromptName {
    /// # Errors
    ///
    /// [`PromptError::InvalidName`] when the value is empty, too long, or
    /// contains anything outside `[a-z0-9._-]`.
    pub fn parse(value: impl Into<String>) -> Result<Self, PromptError> {
        let value = value.into();
        let invalid = || PromptError::InvalidName {
            value: value.clone(),
        };
        if value.is_empty() || value.len() > MAX_NAME_LENGTH {
            return Err(invalid());
        }
        let mut characters = value.chars();
        // A leading '.' or '-' would make the object key look like a relative
        // path or a flag; requiring an alphanumeric first character rules both
        // out without a second check.
        if !characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        {
            return Err(invalid());
        }
        if !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        }) {
            return Err(invalid());
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PromptName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for PromptName {
    type Err = PromptError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for PromptName {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(de)?).map_err(serde::de::Error::custom)
    }
}

/// `sha256(text)`, lowercase hex.
///
/// Content addressing is what makes publishing idempotent: a redelivered
/// publish, a CI job that runs twice, and a producer that hashes its own
/// prompt before sending it all land on one version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, utoipa::ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct PromptVersionId(String);

impl PromptVersionId {
    /// The id of this text. Total: any string has a hash.
    #[must_use]
    pub fn of(text: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        Self(hex::encode(hasher.finalize()))
    }

    /// # Errors
    ///
    /// [`PromptError::InvalidVersionId`] unless the value is 64 lowercase hex
    /// characters. Parsing is strict because the id is also an object key.
    pub fn parse(value: impl Into<String>) -> Result<Self, PromptError> {
        let value = value.into();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(Self(value));
        }
        Err(PromptError::InvalidVersionId { value })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 characters — what a panel or a log line shows.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.0[..12]
    }
}

impl fmt::Display for PromptVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for PromptVersionId {
    type Err = PromptError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for PromptVersionId {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(de)?).map_err(serde::de::Error::custom)
    }
}

/// Where a version came from.
///
/// The distinction the panel needs to answer "did a person write this, or did
/// an optimiser?" — which is the first question asked about a prompt that is
/// behaving strangely.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "origin")]
pub enum VersionOrigin {
    /// A person wrote it, or it was imported from a repository.
    #[default]
    Authored,
    /// An optimiser produced it. `optimization_id` is the record that says
    /// what it was measured against.
    Optimized {
        optimization_id: String,
        algorithm: String,
    },
}

/// One immutable version of a prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PromptVersion {
    pub name: PromptName,
    pub version_id: PromptVersionId,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Why this version exists. The commit message of a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// What it was written for. A prompt tuned on one model is not evidence
    /// about another, and this is what says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The `{{ placeholders }}` the text interpolates, sorted. Derived from
    /// the text by [`variables_of`], never supplied — a declared list that
    /// disagrees with the text is worse than no list.
    #[serde(default)]
    pub variables: Vec<String>,
    /// The version this one was derived from, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PromptVersionId>,
    #[serde(default, flatten)]
    pub origin: VersionOrigin,
    /// Anything the producer wants to carry: a git sha, a ticket, a locale.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl PromptVersion {
    /// Build a version from its text, deriving the id and the variables.
    ///
    /// # Errors
    ///
    /// [`PromptError::EmptyText`] for text that is empty or only whitespace.
    /// An empty prompt is always a bug in the caller, and storing it would
    /// make an optimiser's "improvement" over nothing look like progress.
    pub fn new(
        name: PromptName,
        text: impl Into<String>,
        created_at: OffsetDateTime,
    ) -> Result<Self, PromptError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(PromptError::EmptyText);
        }
        Ok(Self {
            name,
            version_id: PromptVersionId::of(&text),
            variables: variables_of(&text),
            text,
            created_at,
            author: None,
            notes: None,
            model: None,
            parent: None,
            origin: VersionOrigin::Authored,
            metadata: BTreeMap::new(),
        })
    }

    /// What it costs to hold, in bytes of prompt text.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.text.len()
    }

    /// A row for the version list, without the text.
    #[must_use]
    pub fn summary(&self) -> PromptVersionSummary {
        PromptVersionSummary {
            version_id: self.version_id.clone(),
            created_at: self.created_at,
            author: self.author.clone(),
            notes: self.notes.clone(),
            model: self.model.clone(),
            variables: self.variables.clone(),
            parent: self.parent.clone(),
            origin: self.origin.clone(),
            size_bytes: self.size_bytes(),
        }
    }
}

/// A version without its text.
///
/// The list view's row. Kept separate so listing a prompt with two hundred
/// versions does not transfer two hundred prompts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PromptVersionSummary {
    pub version_id: PromptVersionId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub variables: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PromptVersionId>,
    #[serde(default, flatten)]
    pub origin: VersionOrigin,
    pub size_bytes: usize,
}

/// One metric, on the baseline and on the candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Score {
    pub metric: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<f64>,
}

impl Score {
    /// `candidate - baseline`, and `None` when either side is missing.
    ///
    /// A metric only one side reported is not a delta of zero — the same rule
    /// the evaluation comparison applies.
    #[must_use]
    pub fn delta(&self) -> Option<f64> {
        match (self.candidate, self.baseline) {
            (Some(candidate), Some(baseline)) => Some(candidate - baseline),
            _ => None,
        }
    }

    /// Whether the candidate beat the baseline by more than floating-point
    /// noise. A tie is not an improvement.
    #[must_use]
    pub fn improves(&self) -> bool {
        self.delta().is_some_and(|delta| delta > 1e-12)
    }
}

/// What the registry decided about an optimisation.
///
/// Never taken from the client. An optimiser is the last thing that should
/// grade its own output: it selected the candidate by maximising the number it
/// is now reporting.
///
/// Two flat variants with the reason beside them rather than inside them, so
/// the JSON is `{"outcome":"rejected","reason":"variables_lost"}` and the
/// generated TypeScript is a string union. An enum carrying its payload would
/// serialise a level deeper and make every reader unwrap it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationOutcome {
    /// Improved the held-out score and kept every variable. Eligible to become
    /// the production version; moving the label is still a separate act.
    Admitted,
    /// Ran, and did not earn a promotion. `reason` says why.
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// The held-out score did not improve. The ordinary outcome, and the one
    /// the whole dev/test split exists to produce.
    NoHeldOutImprovement,
    /// Nothing was measured on the held-out split. A dev-only result is a
    /// hypothesis; admitting it admits whatever the optimiser overfitted to.
    NoHeldOutMeasurement,
    /// The candidate no longer interpolates a variable the baseline declared.
    /// It would ship a prompt that ignores its input.
    VariablesLost,
    /// The candidate text is identical to the baseline.
    NoChange,
}

/// The outcome and, when there is one, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Verdict {
    pub outcome: OptimizationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RejectionReason>,
}

impl Verdict {
    #[must_use]
    pub const fn admitted() -> Self {
        Self {
            outcome: OptimizationOutcome::Admitted,
            reason: None,
        }
    }

    #[must_use]
    pub const fn rejected(reason: RejectionReason) -> Self {
        Self {
            outcome: OptimizationOutcome::Rejected,
            reason: Some(reason),
        }
    }

    #[must_use]
    pub const fn is_admitted(self) -> bool {
        matches!(self.outcome, OptimizationOutcome::Admitted)
    }
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoHeldOutImprovement => "the held-out score did not improve",
            Self::NoHeldOutMeasurement => "nothing was measured on the held-out split",
            Self::VariablesLost => "the candidate dropped a variable the baseline declared",
            Self::NoChange => "the candidate is identical to the baseline",
        })
    }
}

/// One optimisation run against one prompt.
///
/// Written by whoever ran the optimiser — `deepeval`'s `PromptOptimizer` is
/// the case this was built for — and read by the panel as "what happened to
/// this prompt lately".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OptimizationRecord {
    pub optimization_id: String,
    pub prompt: PromptName,
    /// What produced the candidate, e.g. `deepeval/SIMBA`. Free text: the
    /// registry does not have opinions about optimisers, only about evidence.
    pub algorithm: String,
    pub baseline: PromptVersionId,
    pub candidate: PromptVersionId,
    /// The metric the verdict is decided on. Every other metric is reported
    /// and none of them promotes anything — a gate with several thresholds is
    /// a gate somebody will tune until it opens.
    pub primary_metric: String,
    /// What the optimiser optimised against. Guides the search; proves nothing.
    #[serde(default)]
    pub dev: Vec<Score>,
    /// The held-out split. The only evidence that admits a candidate.
    #[serde(default)]
    pub test: Vec<Score>,
    /// Which cases each split was drawn from, ideally versioned. Two scores on
    /// different cases are two facts, not a comparison — the same rule
    /// `baseline_for` applies to evaluation reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    /// Variables the baseline declared and the candidate does not. Computed by
    /// the registry from both texts.
    #[serde(default)]
    pub variables_lost: Vec<String>,
    pub outcome: OptimizationOutcome,
    /// Why it was rejected. Absent on an admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RejectionReason>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<u32>,
    /// The evaluation report this optimisation published, where it published
    /// one. The join between the registry and the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_id: Option<String>,
    /// Whatever the optimiser produced — `deepeval`'s serialised
    /// `OptimizationReport`, an iteration trace, anything. Bounded by the
    /// registry, not by this type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub report: Option<serde_json::Value>,
}

impl OptimizationRecord {
    /// Decide the outcome from the evidence.
    ///
    /// The order matters. `NoChange` first, because an optimiser that returned
    /// the baseline has not produced a candidate at all and every score it
    /// reports is trivially unchanged. `VariablesLost` before the scores,
    /// because a prompt that ignores its input can score arbitrarily well on a
    /// harness that never varied it — reporting "did not improve" would be the
    /// wrong reason and would invite raising the iteration count.
    #[must_use]
    pub fn verdict(
        baseline: &PromptVersionId,
        candidate: &PromptVersionId,
        variables_lost: &[String],
        primary_metric: &str,
        test: &[Score],
    ) -> Verdict {
        if baseline == candidate {
            return Verdict::rejected(RejectionReason::NoChange);
        }
        if !variables_lost.is_empty() {
            return Verdict::rejected(RejectionReason::VariablesLost);
        }
        let Some(primary) = test.iter().find(|score| score.metric == primary_metric) else {
            return Verdict::rejected(RejectionReason::NoHeldOutMeasurement);
        };
        if primary.delta().is_none() {
            return Verdict::rejected(RejectionReason::NoHeldOutMeasurement);
        }
        if primary.improves() {
            Verdict::admitted()
        } else {
            Verdict::rejected(RejectionReason::NoHeldOutImprovement)
        }
    }

    #[must_use]
    pub fn is_admitted(&self) -> bool {
        self.outcome == OptimizationOutcome::Admitted
    }

    /// One metric from a split, by name.
    #[must_use]
    pub fn score(&self, split: Split, metric: &str) -> Option<&Score> {
        let scores = match split {
            Split::Dev => &self.dev,
            Split::Test => &self.test,
        };
        scores.iter().find(|score| score.metric == metric)
    }

    /// How far the dev gain outran the held-out gain on the primary metric.
    ///
    /// The overfitting signal, and the number worth watching across a series
    /// of optimisations: a run that gains 0.2 on dev and 0.0 on test found
    /// something about the dev split, not about the task.
    #[must_use]
    pub fn overfit_gap(&self) -> Option<f64> {
        let dev = self.score(Split::Dev, &self.primary_metric)?.delta()?;
        let test = self.score(Split::Test, &self.primary_metric)?.delta()?;
        Some(dev - test)
    }

    /// A row for the "recent optimisations" list, without the report.
    #[must_use]
    pub fn summary(&self) -> OptimizationSummary {
        OptimizationSummary {
            optimization_id: self.optimization_id.clone(),
            algorithm: self.algorithm.clone(),
            baseline: self.baseline.clone(),
            candidate: self.candidate.clone(),
            primary_metric: self.primary_metric.clone(),
            dev_delta: self
                .score(Split::Dev, &self.primary_metric)
                .and_then(Score::delta),
            test_delta: self
                .score(Split::Test, &self.primary_metric)
                .and_then(Score::delta),
            test_score: self
                .score(Split::Test, &self.primary_metric)
                .and_then(|score| score.candidate),
            overfit_gap: self.overfit_gap(),
            dataset: self.dataset.clone(),
            variables_lost: self.variables_lost.clone(),
            outcome: self.outcome,
            reason: self.reason,
            started_at: self.started_at,
            duration_ms: self.duration_ms,
            iterations: self.iterations,
            evaluation_id: self.evaluation_id.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Dev,
    Test,
}

/// An optimisation without its report document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OptimizationSummary {
    pub optimization_id: String,
    pub algorithm: String,
    pub baseline: PromptVersionId,
    pub candidate: PromptVersionId,
    pub primary_metric: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_delta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_delta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overfit_gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    #[serde(default)]
    pub variables_lost: Vec<String>,
    pub outcome: OptimizationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RejectionReason>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_id: Option<String>,
}

/// A prompt's mutable head: the labels, the description, and the index of what
/// is stored under it.
///
/// **Derived, not authoritative.** The versions and the optimisation records
/// are the truth; this is what makes listing them one request instead of one
/// request per object. It can be rebuilt from the store at any time, which is
/// what makes a lost concurrent write survivable rather than a corruption.
/// The exception is [`Self::labels`], which exists nowhere else — a label is a
/// pointer somebody moved, not a fact about an object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PromptHead {
    pub name: PromptName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `production` → the version a deployment should read. Any other label a
    /// team wants (`staging`, `canary`) works the same way.
    #[serde(default)]
    pub labels: BTreeMap<String, PromptVersionId>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Newest first.
    #[serde(default)]
    pub versions: Vec<PromptVersionSummary>,
    /// Newest first, and capped — see `aiwatcher_prompts::RegistryConfig`.
    #[serde(default)]
    pub optimizations: Vec<OptimizationSummary>,
}

impl PromptHead {
    #[must_use]
    pub fn new(name: PromptName, at: OffsetDateTime) -> Self {
        Self {
            name,
            description: None,
            labels: BTreeMap::new(),
            tags: Vec::new(),
            created_at: at,
            updated_at: at,
            versions: Vec::new(),
            optimizations: Vec::new(),
        }
    }

    /// The version a deployment reading `production` would get, falling back
    /// to the newest version when no label has been moved yet.
    ///
    /// The fallback is what makes the registry usable from the first publish:
    /// requiring an explicit promotion before anything can be read turns
    /// "store a prompt" into a two-step ceremony.
    #[must_use]
    pub fn current(&self) -> Option<&PromptVersionId> {
        self.labels
            .get(PRODUCTION_LABEL)
            .or_else(|| self.versions.first().map(|version| &version.version_id))
    }

    /// A row for the prompts list.
    ///
    /// `admitted_optimizations` is on it because a prompt with five
    /// optimisations and no admission is a prompt the optimiser cannot
    /// improve, and that is worth seeing without opening it.
    #[must_use]
    pub fn summary(&self) -> PromptSummary {
        let last = self.optimizations.first();
        PromptSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            labels: self.labels.clone(),
            current: self.current().cloned(),
            versions: self.versions.len(),
            optimizations: self.optimizations.len(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_optimization: last.cloned(),
            admitted_optimizations: self
                .optimizations
                .iter()
                .filter(|record| record.outcome == OptimizationOutcome::Admitted)
                .count(),
        }
    }
}

/// One row in the prompts list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PromptSummary {
    pub name: PromptName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, PromptVersionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<PromptVersionId>,
    pub versions: usize,
    pub optimizations: usize,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// The most recent one, so the list answers "what happened to this prompt"
    /// without a second request per row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_optimization: Option<OptimizationSummary>,
    pub admitted_optimizations: usize,
}

/// The `{{ placeholders }}` a prompt text interpolates, sorted and
/// deduplicated.
///
/// The syntax is the one `planner`'s `PromptBuilder` already enforces:
/// `{{ name }}`, optionally spaced, names starting with a letter. A prompt
/// using some other templating still works — it simply declares no variables,
/// and the variable-loss bar does not apply to it.
#[must_use]
pub fn variables_of(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(open) = text[index..].find("{{").map(|at| index + at) {
        let after = open + 2;
        let Some(close) = text[after..].find("}}").map(|at| after + at) else {
            break;
        };
        let name = text[after..close].trim();
        index = close + 2;
        // A `{{{ x }}}` or a Jinja expression is not a variable this
        // understands; skipping it is better than inventing a name for it.
        if name.is_empty() || bytes.get(open.wrapping_sub(1)) == Some(&b'{') {
            continue;
        }
        let mut characters = name.chars();
        if !characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        {
            continue;
        }
        if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
            continue;
        }
        if !found.iter().any(|existing| existing == name) {
            found.push(name.to_owned());
        }
    }
    found.sort();
    found
}

/// Variables the baseline declares and the candidate does not.
#[must_use]
pub fn variables_lost(baseline: &str, candidate: &str) -> Vec<String> {
    let kept = variables_of(candidate);
    variables_of(baseline)
        .into_iter()
        .filter(|variable| !kept.contains(variable))
        .collect()
}

/// One object in the store.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<OffsetDateTime>,
}

/// Bytes, by key.
///
/// Deliberately not a `PromptStore`. The key layout, the immutability rule and
/// the head index are the same whether the bytes land in RustFS or on a local
/// disk, and a domain-level port would make every adapter reimplement them —
/// which is how two adapters end up disagreeing about where a version lives.
/// So the port is the part that genuinely differs between a bucket and a
/// directory, and `aiwatcher_prompts::Registry` owns the rest.
///
/// The contract is S3's, because that is what the production implementation
/// is: `put` overwrites, `get` returns `None` for a missing key rather than an
/// error, `list` is prefix-scoped and returns every match, and `delete` on a
/// missing key succeeds.
#[async_trait]
pub trait ObjectStore: Send + Sync + std::fmt::Debug {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()>;

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>>;

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>>;

    async fn delete(&self, key: &str) -> PortResult<()>;
}

impl From<PromptError> for PortError {
    fn from(error: PromptError) -> Self {
        Self::Rejected {
            target: "prompt-registry",
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000 + seconds).expect("valid")
    }

    fn version(text: &str) -> PromptVersion {
        PromptVersion::new(
            PromptName::parse("planner.floor-plan").expect("valid"),
            text,
            at(0),
        )
        .expect("non-empty")
    }

    #[test]
    fn a_version_id_is_the_hash_of_its_text_so_publishing_twice_is_one_version() {
        let first = version("Answer in {{ language }}.");
        let second = version("Answer in {{ language }}.");
        assert_eq!(first.version_id, second.version_id);

        let different = version("Answer in {{ language }}!");
        assert_ne!(first.version_id, different.version_id);
    }

    #[test]
    fn a_version_id_matches_a_plain_sha256_of_the_text() {
        // `planner` computes `hashlib.sha256(prompt_text.encode()).hexdigest()`
        // before it ever talks to this registry. If the two disagreed, the same
        // prompt would exist under two ids.
        let expected = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        assert_eq!(PromptVersionId::of("test").as_str(), expected);
    }

    #[test]
    fn a_name_that_could_be_mistaken_for_a_path_is_refused() {
        for valid in ["planner", "planner.floor-plan.system", "a", "v2_prompt"] {
            assert!(PromptName::parse(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "planner/floor-plan",
            "../etc/passwd",
            ".hidden",
            "-flag",
            "Planner",
            "planner floor plan",
        ] {
            assert!(PromptName::parse(invalid).is_err(), "{invalid}");
        }
        assert!(PromptName::parse("a".repeat(MAX_NAME_LENGTH)).is_ok());
        assert!(PromptName::parse("a".repeat(MAX_NAME_LENGTH + 1)).is_err());
    }

    #[test]
    fn an_empty_prompt_is_refused_rather_than_stored() {
        let name = PromptName::parse("empty").expect("valid");
        assert_eq!(
            PromptVersion::new(name.clone(), "   \n\t ", at(0)),
            Err(PromptError::EmptyText)
        );
        assert!(PromptVersion::new(name, "x", at(0)).is_ok());
    }

    #[test]
    fn variables_are_read_from_the_text_rather_than_declared() {
        let text = "Plan for {{ city }} in {{language}}. Repeat: {{ city }}.";
        assert_eq!(variables_of(text), vec!["city", "language"]);

        // Not variables: an unclosed brace, a triple brace, an expression.
        assert!(variables_of("{{ unclosed").is_empty());
        assert!(variables_of("{{{ raw }}}").is_empty());
        assert!(variables_of("{{ 1 + 2 }}").is_empty());
        assert!(variables_of("{{ }}").is_empty());
        assert!(variables_of("no placeholders here").is_empty());
    }

    #[test]
    fn a_candidate_that_drops_a_variable_is_rejected_before_its_score_is_read() {
        let baseline = version("Describe {{ page }} for {{ language }}.");
        let candidate = version("Describe the floor plan in detail.");
        let lost = variables_lost(&baseline.text, &candidate.text);
        assert_eq!(lost, vec!["language", "page"]);

        // Even with a large held-out gain: the harness fed it fixed text, so
        // the number is about the harness rather than the prompt.
        let outcome = OptimizationRecord::verdict(
            &baseline.version_id,
            &candidate.version_id,
            &lost,
            "accuracy",
            &[Score {
                metric: "accuracy".to_owned(),
                baseline: Some(0.5),
                candidate: Some(0.9),
            }],
        );
        assert_eq!(outcome, Verdict::rejected(RejectionReason::VariablesLost));
    }

    #[test]
    fn a_dev_only_gain_never_admits_a_candidate() {
        let baseline = version("a {{ x }}");
        let candidate = version("b {{ x }}");
        // Whatever the dev split says, there is no held-out measurement.
        let outcome = OptimizationRecord::verdict(
            &baseline.version_id,
            &candidate.version_id,
            &[],
            "accuracy",
            &[],
        );
        assert_eq!(
            outcome,
            Verdict::rejected(RejectionReason::NoHeldOutMeasurement)
        );

        // A held-out metric with only one side reported is not a measurement
        // either — there is nothing to compare it to.
        let one_sided = OptimizationRecord::verdict(
            &baseline.version_id,
            &candidate.version_id,
            &[],
            "accuracy",
            &[Score {
                metric: "accuracy".to_owned(),
                baseline: None,
                candidate: Some(0.9),
            }],
        );
        assert_eq!(
            one_sided,
            Verdict::rejected(RejectionReason::NoHeldOutMeasurement)
        );
    }

    #[test]
    fn a_tie_on_the_held_out_split_is_not_an_improvement() {
        let baseline = version("a {{ x }}");
        let candidate = version("b {{ x }}");
        let tie = |candidate_score: f64| {
            OptimizationRecord::verdict(
                &baseline.version_id,
                &candidate.version_id,
                &[],
                "accuracy",
                &[Score {
                    metric: "accuracy".to_owned(),
                    baseline: Some(0.8),
                    candidate: Some(candidate_score),
                }],
            )
        };
        assert_eq!(
            tie(0.8),
            Verdict::rejected(RejectionReason::NoHeldOutImprovement)
        );
        assert_eq!(
            tie(0.79),
            Verdict::rejected(RejectionReason::NoHeldOutImprovement)
        );
        assert_eq!(tie(0.81), Verdict::admitted());
    }

    #[test]
    fn an_optimiser_that_returned_the_baseline_produced_no_candidate() {
        let baseline = version("a {{ x }}");
        assert_eq!(
            OptimizationRecord::verdict(
                &baseline.version_id,
                &baseline.version_id,
                &[],
                "accuracy",
                &[Score {
                    metric: "accuracy".to_owned(),
                    baseline: Some(0.5),
                    candidate: Some(0.9),
                }],
            ),
            Verdict::rejected(RejectionReason::NoChange)
        );
    }

    #[test]
    fn the_overfit_gap_is_what_dev_gained_over_the_held_out_split() {
        let baseline = version("a {{ x }}");
        let candidate = version("b {{ x }}");
        let record = OptimizationRecord {
            optimization_id: "opt-1".to_owned(),
            prompt: baseline.name.clone(),
            algorithm: "deepeval/SIMBA".to_owned(),
            baseline: baseline.version_id.clone(),
            candidate: candidate.version_id.clone(),
            primary_metric: "accuracy".to_owned(),
            dev: vec![Score {
                metric: "accuracy".to_owned(),
                baseline: Some(0.50),
                candidate: Some(0.80),
            }],
            test: vec![Score {
                metric: "accuracy".to_owned(),
                baseline: Some(0.50),
                candidate: Some(0.55),
            }],
            dataset: Some("catalog@1".to_owned()),
            variables_lost: Vec::new(),
            outcome: OptimizationOutcome::Admitted,
            reason: None,
            started_at: at(0),
            ended_at: Some(at(60)),
            duration_ms: Some(60_000),
            iterations: Some(8),
            evaluation_id: None,
            report: None,
        };
        // 0.30 on dev against 0.05 held out: it learned the dev split.
        let gap = record.overfit_gap().expect("both splits reported");
        assert!((gap - 0.25).abs() < 1e-9, "{gap}");

        let summary = record.summary();
        assert_eq!(summary.test_score, Some(0.55));
        assert_eq!(summary.outcome, OptimizationOutcome::Admitted);
        assert_eq!(summary.reason, None, "an admission has no reason to give");
    }

    #[test]
    fn a_head_reads_the_newest_version_until_a_label_is_moved() {
        let name = PromptName::parse("planner.floor-plan").expect("valid");
        let mut head = PromptHead::new(name.clone(), at(0));
        assert_eq!(head.current(), None);

        let newest = version("b {{ x }}");
        let older = version("a {{ x }}");
        head.versions = vec![newest.summary(), older.summary()];
        assert_eq!(head.current(), Some(&newest.version_id));

        head.labels
            .insert(PRODUCTION_LABEL.to_owned(), older.version_id.clone());
        assert_eq!(
            head.current(),
            Some(&older.version_id),
            "a moved label wins over recency"
        );
    }
}
