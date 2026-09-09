//! Claim an attempt, run it, report what happened.
//!
//! Steps 1, 2, 7 and 8 of [`crate::activity`]'s eight — the four that are the
//! same for every runtime, so an [`ActivityExecutor`] only owns 3 to 6.
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

/// One claimed attempt, and everything performing it needs.
///
/// What [`Reactor::take`] hands out and [`Reactor::settle`] takes back. The
/// lease is live while a caller holds one of these, so it is not something to
/// keep: a worker that put one in a queue would be holding a claim nothing is
/// renewing.
#[derive(Debug)]
pub struct Claimed {
    pub row: AttemptRow,
    pub command: ActivityCommand,
    pub context: ActivityContext,
    /// The key this result may answer for later, if it may answer for one.
    ///
    /// Carried across the seam rather than recomputed at settlement, because
    /// it is read from the *decision* that dispatched the attempt and a second
    /// read could see a plan that has since been superseded.
    pub(crate) cache_key: Option<String>,
}

/// What one claim found.
#[derive(Debug)]
pub enum Taken {
    /// Nothing was claimable.
    Idle,
    /// A row was claimed and is already finished with — a cache hit, or a row
    /// this claimant released. Nobody performs anything.
    Settled(Performed),
    /// Work for the caller to perform, and then to [`Reactor::settle`].
    Work(Box<Claimed>),
}

