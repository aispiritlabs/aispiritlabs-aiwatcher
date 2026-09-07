//! What a reactor does with an attempt it claimed.
//!
//! Section 14. The claim table hands a process one attempt; this is the port
//! for performing it, and the eight steps that stand between "claimed" and
//! "reported".
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
//! Steps 1, 2, 7 and 8 belong to [`crate::reactor`], because they are the same
//! for every runtime. What an implementation of [`ActivityExecutor`] owns is 3
//! to 6 — and the one thing it must get right is step 4's key.
//!
//! ## A timeout proves nothing
//!
//! It says the caller stopped waiting. It does not say the runtime stopped
//! working, and a Flow query that took eleven minutes has still written its
//! rows. So a retry is not the first move: [`ActivityExecutor::lookup`] asks
//! the runtime *by the idempotency key* whether that attempt already finished,
//! and only an honest `Absent` justifies running it again. A runtime that
//! cannot answer says so by leaving the default in place, and then a timeout is
//! a retry that may duplicate work — which is why the two Flow and marimo
//! routes that gain this lookup are named in sections 15.4 and 16.4.
//!
//! ## Nothing here executes in the serve role
//!
//! An implementation holds a client for a service, or a credential for a
//! cluster. That is the work role's, and it is why the two roles exist
//! (ADR_0025). `PublishDataset` is the one binding that runs in the serve role
//! and it executes nothing: it writes a content-addressed version.

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
    /// each other's rows (section 16.2).
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
    /// runtime supports it safely (section 26).
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
/// is *configuration*: the work role holds a Flow client only when
/// `AIWATCHER_FLOW_URL` is set, and a claim filter built from what is actually
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

    /// What this process may claim: exactly the runtimes it registered.
    #[must_use]
    pub fn claim_filter(&self) -> crate::claim::ClaimFilter {
        crate::claim::ClaimFilter::for_runtimes(
            &self
                .executors
                .iter()
                .map(|executor| executor.runtime())
                .collect::<Vec<_>>(),
        )
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
        }
    }

    #[test]
    fn the_key_an_executor_sends_is_the_one_a_lookup_asks_by() {
        assert_eq!(command().idempotency_key(), "exec-1/extract/1");
    }
}
