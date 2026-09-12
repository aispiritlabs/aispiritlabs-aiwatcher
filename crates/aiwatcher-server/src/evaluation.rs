//! Operator-approved evidence, source owner adapters and the retention worker.
mod annotations;
mod conversations;
use aiwatcher_evaluation::{
    CollectionReport, DatasetKind, Evaluation, EvaluationError, EvaluationManifest, EvidenceState,
    Result, SourceAuthority, SourceEvidence,
};
use async_trait::async_trait;
pub use conversations::ConversationCipher;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// One operator-selected bundle, with native source owners.
/// API authentication enforces the shared instance's Viewer/Editor roles;
/// the approved bundle further restricts which source pins may be retained.
#[derive(Debug)]
pub struct LocalSource {
    directory: Option<String>,
    datasets: Option<Arc<aiwatcher_datasets::Registry>>,
    prompts: Option<Arc<aiwatcher_prompts::Registry>>,
    training: Option<Arc<aiwatcher_training::Registry>>,
    annotations: Option<Arc<aiwatcher_annotations::Registry>>,
    conversations: Option<Arc<aiwatcher_conversations::Registry>>,
}
impl LocalSource {
    #[must_use]
    pub fn new(directory: Option<String>) -> Self {
        Self {
            directory,
            datasets: None,
            prompts: None,
            training: None,
            annotations: None,
            conversations: None,
        }
    }

    #[must_use]
    pub fn with_conversations(
        mut self,
        conversations: Arc<aiwatcher_conversations::Registry>,
    ) -> Self {
        self.conversations = Some(conversations);
        self
    }

    #[must_use]
    pub fn with_annotations(mut self, annotations: Arc<aiwatcher_annotations::Registry>) -> Self {
        self.annotations = Some(annotations);
        self
    }

    #[must_use]
    pub fn with_curation(mut self, datasets: Arc<aiwatcher_datasets::Registry>) -> Self {
        self.datasets = Some(datasets);
        self
    }

    #[must_use]
    pub fn with_training(mut self, training: Arc<aiwatcher_training::Registry>) -> Self {
        self.training = Some(training);
        self
    }

    #[must_use]
    pub fn with_prompts(mut self, prompts: Arc<aiwatcher_prompts::Registry>) -> Self {
        self.prompts = Some(prompts);
        self
    }
}

