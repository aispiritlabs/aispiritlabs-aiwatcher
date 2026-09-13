//! What a reactor does with an attempt it claimed.
//!
//! ```text
//!   1  deduplicate by command_id
//!   2  claim or renew the lease
//!   3  load and verify input artifact digests
//!   4  call the runtime with <execution>/<step>/<attempt>
//!   5  stream or upload output to the object store
//!   6  verify digest and size
//!   7  append StepCompleted or StepFailed to the workflow
//!   8  advance the checkpoint only after that fact is durable
//! ```
//!
//! 1, 2, 7 and 8 belong to [`crate::reactor`] — they are the same for every
//! runtime. An [`ActivityExecutor`] owns 3 to 6, and must get step 4's key
//! right.
//!
//! **A timeout proves nothing.** It says the caller stopped waiting, not that
//! the runtime stopped working — a Flow query that took eleven minutes has
//! written its rows. So [`ActivityExecutor::lookup`] asks the runtime by the
//! idempotency key whether that attempt already finished, and only an honest
//! `Absent` justifies running it again. A runtime that cannot answer leaves the
//! default in place, and then a timeout is a retry that may duplicate work.
//!
//! Nothing here executes in the `serve` role: an implementation holds a client
//! or a credential, and those are the work role's. `PublishDataset` is the
//! exception and executes nothing — it writes a content-addressed version.

use std::collections::BTreeMap;
use std::time::Duration;

use aiwatcher_core::ArtifactRef;
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::claim::AttemptKey;
use crate::plan::{PlanStep, RuntimeKind};
use crate::state::{FailureClass, InputRequest, StepError};

/// One attempt, as the executor receives it.
///
/// Carries the plan step rather than a copy of its parameters: a reactor that
/// re-derived "which notebook, which revision" from loose fields would be a
/// second reading of a plan that is already immutable.
#[derive(Clone, Debug)]
pub struct ActivityCommand {
    pub key: AttemptKey,
    /// The dispatch that authorised this. Step 1 deduplicates by it.
    pub command_id: aiwatcher_core::MessageId,
    pub step: PlanStep,
    /// The artifacts this step's inputs resolved to, in the order the plan
    /// declares them. Digests verified before the runtime is called (step 3).
    pub inputs: Vec<ArtifactRef>,
    /// Values bound when the execution was requested.
    pub parameters: BTreeMap<String, Value>,
    /// Every answer this step has already been given, oldest first.
    ///
    /// Empty for almost every attempt, and the exception is what it is here
    /// for: an attempt scheduled because somebody answered the question the
    /// *previous* one stopped to ask. That attempt re-runs the work from the
    /// beginning, so it reaches the same question again and has to read the
    /// answer rather than park a second time.
    ///
    /// Consumed in order. See [`crate::state::InputAnswer`] for why there can
    /// be more than one.
    pub answers: Vec<crate::state::InputAnswer>,
}

impl ActivityCommand {
    /// `<execution>/<step>/<attempt>`. The key step 4 sends and
    /// [`ActivityExecutor::lookup`] asks by.
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        self.key.idempotency_key()
    }
}

/// What an executor may use while it runs, and what it must respect.
#[derive(Clone, Debug)]
pub struct ActivityContext {
    /// The name this reactor holds its lease under. An executor that reports
    /// progress carries it so the lease is renewed by the holder and nobody
    /// else.
    pub owner: String,
    /// The step's own deadline. Past it the reactor stops waiting — which is
    /// step 4's timeout, and why `lookup` exists.
    pub timeout: Duration,
    /// Where this step's staged input, parameters and output live:
    /// `<execution>/<step>/<attempt>/…`. Keyed by context and never by the
    /// notebook's name, so two pipelines editing one notebook stop overwriting
    /// each other's rows.
    pub context_id: String,
    /// The whole plan this step belongs to.
    ///
    /// The reactor has already loaded it, so this costs a clone of an `Arc`.
    /// It is here for the one executor that has a question about its
    /// *neighbours* rather than about itself: publishing a dataset version has
    /// to record the query that produced the rows, and that query is a field
    /// of the step upstream. Copying it into the publish step at compile time
    /// would put one script in a plan twice and in `plan_id` twice.
    pub plan: std::sync::Arc<crate::plan::ExecutionPlan>,
    /// Set by the reactor when this attempt should stop: its run is being
    /// cancelled or has ended around it, or its own deadline passed.
    ///
    /// An executor that does long work in pieces looks at it between them and
    /// returns [`StopSignal::check`]'s error. One that never looks is abandoned
    /// once the reactor's grace runs out — its outcome is then the reactor's
    /// word, and whatever it was in the middle of is left where it stopped.
    pub stop: StopSignal,
}