/// One process's reactors: what it can run, and the name it holds leases under.
#[derive(Debug)]
pub struct Reactor<S> {
    handler: ExecutionHandler<S>,
    executors: ExecutorRegistry,
    owner: String,
    /// Where a hit is looked up and a result is recorded. `None` runs
    /// everything and remembers nothing, which is a working state and the one a
    /// deployment with no object store is in: deleting the index never loses
    /// an authoritative result, taken to its limit.
    catalog: Option<Arc<dyn crate::ArtifactCatalog>>,
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
            catalog: None,
        }
    }

    /// Give it somewhere to look a hit up and record a result.
    #[must_use]
    pub fn with_catalog(mut self, catalog: Arc<dyn crate::ArtifactCatalog>) -> Self {
        self.catalog = Some(catalog);
        self
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
    /// [`Self::take`] and [`Self::settle`] are the two halves, and this is the
    /// only caller that has an [`ActivityExecutor`] to put between them.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do. An executor's failure is not an error
    /// here — it is a `StepFailed` the decider acts on.
    pub async fn poll_once(&self, now: OffsetDateTime) -> Result<Performed, HandleError> {
        if self.executors.is_empty() {
            return Ok(Performed::Idle);
        }
        let performable: Vec<crate::plan::RuntimeKind> = self.executors.runtimes();
        let claimed = match self
            .take(&self.executors.claim_filter(), &performable, now)
            .await?
        {
            Taken::Idle => return Ok(Performed::Idle),
            Taken::Settled(performed) => return Ok(performed),
            Taken::Work(claimed) => claimed,
        };

        // Defensive rather than reachable: the filter was built from this
        // registry, so a row it handed back is one this process registered an
        // executor for. `take` has already checked the same list.
        let Some(executor) = self.executors.get(claimed.command.step.runtime.kind()) else {
            return Ok(Performed::Released {
                step_id: claimed.row.key.step_id.clone(),
                reason: format!(
                    "this process holds no {} executor",
                    claimed.command.step.runtime.kind().as_str()
                ),
            });
        };

        let outcome = self
            .perform(executor, &claimed.command, &claimed.context, &claimed.row)
            .await;
        self.settle(*claimed, outcome, now).await
    }

    /// Steps 1 and 2: claim an attempt, and get it as far as somebody can
    /// perform it.
    ///
    /// The half of [`Self::poll_once`] that runs **before** anybody does the
    /// work, and it is public because a worker's is done in another process.
    /// What it decides here rather than there is everything that must not be
    /// decided twice: which row, whether the plan still holds the step, whether
    /// a cache entry already answers, and the `step.started` that says an
    /// attempt began. A worker re-implementing any of it in another language
    /// is the drift this seam exists to prevent.
    ///
    /// `performable` is what the caller can actually run. It is separate from
    /// the filter because a filter is a *query* and this is the claimant's own
    /// answer about the row that came back — and it is checked before
    /// `step.started`, so a released attempt never leaves a start on the log.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do.
    pub async fn take(
        &self,
        filter: &crate::claim::ClaimFilter,
        performable: &[crate::plan::RuntimeKind],
        now: OffsetDateTime,
    ) -> Result<Taken, HandleError> {
        // 1–2: the claim is the deduplication and the lease at once. A second
        // reactor polling this instant sees the row taken.
        let Some(row) = self
            .handler
            .store()
            .claim_attempt(filter, &self.owner, now)
            .await?
        else {
            return Ok(Taken::Idle);
        };

        let execution = row.key.execution_id.clone();
        let Some(run) = self.load_execution(&execution).await? else {
            return Ok(Taken::Settled(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: "the execution's stream holds no plan".to_owned(),
            }));
        };
        let Some(step) = run.plan.step(&row.key.step_id).cloned() else {
            return Ok(Taken::Settled(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: "the plan has no such step".to_owned(),
            }));
        };
        if !performable.contains(&step.runtime.kind()) {
            return Ok(Taken::Settled(Performed::Released {
                step_id: row.key.step_id.clone(),
                reason: format!(
                    "this claimant does not perform {}",
                    step.runtime.kind().as_str()
                ),
            }));
        }

        let command = ActivityCommand {
            key: row.key.clone(),
            command_id: row.command_id.clone(),
            step,
            inputs: run.resolved_inputs(&row.key.step_id),
            parameters: run.input.clone(),
        };
        let context = ActivityContext {
            owner: self.owner.clone(),
            timeout: Duration::from_secs(command.step.timeout_seconds),
            context_id: command.idempotency_key(),
            // The plan this run pinned, not the definition's current head: an
            // attempt reads what its own execution was compiled from.
            plan: Arc::new(run.plan.clone()),
        };

        // Before `step.started`, because a hit is not a start: there was no
        // attempt at the runtime, and a zero-duration bar in the waterfall
        // would be claiming there was. The key was computed by the decider,
        // which may not read an index; this is the read.
        let cache_key = run
            .step(&row.key.step_id)
            .and_then(|step| step.cache_key.clone());
        if let Some(hit) = self.cached(cache_key.as_deref(), now).await {
            self.report(
                &row.key,
                WorkflowEvent::StepCacheHit {
                    step_id: row.key.step_id.clone(),
                    attempt: row.key.attempt,
                    cache_key: hit.cache_key,
                    outputs: hit.artifacts,
                },
                now,
                "cache-hit",
            )
            .await?;
            return Ok(Taken::Settled(Performed::Reported {
                step_id: row.key.step_id,
                attempt: row.key.attempt,
                succeeded: true,
            }));
        }

        self.report(
            &row.key,
            WorkflowEvent::StepStarted {
                step_id: row.key.step_id.clone(),
                attempt: row.key.attempt,
            },
            now,
            "started",
        )
        .await?;

        Ok(Taken::Work(Box::new(Claimed {
            row,
            command,
            context,
            cache_key,
        })))
    }

    /// Rebuild a claim this owner still holds, without claiming anything.
    ///
    /// The seam's third method, and it exists because an HTTP claimant does not
    /// keep a [`Claimed`] across requests — the assignment went out over the
    /// wire and the result comes back in a different one. `None` is a caller
    /// asking about an attempt it does not hold: an expired lease, a takeover,
    /// or an attempt that was never dispatched.
    ///
    /// It re-reads rather than trusting the key, which is the point. A worker
    /// naming somebody else's attempt gets `None` here rather than the ability
    /// to settle it, and that one check is what makes every other worker route
    /// safe to expose — the lease *is* the authorization.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do.
    pub async fn resume(
        &self,
        key: &crate::claim::AttemptKey,
        now: OffsetDateTime,
    ) -> Result<Option<Claimed>, HandleError> {
        let Some(row) = self.handler.store().attempt(key).await? else {
            return Ok(None);
        };
        if !row.is_held_by(&self.owner, now) {
            return Ok(None);
        }
        let Some(run) = self.load_execution(&key.execution_id).await? else {
            return Ok(None);
        };
        let Some(step) = run.plan.step(&key.step_id).cloned() else {
            return Ok(None);
        };

        let command = ActivityCommand {
            key: row.key.clone(),
            command_id: row.command_id.clone(),
            step,
            inputs: run.resolved_inputs(&key.step_id),
            parameters: run.input.clone(),
        };
        let context = ActivityContext {
            owner: self.owner.clone(),
            timeout: Duration::from_secs(command.step.timeout_seconds),
            context_id: command.idempotency_key(),
            plan: Arc::new(run.plan.clone()),
        };
        let cache_key = run
            .step(&key.step_id)
            .and_then(|step| step.cache_key.clone());

        Ok(Some(Claimed {
            row,
            command,
            context,
            cache_key,
        }))
    }

    /// Steps 7 and 8: check the lease still holds, record what was produced,
    /// and report the outcome.
    ///
    /// The half of [`Self::poll_once`] that runs **after** the work, and the
    /// reason a worker cannot simply post its own result: the lease re-check
    /// is a precondition a claimant cannot be trusted to apply to itself, and
    /// the cache entry must not be written by whoever benefits from it.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do.
    pub async fn settle(
        &self,
        claimed: Claimed,
        outcome: Result<ActivityResult, StepError>,
        now: OffsetDateTime,
    ) -> Result<Performed, HandleError> {
        let Claimed {
            row,
            command,
            cache_key,
            ..
        } = claimed;

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

        // Recorded before the completion is reported, and only for work that
        // actually ran. A manifest written after the fact could be lost by a
        // crash that the completion survived, which would leave an artifact on
        // the log that the catalog cannot describe — and a cache entry written
        // *before* the outputs exist would be a hit resolving to nothing.
        if let Ok(result) = &outcome
            && result.awaiting.is_none()
        {
            // The key is remembered only when the executor says the work ran
            // under the conditions the key assumes. A Flow step whose plan
            // pinned a span against a service that narrowed it produced correct
            // rows for a *different* question, and storing them here would
            // serve them to the one that was asked.
            let remember = result.cacheable.then_some(cache_key.as_deref()).flatten();
            self.record(&row, &command.inputs, &result.outputs, remember, now)
                .await;
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
        self.report(&row.key, event, now, "outcome").await?;

        Ok(Performed::Reported {
            step_id: row.key.step_id,
            attempt: row.key.attempt,
            succeeded,
        })
    }

    /// A usable entry for this key, or nothing.
    ///
    /// A catalog that could not be read answers `None` and the work is done
    /// again, which is what is meant by the index losing nothing
    /// authoritative. It is logged rather than returned: a cache being down is
    /// not a reason to fail a step.
    async fn cached(
        &self,
        cache_key: Option<&str>,
        now: OffsetDateTime,
    ) -> Option<crate::CacheEntry> {
        let (catalog, cache_key) = (self.catalog.as_ref()?, cache_key?);
        match catalog.cached(cache_key, now).await {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, cache_key, "the artifact catalog could not be read");
                None
            }
        }
    }

    /// What an attempt produced, and what key it answers for next time.
    ///
    /// Failures are swallowed for the same reason a miss is: the work is done
    /// and its result is stored, and a catalog that refused the note about it
    /// must not turn a completed step into a failed one.
    async fn record(
        &self,
        row: &AttemptRow,
        inputs: &[aiwatcher_core::ArtifactRef],
        outputs: &[aiwatcher_core::ArtifactRef],
        cache_key: Option<&str>,
        now: OffsetDateTime,
    ) {
        let Some(catalog) = self.catalog.as_ref() else {
            return;
        };
        if outputs.is_empty() {
            return;
        }
        let recorded = crate::artifact::object::record_outputs(
            catalog.as_ref(),
            crate::Provenance {
                execution_id: row.key.execution_id.clone(),
                step_id: row.key.step_id.clone(),
                attempt: row.key.attempt,
            },
            inputs,
            outputs,
            cache_key,
            now,
        )
        .await;
        if let Err(error) = recorded {
            tracing::warn!(
                %error,
                attempt = %row.key,
                "the artifact catalog could not record what this attempt produced"
            );
        }
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

    /// Report one fact about one attempt, under an id derived from that
    /// attempt.
    ///
    /// The derivation is the whole mechanism and every part of it is
    /// load-bearing. It is *derived* so a reactor that crashed between the
    /// runtime's answer and this call reports the same id on its next pass,
    /// which the inbox absorbs rather than deciding twice. And it names the
    /// **attempt** — the step and the number, not only the execution and the
    /// event — because two steps of one run each report a `step_started`, and
    /// an id built from the execution and the event name alone makes the second
    /// one a redelivery of the first. The decider then never hears about it:
    /// the step stays `pending` behind a lease nothing releases, which reads
    /// exactly like a runtime that is busy.
    async fn report(
        &self,
        key: &crate::claim::AttemptKey,
        event: WorkflowEvent,
        now: OffsetDateTime,
        what: &str,
    ) -> Result<(), HandleError> {
        let execution = &key.execution_id;
        let message_id = MessageId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/report/{execution}/{}/{}/{what}",
            key.step_id, key.attempt
        )));
        let metadata = MessageMetadata {
            schema_version: SCHEMA_VERSION,
            message_id,
            occurred_at: now,
            correlation_id: CorrelationId::new(execution.as_str()),
            causation_id: CausationId::new(execution.as_str()),
            trace_id: None,
            span_id: None,
            // What the fact is about, so a stream read afterwards says which
            // step a message belonged to without decoding its payload.
            step_id: Some(key.step_id.clone()),
            attempt: Some(key.attempt),
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
