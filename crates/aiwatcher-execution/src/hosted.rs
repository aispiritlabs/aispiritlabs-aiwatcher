//! The append side of a hosted execution (section 40.3).
//!
//! A hosted run's decider is the worker. An agent graph's conditions are
//! LLM-decided, its fan-out is chosen by a `planner` node at run time and its
//! join arity is discovered, so there is no static plan for this engine to
//! schedule. What this engine offers instead is the half `agentic.workflow`
//! does not have: a **shared** history with compare-and-append, an inbox, one
//! lease and timers. The decider runs in the worker; the history lives here.
//!
//! ## The one thing this must not do
//!
//! Interpret the messages. It stores a type name, the worker's metadata and a
//! reference to the content ([`crate::message::HostedMessage`]), and it folds
//! none of it — an engine that read these would be a second decider, which is
//! the thing hosted mode exists to avoid.
//!
//! ## Why this does not retry a conflict, and [`crate::handler::ExecutionHandler::handle`] does
//!
//! For a `compiled` run the decision is *this* process's, so a conflict is
//! contention: re-read, decide again, append. For a hosted run the decision is
//! the worker's and this process cannot make it again. A conflict is returned
//! — a 409 — and the worker reloads and decides on what it now sees. Its own
//! cached decision across OCC retries is what stops that reload calling the
//! model a second time to discover it lost.

use aiwatcher_core::MessageId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::decide::replay;
use crate::error::DecisionError;
use crate::handler::{ExecutionHandler, HandleError, Handled, projection_of};
use crate::message::{HostedMessage, MessageMetadata, PendingMessage, WorkflowMessage};
use crate::state::{ExecutionId, ExecutionMode};
use crate::store::{AppendOutcome, AppendRequest, ExpectedVersion, WorkflowStore};

/// Who is deciding a hosted run, and since when.
///
/// `agentic.workflow`'s `ProcessorLock` with the lease this system already
/// keeps — [`aiwatcher_jobs::LEASE_SECONDS`], called rather than copied, for
/// the reason the attempt lease gives: two lease rules that agree today are a
/// silent corruption the day one of them changes.
///
/// ## What this protects, and what it does not
///
/// It does **not** protect the history. Two deciders that both believe they
/// hold this still cannot corrupt a stream, because each append names the
/// version it read and the second one is a 409 — which is the guarantee, and it
/// holds whether or not anybody takes a lease at all.
///
/// What it protects is the *work*. An agent turn is a model call: without a
/// lease a worker that was merely slow is one whose replacement runs the same
/// turn beside it, pays for it, and finds out at the append. With one, the
/// second worker is told who holds it before it starts thinking.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct DeciderLease {
    pub execution: ExecutionId,
    pub holder: String,
    #[serde(with = "time::serde::rfc3339")]
    pub claimed_at: OffsetDateTime,
    /// Who held it before this holder, and only across a takeover.
    ///
    /// The attempt row's `previous_owner`, for the same reason it cannot be
    /// read off `holder`: by the time a takeover is visible the field already
    /// names its new one, and the question a replacement asks — "was somebody
    /// else in the middle of this?" — has no other answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_holder: Option<String>,
}

impl DeciderLease {
    #[must_use]
    pub fn taken_by(execution: &ExecutionId, holder: &str, now: OffsetDateTime) -> Self {
        Self {
            execution: execution.clone(),
            holder: holder.to_owned(),
            claimed_at: now,
            previous_holder: None,
        }
    }

    /// When this runs out if nothing renews it.
    #[must_use]
    pub fn expires_at(&self) -> OffsetDateTime {
        self.claimed_at + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS)
    }

    /// Whether nobody holds this any more.
    #[must_use]
    pub fn expired(&self, now: OffsetDateTime) -> bool {
        aiwatcher_jobs::lease_expired(Some(self.claimed_at), now)
    }

    /// Whether `holder` still holds this.
    ///
    /// The question a worker may not answer about itself: it asks, and the
    /// store answers from the row. A claimant that decided this locally would
    /// be deciding it from a clock and a name it also owns.
    #[must_use]
    pub fn held_by(&self, holder: &str, now: OffsetDateTime) -> bool {
        self.holder == holder && !self.expired(now)
    }

    /// Take this for `holder`, until the lease runs out.
    ///
    /// Renewing is taking it again: the same holder refreshes the clock and
    /// leaves `previous_holder` alone, so a heartbeat does not read as a
    /// takeover of itself.
    pub fn take(&mut self, holder: &str, now: OffsetDateTime) {
        if self.holder != holder {
            self.previous_holder = Some(std::mem::replace(&mut self.holder, holder.to_owned()));
        }
        self.claimed_at = now;
    }
}

