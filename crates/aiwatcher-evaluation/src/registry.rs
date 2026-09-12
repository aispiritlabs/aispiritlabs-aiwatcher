//! Durable publication, access and retention behind the domain facade.
use crate::{
    Aggregation, Approval, ApprovalRecord, CaseDiffPage, CaseMeasurement, CasePage, Comparability,
    DiffQuery, DurableEvaluation, DurablePage, Evaluation, EvaluationError, EvaluationManifest,
    EvaluationReceipt, EvidenceCase, EvidenceState, PreparedEvaluation, PublishEvaluation, Result,
    ResultCounts, ResultStatus, RetentionReport, Withdrawal, approval_id, canonical,
    comparison::{comparability, diff_case, merged},
    require,
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

/// One catalogue row, as published.
///
/// `retired` is written *before* the tombstone, so the only half of a crash a
/// reader can see is the half that hides a result — never the half that shows
/// a retired one as live. It is what lets a catalogue page skip the tombstone
/// read a detail still makes.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexEntry {
    receipt: EvaluationReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retired: Option<EvidenceState>,
}

/// A row with nothing behind it: the receipt, and the state that is why.
fn row(receipt: EvaluationReceipt, state: EvidenceState) -> DurableEvaluation {
    DurableEvaluation {
        receipt,
        state,
        manifest: None,
        status: None,
        counts: None,
        metrics: BTreeMap::new(),
    }
}

/// What one collection pass removed, and what it found missing.
///
/// The gaps are a by-product rather than a second pass: deleting what a result
/// does not hold means listing what it does, beside the header that says what
/// it should. A summary answers from that header alone, so this is the only
/// place that learns a complete-looking result has lost its shards.
#[derive(Clone, Debug, Default)]
pub struct CollectionReport {
    pub removed: usize,
    /// The first few, so one report stays a small object.
    pub damaged: Vec<String>,
    /// All of them.
    pub damaged_count: usize,
}
impl CollectionReport {
    fn damage(&mut self, id: &str) {
        self.damaged_count += 1;
        if self.damaged.len() < DAMAGED_SAMPLE {
            self.damaged.push(id.into());
        }
    }
}
const DAMAGED_SAMPLE: usize = 50;

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

/// How many cases one request may walk to fill a page of the diff.
///
/// Without it, a pair whose ten thousand cases hold twelve regressions would
/// be one request reading every shard of both sides. A page ends at whichever
/// comes first — the rows it was asked for, or this — and says so with a
/// cursor.
const DIFF_SCAN: usize = 2_000;

/// One side of a case diff, read a shard at a time.
///
/// Both sides were sorted by `case_id` before they were sharded, so the diff
/// is a merge of two ordered streams and its cursor is a pair of offsets — the
/// same number a case page's own cursor holds, once per side.
struct Side {
    id: String,
    protected: bool,
    shards: Vec<Shard>,
    offset: usize,
    /// The shard `offset` falls in once it has been read: its index, the
    /// offset of its first case, and its measurements.
    loaded: Option<(usize, usize, Vec<CaseMeasurement>)>,
}

impl Side {
    fn new(id: &str, protected: bool, shards: Vec<Shard>, offset: usize) -> Self {
        Self {
            id: id.to_owned(),
            protected,
            shards,
            offset,
            loaded: None,
        }
    }

    fn total(&self) -> usize {
        self.shards.iter().map(|shard| shard.count).sum()
    }

    fn done(&self) -> bool {
        self.offset >= self.total()
    }

    /// Read the shard `offset` falls in, unless it is already in hand.
    async fn load(&mut self, registry: &Registry) -> Result<()> {
        let mut start = 0;
        for (index, shard) in self.shards.iter().enumerate() {
            if self.offset < start + shard.count {
                if self.loaded.as_ref().is_none_or(|(held, ..)| *held != index) {
                    let rows = registry
                        .read_measurements(&self.id, shard, self.protected)
                        .await?;
                    self.loaded = Some((index, start, rows));
                }
                return Ok(());
            }
            start += shard.count;
        }
        self.loaded = None;
        Ok(())
    }

    fn peek(&self) -> Option<&CaseMeasurement> {
        let (_, start, rows) = self.loaded.as_ref()?;
        rows.get(self.offset.saturating_sub(*start))
    }

