//! The facade, and the only public door.
//!
//! Every slice below is `pub(crate)` and takes an already-resolved
//! [`Backend`]. What this adds is the two things a caller must not be able to
//! skip: the deployment's [`ArchivePolicy`], which outranks whatever a producer
//! asserted, and the identity of whoever is asking, which is what a review
//! decision is attributed to.
//!
//! It is the same shape `aiwatcher_annotations::Registry` has, and it is here
//! for the same reason: a consumer that could reach the key layout could write
//! objects the registry's own reads never look for.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use time::OffsetDateTime;

use crate::archive::crypt::Keyring;
use crate::archive::{
    ArchivedTurn, ConversationHead, ConversationPage, ErasureReport, TurnFilter, TurnPage,
};
use crate::export::{
    ExportJob, ExportJobPage, ExportManifest, ExportPage, ExportRequest, ExportRowsPage,
};
use crate::policy::ArchivePolicy;
use crate::review::ReviewRequest;
use crate::store::Backend;
use crate::turn::{RecordTurnRequest, RecordedTurn, TurnContent};
use crate::{Error, MAX_TURNS_PER_WRITE, Result};

/// The archive, as everything outside this crate sees it.
#[derive(Clone, Debug)]
pub struct Registry {
    backend: Backend,
    policy: ArchivePolicy,
}

impl Registry {
    #[must_use]
    pub fn new(
        store: Arc<dyn ObjectStore>,
        prefix: impl Into<String>,
        keyring: Keyring,
        policy: ArchivePolicy,
    ) -> Self {
        Self {
            backend: Backend::new(store, prefix, keyring),
            policy,
        }
    }

    /// What this deployment demands. Read by the API so a producer can be told
    /// before it sends a megabyte of content that will be refused.
    #[must_use]
    pub fn policy(&self) -> ArchivePolicy {
        self.policy
    }

    /// Which keys this deployment can open with. Never the keys.
    #[must_use]
    pub fn key_ids(&self) -> Vec<String> {
        self.backend
            .keyring()
            .key_ids()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    // ── Writing ──────────────────────────────────────────────────────────

    /// Record one turn.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] with every policy problem at once, [`Error::TooLarge`]
    /// past the content caps, and whatever the object store says.
    pub async fn record(&self, request: RecordTurnRequest) -> Result<RecordedTurn> {
        crate::archive::record(&self.backend, self.policy, request).await
    }

    /// Record a batch — one exchange, as a producer flushes it.
    ///
    /// Not a transaction, and it does not pretend to be: each turn is written
    /// as it is validated, and a refusal partway through leaves the earlier
    /// ones stored. That is the right behaviour for an at-least-once producer
    /// whose retry lands on the same turn ids, and the wrong one for anything
    /// that expected all-or-nothing — which is why the response reports each
    /// turn rather than a count.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] when the batch is past [`MAX_TURNS_PER_WRITE`], and
    /// whatever the first refused turn says.
    pub async fn record_batch(
        &self,
        requests: Vec<RecordTurnRequest>,
    ) -> Result<Vec<RecordedTurn>> {
        if requests.len() > MAX_TURNS_PER_WRITE {
            return Err(Error::TooLarge {
                what: "the batch of turns",
                size: requests.len(),
                limit: MAX_TURNS_PER_WRITE,
            });
        }
        let mut recorded = Vec::with_capacity(requests.len());
        for request in requests {
            recorded.push(self.record(request).await?);
        }
        Ok(recorded)
    }

    /// Approve or reject one turn, attributed to the caller.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown turn, [`Error::Invalid`] for a
    /// rejection with no stated reason.
    pub async fn review(
        &self,
        conversation_id: &str,
        turn_id: &str,
        reviewer: &str,
        request: &ReviewRequest,
    ) -> Result<ArchivedTurn> {
        crate::archive::review(&self.backend, conversation_id, turn_id, reviewer, request).await
    }

    // ── Reading ──────────────────────────────────────────────────────────

    /// Every conversation the archive holds, newest activity first.
    ///
    /// # Errors
    ///
    /// Whatever the object store says.
    pub async fn conversations(&self) -> Result<ConversationPage> {
        crate::archive::conversations(&self.backend).await
    }

    /// One conversation's counts.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the archive has never seen it.
    pub async fn conversation(&self, conversation_id: &str) -> Result<ConversationHead> {
        crate::archive::conversation_head(&self.backend, conversation_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("the conversation {conversation_id}")))
    }

    /// One conversation's turns, in the order an export reads them. Heads only:
    /// no content is decrypted.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown conversation.
    pub async fn turns(
        &self,
        conversation_id: &str,
        filter: &TurnFilter,
        offset: usize,
        limit: usize,
    ) -> Result<TurnPage> {
        crate::archive::turns(&self.backend, conversation_id, filter, offset, limit).await
    }

