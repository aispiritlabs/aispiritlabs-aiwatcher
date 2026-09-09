//! The append side of a hosted execution.
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
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::decide::{Now, replay};
use crate::error::DecisionError;
use crate::handler::{ExecutionHandler, HandleError, Handled, projection_of};
use crate::message::{
    HostedMessage, MessageMetadata, PayloadPolicy, PendingMessage, WorkflowCommand, WorkflowMessage,
};
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

/// A message a hosted decider asked this engine to append later.
///
/// The one *active* thing this engine does for a hosted run, and it is
/// deliberately the smallest possible active thing: a deferred append.
/// The worker composes the message when it schedules the timer, and the engine
/// stores it and hands it back at the time — so "the engine does not interpret
/// the messages" survives intact. It is not reading a stream to work out that
/// something is due; it was told, explicitly, in a row.
///
/// ## Why a row rather than the stream
///
/// `agentic.workflow.Saga` already keeps its timers *in* the stream, as
/// `saga.timeout_scheduled` events, and works out what is due by folding it.
/// That fold is the worker's and stays the worker's. What it cannot do from
/// there is *notice*: nothing wakes up and looks. A row is what an index over
/// every hosted run can be built on, so one tick finds what is due across all
/// of them without reading a single stream.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct Timer {
    pub execution: ExecutionId,
    /// The worker's own id, unique inside one execution. `agentic`'s
    /// `timeout_id`, and the reason a repeated schedule is one timer.
    pub timer_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub due_at: OffsetDateTime,
    /// What to append when it comes due. Composed by the worker at the moment
    /// it schedules, never assembled here — an engine that built this would be
    /// deciding what a timeout means.
    pub message: HostedMessage,
}

impl Timer {
    /// The id the fired message is recorded under.
    ///
    /// Derived from the execution and the timer, so two ticks that both find it
    /// due converge on one inbox entry rather than appending twice. `agentic`'s
    /// own `saga-timeout:{name}:{saga}:{timeout}` reasoning, in this store's
    /// vocabulary, and the rule that an id names everything it identifies: one
    /// derived from less than that reads as a redelivery of something else.
    #[must_use]
    pub fn fired_id(&self) -> MessageId {
        MessageId::new(crate::derive_uuid(&format!(
            "hosted-timer/{}/{}",
            self.execution.as_str(),
            self.timer_id
        )))
    }

    #[must_use]
    pub fn is_due(&self, now: OffsetDateTime) -> bool {
        self.due_at <= now
    }
}

/// What one decision does to the timer table.
///
/// Applied in the same transaction as the decision that authorised it, for
/// [`crate::claim::AttemptWrite`]'s reason: a timer scheduled by anything other
/// than a decision would be work nobody decided on, and one retired outside the
/// append that fired it could fire twice.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum TimerWrite {
    /// Set it, or move an existing one. Scheduling the same `timer_id` twice is
    /// one timer, which is what makes a decider's retry safe.
    Schedule(Timer),
    /// Withdraw it. A no-op when there is none, because a saga that cancels a
    /// timeout it already handled is doing the ordinary thing.
    Cancel(String),
    /// It has fired. Written *with* the message it produced, so "fired once" is
    /// a property of the transaction rather than of a lease somebody renews.
    Fire(String),
}

impl TimerWrite {
    #[must_use]
    pub fn timer_id(&self) -> &str {
        match self {
            Self::Schedule(timer) => &timer.timer_id,
            Self::Cancel(id) | Self::Fire(id) => id,
        }
    }
}

/// The most timers one tick fires in a pass.
///
/// A bound rather than a batch size: every one of these is an append, and a
/// thousand due at once must not hold the loop that also has to notice the next
/// thousand. What is left over is due on the next pass, which is what makes
/// this an operational choice rather than a correctness one — the scheduler
/// tick's rule, in a second loop.
pub const TIMERS_PER_TICK: usize = 100;

/// The message type a gate's deadline is stored under.
///
/// A hosted timer carries the message a worker composed and this one carries
/// nothing to hand back: what a lapsed deadline *means* is on the plan, which
/// is what pinned the policy. The type is here so a row read out of the table
/// says which of the two it is without anybody guessing from its id.
pub const DEADLINE_TIMER: &str = "input_deadline";

/// The type name of the row that records one append.
///
/// A hosted append is stored the way every other decision is: the input that
/// caused it beside the outputs it produced, at the version it was accepted at.
/// The input is this marker, and its `message_id` is the caller's
/// `Idempotency-Key` — which is what makes the durable inbox work for a hosted
/// run without a second dedup table.
pub const HOSTED_APPEND: &str = "HostedAppend";

/// The type name of the row that records one timer coming due.
///
/// The *input* of that decision, exactly as [`HOSTED_APPEND`] is the input of a
/// worker's. What it produces is the message the worker composed when it
/// scheduled the timer, unchanged — this engine defers an append and composes
/// nothing.
pub const HOSTED_TIMER: &str = "HostedTimerFired";