/// Why a reactor asked an attempt to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The run is being cancelled, or ended while this step was running — a
    /// sibling failed for the last time, and what is downstream will not run.
    RunStopping,
    /// The step ran past its own `timeout_seconds`.
    TimedOut,
}

impl StopReason {
    /// The failure an attempt stopped for this reason reports.
    ///
    /// `Policy` for a run that is stopping, because it is the class nothing
    /// retries: a retried attempt would be work for a run that asked for none.
    /// A timeout keeps its own class and its own budget, because the work may
    /// have been nearly done.
    #[must_use]
    pub fn as_error(self) -> ActivityError {
        match self {
            Self::RunStopping => ActivityError::new(
                FailureClass::Policy,
                "stopped: the execution is no longer running",
            ),
            Self::TimedOut => ActivityError::timed_out("stopped: the step ran past its timeout"),
        }
    }
}

/// A reactor's request that one running attempt stop.
///
/// Cooperative first: the reactor sets it, asks the runtime through
/// [`ActivityExecutor::cancel`], and waits a grace period for the executor to
/// return. Only then is the attempt abandoned, which is the forced half — safe
/// here because a dropped future stops what this process was doing, and every
/// write an executor makes is ordered so that stopping between two of them
/// leaves nothing that reads as finished.
#[derive(Clone, Debug, Default)]
pub struct StopSignal {
    token: tokio_util::sync::CancellationToken,
    reason: std::sync::Arc<std::sync::OnceLock<StopReason>>,
    committing: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    committed: std::sync::Arc<tokio::sync::Notify>,
}

/// An attempt past the point where stopping would leave half of something.
///
/// Held around a write that has to finish once it began — a dataset version, a
/// scoring run's evidence. While one is outstanding the reactor waits for the
/// executor rather than abandoning it when the grace runs out: a deadline
/// measures the work, and a publication cut off halfway is worth less than one
/// finished a few seconds late. Dropping it ends the commit.
#[derive(Debug)]
#[must_use = "a commit lasts as long as its guard"]
pub struct Committing {
    signal: StopSignal,
}

impl Drop for Committing {
    fn drop(&mut self) {
        if self
            .signal
            .committing
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            == 1
        {
            self.signal.committed.notify_waiters();
        }
    }
}

