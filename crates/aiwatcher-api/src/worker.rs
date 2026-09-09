//! The protocol a process somebody else operates speaks to this one.
//!
//! A worker runs registered Python functions on a laptop or in someone's own
//! pod, and reaches the store through the API it already holds a token for.
//!
//! It is the reactor with a seam in the middle. [`Reactor::poll_once`] is
//! claim → load the plan → cache lookup → `step.started` → **perform** →
//! re-check the lease → record → report, and only `perform` crosses the seam:
//!
//! ```text
//!   POST /worker/claims          Reactor::take      claim, plan, cache, step.started
//!        ── assignment ──►                          the worker performs it
//!   POST /worker/claims/…/result Reactor::resume    rebuild the claim
//!                                Reactor::settle    lease, catalog, step.completed
//! ```
//!
//! The worker owns its own function and nothing else — not whether a cache
//! entry answers, not whether a retry is due, and least of all whether its
//! lease still holds, which is the one thing a claimant cannot check about
//! itself.
//!
//! **The lease is the authorization.** A token names queues
//! (`AIWATCHER_AUTH_INGEST_TOKENS`, `name[queue]=secret`), which decides what
//! may be *claimed*; every route afterwards checks that this caller holds that
//! attempt's lease. Worker names are not secret, so the lease and the token's
//! queue scope are checked **together** — either alone lets a token settle
//! another queue's attempt by guessing the name it was claimed under.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{OpenApi, ToSchema};

use aiwatcher_core::ArtifactRef;
use aiwatcher_core::ports::AttemptArtifacts;
use aiwatcher_execution::claim::{AttemptKey, ClaimFilter};
use aiwatcher_execution::message::{Direction, WorkflowEvent};
use aiwatcher_execution::plan::{RuntimeBinding, RuntimeKind};
use aiwatcher_execution::reactor::{Claimed, Reactor, Taken};
use aiwatcher_execution::state::{ExecutionId, FailureClass, StepError};
use aiwatcher_execution::{ActivityResult, ExecutionHandler, ExecutorRegistry, WorkflowStore};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(claim, heartbeat, report, read_input, write_output))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/worker/claims", post(claim))
        .route(
            "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/heartbeat",
            post(heartbeat),
        )
        .route(
            "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/result",
            post(report),
        )
        .route(
            "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/inputs/{name}",
            axum::routing::get(read_input),
        )
        .route(
            "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/outputs/{name}",
            post(write_output),
        )
}

// ── What crosses the wire ────────────────────────────────────────────────────

/// A worker saying what it is and what it can run.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimRequest {
    /// Claim only this attempt, while still enforcing queue and task capabilities.
    #[serde(default)]
    pub attempt: Option<AttemptKey>,
    /// The name this worker holds leases under.
    ///
    /// Unique per process — a pod name, a host and a pid. Two workers sharing
    /// one name would each believe they hold the other's leases, which is the
    /// one thing the lease exists to prevent.
    pub worker: String,
    /// The queues to take from. Every one must be a queue this caller's token
    /// authorises; an empty list means all of them.
    #[serde(default)]
    pub queues: Vec<String>,
    /// The `name@version` refs this worker has code for.
    ///
    /// Required, and an empty list claims nothing. **Never let a process claim
    /// work it cannot perform**: a worker holding `stage@1` that took a
    /// `stage@2` attempt would fail a run over a rolling deploy in which both
    /// versions are briefly alive.
    pub tasks: Vec<String>,
}