/// What firing one timer did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fired {
    pub execution: ExecutionId,
    pub timer_id: String,
    /// `false` when the run had already ended, or was never a hosted one. The
    /// timer is retired either way — a row nothing can deliver would otherwise
    /// come back due on every tick for ever — and the difference is worth
    /// reporting rather than counting as a delivery.
    pub delivered: bool,
}

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
    /// becomes an id. A key naming less than what it identifies collides with
    /// a different intention, and it bites here under several names.
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
    /// Deferred appends this decision sets or withdraws.
    ///
    /// In the same append rather than beside it, because a decision that
    /// schedules a timeout and one that records having scheduled it are the
    /// same decision — split in two, a crash between them leaves either a timer
    /// nobody decided on or a decision the timer never happened for.
    pub timers: Vec<TimerWrite>,
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

/// Refuse a message whose payload is not governed the way the run is.
fn check_payload(message: &HostedMessage, expected: PayloadPolicy) -> Result<(), HostedError> {
    // A message with no payload carries no words, so there is nothing for a
    // policy to govern — a join bucket and a timeout are exactly that, and
    // demanding a policy of them would be demanding one of silence.
    let Some(payload) = &message.payload else {
        return Ok(());
    };
    if payload.policy == expected {
        return Ok(());
    }
    Err(HostedError::PayloadPolicyMismatch {
        timer_or_message: message.message_type.clone(),
        found: payload.policy.as_str(),
        expected: expected.as_str(),
    })
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

    /// A message whose payload is not governed the way the run is.
    ///
    /// Refused rather than corrected. A run started `sealed` and appending
    /// `external` references is one whose words are somewhere this instance
    /// cannot reach, and quietly accepting them would make the policy a label
    /// on a page rather than a rule — which is the silent downgrade the whole
    /// of it exists to prevent.
    #[error(
        "`{timer_or_message}` carries a {found} payload and this run was started \
         under {expected}. A run's policy is fixed when it starts"
    )]
    PayloadPolicyMismatch {
        timer_or_message: String,
        found: &'static str,
        expected: &'static str,
    },

    #[error(transparent)]
    Handle(#[from] HandleError),
}