/// What asking for a hosted run's decider lease did.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum LeaseOutcome {
    /// It is this caller's, until [`DeciderLease::expires_at`].
    Taken(DeciderLease),
    /// Somebody else has it and it has not run out. Carries *when* it does, so
    /// a second worker waits rather than polling — the one thing it can
    /// usefully do with the refusal.
    Held {
        holder: String,
        #[serde(with = "time::serde::rfc3339")]
        expires_at: OffsetDateTime,
    },
}

impl LeaseOutcome {
    #[must_use]
    pub const fn taken(&self) -> Option<&DeciderLease> {
        match self {
            Self::Taken(lease) => Some(lease),
            Self::Held { .. } => None,
        }
    }
}

/// The type name of the row that records one append.
///
/// A hosted append is stored the way every other decision is: the input that
/// caused it beside the outputs it produced, at the version it was accepted at.
/// The input is this marker, and its `message_id` is the caller's
/// `Idempotency-Key` — which is what makes the durable inbox work for a hosted
/// run without a second dedup table.
pub const HOSTED_APPEND: &str = "HostedAppend";

/// The most messages one append may carry.
///
/// Each is already bounded by [`crate::message::MAX_PAYLOAD_BYTES`]; this bounds
/// the *transaction*, because six writes that hold a row lock while a thousand
/// inserts run is a lock somebody else is waiting behind. A worker with more
/// than this appends twice, which its expected version already makes safe.
pub const MAX_BATCH: usize = 256;

/// What a worker is asking to append.
#[derive(Clone, Debug)]
pub struct HostedAppend {
    /// What the caller believed the stream was at.
    ///
    /// A number rather than an [`ExpectedVersion`], because the only other arm
    /// that would type-check here is `Any` and it is wrong twice over: a hosted
    /// decider's every append depends on what it read, and the projection this
    /// writes carries a version computed from that read — under `Any` another
    /// append could land in between and the projection would record a version
    /// the stream had already passed. The worker learns its first one from the
    /// start it was answered with.
    pub expected_version: u64,
    /// The caller's `Idempotency-Key`, scoped to this execution before it
    /// becomes an id — a key that named less than what it identifies is
    /// section 43.10, and it bites here under different names.
    pub idempotency_key: String,
    /// Which decider is appending.
    ///
    /// Checked against the lease rather than trusted: a worker whose lease was
    /// taken over is refused here instead of finding out from a 409 after it
    /// has already paid for the turn. It is not a credential — the route's own
    /// role check is — and it is required because a decider always has a name.
    pub holder: String,
    /// The worker's own messages, in the order it wrote them.
    pub messages: Vec<HostedMessage>,
}

impl HostedAppend {
    /// The inbox key this batch is recognised by on a redelivery.
    #[must_use]
    pub fn input_id(&self, execution: &ExecutionId) -> MessageId {
        MessageId::new(crate::derive_uuid(&format!(
            "hosted-append/{}/{}",
            execution.as_str(),
            self.idempotency_key
        )))
    }

    /// The id of the `index`th message in this batch.
    ///
    /// Names the execution, the batch and the position, so a redelivered batch
    /// lands on the ids it already wrote rather than beside them. Derived
    /// rather than generated for the same reason `TraceId::derive` is.
    #[must_use]
    pub fn message_id(&self, execution: &ExecutionId, index: usize) -> MessageId {
        MessageId::new(crate::derive_uuid(&format!(
            "hosted-message/{}/{}/{index}",
            execution.as_str(),
            self.idempotency_key
        )))
    }
}