fn unavailable(state: EvidenceState) -> EvaluationError {
    EvaluationError::Unavailable(state)
}
fn io_error(error: std::io::Error) -> EvaluationError {
    unavailable(match error.kind() {
        std::io::ErrorKind::NotFound => EvidenceState::DeletedSource,
        _ => EvidenceState::Forbidden,
    })
}
async fn bytes(root: &Path, name: &str, limit: usize) -> Result<Vec<u8>> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == ".." {
        return Err(unavailable(EvidenceState::Forbidden));
    }
    let path = tokio::fs::canonicalize(root.join(name))
        .await
        .map_err(io_error)?;
    if !path.starts_with(root) {
        return Err(unavailable(EvidenceState::Forbidden));
    }
    let file = tokio::fs::File::open(path).await.map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(unavailable(EvidenceState::CorruptArtifact));
    }
    Ok(bytes)
}
async fn verified(root: &Path, name: &str, digest: &str, length: Option<u64>) -> Result<Vec<u8>> {
    let found = bytes(root, name, 100 * 1024 * 1024).await?;
    if hex::encode(Sha256::digest(&found)) != digest
        || length.is_some_and(|len| len != found.len() as u64)
    {
        return Err(unavailable(EvidenceState::CorruptArtifact));
    }
    Ok(found)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    schema_version: u32,
    cases: Vec<SourceCase>,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceCase {
    case_id: String,
    input: Question,
    expected: Answer,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Question {
    question: String,
}
#[derive(Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Answer {
    answer: String,
}

#[async_trait]
impl SourceAuthority for LocalSource {
    async fn resolve(
        &self,
        manifest: &EvaluationManifest,
        _subject: &str,
    ) -> Result<SourceEvidence> {
        let directory = self
            .directory
            .as_ref()
            .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
        if !matches!(
            manifest.context.dataset.kind,
            DatasetKind::External
                | DatasetKind::Curation
                | DatasetKind::Annotations
                | DatasetKind::Conversations
        ) || manifest.context.judge.is_some()
        {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let asked = Evaluation::prepare(manifest.clone())?;
        let configured: PathBuf = tokio::fs::canonicalize(directory).await.map_err(io_error)?;
        // A directory of approvals, addressed by the pair each one admits, so a
        // second variant is a second subdirectory rather than a swap that hides
        // the first. One bundle directly under the root stays readable.
        let approval = aiwatcher_evaluation::approval_id(asked.variant_id(), asked.context_id())?;
        let root = match tokio::fs::canonicalize(configured.join(&approval)).await {
            Ok(path) if path.starts_with(&configured) => path,
            _ => configured.clone(),
        };
        let declaration = match bytes(
            &root,
            "manifest.json",
            aiwatcher_evaluation::MAX_MANIFEST_BYTES,
        )
        .await
        {
            Ok(found) => found,
            // Nothing admits this pair here. That is a refusal to admit it, not
            // a source somebody deleted: no earlier evidence pointed at it.
            Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
                if root == configured =>
            {
                return Err(unavailable(EvidenceState::Forbidden));
            }
            Err(error) => return Err(error),
        };
        let mut bundle = Sha256::new();
        bundle.update(b"aiwatcher.evaluation.bundle.v1");
        bundle.update(&declaration);
        let approved =
            Evaluation::prepare(serde_json::from_slice::<EvaluationManifest>(&declaration)?)?;
        if approved.variant_id() != asked.variant_id()
            || approved.context_id() != asked.context_id()
        {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let v = &manifest.variant;
        let c = &manifest.context;
        if let Some(model) = &v.model {
            let owner = self
                .training
                .as_ref()
                .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
            let version = owner
                .verified_version(&model.name, &model.version)
                .await
                .map_err(training_error)?;
            let package = version
                .package
                .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
            // Historical model IDs bind artifact digests, not the whole package.
            // The operator approves its full declaration separately; no URI is fetched.
            let declared = bytes(&root, "model-package.json", 1024 * 1024).await?;
            bundle.update(&declared);
            let approved: aiwatcher_training::ModelPackage = serde_json::from_slice(&declared)
                .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
            if serde_json::to_value(&approved)? != serde_json::to_value(&package)? {
                return Err(unavailable(EvidenceState::Forbidden));
            }
            let artifacts = tokio::fs::canonicalize(root.join("model-artifacts"))
                .await
                .map_err(io_error)?;
            if !artifacts.starts_with(&root) {
                return Err(unavailable(EvidenceState::Forbidden));
            }
            let mut remaining = 100 * 1024 * 1024;
            for artifact in &package.artifacts {
                if artifact
                    .size_bytes
                    .is_some_and(|size| size > remaining as u64)
                {
                    return Err(unavailable(EvidenceState::CorruptArtifact));
                }
                let found = bytes(&artifacts, &artifact.name, remaining).await?;
                if hex::encode(Sha256::digest(&found)) != artifact.digest
                    || artifact
                        .size_bytes
                        .is_some_and(|size| size != found.len() as u64)
                {
                    return Err(unavailable(EvidenceState::CorruptArtifact));
                }
                remaining -= found.len();
            }
        }
        if let Some(prompt) = &v.prompt {
            use aiwatcher_core::prompts::{PromptName, PromptVersionId};
            let owner = self
                .prompts
                .as_ref()
                .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
            let name = PromptName::parse(&prompt.name)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?;
            let version = PromptVersionId::parse(&prompt.version)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?;
            owner
                .verified_version(&name, &version)
                .await
                .map_err(prompt_error)?
                .ok_or_else(|| unavailable(EvidenceState::DeletedSource))?;
        }
        for artifact in [
            Some(&v.code),
            Some(&v.generation_config),
            v.response_schema.as_ref(),
            v.tools.as_ref(),
            Some(&c.case_manifest),
            Some(&c.input_schema),
            Some(&c.expectations_schema),
        ]
        .into_iter()
        .flatten()
        {
            // The URI is a pin only. The operator bundle, not the URI, selects
            // bytes, so a manifest can never turn this into an HTTP/file proxy.
            verified(&root, &artifact.name, &artifact.digest, artifact.size_bytes).await?;
        }
        verified(&root, "suite.json", &c.suite.version, None).await?;
        verified(&root, "scorer.py", &c.scorer.version, None).await?;
        if let Some(workflow) = &v.workflow {
            verified(&root, "workflow.json", &workflow.version, None).await?;
        }
        if c.dataset.kind == DatasetKind::External && c.dataset.version != c.case_manifest.digest {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        let bundle_digest = Some(hex::encode(bundle.finalize()));
        if c.dataset.kind == DatasetKind::Conversations {
            let mut evidence = self.conversation_cases(&root, c).await?;
            evidence.bundle_digest = bundle_digest;
            return Ok(evidence);
        }
        if c.dataset.kind == DatasetKind::Annotations {
            let mut evidence = self.annotation_cases(&root, c).await?;
            evidence.bundle_digest = bundle_digest;
            return Ok(evidence);
        }
        let cases: Cases = serde_json::from_slice(
            &verified(
                &root,
                &c.case_manifest.name,
                &c.case_manifest.digest,
                c.case_manifest.size_bytes,
            )
            .await?,
        )?;
        if cases.schema_version != 1 || cases.cases.len() as u64 != c.case_count {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        if c.dataset.kind == DatasetKind::Curation {
            let owner = self
                .datasets
                .as_ref()
                .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
            let snapshot = owner
                .verified_version(&c.dataset.name, &c.dataset.version)
                .await
                .map_err(dataset_error)?;
            let rows: Vec<SourceCase> = snapshot
                .items
                .into_iter()
                .map(|row| serde_json::from_value(serde_json::to_value(row)?))
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
            // Order, IDs, inputs and expectations must agree with the pinned
            // case manifest. Matching only answers would admit different work.
            if rows != cases.cases {
                return Err(unavailable(EvidenceState::CorruptArtifact));
            }
        }
        let mut expected = BTreeMap::new();
        for case in cases.cases {
            if case.case_id.is_empty()
                || case.input.question.len() > 256 * 1024
                || expected
                    .insert(case.case_id, serde_json::to_value(case.expected)?)
                    .is_some()
            {
                return Err(unavailable(EvidenceState::CorruptArtifact));
            }
        }
        Ok(SourceEvidence {
            expected,
            expires_at: None,
            bundle_digest,
        })
    }
}

fn dataset_error(error: aiwatcher_datasets::RegistryError) -> EvaluationError {
    use aiwatcher_datasets::RegistryError;
    match error {
        RegistryError::NotFound(_) => unavailable(EvidenceState::DeletedSource),
        RegistryError::Corrupt { .. } | RegistryError::TooLarge { .. } => {
            unavailable(EvidenceState::CorruptArtifact)
        }
        RegistryError::Store(error) => EvaluationError::Storage(error),
        RegistryError::Invalid(_) | RegistryError::Rejected(_) => {
            unavailable(EvidenceState::Forbidden)
        }
    }
}

fn training_error(error: aiwatcher_training::Error) -> EvaluationError {
    use aiwatcher_training::Error;
    match error {
        Error::NotFound(_) => unavailable(EvidenceState::DeletedSource),
        Error::Corrupt { .. } | Error::TooLarge { .. } => {
            unavailable(EvidenceState::CorruptArtifact)
        }
        Error::Store(error) => EvaluationError::Storage(error),
        Error::Invalid(_) | Error::Refused(_) => unavailable(EvidenceState::Forbidden),
    }
}

fn prompt_error(error: aiwatcher_prompts::RegistryError) -> EvaluationError {
    use aiwatcher_prompts::RegistryError;
    match error {
        RegistryError::UnknownPrompt(_) | RegistryError::UnknownVersion { .. } => {
            unavailable(EvidenceState::DeletedSource)
        }
        RegistryError::Corrupt { .. }
        | RegistryError::Integrity { .. }
        | RegistryError::TooLarge { .. } => unavailable(EvidenceState::CorruptArtifact),
        RegistryError::Store(error) => EvaluationError::Storage(error),
        RegistryError::Invalid(_)
        | RegistryError::InvalidIdentifier { .. }
        | RegistryError::NotAdmitted { .. }
        | RegistryError::UnknownOptimization { .. } => unavailable(EvidenceState::Forbidden),
    }
}

/// Retention runs even when no client opens the report. Multiple replicas may
/// sweep concurrently: markers are create-only and deletion is idempotent.
///
/// Two passes, at two rates, because they answer different questions. Every
/// minute: has anything reached its deadline or lost its source — which reads
/// a claim and stops for most rows. Every hour: is there anything nobody
/// claimed — which lists a prefix per published result and is the expensive
/// half. Collection is about a writer that stopped, and an hour late is the
/// same answer as a minute late.
///
/// Every pass is written down. A sweep that has been failing for a week looks
/// exactly like one that had nothing to do, and the record is what tells them
/// apart — durably, so a restart does not reset the evidence of a problem.
pub fn spawn(
    state: &aiwatcher_api::state::AppState,
    shutdown: tokio_util::sync::CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    let registry = state.evaluations.clone()?;
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut collected_at = None;
        let mut report = registry
            .retention()
            .await
            .unwrap_or_default()
            .unwrap_or_default();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    let due = collected_at
                        .is_none_or(|last: i64| now - last >= COLLECTION_INTERVAL.as_secs() as i64);
                    match pass(&registry, due, now).await {
                        Ok((retired, collection)) => {
                            if due {
                                collected_at = Some(now);
                            }
                            // Collection is hourly, so the passes in between
                            // carry its findings rather than blanking them: a
                            // result with gaps must not vanish from the report
                            // fifty-nine minutes out of sixty. What each pass
                            // counts for itself stays its own.
                            let found = collection.unwrap_or(CollectionReport {
                                damaged: report.damaged.clone(),
                                damaged_count: report.damaged_count,
                                ..CollectionReport::default()
                            });
                            if due && found.damaged_count > 0 {
                                tracing::warn!(
                                    damaged = found.damaged_count,
                                    "published evaluation results are missing bytes"
                                );
                            }
                            report = aiwatcher_evaluation::RetentionReport {
                                ran_at: now,
                                retired,
                                collected: found.removed,
                                collected_at,
                                damaged: found.damaged,
                                damaged_count: found.damaged_count,
                                ..Default::default()
                            };
                            if retired + report.collected > 0 {
                                tracing::info!(
                                    retired,
                                    collected = report.collected,
                                    "evaluation retention pass"
                                );
                            }
                        }
                        Err(error) => {
                            report.failures = report.failures.saturating_add(1);
                            report.failed_at = Some(now);
                            report.error = Some(error.to_string());
                            tracing::error!(
                                %error,
                                failures = report.failures,
                                "evaluation retention sweep failed"
                            );
                        }
                    }
                    if let Err(error) = registry.record_sweep(&report).await {
                        tracing::error!(%error, "cannot record the evaluation retention pass");
                    }
                }
            }
        }
    }))
}

const RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const COLLECTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

async fn pass(
    registry: &std::sync::Arc<aiwatcher_evaluation::Registry>,
    collect: bool,
    now: i64,
) -> Result<(usize, Option<CollectionReport>)> {
    // The worker holds the content capability explicitly: retention applies to
    // governed evidence, and a pass that could not read it would keep it.
    let registry = registry.as_ref().clone().with_content_access(true);
    let collected = match collect {
        true => Some(registry.collect_orphans(now).await?),
        false => None,
    };
    Ok((registry.sweep("retention-worker", now).await?, collected))
}
