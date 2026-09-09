//! What every handler is given.

use std::sync::Arc;

use aiwatcher_annotations::Registry as AnnotationRegistry;
use aiwatcher_annotations::SourceCatalog;
use aiwatcher_annotations::integrations::hubs::Hubs;
use aiwatcher_auth::Authenticator;
use aiwatcher_bus::{MessageSink, MessageSource};
use aiwatcher_conversations::Registry as ConversationArchive;
use aiwatcher_core::engine::WorkflowEngine;
use aiwatcher_core::ports::{AttemptArtifacts, EditorHost, WorkflowRunner};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_execution::message::PayloadPolicy;
use aiwatcher_execution::{ArtifactCatalog, ExecutionHandler, WorkflowStore};
use aiwatcher_projector::{LiveHub, ReadModel};
use aiwatcher_prompts::Registry;
use aiwatcher_training::Registry as TrainingRegistry;

/// What this deployment decided about a hosted run's words.
#[derive(Clone, Copy, Debug, Default)]
pub struct PayloadDefault {
    /// `AIWATCHER_EXECUTION_PAYLOADS`.
    pub policy: PayloadPolicy,
    /// `AIWATCHER_EXECUTION_PAYLOADS_LOCKED`: a run may not choose its own.
    pub locked: bool,
}

impl PayloadDefault {
    /// What this run gets, given what it asked for.
    ///
    /// `None` is the ordinary case — nothing asked, so the deployment's answer.
    /// A run that asks while the deployment has pinned its choice is refused
    /// rather than quietly given the pin: the point of asking for `external` on
    /// a `sealed` instance is to keep words out of the archive, and silently
    /// putting them in is the failure the lock exists to prevent, reached from
    /// the other side.
    ///
    /// # Errors
    ///
    /// The refusal, as prose, when the lock forbids the request.
    pub fn resolve(self, asked: Option<PayloadPolicy>) -> Result<PayloadPolicy, String> {
        match asked {
            None => Ok(self.policy),
            Some(asked) if asked == self.policy => Ok(asked),
            Some(_) if self.locked => Err(format!(
                "this instance pins every hosted run to `{}` \
                 (AIWATCHER_EXECUTION_PAYLOADS_LOCKED=true)",
                self.policy.as_str()
            )),
            Some(asked) => Ok(asked),
        }
    }
}

/// How many times one step may be answered, and how big an answer may be.
///
/// A step's answers accumulate: a parked attempt is resumed by a new one that
/// re-runs the work and reads them all, so a task that asks per tool call adds
/// one per turn and nothing takes any away. Left unbounded, the only backstop
/// is the store refusing a message that has grown too large — a failure that
/// arrives late, breaks the run, and names the wrong thing.
///
/// Bounded here rather than in `decide`, which reads no configuration and must
/// not: the answer route is the one door every answer comes through, and a
/// refusal there names what is wrong while the run is still fine. A timeout's
/// own answer does not pass this way and does not need to — its response was
/// authored with the gate and is bounded by whatever accepted the definition.
#[derive(Clone, Copy, Debug)]
pub struct AnswerLimits {
    /// `AIWATCHER_MAX_ANSWERS_PER_STEP`. `None` means no ceiling, which is a
    /// deployment saying so rather than a default nobody chose.
    pub per_step: Option<usize>,
    /// `AIWATCHER_MAX_ANSWER_BYTES`, over the answer's JSON.
    pub bytes: usize,
}

impl Default for AnswerLimits {
    fn default() -> Self {
        Self {
            // High enough that an approval per tool call over a long turn is
            // ordinary, low enough that a task looping on its own question
            // stops before the stream does.
            per_step: Some(256),
            // The same ceiling a step's inline result gets, and for the same
            // reason: an answer is a bounded control value, and anything that
            // grows with the data belongs in an artifact.
            bytes: aiwatcher_execution::message::MAX_INLINE_RESULT_BYTES,
        }
    }
}