/// Why a hosted append was refused.
#[derive(Debug, thiserror::Error)]
pub enum HostedError {
    /// The run this engine holds is not one a worker decides. Refused rather
    /// than accepted, because the messages would then sit in a stream whose
    /// decider is this process — two deciders on one run, which is the failure
    /// the mode exists to prevent.
    #[error(
        "`{execution}` is a compiled execution: this engine schedules its steps, \
         so a worker may not append to its history. Start it with mode=hosted"
    )]
    NotHosted { execution: String },

    /// An append of nothing at an expected version is a version check, and
    /// there is a read for that.
    #[error("an append carries at least one message")]
    Empty,

    #[error("an append carries at most {MAX_BATCH} messages, and this one carries {count}")]
    TooManyMessages { count: usize },

    /// Somebody else is deciding this run and has not run out of time.
    ///
    /// Refused *before* the work rather than after it: the append would very
    /// likely 409 anyway, but by then this worker has already made the model
    /// call the other one is making.
    #[error(
        "`{holder}` is deciding this execution until {expires_at}. \
         Wait for the lease to run out, or take it over once it has"
    )]
    LeaseHeld {
        holder: String,
        expires_at: OffsetDateTime,
    },

    #[error(transparent)]
    Handle(#[from] HandleError),
}

impl<S: WorkflowStore> ExecutionHandler<S> {
    /// Take, or renew, the right to decide one hosted run.
    ///
    /// Renewing is taking it again under the same name, so a heartbeat and a
    /// first claim are one call. The run has to exist and be hosted before
    /// anything is written: a lease on a compiled run would be a row nothing
    /// ever reads, and one on an execution nobody started would be a lease on
    /// an id somebody mistyped.
    ///
    /// # Errors
    ///
    /// [`HostedError::NotHosted`], [`DecisionError::NotStarted`],
    /// [`DecisionError::AlreadyFinished`], and whatever the store could not do.
    pub async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome, HostedError> {
        self.hosted_run(execution).await?;
        Ok(self
            .store()
            .take_decider_lease(execution, holder, now)
            .await
            .map_err(HandleError::from)?)
    }

    /// Give up the right to decide. `false` when it was not this holder's.
    ///
    /// Worth calling rather than waiting the lease out: it is the difference
    /// between a replacement starting now and starting in five minutes.
    ///
    /// # Errors
    ///
    /// As [`Self::take_decider_lease`].
    pub async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool, HostedError> {
        self.hosted_run(execution).await?;
        Ok(self
            .store()
            .release_decider_lease(execution, holder, now)
            .await
            .map_err(HandleError::from)?)
    }

    /// Who is deciding this run, if anybody still is.
    ///
    /// The read behind "the worker asks; the server answers": a claimant cannot
    /// tell whether it still holds a lease from a clock and a name it owns
    /// both of.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do.
    pub async fn decider_lease(
        &self,
        execution: &ExecutionId,
        now: OffsetDateTime,
    ) -> Result<Option<DeciderLease>, HostedError> {
        let lease = self
            .store()
            .decider_lease(execution)
            .await
            .map_err(HandleError::from)?;
        // Expired and never taken are the same answer to every caller: it is
        // free. The row stays so that a takeover can still name who it
        // interrupted.
        Ok(lease.filter(|lease| !lease.expired(now)))
    }

    /// Refuse when somebody else is deciding this run right now.
    async fn check_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<(), HostedError> {
        let Some(lease) = self.decider_lease(execution, now).await? else {
            // Nobody has taken one. Leasing is what a decider does to avoid
            // duplicated work, not a permission this route invents — and the
            // version check behind this is unaffected either way.
            return Ok(());
        };
        if lease.holder == holder {
            return Ok(());
        }
        Err(HostedError::LeaseHeld {
            expires_at: lease.expires_at(),
            holder: lease.holder,
        })
    }

    /// The run behind an id, once it is known to be one a worker may decide.
    async fn hosted_run(&self, execution: &ExecutionId) -> Result<(), HostedError> {
        let slice = self
            .store()
            .load(execution)
            .await
            .map_err(HandleError::from)?;
        let state = replay(slice.events());
        let Some(run) = state.active() else {
            return Err(HandleError::Decision(DecisionError::NotStarted {
                message: HOSTED_APPEND.to_owned(),
            })
            .into());
        };
        if run.mode != ExecutionMode::Hosted {
            return Err(HostedError::NotHosted {
                execution: execution.to_string(),
            });
        }
        if run.state.state_type.is_terminal() {
            return Err(HandleError::Decision(DecisionError::AlreadyFinished {
                state: run.state.state_type,
            })
            .into());
        }
        Ok(())
    }

