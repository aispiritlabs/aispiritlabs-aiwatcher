//! Durable publication, access and retention behind the domain facade.
use crate::{
    Aggregation, Approval, ApprovalRecord, CaseDiffPage, CaseMeasurement, CasePage, Comparability,
    DiffQuery, DurableEvaluation, DurablePage, Evaluation, EvaluationError, EvaluationManifest,
    EvaluationReceipt, EvidenceCase, EvidenceState, PinnedMember, PreparedEvaluation,
    PublishEvaluation, Result, ResultCounts, ResultStatus, RetentionReport, Withdrawal,
    approval_id, canonical,
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

    /// The cohort files for the first `limit` cases of a dataset version's
    /// split, derived from its owner the way [`Self::resolve`] derives the
    /// cases it checks a cohort against — one derivation, so the files a
    /// declaration pins and the cases admission reads cannot disagree.
    ///
    /// The default derives nothing: an adapter that owns no dataset has no
    /// cases to hand out, and says so rather than inventing a shape.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] from the default, and whatever the owner
    /// refused otherwise.
    async fn derive_cohort(
        &self,
        request: &crate::CohortRequest,
        _subject: &str,
    ) -> Result<crate::CohortFiles> {
        Err(EvaluationError::Invalid {
            field: "cohort.dataset".into(),
            reason: format!(
                "this deployment derives no cohort from a {:?} dataset",
                request.dataset.kind
            ),
        })
    }
}

#[derive(Debug, Default)]
pub struct SourceEvidence {
    pub expected: BTreeMap<String, serde_json::Value>,
    /// What each case was asked, where the owner keeps it. Read only by a
    /// judge a card points at it, and never published: a result's shards hold
    /// what was answered and what was expected. An adapter whose cases have no
    /// input it can hand over leaves it empty, and a judge pointed at one is
    /// told so per case.
    pub inputs: BTreeMap<String, serde_json::Value>,
    pub expires_at: Option<i64>,
    /// What this adapter admitted beyond the manifest's own pinned digests.
    /// An approval records it, so bytes cannot change under an admitted pair.
    pub bundle_digest: Option<String>,
    /// The digest this adapter recorded for the same bundle before it digested
    /// only what the bundle adds (see [`crate::bundle_digest`]). An approval
    /// written then holds it, and still admits while those bytes stand; a new
    /// approval records `bundle_digest`.
    pub earlier_bundle_digest: Option<String>,
}

/// How many of the newest results an experiment reading walks.
pub const EXPERIMENT_WALK: usize = 1_000;

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
        reproducible: true,
        judge: None,
        external: None,
        usage: None,
        traces: None,
    }
}

