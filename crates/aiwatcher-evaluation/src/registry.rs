//! Durable publication, access and retention behind the domain facade.
use crate::{
    Aggregation, Approval, ApprovalRecord, CaseMeasurement, CasePage, DurableEvaluation,
    DurablePage, Evaluation, EvaluationError, EvaluationManifest, EvaluationReceipt, EvidenceCase,
    EvidenceState, PreparedEvaluation, PublishEvaluation, Result, ResultCounts, ResultStatus,
    RetentionReport, Withdrawal, approval_id, canonical, require,
    store::{self, Claim, Pending, Store},
    text,
};
use aiwatcher_core::storage::ObjectStore;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// An incomplete upload can reserve its ID for one hour. After collection wins
/// the commit gate, a fresh measurement must use a fresh ID.
pub const PUBLICATION_GRACE_SECONDS: i64 = 3600;

/// A deployment adapter resolves every pin through its owner and verifies bytes.
/// It must recheck deletion, retention and the caller's rights on every call.
/// Unknown sources fail closed. No fetch of arbitrary producer URLs is implied.
#[async_trait]
pub trait SourceAuthority: Send + Sync + std::fmt::Debug {
    async fn resolve(&self, manifest: &EvaluationManifest, subject: &str)
    -> Result<SourceEvidence>;
}

