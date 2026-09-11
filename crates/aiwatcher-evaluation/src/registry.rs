//! Durable publication, access and retention behind the domain facade.
use crate::{
    Aggregation, CaseMeasurement, CasePage, DurableEvaluation, DurablePage, Evaluation,
    EvaluationError, EvaluationManifest, EvaluationReceipt, EvidenceCase, EvidenceState,
    PublishEvaluation, Result, ResultCounts, ResultStatus, canonical, require,
    store::{self, Store},
    text,
};
use aiwatcher_core::storage::ObjectStore;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A deployment adapter resolves every pin through its owner and verifies bytes.
/// It must recheck deletion, retention and the caller's rights on every call.
/// Unknown sources fail closed. No fetch of arbitrary producer URLs is implied.
#[async_trait]
pub trait SourceAuthority: Send + Sync + std::fmt::Debug {
    async fn resolve(&self, manifest: &EvaluationManifest, subject: &str)
    -> Result<SourceEvidence>;
}

#[derive(Debug)]
pub struct SourceEvidence {
    pub expected: BTreeMap<String, serde_json::Value>,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct RegistryConfig {
    pub max_cases: usize,
    pub max_bytes: usize,
    pub page_size: usize,
    pub retention_seconds: i64,
}
impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            max_cases: 10_000,
            max_bytes: 100 * 1024 * 1024,
            page_size: 200,
            retention_seconds: 30 * 86400,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Registry {
    store: Store,
    authority: Arc<dyn SourceAuthority>,
    config: RegistryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Shard {
    actual: String,
    expected: String,
    count: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Metadata {
    manifest: EvaluationManifest,
    status: ResultStatus,
    counts: ResultCounts,
    metrics: BTreeMap<String, f64>,
    shards: Vec<Shard>,
}

impl Registry {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        authority: Arc<dyn SourceAuthority>,
        config: RegistryConfig,
    ) -> Result<Self> {
        require(
            config.max_cases > 0
                && config.max_bytes > 0
                && config.page_size > 0
                && config.page_size <= 200
                && config.retention_seconds > 0,
            "registry.config",
            "limits must be positive; page size must not exceed 200",
        )?;
        Ok(Self {
            store: Store(store),
            authority,
            config,
        })
    }

    /// Same logical ID and content is idempotent, including a lost HTTP response.
    /// The terminal payload determines version; the first claim fixes the clock.
    pub async fn publish(
        &self,
        mut request: PublishEvaluation,
        subject: &str,
        now: i64,
    ) -> Result<EvaluationReceipt> {
        let prepared = Evaluation::prepare(request.manifest.clone())?;
        let id = &request.manifest.origin.evaluation_id;
        if let Some(state) = self
            .store
            .read::<EvidenceState>(&store::tombstone(id))
            .await?
        {
            return Err(EvaluationError::Unavailable(state));
        }
        require(
            request.manifest.context.case_count <= self.config.max_cases as u64,
            "context.case_count",
            "exceeds instance limit",
        )?;
        require(
            request.cases.len() <= self.config.max_cases,
            "cases",
            "exceeds instance limit",
        )?;
        require(
            canonical(&request)?.len() <= self.config.max_bytes,
            "result",
            "exceeds instance byte limit",
        )?;
        let source = self.authority.resolve(&request.manifest, subject).await?;
        require(
            source.expected.len() as u64 == request.manifest.context.case_count,
            "source",
            "selected case count differs from pinned manifest",
        )?;
        let expires_at = source
            .expires_at
            .unwrap_or(i64::MAX)
            .min(now.saturating_add(self.config.retention_seconds));
        if expires_at <= now {
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        request.cases.sort_by(|a, b| a.case_id.cmp(&b.case_id));
        let counts = validate(&request, &source)?;
        let metrics = aggregate(&request)?;
        // Shard shape is a versioned storage constant, independent of read-page
        // configuration, so a retry across a config change keeps its version.
        let mut shards = Vec::new();
        let mut bytes = canonical(&request)?.len();
        for cases in request.cases.chunks(200) {
            let expected: Vec<_> = cases
                .iter()
                .map(|case| &source.expected[&case.case_id])
                .collect();
            bytes = bytes.saturating_add(canonical(&expected)?.len());
            require(
                bytes <= self.config.max_bytes,
                "result",
                "responses and expectations exceed byte limit",
            )?;
            shards.push(Shard {
                actual: store::hash(&canonical(&cases)?),
                expected: store::hash(&canonical(&expected)?),
                count: cases.len(),
            });
        }
        let metadata = Metadata {
            manifest: request.manifest.clone(),
            status: request.status,
            counts,
            metrics,
            shards,
        };
        require(
            bytes.saturating_add(canonical(&metadata)?.len()) <= self.config.max_bytes,
            "result",
            "metadata and artifacts exceed byte limit",
        )?;
        // Validate the entire byte budget before the first write. Serialize one
        // shard at a time again, avoiding a second full report kept in memory.
        for cases in request.cases.chunks(200) {
            let expected: Vec<_> = cases
                .iter()
                .map(|case| &source.expected[&case.case_id])
                .collect();
            self.store.artifact(id, &cases).await?;
            self.store.artifact(id, &expected).await?;
        }
        let version = self.store.artifact(id, &metadata).await?;
        let mut receipt = EvaluationReceipt {
            evaluation_id: id.clone(),
            version,
            variant_id: prepared.variant_id().into(),
            context_id: prepared.context_id().into(),
            committed_at: now,
            expires_at,
        };
        // Recheck the source before advertising any bytes. Reads recheck too.
        let latest = self.authority.resolve(&request.manifest, subject).await?;
        receipt.expires_at = receipt
            .expires_at
            .min(latest.expires_at.unwrap_or(i64::MAX));
        if receipt.expires_at <= now {
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        self.store.create(&store::claim(id), &receipt).await?;
        let winner: EvaluationReceipt = self
            .store
            .read(&store::claim(id))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        if winner.version != receipt.version {
            return Err(EvaluationError::Conflict);
        }
        if let Some(state) = self
            .store
            .read::<EvidenceState>(&store::tombstone(id))
            .await?
        {
            self.store.erase(id).await?;
            return Err(EvaluationError::Unavailable(state));
        }
        if winner.expires_at <= now {
            self.retire(id, EvidenceState::Expired).await?;
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        Ok(winner)
    }

    /// Only `None` means unknown and permits a legacy read fallback.
    pub async fn get(
        &self,
        id: &str,
        subject: &str,
        now: i64,
    ) -> Result<Option<DurableEvaluation>> {
        let Some(receipt) = self
            .store
            .read::<EvaluationReceipt>(&store::claim(id))
            .await?
        else {
            return Ok(None);
        };
        let mut result = DurableEvaluation {
            receipt,
            state: EvidenceState::Complete,
            manifest: None,
            status: None,
            counts: None,
            metrics: BTreeMap::new(),
        };
        if let Some(state) = self.store.read(&store::tombstone(id)).await? {
            self.store.erase(id).await?;
            result.state = state;
            return Ok(Some(result));
        }
        if result.receipt.expires_at <= now {
            self.retire(id, EvidenceState::Expired).await?;
            result.state = EvidenceState::Expired;
            return Ok(Some(result));
        }
        match self.read_metadata(&result.receipt, subject, now).await {
            Ok(metadata) => {
                result.state = if metadata.status == ResultStatus::Succeeded
                    && metadata.counts.unscored == 0
                    && metadata.counts.failed == 0
                {
                    EvidenceState::Complete
                } else {
                    EvidenceState::Partial
                };
                result.manifest = Some(metadata.manifest);
                result.status = Some(metadata.status);
                result.counts = Some(metadata.counts);
                result.metrics = metadata.metrics;
            }
            Err(EvaluationError::Unavailable(state)) => {
                if matches!(state, EvidenceState::Expired | EvidenceState::DeletedSource) {
                    self.retire(id, state).await?;
                }
                result.state = state;
            }
            Err(error) => return Err(error),
        }
        Ok(Some(result))
    }

    async fn read_metadata(
        &self,
        receipt: &EvaluationReceipt,
        subject: &str,
        now: i64,
    ) -> Result<Metadata> {
        let metadata: Metadata = self
            .store
            .verified(&receipt.evaluation_id, &receipt.version)
            .await?;
        let prepared = Evaluation::prepare(metadata.manifest.clone())?;
        require(
            prepared.variant_id() == receipt.variant_id
                && prepared.context_id() == receipt.context_id
                && metadata.manifest.origin.evaluation_id == receipt.evaluation_id,
            "receipt",
            "manifest identity mismatch",
        )?;
        let source = self.authority.resolve(&metadata.manifest, subject).await?;
        if source.expires_at.is_some_and(|expiry| expiry <= now) {
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        for shard in &metadata.shards {
            self.read_shard(&receipt.evaluation_id, shard).await?;
        }
        Ok(metadata)
    }

    async fn read_shard(&self, id: &str, shard: &Shard) -> Result<Vec<EvidenceCase>> {
        let actual: Vec<CaseMeasurement> = self.store.verified(id, &shard.actual).await?;
        let expected: Vec<serde_json::Value> = self.store.verified(id, &shard.expected).await?;
        if actual.len() != shard.count || expected.len() != shard.count {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(actual
            .into_iter()
            .zip(expected)
            .map(|(measurement, expected)| EvidenceCase {
                measurement,
                expected,
            })
            .collect())
    }

    pub async fn cases(
        &self,
        id: &str,
        version: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
        subject: &str,
        now: i64,
    ) -> Result<Option<CasePage>> {
        let Some(detail) = self.get(id, subject, now).await? else {
            return Ok(None);
        };
        require(
            detail.receipt.version == version,
            "version",
            "does not match the immutable result",
        )?;
        let mut page = CasePage {
            version: version.into(),
            cases: Vec::new(),
            next_cursor: None,
            state: detail.state,
        };
        if !matches!(
            detail.state,
            EvidenceState::Complete | EvidenceState::Partial
        ) {
            return Ok(Some(page));
        }
        let offset = match cursor {
            None => 0,
            Some(cursor) => cursor
                .strip_prefix(&format!("{version}:"))
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "cursor".into(),
                    reason: "must belong to this result version".into(),
                })?,
        };
        let limit = limit.unwrap_or(self.config.page_size);
        require(limit > 0 && limit <= 200, "limit", "must be 1..200")?;
        let metadata: Metadata = self.store.verified(id, version).await?;
        let total: usize = metadata.shards.iter().map(|s| s.count).sum();
        require(offset <= total, "cursor", "beyond result")?;
        let end = offset.saturating_add(limit).min(total);
        let mut start = 0;
        for shard in &metadata.shards {
            if start < end && start + shard.count > offset {
                let rows = self.read_shard(id, shard).await?;
                page.cases.extend(
                    rows.into_iter()
                        .skip(offset.saturating_sub(start))
                        .take(end - start.max(offset)),
                );
            }
            start += shard.count;
        }
        if end < total {
            page.next_cursor = Some(format!("{version}:{end}"));
        }
        Ok(Some(page))
    }

    /// Minimal identity index for the compatibility adapter. No retained content.
    pub async fn known_ids(&self) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for entry in self.store.0.list("evaluations/").await? {
            if entry.key.ends_with("/claim.json")
                && let Some(receipt) = self.store.read::<EvaluationReceipt>(&entry.key).await?
            {
                ids.insert(receipt.evaluation_id);
            }
        }
        Ok(ids)
    }

    pub async fn list(
        &self,
        cursor: Option<&str>,
        limit: usize,
        subject: &str,
        now: i64,
    ) -> Result<DurablePage> {
        require(limit > 0 && limit <= 200, "limit", "must be 1..200")?;
        let mut entries = self.store.0.list("evaluations/").await?;
        entries.retain(|e| e.key.ends_with("/claim.json"));
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        if let Some(cursor) = cursor {
            require(
                entries.iter().any(|e| e.key == cursor),
                "cursor",
                "unknown discovery cursor",
            )?;
        }
        let selected: Vec<_> = entries
            .into_iter()
            .filter(|e| cursor.is_none_or(|c| e.key.as_str() > c))
            .take(limit + 1)
            .collect();
        let next_cursor = (selected.len() > limit).then(|| selected[limit - 1].key.clone());
        let mut evaluations = Vec::new();
        for entry in selected.iter().take(limit) {
            if let Some(receipt) = self.store.read::<EvaluationReceipt>(&entry.key).await?
                && let Some(detail) = self.get(&receipt.evaluation_id, subject, now).await?
            {
                evaluations.push(detail);
            }
        }
        Ok(DurablePage {
            evaluations,
            next_cursor,
        })
    }

    async fn retire(&self, id: &str, reason: EvidenceState) -> Result<()> {
        // The durable marker precedes erasure; a crash can leave bytes but
        // cannot make the API disclose them or fall back to old telemetry.
        self.store.create(&store::tombstone(id), &reason).await?;
        self.store.erase(id).await
    }

    /// An operator sweep enforces expiry and owner-wide deletion without a read.
    /// The authority must distinguish global deletion from caller-specific denial.
    pub async fn sweep(&self, subject: &str, now: i64) -> Result<usize> {
        let mut retired = 0;
        for entry in self.store.0.list("evaluations/").await? {
            if !entry.key.ends_with("/claim.json") {
                continue;
            }
            if let Some(receipt) = self.store.read::<EvaluationReceipt>(&entry.key).await?
                && let Some(detail) = self.get(&receipt.evaluation_id, subject, now).await?
                && matches!(
                    detail.state,
                    EvidenceState::Expired | EvidenceState::DeletedSource
                )
            {
                retired += 1;
            }
        }
        Ok(retired)
    }
}

fn validate(request: &PublishEvaluation, source: &SourceEvidence) -> Result<ResultCounts> {
    let mut seen = BTreeSet::new();
    let names: BTreeSet<_> = request
        .manifest
        .context
        .metrics
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    let mut failed = 0;
    for case in &request.cases {
        text(&case.case_id, "case_id")?;
        require(
            seen.insert(&case.case_id) && source.expected.contains_key(&case.case_id),
            "case_id",
            "duplicate or outside selected cohort",
        )?;
        require(
            case.repetition_id == request.manifest.origin.repetition_id,
            "repetition_id",
            "must match manifest",
        )?;
        require(
            case.span_id.is_none() || case.trace_id.is_some(),
            "span_id",
            "requires trace_id",
        )?;
        if let Some(error) = &case.error {
            text(error, "case.error")?;
            require(
                case.metrics.is_empty(),
                "case.metrics",
                "failed cases cannot carry scores",
            )?;
            failed += 1;
        } else {
            require(
                case.actual.is_some(),
                "case.actual",
                "scored case requires an actual response",
            )?;
            require(
                case.metrics
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    == names,
                "case.metrics",
                "requires exactly the declared metrics",
            )?;
        }
        for (name, value) in &case.metrics {
            require(value.is_finite(), "case.metrics", "requires finite numbers")?;
            if request
                .manifest
                .context
                .metrics
                .iter()
                .any(|m| m.name == *name && m.aggregation == Aggregation::Rate)
            {
                require(
                    (0.0..=1.0).contains(value),
                    "case.metrics",
                    "rate values must be in 0..1",
                )?;
            }
        }
    }
    let counts = ResultCounts {
        selected: source.expected.len(),
        scored: request.cases.len() - failed,
        failed,
        unscored: source.expected.len() - request.cases.len(),
    };
    if request.status == ResultStatus::Succeeded {
        require(
            counts.failed == 0 && counts.unscored == 0,
            "status",
            "success requires every selected case scored",
        )?;
    }
    Ok(counts)
}

fn aggregate(request: &PublishEvaluation) -> Result<BTreeMap<String, f64>> {
    let mut result = BTreeMap::new();
    for metric in &request.manifest.context.metrics {
        let values: Vec<f64> = request
            .cases
            .iter()
            .filter_map(|c| c.metrics.get(&metric.name).copied())
            .collect();
        if values.is_empty() || metric.aggregation == Aggregation::None {
            continue;
        }
        let value = match metric.aggregation {
            Aggregation::Mean | Aggregation::Rate => {
                values.iter().map(|v| v / values.len() as f64).sum()
            }
            Aggregation::Sum => values.iter().sum(),
            Aggregation::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
            Aggregation::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Aggregation::None => continue,
        };
        require(value.is_finite(), "aggregate", "overflow")?;
        result.insert(metric.name.clone(), value);
    }
    Ok(result)
}