/// One attempt, and everything performing it needs.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkAssignment {
    /// The result route can acknowledge a matching outcome from durable history.
    pub report_idempotent: bool,
    /// Named row outputs required by the pinned plan.
    pub outputs: Vec<String>,
    pub execution_id: String,
    /// Correlation supplied by the platform; the worker emits only child spans.
    pub workflow_id: String,
    pub trace_id: String,
    pub parent_span_id: String,
    pub step_id: String,
    pub attempt: u32,
    /// `name@version`, from the plan. The worker matches it against what it
    /// registered; the server already did, which is why this is a fact rather
    /// than a request.
    pub task_ref: String,
    pub queue: String,
    /// `<execution>/<step>/<attempt>` — what the work is idempotent by, and
    /// what a task with a side effect keys it on.
    pub context_id: String,
    /// The step's own deadline. Past it the server stops waiting for a report.
    pub timeout_seconds: u64,
    /// When this claim stops being trusted unless it is renewed.
    ///
    /// An instant rather than a duration, and the difference matters: a worker
    /// told "five minutes" has to agree with this process about when the five
    /// minutes started, and the two clocks are the whole question. A deadline
    /// is one fact. It also means a client cannot hard-code the lease length
    /// and keep beating at the wrong rate the day the rule moves —
    /// `aiwatcher_jobs` owns that number, and nothing here restates it.
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub lease_expires_at: time::OffsetDateTime,
    /// The step's authored parameters.
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Value>,
    /// What the run itself was started with.
    #[schema(value_type = Object)]
    pub parameters: BTreeMap<String, Value>,
    /// What the parents produced, by name. The bytes are read one at a time
    /// through this attempt's own `inputs/{name}` route.
    pub inputs: Vec<ArtifactRef>,
    /// Whether somebody else held this attempt first.
    ///
    /// The reactor asks its runtime by idempotency key before re-running a
    /// takeover, because a timeout proves nothing about whether the previous
    /// call finished. A worker has no such lookup to offer, so it is told
    /// instead: a task with a side effect must be idempotent by `context_id`,
    /// and this is when that matters.
    pub is_retake: bool,
}

/// A worker saying which attempt it is talking about.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerBody {
    pub worker: String,
}

/// What a worker did with its attempt.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WorkReport {
    /// It ran and produced something.
    Completed {
        /// References this attempt's own `outputs/{name}` route handed back.
        #[serde(default)]
        outputs: Vec<ArtifactRef>,
        /// A bounded control value. Rows go in an artifact — a result that
        /// grows with the data is one that eventually cannot be replayed.
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        diagnostics: Option<String>,
        /// Whether this may answer for its cache key later. The worker
        /// answers it because only the process that ran the work knows what it
        /// managed to do.
        #[serde(default = "yes")]
        cacheable: bool,
    },
    /// It did not.
    Failed {
        /// Whether trying again could plausibly produce a different answer.
        /// The worker classifies it, because the process that made the call is
        /// the one that knows.
        class: FailureClass,
        message: String,
        #[serde(default)]
        diagnostics: Option<String>,
    },
}

const fn yes() -> bool {
    true
}

/// What the decider did with a report.
#[derive(Debug, Serialize, ToSchema)]
pub struct Settled {
    pub step_id: String,
    pub attempt: u32,
    pub succeeded: bool,
}

/// Rows, on their way to or from a worker.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct RowsBody {
    #[schema(value_type = Vec<Object>)]
    pub rows: Vec<Value>,
}

// ── The seam ─────────────────────────────────────────────────────────────────

fn handler(state: &AppState) -> ApiResult<&Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>> {
    state
        .executions
        .as_ref()
        .ok_or(ApiError::ExecutionsDisabled)
}

fn artifacts(state: &AppState) -> ApiResult<&Arc<dyn AttemptArtifacts>> {
    state
        .artifacts
        .as_ref()
        .ok_or(ApiError::WorkerArtifactsDisabled)
}

/// The reactor half of this process, leasing under `worker`'s name.
///
/// It holds no executors on purpose: the work happens elsewhere, and
/// [`Reactor::take`] is told what is performable separately. Registering one
/// here would be this process claiming a worker's attempt, which is the first
/// guardrail read from the other end.
fn reactor(state: &AppState, worker: &str) -> ApiResult<Reactor<Arc<dyn WorkflowStore>>> {
    let handler = ExecutionHandler::new(Arc::clone(handler(state)?.store()));
    let reactor = Reactor::new(handler, ExecutorRegistry::new(), worker.to_owned());
    Ok(match &state.catalog {
        Some(catalog) => reactor.with_catalog(Arc::clone(catalog)),
        None => reactor,
    })
}

/// The one runtime a worker performs.
const PERFORMABLE: &[RuntimeKind] = &[RuntimeKind::PythonTask];

fn key_of(execution_id: &str, step_id: &str, attempt: u32) -> AttemptKey {
    AttemptKey::new(ExecutionId::new(execution_id.to_owned()), step_id, attempt)
}

