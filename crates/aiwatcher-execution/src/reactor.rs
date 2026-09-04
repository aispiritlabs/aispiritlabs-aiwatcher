//! Claim an attempt, run it, report what happened.
//!
//! Steps 1, 2, 7 and 8 of section 14 — the four that are the same for every
//! runtime, so an [`ActivityExecutor`] only has to own 3 to 6.
//!
//! ```text
//!   claim ──► load the plan ──► lookup? ──► execute ──► still holding? ──► report
//!     │                            │                          │
//!     └── nothing to do            └── the timeout case        └── no: stop, silently
//! ```
//!
//! Three rules carry it, and each is a failure somebody would otherwise meet in
//! production.
//!
//! **Re-check the lease before writing.** A worker that lost its claim under it
//! has been taken over, and reporting anyway would put a completion beside its
//! replacement's — the same rule ADR_0022 keeps for an export shard, and the
//! same reason. The work is lost; the alternative is a stream with two answers
//! for one attempt.
//!
//! **A takeover asks before it runs.** A claim whose row already had an owner
//! is an attempt somebody else started, and a timeout proves nothing about
//! whether their call finished. [`ActivityExecutor::lookup`] is asked first,
//! and only `Absent` justifies running it again.
//!
//! **The reactor never decides.** It reports `StepStarted`, `StepCompleted` or
//! `StepFailed` through [`ExecutionHandler`] and the decider works out what
//! follows — the retry, the next step, the end of the run. A reactor that
//! scheduled its own retry would be the second orchestrator ADR_0025 refuses.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_core::{CausationId, CorrelationId, MessageId};
use time::OffsetDateTime;

use crate::activity::{
    ActivityCommand, ActivityContext, ActivityExecutor, ActivityResult, ExecutorRegistry,
    PriorAttempt,
};
use crate::claim::AttemptRow;
use crate::decide::{Now, replay};
use crate::handler::{ExecutionHandler, HandleError};
use crate::message::{MessageMetadata, SCHEMA_VERSION, WorkflowEvent, WorkflowMessage};
use crate::state::{Execution, ExecutionId, FailureClass, InputRequest, StepError};
use crate::store::WorkflowStore;

/// What one poll did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Performed {
    /// Nothing was claimable. The caller waits before asking again.
    Idle,
    /// An attempt ran and its outcome was reported.
    Reported {
        step_id: String,
        attempt: u32,
        succeeded: bool,
    },
    /// An attempt ran and the lease was gone before it could be reported. The
    /// work is lost on purpose — see the module docs.
    LeaseLost { step_id: String, attempt: u32 },
    /// An attempt was claimed for a runtime this process does not hold, or for
    /// a plan it could not read. Released rather than failed: the attempt is
    /// somebody else's to run, and failing it would end a run over a
    /// misconfiguration here.
    Released { step_id: String, reason: String },
}

/// One process's reactors: what it can run, and the name it holds leases under.
#[derive(Debug)]
pub struct Reactor<S> {
    handler: ExecutionHandler<S>,
    executors: ExecutorRegistry,
    owner: String,
}

impl<S: WorkflowStore> Reactor<S> {
    /// A reactor holding `executors`, leasing under `owner`.
    ///
    /// `owner` has to be unique per process — a pod name, a host and a pid. Two
    /// reactors sharing one name would each believe they hold the other's
    /// leases, which is the one thing the lease exists to prevent.
    #[must_use]
    pub fn new(handler: ExecutionHandler<S>, executors: ExecutorRegistry, owner: String) -> Self {
        Self {
            handler,
            executors,
            owner,
        }
    }

    #[must_use]
    pub const fn handler(&self) -> &ExecutionHandler<S> {
        &self.handler
    }

