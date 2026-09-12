//! Compile a definition and start one run: the use case, and the only one.
//!
//! Three callers ask for a managed run — the route somebody presses, the
//! schedule route's `run_now`, and the tick that finds a slot due — and what
//! starting *means* is the same for all three: read the definition at the
//! revision it names, compile it, resolve the payload policy this deployment
//! allows, derive the id, and write one transaction. What differs is only who
//! is asking and which id the run gets, and both of those are inputs.
//!
//! It lived in the HTTP module, which made the scheduler a client of an axum
//! router: it called in, got an `ApiError` back, and read the **status code**
//! to decide whether the slot should stay due. That is a lossy encoding of the
//! one question it had — 502 and 500 are 5xx by number and permanent by
//! meaning, so a corrupt definition and an object store that refused the read
//! were both filed as "come back in a minute", every minute, for ever. The
//! refusal answers it itself now: [`StartRefused::says_the_same_next_time`].
//!
//! What stays with the caller is what only the caller knows. **Who is asking**
//! is the route's — a role check against a session, which a tick does not have
//! and must not fake. **Which id** is [`RunIdentity`], because a slot's id is
//! derived from the definition and the moment while a browser's comes from an
//! `Idempotency-Key`. Everything between the two is here.
//!
//! ADR_0025.

use std::collections::BTreeMap;

use aiwatcher_core::MessageId;
use aiwatcher_datasets::{QueryEngine, Registry as PipelineRegistry, RegistryError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::compile::CompileOptions;
use crate::definition::DefinitionRegistry;
use crate::error::DefinitionError;
use crate::handler::{ExecutionHandler, Handled};
use crate::message::{PayloadDefault, PayloadPolicy};
use crate::plan::{DefinitionKind, ResolvedWindow};
use crate::{
    ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, HandleError, MessageMetadata, Now,
    WorkflowCommand, WorkflowMessage, WorkflowStore, compile_curation, derive_uuid,
};

/// Which definition, at which revision.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    /// Which registry the name is in. Never guessed from the name: a
    /// `WorkflowSpec` and a `CurationPipeline` may share one, and they compile
    /// through different editors with different provenance.
    pub kind: DefinitionKind,
    pub name: String,
    /// The immutable revision to compile. Left out, the definition's head is
    /// read and *pinned* — a run always names one revision, so editing the
    /// definition while it goes changes nothing about what is running.
    #[serde(default)]
    pub revision: Option<String>,
}

/// Who decides what this run does next.
///
/// Not a pair of `owner`/`mode` fields, because only two of their combinations
/// mean anything to a caller and the other two are a run nobody would want:
/// `local`+`hosted` is a decider with no plan to schedule, and `worker`+
/// `compiled` is a worker that may not decide. One field with two arms is the
/// choice that actually exists.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    /// The Rust decider schedules the plan's steps and owns their retries.
    #[default]
    Local,
    /// A worker runs `decide` and appends to the history this system keeps
    /// (ADR_0025). What an agent graph needs, because its next node depends on
    /// what the last one said.
    Worker,
}

impl Decider {
    #[must_use]
    pub const fn parts(self) -> (ExecutionOwner, ExecutionMode) {
        match self {
            Self::Local => (ExecutionOwner::Local, ExecutionMode::Compiled),
            Self::Worker => (ExecutionOwner::Worker, ExecutionMode::Hosted),
        }
    }
}

/// How wide the source's time window is, and where it ends.
///
/// Relative as it was asked for, and pinned to exact bounds *once* by
/// [`Self::resolve`]. Resolving it at dispatch would move it under a retry,
/// after which two attempts at one step read different rows and a cache key
/// could not exist at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct Window {
    /// Seconds back from where it ends.
    pub seconds: Option<u64>,
    /// Where it ends, in seconds since the epoch. `None` is now.
    pub as_of: Option<i64>,
}

impl Window {
    /// Whether the caller said anything about a window at all.
    ///
    /// `as_of` alone counts: it is half a window, and a caller who sent one to
    /// something that has none has misunderstood the request rather than sent
    /// a field that can be ignored.
    #[must_use]
    pub const fn asked(self) -> bool {
        self.seconds.is_some() || self.as_of.is_some()
    }