/// The claim this caller holds, or a refusal saying which way it failed.
///
/// Two checks, and they are not the same one. Holding the lease says this
/// worker took the attempt and still has it; the queue scope says this
/// caller's *token* was allowed to. Worker names are not secret, so without
/// the second a token for one queue could settle another queue's attempt by
/// naming the worker that claimed it.
async fn held(
    state: &AppState,
    caller: &Caller,
    worker: &str,
    key: &AttemptKey,
) -> ApiResult<(Reactor<Arc<dyn WorkflowStore>>, Claimed)> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    let reactor = reactor(state, worker)?;
    let now = time::OffsetDateTime::now_utc();
    let claimed = reactor
        .resume(key, now)
        .await
        .map_err(ApiError::Execution)?
        .ok_or_else(|| ApiError::LeaseLost(key.idempotency_key()))?;

    let queue = claimed.row.queue.as_deref().unwrap_or_default();
    if !caller.identity().may_claim(queue) {
        return Err(ApiError::Forbidden {
            needed: aiwatcher_auth::Role::Editor,
            held: caller.identity().role(),
        });
    }
    Ok((reactor, claimed))
}

// ── Routes ───────────────────────────────────────────────────────────────────

/// Take one attempt, or say there was none.
///
/// The server does everything up to the work itself: it claims the row, loads
/// the plan the run pinned, answers from the cache if an entry already covers
/// this key, and reports `step.started`. A 204 means there was nothing — or
/// that a cache entry settled it without anybody running anything, which is a
/// finished attempt rather than an idle one and looks the same from here.
#[utoipa::path(
    post,
    path = "/api/v1/worker/claims",
    request_body = ClaimRequest,
    responses(
        (status = 200, body = WorkAssignment),
        (status = 204, description = "nothing claimable"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "worker",
)]
async fn claim(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ClaimRequest>,
) -> ApiResult<axum::response::Response> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    if body.worker.trim().is_empty()
        || body.worker.len() > 256
        || body.worker.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest(
            "worker must be a nonempty process identity of at most 256 bytes".to_owned(),
        ));
    }

    let queues = match caller.identity().claimable_queues() {
        // A token names its queues, and an empty request means all of them.
        // Naming one it does not hold is a refusal rather than a silent
        // narrowing: a worker configured for the wrong queue would otherwise
        // poll an empty filter forever with nothing saying why.
        Some(allowed) => {
            if body.queues.is_empty() {
                allowed.to_vec()
            } else {
                for wanted in &body.queues {
                    if !allowed.iter().any(|held| held == wanted) {
                        return Err(ApiError::BadRequest(format!(
                            "this token does not authorise the queue {wanted:?}"
                        )));
                    }
                }
                body.queues.clone()
            }
        }
        // No provider is configured, so there is nothing to narrow against.
        None => body.queues.clone(),
    };
    if queues.is_empty() {
        return Err(ApiError::BadRequest(
            "no queue to claim on: this token authorises none and the request named none"
                .to_owned(),
        ));
    }

    let reactor = reactor(&state, &body.worker)?;
    let mut filter = ClaimFilter::for_queues(&queues, &body.tasks);
    filter.attempt = body.attempt;
    let now = time::OffsetDateTime::now_utc();
    let taken = reactor
        .take(&filter, PERFORMABLE, now)
        .await
        .map_err(ApiError::Execution)?;

    let claimed = match taken {
        Taken::Idle => return Ok(StatusCode::NO_CONTENT.into_response()),
        Taken::Settled(performed) => {
            // A cache hit or a release. Either way there is nothing for the
            // worker to do and the attempt is no longer waiting on it.
            tracing::debug!(worker = %body.worker, ?performed, "a claim settled without work");
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        Taken::Work(claimed) => *claimed,
    };

    Ok(Json(assignment(&claimed)).into_response())
}

use axum::response::IntoResponse;