impl<S: WorkflowStore> ExecutionHandler<S> {
    /// Append every timer that has come due, and retire it in the same
    /// transaction.
    ///
    /// Fired **once**, and neither half of that is a lease. The message is
    /// recorded under [`Timer::fired_id`], so two ticks that both find it due
    /// land on one inbox entry rather than beside each other; and the append
    /// that delivers it carries [`TimerWrite::Fire`], so delivery and
    /// retirement are one transaction — split in two, a crash between them
    /// fires twice.
    ///
    /// A conflict is not retried here: the timer is still due and the next tick
    /// has it. One tick late is an operational cost; twice is a bug.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do while *reading* what is due. A single
    /// timer that cannot be fired is reported and skipped, because one bad row
    /// must not stop the loop that delivers every other run's.
    pub async fn fire_due_timers(
        &self,
        now: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<Fired>, HostedError> {
        let due = self
            .store()
            .due_timers(now, limit)
            .await
            .map_err(HandleError::from)?;
        let mut fired = Vec::new();
        for timer in due {
            match self.fire(&timer, now).await {
                Ok(outcome) => fired.push(outcome),
                // Not fatal to the pass. A stream that could not be read is one
                // run's problem, and the next tick has this timer again.
                Err(error) => tracing::warn!(
                    execution_id = %timer.execution,
                    timer_id = %timer.timer_id,
                    %error,
                    "a due timer could not be fired"
                ),
            }
        }
        Ok(fired)
    }

    async fn fire(&self, timer: &Timer, now: OffsetDateTime) -> Result<Fired, HostedError> {
        // A gate's deadline is the engine's own, and the engine decides what it
        // means: the command goes through `decide` like every other, so the
        // step's `on_timeout` is read from the plan that pinned it. Retiring
        // the row is that decision's own consequence — the handler derives a
        // `Cancel` from the step ending — so there is no second write to keep
        // in step with it.
        if timer.message.message_type == DEADLINE_TIMER {
            return self.fire_deadline(timer, now).await;
        }
        let slice = self
            .store()
            .load(&timer.execution)
            .await
            .map_err(HandleError::from)?;
        let state = replay(slice.events());
        // A run that has ended, or one that was never hosted, cannot be handed
        // a message — and the timer has to go anyway, or it is due on every
        // tick for ever. Retired with no output, which leaves the reason in the
        // stream rather than in a log line nobody reads.
        let deliverable = state.active().is_some_and(|run| {
            run.mode == ExecutionMode::Hosted && !run.state.state_type.is_terminal()
        });

        let input_id = timer.fired_id();
        let outputs = if deliverable {
            vec![PendingMessage::output(
                WorkflowMessage::Hosted(timer.message.clone()),
                MessageMetadata::caused_by(
                    &timer.execution,
                    &input_id,
                    MessageId::new(crate::derive_uuid(&format!(
                        "hosted-timer-message/{}/{}",
                        timer.execution.as_str(),
                        timer.timer_id
                    ))),
                    now,
                ),
            )]
        } else {
            Vec::new()
        };

        let version = slice.version + 1 + outputs.len() as u64;
        let request = AppendRequest {
            expected_version: ExpectedVersion::Exact(slice.version),
            input: PendingMessage::input(
                WorkflowMessage::Hosted(HostedMessage {
                    message_type: HOSTED_TIMER.to_owned(),
                    metadata: serde_json::json!({
                        "timer_id": timer.timer_id,
                        "delivered": deliverable,
                    }),
                    payload: None,
                }),
                MessageMetadata::caused_by(&timer.execution, &input_id, input_id.clone(), now),
            ),
            outputs,
            projection: projection_of(&timer.execution, &state, version),
            outbox: Vec::new(),
            checkpoint: None,
            timers: vec![TimerWrite::Fire(timer.timer_id.clone())],
            attempts: Vec::new(),
        };
        self.store()
            .append(&timer.execution, request)
            .await
            .map_err(HandleError::from)?;
        Ok(Fired {
            execution: timer.execution.clone(),
            timer_id: timer.timer_id.clone(),
            delivered: deliverable,
        })
    }

    /// A question whose deadline ran out, handed to the decider.
    ///
    /// The refusals are the two ways a timer outlives its question — somebody
    /// answered, or a retry replaced the attempt that asked — and both retire
    /// the row rather than leaving it due for ever. Neither is an error worth
    /// reporting: a timer racing an answer is the ordinary thing, and the
    /// answer won.
    async fn fire_deadline(
        &self,
        timer: &Timer,
        now: OffsetDateTime,
    ) -> Result<Fired, HostedError> {
        let (Some(step_id), Some(attempt)) = (
            timer
                .message
                .metadata
                .get("step_id")
                .and_then(Value::as_str),
            timer
                .message
                .metadata
                .get("attempt")
                .and_then(Value::as_u64),
        ) else {
            self.retire(timer, now).await?;
            return Ok(Fired {
                execution: timer.execution.clone(),
                timer_id: timer.timer_id.clone(),
                delivered: false,
            });
        };

        let input = WorkflowMessage::Command(WorkflowCommand::TimeoutInput {
            step_id: step_id.to_owned(),
            attempt: attempt as u32,
        });
        let input_id = timer.fired_id();
        let handled = self
            .deliver(
                &timer.execution,
                input,
                MessageMetadata::caused_by(&timer.execution, &input_id, input_id.clone(), now),
                Now::at(now),
            )
            .await;

        match handled {
            Ok(_) => Ok(Fired {
                execution: timer.execution.clone(),
                timer_id: timer.timer_id.clone(),
                delivered: true,
            }),
            Err(HandleError::Decision(_)) => {
                self.retire(timer, now).await?;
                Ok(Fired {
                    execution: timer.execution.clone(),
                    timer_id: timer.timer_id.clone(),
                    delivered: false,
                })
            }
            Err(error) => Err(HostedError::Handle(error)),
        }
    }

    /// Retire a timer whose question is over, and say so in the stream.
    ///
    /// Recorded rather than quietly deleted, for the reason the hosted path
    /// records a lapse: a row that fires on every tick for ever is worse than
    /// one forgotten, and *why* it was forgotten is a fact about this run. No
    /// outputs, because nothing follows from it — whatever ended the question
    /// already did what there was to do.
    async fn retire(&self, timer: &Timer, now: OffsetDateTime) -> Result<(), HostedError> {
        let slice = self
            .store()
            .load(&timer.execution)
            .await
            .map_err(HandleError::from)?;
        let state = replay(slice.events());
        let input_id = MessageId::new(crate::derive_uuid(&format!(
            "input-deadline-lapsed/{}/{}",
            timer.execution.as_str(),
            timer.timer_id
        )));
        let request = AppendRequest {
            expected_version: ExpectedVersion::Exact(slice.version),
            input: PendingMessage::input(
                WorkflowMessage::Hosted(HostedMessage {
                    message_type: DEADLINE_TIMER.to_owned(),
                    metadata: serde_json::json!({
                        "timer_id": timer.timer_id,
                        "delivered": false,
                    }),
                    payload: None,
                }),
                MessageMetadata::caused_by(&timer.execution, &input_id, input_id.clone(), now),
            ),
            outputs: Vec::new(),
            projection: projection_of(&timer.execution, &state, slice.version + 1),
            outbox: Vec::new(),
            checkpoint: None,
            timers: vec![TimerWrite::Fire(timer.timer_id.clone())],
            attempts: Vec::new(),
        };
        self.store()
            .append(&timer.execution, request)
            .await
            .map_err(HandleError::from)?;
        Ok(())
    }

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
        // The run's policy is fixed when it starts, and every message is
        // checked against it here rather than trusted: a graph whose words were
        // meant to be sealed and are sitting in a worker's directory instead is
        // a fact nothing downstream can recover.
        for message in append
            .messages
            .iter()
            .chain(append.timers.iter().filter_map(|write| match write {
                TimerWrite::Schedule(timer) => Some(&timer.message),
                TimerWrite::Cancel(_) | TimerWrite::Fire(_) => None,
            }))
        {
            check_payload(message, run.payloads)?;
        }

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
            timers: append.timers,
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
