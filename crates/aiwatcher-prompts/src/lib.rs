//! The prompt registry.
//!
//! Everything else in aiwatcher is a fold over the durable log, and everything
//! else is therefore bounded by retention. A prompt is the exception: it is
//! authored rather than observed, and the version a run used has to outlive
//! every trace of that run. So it lives in an object store — RustFS in a
//! deployment, a directory under `just run` — and this crate owns the layout
//! and the rules.
//!
//! ```text
//! {prefix}/{name}/head.json                        mutable index: labels, description, summaries
//! {prefix}/{name}/versions/{version_id}.json       immutable, content-addressed
//! {prefix}/{name}/optimizations/{id}.json          immutable
//! ```
//!
//! Three things decide how this behaves.
//!
//! **The head is derived; the versions are the truth.** `head.json` exists so
//! that listing a prompt is one request instead of one per object. It holds no
//! fact that is not also in an object it points at — except the labels, which
//! are pointers somebody moved and live nowhere else. Anything the head loses
//! to a concurrent write, [`Registry::rebuild`] recovers by listing.
//!
//! **The version is written before the head.** The same ordering the pipeline
//! applies to its checkpoint, for the same reason: an index naming an object
//! that was never stored is a list whose rows 404, while an object nobody
//! indexed is simply waiting to be rebuilt. Writing the head first would turn
//! a crash into the first case.
//!
//! **The verdict is computed here, not sent.** An optimiser reports what it
//! measured; [`Registry::record_optimization`] fetches the baseline's text,
//! works out what the candidate dropped, and decides. See
//! [`aiwatcher_core::prompts::OptimizationRecord::verdict`].

pub mod adapters;
pub mod sigv4;

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::ports::PortError;
use aiwatcher_core::prompts::{
    ObjectStore, OptimizationRecord, PRODUCTION_LABEL, PromptError, PromptHead, PromptName,
    PromptSummary, PromptVersion, PromptVersionId, Score, VersionOrigin, variables_lost,
};

/// How many heads are fetched at once when listing.
///
/// Listing is one `GET` per prompt: an object store has no query, and a global
/// index would be a second thing to keep consistent with the objects that are
/// already the truth. Sixteen in flight keeps a hundred-prompt registry inside
/// one round trip's worth of latency, which is the size this is for. A
/// registry of ten thousand prompts wants a different design, not a larger
/// number here.
const LIST_CONCURRENCY: usize = 16;

const PROMPTS_PAGE_DEFAULT: usize = 50;
const PROMPTS_PAGE_MAX: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("no prompt named {0}")]
    UnknownPrompt(PromptName),

    #[error("prompt {name} has no version {version}")]
    UnknownVersion {
        name: PromptName,
        version: PromptVersionId,
    },

    #[error("prompt {name} has no optimisation {optimization_id}")]
    UnknownOptimization {
        name: PromptName,
        optimization_id: String,
    },

    #[error(transparent)]
    Invalid(#[from] PromptError),

    #[error("{what} is {size} bytes, over the {limit} byte limit")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error("{value:?} is not a usable identifier: use letters, digits, '.', '_' and '-'")]
    InvalidIdentifier { value: String },

    #[error(transparent)]
    Store(#[from] PortError),

    #[error("the stored object {key} could not be read: {source}")]
    Corrupt {
        key: String,
        #[source]
        source: serde_json::Error,
    },
}

impl RegistryError {
    /// Whether the caller should try again rather than change the request.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Store(error) if error.is_retryable())
    }
}

pub type Result<T> = std::result::Result<T, RegistryError>;

/// What the registry is allowed to hold and index.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    /// Key prefix inside the bucket. Lets one bucket hold this registry beside
    /// whatever else a cluster keeps in it.
    pub prefix: String,
    /// The largest prompt text accepted. A prompt over a quarter of a megabyte
    /// is a document that got pasted into the wrong field.
    pub max_text_bytes: usize,
    /// The largest optimisation report accepted.
    ///
    /// Rejected rather than trimmed, and rejected rather than silently
    /// dropped: unlike the evaluation projection — which sheds documents to
    /// stay inside a memory budget nobody can retry against — the caller here
    /// is a synchronous request that can be told, and can send the scores
    /// without the report.
    pub max_report_bytes: usize,
    /// Version summaries kept in the head.
    ///
    /// A cap on the *index*, never on the store: a version that falls out of
    /// it is still readable by id and still returned by [`Registry::rebuild`]
    /// if the cap is raised. The list is what is bounded, because the list is
    /// what one request has to carry.
    pub max_versions_indexed: usize,
    /// Optimisation summaries kept in the head, newest first. This is what
    /// "the last few optimisations" means on the prompt page.
    pub max_optimizations_indexed: usize,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            prefix: "prompts".to_owned(),
            max_text_bytes: 256 * 1024,
            max_report_bytes: 1024 * 1024,
            max_versions_indexed: 200,
            max_optimizations_indexed: 50,
        }
    }
}