impl AnswerLimits {
    /// Whether one more answer of this size may be given, or why not.
    ///
    /// # Errors
    ///
    /// The refusal, as prose a person reads, naming the variable that sets it.
    pub fn admit(self, given: usize, bytes: usize) -> Result<(), String> {
        if bytes > self.bytes {
            return Err(format!(
                "that answer is {bytes} bytes and the limit is {} \
                 (AIWATCHER_MAX_ANSWER_BYTES). An answer is a decision, not data: \
                 hand rows to the step as an artifact instead",
                self.bytes
            ));
        }
        match self.per_step {
            Some(ceiling) if given >= ceiling => Err(format!(
                "this step has already been answered {given} times and the limit is {ceiling} \
                 (AIWATCHER_MAX_ANSWERS_PER_STEP). A step that keeps asking is a task looping \
                 on its own question rather than one waiting on a person"
            )),
            _ => Ok(()),
        }
    }
}

/// Shared application state.
///
/// The bus is held behind trait objects so the same router runs over the
/// in-memory bus in tests, the write-ahead log in development and Laser in
/// production, with no conditional compilation.
#[derive(Clone)]
pub struct AppState {
    pub read_model: Arc<ReadModel>,
    pub live: Arc<LiveHub>,
    pub source: Arc<dyn MessageSource>,
    /// `None` disables the HTTP ingest endpoint. A deployment whose producers
    /// all publish to Laser directly should leave it off rather than expose a
    /// second write path.
    pub sink: Option<Arc<dyn MessageSink>>,
    /// `None` when no prompt store is configured, which makes every
    /// `/api/v1/prompts` route answer 501 rather than 404. The registry is the
    /// one thing here that outlives retention, so running without it is a
    /// deliberate choice and the API says so instead of pretending the routes
    /// do not exist.
    pub prompts: Option<Arc<Registry>>,
    /// The durable half of Data Curation: saved Flow recipes and the immutable
    /// rows each execution produced. It shares the configured object store
    /// with prompts, under a separate key prefix.
    pub datasets: Option<Arc<DatasetRegistry>>,