fn assignment(claimed: &Claimed) -> WorkAssignment {
    let (task_ref, queue, params) = match &claimed.command.step.runtime {
        RuntimeBinding::PythonTask(spec) => (
            spec.task_ref.clone(),
            spec.queue.clone(),
            spec.params.clone(),
        ),
        // Unreachable: `take` was told `PERFORMABLE` and released anything
        // else before it reported a start. Answered rather than panicked,
        // because a claimed attempt is a live lease and a panic here would
        // hold it for the whole five minutes.
        other => (
            other.kind().as_str().to_owned(),
            String::new(),
            BTreeMap::new(),
        ),
    };
    let trace = aiwatcher_core::TraceId::derive(claimed.row.key.execution_id.as_str());
    let span_key = aiwatcher_core::EventType::StepStarted.span_key(
        Some(&format!(
            "{}/{}",
            claimed.row.key.step_id, claimed.row.key.attempt
        )),
        None,
    );
    WorkAssignment {
        report_idempotent: true,
        outputs: claimed
            .command
            .step
            .outputs
            .iter()
            .map(|output| output.name.clone())
            .collect(),
        workflow_id: claimed.context.plan.definition_name.clone(),
        trace_id: trace.to_hex(),
        parent_span_id: aiwatcher_core::SpanId::derive(trace, &span_key).to_hex(),
        execution_id: claimed.row.key.execution_id.as_str().to_owned(),
        step_id: claimed.row.key.step_id.clone(),
        attempt: claimed.row.key.attempt,
        task_ref,
        queue,
        context_id: claimed.context.context_id.clone(),
        timeout_seconds: claimed.command.step.timeout_seconds,
        // Always `Some`: `take` claimed this row, so it has a `claimed_at`.
        // Falling back to now rather than unwrapping — an expired-looking
        // deadline makes a worker re-claim, which is the safe direction.
        lease_expires_at: claimed
            .row
            .lease_expires_at()
            .unwrap_or_else(time::OffsetDateTime::now_utc),
        params,
        parameters: claimed.command.parameters.clone(),
        inputs: claimed.command.inputs.clone(),
        is_retake: claimed.row.previous_owner.is_some(),
    }
}

/// Renew a claim.
///
/// A 409 is the signal to stop rather than to try harder: the lease went, the
/// attempt has been taken over, and whatever this worker is in the middle of
/// will not be accepted.
#[utoipa::path(
    post,
    path = "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/heartbeat",
    params(
        ("execution_id" = String, Path, description = "The run this attempt belongs to"),
        ("step_id" = String, Path, description = "The step of that run's pinned plan"),
        ("attempt" = u32, Path, description = "Which attempt"),
    ),
    request_body = WorkerBody,
    responses(
        (status = 204, description = "still held"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "worker",
)]
async fn heartbeat(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id, attempt)): Path<(String, String, u32)>,
    Json(body): Json<WorkerBody>,
) -> ApiResult<StatusCode> {
    let key = key_of(&execution_id, &step_id, attempt);
    // `held` re-reads the row, which is what checks the queue scope as well as
    // the lease. The renewal itself is the store's, and it refuses a caller
    // that is not the holder — so a lease that expired between the two is a
    // 409 rather than a renewal of somebody else's claim.
    let (reactor, _) = held(&state, &caller, &body.worker, &key).await?;
    let renewed = reactor
        .handler()
        .store()
        .heartbeat(&key, &body.worker, time::OffsetDateTime::now_utc())
        .await
        .map_err(aiwatcher_execution::HandleError::Store)
        .map_err(ApiError::Execution)?;

    if renewed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::LeaseLost(key.idempotency_key()))
    }
}