/// Publish a version, and optionally update the prompt around it.
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
pub struct PublishRequest {
    pub name: PromptName,
    pub text: String,
    #[serde(default)]
    pub author: Option<String>,
    /// Why this version exists. The commit message of a prompt.
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// The version this was edited from, where the caller knows.
    #[serde(default)]
    pub parent: Option<PromptVersionId>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Replaces the prompt's description when present.
    #[serde(default)]
    pub description: Option<String>,
    /// Replaces the prompt's tags when present.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Move this label to the new version — `production` to make it live.
    ///
    /// Optional, and separate from publishing, because storing a prompt and
    /// deploying it are different decisions. A publish with no label is a
    /// draft that everything can read and nothing is using.
    #[serde(default)]
    pub label: Option<String>,
}

/// What a publish did.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Published {
    pub version: PromptVersion,
    /// `false` when this exact text was already stored. Publishing is
    /// content-addressed, so a re-run of the same job is not a new version —
    /// and the caller usually wants to know which of the two happened.
    pub created: bool,
    pub head: PromptHead,
}

/// Record what an optimiser did to a prompt.
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
pub struct OptimizationRequest {
    /// The optimiser's own id where it has one — `deepeval` supplies
    /// `OptimizationReport.optimization_id`. Derived from the request when
    /// absent, so recording the same result twice writes one record.
    #[serde(default)]
    pub optimization_id: Option<String>,
    /// What produced the candidate, e.g. `deepeval/SIMBA`.
    pub algorithm: String,
    /// The version it started from. Must already be in the registry: an
    /// optimisation against a prompt nobody stored is a claim with no subject.
    pub baseline: PromptVersionId,
    /// The candidate's text. Published as a version as part of recording, so
    /// the record cannot name a prompt that does not exist.
    pub candidate_text: String,
    /// The metric the verdict is decided on.
    pub primary_metric: String,
    /// What the optimiser optimised against. Guides the search, proves
    /// nothing.
    #[serde(default)]
    pub dev: Vec<Score>,
    /// The held-out split. The only evidence that admits a candidate.
    #[serde(default)]
    pub test: Vec<Score>,
    #[serde(default)]
    pub dataset: Option<String>,
    /// The evaluation report this run published, where it published one.
    #[serde(default)]
    pub evaluation_id: Option<String>,
    #[serde(default)]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub iterations: Option<u32>,
    #[serde(default)]
    pub report: Option<serde_json::Value>,
    /// Move `production` to the candidate if — and only if — it is admitted.
    ///
    /// Off by default. An admitted optimisation has cleared the evidence bar;
    /// deploying it is still somebody's decision, and a registry that promotes
    /// on its own makes every CI run a release.
    #[serde(default)]
    pub promote: bool,
}

/// Filters for the prompts list.
#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct PromptFilter {
    /// Case-insensitive substring over the name, the description and the tags.
    pub search: Option<String>,
    pub tag: Option<String>,
    /// Cursor: the last name on the previous page. Exclusive.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct PromptPage {
    pub prompts: Vec<PromptSummary>,
    /// Pass as `after` for the next page. Absent on the last one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Prompts in the store, before the filter. The difference between this
    /// and `prompts.len()` is what the filter removed.
    pub total: usize,
}