    /// Append a worker's own messages to a hosted execution's history.
    ///
    /// Returns [`AppendOutcome::Duplicate`] for a redelivered batch, and the
    /// store's [`crate::error::StoreError::VersionConflict`] — the 409 — for one
    /// that lost the race. Never retries: see the module note.
    ///
    /// # Errors
    ///
    /// [`HostedError::NotHosted`] for a compiled run, [`HostedError::Empty`] and
    /// [`HostedError::TooManyMessages`] for a batch that is not one this accepts,
    /// [`DecisionError::NotStarted`] for a stream nothing has started,
    /// [`DecisionError::AlreadyFinished`] for a run that has ended, and whatever
    /// the store could not do.
    pub async fn append_hosted(
        &self,
        execution: &ExecutionId,
        append: HostedAppend,
        now: OffsetDateTime,
    ) -> Result<Handled, HostedError> {
        if append.messages.is_empty() {
            return Err(HostedError::Empty);
        }
        if append.messages.len() > MAX_BATCH {
            return Err(HostedError::TooManyMessages {
                count: append.messages.len(),
            });
        }

        let slice = self
            .store()
            .load(execution)
            .await
            .map_err(HandleError::from)?;
        let input_id = append.input_id(execution);

        // The inbox before anything else: a redelivery is not a race, and
        // appending again would give the worker's history a second copy of
        // every message in the batch.
        if let Some(seen) = slice
            .messages
            .iter()
            .find(|recorded| recorded.metadata.message_id == input_id)
        {
            let state = replay(slice.events());
            return Ok(Handled {
                outcome: AppendOutcome::Duplicate {
                    version: seen.stream_version,
                },
                projection: projection_of(execution, &state, slice.version),
                outbox: Vec::new(),
                duplicate: true,
            });
        }

        let state = replay(slice.events());
        let Some(run) = state.active() else {
            return Err(HandleError::Decision(DecisionError::NotStarted {
                message: HOSTED_APPEND.to_owned(),
            })
            .into());
        };
        if run.mode != ExecutionMode::Hosted {
            return Err(HostedError::NotHosted {
                execution: execution.to_string(),
            });
        }
        if run.state.state_type.is_terminal() {
            return Err(HandleError::Decision(DecisionError::AlreadyFinished {
                state: run.state.state_type,
            })
            .into());
        }
        // Not a second version check — the store's is the one that keeps the
        // history correct, and it runs whether or not anybody leased anything.
        // This is what turns "you lost, and you have already paid for the turn"
        // into "somebody else is doing this", which is the only form of that
        // news a worker can act on before it spends the money.
        self.check_decider_lease(execution, &append.holder, now)
            .await?;

        let metadata = MessageMetadata::caused_by(execution, &input_id, input_id.clone(), now);
        let outputs: Vec<PendingMessage> = append
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let id = append.message_id(execution, index);
                PendingMessage::output(
                    WorkflowMessage::Hosted(message.clone()),
                    MessageMetadata::caused_by(execution, &input_id, id, now),
                )
            })
            .collect();

        // The state is unchanged: none of these are events this engine folds,
        // which is [`WorkflowMessage::event`] answering `None` and not a rule
        // written here a second time. The version moves, and that is what the
        // worker's next expected version reads.
        let version = slice.version + 1 + outputs.len() as u64;
        let projection = projection_of(execution, &state, version);

        let request = AppendRequest {
            expected_version: ExpectedVersion::Exact(append.expected_version),
            input: PendingMessage::input(
                WorkflowMessage::Hosted(HostedMessage {
                    message_type: HOSTED_APPEND.to_owned(),
                    metadata: serde_json::json!({ "messages": outputs.len() }),
                    payload: None,
                }),
                metadata,
            ),
            outputs,
            projection: projection.clone(),
            // Facts about a hosted run reach the log when the worker's own
            // messages are mapped to them, which is its own step with its own
            // gate. An outbox row invented here would publish a `step.*` this
            // engine cannot honestly describe.
            outbox: Vec::new(),
            checkpoint: None,
            // A hosted run has no claimable attempts: the worker schedules its
            // own next node.
            attempts: Vec::new(),
        };
        request.check_payloads().map_err(HandleError::from)?;

        let outcome = self
            .store()
            .append(execution, request)
            .await
            .map_err(HandleError::from)?;
        Ok(Handled {
            duplicate: outcome.is_duplicate(),
            outcome,
            projection,
            outbox: Vec::new(),
        })
    }
}