    /// The case in hand, and the cursor the case route would want for it.
    fn here(&self, version: &str) -> Option<(&CaseMeasurement, String)> {
        Some((self.peek()?, format!("{version}:{}", self.offset)))
    }
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
            self.retire(id, Some(&winner), EvidenceState::Expired)
                .await?;
            return Err(EvaluationError::Unavailable(EvidenceState::Expired));
        }
        // The catalogue row, after the gate that decided this ID may publish.
        // A process that stops here leaves a result the collection pass will
        // index: the claim is the truth and this is only the order.
        self.index(&winner, None).await?;
        Ok(winner)
    }

    /// Write this result's catalogue row, published or retired.
    async fn index(
        &self,
        receipt: &EvaluationReceipt,
        retired: Option<EvidenceState>,
    ) -> Result<()> {
        let key = store::indexed(receipt.committed_at, &receipt.evaluation_id);
        let entry = IndexEntry {
            receipt: receipt.clone(),
            retired,
        };
        Ok(self.store.0.put(&key, canonical(&entry)?).await?)
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

    /// One result against another named one, as two headers.
    ///
    /// Explicit: a baseline nobody chose is a baseline nobody checked, and the
    /// automatic one on the folded half exists because a log fold has no other
    /// way to offer a pair. Here the catalogue answers that — every result in
    /// this one's context is a candidate, and which of them is the baseline is
    /// a decision. Both sides are read through one `Sources`, so a pair
    /// admitted once is resolved once.
    pub async fn compare(
        &self,
        id: &str,
        baseline_id: &str,
        subject: &str,
        now: i64,
    ) -> Result<Option<crate::EvidenceComparison>> {
        let mut sources = Sources::default();
        let Some((current, _)) = self.read(id, subject, now, &mut sources).await? else {
            return Ok(None);
        };
        let Some((baseline, _)) = self.read(baseline_id, subject, now, &mut sources).await? else {
            return Ok(None);
        };
        Ok(Some(crate::comparison::compare(current, baseline)))
    }

    /// Which cases moved, between one result and another named one.
    ///
    /// The comparison beside this reads two headers and costs what two
    /// summaries cost. This is the other question and is a full read of both
    /// sides, which is why it is paged, narrowed by the server rather than by
    /// a browser, and bounded in how far one request may walk. A pair the server calls
    /// incompatible gets no rows — a diff over two results it has just
    /// declined to subtract would be a second answer to whether it may — and
    /// an unverified one keeps them, because the cases that failed are exactly
    /// what a reader opens this to find.
    pub async fn compare_cases(
        &self,
        id: &str,
        baseline_id: &str,
        query: DiffQuery<'_>,
        subject: &str,
        now: i64,
    ) -> Result<Option<CaseDiffPage>> {
        let mut sources = Sources::default();
        let Some((current, current_metadata)) = self.read(id, subject, now, &mut sources).await?
        else {
            return Ok(None);
        };
        let Some((baseline, baseline_metadata)) =
            self.read(baseline_id, subject, now, &mut sources).await?
        else {
            return Ok(None);
        };
        let (verdict, reasons) = comparability(&current, &baseline);
        let mut page = CaseDiffPage {
            comparability: verdict,
            reasons,
            cases: Vec::new(),
            next_cursor: None,
            state: merged(current.state, baseline.state),
        };
        let (Some(current_metadata), Some(baseline_metadata)) =
            (current_metadata, baseline_metadata)
        else {
            return Ok(Some(page));
        };
        // Only a pair that may not be subtracted at all. `Unverified` keeps
        // its rows, and this is the one place the two halves of a comparison
        // part company: the header withholds its delta there because an
        // aggregate over cases that failed or went unscored is a number about
        // a denominator nobody agreed on, while a case's own delta subtracts
        // two measurements of that same case and is sound whatever happened to
        // the others. Cases that failed are what somebody opens this to read.
        if verdict == Comparability::Incompatible {
            return Ok(Some(page));
        }
        let limit = query.limit.unwrap_or(self.config.page_size);
        require(limit > 0 && limit <= 200, "limit", "must be 1..200")?;
        let (left, right) = match query.cursor {
            None => (0, 0),
            Some(cursor) => match *cursor.split(':').collect::<Vec<_>>().as_slice() {
                [version, left, baseline_version, right]
                    if version == current.receipt.version
                        && baseline_version == baseline.receipt.version =>
                {
                    left.parse::<usize>().ok().zip(right.parse::<usize>().ok())
                }
                _ => None,
            }
            .ok_or_else(|| EvaluationError::Invalid {
                field: "cursor".into(),
                reason: "must belong to both immutable results".into(),
            })?,
        };
        // Both sides share the context — that equality is what made them
        // comparable — so which of the two declares the metrics is arbitrary.
        let declared = current_metadata.manifest.context.metrics.clone();
        let mut sides = (
            Side::new(
                id,
                self.protected(&current_metadata.manifest)?,
                current_metadata.shards,
                left,
            ),
            Side::new(
                baseline_id,
                self.protected(&baseline_metadata.manifest)?,
                baseline_metadata.shards,
                right,
            ),
        );
        require(
            left <= sides.0.total() && right <= sides.1.total(),
            "cursor",
            "beyond result",
        )?;

        let mut walked = 0;
        while page.cases.len() < limit && walked < DIFF_SCAN {
            // A damaged shard is this page's state, not an error and never a
            // short page: the headers are intact and say how many cases there
            // are, so rows simply missing would read as cases that agreed.
            for side in [&mut sides.0, &mut sides.1] {
                match side.load(self).await {
                    Ok(()) => {}
                    Err(EvaluationError::Unavailable(state)) => {
                        page.cases.clear();
                        page.next_cursor = None;
                        page.state = state;
                        return Ok(Some(page));
                    }
                    Err(error) => return Err(error),
                }
            }
            let order = match (sides.0.peek(), sides.1.peek()) {
                (None, None) => break,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(now), Some(then)) => now.case_id.cmp(&then.case_id),
            };
            // The cursor each row carries is this walk's own position, in the
            // case route's words — so a reader who wants one case's answers
            // asks the route that already serves them.
            let delta = diff_case(
                &declared,
                (order != std::cmp::Ordering::Greater)
                    .then(|| sides.0.here(&current.receipt.version))
                    .flatten(),
                (order != std::cmp::Ordering::Less)
                    .then(|| sides.1.here(&baseline.receipt.version))
                    .flatten(),
            );
            if order != std::cmp::Ordering::Greater {
                sides.0.offset += 1;
            }
            if order != std::cmp::Ordering::Less {
                sides.1.offset += 1;
            }
            walked += 1;
            if query.only.is_none_or(|filter| filter.keeps(delta.change)) {
                page.cases.push(delta);
            }
        }
        if !sides.0.done() || !sides.1.done() {
            page.next_cursor = Some(format!(
                "{}:{}:{}:{}",
                current.receipt.version, sides.0.offset, baseline.receipt.version, sides.1.offset
            ));
        }
        Ok(Some(page))
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
        let id = receipt.evaluation_id.clone();
        if let Some(state) = self.store.read(&store::tombstone(&id)).await? {
            self.store.erase(&id).await?;
            return Ok((row(receipt, state), None));
        }
        self.read_committed(receipt, subject, now, sources).await
    }

    /// The same row for a receipt already known not to be retired — which is
    /// what a catalogue entry knows, and a detail read has to ask.
    async fn read_committed(
        &self,
        receipt: EvaluationReceipt,
        subject: &str,
        now: i64,
        sources: &mut Sources,
    ) -> Result<(DurableEvaluation, Option<Metadata>)> {
        let id = &receipt.evaluation_id.clone();
        let mut result = row(receipt, EvidenceState::Complete);
        if result.receipt.expires_at <= now {
            self.retire(id, Some(&result.receipt), EvidenceState::Expired)
                .await?;
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
                    self.retire(id, Some(&result.receipt), state).await?;
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

    /// One shard's measurements, without the expectations beside them.
    ///
    /// A diff reads this half only: the expectations are the cohort, which a
    /// comparable pair shares by construction, so subtracting two results
    /// costs half of what reading either one's cases does.
    async fn read_measurements(
        &self,
        id: &str,
        shard: &Shard,
        protected: bool,
    ) -> Result<Vec<CaseMeasurement>> {
        let actual: Vec<CaseMeasurement> = self
            .store
            .verified_protected(id, &shard.actual, protected)
            .await?;
        if actual.len() != shard.count {
            return Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(actual)
    }

    async fn read_shard(
        &self,
        id: &str,
        shard: &Shard,
        protected: bool,
    ) -> Result<Vec<EvidenceCase>> {
        let actual = self.read_measurements(id, shard, protected).await?;
        let expected: Vec<serde_json::Value> = self
            .store
            .verified_protected(id, &shard.expected, protected)
            .await?;
        if expected.len() != shard.count {
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

    /// The catalogue, newest first, optionally narrowed to a period.
    ///
    /// Narrowed to one `context_id`, every row it returns is a legitimate
    /// baseline for every other — which is the whole of the comparability
    /// question for published evidence, answered by the server rather than by
    /// a browser comparing two strings. The filter walks the index rather than
    /// a second one keyed by context: a page therefore costs the rows it
    /// passed over as well as the ones it carries, and a cursor on a filtered
    /// page promises another *entry* rather than another match.
    ///
    /// It reads the index rather than the results: a result's own key is the
    /// hash of its ID, so ordering by anything but that hash meant listing
    /// every object under `evaluations/` — content included, four keys per
    /// row — and sorting what came back. The index is one key per published
    /// result, in published order, and a row that says it is retired needs
    /// nothing behind it. A window is therefore a bound on the key.
    pub async fn list(
        &self,
        cursor: Option<&str>,
        limit: usize,
        window_seconds: Option<i64>,
        context_id: Option<&str>,
        subject: &str,
        now: i64,
    ) -> Result<DurablePage> {
        require(limit > 0 && limit <= 200, "limit", "must be 1..200")?;
        let mut entries = self.store.0.list(store::INDEX).await?;
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        if let Some(window) = window_seconds {
            require(window > 0, "window_seconds", "must be positive")?;
            let since = store::indexed_since(now.saturating_sub(window));
            entries.retain(|entry| entry.key <= since);
        }
        if let Some(cursor) = cursor {
            require(
                entries.iter().any(|e| e.key == cursor),
                "cursor",
                "unknown discovery cursor",
            )?;
        }
        let mut evaluations = Vec::new();
        let mut next_cursor = None;
        let mut last = None;
        let mut sources = Sources::default();
        for entry in entries
            .iter()
            .filter(|e| cursor.is_none_or(|c| e.key.as_str() > c))
        {
            if evaluations.len() == limit {
                next_cursor = last;
                break;
            }
            let Some(indexed) = self.store.read::<IndexEntry>(&entry.key).await? else {
                continue;
            };
            if context_id.is_some_and(|wanted| indexed.receipt.context_id != wanted) {
                continue;
            }
            match indexed.retired {
                Some(state) => evaluations.push(row(indexed.receipt, state)),
                None => {
                    let (detail, _) = self
                        .read_committed(indexed.receipt, subject, now, &mut sources)
                        .await?;
                    evaluations.push(detail);
                }
            }
            last = Some(entry.key.clone());
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
        let Some(claim) = self.store.read::<Claim>(&store::claim(id)).await? else {
            return Ok(false);
        };
        let published = match &claim {
            Claim::Committed(receipt) => Some(receipt),
            // Nothing was ever published under this ID, so there is no row.
            Claim::Abandoned { .. } => None,
        };
        self.retire(id, published, EvidenceState::DeletedSource)
            .await?;
        Ok(true)
    }

    async fn retire(
        &self,
        id: &str,
        published: Option<&EvaluationReceipt>,
        reason: EvidenceState,
    ) -> Result<()> {
        // The catalogue row is marked before the tombstone, and the tombstone
        // precedes erasure. Every window between the three shows less than the
        // truth rather than more: a crash can leave bytes but cannot make the
        // API disclose them or fall back to old telemetry.
        if let Some(receipt) = published {
            self.index(receipt, Some(reason)).await?;
        }
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
                self.retire(id, Some(&receipt), EvidenceState::Expired)
                    .await?;
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
                self.retire(id, Some(&receipt), state).await?;
                retired += 1;
            }
        }
        Ok(retired)
    }

    /// Collect uncommitted uploads and losing versions without source access.
    /// An immutable claim decides whether an ID can ever publish. We may delete
    /// unclaimed content ONLY after abandonment wins that same atomic gate.
    ///
    /// It also reports what it found missing, because it cannot do its own job
    /// without finding out: deleting what a result does not hold means listing
    /// what it does, beside the header that says what it should.
    pub async fn collect_orphans(&self, now: i64) -> Result<CollectionReport> {
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
        let mut report = CollectionReport::default();
        // What the catalogue already holds, once for the pass. A row is written
        // by the publication that won the gate, so a missing one is a process
        // that stopped between the two writes — or a result published before
        // this instance kept a catalogue at all.
        let catalogued: BTreeSet<String> = self
            .store
            .0
            .list(store::INDEX)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .collect();
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
                if !catalogued.contains(&store::indexed(receipt.committed_at, id)) {
                    // The tombstone is read here only, where a row has to be
                    // written anyway and a wrong one would show a retired
                    // result as live.
                    let retired = self.store.read(&store::tombstone(id)).await?;
                    self.index(receipt, retired).await?;
                }
                // Retention owns removal of winning bytes. Preserve everything
                // if metadata is unavailable: absence is not evidence of waste.
                let metadata: Metadata = match self.store.verified(id, &receipt.version).await {
                    Ok(metadata) => metadata,
                    Err(EvaluationError::Unavailable(_)) => {
                        // A retired result has no header by design. The
                        // tombstone is read only here, where the answer is
                        // already unusual: one get per broken result rather
                        // than one per result.
                        if self
                            .store
                            .read::<EvidenceState>(&store::tombstone(id))
                            .await?
                            .is_none()
                        {
                            report.damage(id);
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                keep.insert(format!("{}{}.json", store::content(id), receipt.version));
                for shard in metadata.shards {
                    keep.insert(format!("{}{}.json", store::content(id), shard.actual));
                    keep.insert(format!("{}{}.json", store::content(id), shard.expected));
                }
            }
            let mut found = 0;
            for artifact in self.store.0.list(&store::content(id)).await? {
                if keep.contains(&artifact.key) {
                    found += 1;
                } else {
                    self.store.0.delete(&artifact.key).await?;
                    report.removed += 1;
                }
            }
            if found != keep.len() {
                report.damage(id);
            }
        }
        Ok(report)
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

/// Rubrics and the judgements made under them.
///
/// The same door as the evidence, because the owner is the same and a
/// judgement about a case is meaningless beside a result somebody else holds.
/// What is not the same is the clock: a rubric and an assessment are authored,
/// so nothing here expires with a receipt.
impl Registry {
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the form is unusable. Publishing the
    /// same words twice answers with the version that is already there.
    pub async fn publish_rubric(
        &self,
        rubric: &crate::Rubric,
        published_by: &str,
        now: i64,
    ) -> Result<crate::RubricVersion> {
        crate::assessment::publish_rubric(&self.store, rubric, published_by, now).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn rubrics(&self) -> Result<Vec<crate::RubricHead>> {
        crate::assessment::rubrics(&self.store).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn rubric(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<crate::RubricVersion>> {
        text(name, "rubric")?;
        let version = match version {
            Some(version) => version.to_owned(),
            None => match crate::assessment::rubric_head(&self.store, name).await? {
                Some(head) => head.version,
                None => return Ok(None),
            },
        };
        crate::assessment::rubric_version(&self.store, name, &version).await
    }

    /// Publish a scorecard. Idempotent by content: the same measurements
    /// answer with the version that is already there, and the head moves to it
    /// either way.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when a scorer, a metric name or a pointer
    /// is unusable, and [`EvaluationError::Storage`] when the store cannot be
    /// reached.
    pub async fn publish_scorecard(
        &self,
        scorecard: &crate::Scorecard,
        published_by: &str,
        now: i64,
    ) -> Result<crate::ScorecardVersion> {
        crate::scorecard::publish(&self.store, scorecard, published_by, now).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scorecards(&self) -> Result<Vec<crate::ScorecardHead>> {
        crate::scorecard::all(&self.store).await
    }

    /// One scorecard, at the version asked for or at the head.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scorecard(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<crate::ScorecardVersion>> {
        text(name, "scorecard")?;
        let version = match version {
            Some(version) => version.to_owned(),
            None => match crate::scorecard::head(&self.store, name).await? {
                Some(head) => head.version,
                None => return Ok(None),
            },
        };
        crate::scorecard::version(&self.store, name, &version).await
    }

    /// Write down what a run of saved answers will measure.
    ///
    /// Addressed by its content, so declaring the same intention twice is the
    /// same document and a plan that names it names a digest. Idempotent for
    /// the same reason a prompt version is: the words decide the key.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when a reference or a pinned artifact is
    /// unusable, and [`EvaluationError::Storage`] when the store cannot be
    /// reached.
    pub async fn declare_scoring_run(
        &self,
        run: &crate::ScoringRun,
        declared_by: &str,
        now: i64,
    ) -> Result<crate::DeclaredRun> {
        crate::scoring::declare(&self.store, run, declared_by, now).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scoring_run(&self, id: &str) -> Result<Option<crate::DeclaredRun>> {
        text(id, "run")?;
        crate::scoring::declared(&self.store, id).await
    }

    /// Record one judgement. The caller is who filed it, always.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the target, the rubric or the answer
    /// is unusable, and [`EvaluationError::Contested`] when another revision
    /// kept winning the write.
    pub async fn assess(
        &self,
        request: &crate::AssessmentRequest,
        recorded_by: &str,
        now: i64,
    ) -> Result<crate::Assessment> {
        crate::assessment::assess(&self.store, request, recorded_by, now).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the target is unusable.
    pub async fn assessments(
        &self,
        target: &crate::AssessmentTarget,
    ) -> Result<crate::AssessmentPage> {
        crate::assessment::assessments(&self.store, target).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when either ID or the page is unusable.
    pub async fn assessment_history(
        &self,
        target_id: &str,
        standing_id: &str,
        before: Option<u32>,
        limit: Option<u32>,
    ) -> Result<crate::AssessmentHistory> {
        crate::assessment::history(&self.store, target_id, standing_id, before, limit).await
    }
}