/// The registry.
#[derive(Debug)]
pub struct Registry {
    store: Arc<dyn ObjectStore>,
    config: RegistryConfig,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, config: RegistryConfig) -> Self {
        Self { store, config }
    }

    #[must_use]
    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    // ── Keys ────────────────────────────────────────────────────────────

    fn head_key(&self, name: &PromptName) -> String {
        format!("{}/{name}/head.json", self.config.prefix)
    }

    fn versions_prefix(&self, name: &PromptName) -> String {
        format!("{}/{name}/versions/", self.config.prefix)
    }

    fn version_key(&self, name: &PromptName, version: &PromptVersionId) -> String {
        format!("{}{version}.json", self.versions_prefix(name))
    }

    fn optimizations_prefix(&self, name: &PromptName) -> String {
        format!("{}/{name}/optimizations/", self.config.prefix)
    }

    fn optimization_key(&self, name: &PromptName, optimization_id: &str) -> String {
        format!("{}{optimization_id}.json", self.optimizations_prefix(name))
    }

    // ── Reads ───────────────────────────────────────────────────────────

    /// One prompt's head, or `None` when nothing has been published under the
    /// name.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Store`] when the store is unreachable,
    /// [`RegistryError::Corrupt`] when the head is not readable JSON.
    pub async fn head(&self, name: &PromptName) -> Result<Option<PromptHead>> {
        let key = self.head_key(name);
        self.read_json(&key).await
    }

    /// One version, with its text.
    ///
    /// # Errors
    ///
    /// As [`Registry::head`].
    pub async fn version(
        &self,
        name: &PromptName,
        version: &PromptVersionId,
    ) -> Result<Option<PromptVersion>> {
        let key = self.version_key(name, version);
        self.read_json(&key).await
    }

    /// The version a label points at, resolving `production` through
    /// [`PromptHead::current`].
    ///
    /// This is what a deployment calls, and the reason it is one request:
    /// reading a prompt at start-up should not be two.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownPrompt`] for a name nothing was published
    /// under, [`RegistryError::UnknownVersion`] when the label points at a
    /// version the store no longer has.
    pub async fn resolve(&self, name: &PromptName, label: Option<&str>) -> Result<PromptVersion> {
        let head = self
            .head(name)
            .await?
            .ok_or_else(|| RegistryError::UnknownPrompt(name.clone()))?;
        let version = match label {
            Some(label) => head.labels.get(label).cloned(),
            None => head.current().cloned(),
        }
        .ok_or_else(|| RegistryError::UnknownPrompt(name.clone()))?;
        self.version(name, &version)
            .await?
            .ok_or(RegistryError::UnknownVersion {
                name: name.clone(),
                version,
            })
    }

    /// One optimisation record, with its report.
    ///
    /// # Errors
    ///
    /// As [`Registry::head`].
    pub async fn optimization(
        &self,
        name: &PromptName,
        optimization_id: &str,
    ) -> Result<Option<OptimizationRecord>> {
        let key = self.optimization_key(name, &validate_identifier(optimization_id)?);
        self.read_json(&key).await
    }

    /// Every prompt, filtered and paged.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Store`] when the store is unreachable. A head that
    /// does not parse is skipped with a warning rather than failing the whole
    /// list — one unreadable object should not take the page down with it.
    pub async fn list(&self, filter: &PromptFilter) -> Result<PromptPage> {
        let prefix = format!("{}/", self.config.prefix);
        let mut names: Vec<PromptName> = self
            .store
            .list(&prefix)
            .await?
            .into_iter()
            .filter_map(|entry| {
                let rest = entry.key.strip_prefix(&prefix)?;
                let name = rest.strip_suffix("/head.json")?;
                PromptName::parse(name).ok()
            })
            .collect();
        names.sort();
        names.dedup();
        let total = names.len();

        let heads: Vec<PromptHead> = futures::stream::iter(names)
            .map(|name| async move {
                match self.head(&name).await {
                    Ok(head) => head,
                    Err(error) => {
                        tracing::warn!(%name, %error, "skipping an unreadable prompt head");
                        None
                    }
                }
            })
            .buffer_unordered(LIST_CONCURRENCY)
            .filter_map(|head| async move { head })
            .collect()
            .await;

        let mut summaries: Vec<PromptSummary> = heads
            .iter()
            .map(PromptHead::summary)
            .filter(|summary| matches_filter(summary, filter))
            .collect();
        summaries.sort_by(|left, right| left.name.cmp(&right.name));

        // Paged by name rather than by position: a prompt published between
        // two requests must not shift the page under a reader.
        if let Some(after) = &filter.after {
            summaries.retain(|summary| summary.name.as_str() > after.as_str());
        }
        let limit = filter
            .limit
            .unwrap_or(PROMPTS_PAGE_DEFAULT)
            .clamp(1, PROMPTS_PAGE_MAX);
        let next_cursor = (summaries.len() > limit).then(|| {
            summaries
                .get(limit - 1)
                .map(|summary| summary.name.to_string())
                .unwrap_or_default()
        });
        summaries.truncate(limit);

        Ok(PromptPage {
            prompts: summaries,
            next_cursor,
            total,
        })
    }

    // ── Writes ──────────────────────────────────────────────────────────

    /// Publish a version.
    ///
    /// Idempotent: the version id is `sha256(text)`, so republishing the same
    /// text returns the version already stored with `created: false` and does
    /// not overwrite its provenance. What the head records — the description,
    /// the tags, the label — is still applied, because those are the mutable
    /// half and re-sending them is how a caller corrects them.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Invalid`] for empty text, [`RegistryError::TooLarge`]
    /// past `max_text_bytes`, [`RegistryError::Store`] when a write fails.
    pub async fn publish(&self, request: PublishRequest) -> Result<Published> {
        if request.text.len() > self.config.max_text_bytes {
            return Err(RegistryError::TooLarge {
                what: "the prompt text",
                size: request.text.len(),
                limit: self.config.max_text_bytes,
            });
        }
        let now = OffsetDateTime::now_utc();
        let mut version = PromptVersion::new(request.name.clone(), request.text, now)?;
        version.author = request.author;
        version.notes = request.notes;
        version.model = request.model;
        version.parent = request.parent;
        version.metadata = request.metadata;

        let (version, created) = self.store_version(version).await?;

        let mut head = self
            .head(&request.name)
            .await?
            .unwrap_or_else(|| PromptHead::new(request.name.clone(), now));
        if request.description.is_some() {
            head.description = request.description;
        }
        if let Some(tags) = request.tags {
            head.tags = tags;
        }
        if let Some(label) = request.label {
            head.labels
                .insert(validate_identifier(&label)?, version.version_id.clone());
        }
        self.index_version(&mut head, &version, now);
        self.write_head(&head).await?;

        Ok(Published {
            version,
            created,
            head,
        })
    }

    /// Point a label at a version.
    ///
    /// The deploy step, kept separate from publishing on purpose. `production`
    /// is the one every SDK reads by default; any other name works the same
    /// way.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownPrompt`] or [`RegistryError::UnknownVersion`]
    /// when either side of the pointer is missing — a label may only name a
    /// version that is actually stored.
    pub async fn set_label(
        &self,
        name: &PromptName,
        label: &str,
        version: &PromptVersionId,
    ) -> Result<PromptHead> {
        let label = validate_identifier(label)?;
        let mut head = self
            .head(name)
            .await?
            .ok_or_else(|| RegistryError::UnknownPrompt(name.clone()))?;
        if self.version(name, version).await?.is_none() {
            return Err(RegistryError::UnknownVersion {
                name: name.clone(),
                version: version.clone(),
            });
        }
        head.labels.insert(label, version.clone());
        head.updated_at = OffsetDateTime::now_utc();
        self.write_head(&head).await?;
        Ok(head)
    }

    /// Record an optimisation, publishing its candidate as a version.
    ///
    /// The order is deliberate. The baseline is fetched first — an
    /// optimisation whose starting point is not in the registry is a claim
    /// about nothing — then the candidate is stored, then the verdict is
    /// computed from both texts and the held-out scores, then the record is
    /// written, and only then does the head learn about any of it.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownVersion`] for a baseline that is not stored,
    /// [`RegistryError::TooLarge`] for an oversized report,
    /// [`RegistryError::Store`] when a write fails.
    pub async fn record_optimization(
        &self,
        name: &PromptName,
        request: OptimizationRequest,
    ) -> Result<OptimizationRecord> {
        if let Some(report) = &request.report {
            let size = serde_json::to_vec(report)
                .map(|bytes| bytes.len())
                .unwrap_or(0);
            if size > self.config.max_report_bytes {
                return Err(RegistryError::TooLarge {
                    what: "the optimisation report",
                    size,
                    limit: self.config.max_report_bytes,
                });
            }
        }

        let baseline = self
            .version(name, &request.baseline)
            .await?
            .ok_or_else(|| RegistryError::UnknownVersion {
                name: name.clone(),
                version: request.baseline.clone(),
            })?;

        let now = OffsetDateTime::now_utc();
        let started_at = request.started_at.unwrap_or(now);
        let optimization_id = match request.optimization_id {
            Some(supplied) => validate_identifier(&supplied)?,
            None => derive_optimization_id(
                name,
                &request.algorithm,
                &request.baseline,
                &PromptVersionId::of(&request.candidate_text),
                started_at,
            ),
        };

        // The candidate is stored as an ordinary version, tagged with where it
        // came from. A prompt that an optimiser wrote has to be as readable,
        // as fetchable and as promotable as one a person wrote — it is going
        // to end up in production either way.
        if request.candidate_text.len() > self.config.max_text_bytes {
            return Err(RegistryError::TooLarge {
                what: "the candidate text",
                size: request.candidate_text.len(),
                limit: self.config.max_text_bytes,
            });
        }
        let mut candidate = PromptVersion::new(name.clone(), request.candidate_text, now)?;
        candidate.parent = Some(baseline.version_id.clone());
        candidate.model = baseline.model.clone();
        candidate.origin = VersionOrigin::Optimized {
            optimization_id: optimization_id.clone(),
            algorithm: request.algorithm.clone(),
        };
        candidate.notes = Some(format!(
            "candidate from {} against {}",
            request.algorithm,
            baseline.version_id.short()
        ));
        let (candidate, _) = self.store_version(candidate).await?;

        let lost = variables_lost(&baseline.text, &candidate.text);
        let verdict = OptimizationRecord::verdict(
            &baseline.version_id,
            &candidate.version_id,
            &lost,
            &request.primary_metric,
            &request.test,
        );

        let record = OptimizationRecord {
            optimization_id: optimization_id.clone(),
            prompt: name.clone(),
            algorithm: request.algorithm,
            baseline: baseline.version_id.clone(),
            candidate: candidate.version_id.clone(),
            primary_metric: request.primary_metric,
            dev: request.dev,
            test: request.test,
            dataset: request.dataset,
            variables_lost: lost,
            outcome: verdict.outcome,
            reason: verdict.reason,
            started_at,
            ended_at: Some(now),
            duration_ms: request.duration_ms,
            iterations: request.iterations,
            evaluation_id: request.evaluation_id,
            report: request.report,
        };
        self.write_json(&self.optimization_key(name, &optimization_id), &record)
            .await?;

        let mut head = self
            .head(name)
            .await?
            .unwrap_or_else(|| PromptHead::new(name.clone(), now));
        self.index_version(&mut head, &candidate, now);
        self.index_optimization(&mut head, &record);
        if request.promote && verdict.is_admitted() {
            head.labels
                .insert(PRODUCTION_LABEL.to_owned(), candidate.version_id.clone());
        }
        self.write_head(&head).await?;

        tracing::info!(
            prompt = %name,
            optimization = %optimization_id,
            candidate = %candidate.version_id.short(),
            outcome = ?record.outcome,
            "recorded a prompt optimisation",
        );
        Ok(record)
    }

    /// Re-derive the head by listing what is actually stored.
    ///
    /// The repair path. The head is an index, and an index a concurrent write
    /// truncated is a display problem rather than a data loss — this is what
    /// turns it back. Labels, description and tags are carried over, because
    /// they exist nowhere else.
    ///
    /// # Errors
    ///
    /// [`RegistryError::UnknownPrompt`] when the name has no objects at all.
    pub async fn rebuild(&self, name: &PromptName) -> Result<PromptHead> {
        let mut versions: Vec<PromptVersion> = self.read_all(&self.versions_prefix(name)).await?;
        let mut optimizations: Vec<OptimizationRecord> =
            self.read_all(&self.optimizations_prefix(name)).await?;
        if versions.is_empty() && optimizations.is_empty() {
            return Err(RegistryError::UnknownPrompt(name.clone()));
        }

        versions.sort_by_key(|version| std::cmp::Reverse(version.created_at));
        optimizations.sort_by_key(|record| std::cmp::Reverse(record.started_at));

        let now = OffsetDateTime::now_utc();
        let existing = self.head(name).await?;
        let created_at = versions.last().map_or(now, |oldest| oldest.created_at);
        let mut head = PromptHead::new(name.clone(), created_at);
        if let Some(existing) = existing {
            head.description = existing.description;
            head.tags = existing.tags;
            // A label naming a version that is no longer stored is dropped
            // rather than carried: a `production` pointer into nothing is
            // worse than no pointer, because a reader follows it.
            head.labels = existing
                .labels
                .into_iter()
                .filter(|(_, version)| versions.iter().any(|stored| &stored.version_id == version))
                .collect();
        }
        head.updated_at = now;
        head.versions = versions
            .iter()
            .take(self.config.max_versions_indexed)
            .map(PromptVersion::summary)
            .collect();
        head.optimizations = optimizations
            .iter()
            .take(self.config.max_optimizations_indexed)
            .map(OptimizationRecord::summary)
            .collect();
        self.write_head(&head).await?;
        Ok(head)
    }

    // ── Internals ───────────────────────────────────────────────────────

    /// Write a version, unless that exact text is already stored.
    ///
    /// The read before the write is what keeps provenance: a second publish of
    /// the same text must not replace "written by a person on Tuesday" with
    /// "written by CI just now".
    async fn store_version(&self, version: PromptVersion) -> Result<(PromptVersion, bool)> {
        let key = self.version_key(&version.name, &version.version_id);
        if let Some(existing) = self.read_json::<PromptVersion>(&key).await? {
            return Ok((existing, false));
        }
        self.write_json(&key, &version).await?;
        Ok((version, true))
    }

    /// Put a version summary at the front of the head's index.
    fn index_version(&self, head: &mut PromptHead, version: &PromptVersion, now: OffsetDateTime) {
        head.versions
            .retain(|summary| summary.version_id != version.version_id);
        head.versions.insert(0, version.summary());
        head.versions.truncate(self.config.max_versions_indexed);
        head.updated_at = now;
    }

    fn index_optimization(&self, head: &mut PromptHead, record: &OptimizationRecord) {
        head.optimizations
            .retain(|summary| summary.optimization_id != record.optimization_id);
        head.optimizations.insert(0, record.summary());
        head.optimizations
            .truncate(self.config.max_optimizations_indexed);
        head.updated_at = record.ended_at.unwrap_or(record.started_at);
    }

    async fn write_head(&self, head: &PromptHead) -> Result<()> {
        self.write_json(&self.head_key(&head.name), head).await
    }

    async fn read_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| RegistryError::Corrupt {
                key: key.to_owned(),
                source,
            })
    }

    async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec_pretty(value).map_err(|source| RegistryError::Corrupt {
            key: key.to_owned(),
            source,
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }

    /// Every object under a prefix, parsed. Unreadable ones are skipped with a
    /// warning: a rebuild that refuses to run because one object is damaged is
    /// a repair path that stops working exactly when it is needed.
    async fn read_all<T: serde::de::DeserializeOwned>(&self, prefix: &str) -> Result<Vec<T>> {
        let keys: Vec<String> = self
            .store
            .list(prefix)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .filter(|key| key.ends_with(".json"))
            .collect();
        Ok(futures::stream::iter(keys)
            .map(|key| async move {
                match self.read_json::<T>(&key).await {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(%key, %error, "skipping an unreadable object");
                        None
                    }
                }
            })
            .buffer_unordered(LIST_CONCURRENCY)
            .filter_map(|value| async move { value })
            .collect()
            .await)
    }
}