    /// When a definition runs unattended. `None` when this deployment has no
    /// object store, which is the same condition that leaves it no definitions
    /// to schedule.
    ///
    /// Its own field rather than a method on the dataset registry: a schedule
    /// is keyed by [`DefinitionKind`](aiwatcher_execution::plan::DefinitionKind)
    /// and will hold a workflow's as readily as a pipeline's, and it lives
    /// under its own prefix for that reason.
    pub schedules: Option<Arc<aiwatcher_execution::ScheduleStore>>,
    /// Authored Python workflows, versioned outside execution retention.
    pub workflow_definitions: Option<Arc<aiwatcher_execution::definition::DefinitionRegistry>>,
    /// Vector image annotations and the training exports built from them.
    /// Same store, third prefix, and the same reason all three are here rather
    /// than on the log: a training label has to outlive every run that used
    /// it. See ADR_0017.
    pub annotations: Option<Arc<AnnotationRegistry>>,
    /// The governed conversation archive, and the one authored store whose
    /// absence is the *default*. Every other `Option` here is off because a
    /// deployment did not wire something; this one is off because keeping
    /// somebody's words is a decision that has to be made rather than
    /// inherited. See ADR_0021.
    pub conversations: Option<Arc<ConversationArchive>>,
    /// How a handler tells the export worker that there is something to do.
    ///
    /// `None` outside the server binary — a router built for a test has no
    /// worker, and a notify nobody waits on would be a silent no-op rather
    /// than an obvious one.
    pub export_worker: Option<Arc<tokio::sync::Notify>>,
    /// Managed execution: the transactional store, behind the handler that is
    /// the only way to write to it.
    ///
    /// Never `None` in the server binary — an execution store needs no more
    /// configuration than a directory, so there is no "this deployment has
    /// none" to report. It is an `Option` for the routers a test builds, which
    /// have no store and should answer 501 rather than hold one.
    ///
    /// What the API does with it is deliberately narrow: accept a command, and
    /// read one run's own page. Never a *list* — the workflow fold already
    /// serves that from the log, and a second list would be the second picture
    /// of one run that ADR_0026 refuses.
    pub executions: Option<Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>>,
    /// How a handler tells this process's work role that there is something to
    /// do.
    ///
    /// Two loops wait on it — the outbox publisher and a reactor — so this one
    /// wakes *every* waiter rather than one of them, unlike the two queues
    /// above. A loop that was not yet waiting misses the nudge and picks the
    /// work up on its next poll, which is why the poll still exists.
    pub execution_worker: Option<Arc<tokio::sync::Notify>>,
    /// The same, for the annotation import queue.
    ///
    /// A second notify rather than a shared one: the two queues live in
    /// different stores behind different configuration, and a deployment that
    /// imports corpora while keeping no conversation archive is the ordinary
    /// case rather than an odd one.
    pub import_worker: Option<Arc<tokio::sync::Notify>>,
    /// `None` when no dataset hub is configured, which makes
    /// `/api/v1/dataset-hubs` answer 501 naming the variable. Unlike every
    /// other option here this one is *outbound*: it is the only thing in this
    /// state that reaches a service aiwatcher does not run, for a question
    /// whose answer it deliberately refuses to trust — see
    /// [`aiwatcher_annotations::integrations::hubs`].
    pub hubs: Option<Arc<Hubs>>,
    /// The corpora somebody read the licence of, loaded from
    /// `AIWATCHER_DATASET_SOURCES`.
    ///
    /// Not an `Option`: an empty catalogue is a working state rather than a
    /// disabled one. Nothing matches, every hub result stays `unclear`, and an
    /// import records unknown rights — the safe direction, reached by
    /// configuring nothing. See `aiwatcher_annotations::sources`.
    pub sources: Arc<SourceCatalog>,
    /// Training runs and the model versions they produce. Same store, fourth
    /// prefix — and the one registry here whose contents never came from the
    /// event log at all. See ADR_0018.
    pub training: Option<Arc<TrainingRegistry>>,
    /// `None` when no orchestrator is configured, which makes the rerun route
    /// answer 501 rather than 404 — the same reasoning as `prompts`, with a
    /// sharper edge. This is the only thing here that makes something happen
    /// rather than reporting that it did, so the disabled case must be
    /// unmistakable: a no-op adapter would acknowledge a rerun nobody ran.
    pub runner: Option<Arc<dyn WorkflowRunner>>,
    /// `None` when no orchestrator is configured, which makes every
    /// `/api/v1/engine` route answer 501. Same reasoning as `runner`, and the
    /// same sharp edge: this is the other thing here that makes something
    /// happen. It is a separate field rather than the same one because the
    /// two ports answer different questions — a deployment can perfectly well
    /// dispatch reruns to a webhook while having no inventory to browse.
    pub engine: Option<Arc<dyn WorkflowEngine>>,
    /// `None` when this process has no notebook runtime address or no object
    /// store, which makes `POST /executions/{id}/steps/{step}/editor` answer
    /// 501 naming the variable. The third port here that makes something
    /// happen, and the third whose absence has to be unmistakable: an editor
    /// that acknowledged without staging would send somebody to a live app
    /// showing last week's rows and say nothing.
    pub editor: Option<Arc<dyn EditorHost>>,
    /// The bytes a worker reads and writes, proxied through this process.
    ///
    /// `None` when there is no object store, which makes every artifact route
    /// under `/api/v1/worker` answer 501 naming the variable. A worker can
    /// still claim and settle without one — a task that takes its parameters
    /// and returns a bounded value needs no artifact at all — so this is a
    /// separate field from the handler rather than a condition on it.
    pub artifacts: Option<Arc<dyn AttemptArtifacts>>,
    /// Where a cache hit is looked up and a result is recorded.
    ///
    /// The same catalog the work role's reactors hold, because a worker's step
    /// is cached and traced like any other. `None` runs everything and
    /// remembers nothing, which is the state a deployment with no object store
    /// is in: deleting the index never loses an authoritative result, taken to
    /// its limit.
    pub catalog: Option<Arc<dyn ArtifactCatalog>>,
    /// Where a hosted run's words live unless the run says otherwise, and
    /// whether a run may say otherwise at all.
    ///
    /// The default is the visible one: a hosted execution starts with no
    /// archive, no key and no flag, and a deployment that wants its words held
    /// here turns that on rather than finding it was already happening.
    pub execution_payloads: PayloadDefault,
    /// What this deployment allows an answer to be. See [`AnswerLimits`].
    pub answer_limits: AnswerLimits,
    /// `None` when no identity provider is configured, which is the default.
    /// Unlike `prompts` and `runner`, absence here is not a 501 on a few
    /// routes — it is every caller being [`aiwatcher_auth::Identity::anonymous`]
    /// and every role check passing, which is what `AIWATCHER_AUTH_MODE=none`
    /// means. The 501 is reserved for the sign-in routes, which cannot do
    /// anything useful without a provider.
    pub auth: Option<Arc<Authenticator>>,
    pub health: HealthState,
}

