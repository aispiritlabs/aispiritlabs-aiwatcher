//! Attempts nothing in this process will claim, said out loud.
//!
//! The work role's claim filter is built from its `ExecutorRegistry`, so a
//! process never claims work it cannot perform. The other side of that rule is
//! an attempt whose runtime no registered executor performs: nothing takes it,
//! and it sits `pending` until its run is cancelled. The sharpest way to get
//! one is a release that switches `AIWATCHER_QUERY_ENGINE` while a run still
//! has an attempt of the old engine pending or retrying (ADR_0028).
//!
//! So the work role counts them at start-up and every [`EVERY`] after, and
//! warns with the kind, the count and the setting that decided it. It reads
//! [`WorkflowStore::unclaimed_attempts`] and nothing else, so it claims, leases
//! and rewrites nothing; and a count that fails is a warning, because a
//! measurement must never cost a start or a tick.
//!
//! Only the runtimes the work role's reactor would claim are judged:
//! `publish_dataset` is the serve role's, `python_task` a worker's, and
//! `human_input` never reaches the claim table. Two work processes may hold
//! different executors, so the message says "here" and names the setting —
//! except for a query engine, which is one per deployment: an attempt written
//! for another engine is claimed by nobody.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aiwatcher_datasets::QueryEngine;
use aiwatcher_execution::{RuntimeKind, StoreError, WorkflowStore};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// How often the count is taken after the one at start-up.
///
/// Five minutes. The claim table holds live attempts only, so a count is one
/// small query; what bounds the cadence is the log, where a stranded attempt
/// repeats until somebody acts on it, and once a second would bury everything
/// else. The run was stranded at the release, and the count at start-up is
/// the one that says so first.
pub const EVERY: Duration = Duration::from_secs(5 * 60);

/// What this process registered, and the settings that decided it.
///
/// Carried so a warning can say *why* nothing claims an attempt rather than
/// only that nothing does: "no executor" sends somebody looking, and the name
/// of the variable does not.
#[derive(Clone, Debug)]
pub struct Wiring {
    /// The runtimes the work role's reactor registered. The same list its
    /// claim filter is built from.
    pub registered: Vec<RuntimeKind>,
    pub query_engine: QueryEngine,
    pub query_url: bool,
    pub ml_pipeline_url: bool,
    /// Both executors write a step's rows to the object store, and neither is
    /// built without one, whatever address it was given.
    pub object_store: bool,
}

impl Wiring {
    #[must_use]
    pub fn of(config: &Config, registered: Vec<RuntimeKind>, object_store: bool) -> Self {
        Self {
            registered,
            query_engine: config.query_engine,
            query_url: config.query_url.is_some(),
            ml_pipeline_url: config.ml_pipeline_url.is_some(),
            object_store,
        }
    }
}

/// Whether a reactor in the work role is the process that would claim this.
///
/// A `match` with no wildcard, so a runtime added later is a decision rather
/// than a default. Left out here while the work role claims it, it is the
/// stranded attempt this module exists for; judged here while another role
/// claims it, it is a warning that is wrong every five minutes.
const fn claimed_by_the_work_role(kind: RuntimeKind) -> bool {
    match kind {
        RuntimeKind::FlowPhp
        | RuntimeKind::DataFusion
        | RuntimeKind::DuckDb
        | RuntimeKind::Marimo => true,
        // The serve role's reactor, a worker by its queue, a pod by its key —
        // the work role starts the pod and claims nothing (ADR_0029) — and a
        // wait.
        RuntimeKind::PublishDataset
        | RuntimeKind::ScoreEvaluation
        | RuntimeKind::PythonTask
        | RuntimeKind::ContainerJob
        | RuntimeKind::HumanInput => false,
    }
}

/// Attempts of one runtime that this process will not claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stranded {
    pub runtime: RuntimeKind,
    pub attempts: u64,
}