    /// The bounds this meant when it was asked for.
    ///
    /// The clock is read here and nowhere below: [`crate::decide`] may not read
    /// one.
    #[must_use]
    pub fn resolve(self) -> Option<ResolvedWindow> {
        let seconds = self.seconds?;
        let to = self
            .as_of
            .unwrap_or_else(|| OffsetDateTime::now_utc().unix_timestamp());
        Some(ResolvedWindow {
            from: to.saturating_sub(i64::try_from(seconds).unwrap_or(i64::MAX)),
            to,
        })
    }
}

/// The id this run will have.
///
/// Three callers, three answers, and the reason each is what it is lives here
/// rather than in the caller that happens to need it.
#[derive(Clone, Debug)]
pub enum RunIdentity {
    /// The id this run must have.
    ///
    /// A scheduled slot's, derived from the definition and the moment: two
    /// workers that both notice nine o'clock derive one id, and the second is a
    /// conflict rather than a second run. No lease, nothing to expire.
    Named(ExecutionId),
    /// Repeat this key and reach the same run.
    ///
    /// Derived from the key **and the plan**, so one key cannot address two
    /// different plans: a caller who edited the pipeline and repeated their key
    /// would otherwise get the *old* run back and a 202 saying so.
    Key(String),
    /// Two clicks are two runs, which is what clicking twice on purpose means.
    Fresh,
}

impl RunIdentity {
    /// The id, once there is a plan to derive it against.
    fn of(&self, plan: &ExecutionPlan) -> ExecutionId {
        match self {
            Self::Named(id) => id.clone(),
            Self::Key(key) => ExecutionId::new(derive_uuid(&format!(
                "aiwatcher/execution/request/{}/{key}",
                plan.plan_id
            ))),
            // Hyphen-free, like a launch's `workflow_run_id`: this id becomes a
            // correlation id, a partition key and a file name.
            Self::Fresh => ExecutionId::new(uuid::Uuid::now_v7().simple().to_string()),
        }
    }
}

/// What to start, on a plan the caller already has.
#[derive(Clone, Debug)]
pub struct StartRun {
    pub identity: RunIdentity,
    /// Values bound when the execution was requested, available to every step.
    pub parameters: BTreeMap<String, Value>,
    /// Who asked. Recorded rather than derived, so a run always says who was
    /// responsible for it.
    pub requested_by: String,
    pub decided_by: Decider,
    /// Where this run's words live. `None` is the deployment's own answer.
    pub payloads: Option<PayloadPolicy>,
}

/// A definition to compile, and the run to start on it.
#[derive(Clone, Debug)]
pub struct StartRequest {
    pub target: ExecutionTarget,
    pub window: Window,
    pub run: StartRun,
}

/// An accepted command, and the run it started.
#[derive(Debug)]
pub struct Started {
    /// The id this run has. Derived, so a repeat lands on it rather than
    /// beside it.
    pub execution_id: ExecutionId,
    pub handled: Handled,
}

/// What a deployment wired nothing behind.
///
/// An enum rather than the variable's name as a string, because the caller that
/// renders this has its own word for each — and matching a configuration key by
/// spelling is the kind of join that survives a rename by going quiet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The store every decision is written to. An instance with none runs
    /// nothing, whatever else it has.
    WorkflowStore,
    /// Where curation pipelines are read from.
    PipelineRegistry,
    /// Where registered workflows are read from.
    WorkflowRegistry,
}

impl Missing {
    /// What is absent, as the sentence a caller shows.
    #[must_use]
    pub const fn what(self) -> &'static str {
        match self {
            Self::WorkflowStore => "this instance has no workflow store configured",
            Self::PipelineRegistry => "this instance has no dataset registry configured",
            Self::WorkflowRegistry => "this instance has no workflow definition registry",
        }
    }

    /// The variable that would turn it on.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::WorkflowStore => "AIWATCHER_WORKFLOW_STORE",
            Self::PipelineRegistry | Self::WorkflowRegistry => "AIWATCHER_PROMPT_STORE",
        }
    }
}

/// Why a run was not started.
#[derive(Debug, thiserror::Error)]
pub enum StartRefused {
    /// Nothing is wired behind what this run needs.
    #[error("{} ({})", .0.what(), .0.variable())]
    NotConfigured(Missing),

    /// Nothing is saved under that name.
    #[error("{0} not found")]
    Unknown(String),

    /// The request does not describe a run this system could start: a name
    /// that is not one, a time window on something that has none.
    #[error("{0}")]
    Invalid(String),