impl AppState {
    /// Wake the export worker, if this process runs one.
    ///
    /// Queueing an export and then waiting for a poll interval would make the
    /// panel's "queued" state last fifteen seconds for no reason. A missing
    /// worker is not an error: the job is durable, and whichever process does
    /// run one will pick it up.
    pub fn notify_export_worker(&self) {
        if let Some(worker) = &self.export_worker {
            worker.notify_one();
        }
    }

    /// Wake the import worker, if this process runs one.
    pub fn notify_import_worker(&self) {
        if let Some(worker) = &self.import_worker {
            worker.notify_one();
        }
    }

    /// Wake this process's work role, if it runs one.
    ///
    /// `notify_waiters` rather than `notify_one`: a decision produces both an
    /// outbox row and, usually, a claimable attempt, and the two are drained by
    /// different loops. Waking one of them would leave the other on its poll.
    pub fn notify_execution_worker(&self) {
        if let Some(worker) = &self.execution_worker {
            worker.notify_waiters();
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("source", &self.source)
            .field("ingest_enabled", &self.sink.is_some())
            .field("prompt_registry", &self.prompts.is_some())
            .field("dataset_registry", &self.datasets.is_some())
            .field("annotation_registry", &self.annotations.is_some())
            .field("conversation_archive", &self.conversations.is_some())
            .field("execution_store", &self.executions.is_some())
            .field("training_registry", &self.training.is_some())
            .field("dataset_hubs", &self.hubs.is_some())
            .field("dataset_sources", &self.sources.sources.len())
            .field("workflow_runner", &self.runner)
            .field("editor", &self.editor)
            .field("engine", &self.engine)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// Liveness and readiness, kept separate on purpose.
///
/// Liveness means "the process is not wedged" — a failing liveness probe gets
/// the container killed. Readiness means "it can serve traffic right now"; a
/// projector still replaying a backlog is alive but not ready, and restarting
/// it would only make the replay start over.
#[derive(Clone, Debug, Default)]
pub struct HealthState {
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl HealthState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn mark_unready(&self) {
        self.ready
            .store(false, std::sync::atomic::Ordering::Release);
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[cfg(test)]
mod answer_limit_tests {
    use super::AnswerLimits;

    #[test]
    fn a_step_that_keeps_asking_is_stopped_before_the_stream_is() {
        // The number this exists for. Answers accumulate because a resumed
        // attempt re-runs the work and reads all of them, so a task looping on
        // its own question adds one per turn for ever — and unbounded, the
        // first thing to say so is the store refusing an oversized message,
        // which breaks the run and names the wrong thing.
        let limits = AnswerLimits {
            per_step: Some(2),
            bytes: 1024,
        };
        assert!(limits.admit(1, 10).is_ok());
        let refused = limits.admit(2, 10).expect_err("the third answer");
        assert!(
            refused.contains("AIWATCHER_MAX_ANSWERS_PER_STEP"),
            "{refused}"
        );
    }

    #[test]
    fn an_answer_that_is_data_rather_than_a_decision_is_refused_by_name() {
        let limits = AnswerLimits {
            per_step: None,
            bytes: 16,
        };
        let refused = limits.admit(0, 17).expect_err("too large");
        assert!(refused.contains("AIWATCHER_MAX_ANSWER_BYTES"), "{refused}");
        assert!(limits.admit(0, 16).is_ok(), "the limit itself is allowed");
    }

    #[test]
    fn no_ceiling_is_a_deployment_saying_so_rather_than_a_default() {
        // `0` reads as "no ceiling" and never as "answer nothing", the same way
        // retention's zero keeps rather than deletes: one character must not be
        // able to ask for a run nobody can ever answer.
        let limits = AnswerLimits {
            per_step: None,
            bytes: 1024,
        };
        assert!(limits.admit(10_000, 10).is_ok());
    }

    #[test]
    fn the_default_admits_an_approval_per_tool_call_over_a_long_turn() {
        let limits = AnswerLimits::default();
        assert!(limits.admit(200, 512).is_ok());
    }
}