/// What one count found.
#[derive(Debug)]
pub enum Finding {
    /// Nothing is waiting that this process cannot run.
    Clear,
    Stranded(Vec<Stranded>),
    /// The store could not be asked. Said as a warning, and the next count is
    /// the retry.
    CouldNotCount(StoreError),
}

/// The waiting attempts whose runtime this role would claim and did not
/// register.
#[must_use]
pub fn stranded(
    unclaimed: &BTreeMap<RuntimeKind, u64>,
    registered: &[RuntimeKind],
) -> Vec<Stranded> {
    unclaimed
        .iter()
        .filter(|&(&runtime, &attempts)| {
            attempts > 0 && claimed_by_the_work_role(runtime) && !registered.contains(&runtime)
        })
        .map(|(&runtime, &attempts)| Stranded { runtime, attempts })
        .collect()
}

/// One count's finding, from the store's answer.
///
/// Takes the answer rather than the store, so the rule that a failure is a
/// finding and never an error is a function of a value rather than of
/// whichever backend happened to fail.
#[must_use]
pub fn assess(
    counted: aiwatcher_execution::Result<BTreeMap<RuntimeKind, u64>>,
    registered: &[RuntimeKind],
) -> Finding {
    match counted {
        Err(error) => Finding::CouldNotCount(error),
        Ok(unclaimed) => {
            let found = stranded(&unclaimed, registered);
            if found.is_empty() {
                Finding::Clear
            } else {
                Finding::Stranded(found)
            }
        }
    }
}

/// Why nothing here will claim these, in the words of the setting that
/// decided it.
#[must_use]
pub fn explain(stranded: Stranded, wiring: &Wiring) -> String {
    let Stranded { runtime, attempts } = stranded;
    let one = attempts == 1;
    let head = format!(
        "{attempts} {} {} waiting and no executor here will claim {}",
        runtime.as_str(),
        if one { "attempt is" } else { "attempts are" },
        if one { "it" } else { "them" },
    );
    let pending = if one {
        "its run stays pending until it is cancelled"
    } else {
        "their runs stay pending until they are cancelled"
    };
    let deployed = wiring.query_engine.as_str();

    match runtime.query_engine() {
        // Another engine's. Nothing in this deployment will ever take it,
        // whatever address is set: the engine is one per deployment.
        Some(written_for) if written_for != wiring.query_engine => format!(
            "{head}: {} written for {}, and this deployment runs AIWATCHER_QUERY_ENGINE={deployed}. \
             {}; a release switches engines between runs, not during one (ADR_0028)",
            if one { "it was" } else { "they were" },
            written_for.as_str(),
            capitalised(pending),
        ),
        Some(_) => format!(
            "{head}: this deployment runs AIWATCHER_QUERY_ENGINE={deployed}, but {}. Unless another \
             work process holds a {deployed} executor, {pending}",
            missing_client("AIWATCHER_QUERY_URL", wiring.query_url, wiring.object_store),
        ),
        None if runtime == RuntimeKind::Marimo => format!(
            "{head}: {}. Unless another work process holds a notebook executor, {pending}",
            missing_client(
                "AIWATCHER_ML_PIPELINE_URL",
                wiring.ml_pipeline_url,
                wiring.object_store
            ),
        ),
        None => format!(
            "{head}: no {} executor is registered in this process. Unless another work process \
             holds one, {pending}",
            runtime.as_str(),
        ),
    }
}

/// Why a client that would have claimed a runtime is not registered.
fn missing_client(variable: &str, address: bool, object_store: bool) -> String {
    if !address {
        format!("{variable} is unset")
    } else if !object_store {
        format!("{variable} is set and there is no object store to write a step's rows to")
    } else {
        format!("{variable} is set and its client did not build (see the error logged at start-up)")
    }
}