impl StopSignal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the attempt to stop. The first reason given is the one kept.
    pub fn stop(&self, reason: StopReason) {
        // The reason before the token, so whoever wakes on the token reads it.
        let _ = self.reason.set(reason);
        self.token.cancel();
    }

    /// Why the attempt was asked to stop, if it was.
    #[must_use]
    pub fn requested(&self) -> Option<StopReason> {
        self.reason.get().copied()
    }

    /// Resolves once the attempt is asked to stop, and never otherwise.
    pub async fn stopped(&self) -> StopReason {
        self.token.cancelled().await;
        self.requested().unwrap_or(StopReason::RunStopping)
    }

    /// Carry on, or the error an attempt that stopped here reports.
    ///
    /// # Errors
    ///
    /// [`StopReason::as_error`] once a stop was requested.
    pub fn check(&self) -> Result<(), ActivityError> {
        self.requested()
            .map_or(Ok(()), |reason| Err(reason.as_error()))
    }

    /// The error an attempt reports for a failure that may be the stop itself.
    ///
    /// A runtime asked to stop answers the request it was serving with a
    /// refusal of its own — a 409, a closed connection — and reported as it
    /// came, that reads as user code or an outage and is retried. Once a stop
    /// was requested, the stop is the reason.
    #[must_use]
    pub fn or_stopped(&self, error: ActivityError) -> ActivityError {
        self.requested().map_or(error, StopReason::as_error)
    }

    /// The last look, and then a commit the reactor will wait for.
    ///
    /// # Errors
    ///
    /// [`StopReason::as_error`] when a stop was requested before the commit
    /// began — then nothing was begun, and the attempt stops here.
    pub fn committing(&self) -> Result<Committing, ActivityError> {
        self.committing
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let guard = Committing {
            signal: self.clone(),
        };
        // Counted before the look, so a stop landing between the two is either
        // seen here or finds the commit already outstanding.
        self.check()?;
        Ok(guard)
    }

    /// Whether a commit is outstanding.
    #[must_use]
    pub fn is_committing(&self) -> bool {
        self.committing.load(std::sync::atomic::Ordering::SeqCst) > 0
    }

    /// Resolves once no commit is outstanding.
    pub async fn committed(&self) {
        let notified = self.committed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.is_committing() {
            return;
        }
        notified.await;
    }
}

/// What an attempt produced.
#[derive(Clone, Debug)]
pub struct ActivityResult {
    pub outputs: Vec<ArtifactRef>,
    /// A bounded control value. Rows go in an artifact — a result that grows
    /// with the data is one that eventually cannot be stored or replayed.
    pub result: Option<Value>,
    /// Captured stdout/stderr, already bounded by the executor. `None` when
    /// there was none worth keeping; an artifact when there was too much.
    pub diagnostics: Option<String>,
    /// The executor is asking somebody a question and has parked the attempt.
    /// The lease is released while it waits: a worker that stopped to ask does
    /// not hold a pod for the answer.
    pub awaiting: Option<InputRequest>,
    /// Whether this result may answer for its cache key later.
    ///
    /// `true` for almost everything, and the exception is what it is here for:
    /// an executor that could not honour something the key assumed. A Flow step
    /// whose plan pinned a span against a query service that narrowed it to a
    /// duration produced *correct rows for a different question*, and storing
    /// them under the key would serve them to the question that was asked.
    ///
    /// The executor answers this rather than the key's author, because only the
    /// runtime knows what it managed to do — which is why the query service
    /// declares `window_applied` rather than leaving it to be inferred.
    pub cacheable: bool,
}

impl Default for ActivityResult {
    fn default() -> Self {
        Self {
            outputs: Vec::new(),
            result: None,
            diagnostics: None,
            awaiting: None,
            // Derived `Default` would make this `false`, which would silently
            // turn the cache off for every executor that built a result the
            // short way.
            cacheable: true,
        }
    }
}

/// Why an attempt did not produce a result.
///
/// Carries the class rather than leaving the reactor to guess: whether to retry
/// is a claim about *this* error, and the process that made the call is the one
/// that knows. Same split as `aiwatcher_jobs::after_failure` taking
/// `retryable`, and `PortError`'s one layer down.
#[derive(Clone, Debug, Error)]
#[error("{class:?}: {message}")]
pub struct ActivityError {
    pub class: FailureClass,
    pub message: String,
}

impl ActivityError {
    #[must_use]
    pub fn new(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    /// The runtime answered and the answer was wrong. Not retried.
    #[must_use]
    pub fn user_code(message: impl Into<String>) -> Self {
        Self::new(FailureClass::UserCode, message)
    }

    /// The runtime could not be reached, or asked to come back. Retried.
    #[must_use]
    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Transient, message)
    }

    /// The caller stopped waiting. Proves nothing about the runtime — see the
    /// module docs.
    #[must_use]
    pub fn timed_out(message: impl Into<String>) -> Self {
        Self::new(FailureClass::Timeout, message)
    }

    /// What the workflow records.
    #[must_use]
    pub fn as_step_error(&self) -> StepError {
        StepError::new(self.class, self.message.clone())
    }
}