#[derive(Debug, Default)]
pub struct SourceEvidence {
    pub expected: BTreeMap<String, serde_json::Value>,
    pub expires_at: Option<i64>,
    /// What this adapter admitted beyond the manifest's own pinned digests.
    /// An approval records it, so bytes cannot change under an admitted pair.
    pub bundle_digest: Option<String>,
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
    content_access: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Shard {
    actual: String,
    expected: String,
    count: usize,
}
/// One verdict per admitted pair, for the length of one list or one sweep.
///
/// Resolving a source reads the owner's own bytes — a model's artifacts inside
/// a 100 MiB budget, a conversation corpus shard by shard — and a catalogue is
/// mostly repetitions of a handful of pairs. Only a verdict *about the source*
/// is remembered; a transport failure is not one.
#[derive(Debug, Default)]
struct Sources(BTreeMap<String, std::result::Result<Option<i64>, EvidenceState>>);

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
            store: Store(store, None),
            authority,
            config,
            content_access: false,
        })
    }

    /// Grant content access only after the transport has authenticated the
    /// archive's content-reader role. Subject names never grant this capability.
    #[must_use]
    pub fn with_content_access(mut self, allowed: bool) -> Self {
        self.content_access = allowed;
        self
    }

    #[must_use]
    pub fn with_cipher(mut self, cipher: Arc<dyn crate::EvidenceCipher>) -> Self {
        self.store.1 = Some(cipher);
        self
    }

    fn protected(&self, manifest: &EvaluationManifest) -> Result<bool> {
        let protected = manifest.context.dataset.kind == crate::DatasetKind::Conversations;
        if protected && (!self.content_access || self.store.1.is_none()) {
            return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
        }
        Ok(protected)
    }

    /// Admit one pinned pair, once, attributably.
    ///
    /// The adapter proves the declaration still resolves before anything is
    /// written, so an approval is never a promise about bytes nobody has read.
    /// Approving is idempotent for the pair and refuses a bundle that has
    /// changed underneath it — the pair is the identity, so a second answer for
    /// it would silently move what every earlier publication was measured
    /// against. A withdrawn pair is never admitted again under the same ID.
    pub async fn approve(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
        now: i64,
    ) -> Result<Approval> {
        text(subject, "approved_by")?;
        let prepared = Evaluation::prepare(manifest.clone())?;
        self.protected(manifest)?;
        let id = approval_id(prepared.variant_id(), prepared.context_id())?;
        if let Some(approval) = self.approval(&id).await? {
            require(
                approval.admits(),
                "approval",
                "was withdrawn; a withdrawn pair needs a new declaration",
            )?;
        }
        let source = self.authority.resolve(manifest, subject).await?;
        require(
            source.expected.len() as u64 == manifest.context.case_count,
            "source",
            "selected case count differs from the pinned manifest",
        )?;
        let record = ApprovalRecord {
            approval_id: id.clone(),
            variant_id: prepared.variant_id().into(),
            context_id: prepared.context_id().into(),
            bundle_digest: source.bundle_digest,
            approved_by: subject.into(),
            approved_at: now,
        };
        self.store.create(&store::approval(&id), &record).await?;
        let approval = self
            .approval(&id)
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        if approval.record.bundle_digest != record.bundle_digest {
            return Err(EvaluationError::Conflict);
        }
        Ok(approval)
    }

    /// Hide every result measured under one pair, without touching retention.
    /// The marker is create-only and the record is never rewritten, so two
    /// operators withdrawing at once agree on who did it and when.
    pub async fn withdraw(
        &self,
        approval_id: &str,
        subject: &str,
        now: i64,
    ) -> Result<Option<Approval>> {
        text(subject, "withdrawn_by")?;
        if self.approval(approval_id).await?.is_none() {
            return Ok(None);
        }
        self.store
            .create(
                &store::withdrawal(approval_id),
                &Withdrawal {
                    withdrawn_by: subject.into(),
                    withdrawn_at: now,
                },
            )
            .await?;
        self.approval(approval_id).await
    }

    /// Every pair this instance has admitted, withdrawn ones included: an
    /// approval that vanished from the list would read as one nobody made.
    pub async fn approvals(&self) -> Result<Vec<Approval>> {
        let mut approvals = Vec::new();
        let mut entries = self.store.0.list(store::APPROVALS).await?;
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        for entry in entries {
            if !entry.key.ends_with("/record.json") {
                continue;
            }
            if let Some(record) = self.store.read::<ApprovalRecord>(&entry.key).await?
                && entry.key == store::approval(&record.approval_id)
                && let Some(approval) = self.approval(&record.approval_id).await?
            {
                approvals.push(approval);
            }
        }
        Ok(approvals)
    }

    async fn approval(&self, id: &str) -> Result<Option<Approval>> {
        let Some(record) = self
            .store
            .read::<ApprovalRecord>(&store::approval(id))
            .await?
        else {
            return Ok(None);
        };
        require(record.approval_id == id, "approval", "identity mismatch")?;
        Ok(Some(Approval {
            record,
            withdrawn: self.store.read(&store::withdrawal(id)).await?,
        }))
    }

    /// The gate a publication passes and a read is refused by.
    ///
    /// Absence is not withdrawal: evidence published before an instance kept
    /// approvals stays readable, and only the adapter admits it. Absence *is*
    /// a refusal to publish, because that is where the operator's act belongs.
    async fn admitted(
        &self,
        prepared: &PreparedEvaluation,
        publishing: bool,
    ) -> Result<Option<Approval>> {
        let id = approval_id(prepared.variant_id(), prepared.context_id())?;
        let approval = self.approval(&id).await?;
        match &approval {
            Some(approval) if !approval.admits() => {
                Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
            }
            None if publishing => Err(EvaluationError::Unavailable(EvidenceState::Forbidden)),
            _ => Ok(approval),
        }
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
        let protected = self.protected(&request.manifest)?;
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
        // After the adapter, never before it: a source that is gone says so,
        // rather than arriving as "nobody approved this" and sending somebody
        // to approve a pair whose bytes are not there to admit.
        let approval = self.admitted(&prepared, true).await?;
        if approval.is_some_and(|approval| approval.record.bundle_digest != source.bundle_digest) {
            return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
        }
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
        self.store
            .begin(
                Pending {
                    evaluation_id: id.clone(),
                    expires_at: expires_at.min(now.saturating_add(PUBLICATION_GRACE_SECONDS)),
                },
                now,
            )
            .await?;
        for cases in request.cases.chunks(200) {
            let expected: Vec<_> = cases
                .iter()
                .map(|case| &source.expected[&case.case_id])
                .collect();
            self.store.artifact(id, &cases, protected).await?;
            self.store.artifact(id, &expected, protected).await?;
        }
        let version = self.store.artifact(id, &metadata, protected).await?;
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
        let winner = self
            .store
            .read::<Claim>(&store::claim(id))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?
            .receipt()?;
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
        Ok(self
            .read(id, subject, now, &mut Sources::default())
            .await?
            .map(|(detail, _)| detail))
    }

    /// The summary, and the metadata it was read from when there was one.
    ///
    /// Everything a header says is in that one object. The shards behind it are
    /// verified when somebody reads the page they are on — a fifty-shard result
    /// used to be read whole to answer how many cases it had, which is what
    /// made a catalogue page cost the corpus. `Sources` is what stops one pair
    /// being resolved once per row of a list that is mostly repetitions of it.
    async fn read(
        &self,
        id: &str,
        subject: &str,
        now: i64,
        sources: &mut Sources,
    ) -> Result<Option<(DurableEvaluation, Option<Metadata>)>> {
        let Some(claim) = self.store.read::<Claim>(&store::claim(id)).await? else {
            return Ok(None);
        };
        Ok(Some(
            self.read_receipt(claim.receipt()?, subject, now, sources)
                .await?,
        ))
    }

    async fn read_receipt(
        &self,
        receipt: EvaluationReceipt,
        subject: &str,
        now: i64,
        sources: &mut Sources,
    ) -> Result<(DurableEvaluation, Option<Metadata>)> {
        let id = &receipt.evaluation_id.clone();
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
            return Ok((result, None));
        }
        if result.receipt.expires_at <= now {
            self.retire(id, EvidenceState::Expired).await?;
            result.state = EvidenceState::Expired;
            return Ok((result, None));
        }
        match self
            .read_metadata(&result.receipt, subject, now, sources)
            .await
        {
            Ok(metadata) => {
                result.state = if metadata.status == ResultStatus::Succeeded
                    && metadata.counts.unscored == 0
                    && metadata.counts.failed == 0
                {
                    EvidenceState::Complete
                } else {
                    EvidenceState::Partial
                };
                result.manifest = Some(metadata.manifest.clone());
                result.status = Some(metadata.status);
                result.counts = Some(metadata.counts.clone());
                result.metrics = metadata.metrics.clone();
                return Ok((result, Some(metadata)));
            }
            Err(EvaluationError::Unavailable(state)) => {
                if matches!(state, EvidenceState::Expired | EvidenceState::DeletedSource) {
                    self.retire(id, state).await?;
                }
                result.state = state;
            }
            Err(error) => return Err(error),
        }
        Ok((result, None))
    }

    async fn read_metadata(
        &self,
        receipt: &EvaluationReceipt,
        subject: &str,
        now: i64,
        sources: &mut Sources,
    ) -> Result<Metadata> {
        let (metadata, sealed): (Metadata, bool) = self
            .store
            .opened(&receipt.evaluation_id, &receipt.version)
            .await?;
        if self.protected(&metadata.manifest)? && !sealed {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        let prepared = Evaluation::prepare(metadata.manifest.clone())?;
        require(
            prepared.variant_id() == receipt.variant_id
                && prepared.context_id() == receipt.context_id
                && metadata.manifest.origin.evaluation_id == receipt.evaluation_id,
            "receipt",
            "manifest identity mismatch",
        )?;
        let key = approval_id(prepared.variant_id(), prepared.context_id())?;
        let source = match sources.0.get(&key) {
            Some(remembered) => *remembered,
            None => {
                let answer = self.source_of(&prepared, &metadata.manifest, subject).await;
                let remembered = match answer {
                    Ok(expiry) => Ok(expiry),
                    // Only a verdict about the source is worth remembering. A
                    // store that was briefly unreachable is not one, and would
                    // otherwise condemn every other row of the same pair.
                    Err(EvaluationError::Unavailable(state)) => Err(state),
                    Err(error) => return Err(error),
                };
                sources.0.insert(key, remembered);
                remembered
            }
        };
        let expires_at = source.map_err(EvaluationError::Unavailable)?;
        if expires_at.is_some_and(|expiry| expiry <= now) {
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        Ok(metadata)
    }

    /// Whether the source still admits this pair, and until when.
    async fn source_of(
        &self,
        prepared: &PreparedEvaluation,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> Result<Option<i64>> {
        let approval = self.admitted(prepared, false).await?;
        let source = self.authority.resolve(manifest, subject).await?;
        if approval.is_some_and(|approval| approval.record.bundle_digest != source.bundle_digest) {
            return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
        }
        Ok(source.expires_at)
    }

    async fn read_shard(
        &self,
        id: &str,
        shard: &Shard,
        protected: bool,
    ) -> Result<Vec<EvidenceCase>> {
        let actual: Vec<CaseMeasurement> = self
            .store
            .verified_protected(id, &shard.actual, protected)
            .await?;
        let expected: Vec<serde_json::Value> = self
            .store
            .verified_protected(id, &shard.expected, protected)
            .await?;
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
        let Some((detail, metadata)) = self.read(id, subject, now, &mut Sources::default()).await?
        else {
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
        let Some(metadata) = metadata.filter(|_| {
            matches!(
                detail.state,
                EvidenceState::Complete | EvidenceState::Partial
            )
        }) else {
            return Ok(Some(page));
        };
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
        let protected = self.protected(&metadata.manifest)?;
        let total: usize = metadata.shards.iter().map(|s| s.count).sum();
        require(offset <= total, "cursor", "beyond result")?;
        let end = offset.saturating_add(limit).min(total);
        let mut start = 0;
        for shard in &metadata.shards {
            if start < end && start + shard.count > offset {
                // A damaged shard is this page's state, not an error and never
                // a short page: the header is intact and says how many cases
                // there are, so cases simply missing would read as a result
                // that had fewer of them.
                let rows = match self.read_shard(id, shard, protected).await {
                    Ok(rows) => rows,
                    Err(EvaluationError::Unavailable(state)) => {
                        page.cases.clear();
                        page.next_cursor = None;
                        page.state = state;
                        return Ok(Some(page));
                    }
                    Err(error) => return Err(error),
                };
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
                && let Some(claim) = self.store.read::<Claim>(&entry.key).await?
            {
                ids.insert(claim.id().into());
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
        let mut sources = Sources::default();
        for entry in selected.iter().take(limit) {
            if let Some(Claim::Committed(receipt)) = self.store.read::<Claim>(&entry.key).await? {
                let (detail, _) = self
                    .read_receipt(receipt, subject, now, &mut sources)
                    .await?;
                evaluations.push(detail);
            }
        }
        Ok(DurablePage {
            evaluations,
            next_cursor,
            retention: self.retention().await?,
        })
    }

    /// What the last retention pass did. `None` means none has ever finished.
    pub async fn retention(&self) -> Result<Option<RetentionReport>> {
        self.store.read(store::RETENTION).await
    }

    /// Record one pass, overwriting the last. The worker calls this whether the
    /// pass worked or not — a pass nobody can see is the failure this prevents.
    pub async fn record_sweep(&self, report: &RetentionReport) -> Result<()> {
        Ok(self
            .store
            .0
            .put(store::RETENTION, canonical(report)?)
            .await?)
    }

    /// Forget one published result on request.
    ///
    /// The marker is the retention sweep's own, so a forgotten ID is
    /// permanently unreadable and never falls back to telemetry. `false` means
    /// there was no such result — nothing here invents one to delete.
    pub async fn forget(&self, id: &str) -> Result<bool> {
        if self.store.read::<Claim>(&store::claim(id)).await?.is_none() {
            return Ok(false);
        }
        self.retire(id, EvidenceState::DeletedSource).await?;
        Ok(true)
    }

    async fn retire(&self, id: &str, reason: EvidenceState) -> Result<()> {
        // The durable marker precedes erasure; a crash can leave bytes but
        // cannot make the API disclose them or fall back to old telemetry.
        self.store.create(&store::tombstone(id), &reason).await?;
        self.store.erase(id).await
    }

    /// An operator sweep enforces expiry and owner-wide deletion without a read.
    /// The authority must distinguish global deletion from caller-specific denial.
    ///
    /// Expiry is answered from the receipt's own deadline, which is already the
    /// minimum of the instance's clock and the source's — so the common pass
    /// reads a claim and stops. Only what is still live is resolved, once per
    /// admitted pair rather than once per result: a measurement must not cost
    /// the corpus it is measuring.
    pub async fn sweep(&self, subject: &str, now: i64) -> Result<usize> {
        let mut retired = 0;
        let mut sources = Sources::default();
        for entry in self.store.0.list("evaluations/").await? {
            if !entry.key.ends_with("/claim.json") {
                continue;
            }
            let Some(Claim::Committed(receipt)) = self.store.read::<Claim>(&entry.key).await?
            else {
                continue;
            };
            let id = &receipt.evaluation_id;
            // Already retired. Erase whatever a late write left behind and say
            // nothing: a count that includes what yesterday's sweep did cannot
            // tell a working sweep from one that has been failing for a week.
            if self
                .store
                .read::<EvidenceState>(&store::tombstone(id))
                .await?
                .is_some()
            {
                self.store.erase(id).await?;
                continue;
            }
            if receipt.expires_at <= now {
                self.retire(id, EvidenceState::Expired).await?;
                retired += 1;
                continue;
            }
            // Still live by the clock, so the only question left is whether the
            // owner still has it — one answer per admitted pair, and the pair
            // is on the receipt rather than inside the result.
            let key = approval_id(&receipt.variant_id, &receipt.context_id)?;
            let verdict = match sources.0.get(&key) {
                Some(remembered) => *remembered,
                None => match self
                    .read_metadata(&receipt, subject, now, &mut sources)
                    .await
                {
                    Ok(_) => Ok(None),
                    Err(EvaluationError::Unavailable(state)) => Err(state),
                    Err(error) => return Err(error),
                },
            };
            if let Err(state @ (EvidenceState::Expired | EvidenceState::DeletedSource)) = verdict {
                self.retire(id, state).await?;
                retired += 1;
            }
        }
        Ok(retired)
    }

    /// Collect uncommitted uploads and losing versions without source access.
    /// An immutable claim decides whether an ID can ever publish. We may delete
    /// unclaimed content ONLY after abandonment wins that same atomic gate.
    pub async fn collect_orphans(&self, now: i64) -> Result<usize> {
        let entries = self.store.0.list("evaluations/").await?;
        for entry in &entries {
            if !entry.key.ends_with("/pending.json") {
                continue;
            }
            if let Some(pending) = self.store.read::<Pending>(&entry.key).await? {
                require(
                    entry.key == store::pending(&pending.evaluation_id),
                    "pending",
                    "identity mismatch",
                )?;
                if pending.expires_at <= now {
                    self.store
                        .create(
                            &store::claim(&pending.evaluation_id),
                            &Claim::Abandoned {
                                abandoned: pending.clone(),
                            },
                        )
                        .await?;
                }
            }
        }
        let mut removed = 0;
        // Relist so this pass also sees claims created above. Claims are never
        // replaced/deleted; concurrent collectors therefore choose the same set.
        for entry in self.store.0.list("evaluations/").await? {
            if !entry.key.ends_with("/claim.json") {
                continue;
            }
            let Some(claim) = self.store.read::<Claim>(&entry.key).await? else {
                continue;
            };
            let id = claim.id();
            require(entry.key == store::claim(id), "claim", "identity mismatch")?;
            let mut keep = BTreeSet::new();
            if let Claim::Committed(receipt) = &claim {
                // Retention owns removal of winning bytes. Preserve everything
                // if metadata is unavailable: absence is not evidence of waste.
                let metadata: Metadata = match self.store.verified(id, &receipt.version).await {
                    Ok(metadata) => metadata,
                    Err(EvaluationError::Unavailable(_)) => continue,
                    Err(error) => return Err(error),
                };
                keep.insert(format!("{}{}.json", store::content(id), receipt.version));
                for shard in metadata.shards {
                    keep.insert(format!("{}{}.json", store::content(id), shard.actual));
                    keep.insert(format!("{}{}.json", store::content(id), shard.expected));
                }
            }
            for artifact in self.store.0.list(&store::content(id)).await? {
                if !keep.contains(&artifact.key) {
                    self.store.0.delete(&artifact.key).await?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
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