    /// One turn's head.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown turn.
    pub async fn turn(&self, conversation_id: &str, turn_id: &str) -> Result<ArchivedTurn> {
        crate::archive::turn(&self.backend, conversation_id, turn_id).await
    }

    /// The words. The one read here that decrypts anything, and the one the
    /// API guards with the strongest role it has.
    ///
    /// # Errors
    ///
    /// [`Error::Erased`] when the content has been removed — which is an answer
    /// rather than a 404, because an auditor asked for exactly that
    /// distinction — and [`Error::NotFound`] for a turn that never existed.
    pub async fn content(&self, conversation_id: &str, turn_id: &str) -> Result<TurnContent> {
        crate::archive::content(&self.backend, conversation_id, turn_id).await
    }

    // ── Erasure ──────────────────────────────────────────────────────────

    /// Remove everything recorded about one consent subject.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an unusable subject, and whatever the store says.
    pub async fn erase_subject(&self, subject: &str, by: &str) -> Result<ErasureReport> {
        crate::archive::erase_subject(&self.backend, subject, by).await
    }

    /// Remove one whole conversation's content.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown conversation.
    pub async fn erase_conversation(
        &self,
        conversation_id: &str,
        by: &str,
    ) -> Result<ErasureReport> {
        crate::archive::erase_conversation(&self.backend, conversation_id, by).await
    }

    /// Remove the content of everything whose declared retention has run out.
    ///
    /// Idempotent, and cheap when there is nothing to do: only conversations
    /// whose earliest expiry has passed are opened at all.
    ///
    /// # Errors
    ///
    /// Whatever the object store says.
    pub async fn sweep(&self, now: OffsetDateTime) -> Result<ErasureReport> {
        crate::archive::sweep(&self.backend, now).await
    }

    // ── Exports ──────────────────────────────────────────────────────────

    /// Queue an export, pinning what it will read.
    ///
    /// Idempotent: the job id is derived from the request and the resolved
    /// selection, so a retried POST joins the job it already started.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when the selection matches no conversation, and
    /// [`Error::Invalid`] for an unusable name.
    pub async fn create_export(
        &self,
        request: ExportRequest,
        created_by: &str,
    ) -> Result<ExportJob> {
        crate::export::create(&self.backend, request, created_by).await
    }

    /// One job.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown job id.
    pub async fn export_job(&self, job_id: &str) -> Result<ExportJob> {
        crate::export::job(&self.backend, job_id).await
    }

    /// Every job, newest first.
    ///
    /// # Errors
    ///
    /// Whatever the object store says.
    pub async fn export_jobs(&self) -> Result<ExportJobPage> {
        crate::export::jobs(&self.backend).await
    }

    /// Stop a job. Whatever it has already written stays written; there is no
    /// completed version, because there is no manifest.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for a job that already finished.
    pub async fn cancel_export(&self, job_id: &str) -> Result<ExportJob> {
        crate::export::cancel(&self.backend, job_id).await
    }

    /// Job ids nobody is working on, oldest first.
    ///
    /// Includes a job left `running` by a process that died, once its lease has
    /// run out — that is the restart case, and the lease is what tells it apart
    /// from a worker that is simply still going.
    ///
    /// # Errors
    ///
    /// Whatever the object store says.
    pub async fn claimable_exports(&self) -> Result<Vec<String>> {
        crate::export::claimable(&self.backend, OffsetDateTime::now_utc()).await
    }

    /// Run one job to completion, cancellation, failure or takeover.
    ///
    /// `worker` is this process's identity — a pod name in a cluster. A job
    /// whose lease is live and held by somebody else is returned untouched, so
    /// two replicas do not export one corpus side by side.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown job. Failures *of* the export are
    /// recorded on the job rather than returned: a job that could not read its
    /// store is a job somebody has to look at, not a lost error.
    pub async fn run_export(&self, job_id: &str, worker: &str) -> Result<ExportJob> {
        crate::export::run(&self.backend, job_id, worker).await
    }

    /// Every export name and the versions under it.
    ///
    /// # Errors
    ///
    /// Whatever the object store says.
    pub async fn exports(&self) -> Result<ExportPage> {
        crate::export::exports(&self.backend).await
    }

    /// One immutable export's manifest.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when that version was never built.
    pub async fn export(&self, name: &str, version: &str) -> Result<ExportManifest> {
        crate::export::manifest(&self.backend, name, version).await
    }

    /// One page of an export's rows, decrypted.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for an unknown version or a missing shard.
    pub async fn export_rows(
        &self,
        name: &str,
        version: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ExportRowsPage> {
        crate::export::rows(&self.backend, name, version, offset, limit).await
    }
}