/// What a runtime says about an attempt somebody already sent it.
#[derive(Clone, Debug)]
pub enum PriorAttempt {
    /// It is still working. The reactor waits rather than sending it again.
    Running,
    /// It finished, and here is what it produced.
    Done(Box<ActivityResult>),
    /// It has no record of that key. Only this justifies running it again.
    Absent,
}

/// One runtime, behind the address its configuration names.
#[async_trait]
pub trait ActivityExecutor: Send + Sync + std::fmt::Debug {
    /// Which binding this runs. The reactor claims by it.
    fn runtime(&self) -> RuntimeKind;

    /// Steps 3 to 6: verify the inputs, call the runtime with the idempotency
    /// key, store what came back, and check its digest.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] carrying the class that decides whether to retry.
    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError>;

    /// Whether this runtime already ran an attempt, asked by its idempotency
    /// key.
    ///
    /// The default is `Absent`, which means "this runtime cannot be asked" —
    /// and a timeout against it is a retry that may duplicate work. Overriding
    /// it is what sections 15.4 and 16.4 ask of the Flow and notebook services,
    /// and it is one field and one route each.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the runtime could not be asked at all, which is
    /// different from it answering `Absent`.
    async fn lookup(&self, _command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        Ok(PriorAttempt::Absent)
    }

    /// Ask the runtime to stop. Cooperative, and best-effort by design:
    /// cancellation is cooperative first and forced termination only where a
    /// runtime supports it safely.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the request could not be made.
    async fn cancel(&self, _command: &ActivityCommand) -> Result<(), ActivityError> {
        Ok(())
    }
}

/// Every executor this process holds, by the runtime it runs.
///
/// A registry rather than a `match`, because which runtimes a process can run
/// is *configuration*: the work role holds a query client only when
/// `AIWATCHER_QUERY_URL` is set — and only for the engine
/// `AIWATCHER_QUERY_ENGINE` names — and a claim filter built from what is actually
/// registered is what stops it claiming work it cannot perform.
#[derive(Debug, Default)]
pub struct ExecutorRegistry {
    executors: Vec<std::sync::Arc<dyn ActivityExecutor>>,
}