/// Whether the bytes an approval admitted are the bytes the adapter holds now,
/// under the digest the approval was recorded with.
fn same_bundle(record: &ApprovalRecord, source: &SourceEvidence) -> bool {
    record.bundle_digest == source.bundle_digest
        || (record.bundle_digest.is_some() && record.bundle_digest == source.earlier_bundle_digest)
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
    /// Beside the numbers it qualifies, in the one object a summary reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    judge: Option<crate::JudgeReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    external: Option<crate::ExternalReport>,
    /// Derived from the cases at publication; absent when none reported any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<crate::ResultUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    traces: Option<crate::GenerationTrace>,
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
        self.admit_scoring(&manifest.context).await?;
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
            bundle_digest: source.bundle_digest.clone(),
            approved_by: subject.into(),
            approved_at: now,
        };
        self.store.create(&store::approval(&id), &record).await?;
        let approval = self
            .approval(&id)
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        if !same_bundle(&approval.record, &source) {
            return Err(EvaluationError::AdmittedOtherBytes(id));
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

    /// Admit every variant of one experiment measured in one context.
    ///
    /// `template` is any declaration of the line: its context is what is
    /// admitted, its variant's `experiment_id` which variants, and nothing
    /// else of the variant is read. Checked as a pair's context is — the card
    /// and scorer this deployment measures with — and refused for evidence a
    /// producer measured, whose numbers nothing here computed.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] naming what cannot be a line.
    pub async fn admit_line(
        &self,
        template: &EvaluationManifest,
        subject: &str,
        now: i64,
    ) -> Result<crate::ApprovalLine> {
        text(subject, "admitted_by")?;
        let prepared = Evaluation::prepare(template.clone())?;
        require(
            template.context.scored_here(),
            "context.scorer",
            "a line admits evidence this deployment measures; a producer's evidence is admitted \
             pair by pair",
        )?;
        // A pair over the archive is admitted by an admin who may read what its
        // run reads; a line would let every later variant's start stand in for
        // that person.
        require(
            template.context.dataset.kind != crate::DatasetKind::Conversations
                && template
                    .context
                    .external_calibration
                    .as_ref()
                    .is_none_or(|pin| !pin.reads_archive)
                && template
                    .context
                    .judge
                    .as_ref()
                    .is_none_or(|judge| !judge.reads_archive),
            "context.dataset",
            "reads the conversation archive, whose pairs an admin admits one at a time",
        )?;
        self.admit_scoring(&template.context).await?;
        let id = crate::line_id(prepared.context_id(), &template.variant.experiment_id)?;
        if let Some(line) = self.line(&id).await? {
            require(
                line.admits(),
                "line",
                "was withdrawn; a withdrawn line admits nothing again",
            )?;
        }
        let record = crate::ApprovalLineRecord {
            line_id: id.clone(),
            context_id: prepared.context_id().into(),
            experiment_id: template.variant.experiment_id.clone(),
            admitted_by: subject.into(),
            admitted_at: now,
        };
        self.store.create(&store::line(&id), &record).await?;
        self.line(&id)
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))
    }

    /// Stop a line admitting further variants.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn withdraw_line(
        &self,
        line_id: &str,
        subject: &str,
        now: i64,
    ) -> Result<Option<crate::ApprovalLine>> {
        text(subject, "withdrawn_by")?;
        if self.line(line_id).await?.is_none() {
            return Ok(None);
        }
        self.store
            .create(
                &store::line_withdrawal(line_id),
                &Withdrawal {
                    withdrawn_by: subject.into(),
                    withdrawn_at: now,
                },
            )
            .await?;
        self.line(line_id).await
    }

    /// Every line, withdrawn ones included.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn lines(&self) -> Result<Vec<crate::ApprovalLine>> {
        let mut entries = self.store.0.list(store::LINES).await?;
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let mut lines = Vec::new();
        for entry in entries
            .iter()
            .filter(|entry| entry.key.ends_with("/record.json"))
        {
            if let Some(record) = self
                .store
                .read::<crate::ApprovalLineRecord>(&entry.key)
                .await?
                && entry.key == store::line(&record.line_id)
                && let Some(line) = self.line(&record.line_id).await?
            {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    async fn line(&self, id: &str) -> Result<Option<crate::ApprovalLine>> {
        let Some(record) = self
            .store
            .read::<crate::ApprovalLineRecord>(&store::line(id))
            .await?
        else {
            return Ok(None);
        };
        require(record.line_id == id, "line", "identity mismatch")?;
        Ok(Some(crate::ApprovalLine {
            record,
            withdrawn: self.store.read(&store::line_withdrawal(id)).await?,
        }))
    }

    /// Keep one of a variant's pinned files by its content, for a line to
    /// admit the variant from.
    ///
    /// Any bytes: a variant's code may be an archive or a note naming a
    /// commit, and a pin is a pin. What they are is the digest, computed here.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for a name that is not one path segment or
    /// bytes over the instance's limit.
    pub async fn stage_variant_artifact(
        &self,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<aiwatcher_core::ArtifactRef> {
        text(name, "artifact.name")?;
        require(
            !name.contains('/') && !name.contains('\\') && name != "manifest.json",
            "artifact.name",
            "must be one path segment, and not the bundle's manifest",
        )?;
        require(
            bytes.len() <= self.config.max_bytes,
            "artifact",
            "exceeds instance byte limit",
        )?;
        let digest = store::hash(&bytes);
        let size = bytes.len() as u64;
        self.store
            .0
            .create(&store::variant_artifact(&digest), bytes)
            .await?;
        Ok(aiwatcher_core::ArtifactRef {
            name: name.to_owned(),
            uri: format!("evaluation://variant-artifacts/{digest}"),
            digest,
            size_bytes: Some(size),
            content_type: String::new(),
            kind: aiwatcher_core::ArtifactKind::Blob,
            schema_ref: None,
        })
    }

    /// Admit a pair through the line its context and experiment belong to,
    /// when nothing admitted it yet and a live line does.
    ///
    /// The bundle an operator would have staged is built from what a line
    /// already decided and what the pipeline sent: the declaration's own
    /// manifest, and each file its variant pins from the bytes kept under
    /// that digest. Then the pair is approved the way an operator approves
    /// one — resolved by the adapter from those bytes — naming the line and
    /// who started it. A variant naming a model or a workflow implies members
    /// beyond its pins, which the adapter names ([`crate::ApprovalBundles::pinned_members`]):
    /// a model's package, derived from its owner, and the weights and workflow
    /// declaration the pipeline sent by digest.
    ///
    /// `None` when no live line covers the pair.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] naming a pinned file nothing staged, and
    /// whatever approving refuses.
    pub async fn admit_through_line(
        &self,
        manifest: &EvaluationManifest,
        started_by: &str,
        bundles: &dyn crate::ApprovalBundles,
        now: i64,
    ) -> Result<Option<Approval>> {
        let prepared = Evaluation::prepare(manifest.clone())?;
        let Some(line) = self
            .line(&crate::line_id(
                prepared.context_id(),
                &manifest.variant.experiment_id,
            )?)
            .await?
            .filter(crate::ApprovalLine::admits)
        else {
            return Ok(None);
        };
        // What the model and workflow references imply, as their owners here
        // declare it, before anything is staged: a variant naming either that
        // this adapter derives nothing for is one a line cannot admit.
        let mut sent = BTreeMap::new();
        let mut implied = bundles.pinned_members(&manifest.variant, &sent).await?;
        require(
            (manifest.variant.model.is_none() && manifest.variant.workflow.is_none())
                || !implied.is_empty(),
            "variant",
            "names a model or a workflow, and this deployment derives no bundle members for \
             either; admit this pair by hand",
        )?;
        let id = approval_id(prepared.variant_id(), prepared.context_id())?;
        bundles
            .stage(&id, "manifest.json", canonical(manifest)?)
            .await?;
        let pinned = [
            ("variant.code", Some(&manifest.variant.code)),
            (
                "variant.generation_config",
                Some(&manifest.variant.generation_config),
            ),
            (
                "variant.response_schema",
                manifest.variant.response_schema.as_ref(),
            ),
            ("variant.tools", manifest.variant.tools.as_ref()),
        ];
        for (field, artifact) in pinned {
            let Some(artifact) = artifact else { continue };
            let bytes = self
                .store
                .0
                .get(&store::variant_artifact(&artifact.digest))
                .await?
                .filter(|bytes| store::hash(bytes) == artifact.digest)
                .ok_or_else(|| EvaluationError::Invalid {
                    field: field.into(),
                    reason: format!(
                        "no bytes were staged under {}; send them to \
                         PUT /api/v1/evaluation-variant-artifacts/{} first",
                        artifact.digest, artifact.name
                    ),
                })?;
            bundles.stage(&id, &artifact.name, bytes).await?;
        }
        // A member may name more once it is staged — a package nobody here
        // registered lists its artifacts only inside itself — so ask again
        // until nothing new is named. A package names no package, so the
        // second answer is the last one that can add anything.
        for _ in 0..3 {
            let fresh: Vec<PinnedMember> = implied
                .into_iter()
                .filter(|member| !sent.contains_key(&member.name))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for member in fresh {
                let bytes = self.implied_bytes(&member).await?;
                bundles.stage(&id, &member.name, bytes.clone()).await?;
                sent.insert(member.name, bytes);
            }
            implied = bundles.pinned_members(&manifest.variant, &sent).await?;
        }
        let approved_by = format!(
            "line {} ({}), started by {started_by}",
            line.record.line_id, line.record.admitted_by
        );
        self.approve(manifest, &approved_by, now).await.map(Some)
    }

    /// The bytes behind one member a variant implies: what its owner derived,
    /// or what a pipeline sent under its digest.
    async fn implied_bytes(&self, member: &PinnedMember) -> Result<Vec<u8>> {
        require(
            member.digest.len() == 64
                && member
                    .digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "variant",
            &format!(
                "{} is pinned by {}, which is not a lowercase sha256 digest a line can find \
                 bytes under",
                member.name, member.digest
            ),
        )?;
        if let Some(derived) = &member.bytes {
            return Ok(derived.clone());
        }
        self.store
            .0
            .get(&store::variant_artifact(&member.digest))
            .await?
            .filter(|bytes| {
                store::hash(bytes) == member.digest
                    && member
                        .size_bytes
                        .is_none_or(|size| size == bytes.len() as u64)
            })
            .ok_or_else(|| EvaluationError::Invalid {
                field: "variant".into(),
                reason: format!(
                    "{} needs {} ({}), and no bytes were staged under that digest; send them \
                     to PUT /api/v1/evaluation-variant-artifacts/{} first",
                    member.name,
                    member.digest,
                    member
                        .size_bytes
                        .map_or_else(|| "any size".to_owned(), |size| format!("{size} bytes")),
                    member.name.rsplit('/').next().unwrap_or(&member.name),
                ),
            })
    }

    /// Propose a case for a dataset; answers the review and whether this
    /// proposal started it rather than landing on one already under way.
    ///
    /// A proposal without a question names a case of a published result and
    /// where it sits there, and its words are read from that result: what the
    /// cohort asked, and what the variant answered.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] naming the field that is not one.
    pub async fn propose_case(
        &self,
        proposal: &crate::CaseProposal,
        proposed_by: &str,
        now: i64,
    ) -> Result<(crate::ReviewItem, bool)> {
        use crate::review::Words;
        // Landing on a review under way reads nothing: the words it has stand.
        if let Some(existing) =
            crate::review::existing(&self.store, &proposal.dataset, &proposal.target).await?
        {
            return Ok((existing, false));
        }
        let words = match (&proposal.question, proposal.content) {
            (Some(question), Some(content)) => Words {
                question: question.clone(),
                answer: proposal.answer.clone(),
                content,
            },
            (Some(_), None) => {
                return Err(EvaluationError::Invalid {
                    field: "content".into(),
                    reason: "says whose words the question is: `written` by a reviewer, or \
                             `observed` and copied from somebody using the application"
                        .into(),
                });
            }
            (None, _) => self.case_words(proposal, proposed_by, now).await?,
        };
        crate::review::propose(&self.store, proposal, words, proposed_by, now).await
    }

    /// The question and answer of the result case a proposal names, read at
    /// the position it names.
    async fn case_words(
        &self,
        proposal: &crate::CaseProposal,
        subject: &str,
        now: i64,
    ) -> Result<crate::review::Words> {
        let crate::AssessmentTarget::Case {
            evaluation_id,
            case_id,
            repetition_id,
        } = &proposal.target
        else {
            return Err(EvaluationError::Invalid {
                field: "question".into(),
                reason: "a trace, a span or a session holds no words here — the Collector keeps \
                         prompts and completions off them — so write the question"
                    .into(),
            });
        };
        let at = proposal
            .at
            .as_deref()
            .ok_or_else(|| EvaluationError::Invalid {
                field: "at".into(),
                reason: "names where the case sits in its result — the `at` its row carries on \
                         the case route or a comparison — or the proposal writes its question"
                    .into(),
            })?;
        let invalid = |field: &str, reason: String| EvaluationError::Invalid {
            field: field.into(),
            reason,
        };
        let result = self
            .get(evaluation_id, subject, now)
            .await?
            .ok_or_else(|| {
                invalid(
                    "target",
                    format!("no result is published as {evaluation_id}"),
                )
            })?;
        let manifest = result
            .manifest
            .ok_or(EvaluationError::Unavailable(result.state))?;
        require(
            manifest.context.dataset.kind != crate::DatasetKind::Conversations,
            "target",
            "is a case of conversation evidence, and the archive is not a source of cases: its \
             words leave the seal only through a corpus export",
        )?;
        let page = self
            .cases(
                evaluation_id,
                &result.receipt.version,
                Some(at),
                Some(1),
                subject,
                now,
            )
            .await?
            .ok_or_else(|| {
                invalid(
                    "target",
                    format!("no result is published as {evaluation_id}"),
                )
            })?;
        let case = page
            .cases
            .into_iter()
            .next()
            .filter(|case| {
                case.measurement.case_id == *case_id
                    && case.measurement.repetition_id == *repetition_id
            })
            .ok_or_else(|| {
                invalid(
                    "at",
                    format!("does not point at {case_id} ({repetition_id}) in {evaluation_id}"),
                )
            })?;
        let asked = self
            .cohort_cases(&manifest, subject)
            .await?
            .inputs
            .remove(case_id)
            .ok_or_else(|| {
                invalid(
                    "question",
                    format!(
                        "the cohort keeps no input for {case_id}, so there is no question to \
                         read; write it"
                    ),
                )
            })?;
        let words = |value: &serde_json::Value| match value {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Object(fields) => match fields.get("question") {
                Some(serde_json::Value::String(text)) => text.clone(),
                _ => value.to_string(),
            },
            other => other.to_string(),
        };
        Ok(crate::review::Words {
            question: words(&asked),
            answer: case.measurement.actual.as_ref().map(|actual| match actual {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            }),
            content: crate::ReviewContent::Measured,
        })
    }

    /// The proposals under way on one target, whichever dataset each joins.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn reviews_of(
        &self,
        target: &crate::AssessmentTarget,
    ) -> Result<crate::TargetReviews> {
        crate::review::for_target(&self.store, target).await
    }

    /// Write an expected answer, approve or reject; `None` when there is no
    /// such proposal in that dataset.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for an action its state refuses, and
    /// [`EvaluationError::Unavailable`] `Forbidden` for approving somebody
    /// else's words without the admin role.
    pub async fn review_case(
        &self,
        dataset: &str,
        id: &str,
        action: &crate::ReviewAction,
        subject: &str,
        admin: bool,
        now: i64,
    ) -> Result<Option<crate::ReviewItem>> {
        crate::review::act(&self.store, dataset, id, action, subject, admin, now).await
    }

    /// Every proposal for one dataset, oldest first.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn reviews(&self, dataset: &str) -> Result<crate::ReviewPage> {
        crate::review::page(&self.store, dataset).await
    }

    /// Mark approved proposals as published in a dataset version, after that
    /// version was written.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Contested`] when another write kept winning.
    pub async fn reviews_published(
        &self,
        items: &[crate::ReviewItem],
        version: &str,
        subject: &str,
        now: i64,
    ) -> Result<Vec<crate::ReviewItem>> {
        crate::review::published(&self.store, items, version, subject, now).await
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

    /// What no adapter can read back for evidence this deployment measured.
    ///
    /// Every other pin is admitted by re-reading its owner's bytes from the
    /// operator's bundle, and a producer's suite and scorer are two of those
    /// files. Here neither is a file. The scorer is this binary's vocabulary,
    /// so its version is compared with the one compiled in; the suite is a
    /// scorecard in this registry's own store, so it is read from there — and
    /// the metrics the context declares must be exactly the ones that card
    /// derives, or a direction somebody edited would be admitted as the card's.
    async fn admit_scoring(&self, context: &crate::EvaluationContext) -> Result<()> {
        if !context.scored_here() {
            return Ok(());
        }
        require(
            context.scorer.version == crate::SCORING_VERSION,
            "context.scorer.version",
            &format!(
                "this deployment scores with version {}",
                crate::SCORING_VERSION
            ),
        )?;
        let card = self
            .scorecard(&context.suite.name, Some(&context.suite.version))
            .await?
            .ok_or_else(|| EvaluationError::Invalid {
                field: "context.suite".into(),
                reason: "names no scorecard this registry published".into(),
            })?;
        let rubrics = self.rubrics_for(&card.scorecard).await?;
        require(
            card.scorecard.metrics(&rubrics)? == context.metrics,
            "context.metrics",
            "must be exactly the metrics the scorecard declares",
        )?;
        self.admit_judge(context, &card.scorecard).await?;
        self.admit_external_calibration(context, &card.scorecard)
            .await
    }

    /// A judge's own rule rather than the bytes rule: its settings and its
    /// calibration set are read from this registry by the digests the context
    /// pins, because a model's answer is not something to re-read.
    async fn admit_judge(
        &self,
        context: &crate::EvaluationContext,
        card: &crate::Scorecard,
    ) -> Result<()> {
        let asks = card.judges();
        let Some(judge) = &context.judge else {
            return require(
                asks.is_empty(),
                "context.judge",
                "the scorecard asks a judge, and a result it measured names one",
            );
        };
        require(
            !asks.is_empty(),
            "context.judge",
            "names a judge the scorecard does not ask",
        )?;
        require(
            matches!(judge.provider.as_str(), "openai" | "llamacpp"),
            "context.judge.provider",
            "names a judge profile this deployment does not implement",
        )?;
        crate::judge::settings(&self.store, &judge.configuration)
            .await?
            .ok_or_else(|| EvaluationError::Invalid {
                field: "context.judge.configuration".into(),
                reason: "names judge settings no run declared".into(),
            })?;
        let taken = self
            .pinned_calibration(
                &judge.calibration_dataset,
                "context.judge.calibration_dataset",
            )
            .await?;
        // Derived, so a context that says otherwise was written by hand — and
        // the one thing it must not be able to do is admit a judge that reads
        // the archive under a context that says it does not.
        require(
            judge.reads_archive
                == (context.dataset.kind == crate::DatasetKind::Conversations
                    || taken.calibration.from_archive),
            "context.judge.reads_archive",
            "must say whether the judge is sent the conversation archive's words, as this cohort \
             and this calibration set decide",
        )?;
        covers(&taken.calibration, &asks)
    }

    /// The same rule for the set a card's calibrated framework metrics are
    /// held against.
    async fn admit_external_calibration(
        &self,
        context: &crate::EvaluationContext,
        card: &crate::Scorecard,
    ) -> Result<()> {
        let asks = card.external_calibrations();
        let Some(pin) = &context.external_calibration else {
            return require(
                asks.is_empty(),
                "context.external_calibration",
                "the scorecard holds a framework metric against people, and a result it measured \
                 names the calibration set",
            );
        };
        require(
            !asks.is_empty(),
            "context.external_calibration",
            "names a calibration set for framework metrics the scorecard does not calibrate",
        )?;
        let taken = self
            .pinned_calibration(
                &pin.calibration_dataset,
                "context.external_calibration.calibration_dataset",
            )
            .await?;
        require(
            pin.reads_archive == taken.calibration.from_archive,
            "context.external_calibration.reads_archive",
            "must say whether the scorer service is sent the conversation archive's words, as \
             this calibration set decides",
        )?;
        covers(&taken.calibration, &asks)
    }

    async fn pinned_calibration(
        &self,
        pinned: &crate::DatasetReference,
        field: &str,
    ) -> Result<crate::CalibrationVersion> {
        require(
            pinned.kind == crate::DatasetKind::Assessments,
            field,
            "a model's word is calibrated against people's judgements",
        )?;
        self.calibration(&pinned.version)
            .await?
            .filter(|taken| taken.calibration.name == pinned.name)
            .ok_or_else(|| EvaluationError::Invalid {
                field: field.into(),
                reason: "names no calibration set this registry took".into(),
            })
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
            // Not yet is a state somebody can change by admitting it, so it is
            // not the refusal a withdrawal is, and it names the approval.
            None if publishing => Err(EvaluationError::NotAdmitted(id)),
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
        self.admit_scoring(&request.manifest.context).await?;
        judged_as_declared(&request)?;
        calibrated_as_declared(&request)?;
        require(
            request.traces.is_none() || request.manifest.context.scored_here(),
            "traces",
            "says what the traces of generated answers showed, and only a run this deployment \
             measured looked at them",
        )?;
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
        let source = match self.authority.resolve(&request.manifest, subject).await {
            // An adapter with nothing that admits this pair is, for a pair no
            // operator admitted either, the pair not being admitted yet — and
            // admitting it is where the adapter's own reason will surface.
            Err(EvaluationError::Unavailable(EvidenceState::Forbidden)) => {
                self.admitted(&prepared, true).await?;
                return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
            }
            resolved => resolved?,
        };
        require(
            source.expected.len() as u64 == request.manifest.context.case_count,
            "source",
            "selected case count differs from pinned manifest",
        )?;
        // After the adapter, never before it: a source that is gone says so,
        // rather than arriving as "nobody approved this" and sending somebody
        // to approve a pair whose bytes are not there to admit.
        let approval = self.admitted(&prepared, true).await?;
        if approval.is_some_and(|approval| !same_bundle(&approval.record, &source)) {
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
            judge: request.judge.clone(),
            external: request.external.clone(),
            usage: crate::ResultUsage::of(&request.cases),
            traces: request.traces.clone(),
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

    /// Whether one result may ship against a baseline, under a policy.
    ///
    /// The header comparison decides whether the two compare and by how much
    /// each metric moved; the critical cases are read from the case diff,
    /// walked page by page until every one named is found or both results
    /// are exhausted. `None` when either result is not there.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for a policy that says nothing a gate can
    /// hold, and whatever reading the two results refuses.
    pub async fn gate(
        &self,
        id: &str,
        baseline_id: &str,
        policy: &crate::GatePolicy,
        subject: &str,
        now: i64,
    ) -> Result<Option<crate::GateDecision>> {
        policy.validate()?;
        let Some(comparison) = self.compare(id, baseline_id, subject, now).await? else {
            return Ok(None);
        };
        let mut wanted: BTreeSet<&str> = policy.critical_cases.iter().map(String::as_str).collect();
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        while !wanted.is_empty() && comparison.comparability != Comparability::Incompatible {
            let Some(page) = self
                .compare_cases(
                    id,
                    baseline_id,
                    DiffQuery {
                        cursor: cursor.as_deref(),
                        limit: Some(200),
                        only: None,
                    },
                    subject,
                    now,
                )
                .await?
            else {
                break;
            };
            for row in page.cases {
                if wanted.remove(row.case_id.as_str()) {
                    rows.push(row);
                }
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(Some(crate::gate_decision(&comparison, policy, &rows)))
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
                // Neither a judge nor a model grading an external metric gives
                // numbers that re-reading reproduces.
                result.reproducible = metadata.manifest.context.judge.is_none()
                    && metadata.manifest.context.metrics.iter().all(|metric| {
                        metric
                            .measured_by
                            .as_ref()
                            .is_none_or(|measure| measure.model.is_none())
                    });
                result.judge = metadata.judge.clone();
                result.external = metadata.external.clone();
                result.usage = metadata.usage.clone();
                result.traces = metadata.traces.clone();
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
        if approval.is_some_and(|approval| !same_bundle(&approval.record, &source)) {
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
                at: None,
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
                let first = start.max(offset);
                page.cases.extend(
                    rows.into_iter()
                        .skip(offset.saturating_sub(start))
                        .take(end - first)
                        .zip(first..)
                        .map(|(case, position)| EvidenceCase {
                            at: Some(format!("{version}:{position}")),
                            ..case
                        }),
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

    /// The contexts results were published in, newest first, from at most
    /// [`EXPERIMENT_WALK`] of the newest results.
    ///
    /// Grouped from the index receipts, which name the context and the
    /// variant, so only one header per context is read — for what it
    /// measures.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn experiments(&self, subject: &str, now: i64) -> Result<crate::ExperimentIndex> {
        let mut entries = self.store.0.list(store::INDEX).await?;
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let truncated = entries.len() > EXPERIMENT_WALK;
        let mut grouped: Vec<(String, Vec<IndexEntry>)> = Vec::new();
        for entry in entries.iter().take(EXPERIMENT_WALK) {
            let Some(indexed) = self.store.read::<IndexEntry>(&entry.key).await? else {
                continue;
            };
            match grouped
                .iter_mut()
                .find(|(context, _)| *context == indexed.receipt.context_id)
            {
                Some((_, results)) => results.push(indexed),
                None => grouped.push((indexed.receipt.context_id.clone(), vec![indexed])),
            }
        }
        let mut sources = Sources::default();
        let mut experiments = Vec::with_capacity(grouped.len());
        for (context_id, results) in grouped {
            let mut described = None;
            for indexed in results.iter().filter(|indexed| indexed.retired.is_none()) {
                let (detail, _) = self
                    .read_committed(indexed.receipt.clone(), subject, now, &mut sources)
                    .await?;
                if let Some(manifest) = detail.manifest {
                    described = Some(manifest.context);
                    break;
                }
            }
            let variants: BTreeSet<&str> = results
                .iter()
                .map(|indexed| indexed.receipt.variant_id.as_str())
                .collect();
            experiments.push(crate::ExperimentEntry {
                context_id,
                results: results.len(),
                variants: variants.len(),
                latest_committed_at: results
                    .iter()
                    .map(|indexed| indexed.receipt.committed_at)
                    .max()
                    .unwrap_or_default(),
                suite: described.as_ref().map(|context| context.suite.clone()),
                dataset: described.as_ref().map(|context| context.dataset.clone()),
                split: described.as_ref().map(|context| context.split.clone()),
                case_count: described.as_ref().map(|context| context.case_count),
            });
        }
        Ok(crate::ExperimentIndex {
            experiments,
            truncated,
        })
    }

    /// Every result in one context, newest first, each compared to the
    /// baseline when one is named. `None` when nothing was published in it,
    /// or the baseline is not one of its results.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn experiment(
        &self,
        context_id: &str,
        baseline: Option<&str>,
        subject: &str,
        now: i64,
    ) -> Result<Option<crate::Experiment>> {
        let mut results = Vec::new();
        let mut cursor = None;
        let truncated = loop {
            let page = self
                .list(cursor.as_deref(), 200, None, Some(context_id), subject, now)
                .await?;
            results.extend(page.evaluations);
            match page.next_cursor {
                Some(next) if results.len() < EXPERIMENT_WALK => cursor = Some(next),
                Some(_) => break true,
                None => break false,
            }
        };
        if results.is_empty() {
            return Ok(None);
        }
        let chosen = match baseline {
            Some(id) => match results
                .iter()
                .find(|result| result.receipt.evaluation_id == id)
            {
                Some(found) => Some(found.clone()),
                None => return Ok(None),
            },
            None => None,
        };
        let metrics = results
            .iter()
            .find_map(|result| result.manifest.as_ref())
            .map(|manifest| manifest.context.metrics.clone())
            .unwrap_or_default();
        Ok(Some(crate::Experiment {
            context_id: context_id.to_owned(),
            metrics,
            baseline: baseline.map(str::to_owned),
            rows: results
                .into_iter()
                .map(|result| crate::ExperimentRow::of(result, chosen.as_ref()))
                .collect(),
            truncated,
        }))
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
        require(
            case.usage
                .as_ref()
                .and_then(|usage| usage.latency_ms)
                .is_none_or(|latency| latency.is_finite() && latency >= 0.0),
            "case.usage.latency_ms",
            "must be a finite number of milliseconds, nought or more",
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

/// Whether a calibration set holds a person's judgement under every rubric a
/// card's judges ask. One rubric with none would be a judge reported as
/// calibrated on the strength of another rubric's people.
fn covers(set: &crate::CalibrationSet, asked: &[&crate::VersionReference]) -> Result<()> {
    for pinned in asked {
        require(
            set.items.iter().any(|item| item.rubric == **pinned),
            "calibration",
            &format!(
                "holds no human judgement under {} version {}; a judge calibrated against \
                 nothing is refused rather than defaulted",
                pinned.name, pinned.version
            ),
        )?;
    }
    Ok(())
}

/// A result whose framework metrics were calibrated carries their agreement,
/// and only such a result does.
fn calibrated_as_declared(request: &PublishEvaluation) -> Result<()> {
    match (
        &request.manifest.context.external_calibration,
        &request.external,
    ) {
        (None, None) => Ok(()),
        (Some(pin), Some(report)) => require(
            request.manifest.context.scored_here()
                && report.calibration.name == pin.calibration_dataset.name
                && report.calibration.version == pin.calibration_dataset.version,
            "external",
            "reports agreement against a calibration set other than the one the context pins",
        ),
        (Some(_), None) => Err(EvaluationError::Invalid {
            field: "external".into(),
            reason: "a result whose framework metrics were calibrated carries how far their \
                     verdicts agreed with its calibration set"
                .into(),
        }),
        (None, Some(_)) => Err(EvaluationError::Invalid {
            field: "external".into(),
            reason: "reports framework metrics' agreement for a context that calibrates none"
                .into(),
        }),
    }
}

/// A judged result carries its agreement, and only a judged result does.
fn judged_as_declared(request: &PublishEvaluation) -> Result<()> {
    match (&request.manifest.context.judge, &request.judge) {
        (None, None) => Ok(()),
        (Some(judge), Some(report)) => require(
            request.manifest.context.scored_here()
                && report.calibration.name == judge.calibration_dataset.name
                && report.calibration.version == judge.calibration_dataset.version,
            "judge",
            "reports agreement against a calibration set other than the one the context pins",
        ),
        (Some(_), None) => Err(EvaluationError::Invalid {
            field: "judge".into(),
            reason: "a judged result carries how far the judge agreed with its calibration set"
                .into(),
        }),
        (None, Some(_)) => Err(EvaluationError::Invalid {
            field: "judge".into(),
            reason: "reports a judge's agreement for a context that names no judge".into(),
        }),
    }
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
        let rubrics = self.rubrics_for(scorecard).await?;
        // What each external metric is, pinned from the catalog now — so the
        // card's version names the framework release and the model, and a
        // later catalog changes no card already published.
        let scorecard = if scorecard.asks_a_scorer_service() {
            let recorded = self.scorer_catalog().await?;
            scorecard.declared_against(recorded.as_ref().map(|recorded| &recorded.catalog))?
        } else {
            scorecard.clone()
        };
        crate::scorecard::publish(&self.store, &scorecard, &rubrics, published_by, now).await
    }

    /// Keep what a scorer service says it measures, for the role that opens no
    /// socket to it.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for a catalog this build cannot pin a card
    /// against, and [`EvaluationError::Storage`] when the store cannot be
    /// reached.
    pub async fn record_scorer_catalog(
        &self,
        catalog: &crate::ScorerCatalog,
        recorded_by: &str,
        now: i64,
    ) -> Result<crate::RecordedCatalog> {
        catalog.validate()?;
        let recorded = crate::RecordedCatalog {
            catalog: catalog.clone(),
            recorded_at: now,
            recorded_by: recorded_by.to_owned(),
        };
        self.store
            .0
            .put(crate::store::SCORER_CATALOG, crate::canonical(&recorded)?)
            .await?;
        Ok(recorded)
    }

    /// The scorer service's catalog, as it was last recorded.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scorer_catalog(&self) -> Result<Option<crate::RecordedCatalog>> {
        self.store.read(crate::store::SCORER_CATALOG).await
    }

    /// What a scorer service already answered to this question in this run.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn remembered_score(
        &self,
        declaration: &str,
        call: &crate::ExternalCall,
    ) -> Result<Option<crate::ExternalReply>> {
        text(declaration, "run")?;
        Ok(self
            .store
            .read::<crate::KeptScore>(&crate::store::external_reply(
                declaration,
                &call.question()?,
            ))
            .await?
            .map(Into::into))
    }

    /// Keep what a scorer service answered, and hand back the reply that
    /// stands: the first write wins, as a judge's does, so two attempts publish
    /// the same bytes.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn remember_score(
        &self,
        declaration: &str,
        call: &crate::ExternalCall,
        reply: &crate::ExternalReply,
    ) -> Result<crate::ExternalReply> {
        text(declaration, "run")?;
        let key = crate::store::external_reply(declaration, &call.question()?);
        let kept = crate::KeptScore::from(reply);
        if self.store.create(&key, &kept).await? {
            return Ok(kept.into());
        }
        Ok(self
            .store
            .read::<crate::KeptScore>(&key)
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?
            .into())
    }

    /// The rubric versions a card's judges ask and its framework metrics are
    /// calibrated against, resolved.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] naming a rubric version nobody published,
    /// and [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn rubrics_for(&self, scorecard: &crate::Scorecard) -> Result<crate::Rubrics> {
        let mut rubrics = crate::Rubrics::default();
        for pinned in scorecard
            .judges()
            .into_iter()
            .chain(scorecard.external_calibrations())
        {
            let published = self
                .rubric(&pinned.name, Some(&pinned.version))
                .await?
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "scorecard.scorers.scorer.rubric".into(),
                    reason: format!(
                        "{} has no published version {}",
                        pinned.name, pinned.version
                    ),
                })?;
            rubrics = rubrics.with(pinned, published.rubric);
        }
        Ok(rubrics)
    }

    /// Freeze the judgements people made of one result's cases, as the set a
    /// judge is calibrated against.
    ///
    /// Only a person's judgement, and only under the rubric versions asked for:
    /// a revision made under other words answered another question. The result
    /// is read at the version it has now, and the set names that version, so
    /// the judge is later put the same answers the people were.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the result is not readable, is
    /// conversation evidence, or holds no human judgement under those rubrics;
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn take_calibration(
        &self,
        request: &crate::CalibrationRequest,
        subject: &str,
        now: i64,
    ) -> Result<crate::CalibrationVersion> {
        text(&request.name, "calibration.name")?;
        require(
            (1..=32).contains(&request.rubrics.len()),
            "calibration.rubrics",
            "requires 1–32 rubric versions",
        )?;
        for pinned in &request.rubrics {
            pinned.validate("calibration.rubrics")?;
            self.rubric(&pinned.name, Some(&pinned.version))
                .await?
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "calibration.rubrics".into(),
                    reason: format!(
                        "{} has no published version {}",
                        pinned.name, pinned.version
                    ),
                })?;
        }
        let unreadable = || EvaluationError::Invalid {
            field: "calibration.evaluation_id".into(),
            reason: "names no result whose cases can be read now".into(),
        };
        let header = self
            .get(&request.evaluation_id, subject, now)
            .await?
            .ok_or_else(unreadable)?;
        // Conversation evidence a caller may not read is refused as that, not
        // as a result that is not there: an admin takes this set.
        if header.state == EvidenceState::Forbidden {
            return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
        }
        if !matches!(
            header.state,
            EvidenceState::Complete | EvidenceState::Partial
        ) {
            return Err(unreadable());
        }
        let manifest = header.manifest.as_ref().ok_or_else(unreadable)?;
        // A judge is put what the people were shown. From conversation evidence
        // that is the archive's words, which a run calibrated on this set says.
        let from_archive = manifest.context.dataset.kind == crate::DatasetKind::Conversations;
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .cases(
                    &request.evaluation_id,
                    &header.receipt.version,
                    cursor.as_deref(),
                    Some(200),
                    subject,
                    now,
                )
                .await?
                .filter(|page| {
                    matches!(page.state, EvidenceState::Complete | EvidenceState::Partial)
                })
                .ok_or_else(unreadable)?;
            for case in &page.cases {
                let target = crate::AssessmentTarget::Case {
                    evaluation_id: request.evaluation_id.clone(),
                    case_id: case.measurement.case_id.clone(),
                    repetition_id: case.measurement.repetition_id.clone(),
                };
                for said in self.assessments(&target).await?.assessments {
                    if said.source != crate::AssessmentSource::Human {
                        continue;
                    }
                    let Some(pinned) = request.rubrics.iter().find(|pinned| {
                        pinned.name == said.rubric && pinned.version == said.rubric_version
                    }) else {
                        continue;
                    };
                    items.push(crate::CalibrationItem {
                        case_id: case.measurement.case_id.clone(),
                        repetition_id: case.measurement.repetition_id.clone(),
                        rubric: pinned.clone(),
                        value: said.value,
                        author: said.author,
                        standing_id: said.standing_id,
                        revision: said.revision,
                    });
                }
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        items.sort_by(|a, b| {
            (&a.case_id, &a.repetition_id, &a.rubric.name, &a.author).cmp(&(
                &b.case_id,
                &b.repetition_id,
                &b.rubric.name,
                &b.author,
            ))
        });
        let set = crate::CalibrationSet {
            name: request.name.clone(),
            result: crate::VersionReference {
                name: request.evaluation_id.clone(),
                version: header.receipt.version,
            },
            items,
            from_archive,
        };
        let asked: Vec<&crate::VersionReference> = request.rubrics.iter().collect();
        covers(&set, &asked)?;
        crate::judge::keep_calibration(&self.store, set, subject, now).await
    }

    /// One calibration set, re-verified against its own address.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when the stored set is not the one
    /// its address names.
    pub async fn calibration(&self, version: &str) -> Result<Option<crate::CalibrationVersion>> {
        crate::judge::calibration(&self.store, version).await
    }

    /// What this declared run's judge already said to exactly this question.
    ///
    /// A judged step that failed halfway, or published and then lost its
    /// settlement, is attempted again under the same declaration. Asking the
    /// model again would pay for every answer twice and, worse, fold different
    /// answers into different bytes — which the first publication of that ID
    /// then refuses as a conflict, failing a run whose result is already there.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached, and
    /// [`EvaluationError::Unavailable`] when the kept reply does not read.
    pub async fn remembered_reply(
        &self,
        declaration: &str,
        call: &crate::JudgeCall,
    ) -> Result<Option<crate::JudgeReply>> {
        crate::judge::remembered(&self.store, declaration, call).await
    }

    /// Keep what the judge said, and hand back the reply that stands.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn remember_reply(
        &self,
        declaration: &str,
        call: &crate::JudgeCall,
        reply: crate::JudgeReply,
    ) -> Result<crate::JudgeReply> {
        crate::judge::remember(&self.store, declaration, call, reply).await
    }

    /// The answers a calibration set's people were shown, from the result it
    /// names at the version it names.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when that result can no longer be
    /// read at that version.
    pub async fn calibrated(
        &self,
        set: &crate::CalibrationSet,
        with_inputs: bool,
        subject: &str,
        now: i64,
    ) -> Result<crate::scoring::Calibrated> {
        // What those cases were asked lives with the result's source rather
        // than in its shards, so it is resolved there — and only when a judge
        // is to be shown it, because a source that went away since would
        // otherwise refuse a calibration that never needed it.
        let inputs = if with_inputs {
            let header = self
                .get(&set.result.name, subject, now)
                .await?
                .filter(|header| header.receipt.version == set.result.version)
                .ok_or(EvaluationError::Unavailable(EvidenceState::DeletedSource))?;
            let manifest = header
                .manifest
                .ok_or(EvaluationError::Unavailable(header.state))?;
            self.cohort_cases(&manifest, subject).await?.inputs
        } else {
            BTreeMap::new()
        };
        let wanted: BTreeSet<(&str, &str)> = set
            .items
            .iter()
            .map(|item| (item.case_id.as_str(), item.repetition_id.as_str()))
            .collect();
        let mut found = crate::scoring::Calibrated::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .cases(
                    &set.result.name,
                    &set.result.version,
                    cursor.as_deref(),
                    Some(200),
                    subject,
                    now,
                )
                .await?
                .ok_or(EvaluationError::Unavailable(EvidenceState::DeletedSource))?;
            // A page that could not be read is not a page with nobody on it:
            // a judge calibrated against the answers that happened to load is
            // reported against fewer people than the set holds.
            if !matches!(page.state, EvidenceState::Complete | EvidenceState::Partial) {
                return Err(EvaluationError::Unavailable(page.state));
            }
            for case in page.cases {
                let key = (
                    case.measurement.case_id.as_str(),
                    case.measurement.repetition_id.as_str(),
                );
                if wanted.contains(&key)
                    && let Some(actual) = case.measurement.actual
                {
                    found.insert(
                        (
                            case.measurement.case_id.clone(),
                            case.measurement.repetition_id.clone(),
                        ),
                        crate::scoring::Shown {
                            input: inputs.get(&case.measurement.case_id).cloned(),
                            answer: actual,
                            expected: case.expected,
                        },
                    );
                }
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(found)
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scorecards(&self) -> Result<Vec<crate::ScorecardHead>> {
        crate::scorecard::all(&self.store).await
    }

    /// Every version of one card, newest first; `None` when it was never
    /// published.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scorecard_versions(&self, name: &str) -> Result<Option<crate::ScorecardVersions>> {
        text(name, "scorecard")?;
        let versions = crate::scorecard::versions(&self.store, name).await?;
        Ok((!versions.is_empty()).then(|| crate::ScorecardVersions {
            name: name.to_owned(),
            versions,
        }))
    }

    /// What changed from one version of a card to another; `None` when either
    /// is not a version of it.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store or a rubric cannot be read.
    pub async fn scorecard_diff(
        &self,
        name: &str,
        from: &str,
        to: &str,
    ) -> Result<Option<crate::ScorecardDiff>> {
        text(name, "scorecard")?;
        let (Some(before), Some(after)) = (
            crate::scorecard::version(&self.store, name, from).await?,
            crate::scorecard::version(&self.store, name, to).await?,
        ) else {
            return Ok(None);
        };
        let mut rubrics = self.rubrics_for(&before.scorecard).await?;
        for (reference, rubric) in self.rubrics_for(&after.scorecard).await?.entries() {
            rubrics = rubrics.with(&reference, rubric);
        }
        crate::scorecard_diff(&before, &after, &rubrics).map(Some)
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
    /// unusable or the card cannot measure these answers,
    /// [`EvaluationError::Unavailable`] when nobody published the card at that
    /// version, and [`EvaluationError::Storage`] when the store cannot be
    /// reached.
    pub async fn declare_scoring_run(
        &self,
        run: &crate::ScoringRun,
        declared_by: &str,
        now: i64,
    ) -> Result<crate::DeclaredRun> {
        run.validate()?;
        let card = self
            .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        run.check(&card.scorecard)?;
        if let Some(judge) = &run.judge {
            let taken = self
                .calibration(&judge.calibration.version)
                .await?
                .filter(|taken| taken.calibration.name == judge.calibration.name)
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "run.judge.calibration".into(),
                    reason: "names no calibration set this registry took".into(),
                })?;
            require(
                taken.calibration.result.name != run.evaluation_id,
                "run.judge.calibration",
                "is taken from this run's own result; a judge is calibrated on answers people \
                 already judged, not on the ones it is about to",
            )?;
            covers(&taken.calibration, &card.scorecard.judges())?;
            // The settings' bytes, where admission reads them by the digest the
            // context will pin — before the declaration that names them.
            crate::judge::keep_settings(&self.store, &judge.settings).await?;
        }
        if let Some(named) = &run.external_calibration {
            let taken = self
                .calibration(&named.version)
                .await?
                .filter(|taken| taken.calibration.name == named.name)
                .ok_or_else(|| EvaluationError::Invalid {
                    field: "run.external_calibration".into(),
                    reason: "names no calibration set this registry took".into(),
                })?;
            require(
                taken.calibration.result.name != run.evaluation_id,
                "run.external_calibration",
                "is taken from this run's own result; a metric is calibrated on answers people \
                 already judged, not on the ones it is about to",
            )?;
            covers(&taken.calibration, &card.scorecard.external_calibrations())?;
        }
        crate::scoring::declare(&self.store, run, declared_by, now).await
    }

    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scoring_run(&self, id: &str) -> Result<Option<crate::DeclaredRun>> {
        text(id, "run")?;
        crate::scoring::declared(&self.store, id).await
    }

    /// A declaration with the manifest it publishes and the approval that
    /// admits it.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when the card the declaration pinned is
    /// gone, and [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn scoring_run_view(&self, id: &str) -> Result<Option<crate::ScoringRunView>> {
        let Some(declaration) = self.scoring_run(id).await? else {
            return Ok(None);
        };
        let pinned = &declaration.run.scorecard;
        let card = self
            .scorecard(&pinned.name, Some(&pinned.version))
            .await?
            .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?;
        let rubrics = self.rubrics_for(&card.scorecard).await?;
        let calibration = match &declaration.run.judge {
            Some(judge) => Some(
                self.calibration(&judge.calibration.version)
                    .await?
                    .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?
                    .calibration,
            ),
            None => None,
        };
        let external_calibration = match &declaration.run.external_calibration {
            Some(named) => Some(
                self.calibration(&named.version)
                    .await?
                    .ok_or(EvaluationError::Unavailable(EvidenceState::MissingArtifact))?
                    .calibration,
            ),
            None => None,
        };
        let manifest = declaration.run.manifest(
            &card.scorecard,
            &rubrics,
            calibration.as_ref(),
            external_calibration.as_ref(),
            None,
        )?;
        let prepared = Evaluation::prepare(manifest.clone())?;
        let cohort = self
            .derived_cohort(&declaration.run.cohort.case_manifest.digest)
            .await
            .ok()
            .flatten()
            .filter(|derived| {
                derived.describes(&declaration.run.variant.dataset, &declaration.run.cohort)
            });
        Ok(Some(crate::ScoringRunView {
            cohort,
            approval_id: approval_id(prepared.variant_id(), prepared.context_id())?,
            variant_id: prepared.variant_id().to_owned(),
            context_id: prepared.context_id().to_owned(),
            admitted: self.admits(&manifest).await?,
            warnings: crate::warnings(&manifest, &card.scorecard, calibration.as_ref()),
            manifest,
            declaration,
        }))
    }

    /// Whether a publication of this manifest would pass the operator's gate.
    ///
    /// The gate itself, asked without publishing: a caller that checked for an
    /// approval record on its own would be a second answer to what admitted
    /// means, and the first one also knows about withdrawal.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the manifest does not prepare, and
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn admits(&self, manifest: &EvaluationManifest) -> Result<bool> {
        match self.admission(manifest).await {
            Ok(()) => Ok(true),
            Err(
                EvaluationError::NotAdmitted(_)
                | EvaluationError::Unavailable(EvidenceState::Forbidden),
            ) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The same gate, answering with its own refusal rather than a boolean.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::NotAdmitted`] naming the approval while nobody has
    /// admitted the pair, [`EvaluationError::Unavailable`] with
    /// [`EvidenceState::Forbidden`] once it was withdrawn, and the errors of
    /// [`Registry::admits`].
    pub async fn admission(&self, manifest: &EvaluationManifest) -> Result<()> {
        let prepared = Evaluation::prepare(manifest.clone())?;
        self.admitted(&prepared, true).await.map(drop)
    }

    /// Keep the answers a run will measure, and hand back the reference.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] when the bytes are not recorded answers,
    /// and [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn stage_recording(
        &self,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<aiwatcher_core::ArtifactRef> {
        crate::scoring::stage(&self.store, name, bytes).await
    }

    /// The answers a declaration named, re-verified against its digest.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when the bytes are gone or are not
    /// what the declaration pinned.
    pub async fn recording(
        &self,
        answers: &aiwatcher_core::ArtifactRef,
    ) -> Result<crate::RecordedAnswers> {
        crate::scoring::recorded(&self.store, answers).await
    }

    /// What the cohort a manifest selects expected each case to answer.
    ///
    /// The one door onto the source authority for a caller that has to score
    /// before it publishes. Resolving it here rather than holding a second
    /// reference to the adapter keeps the owner's rights, retention and
    /// deletion checks on one path — the same call publication makes.
    ///
    /// # Errors
    ///
    /// Whatever the adapter refused: a source that is gone, forbidden, or
    /// unreachable.
    pub async fn cohort(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        Ok(self.cohort_cases(manifest, subject).await?.expected)
    }

    /// Derive a cohort from a dataset version's owner, and remember where it
    /// came from.
    ///
    /// A conversation corpus's cases are content, so deriving from one takes
    /// content access, as reading its cases does. What is remembered holds no
    /// case: the request, the pins and the count.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Invalid`] for a request that selects nothing or a
    /// dataset nothing here owns, [`EvaluationError::Unavailable`] with
    /// [`EvidenceState::Forbidden`] for a conversation corpus without content
    /// access, and whatever the owner refused.
    pub async fn derive_cohort(
        &self,
        request: &crate::CohortRequest,
        subject: &str,
        now: i64,
    ) -> Result<crate::DerivedCohort> {
        request.validate()?;
        text(subject, "derived_by")?;
        if request.dataset.kind == crate::DatasetKind::Conversations && !self.content_access {
            return Err(EvaluationError::Unavailable(EvidenceState::Forbidden));
        }
        let files = self.authority.derive_cohort(request, subject).await?;
        require(
            files.count > 0,
            "cohort.split",
            "selects no case: the owner holds none for this version and split",
        )?;
        let derived = crate::DerivedCohort {
            request: request.clone(),
            cohort: files.cohort(&request.split),
            available: files.available,
            unsplit: files.unsplit,
            derived_by: subject.to_owned(),
            derived_at: now,
        };
        let key = crate::store::derived_cohort(&derived.cohort.case_manifest.digest);
        // Create-only: the first derivation of these cases is the one kept, and
        // a later one of the same cases says nothing it did not about where
        // they came from. How many the split held, and how many of these name
        // no split, are this request's: another version or split can select
        // the same cases out of more of them.
        self.store.create(&key, &derived).await?;
        Ok(match self.store.read(&key).await? {
            Some(kept) => crate::DerivedCohort {
                available: derived.available,
                unsplit: derived.unsplit,
                ..kept
            },
            None => derived,
        })
    }

    /// Where the cases under this digest were derived from, if they were.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Storage`] when the store cannot be reached.
    pub async fn derived_cohort(&self, cases: &str) -> Result<Option<crate::DerivedCohort>> {
        require(
            cases.len() == 64 && cases.bytes().all(|b| b.is_ascii_hexdigit()),
            "cases",
            "is the SHA-256 digest of a cohort's cases",
        )?;
        self.store.read(&crate::store::derived_cohort(cases)).await
    }

    /// The same cohort, with what each case was asked beside what it expected.
    ///
    /// # Errors
    ///
    /// Whatever the adapter refused, as for [`Registry::cohort`].
    pub async fn cohort_cases(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> Result<crate::CohortCases> {
        // Before the adapter, as publication does: a conversation cohort's
        // expectations are content, and reading them is the thing the gate is
        // about rather than something it checks afterwards.
        self.protected(manifest)?;
        let source = self.authority.resolve(manifest, subject).await?;
        Ok(crate::CohortCases {
            expected: source.expected,
            inputs: source.inputs,
        })
    }

    /// The answers a declaration reads, from wherever it said they are.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when a recording's bytes are gone or
    /// are not what the declaration pinned.
    pub async fn answers(
        &self,
        run: &crate::ScoringRun,
        cohort: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Vec<crate::RecordedAnswer>> {
        match &run.answers {
            crate::Answers::Recording(recording) => {
                Ok(crate::scoring::recorded(&self.store, recording)
                    .await?
                    .answers)
            }
            crate::Answers::Archive(_) => Ok(crate::archived(cohort)),
            crate::Answers::Generated(_) => Err(EvaluationError::Invalid {
                field: "run.answers".into(),
                reason: "generated answers are the rows the generation step wrote, which the \
                         step that scores them reads from its own input"
                    .into(),
            }),
        }
    }

    /// The same answers as their JSON spells them, by case: empty for any but
    /// a recording, whose bytes are kept as they were staged.
    ///
    /// # Errors
    ///
    /// [`EvaluationError::Unavailable`] when a recording's bytes are gone or
    /// are not what the declaration pinned.
    pub async fn spelled_answers(
        &self,
        run: &crate::ScoringRun,
    ) -> Result<BTreeMap<String, String>> {
        match &run.answers {
            crate::Answers::Recording(recording) => {
                crate::scoring::recorded_spelled(&self.store, recording).await
            }
            crate::Answers::Archive(_) | crate::Answers::Generated(_) => Ok(BTreeMap::new()),
        }
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
