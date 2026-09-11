//! Operator-approved synthetic evidence and the independent retention worker.
use aiwatcher_evaluation::{
    DatasetKind, Evaluation, EvaluationError, EvaluationManifest, EvidenceState, Result,
    SourceAuthority, SourceEvidence,
};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

/// The first supported source owner: one operator-selected synthetic bundle.
/// A missing adapter for native governed data is a refusal, never public access.
#[derive(Debug)]
pub struct LocalSource {
    directory: Option<String>,
}
impl LocalSource {
    #[must_use]
    pub fn new(directory: Option<String>) -> Self {
        Self { directory }
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCase {
    case_id: String,
    input: Question,
    expected: Answer,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Question {
    question: String,
}
#[derive(Deserialize, serde::Serialize)]
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
        if manifest.context.dataset.kind != DatasetKind::External
            || manifest.variant.model.is_some()
            || manifest.variant.prompt.is_some()
            || manifest.context.judge.is_some()
        {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let root: PathBuf = tokio::fs::canonicalize(directory).await.map_err(io_error)?;
        let approved: EvaluationManifest = serde_json::from_slice(
            &bytes(
                &root,
                "manifest.json",
                aiwatcher_evaluation::MAX_MANIFEST_BYTES,
            )
            .await?,
        )?;
        let approved = Evaluation::prepare(approved)?;
        let asked = Evaluation::prepare(manifest.clone())?;
        if approved.variant_id() != asked.variant_id()
            || approved.context_id() != asked.context_id()
        {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let v = &manifest.variant;
        let c = &manifest.context;
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
        if c.dataset.version != c.case_manifest.digest {
            return Err(unavailable(EvidenceState::CorruptArtifact));
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
        })
    }
}

/// Retention runs even when no client opens the report. Multiple replicas may
/// sweep concurrently: markers are create-only and deletion is idempotent.
pub fn spawn(
    state: &aiwatcher_api::state::AppState,
    shutdown: tokio_util::sync::CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    let registry = state.evaluations.clone()?;
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = registry.sweep("retention-worker", time::OffsetDateTime::now_utc().unix_timestamp()).await {
                        tracing::warn!(%error, "evaluation retention sweep failed");
                    }
                }
            }
        }
    }))
}