fn capitalised(sentence: &str) -> String {
    let mut characters = sentence.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// Count once, and say what was found.
///
/// Returns the finding so a caller can act on it. The loop only logs, which is
/// the point: nothing about a count feeds back into what this process claims.
pub async fn check(store: &dyn WorkflowStore, wiring: &Wiring, now: OffsetDateTime) -> Finding {
    let finding = assess(store.unclaimed_attempts(now).await, &wiring.registered);
    match &finding {
        Finding::Clear => {
            tracing::debug!("every attempt waiting for this role has an executor here");
        }
        Finding::Stranded(stranded) => {
            for &one in stranded {
                let why = explain(one, wiring);
                if one.runtime.query_engine().is_some() {
                    tracing::warn!(
                        runtime = one.runtime.as_str(),
                        attempts = one.attempts,
                        query_engine = wiring.query_engine.as_str(),
                        "{why}"
                    );
                } else {
                    tracing::warn!(
                        runtime = one.runtime.as_str(),
                        attempts = one.attempts,
                        "{why}"
                    );
                }
            }
        }
        Finding::CouldNotCount(error) => {
            tracing::warn!(
                %error,
                "could not count the attempts no executor here performs; the next count retries"
            );
        }
    }
    finding
}

/// Count at start-up, then every [`EVERY`], until shutdown.
#[must_use]
pub fn spawn(
    store: Arc<dyn WorkflowStore>,
    wiring: Wiring,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Before the first sleep. A release that switched engines has just
            // happened, and start-up is when somebody is reading the log.
            check(store.as_ref(), &wiring, OffsetDateTime::now_utc()).await;
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(EVERY) => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use aiwatcher_core::{CausationId, CorrelationId, MessageId};
    use aiwatcher_execution::store::memory::MemoryWorkflowStore;
    use aiwatcher_execution::{
        AppendRequest, AttemptKey, AttemptRow, AttemptWrite, ExecutionId, ExecutionMode,
        ExecutionOwner, ExpectedVersion, MessageMetadata, PendingMessage, RunProjection, RunState,
        StateType, WorkflowCommand, WorkflowMessage,
    };

    /// A DataFusion deployment, with its address and its object store.
    fn datafusion() -> Wiring {
        Wiring {
            registered: vec![RuntimeKind::DataFusion],
            query_engine: QueryEngine::DataFusion,
            query_url: true,
            ml_pipeline_url: false,
            object_store: true,
        }
    }

    /// One decision whose only content is attempt rows, written straight into
    /// the store. The loop reads the claim table and nothing else.
    fn dispatched(execution: &ExecutionId, rows: Vec<AttemptWrite>) -> AppendRequest {
        AppendRequest {
            expected_version: ExpectedVersion::Any,
            input: PendingMessage::input(
                WorkflowMessage::Command(WorkflowCommand::PauseExecution),
                MessageMetadata {
                    schema_version: aiwatcher_execution::message::SCHEMA_VERSION,
                    message_id: MessageId::new(format!("{execution}/dispatch")),
                    occurred_at: OffsetDateTime::UNIX_EPOCH,
                    correlation_id: CorrelationId::new(execution.as_str()),
                    causation_id: CausationId::new("test"),
                    trace_id: None,
                    span_id: None,
                    step_id: None,
                    attempt: None,
                },
            ),
            outputs: Vec::new(),
            projection: RunProjection {
                execution_id: execution.clone(),
                plan_id: String::new(),
                definition_name: "switched".to_owned(),
                owner: ExecutionOwner::Local,
                mode: ExecutionMode::Compiled,
                payloads: Default::default(),
                state: RunState::of(StateType::Running),
                requested_by: "a test".to_owned(),
                steps: Vec::new(),
                last_message_version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            outbox: Vec::new(),
            checkpoint: None,
            timers: Vec::new(),
            attempts: rows,
        }
    }

    #[tokio::test]
    async fn an_attempt_left_by_an_engine_switch_is_reported_and_left_where_it_was() {
        // The release switched from Flow to DataFusion while a run still had a
        // Flow attempt waiting. Nothing claims it, and until now nothing said
        // so.
        let store = MemoryWorkflowStore::new();
        let execution = ExecutionId::new("switched-mid-run");
        let row = AttemptRow::claimable(
            AttemptKey::new(execution.clone(), "read", 1),
            RuntimeKind::FlowPhp,
            MessageId::new("cmd-read"),
        );
        store
            .append(
                &execution,
                dispatched(&execution, vec![AttemptWrite::Dispatch(row.clone())]),
            )
            .await
            .expect("a dispatch");

        let finding = check(&store, &datafusion(), OffsetDateTime::UNIX_EPOCH).await;
        let Finding::Stranded(stranded) = finding else {
            panic!("a flow attempt in a datafusion deployment went unreported: {finding:?}");
        };
        assert_eq!(
            stranded,
            [Stranded {
                runtime: RuntimeKind::FlowPhp,
                attempts: 1
            }]
        );

        // Counted, never claimed: the row is exactly what the dispatch wrote.
        assert_eq!(
            store.attempt(&row.key).await.expect("a read"),
            Some(row),
            "the check claimed or rewrote the attempt it was counting"
        );
    }

    #[test]
    fn the_warning_names_both_engines_and_the_setting_that_chose_one() {
        let said = explain(
            Stranded {
                runtime: RuntimeKind::FlowPhp,
                attempts: 3,
            },
            &datafusion(),
        );
        assert_eq!(
            said,
            "3 flow_php attempts are waiting and no executor here will claim them: they were \
             written for flow, and this deployment runs AIWATCHER_QUERY_ENGINE=datafusion. \
             Their runs stay pending until they are cancelled; a release switches engines \
             between runs, not during one (ADR_0028)"
        );
    }

    #[test]
    fn the_deployed_engine_with_no_client_names_the_address_rather_than_the_engine() {
        // Setting the address fixes this one, so that is what it says. Talk of
        // switching engines would send somebody the wrong way.
        let wiring = Wiring {
            registered: Vec::new(),
            query_url: false,
            ..datafusion()
        };
        let said = explain(
            Stranded {
                runtime: RuntimeKind::DataFusion,
                attempts: 2,
            },
            &wiring,
        );
        assert!(said.contains("AIWATCHER_QUERY_URL is unset"), "{said}");
        assert!(said.contains("AIWATCHER_QUERY_ENGINE=datafusion"), "{said}");
        assert!(!said.contains("between runs"), "{said}");
    }

    #[test]
    fn a_notebook_attempt_names_the_address_that_would_have_claimed_it() {
        let said = explain(
            Stranded {
                runtime: RuntimeKind::Marimo,
                attempts: 1,
            },
            &datafusion(),
        );
        assert!(
            said.starts_with("1 marimo attempt is waiting and no executor here will claim it"),
            "{said}"
        );
        assert!(
            said.contains("AIWATCHER_ML_PIPELINE_URL is unset"),
            "{said}"
        );
        assert!(!said.contains("AIWATCHER_QUERY_ENGINE"), "{said}");
    }

    #[test]
    fn a_runtime_registered_here_or_claimed_by_another_role_is_not_stranded() {
        // `publish_dataset` is the serve role's and `python_task` a worker's;
        // this process can speak for neither, and a warning about either would
        // be wrong in every split deployment.
        let unclaimed = BTreeMap::from([
            (RuntimeKind::FlowPhp, 2),
            (RuntimeKind::DataFusion, 5),
            (RuntimeKind::PublishDataset, 1),
            (RuntimeKind::PythonTask, 3),
        ]);
        assert_eq!(
            stranded(&unclaimed, &[RuntimeKind::DataFusion]),
            [Stranded {
                runtime: RuntimeKind::FlowPhp,
                attempts: 2
            }]
        );
        assert!(matches!(assess(Ok(BTreeMap::new()), &[]), Finding::Clear));
    }

    #[test]
    fn a_count_that_fails_is_a_finding_and_never_an_error() {
        // A measurement must never cost a start or a tick. The loop logs this
        // and sleeps, and the next count is the retry.
        let finding = assess(
            Err(StoreError::Backend("the database is down".to_owned())),
            &[RuntimeKind::DataFusion],
        );
        assert!(matches!(finding, Finding::CouldNotCount(_)), "{finding:?}");
    }
}