impl ExecutorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one. A second executor for a runtime replaces the first rather than
    /// sitting behind it, so a misconfiguration is one executor and not a
    /// coin toss.
    #[must_use]
    pub fn with(mut self, executor: std::sync::Arc<dyn ActivityExecutor>) -> Self {
        let runtime = executor.runtime();
        self.executors.retain(|held| held.runtime() != runtime);
        self.executors.push(executor);
        self
    }

    /// Everything two registries hold, with the second winning a runtime they
    /// both name.
    ///
    /// A process registers its executors one address at a time — a Flow client
    /// from one variable, a notebook client from another — and a registry per
    /// address keeps each one's "absent is a working state" local to it.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        other.executors.into_iter().fold(self, Self::with)
    }

    #[must_use]
    pub fn get(&self, runtime: RuntimeKind) -> Option<&std::sync::Arc<dyn ActivityExecutor>> {
        self.executors
            .iter()
            .find(|executor| executor.runtime() == runtime)
    }

    /// The runtimes this process registered an executor for.
    ///
    /// The same list [`Self::claim_filter`] is built from, as the claimant's
    /// own answer rather than as a query — which is what
    /// [`Reactor::take`](crate::reactor::Reactor::take) checks the claimed row
    /// against before it reports a start.
    #[must_use]
    pub fn runtimes(&self) -> Vec<RuntimeKind> {
        self.executors
            .iter()
            .map(|executor| executor.runtime())
            .collect()
    }

    /// What this process may claim: exactly the runtimes it registered.
    #[must_use]
    pub fn claim_filter(&self) -> crate::claim::ClaimFilter {
        crate::claim::ClaimFilter::for_runtimes(&self.runtimes())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Stub(RuntimeKind);

    #[async_trait]
    impl ActivityExecutor for Stub {
        fn runtime(&self) -> RuntimeKind {
            self.0
        }

        async fn execute(
            &self,
            _command: &ActivityCommand,
            _context: &ActivityContext,
        ) -> Result<ActivityResult, ActivityError> {
            Ok(ActivityResult::default())
        }
    }

    #[test]
    fn a_commit_is_refused_once_a_stop_arrived_and_counted_until_its_guard_drops() {
        let signal = StopSignal::new();
        let first = signal.committing().expect("nothing asked it to stop");
        let second = signal.committing().expect("nor now");
        drop(first);
        assert!(signal.is_committing(), "one commit is still outstanding");
        drop(second);
        assert!(!signal.is_committing());

        signal.stop(StopReason::TimedOut);
        let refused = signal.committing().expect_err("a stop came first");
        assert_eq!(refused.class, FailureClass::Timeout);
        assert!(
            !signal.is_committing(),
            "a refused commit leaves nothing outstanding"
        );
    }

    #[test]
    fn a_runtime_refusing_the_request_it_was_asked_to_stop_reports_the_stop() {
        let signal = StopSignal::new();
        let refused = || ActivityError::user_code("409: the query was cancelled");
        assert_eq!(signal.or_stopped(refused()).class, FailureClass::UserCode);
        signal.stop(StopReason::RunStopping);
        let stopped = signal.or_stopped(refused());
        assert_eq!(stopped.class, FailureClass::Policy, "never retried as user code");
    }

    #[test]
    fn a_process_claims_exactly_the_runtimes_it_registered() {
        // Which runtimes a process can run is configuration: the work role
        // holds a Flow client only when its address is set, and a filter built
        // from anything else would claim work it cannot perform.
        let registry =
            ExecutorRegistry::new().with(std::sync::Arc::new(Stub(RuntimeKind::FlowPhp)));
        assert_eq!(
            registry.claim_filter(),
            crate::claim::ClaimFilter::for_runtimes(&[RuntimeKind::FlowPhp])
        );
        assert!(registry.get(RuntimeKind::Marimo).is_none());
        assert!(ExecutorRegistry::new().is_empty());
    }

    #[test]
    fn a_second_executor_for_one_runtime_replaces_the_first() {
        // A misconfiguration should be one executor, not a coin toss.
        let registry = ExecutorRegistry::new()
            .with(std::sync::Arc::new(Stub(RuntimeKind::FlowPhp)))
            .with(std::sync::Arc::new(Stub(RuntimeKind::FlowPhp)));
        assert_eq!(registry.executors.len(), 1);
    }

    #[tokio::test]
    async fn a_runtime_that_cannot_be_asked_says_absent_rather_than_pretending() {
        // The default, and it is the honest one: a timeout against a runtime
        // with no lookup is a retry that may duplicate work, which is what
        // sections 15.4 and 16.4 exist to remove.
        let prior = Stub(RuntimeKind::FlowPhp).lookup(&command()).await;
        assert!(matches!(prior, Ok(PriorAttempt::Absent)));
    }

    fn command() -> ActivityCommand {
        ActivityCommand {
            key: AttemptKey::new(crate::state::ExecutionId::new("exec-1"), "extract", 1),
            command_id: aiwatcher_core::MessageId::new("cmd-1"),
            step: PlanStep {
                id: "extract".to_owned(),
                runtime: crate::plan::RuntimeBinding::PythonTask(crate::plan::PythonTaskSpec {
                    task_ref: "stage@1".to_owned(),
                    queue: "default".to_owned(),
                    params: BTreeMap::new(),
                }),
                inputs: Vec::new(),
                outputs: Vec::new(),
                retry: crate::plan::RetryPolicy::default(),
                timeout_seconds: 60,
                cache: crate::plan::CachePolicy::Never,
            },
            inputs: Vec::new(),
            parameters: BTreeMap::new(),
            answers: Vec::new(),
        }
    }

    #[test]
    fn the_key_an_executor_sends_is_the_one_a_lookup_asks_by() {
        assert_eq!(command().idempotency_key(), "exec-1/extract/1");
    }
}