    /// The definition does not compile, or will not run here — every problem
    /// at once, for [`aiwatcher_datasets::order_of`]'s reason: somebody wiring
    /// a canvas fixes what they can see, and one problem per round trip
    /// teaches them to press the button again instead of reading it.
    #[error("{summary}")]
    Refused {
        summary: String,
        problems: Vec<String>,
    },

    /// What the curation pipeline registry refused.
    #[error(transparent)]
    Pipelines(#[from] RegistryError),

    /// What the workflow definition registry refused. Two variants rather than
    /// one, because two registries answer here and a reader of a refusal has
    /// to know which of them said it.
    #[error(transparent)]
    Definitions(#[from] DefinitionError),

    /// What the transaction that starts a run refused.
    #[error(transparent)]
    Command(#[from] HandleError),
}

impl StartRefused {
    /// Whether asking again would be told the same thing.
    ///
    /// The one question a scheduler has, answered by the type that knows
    /// instead of recovered from an HTTP status. A refused compile says the
    /// same thing on the next tick and so does a deployment with nothing
    /// wired; an object store that was unreachable for ten seconds does not.
    /// Filed the wrong way round, the first costs a slot a note claiming it
    /// was refused, and the second costs the day's run.
    #[must_use]
    pub fn says_the_same_next_time(&self) -> bool {
        match self {
            Self::NotConfigured(_) | Self::Unknown(_) | Self::Invalid(_) | Self::Refused { .. } => {
                true
            }
            // The only half of a registry failure worth coming back for, and
            // the same half on both of them. A rejected request is refused
            // identically forever, and a stored object that will not parse
            // does not start parsing.
            Self::Pipelines(error) => {
                !matches!(error, RegistryError::Store(port) if port.is_retryable())
            }
            Self::Definitions(error) => error.says_the_same_next_time(),
            Self::Command(error) => error.says_the_same_next_time(),
        }
    }
}

/// What may be started here, and with what.
///
/// Borrowed rather than held: the registries and the handler belong to the
/// process that wired them, so this costs nothing to assemble per request and
/// there is no second place a store lives. Every field is optional in the same
/// way the deployment is — a missing one is a [`Missing`] naming its variable
/// rather than a panic or an empty answer.
pub struct Executions<'a, S> {
    /// The transaction every start goes through.
    pub handler: Option<&'a ExecutionHandler<S>>,
    /// Where a curation pipeline's blocks are read from.
    pub pipelines: Option<&'a PipelineRegistry>,
    /// Where a registered workflow's definition is read from.
    pub workflows: Option<&'a DefinitionRegistry>,
    /// What this deployment decided about a hosted run's words.
    pub payloads: PayloadDefault,
    /// Whether there is a conversation archive to seal them into.
    pub archive: bool,
    /// The query engine this deployment runs.
    pub engine: QueryEngine,
    /// How long a query step may run here.
    pub query_timeout_seconds: Option<u64>,
    /// Woken once a run is started, so the loops that drain the outbox and the
    /// claim table do not wait out a poll interval. Absent in a process that
    /// runs neither, which loses nothing: the store is durable and whichever
    /// process does run them picks the work up.
    pub notify: Option<&'a tokio::sync::Notify>,
}

impl<S> std::fmt::Debug for Executions<'_, S> {
    /// What this deployment can start, rather than what it holds: the stores
    /// behind these are not `Debug` and their contents are not this type's to
    /// print.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executions")
            .field("handler", &self.handler.is_some())
            .field("pipelines", &self.pipelines.is_some())
            .field("workflows", &self.workflows.is_some())
            .field("payloads", &self.payloads)
            .field("archive", &self.archive)
            .field("engine", &self.engine)
            .field("query_timeout_seconds", &self.query_timeout_seconds)
            .finish_non_exhaustive()
    }
}

