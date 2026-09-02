//! The object-store side: key layout, the accumulate-in-place write, and the
//! two caps that keep a six-hour run one readable object.

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::{
    ModelDetail, ModelHead, ModelLabelRequest, ModelPage, ModelVersion, ModelVersionSummary,
    RegisterModelRequest, RegisteredModel,
};
use crate::run::{
    BestMetric, CheckpointRecord, EpochRecord, FinishRunRequest, ProfileRecord, ProgressRequest,
    RunFilter, SampleRecord, StartRunRequest, TrainingRun, TrainingRunPage, TrainingRunSummary,
    TrainingStatus,
};
use crate::{
    Error, MAX_CHECKPOINTS, MAX_EPOCHS, MAX_PROFILES, MAX_RUN_BYTES, MAX_SAMPLES, Result, digest,
    validate_slug,
};

/// One namespace in the configured authored object store.
#[derive(Clone, Debug)]
pub struct Registry {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
        }
    }

    // ── Runs ─────────────────────────────────────────────────────────────

    /// Open a run, or return the one already open under this id.
    ///
    /// Idempotent on purpose: a trainer that retries its own start after a
    /// timeout must not lose the epochs it already wrote. Re-starting a
    /// *finished* run is refused, because that is a run id being reused and the
    /// second run would inherit the first one's curve.
    pub async fn start(&self, request: StartRunRequest) -> Result<TrainingRun> {
        request.validate()?;
        let key = self.run_key(&request.run_id);
        if let Some(existing) = self.read_json::<TrainingRun>(&key).await? {
            if existing.status.is_finished() {
                return Err(Error::Refused(format!(
                    "the run {} already finished as {}; a run id is used once",
                    request.run_id,
                    existing.status.as_str()
                )));
            }
            return Ok(existing);
        }

        let now = OffsetDateTime::now_utc();
        let run = TrainingRun {
            run_id: request.run_id,
            model: request.model,
            reproducible: crate::run::is_reproducible(&request.dataset),
            dataset: request.dataset,
            schema_version: request.schema_version,
            framework: request.framework,
            device: request.device,
            code: request.code,
            params: request.params,
            workflow_run_id: request.workflow_run_id,
            status: TrainingStatus::Running,
            error: None,
            started_at: now,
            ended_at: None,
            last_heard_from: now,
            epochs: Vec::new(),
            samples: Vec::new(),
            sample_decimations: 0,
            checkpoints: Vec::new(),
            profiles: Vec::new(),
            best: None,
        };
        self.write_run(&run).await?;
        Ok(run)
    }

    /// Fold one batch of progress into the run's record.
    pub async fn progress(&self, run_id: &str, request: ProgressRequest) -> Result<TrainingRun> {
        let mut run = self.run(run_id).await?;
        if run.status.is_finished() {
            return Err(Error::Refused(format!(
                "the run {run_id} finished as {}; its curve is closed",
                run.status.as_str()
            )));
        }
        let now = OffsetDateTime::now_utc();

        for input in request.epochs {
            if input.epoch as usize >= MAX_EPOCHS {
                return Err(Error::TooLarge {
                    what: "the epoch index",
                    size: input.epoch as usize,
                    limit: MAX_EPOCHS,
                });
            }
            let record = EpochRecord {
                epoch: input.epoch,
                duration_ms: finite(input.duration_ms),
                steps: input.steps,
                metrics: input
                    .metrics
                    .into_iter()
                    .map(|(key, value)| (key, finite(value)))
                    .collect(),
                at: now,
            };
            // Replace rather than append: an epoch retried after a network
            // blip has to land on the epoch it already wrote, or the curve
            // grows a second point at the same x.
            match run
                .epochs
                .iter_mut()
                .find(|held| held.epoch == record.epoch)
            {
                Some(held) => *held = record,
                None => run.epochs.push(record),
            }
        }
        run.epochs.sort_by_key(|epoch| epoch.epoch);

        for input in request.samples {
            run.samples.push(SampleRecord {
                step: input.step,
                metrics: input
                    .metrics
                    .into_iter()
                    .map(|(key, value)| (key, finite(value)))
                    .collect(),
                at: now,
            });
        }
        decimate(&mut run);

        for input in request.checkpoints {
            if input.uri.trim().is_empty() {
                return Err(Error::Invalid("a checkpoint needs a uri".to_owned()));
            }
            let record = CheckpointRecord {
                uri: input.uri,
                epoch: input.epoch,
                step: input.step,
                metric: input.metric,
                value: input.value.map(finite),
                best: input.best,
                at: now,
            };
            if record.best
                && let (Some(metric), Some(value)) = (&record.metric, record.value)
            {
                run.best = Some(BestMetric {
                    metric: metric.clone(),
                    value,
                    epoch: record.epoch,
                });
            }
            match run
                .checkpoints
                .iter_mut()
                .find(|held| held.uri == record.uri)
            {
                Some(held) => *held = record,
                None => run.checkpoints.push(record),
            }
            if run.checkpoints.len() > MAX_CHECKPOINTS {
                run.checkpoints.remove(0);
            }
        }

        for input in request.profiles {
            run.profiles.push(ProfileRecord {
                summary: input.summary,
                uri: input.uri,
                at: now,
            });
            if run.profiles.len() > MAX_PROFILES {
                run.profiles.remove(0);
            }
        }

        run.last_heard_from = now;
        self.write_run(&run).await?;
        Ok(run)
    }

    /// Close a run.
    ///
    /// Idempotent in the direction that matters: closing an already-closed run
    /// with the same status is fine, and re-opening one is not possible at all.
    pub async fn finish(&self, run_id: &str, request: FinishRunRequest) -> Result<TrainingRun> {
        let mut run = self.run(run_id).await?;
        if request.status == TrainingStatus::Running {
            return Err(Error::Invalid(
                "finishing a run needs a terminal status".to_owned(),
            ));
        }
        let now = OffsetDateTime::now_utc();
        run.status = request.status;
        run.error = request.error;
        if let Some(best) = request.best {
            run.best = Some(best);
        }
        run.ended_at = Some(run.ended_at.unwrap_or(now));
        run.last_heard_from = now;
        self.write_run(&run).await?;
        Ok(run)
    }

    pub async fn run(&self, run_id: &str) -> Result<TrainingRun> {
        validate_slug(run_id, "a run id")?;
        self.read_json(&self.run_key(run_id))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the training run {run_id}")))
    }

    /// Every run, newest first.
    ///
    /// Reads the small summary object per run rather than the record, so
    /// listing a hundred runs does not pull a hundred curves across the wire.
    pub async fn runs(&self, filter: &RunFilter, limit: usize) -> Result<TrainingRunPage> {
        let mut summaries = Vec::new();
        for entry in self.store.list(&format!("{}/runs/", self.prefix)).await? {
            if !entry.key.ends_with("/summary.json") {
                continue;
            }
            if let Some(summary) = self.read_json::<TrainingRunSummary>(&entry.key).await? {
                summaries.push(summary);
            }
        }
        summaries.retain(|summary| {
            filter
                .model
                .as_ref()
                .is_none_or(|model| &summary.model == model)
                && filter.status.is_none_or(|status| summary.status == status)
                && filter.dataset.as_ref().is_none_or(|dataset| {
                    // An exact `project@version` and a bare project name are
                    // two different questions — "this cut" and "every cut" —
                    // and both are worth asking.
                    &summary.dataset == dataset
                        || summary
                            .dataset
                            .split_once('@')
                            .is_some_and(|(project, _)| project == dataset)
                })
        });
        summaries.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.run_id.cmp(&right.run_id))
        });
        let total = summaries.len();
        summaries.truncate(limit.clamp(1, 500));
        Ok(TrainingRunPage {
            runs: summaries,
            total,
        })
    }

    // ── Models ───────────────────────────────────────────────────────────

    /// Register what a run produced.
    ///
    /// The provenance is read from the run rather than taken from the request:
    /// a version cannot claim a dataset the run it names was not trained on.
    pub async fn register_model(&self, request: RegisterModelRequest) -> Result<RegisteredModel> {
        request.validate()?;
        let run = self.run(&request.run_id).await?;
        let metrics = request.metrics;
        // The package's digest joins the identity: two versions differing
        // only in which weights they name are two versions, and collapsing
        // them would let a promotion point at bytes nobody measured.
        let identity = serde_json::to_vec(&(
            &request.name,
            &request.run_id,
            &request.checkpoint_uri,
            &run.dataset,
            &metrics.validation,
            &metrics.test,
            request
                .package
                .as_ref()
                .map(super::package::ModelPackage::digest),
        ))
        .map_err(|error| Error::Invalid(format!("the version could not be encoded: {error}")))?;

        let version_id = digest(&identity);
        let key = self.model_version_key(&request.name, &version_id);
        let existing: Option<ModelVersion> = self.read_json(&key).await?;
        let created = existing.is_none();
        let version = match existing {
            Some(version) => version,
            None => ModelVersion {
                name: request.name.clone(),
                version: version_id,
                run_id: request.run_id,
                dataset: run.dataset.clone(),
                checkpoint_uri: request.checkpoint_uri,
                framework: run.framework.clone(),
                code: run.code.clone(),
                metrics,
                package: request.package,
                reproducible: run.reproducible,
                notes: request.notes,
                created_at: OffsetDateTime::now_utc(),
            },
        };
        if created {
            // Version before head, the same ordering every registry here keeps.
            self.write_json(&key, &version).await?;
        }

        let head_key = self.model_head_key(&version.name);
        let mut head = self
            .read_json::<ModelHead>(&head_key)
            .await?
            .unwrap_or_else(|| ModelHead {
                name: version.name.clone(),
                description: String::new(),
                versions: Vec::new(),
                labels: BTreeMap::new(),
                updated_at: version.created_at,
            });
        if !request.description.is_empty() {
            head.description = request.description;
        }
        head.versions.retain(|held| held.version != version.version);
        head.versions.insert(0, ModelVersionSummary::of(&version));
        head.updated_at = OffsetDateTime::now_utc();
        self.write_json(&head_key, &head).await?;

        Ok(RegisteredModel {
            promotion_blocked: version
                .check_promotable()
                .err()
                .map(|error| error.to_string()),
            version,
            head,
            created,
        })
    }

    /// Point a label at a version.
    ///
    /// The one place this registry says no. See
    /// [`ModelVersion::check_promotable`].
    pub async fn set_label(&self, name: &str, request: ModelLabelRequest) -> Result<ModelHead> {
        validate_slug(name, "a model name")?;
        validate_slug(&request.label, "a label")?;
        let version: ModelVersion = self
            .read_json(&self.model_version_key(name, &request.version))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the version {} of {name}", request.version)))?;
        version.check_promotable()?;

        let head_key = self.model_head_key(name);
        let mut head: ModelHead = self
            .read_json(&head_key)
            .await?
            .ok_or_else(|| Error::NotFound(format!("the model {name}")))?;
        head.labels.insert(request.label, request.version);
        head.updated_at = OffsetDateTime::now_utc();
        self.write_json(&head_key, &head).await?;
        Ok(head)
    }

    pub async fn models(&self) -> Result<ModelPage> {
        let mut models = Vec::new();
        for entry in self.store.list(&format!("{}/models/", self.prefix)).await? {
            if !entry.key.ends_with("/head.json") {
                continue;
            }
            if let Some(head) = self.read_json::<ModelHead>(&entry.key).await? {
                models.push(head);
            }
        }
        models.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ModelPage { models })
    }

    /// One model, with a version's full record: the one asked for, then the
    /// one `production` points at, then the newest.
    pub async fn model(&self, name: &str, version: Option<&str>) -> Result<ModelDetail> {
        validate_slug(name, "a model name")?;
        let head: ModelHead = self
            .read_json(&self.model_head_key(name))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the model {name}")))?;
        let wanted = version.map(ToOwned::to_owned).or_else(|| {
            head.labelled(crate::model::PRODUCTION)
                .or_else(|| head.latest())
                .map(|summary| summary.version.clone())
        });
        let current = match wanted {
            Some(version) => {
                self.read_json::<ModelVersion>(&self.model_version_key(name, &version))
                    .await?
            }
            None => None,
        };
        Ok(ModelDetail { head, current })
    }

    // ── Store plumbing ───────────────────────────────────────────────────

    /// Record and summary together, record first.
    ///
    /// The summary is what listing reads; writing it before the record would
    /// put a run in the list whose curve is not there yet.
    async fn write_run(&self, run: &TrainingRun) -> Result<()> {
        let body = serde_json::to_vec(run).map_err(|error| Error::Corrupt {
            key: self.run_key(&run.run_id),
            message: error.to_string(),
        })?;
        if body.len() > MAX_RUN_BYTES {
            return Err(Error::TooLarge {
                what: "the training run record",
                size: body.len(),
                limit: MAX_RUN_BYTES,
            });
        }
        self.store.put(&self.run_key(&run.run_id), body).await?;
        self.write_json(&self.run_summary_key(&run.run_id), &run.summary())
            .await
    }

    fn id(name: &str) -> String {
        digest(name.as_bytes())
    }

    fn run_key(&self, run_id: &str) -> String {
        format!("{}/runs/{}/record.json", self.prefix, Self::id(run_id))
    }

    fn run_summary_key(&self, run_id: &str) -> String {
        format!("{}/runs/{}/summary.json", self.prefix, Self::id(run_id))
    }

    fn model_head_key(&self, name: &str) -> String {
        format!("{}/models/{}/head.json", self.prefix, Self::id(name))
    }

    fn model_version_key(&self, name: &str, version: &str) -> String {
        format!(
            "{}/models/{}/versions/{version}.json",
            self.prefix,
            Self::id(name)
        )
    }

    async fn read_json<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let Some(body) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

/// Halve the sampled series when it passes its cap.
///
/// Dropping every second point rather than the oldest or the newest, because
/// what a learning-rate or gradient-norm series is read for is its *shape*: a
/// truncated head loses the warm-up and a truncated tail loses the divergence,
/// while half the resolution loses neither. `sample_decimations` is what says
/// the interval is no longer the one the client chose.
fn decimate(run: &mut TrainingRun) {
    while run.samples.len() > MAX_SAMPLES {
        let mut keep = Vec::with_capacity(run.samples.len().div_ceil(2));
        for (index, sample) in run.samples.drain(..).enumerate() {
            if index % 2 == 0 {
                keep.push(sample);
            }
        }
        run.samples = keep;
        run.sample_decimations += 1;
    }
}

/// A metric that is not a number is not a point on a curve.
///
/// `NaN` and the infinities reach here from a diverged loss, and a JSON
/// document holding one is a document `serde_json` refuses to serialise —
/// which would turn a diverging run into a failed *write* and lose the very
/// epochs that show the divergence.
fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}