    /// Claim one attempt and carry it out, or report that there was none.
    ///
    /// One attempt per call, so the caller owns the pacing and the shutdown.
    /// A loop inside here would be a process that cannot be drained.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do. An executor's failure is not an error
    /// here — it is a `StepFailed` the decider acts on.
    pub async fn poll_once(&self, now: OffsetDateTime) -> Result<Performed, HandleError> {
        if self.executors.is_empty() {
            return Ok(Performed::Idle);
        }
        // 1–2: the claim is the deduplication and the lease at once. A second
        // reactor polling this instant sees the row taken.
        let Some(row) = self
            .handler
            .store()
            .claim_attempt(&self.executors.claim_filter(), &self.owner, now)
            .await?
        else {
            return Ok(Performed::Idle);
        };

        let execution = row.key.execution_id.clone();
        let Some(run) = self.load_execution(&execution).await? else {
            return Ok(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: "the execution's stream holds no plan".to_owned(),
            });
        };
        let Some(step) = run.plan.step(&row.key.step_id).cloned() else {
            return Ok(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: "the plan has no such step".to_owned(),
            });
        };
        let Some(executor) = self.executors.get(step.runtime.kind()) else {
            return Ok(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: format!(
                    "this process holds no {} executor",
                    step.runtime.kind().as_str()
                ),
            });
        };

        let command = ActivityCommand {
            key: row.key.clone(),
            command_id: row.command_id.clone(),
            step,
            inputs: inputs_for(&run, &row.key.step_id),
            parameters: run.input.clone(),
        };
        let context = ActivityContext {
            owner: self.owner.clone(),
            timeout: Duration::from_secs(command.step.timeout_seconds),
            context_id: command.idempotency_key(),
        };

        self.report(
            &execution,
            WorkflowEvent::StepStarted {
                step_id: row.key.step_id.clone(),
                attempt: row.key.attempt,
            },
            now,
            "started",
        )
        .await?;

        let outcome = self.perform(executor, &command, &context, &row).await;

        // 7's precondition. A worker whose lease expired under it stops rather
        // than writing beside its replacement.
        if !self
            .handler
            .store()
            .heartbeat(&row.key, &self.owner, now)
            .await?
        {
            return Ok(Performed::LeaseLost {
                step_id: row.key.step_id.clone(),
                attempt: row.key.attempt,
            });
        }

        let succeeded = outcome.is_ok();
        let event = match outcome {
            Ok(result) => completion(&row, result),
            Err(error) => WorkflowEvent::StepFailed {
                step_id: row.key.step_id.clone(),
                attempt: row.key.attempt,
                error,
            },
        };
        // 7–8: the fact goes into the workflow, and the outbox that publishes
        // it to the log is written in the same transaction. Nothing here
        // publishes; the ordering is ADR_0026's.
        self.report(&execution, event, now, "outcome").await?;

        Ok(Performed::Reported {
            step_id: row.key.step_id,
            attempt: row.key.attempt,
            succeeded,
        })
    }

    /// Steps 3–6, with the takeover's question in front of them.
    async fn perform(
        &self,
        executor: &Arc<dyn ActivityExecutor>,
        command: &ActivityCommand,
        context: &ActivityContext,
        row: &AttemptRow,
    ) -> Result<ActivityResult, StepError> {
        // A row with a *previous* owner is an attempt somebody else started.
        // Their call may have finished after their lease expired, and running
        // it again would be the duplicate side effect the key exists to
        // prevent. `lease_owner` cannot answer this: by the time the row gets
        // here it already names this reactor.
        if row.previous_owner.is_some() {
            match executor.lookup(command).await {
                Ok(PriorAttempt::Done(result)) => return Ok(*result),
                Ok(PriorAttempt::Running) => {
                    return Err(StepError::new(
                        FailureClass::Transient,
                        "the runtime is still working on the previous attempt",
                    ));
                }
                Ok(PriorAttempt::Absent) => {}
                // Being unable to *ask* is not being told no. Retried, because
                // the alternative is running work that may already be done.
                Err(error) => return Err(error.as_step_error()),
            }
        }

        executor
            .execute(command, context)
            .await
            .map_err(|error| error.as_step_error())
    }

    async fn load_execution(
        &self,
        execution: &ExecutionId,
    ) -> Result<Option<Execution>, HandleError> {
        let slice = self.handler.store().load(execution).await?;
        Ok(replay(slice.events()).active().cloned())
    }

    async fn report(
        &self,
        execution: &ExecutionId,
        event: WorkflowEvent,
        now: OffsetDateTime,
        what: &str,
    ) -> Result<(), HandleError> {
        // Derived, so a reactor that crashed between the runtime's answer and
        // this call reports the same message id on its next pass — which the
        // inbox absorbs rather than deciding twice.
        let message_id = MessageId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/report/{execution}/{}/{what}",
            event.name()
        )));
        let metadata = MessageMetadata {
            schema_version: SCHEMA_VERSION,
            message_id,
            occurred_at: now,
            correlation_id: CorrelationId::new(execution.as_str()),
            causation_id: CausationId::new(execution.as_str()),
            trace_id: None,
            span_id: None,
            step_id: None,
            attempt: None,
        };
        match self
            .handler
            .handle(
                execution,
                WorkflowMessage::Event(event),
                metadata,
                Now::at(now),
            )
            .await
        {
            Ok(_) => Ok(()),
            // A report the decider will not accept is one it already has, or
            // one about an attempt that was taken over. Neither is this
            // reactor's to resolve, and neither should stop its loop.
            Err(HandleError::Decision(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// What a completed attempt reports, including the case where it did not
/// complete at all.
fn completion(row: &AttemptRow, result: ActivityResult) -> WorkflowEvent {
    if let Some(request) = result.awaiting {
        return awaiting(row, request);
    }
    WorkflowEvent::StepCompleted {
        step_id: row.key.step_id.clone(),
        attempt: row.key.attempt,
        outputs: result.outputs,
        result: result.result,
    }
}

fn awaiting(row: &AttemptRow, request: InputRequest) -> WorkflowEvent {
    WorkflowEvent::InputRequested {
        step_id: row.key.step_id.clone(),
        attempt: row.key.attempt,
        request,
    }
}

/// What this step reads, resolved from the steps that fed it.
///
/// The plan's `InputBinding` names a step and an output; the state holds what
/// that step actually produced. Reading it from the state rather than from a
/// catalog is what makes a retry reuse the *pinned* artifacts of its context
/// rather than whatever is newest.
fn inputs_for(run: &Execution, step_id: &str) -> Vec<aiwatcher_core::ArtifactRef> {
    let Some(step) = run.plan.step(step_id) else {
        return Vec::new();
    };
    step.inputs
        .iter()
        .filter_map(|binding| match binding {
            crate::plan::InputBinding::Step { step, output } => {
                let produced = run.step(step)?;
                produced
                    .outputs
                    .iter()
                    .find(|artifact| &artifact.name == output)
                    .cloned()
            }
            crate::plan::InputBinding::Parameter { .. } => None,
        })
        .collect()
}