impl<S: WorkflowStore> Executions<'_, S> {
    /// Compile whichever kind of definition a name refers to, at its head.
    ///
    /// The two callers that have no request body reach a compiler through this
    /// and never pick one themselves — the schedule route, which refuses a
    /// definition that does not compile rather than letting it fail at nine
    /// tomorrow, and the tick that starts it. A `match` in each would be two
    /// answers to "what does this schedule run", free to disagree the day a
    /// third kind arrives: one of them would start a run and the other would
    /// refuse to save the schedule for it.
    ///
    /// The head, never a pinned revision. A schedule says *what* to run, and
    /// the run records the revision it pinned.
    ///
    /// # Errors
    ///
    /// [`StartRefused::Unknown`] when nothing is saved under that name, or
    /// [`StartRefused::Refused`] carrying every reason it does not compile.
    pub async fn compile_head(
        &self,
        kind: DefinitionKind,
        name: &str,
    ) -> Result<ExecutionPlan, StartRefused> {
        self.compile(
            &ExecutionTarget {
                kind,
                name: name.to_owned(),
                revision: None,
            },
            Window::default(),
        )
        .await
    }

    /// Read the definition at the revision it names, and compile it.
    ///
    /// The compiler is this crate's and the chain rules are
    /// `aiwatcher-datasets`'; this only decides *which* definition.
    ///
    /// # Errors
    ///
    /// Whichever of [`StartRefused`] the registry and the compiler produced.
    pub async fn compile(
        &self,
        target: &ExecutionTarget,
        window: Window,
    ) -> Result<ExecutionPlan, StartRefused> {
        match target.kind {
            DefinitionKind::CurationPipeline => {
                self.compile_pipeline(&target.name, target.revision.as_deref(), window.resolve())
                    .await
            }
            // The one kind with nothing to read: a scoring run's declaration
            // is addressed by its content, so what starts it is the digest and
            // there is no name whose head could be compiled. That is also what
            // stops a schedule being saved for one — a schedule says what to
            // run by name, and this has none.
            DefinitionKind::Evaluation => Err(StartRefused::Invalid(
                "a scoring run is started from its declaration rather than compiled from a name"
                    .to_owned(),
            )),
            DefinitionKind::Workflow => {
                if window.asked() {
                    return Err(StartRefused::Invalid(
                        "time windows apply only to curation pipelines".to_owned(),
                    ));
                }
                self.compile_workflow(&target.name, target.revision.as_deref())
                    .await
            }
        }
    }

    /// Start one run on a plan this caller already has.
    ///
    /// Split from [`Self::compile`] for the caller that needs the plan twice:
    /// the schedule route compiles the head to decide whether it may be saved
    /// at all, and then runs *that* plan, so a second read could pin a
    /// revision nobody checked.
    ///
    /// # Errors
    ///
    /// [`StartRefused::NotConfigured`] with no workflow store,
    /// [`StartRefused::Refused`] when the payload policy will not have it, or
    /// whatever the transaction refused.
    pub async fn start(&self, plan: ExecutionPlan, run: StartRun) -> Result<Started, StartRefused> {
        let handler = self.handler()?;
        let payloads = self.payload_policy(run.payloads)?;
        let execution_id = run.identity.of(&plan);
        // Derived from the execution, so a redelivered request — a retried
        // POST, a proxy that repeated it, a second worker on the same slot —
        // lands on the inbox rather than beside it.
        let message_id = MessageId::new(derive_uuid(&format!(
            "aiwatcher/execution/start/{execution_id}"
        )));
        let (owner, mode) = run.decided_by.parts();
        let now = OffsetDateTime::now_utc();
        let handled = handler
            .handle(
                &execution_id,
                WorkflowMessage::Command(WorkflowCommand::StartExecution {
                    execution_id: execution_id.clone(),
                    plan: Box::new(plan),
                    owner,
                    mode,
                    payloads,
                    requested_by: run.requested_by,
                    input: run.parameters,
                }),
                MessageMetadata::caused_by(&execution_id, &message_id, message_id.clone(), now),
                Now::at(now),
            )
            .await?;

        if let Some(notify) = self.notify {
            notify.notify_waiters();
        }
        Ok(Started {
            execution_id,
            handled,
        })
    }

    /// Compile what the request names and start it: the whole use case.
    ///
    /// # Errors
    ///
    /// Whichever of [`StartRefused`] applies.
    pub async fn compile_and_start(&self, request: StartRequest) -> Result<Started, StartRefused> {
        // Before the compile, and the order matters: an instance with no
        // workflow store cannot run *anything*, and saying so is a better
        // answer than reporting whichever other thing is also missing.
        self.handler()?;
        let plan = self.compile(&request.target, request.window).await?;
        self.start(plan, request.run).await
    }

    /// Where this run's words go, given what it asked for.
    ///
    /// # Errors
    ///
    /// [`StartRefused::Refused`] naming both variables when `sealed` was asked
    /// of an instance with no archive, or when the deployment has pinned its
    /// choice.
    pub fn payload_policy(
        &self,
        asked: Option<PayloadPolicy>,
    ) -> Result<PayloadPolicy, StartRefused> {
        let policy = self
            .payloads
            .resolve(asked)
            .map_err(|why| StartRefused::Refused {
                summary: "this instance will not run that execution".to_owned(),
                problems: vec![why],
            })?;
        if policy.needs_archive() && !self.archive {
            return Err(StartRefused::Refused {
                summary: "sealed payloads need the conversation archive".to_owned(),
                // Both, because turning one on without the other refuses again
                // — and a refusal that names one variable at a time is two
                // deployments' worth of round trips to reach a working
                // instance.
                problems: vec![
                    "set AIWATCHER_CONVERSATION_ARCHIVE=true".to_owned(),
                    "set AIWATCHER_CONVERSATION_KEYS to the keys it seals with".to_owned(),
                ],
            });
        }
        Ok(policy)
    }

    fn handler(&self) -> Result<&ExecutionHandler<S>, StartRefused> {
        self.handler
            .ok_or(StartRefused::NotConfigured(Missing::WorkflowStore))
    }

    async fn compile_pipeline(
        &self,
        name: &str,
        revision: Option<&str>,
        window: Option<ResolvedWindow>,
    ) -> Result<ExecutionPlan, StartRefused> {
        let registry = self
            .pipelines
            .ok_or(StartRefused::NotConfigured(Missing::PipelineRegistry))?;
        let pipeline = registry
            .pipeline(name, revision)
            .await?
            .ok_or_else(|| match revision {
                Some(revision) => StartRefused::Unknown(format!("pipeline {name} at {revision}")),
                None => StartRefused::Unknown(format!("pipeline {name}")),
            })?;
        compile_curation(
            &pipeline,
            CompileOptions {
                window,
                engine: self.engine,
                query_timeout_seconds: self.query_timeout_seconds,
            },
        )
        .map_err(|error| StartRefused::Refused {
            summary: format!("{name} does not compile to something that can be run"),
            problems: error.problems().to_vec(),
        })
    }

    async fn compile_workflow(
        &self,
        name: &str,
        revision: Option<&str>,
    ) -> Result<ExecutionPlan, StartRefused> {
        let registry = self
            .workflows
            .ok_or(StartRefused::NotConfigured(Missing::WorkflowRegistry))?;
        let saved = registry
            .get(name, revision)
            .await?
            .ok_or_else(|| StartRefused::Unknown(format!("workflow {name}")))?;
        saved
            .definition
            .compile()
            .map_err(|error| StartRefused::Refused {
                summary: format!("{name} does not compile"),
                problems: error.problems().to_vec(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aiwatcher_core::ports::PortError;
    use aiwatcher_core::prompts::ObjectStore;
    use aiwatcher_datasets::{
        BlockSpec, PipelineBlock, PipelineEdge, Registry as Pipelines, SavePipelineRequest,
    };
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

    use super::*;
    use crate::plan::{DefinitionRevision, PlanStep, RetryPolicy};
    use crate::store::memory::MemoryWorkflowStore;

    fn store() -> Arc<dyn ObjectStore> {
        Arc::new(MemoryObjectStore::new())
    }

    fn block(id: &str, spec: BlockSpec) -> PipelineBlock {
        PipelineBlock {
            id: id.to_owned(),
            title: String::new(),
            position: aiwatcher_datasets::BlockPosition::default(),
            spec,
        }
    }

    /// A chain that compiles: one source and the view that publishes it.
    async fn saved(pipelines: &Pipelines, name: &str) {
        pipelines
            .save_pipeline(SavePipelineRequest {
                name: name.to_owned(),
                description: String::new(),
                blocks: vec![
                    block(
                        "read",
                        BlockSpec::Source {
                            dataset: "runs".to_owned(),
                            arguments: BTreeMap::new(),
                        },
                    ),
                    block(
                        "publish",
                        BlockSpec::View {
                            dataset: Some(format!("{name}-rows")),
                        },
                    ),
                ],
                edges: vec![PipelineEdge {
                    from: "read".to_owned(),
                    to: "publish".to_owned(),
                }],
            })
            .await
            .expect("a pipeline that saves");
    }

    fn run(identity: RunIdentity) -> StartRun {
        StartRun {
            identity,
            parameters: BTreeMap::new(),
            requested_by: "tester".to_owned(),
            decided_by: Decider::Local,
            payloads: None,
        }
    }

    fn target(kind: DefinitionKind, name: &str) -> ExecutionTarget {
        ExecutionTarget {
            kind,
            name: name.to_owned(),
            revision: None,
        }
    }

    fn plan(revision: &str) -> ExecutionPlan {
        ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision(revision.to_owned()),
            vec![PlanStep {
                id: "read".to_owned(),
                runtime: crate::plan::RuntimeBinding::FlowPhp(crate::plan::FlowStepSpec {
                    script: "data_frame()->read(default)".to_owned(),
                    source: crate::plan::FlowSourceRef::default(),
                    blocks: Vec::new(),
                }),
                inputs: Vec::new(),
                outputs: Vec::new(),
                retry: RetryPolicy::default(),
                timeout_seconds: 300,
                cache: crate::plan::CachePolicy::Never,
            }],
            Vec::new(),
        )
    }

    #[tokio::test]
    async fn one_definition_compiles_and_starts_and_a_repeated_key_lands_on_that_run() {
        let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
        let pipelines = Pipelines::new(store(), "datasets");
        saved(&pipelines, "nightly").await;
        let executions = Executions {
            handler: Some(&handler),
            pipelines: Some(&pipelines),
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };

        let request = || StartRequest {
            target: target(DefinitionKind::CurationPipeline, "nightly"),
            window: Window::default(),
            run: run(RunIdentity::Key("once".to_owned())),
        };
        let first = executions
            .compile_and_start(request())
            .await
            .expect("a run this deployment can start");
        assert!(!first.handled.duplicate);

        // The key is the whole mechanism: the same key over the same plan is
        // the same id, the same id is the same stream, and the store's own
        // inbox answers the second request with what the first decided.
        let again = executions
            .compile_and_start(request())
            .await
            .expect("the run that key already names");
        assert_eq!(again.execution_id, first.execution_id);
        assert!(again.handled.duplicate);
    }

    #[test]
    fn one_key_against_an_edited_pipeline_is_a_different_execution() {
        // Otherwise somebody who fixed their pipeline and repeated their key
        // would get the *old* run back, with a 202 saying it was theirs.
        let key = RunIdentity::Key("nightly".to_owned());
        assert_ne!(key.of(&plan("ab")), key.of(&plan("cd")));
    }

    #[test]
    fn two_requests_with_no_key_are_two_runs() {
        let plan = plan("ab");
        assert_ne!(RunIdentity::Fresh.of(&plan), RunIdentity::Fresh.of(&plan));
        // And an id that becomes a correlation id, a partition key and a file
        // name carries none of the characters any of those three dislike.
        let id = RunIdentity::Fresh.of(&plan);
        assert!(
            id.as_str().chars().all(|c| c.is_ascii_alphanumeric()),
            "{id} has to survive being a file name"
        );
    }

    #[test]
    fn a_slot_keeps_the_id_it_was_given() {
        // The scheduler derives it from the definition and the moment, so two
        // workers that both find nine o'clock due reach one run. Deriving it
        // here instead — from the plan — would give them two, because they
        // read the head a moment apart and compile different `plan_id`s.
        let named = ExecutionId::new("schedule/nightly/2026-09-12T09:00:00Z");
        assert_eq!(RunIdentity::Named(named.clone()).of(&plan("ab")), named);
    }

    #[test]
    fn a_window_is_pinned_to_the_bounds_it_meant_when_it_was_asked_for() {
        let window = Window {
            seconds: Some(900),
            as_of: None,
        }
        .resolve()
        .expect("a window that was asked for");
        assert_eq!(window.to - window.from, 900);
    }

    #[test]
    fn two_requests_pinning_one_span_ask_the_same_question() {
        // Which is what makes a scheduled build of a fixed period cheap the
        // second time: the same span compiles to one `plan_id` and one cache
        // key. Without `as_of` every run pins a fresh span, which is right for
        // "curate the last hour" and is why a hit is asked for rather than had
        // by accident.
        let nightly = Window {
            seconds: Some(3600),
            as_of: Some(1_700_003_600),
        };
        assert_eq!(nightly.resolve(), nightly.resolve());
        let pinned = nightly.resolve().expect("a window that was asked for");
        assert_eq!(pinned.from, 1_700_000_000);
        assert_eq!(pinned.to, 1_700_003_600);
    }

    #[tokio::test]
    async fn a_window_on_something_that_has_none_is_refused_rather_than_ignored() {
        let executions = Executions::<MemoryWorkflowStore> {
            handler: None,
            pipelines: None,
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        // `as_of` on its own is half a window, and a caller who sent one to a
        // workflow has misunderstood the request rather than sent a field that
        // can be dropped.
        let refusal = executions
            .compile(
                &target(DefinitionKind::Workflow, "planner"),
                Window {
                    seconds: None,
                    as_of: Some(1_700_000_000),
                },
            )
            .await
            .expect_err("a window a workflow has nowhere to put");
        assert!(matches!(refusal, StartRefused::Invalid(_)));
        assert!(refusal.says_the_same_next_time());
    }

    #[tokio::test]
    async fn a_deployment_that_wired_nothing_refuses_the_slot_rather_than_holding_it_due() {
        // The bug this use case was extracted to fix. Through the HTTP module
        // this answered 501, which is 5xx, which the tick read as "come back in
        // a minute" — every minute, for ever, with the schedule's own card
        // saying it was still trying.
        let executions = Executions::<MemoryWorkflowStore> {
            handler: None,
            pipelines: None,
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        let refusal = executions
            .compile_and_start(StartRequest {
                target: target(DefinitionKind::CurationPipeline, "nightly"),
                window: Window::default(),
                run: run(RunIdentity::Fresh),
            })
            .await
            .expect_err("a deployment with no workflow store");
        // And the workflow store first, because an instance with none runs
        // nothing whatever else is missing.
        assert!(matches!(
            refusal,
            StartRefused::NotConfigured(Missing::WorkflowStore)
        ));
        assert!(refusal.says_the_same_next_time());
        assert_eq!(
            refusal.to_string(),
            "this instance has no workflow store configured (AIWATCHER_WORKFLOW_STORE)"
        );
    }

    #[tokio::test]
    async fn a_definition_nobody_saved_is_refused_and_a_pipeline_that_does_not_compile_says_why() {
        let handler = ExecutionHandler::new(MemoryWorkflowStore::new());
        let pipelines = Pipelines::new(store(), "datasets");
        let executions = Executions {
            handler: Some(&handler),
            pipelines: Some(&pipelines),
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        let missing = executions
            .compile_head(DefinitionKind::CurationPipeline, "nightly")
            .await
            .expect_err("nothing saved under that name");
        assert!(matches!(missing, StartRefused::Unknown(what) if what == "pipeline nightly"));

        // A chain written for another engine. The registry saves it — the text
        // is valid in the language it names — and the compiler refuses it,
        // which is the half this call has to carry back.
        pipelines
            .save_pipeline(SavePipelineRequest {
                name: "elsewhere".to_owned(),
                description: String::new(),
                blocks: vec![
                    block(
                        "read",
                        BlockSpec::Source {
                            dataset: "runs".to_owned(),
                            arguments: BTreeMap::new(),
                        },
                    ),
                    block(
                        "narrow",
                        BlockSpec::Transform {
                            steps: "df.limit(10)".to_owned(),
                            engine: aiwatcher_datasets::QueryEngine::DuckDb,
                        },
                    ),
                ],
                edges: vec![PipelineEdge {
                    from: "read".to_owned(),
                    to: "narrow".to_owned(),
                }],
            })
            .await
            .expect("a chain written for DuckDB saves on a Flow deployment");
        let refused = executions
            .compile_head(DefinitionKind::CurationPipeline, "elsewhere")
            .await
            .expect_err("a chain no process here would claim");
        let StartRefused::Refused { problems, .. } = &refused else {
            panic!("{refused} should carry every problem at once");
        };
        assert!(!problems.is_empty());
        assert!(refused.says_the_same_next_time());
    }

    /// A registered workflow whose stored head will not read back.
    ///
    /// The slot this schedule owns is settled rather than left due, which is
    /// the whole difference: the same case used to arrive as one opaque
    /// string, read as an unreachable store, and be retried every minute for
    /// ever behind a card that said it was still trying.
    #[tokio::test]
    async fn a_corrupt_registered_workflow_settles_its_slot_rather_than_asking_again() {
        let objects = store();
        let registry = DefinitionRegistry::new(objects.clone());
        let definition: crate::definition::WorkflowSpec =
            serde_json::from_value(serde_json::json!(
                {"name": "house/import", "version": "1", "steps": [
                    {"id": "acquire", "task_ref": "acquire@1", "queue": "local",
                     "timeout_seconds": 30, "outputs": ["rows"]}
                ]}
            ))
            .expect("a workflow this deployment would run");
        registry
            .save(
                definition,
                "operator".to_owned(),
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("a definition that compiles saves");

        let head = objects
            .list("workflows/heads/")
            .await
            .expect("the head this registry just wrote")
            .remove(0)
            .key;
        objects
            .put(&head, b"{not a definition".to_vec())
            .await
            .expect("something else entirely, under the key the head is at");

        let executions = Executions::<MemoryWorkflowStore> {
            handler: None,
            pipelines: None,
            workflows: Some(&registry),
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        let refusal = executions
            .compile_head(DefinitionKind::Workflow, "house/import")
            .await
            .expect_err("bytes that are not a workflow definition");
        let StartRefused::Definitions(DefinitionError::Corrupt { key, .. }) = &refusal else {
            panic!("{refusal} should say which object, and that it is this registry's");
        };
        assert_eq!(key, &head);
        assert!(
            refusal.says_the_same_next_time(),
            "a stored object that will not parse does not start parsing"
        );
    }

    #[test]
    fn only_a_bad_moment_is_worth_coming_back_for() {
        // The three that a status code could not tell apart, all 5xx and all
        // permanent: a registry this deployment never wired (501), a stored
        // object that will not read back (500), and a store that understood
        // the read and refused it (502).
        let settled: Vec<StartRefused> = vec![
            StartRefused::NotConfigured(Missing::PipelineRegistry),
            RegistryError::Corrupt {
                key: "datasets/pipelines/x/head.json".to_owned(),
                message: "expected value".to_owned(),
            }
            .into(),
            RegistryError::Store(PortError::Rejected {
                target: "the object store",
                message: "that prefix is not writable".to_owned(),
            })
            .into(),
            HandleError::Decision(crate::DecisionError::AlreadyStarted).into(),
            // And the same three from the other registry a start compiles
            // from. These read as a bad moment until the day this package
            // gave that registry an error type with three answers in it.
            DefinitionError::Corrupt {
                key: "workflows/heads/abc.json".to_owned(),
                message: "expected value".to_owned(),
            }
            .into(),
            DefinitionError::Store(PortError::Rejected {
                target: "the object store",
                message: "that prefix is not readable".to_owned(),
            })
            .into(),
            DefinitionError::Refused(vec!["step 'fit' names no runtime".to_owned()]).into(),
        ];
        for refusal in settled {
            assert!(
                refusal.says_the_same_next_time(),
                "{refusal} will say the same thing at nine tomorrow"
            );
        }

        let come_back: Vec<StartRefused> = vec![
            RegistryError::Store(PortError::Unavailable {
                target: "the object store",
                message: "connection refused".to_owned(),
            })
            .into(),
            HandleError::Contended { retries: 8 }.into(),
            DefinitionError::Store(PortError::Unavailable {
                target: "the object store",
                message: "connection refused".to_owned(),
            })
            .into(),
        ];
        for refusal in come_back {
            assert!(
                !refusal.says_the_same_next_time(),
                "{refusal} leaves the slot due rather than costing the day's run"
            );
        }
    }

    #[test]
    fn sealed_words_need_the_archive_and_the_refusal_names_both_variables() {
        let executions = Executions::<MemoryWorkflowStore> {
            handler: None,
            pipelines: None,
            workflows: None,
            payloads: PayloadDefault::default(),
            archive: false,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        let refusal = executions
            .payload_policy(Some(PayloadPolicy::Sealed))
            .expect_err("an instance with nothing to seal into");
        let StartRefused::Refused { problems, .. } = &refusal else {
            panic!("{refusal} should carry both variables");
        };
        assert_eq!(problems.len(), 2);
        assert!(refusal.says_the_same_next_time());
    }

    #[test]
    fn a_deployment_that_pinned_its_policy_refuses_a_run_that_asks_for_another() {
        let executions = Executions::<MemoryWorkflowStore> {
            handler: None,
            pipelines: None,
            workflows: None,
            payloads: PayloadDefault {
                policy: PayloadPolicy::Sealed,
                locked: true,
            },
            archive: true,
            engine: QueryEngine::default(),
            query_timeout_seconds: None,
            notify: None,
        };
        assert!(
            executions
                .payload_policy(Some(PayloadPolicy::External))
                .is_err()
        );
        // And a run that asked for nothing gets the deployment's answer.
        assert_eq!(
            executions.payload_policy(None).expect("the pinned policy"),
            PayloadPolicy::Sealed
        );
    }
}