/// Report what an attempt produced, and let the decider decide what follows.
///
/// The server re-checks the lease, records the outputs and the cache entry,
/// and reports `step.completed` or `step.failed`. None of that is the worker's
/// to do: a claimant is the one party that cannot check its own lease, and a
/// cache entry must not be written by whoever benefits from it.
/// Redelivery of the same durable outcome is acknowledged from history without
/// restoring a lease. Different outcomes conflict; diagnostics and cache hints
/// do not alter an already committed result.
#[utoipa::path(
    post,
    path = "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/result",
    params(
        ("execution_id" = String, Path, description = "The run this attempt belongs to"),
        ("step_id" = String, Path, description = "The step of that run's pinned plan"),
        ("attempt" = u32, Path, description = "Which attempt"),
    ),
    request_body = WorkerReport,
    responses(
        (status = 200, body = Settled),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "worker",
)]
async fn report(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id, attempt)): Path<(String, String, u32)>,
    Json(body): Json<WorkerReport>,
) -> ApiResult<Json<Settled>> {
    let key = key_of(&execution_id, &step_id, attempt);
    if let Some(receipt) = recorded_result(&state, &caller, &key, &body.report).await? {
        return Ok(Json(receipt));
    }
    let (reactor, claimed) = match held(&state, &caller, &body.worker, &key).await {
        Ok(held) => held,
        Err(error @ ApiError::LeaseLost(_)) => {
            // A concurrent delivery may have retired the lease after the
            // history read above. Its committed outcome still answers us.
            return recorded_result(&state, &caller, &key, &body.report)
                .await?
                .map(Json)
                .ok_or(error);
        }
        Err(error) => return Err(error),
    };

    let outcome = match body.report.clone() {
        WorkReport::Completed {
            outputs,
            result,
            diagnostics,
            cacheable,
        } => {
            let names: std::collections::BTreeSet<_> =
                outputs.iter().map(|output| output.name.as_str()).collect();
            if names.len() != outputs.len() {
                return Err(ApiError::BadRequest(
                    "output names must be unique".to_owned(),
                ));
            }
            for declared in &claimed.command.step.outputs {
                if !outputs
                    .iter()
                    .any(|output| output.name == declared.name && output.kind == declared.kind)
                {
                    return Err(ApiError::BadRequest(format!(
                        "missing declared output {} with kind {:?}",
                        declared.name, declared.kind
                    )));
                }
            }
            // Every reference a worker reports must be one this instance
            // stored. Checked rather than trusted, because a completed step
            // pointing at an object that 404s is the one failure nothing
            // downstream would catch — and the check is cheap next to the work
            // that produced them.
            for output in &outputs {
                let holds = artifacts(&state)?
                    .holds(output)
                    .await
                    .map_err(ApiError::WorkerArtifacts)?;
                if !holds {
                    return Err(ApiError::BadRequest(format!(
                        "this instance stores no artifact at {}; \
                         write outputs through this attempt's outputs route",
                        output.uri
                    )));
                }
            }
            Ok(ActivityResult {
                outputs,
                result,
                diagnostics,
                awaiting: None,
                cacheable,
            })
        }
        WorkReport::Failed {
            class,
            message,
            diagnostics,
        } => {
            if let Some(diagnostics) = diagnostics {
                tracing::debug!(attempt = %key, diagnostics, "a worker's attempt failed");
            }
            Err(StepError::new(class, message))
        }
    };

    let performed = reactor
        .settle(claimed, outcome, time::OffsetDateTime::now_utc())
        .await
        .map_err(ApiError::Execution)?;

    // A concurrent cancellation or a competing report can make the decider
    // decline this report. Only committed history is an acknowledgement.
    tracing::debug!(
        ?performed,
        "worker report processed; checking durable outcome"
    );
    recorded_result(&state, &caller, &key, &body.report)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::LeaseLost(key.idempotency_key()))
}

/// A matching outcome is safe to acknowledge after its lease row was retired.
/// Queue access still comes from the pinned plan, never from the request. This
/// reads history only: it cannot grant an expired worker another write lease.
async fn recorded_result(
    state: &AppState,
    caller: &Caller,
    key: &AttemptKey,
    report: &WorkReport,
) -> ApiResult<Option<Settled>> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    let history = handler(state)?
        .store()
        .load(&key.execution_id)
        .await
        .map_err(aiwatcher_execution::HandleError::from)
        .map_err(ApiError::Execution)?;
    let replayed = aiwatcher_execution::replay(history.events());
    let Some(run) = replayed.active() else {
        return Ok(None);
    };
    let Some(step) = run.plan.step(&key.step_id) else {
        return Ok(None);
    };
    let RuntimeBinding::PythonTask(spec) = &step.runtime else {
        return Ok(None);
    };
    if !caller.identity().may_claim(&spec.queue) {
        return Err(ApiError::Forbidden {
            needed: aiwatcher_auth::Role::Editor,
            held: caller.identity().role(),
        });
    }
    let expected = match report {
        WorkReport::Completed {
            outputs, result, ..
        } => WorkflowEvent::StepCompleted {
            step_id: key.step_id.clone(),
            attempt: key.attempt,
            outputs: outputs.clone(),
            result: result.clone(),
        },
        WorkReport::Failed { class, message, .. } => WorkflowEvent::StepFailed {
            step_id: key.step_id.clone(),
            attempt: key.attempt,
            error: StepError::new(*class, message.clone()),
        },
    };
    for recorded in &history.messages {
        if recorded.direction != Direction::Output {
            continue;
        }
        let Some(event) = recorded.message.event() else {
            continue;
        };
        let matches_attempt = match event {
            WorkflowEvent::StepCompleted {
                step_id, attempt, ..
            }
            | WorkflowEvent::StepFailed {
                step_id, attempt, ..
            } => step_id == &key.step_id && attempt == &key.attempt,
            _ => false,
        };
        if matches_attempt {
            if event != &expected {
                return Err(ApiError::WorkerReportConflict(key.idempotency_key()));
            }
            return Ok(Some(Settled {
                step_id: key.step_id.clone(),
                attempt: key.attempt,
                succeeded: matches!(event, WorkflowEvent::StepCompleted { .. }),
            }));
        }
    }
    Ok(None)
}