fn matches_filter(summary: &PromptSummary, filter: &PromptFilter) -> bool {
    if let Some(tag) = &filter.tag
        && !summary.tags.iter().any(|candidate| candidate == tag)
    {
        return false;
    }
    let Some(search) = &filter.search else {
        return true;
    };
    let needle = search.to_lowercase();
    summary.name.as_str().to_lowercase().contains(&needle)
        || summary
            .description
            .as_deref()
            .is_some_and(|description| description.to_lowercase().contains(&needle))
        || summary
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(&needle))
}

/// An id derived from what it identifies.
///
/// So that recording the same optimisation twice — a retried CI step, a
/// redelivered webhook — writes one record instead of two rows claiming the
/// same experiment. The same reason span ids are derived rather than
/// generated.
fn derive_optimization_id(
    name: &PromptName,
    algorithm: &str,
    baseline: &PromptVersionId,
    candidate: &PromptVersionId,
    started_at: OffsetDateTime,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(name.as_str().as_bytes());
    hasher.update(b"|");
    hasher.update(algorithm.as_bytes());
    hasher.update(b"|");
    hasher.update(baseline.as_str().as_bytes());
    hasher.update(b"|");
    hasher.update(candidate.as_str().as_bytes());
    hasher.update(b"|");
    hasher.update(started_at.unix_timestamp().to_string().as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("opt-{}", &digest[..20])
}

/// Identifiers that become object keys and path segments.
///
/// Narrower than it has to be, because widening it later is a migration and
/// narrowing it later is a break.
fn validate_identifier(value: &str) -> Result<String> {
    let acceptable = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !value.starts_with('.')
        && !value.starts_with('-');
    if acceptable {
        Ok(value.to_owned())
    } else {
        Err(RegistryError::InvalidIdentifier {
            value: value.to_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::adapters::memory::MemoryObjectStore;
    use aiwatcher_core::prompts::{OptimizationOutcome, RejectionReason};

    const BASELINE: &str = "Describe the floor plan on {{ page }} in {{ language }}.";
    const CANDIDATE: &str =
        "Read {{ page }} carefully and describe every room in {{ language }}, with areas.";

    fn registry() -> (Registry, MemoryObjectStore) {
        let store = MemoryObjectStore::new();
        (
            Registry::new(Arc::new(store.clone()), RegistryConfig::default()),
            store,
        )
    }

    fn name() -> PromptName {
        PromptName::parse("planner.floor-plan").expect("valid")
    }

    fn publish(text: &str) -> PublishRequest {
        PublishRequest {
            name: name(),
            text: text.to_owned(),
            author: Some("mkubaszek".to_owned()),
            notes: None,
            model: Some("qwen/qwen3-vl-235b".to_owned()),
            parent: None,
            metadata: BTreeMap::new(),
            description: Some("Floor plan extraction".to_owned()),
            tags: Some(vec!["planner".to_owned()]),
            label: None,
        }
    }

    fn optimization(candidate: &str) -> OptimizationRequest {
        OptimizationRequest {
            optimization_id: None,
            algorithm: "deepeval/SIMBA".to_owned(),
            baseline: PromptVersionId::of(BASELINE),
            candidate_text: candidate.to_owned(),
            primary_metric: "mean_score".to_owned(),
            dev: vec![Score {
                metric: "mean_score".to_owned(),
                baseline: Some(0.61),
                candidate: Some(0.78),
            }],
            test: vec![Score {
                metric: "mean_score".to_owned(),
                baseline: Some(0.60),
                candidate: Some(0.66),
            }],
            dataset: Some("catalog@1".to_owned()),
            evaluation_id: Some("eval-7".to_owned()),
            started_at: None,
            duration_ms: Some(1_800_000),
            iterations: Some(8),
            report: None,
            promote: false,
        }
    }

    #[tokio::test]
    async fn publishing_the_same_text_twice_is_one_version() {
        let (registry, _) = registry();
        let first = registry.publish(publish(BASELINE)).await.unwrap();
        assert!(first.created);

        let second = registry.publish(publish(BASELINE)).await.unwrap();
        assert!(!second.created, "the same text is the same version");
        assert_eq!(first.version.version_id, second.version.version_id);
        assert_eq!(
            second.head.versions.len(),
            1,
            "and it is indexed once, not twice"
        );
    }

    #[tokio::test]
    async fn a_republish_does_not_overwrite_the_provenance_of_what_is_stored() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();

        let ci = PublishRequest {
            author: Some("ci".to_owned()),
            notes: Some("re-uploaded by the deploy job".to_owned()),
            ..publish(BASELINE)
        };
        let republished = registry.publish(ci).await.unwrap();
        assert_eq!(
            republished.version.author.as_deref(),
            Some("mkubaszek"),
            "the stored version keeps who actually wrote it"
        );
        assert_eq!(republished.version.notes, None);
    }

    #[tokio::test]
    async fn a_version_is_written_before_the_head_that_indexes_it() {
        // The list must never name an object the store does not have. Checked
        // by reading every version the head points at.
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        registry.publish(publish(CANDIDATE)).await.unwrap();

        let head = registry.head(&name()).await.unwrap().expect("published");
        assert_eq!(head.versions.len(), 2);
        for summary in &head.versions {
            assert!(
                registry
                    .version(&name(), &summary.version_id)
                    .await
                    .unwrap()
                    .is_some(),
                "{} is indexed but not stored",
                summary.version_id
            );
        }
        assert_eq!(
            head.versions.first().map(|summary| &summary.version_id),
            Some(&PromptVersionId::of(CANDIDATE)),
            "newest first"
        );
    }

    #[tokio::test]
    async fn variables_are_derived_from_the_text_that_was_published() {
        let (registry, _) = registry();
        let published = registry.publish(publish(BASELINE)).await.unwrap();
        assert_eq!(published.version.variables, vec!["language", "page"]);
    }

    #[tokio::test]
    async fn an_optimisation_that_beats_the_held_out_split_is_admitted() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();

        let record = registry
            .record_optimization(&name(), optimization(CANDIDATE))
            .await
            .unwrap();
        assert_eq!(record.outcome, OptimizationOutcome::Admitted);
        assert_eq!(record.candidate, PromptVersionId::of(CANDIDATE));
        assert_eq!(record.baseline, PromptVersionId::of(BASELINE));
        assert!(record.variables_lost.is_empty());

        // The candidate is stored as a version, and it says an optimiser wrote it.
        let candidate = registry
            .version(&name(), &record.candidate)
            .await
            .unwrap()
            .expect("stored");
        assert_eq!(candidate.parent, Some(PromptVersionId::of(BASELINE)));
        match candidate.origin {
            VersionOrigin::Optimized { algorithm, .. } => assert_eq!(algorithm, "deepeval/SIMBA"),
            VersionOrigin::Authored => panic!("an optimiser's candidate is not authored"),
        }

        // And the prompt page can answer "what happened lately" without a
        // second request.
        let head = registry.head(&name()).await.unwrap().expect("published");
        let summary = head.summary();
        assert_eq!(summary.admitted_optimizations, 1);
        let last = summary.last_optimization.expect("recorded");
        assert_eq!(last.test_score, Some(0.66));
        assert!((last.overfit_gap.expect("both splits") - 0.11).abs() < 1e-9);
    }

    #[tokio::test]
    async fn an_admitted_optimisation_does_not_deploy_itself() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        let record = registry
            .record_optimization(&name(), optimization(CANDIDATE))
            .await
            .unwrap();
        assert!(record.is_admitted());

        let head = registry.head(&name()).await.unwrap().expect("published");
        assert!(
            !head.labels.contains_key(PRODUCTION_LABEL),
            "recording evidence is not releasing"
        );

        // Asking for it is what moves the label.
        let promoted = registry
            .record_optimization(
                &name(),
                OptimizationRequest {
                    promote: true,
                    ..optimization(CANDIDATE)
                },
            )
            .await
            .unwrap();
        let head = registry.head(&name()).await.unwrap().expect("published");
        assert_eq!(head.labels.get(PRODUCTION_LABEL), Some(&promoted.candidate));
    }

    #[tokio::test]
    async fn a_rejected_optimisation_is_never_promoted_however_loudly_it_is_asked() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();

        // Big dev gain, nothing held out. The exact shape of an overfit.
        let record = registry
            .record_optimization(
                &name(),
                OptimizationRequest {
                    promote: true,
                    dev: vec![Score {
                        metric: "mean_score".to_owned(),
                        baseline: Some(0.60),
                        candidate: Some(0.95),
                    }],
                    test: Vec::new(),
                    ..optimization(CANDIDATE)
                },
            )
            .await
            .unwrap();
        assert_eq!(record.outcome, OptimizationOutcome::Rejected);
        assert_eq!(record.reason, Some(RejectionReason::NoHeldOutMeasurement));
        let head = registry.head(&name()).await.unwrap().expect("published");
        assert!(!head.labels.contains_key(PRODUCTION_LABEL));
        // The candidate is still stored and still listed: a rejected
        // experiment is a result, and losing it means running it again.
        assert!(
            registry
                .version(&name(), &record.candidate)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_candidate_that_drops_a_variable_is_rejected_for_that_reason() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        let record = registry
            .record_optimization(
                &name(),
                OptimizationRequest {
                    promote: true,
                    candidate_text: "Describe every room in detail, with areas.".to_owned(),
                    ..optimization(CANDIDATE)
                },
            )
            .await
            .unwrap();
        assert_eq!(record.outcome, OptimizationOutcome::Rejected);
        assert_eq!(
            record.reason,
            Some(RejectionReason::VariablesLost),
            "the reason is what stops somebody raising the iteration count"
        );
        assert_eq!(record.variables_lost, vec!["language", "page"]);
    }

    #[tokio::test]
    async fn an_optimisation_against_a_baseline_nobody_stored_is_refused() {
        let (registry, _) = registry();
        let error = registry
            .record_optimization(&name(), optimization(CANDIDATE))
            .await
            .expect_err("refused");
        assert!(
            matches!(error, RegistryError::UnknownVersion { .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn recording_the_same_optimisation_twice_writes_one_record() {
        let (registry, store) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        let started = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let request = || OptimizationRequest {
            started_at: Some(started),
            ..optimization(CANDIDATE)
        };
        let first = registry
            .record_optimization(&name(), request())
            .await
            .unwrap();
        let second = registry
            .record_optimization(&name(), request())
            .await
            .unwrap();
        assert_eq!(first.optimization_id, second.optimization_id);

        let records = store
            .list("prompts/planner.floor-plan/optimizations/")
            .await
            .unwrap();
        assert_eq!(records.len(), 1);

        let head = registry.head(&name()).await.unwrap().expect("published");
        assert_eq!(head.optimizations.len(), 1);
    }

    #[tokio::test]
    async fn a_label_may_only_point_at_a_version_that_is_stored() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        let missing = PromptVersionId::of("never published");
        let error = registry
            .set_label(&name(), PRODUCTION_LABEL, &missing)
            .await
            .expect_err("refused");
        assert!(
            matches!(error, RegistryError::UnknownVersion { .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn resolving_a_prompt_reads_production_and_falls_back_to_the_newest() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        registry.publish(publish(CANDIDATE)).await.unwrap();

        // Nothing promoted yet: a reader gets the newest, so the registry is
        // usable from the first publish.
        assert_eq!(
            registry.resolve(&name(), None).await.unwrap().text,
            CANDIDATE
        );

        registry
            .set_label(&name(), PRODUCTION_LABEL, &PromptVersionId::of(BASELINE))
            .await
            .unwrap();
        assert_eq!(
            registry.resolve(&name(), None).await.unwrap().text,
            BASELINE
        );
        assert_eq!(
            registry
                .resolve(&name(), Some(PRODUCTION_LABEL))
                .await
                .unwrap()
                .text,
            BASELINE
        );
        assert!(registry.resolve(&name(), Some("staging")).await.is_err());
    }

    #[tokio::test]
    async fn a_rebuild_recovers_an_index_that_lost_a_write() {
        let (registry, store) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        registry.publish(publish(CANDIDATE)).await.unwrap();
        registry
            .set_label(&name(), PRODUCTION_LABEL, &PromptVersionId::of(BASELINE))
            .await
            .unwrap();

        // Simulate the head losing a concurrent write: it is rolled back to
        // knowing about one version.
        let mut damaged = registry.head(&name()).await.unwrap().expect("published");
        damaged.versions.truncate(1);
        store
            .put(
                "prompts/planner.floor-plan/head.json",
                serde_json::to_vec(&damaged).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            registry
                .head(&name())
                .await
                .unwrap()
                .unwrap()
                .versions
                .len(),
            1
        );

        let rebuilt = registry.rebuild(&name()).await.unwrap();
        assert_eq!(rebuilt.versions.len(), 2, "the objects were always there");
        assert_eq!(
            rebuilt.labels.get(PRODUCTION_LABEL),
            Some(&PromptVersionId::of(BASELINE)),
            "labels survive a rebuild — they exist nowhere else"
        );
    }

    #[tokio::test]
    async fn a_rebuild_drops_a_label_pointing_at_a_version_that_is_gone() {
        let (registry, store) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        registry
            .set_label(&name(), PRODUCTION_LABEL, &PromptVersionId::of(BASELINE))
            .await
            .unwrap();
        store
            .delete(&format!(
                "prompts/planner.floor-plan/versions/{}.json",
                PromptVersionId::of(BASELINE)
            ))
            .await
            .unwrap();
        registry.publish(publish(CANDIDATE)).await.unwrap();

        let rebuilt = registry.rebuild(&name()).await.unwrap();
        assert!(
            !rebuilt.labels.contains_key(PRODUCTION_LABEL),
            "a pointer into nothing is worse than no pointer: a reader follows it"
        );
    }

    #[tokio::test]
    async fn the_list_pages_by_name_and_filters_on_the_server() {
        let (registry, _) = registry();
        for index in 0..5 {
            registry
                .publish(PublishRequest {
                    name: PromptName::parse(format!("prompt-{index}")).unwrap(),
                    text: format!("body {index} {{{{ x }}}}"),
                    tags: Some(vec![if index % 2 == 0 { "even" } else { "odd" }.to_owned()]),
                    ..publish(BASELINE)
                })
                .await
                .unwrap();
        }

        let page = registry
            .list(&PromptFilter {
                limit: Some(2),
                ..PromptFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 5);
        assert_eq!(
            page.prompts
                .iter()
                .map(|summary| summary.name.to_string())
                .collect::<Vec<_>>(),
            ["prompt-0", "prompt-1"]
        );
        let cursor = page.next_cursor.expect("more to come");
        assert_eq!(cursor, "prompt-1");

        let second = registry
            .list(&PromptFilter {
                limit: Some(2),
                after: Some(cursor),
                ..PromptFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(
            second
                .prompts
                .iter()
                .map(|summary| summary.name.to_string())
                .collect::<Vec<_>>(),
            ["prompt-2", "prompt-3"]
        );

        let tagged = registry
            .list(&PromptFilter {
                tag: Some("even".to_owned()),
                ..PromptFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(tagged.prompts.len(), 3);
        assert_eq!(
            tagged.total, 5,
            "total counts what is stored, not what matched"
        );

        let searched = registry
            .list(&PromptFilter {
                search: Some("PROMPT-4".to_owned()),
                ..PromptFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(searched.prompts.len(), 1);
    }

    #[tokio::test]
    async fn an_oversized_prompt_is_refused_rather_than_stored() {
        let store = MemoryObjectStore::new();
        let registry = Registry::new(
            Arc::new(store),
            RegistryConfig {
                max_text_bytes: 16,
                ..RegistryConfig::default()
            },
        );
        let error = registry
            .publish(publish("a".repeat(17).as_str()))
            .await
            .expect_err("refused");
        assert!(matches!(error, RegistryError::TooLarge { .. }), "{error}");
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn an_identifier_that_could_become_a_path_is_refused() {
        let (registry, _) = registry();
        registry.publish(publish(BASELINE)).await.unwrap();
        for hostile in ["../escape", "a/b", "", ".hidden"] {
            let error = registry
                .record_optimization(
                    &name(),
                    OptimizationRequest {
                        optimization_id: Some(hostile.to_owned()),
                        ..optimization(CANDIDATE)
                    },
                )
                .await
                .expect_err(hostile);
            assert!(
                matches!(error, RegistryError::InvalidIdentifier { .. }),
                "{hostile}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn the_head_index_is_capped_and_the_store_is_not() {
        let store = MemoryObjectStore::new();
        let registry = Registry::new(
            Arc::new(store),
            RegistryConfig {
                max_versions_indexed: 2,
                ..RegistryConfig::default()
            },
        );
        for index in 0..5 {
            registry
                .publish(PublishRequest {
                    text: format!("version {index} {{{{ x }}}}"),
                    ..publish(BASELINE)
                })
                .await
                .unwrap();
        }
        let head = registry.head(&name()).await.unwrap().expect("published");
        assert_eq!(head.versions.len(), 2, "the index is bounded");

        // The one that fell out of the index is still readable by id, which is
        // what makes the cap a display decision rather than a deletion.
        let evicted = PromptVersionId::of("version 0 {{ x }}");
        assert!(registry.version(&name(), &evicted).await.unwrap().is_some());
    }
}