/// A report, with the worker that is making it.
///
/// Two structs rather than a flattened one: serde's `flatten` and
/// `deny_unknown_fields` do not compose, and the report is a tagged enum whose
/// variants are the whole point of the body being checked.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkerReport {
    pub worker: String,
    #[serde(flatten)]
    pub report: WorkReport,
}

/// Read one of this attempt's inputs.
///
/// Proxied rather than presigned. A presigned URL is a bearer credential for a
/// bucket that also holds prompts, datasets, annotations, conversations and
/// training, scoped by a prefix and a clock and handed to a process that may be
/// behind somebody's NAT; this route can check the one thing that matters,
/// which is that the caller holds the lease on the attempt whose input it is
/// asking for.
#[utoipa::path(
    get,
    path = "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/inputs/{name}",
    params(
        ("execution_id" = String, Path, description = "The run this attempt belongs to"),
        ("step_id" = String, Path, description = "The step of that run's pinned plan"),
        ("attempt" = u32, Path, description = "Which attempt"),
        ("name" = String, Path, description = "The input's name, as the assignment listed it"),
        ("worker" = String, Query, description = "The name the claim was made under"),
    ),
    responses(
        (status = 200, body = RowsBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "worker",
)]
async fn read_input(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id, attempt, name)): Path<(String, String, u32, String)>,
    axum::extract::Query(who): axum::extract::Query<WorkerBody>,
) -> ApiResult<Json<RowsBody>> {
    let key = key_of(&execution_id, &step_id, attempt);
    let (_, claimed) = held(&state, &caller, &who.worker, &key).await?;

    let artifact = claimed
        .command
        .inputs
        .iter()
        .find(|input| input.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("input {name} of attempt {key}")))?;

    let rows = artifacts(&state)?
        .read_rows(artifact)
        .await
        .map_err(ApiError::WorkerArtifacts)?;
    Ok(Json(RowsBody { rows }))
}

/// Store rows this attempt produced, and get the pointer back.
///
/// The digest is of the bytes this instance wrote and never of anything the
/// caller claimed — the prompt registry's rule, and what makes the reference
/// safe to accept back on the result route.
#[utoipa::path(
    post,
    path = "/api/v1/worker/claims/{execution_id}/{step_id}/{attempt}/outputs/{name}",
    params(
        ("execution_id" = String, Path, description = "The run this attempt belongs to"),
        ("step_id" = String, Path, description = "The step of that run's pinned plan"),
        ("attempt" = u32, Path, description = "Which attempt"),
        ("name" = String, Path, description = "What the reader will call it"),
        ("worker" = String, Query, description = "The name the claim was made under"),
    ),
    request_body = RowsBody,
    responses(
        (status = 200, body = ArtifactRef),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "worker",
)]
async fn write_output(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id, attempt, name)): Path<(String, String, u32, String)>,
    axum::extract::Query(who): axum::extract::Query<WorkerBody>,
    Json(body): Json<RowsBody>,
) -> ApiResult<Json<ArtifactRef>> {
    let key = key_of(&execution_id, &step_id, attempt);
    held(&state, &caller, &who.worker, &key).await?;

    let stored = artifacts(&state)?
        .put_rows(&name, body.rows)
        .await
        .map_err(ApiError::WorkerArtifacts)?;
    Ok(Json(stored))
}
