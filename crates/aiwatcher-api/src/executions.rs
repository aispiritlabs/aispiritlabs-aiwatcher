//! Managed execution: asking this system to run something, and reading how far
//! it got.
//!
//! Two routes, and the asymmetry between them is the design. `POST` accepts a
//! **command** — it compiles a definition, writes one transaction and answers
//! 202; nothing has run. `GET` reads the store's inline projection for one run:
//! its own page, and the state the next command is accepted against.
//!
//! ## Why there is no list here
//!
//! ADR_0026. The store's projection exists to accept the next command and to
//! draw one run; a list of executions is what `/api/v1/workflow-executions`
//! already serves, folded from `workflow.declared` and `step.*` on the event
//! log. Two lists would be two pictures of one run, and the first time a
//! producer and the engine described the same node they would disagree — with
//! nothing able to say which was right.
//!
//! ## Why this is not `/api/v1/engine/launches`
//!
//! That route asks somebody else's orchestrator to start something it already
//! holds (ADR_0016). This one runs a plan **here**, compiled from a definition
//! this system stores, against reactors this deployment configured. They differ
//! in who owns the retries, which is the whole of ADR_0025 — and a picker that
//! merged them could not say which of the two a row was.
//!
//! ## Why an editor, and not an admin
//!
//! A launch and a rerun need `admin` because they are aiwatcher asking *another
//! system* to do work inside the cluster on the caller's behalf. This is
//! aiwatcher doing its own work, against services this deployment named, and it
//! produces the same artifact as `POST /api/v1/datasets` — which is an editor's
//! to publish. Capping the ingest token at editor still holds: a leaked agent
//! environment can build a dataset it could already have published, and cannot
//! start anything in anybody's cluster.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::OpenApi;

use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_execution::compile::CompileOptions;
use aiwatcher_execution::hosted::{DeciderLease, LeaseOutcome};
use aiwatcher_execution::message::RunProjection;
use aiwatcher_execution::plan::{DefinitionKind, ResolvedWindow};
use aiwatcher_execution::{
    ExecutionHandler, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, MessageMetadata,
    Now, RunAction, WorkflowCommand, WorkflowMessage, WorkflowStore, allowed_run_actions,
    compile_curation, derive_uuid,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The header a caller repeats to reach the same execution twice.
///
/// Section 20 asks every mutating command endpoint for one. Here it does its
/// work by *deriving the execution id* rather than by a table of keys: the same
/// key produces the same id, the same id produces the same stream, and the
/// store's own inbox answers the second request with what the first decided.
/// A table would be a fourth place a decision is recorded.
const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    start_execution,
    get_execution,
    execution_history,
    append_stream,
    take_decider_lease,
    read_decider_lease,
    release_decider_lease,
    cancel_execution,
    pause_execution,
    resume_execution,
    retry_step,
    provide_input
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/executions", post(start_execution))
        .route("/api/v1/executions/{execution_id}", get(get_execution))
        .route(
            "/api/v1/executions/{execution_id}/history",
            get(execution_history),
        )
        // The hosted decider's append side (section 40.3). There is no `GET`
        // beside it on purpose: `…/history` already pages this stream, and a
        // second read of one run is what the guardrail against a second live
        // view is about. The `POST` is here rather than on `…/history` because
        // it is the route the worker's `EventStore` is written against, and
        // because a read that pages and a compare-and-append are not two verbs
        // on one idea — one answers "what happened", the other decides whether
        // anything did.
        .route(
            "/api/v1/executions/{execution_id}/stream",
            post(append_stream),
        )
        // One decider at a time. `GET` is the half that matters most: a lease
        // is not something a claimant may decide about itself, so it asks and
        // this answers from the row.
        .route(
            "/api/v1/executions/{execution_id}/decider-lease",
            get(read_decider_lease).post(take_decider_lease),
        )
        .route(
            "/api/v1/executions/{execution_id}/decider-lease/release",
            post(release_decider_lease),
        )
        // Section 20's command routes. Grouped under `commands/` for the run
        // and under the step for the two that name one, which is the shape the
        // plan asked for and reads correctly: pausing is done to a run, and
        // retrying is done to a step of one.
        .route(
            "/api/v1/executions/{execution_id}/commands/cancel",
            post(cancel_execution),
        )
        .route(
            "/api/v1/executions/{execution_id}/commands/pause",
            post(pause_execution),
        )
        .route(
            "/api/v1/executions/{execution_id}/commands/resume",
            post(resume_execution),
        )
        .route(
            "/api/v1/executions/{execution_id}/steps/{step_id}/commands/retry",
            post(retry_step),
        )
        .route(
            "/api/v1/executions/{execution_id}/steps/{step_id}/input",
            post(provide_input),
        )
}

/// What kind of definition is being run.
///
/// Two arms, which is what the enum was for: both compile to the same
/// `ExecutionPlan` from different editors, with different provenance, and the
/// names live in different registries under different prefixes (section 4). A
/// `kind` nobody had to send would have to be guessed from the name — and two
/// definitions may share one, which is exactly what `WorkflowSpec` being saved
/// beside `CurationPipeline` allows.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// ADR_0024's source/transform/notebook/view chain.
    CurationPipeline,
    /// A registered Python workflow.
    Workflow,
}

/// Which definition, at which revision.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub kind: TargetKind,
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
/// choice that actually exists. `engine:` is [`ExecutionOwner::Engine`] and is
/// not something a caller picks here — it is what ADR_0016's launch produces.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    /// The Rust decider schedules the plan's steps and owns their retries.
    #[default]
    Local,
    /// A worker runs `decide` and appends to the history this system keeps
    /// (ADR_0025, section 40.3). What an agent graph needs, because its next
    /// node depends on what the last one said.
    Worker,
}

impl Decider {
    const fn parts(self) -> (ExecutionOwner, ExecutionMode) {
        match self {
            Self::Local => (ExecutionOwner::Local, ExecutionMode::Compiled),
            Self::Worker => (ExecutionOwner::Worker, ExecutionMode::Hosted),
        }
    }
}

/// What a caller may ask this system to run.
///
/// Note what is not here, which is the same absence as `LaunchBody`'s and
/// `RerunBody`'s: no endpoint, no script, no host. A plan names a binding and
/// its parameters, and every executor's address is configuration.
/// `deny_unknown_fields` so an attempt to supply one — or a `backend`, a `mode`
/// or a `publish` flag from section 20 that this phase does not implement — is
/// a 400 naming it rather than a field silently ignored that reads as accepted.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartExecutionBody {
    pub target: ExecutionTarget,
    /// Values bound when the execution was requested, available to every step.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub parameters: BTreeMap<String, Value>,
    /// How wide the source's time window is, in seconds.
    ///
    /// Resolved to exact bounds **here**, once, rather than at dispatch: the
    /// plan then records what it was asked for, a retry reads the same rows,
    /// and a cache key can exist at all.
    #[serde(default)]
    pub window_seconds: Option<u64>,
    /// Where that window ends, in seconds since the epoch. `None` is now.
    ///
    /// Two runs that pin the same span compile to one `plan_id` and one cache
    /// key, which is what makes a scheduled build of a fixed period cheap the
    /// second time. Left out, every run pins a fresh span — which is right for
    /// "curate the last hour" and is why a hit is something a caller asks for
    /// rather than something they get by accident.
    #[serde(default)]
    pub as_of: Option<i64>,
    /// Who decides what runs next. Left out, this system does — which is what
    /// every curation pipeline and every scheduled run wants, and what this
    /// route did before the field existed.
    #[serde(default)]
    pub decided_by: Decider,
}

/// An accepted command, and the run it started.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionAccepted {
    /// The store's inline projection after the decision that accepted this.
    pub execution: RunProjection,
    /// True when this request started the run, false when an
    /// `Idempotency-Key` landed on one that was already going. Both are 202:
    /// the caller asked for a run with that key and there is one.
    pub created: bool,
}

/// A run, with what may be done to it.
///
/// The actions are computed here rather than left to the caller, for
/// `ContextAction::allowed`'s reason one level up: a panel that decided for
/// itself which of cancel, pause and resume apply would be a second copy of
/// `decide`'s preconditions, in another language, drifting from the first.
///
/// **Compatibility.** `GET /api/v1/executions/{execution_id}` and every
/// command route answered with a bare `RunProjection` before this type
/// existed; they now answer with this. The projection is unchanged and moved
/// under `execution`, so a field read as `state` is read as
/// `execution.state`. Clients generated from `contracts/openapi.json` follow
/// it by regenerating; a hand-written one does not, and this is the note that
/// says so.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunView {
    pub execution: RunProjection,
    /// Which run-level commands would be accepted right now. Empty for a run
    /// that has finished.
    pub allowed: Vec<RunAction>,
}

fn handler(state: &AppState) -> ApiResult<&Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>> {
    state
        .executions
        .as_ref()
        .ok_or(ApiError::ExecutionsDisabled)
}

fn definitions(state: &AppState) -> ApiResult<&Arc<DatasetRegistry>> {
    state
        .datasets
        .as_ref()
        .ok_or(ApiError::DatasetRegistryDisabled)
}

/// Compile a definition and start running it.
///
/// `202`, not `200`: what is durable when this answers is the command and the
/// first decision, and nothing has executed. The browser may close immediately
/// afterwards — that is the whole point of the route, and of ADR_0025.
#[utoipa::path(
    post,
    path = "/api/v1/executions",
    request_body = StartExecutionBody,
    responses(
        (status = 202, body = ExecutionAccepted),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody, description = "No definition by that name, or no such revision"),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "Every reason the definition does not run here, in `details`"),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn start_execution(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<StartExecutionBody>,
) -> ApiResult<(StatusCode, Json<ExecutionAccepted>)> {
    let requester = caller
        .require(aiwatcher_auth::Role::Editor)?
        .log_subject()
        .to_owned();
    // Before the compile, and the order matters: an instance with no workflow
    // store cannot run *anything*, and saying so is a better answer than
    // reporting whichever other thing is also missing. `start` checks it again
    // because it has a second caller.
    handler(&state)?;

    let plan = compile(&state, &body).await?;
    let execution_id = ExecutionId::new(execution_id_for(&headers, &plan));
    let handled = start(
        &state,
        &execution_id,
        plan,
        body.parameters,
        &requester,
        body.decided_by,
    )
    .await?;

    tracing::info!(
        execution_id = %execution_id,
        plan_id = %handled.projection.plan_id,
        definition = %handled.projection.definition_name,
        steps = handled.projection.steps.len(),
        // Who asked. A managed execution is this system doing work on
        // somebody's behalf, and before SSO this line could only have said
        // "somebody".
        requested_by = %requester,
        duplicate = handled.duplicate,
        "accepted a managed execution"
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(ExecutionAccepted {
            execution: handled.projection,
            created: !handled.duplicate,
        }),
    ))
}

/// One run's own page: where it is, where each of its steps is, and what may be
/// done to it.
///
/// The store's inline projection rather than a fold of the log, and the two are
/// not interchangeable. This one is transactional with the decision that
/// produced it, which is what makes it safe to accept the next command from;
/// the fold is what draws the run beside every other execution, with `Pending`
/// nodes and a live stream, and it is the one a list comes from.
///
/// Answers [`RunView`] — the projection under `execution`, beside the actions
/// the run would accept. It answered the bare projection once; see that type
/// for what a hand-written client has to change.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    responses(
        (status = 200, body = RunView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn get_execution(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<RunView>> {
    let execution = handler(&state)?
        .store()
        .projection(&ExecutionId::new(execution_id.clone()))
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .ok_or_else(|| ApiError::NotFound(format!("execution {execution_id}")))?;
    Ok(Json(RunView {
        allowed: allowed_run_actions(execution.state.state_type),
        execution,
    }))
}

/// An ordered page of durable commands and decisions. Live facts use the existing workflow stream.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionHistory {
    pub messages: Vec<aiwatcher_execution::RecordedMessage>,
    pub next_after: Option<u64>,
    pub version: u64,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct HistoryQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[utoipa::path(get, path = "/api/v1/executions/{execution_id}/history",
    params(("execution_id" = String, Path), HistoryQuery),
    responses((status = 200, body = ExecutionHistory), (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn execution_history(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<ExecutionHistory>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    // One over the page, so "is there more" is answered without a second read
    // and without comparing against a version that may have moved since.
    let stream = handler(&state)?
        .store()
        .load_page(
            &ExecutionId::new(execution_id.clone()),
            query.after.unwrap_or(0),
            limit + 1,
        )
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    // The *stream's* version, not the page's contents: a page past the end of a
    // run that exists is empty and is not a 404, and reading `is_empty()` here
    // would have said there is no such execution.
    if stream.version == 0 {
        return Err(ApiError::NotFound(format!("execution {execution_id}")));
    }
    let mut messages = stream.messages;
    let has_more = messages.len() > limit;
    messages.truncate(limit);
    let next_after = if has_more {
        messages.last().map(|message| message.stream_version)
    } else {
        None
    };
    Ok(Json(ExecutionHistory {
        messages,
        next_after,
        version: stream.version,
    }))
}

/// What a worker is appending to a hosted execution's history.
///
/// `deny_unknown_fields` for the reason every body here has it: a worker that
/// sent `version` where this reads `expected_version` would otherwise have its
/// compare-and-append silently become "append at whatever it is at", which is
/// the one guarantee it came here for.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppendStreamBody {
    /// The version the worker read. Required: an append that did not name one
    /// is not a compare-and-append, and a decider that did not read the stream
    /// has nothing to decide from. The first one comes back on the start.
    pub expected_version: u64,
    /// Which decider is appending. Checked against the lease, so a worker that
    /// was taken over is told before it pays for the turn rather than after.
    pub holder: String,
    /// The worker's own messages, in the order it wrote them.
    pub messages: Vec<aiwatcher_execution::message::HostedMessage>,
}

/// Where the stream got to, and whether this call is what put it there.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StreamAppended {
    /// The version to send as the next `expected_version`.
    pub version: u64,
    /// `false` when this batch had already been appended — a redelivery, not a
    /// race. The worker carries on from `version` either way.
    pub created: bool,
}

/// Append a worker's messages to a hosted execution's history (section 40.3).
///
/// The decider is the worker; this is the shared history `agentic.workflow`
/// cannot give itself when every agent worker holds its own SQLite. What this
/// route contributes is the three things a private store has no way to offer: a
/// compare-and-append against a version, an inbox keyed by the
/// `Idempotency-Key`, and a 409 that says where the stream actually got to.
///
/// It reads none of the messages. A `409` is not a failure to retry blindly:
/// the worker reloads and decides on what it now sees, and its own cached
/// decision across OCC retries is what stops that reload calling the model
/// again to discover it lost.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/stream",
    params(
        ("execution_id" = String, Path, description = "The id the run was started with"),
        ("Idempotency-Key" = String, Header, description = "This batch's key. The durable inbox key: repeating it returns the first outcome rather than appending twice"),
    ),
    request_body = AppendStreamBody,
    responses(
        (status = 200, body = StreamAppended),
        (status = 400, body = crate::error::ErrorBody, description = "An empty batch, one past the limit, or no Idempotency-Key"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "Another decider appended first, or this run is not a hosted one"),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn append_stream(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AppendStreamBody>,
) -> ApiResult<Json<StreamAppended>> {
    // An editor, which is what an ingest token is capped at. A hosted decider
    // is a worker, and a worker is exactly the caller that cannot complete an
    // interactive sign-in — the reason that cap exists rather than an exception
    // to it.
    caller.require(aiwatcher_auth::Role::Editor)?;

    // Required rather than defaulted: without it there is no inbox key, and a
    // retried request whose response was lost would append the batch twice.
    // Deriving one from the body would make two different batches with the same
    // content collide, which is the same bug wearing a hash.
    let key = headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "appending to a hosted execution needs an `{IDEMPOTENCY_KEY}` header: \
                 it is the inbox key that stops a retried request appending twice"
            ))
        })?
        .to_owned();

    let execution = ExecutionId::new(execution_id);
    let append = aiwatcher_execution::hosted::HostedAppend {
        expected_version: body.expected_version,
        idempotency_key: key,
        holder: body.holder,
        messages: body.messages,
    };
    let messages = append.messages.len();
    let handled = handler(&state)?
        .append_hosted(&execution, append, time::OffsetDateTime::now_utc())
        .await?;

    tracing::info!(
        execution_id = %execution,
        messages,
        version = handled.projection.last_message_version,
        duplicate = handled.duplicate,
        "appended to a hosted execution"
    );

    Ok(Json(StreamAppended {
        version: handled.projection.last_message_version,
        created: !handled.duplicate,
    }))
}

/// Which decider is asking.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeciderLeaseBody {
    /// A name this decider will keep across its own restarts if it wants to
    /// resume rather than be taken over. Not a credential: the route's role
    /// check is what decides who may ask at all.
    pub holder: String,
}

/// Take, or renew, the right to decide one hosted run (section 40.3).
///
/// `agentic.workflow`'s `ProcessorLock`, in the store that holds the history.
/// Renewing is this same call under the same name, so a heartbeat and a first
/// claim cannot come to disagree.
///
/// **200 either way.** Being told who holds it is an answer to the question
/// rather than a failure of it, and the alternative — a 409 whose structured
/// `expires_at` has to be smuggled through an error body — is worse for the one
/// caller that has to branch on it. What *is* a 409 is appending while somebody
/// else decides.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/decider-lease",
    params(("execution_id" = String, Path, description = "The id the run was started with")),
    request_body = DeciderLeaseBody,
    responses(
        (status = 200, body = LeaseOutcome, description = "Taken by this caller, or held by somebody else until `expires_at`"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "Not a hosted run, or one that has finished"),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn take_decider_lease(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
    Json(body): Json<DeciderLeaseBody>,
) -> ApiResult<Json<LeaseOutcome>> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    let execution = ExecutionId::new(execution_id);
    let outcome = handler(&state)?
        .take_decider_lease(&execution, &body.holder, time::OffsetDateTime::now_utc())
        .await?;
    tracing::info!(
        execution_id = %execution,
        holder = %body.holder,
        taken = outcome.taken().is_some(),
        "asked for a decider lease"
    );
    Ok(Json(outcome))
}

/// Who is deciding this run, if anybody still is.
///
/// The read behind "a lease is not something the claimant can check about
/// itself": the worker asks, and this answers from the row. 404 once it has run
/// out, because expired and never taken are the same answer to a caller — it is
/// free.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/decider-lease",
    params(("execution_id" = String, Path)),
    responses(
        (status = 200, body = DeciderLease),
        (status = 404, body = crate::error::ErrorBody, description = "Nobody holds it"),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn read_decider_lease(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<DeciderLease>> {
    let execution = ExecutionId::new(execution_id.clone());
    handler(&state)?
        .decider_lease(&execution, time::OffsetDateTime::now_utc())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("a decider lease on {execution_id}")))
}

/// Give up the right to decide.
///
/// Worth calling rather than waiting the lease out: it is the difference
/// between a replacement starting now and starting in five minutes. `released`
/// is false when it was never this caller's to give — a worker that had already
/// been taken over must not release its replacement's lease.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/decider-lease/release",
    params(("execution_id" = String, Path)),
    request_body = DeciderLeaseBody,
    responses(
        (status = 200, body = LeaseReleased),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "Not a hosted run, or one that has finished"),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn release_decider_lease(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
    Json(body): Json<DeciderLeaseBody>,
) -> ApiResult<Json<LeaseReleased>> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    let released = handler(&state)?
        .release_decider_lease(
            &ExecutionId::new(execution_id),
            &body.holder,
            time::OffsetDateTime::now_utc(),
        )
        .await?;
    Ok(Json(LeaseReleased { released }))
}

/// Whether the release found a lease this caller was holding.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LeaseReleased {
    pub released: bool,
}

/// Start one execution: the only path there is.
///
/// Taken out of the route rather than left in it because it has a second
/// caller — the scheduler in the work role — and a scheduler with its own copy
/// would be a second way to start a run, free to disagree with this one about
/// the owner, the mode or the derived message id. The route decides *who is
/// asking* and *which id*; this decides what starting means.
///
/// # Errors
///
/// Whatever the handler could not do.
pub async fn start(
    state: &AppState,
    execution_id: &ExecutionId,
    plan: ExecutionPlan,
    parameters: BTreeMap<String, Value>,
    requested_by: &str,
    decided_by: Decider,
) -> ApiResult<aiwatcher_execution::Handled> {
    // Derived from the execution, so a redelivered request — a retried POST, a
    // proxy that repeated it, a second worker on the same slot — lands on the
    // inbox rather than beside it.
    let message_id = aiwatcher_core::MessageId::new(derive_uuid(&format!(
        "aiwatcher/execution/start/{execution_id}"
    )));

    let (owner, mode) = decided_by.parts();
    let now = time::OffsetDateTime::now_utc();
    let handled = handler(state)?
        .handle(
            execution_id,
            WorkflowMessage::Command(WorkflowCommand::StartExecution {
                execution_id: execution_id.clone(),
                plan: Box::new(plan),
                // Recorded rather than derived, so a run always says who was
                // responsible for it. The two arms are the two that exist:
                // `local` schedules the plan here, `worker` keeps the history
                // for a decider that runs somewhere else. `engine:` comes from
                // ADR_0016's launch and never from this route.
                owner,
                mode,
                requested_by: requested_by.to_owned(),
                input: parameters,
            }),
            MessageMetadata::caused_by(execution_id, &message_id, message_id.clone(), now),
            Now::at(now),
        )
        .await?;

    // The outbox has rows and the claim table has a dispatched attempt; both
    // are drained by loops that would otherwise wait a poll interval. Nothing
    // is lost if this process runs neither — the store is durable and whichever
    // one does run them picks the work up.
    state.notify_execution_worker();
    Ok(handled)
}

/// Read the definition at the revision it names, and compile it.
///
/// The compiler is `aiwatcher-execution`'s and the shape rules are
/// `aiwatcher-datasets`'; this function only decides *which* definition. A
/// refusal carries every problem at once, which is what the canvas renders.
async fn compile(state: &AppState, body: &StartExecutionBody) -> ApiResult<ExecutionPlan> {
    match body.target.kind {
        TargetKind::CurationPipeline => {
            compile_curation_named(
                state,
                &body.target.name,
                body.target.revision.as_deref(),
                body.window_seconds
                    .map(|seconds| resolve_window(seconds, body.as_of)),
            )
            .await
        }
        TargetKind::Workflow => {
            if body.window_seconds.is_some() || body.as_of.is_some() {
                return Err(ApiError::BadRequest(
                    "time windows apply only to curation pipelines".to_owned(),
                ));
            }
            compile_workflow_named(state, &body.target.name, body.target.revision.as_deref()).await
        }
    }
}

/// Compile an immutable registered workflow for API or scheduler callers.
/// # Errors
/// A missing definition, storage error, or invalid saved definition.
pub async fn compile_workflow_named(
    state: &AppState,
    name: &str,
    revision: Option<&str>,
) -> ApiResult<ExecutionPlan> {
    let saved = crate::definitions::registry(state)?
        .get(name, revision)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .ok_or_else(|| ApiError::NotFound(format!("workflow {name}")))?;
    saved
        .definition
        .compile()
        .map_err(|error| ApiError::PlanRefused {
            summary: format!("{name} does not compile"),
            problems: error.problems().to_vec(),
        })
}

/// Compile whichever kind of definition a name refers to, at its head.
///
/// The two callers that have no request body — the schedule route, which
/// refuses a definition that does not compile rather than letting it fail at
/// nine tomorrow, and the tick that starts it — reach a compiler through this
/// and never pick one themselves. A `match` in each would be two answers to
/// "what does this schedule run", free to disagree the day a third kind
/// arrives: one of them would start a run and the other would refuse to save
/// the schedule for it.
///
/// The head, never a pinned revision. A schedule says *what* to run, and the
/// run records the revision it pinned.
///
/// # Errors
///
/// A 404 when nothing is saved under that name, or a 422 carrying every reason
/// it does not compile.
pub async fn compile_head(
    state: &AppState,
    kind: DefinitionKind,
    name: &str,
) -> ApiResult<ExecutionPlan> {
    match kind {
        DefinitionKind::CurationPipeline => compile_curation_named(state, name, None, None).await,
        DefinitionKind::Workflow => compile_workflow_named(state, name, None).await,
    }
}

/// The same compile, by name, for a caller that has no request body.
///
/// The scheduler is that caller. Sharing it rather than repeating it is what
/// keeps "which revision does a run pin" one answer: the head, read and pinned
/// at the moment the run starts.
///
/// # Errors
///
/// A 404 when the definition is not there, or a 422 carrying every reason it
/// does not compile.
pub async fn compile_curation_named(
    state: &AppState,
    name: &str,
    revision: Option<&str>,
    window: Option<ResolvedWindow>,
) -> ApiResult<ExecutionPlan> {
    let pipeline = definitions(state)?
        .pipeline(name, revision)
        .await?
        .ok_or_else(|| match revision {
            Some(revision) => ApiError::NotFound(format!("pipeline {name} at {revision}")),
            None => ApiError::NotFound(format!("pipeline {name}")),
        })?;

    compile_curation(&pipeline, CompileOptions { window }).map_err(|error| ApiError::PlanRefused {
        summary: format!("{name} does not compile to something that can be run"),
        problems: error.problems().to_vec(),
    })
}

/// A relative window, pinned to the bounds it meant when it was asked for.
///
/// The clock is read here and nowhere below: `decide` may not read one, and a
/// window resolved at dispatch would move under a retry.
fn resolve_window(seconds: u64, as_of: Option<i64>) -> ResolvedWindow {
    let to = as_of.unwrap_or_else(|| time::OffsetDateTime::now_utc().unix_timestamp());
    ResolvedWindow {
        from: to.saturating_sub(i64::try_from(seconds).unwrap_or(i64::MAX)),
        to,
    }
}

/// The id this run will have.
///
/// With an `Idempotency-Key`, derived from the key *and the plan*: repeating
/// the request reaches the same stream, whose inbox answers with what the first
/// one decided. Without one, a fresh v7 — two clicks are two runs, which is
/// what somebody clicking twice on purpose means.
///
/// The plan is in the derivation so that one key cannot address two different
/// plans: a caller who edited the pipeline and repeated their key would
/// otherwise get the *old* run back and a 202 saying so.
fn execution_id_for(headers: &HeaderMap, plan: &ExecutionPlan) -> String {
    match headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        Some(key) => derive_uuid(&format!(
            "aiwatcher/execution/request/{}/{key}",
            plan.plan_id
        )),
        // Hyphen-free, like a launch's `workflow_run_id`: this id becomes a
        // correlation id, a partition key and a file name.
        None => uuid::Uuid::now_v7().simple().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_execution::plan::{DefinitionKind, DefinitionRevision};

    use super::*;

    fn plan(revision: &str) -> ExecutionPlan {
        ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision(revision.to_owned()),
            vec![aiwatcher_execution::plan::PlanStep {
                id: "read".to_owned(),
                runtime: aiwatcher_execution::plan::RuntimeBinding::FlowPhp(
                    aiwatcher_execution::plan::FlowStepSpec {
                        script: "data_frame()->read(default)".to_owned(),
                        source: aiwatcher_execution::plan::FlowSourceRef::default(),
                        blocks: Vec::new(),
                    },
                ),
                inputs: Vec::new(),
                outputs: Vec::new(),
                retry: aiwatcher_execution::plan::RetryPolicy::default(),
                timeout_seconds: 300,
                cache: aiwatcher_execution::plan::CachePolicy::Never,
            }],
            Vec::new(),
        )
    }

    fn with_key(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_KEY, key.parse().expect("a header value"));
        headers
    }

    #[test]
    fn one_key_repeated_reaches_one_execution() {
        // Which is the whole mechanism: the same id is the same stream, and
        // the store's own inbox answers the second request with what the first
        // decided. A table of keys would be a fourth place a decision lives.
        let plan = plan("ab");
        assert_eq!(
            execution_id_for(&with_key("nightly-2026-09-05"), &plan),
            execution_id_for(&with_key("nightly-2026-09-05"), &plan)
        );
    }

    #[test]
    fn one_key_against_an_edited_pipeline_is_a_different_execution() {
        // Otherwise somebody who fixed their pipeline and repeated their key
        // would get the *old* run back, with a 202 saying it was theirs.
        assert_ne!(
            execution_id_for(&with_key("nightly"), &plan("ab")),
            execution_id_for(&with_key("nightly"), &plan("cd"))
        );
    }

    #[test]
    fn two_clicks_with_no_key_are_two_runs() {
        let plan = plan("ab");
        assert_ne!(
            execution_id_for(&HeaderMap::new(), &plan),
            execution_id_for(&HeaderMap::new(), &plan)
        );
        // And an id that becomes a correlation id, a partition key and a file
        // name carries none of the characters any of those three dislike.
        let id = execution_id_for(&HeaderMap::new(), &plan);
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric()),
            "{id} has to survive being a file name"
        );
    }

    #[test]
    fn a_window_is_pinned_to_the_bounds_it_meant_when_it_was_asked_for() {
        let window = resolve_window(900, None);
        assert_eq!(window.to - window.from, 900);
    }

    #[test]
    fn two_requests_pinning_one_span_ask_the_same_question() {
        // Which is what makes a scheduled build of a fixed period cheap the
        // second time: the same span compiles to one `plan_id` and one cache
        // key. Without `as_of` every run pins a fresh span, which is right for
        // "curate the last hour" and is why a hit is asked for rather than had
        // by accident.
        let nightly = resolve_window(3600, Some(1_700_003_600));
        assert_eq!(nightly, resolve_window(3600, Some(1_700_003_600)));
        assert_eq!(nightly.from, 1_700_000_000);
        assert_eq!(nightly.to, 1_700_003_600);
    }
}

/// A reason, for the one command that carries one.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelBody {
    /// Recorded on the decision and read by whoever finds the run stopped.
    /// Empty is allowed; a wrong reason would be worse than none.
    #[serde(default)]
    pub reason: String,
}

/// The answer a `HumanInput` step asked for.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvideInputBody {
    /// Which attempt asked. Named by the caller rather than read from the
    /// step's latest: an answer typed against a question that has since been
    /// retried is an answer to a question nobody is asking any more, and
    /// silently applying it to the new attempt would record a human decision
    /// that no human made.
    pub attempt: u32,
    /// Whatever the question asked for, as JSON.
    ///
    /// Any value, and deliberately not an object: a step that offered
    /// `choices` is answered with one of them, which `decide` reads as a JSON
    /// **string**. Declaring this an object made the contract unable to express
    /// the commonest valid request, so the generated client could not send one.
    #[schema(value_type = Value)]
    pub response: Value,
}

/// What a command's message id names, beyond the execution and its version.
///
/// Section 43.10 one layer up: a message id derived from less than what it
/// identifies collides with a different intention, and the second is swallowed
/// as a redelivery of the first. A retry of `read` and a retry of `publish` are
/// two commands.
fn intent(command: &WorkflowCommand) -> String {
    match command {
        WorkflowCommand::RetryStep { step_id } => format!("retry_step/{step_id}"),
        WorkflowCommand::ProvideInput {
            step_id, attempt, ..
        } => format!("provide_input/{step_id}/{attempt}"),
        other => other.name().to_owned(),
    }
}

/// One intention, applied to a run that already exists.
///
/// Every command route below is this function with a different
/// [`WorkflowCommand`]. What they share is what is worth writing once: the
/// role, the difference between "no such run" and "that run will not accept
/// this", the message id, and the nudge to the loops that would otherwise wait
/// out a poll interval.
///
/// The command is built from the caller's identity rather than passed in, so a
/// route that records *who* answered cannot forget to ask.
async fn apply<F>(
    state: &AppState,
    execution_id: &str,
    caller: &Caller,
    build: F,
) -> ApiResult<Json<RunView>>
where
    F: FnOnce(&str) -> WorkflowCommand,
{
    let who = caller
        .require(aiwatcher_auth::Role::Editor)?
        .log_subject()
        .to_owned();
    let handler = handler(state)?;
    let execution = ExecutionId::new(execution_id.to_owned());

    // Read before deciding, for two reasons that both matter. A run this
    // instance has never heard of is a 404 rather than a 409, because the fix
    // is a different id and not a different moment. And the version is the
    // message id's discriminator — see below.
    let current = handler
        .store()
        .projection(&execution)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .ok_or_else(|| ApiError::NotFound(format!("execution {execution_id}")))?;

    let command = build(&who);
    debug_assert!(
        !command.is_effect(),
        "a caller may only send an intention; ExecuteStep and RequestInput are the decider's"
    );

    // The run's own version is what tells a double-click from a second
    // intention. Two clicks on Pause read the same version, derive one id and
    // meet the inbox; a Pause after a Resume reads a version the Resume moved,
    // and is a command rather than a redelivery of the first Pause.
    let message_id = aiwatcher_core::MessageId::new(derive_uuid(&format!(
        "aiwatcher/execution/command/{execution}/{}/{}",
        intent(&command),
        current.last_message_version,
    )));

    let now = time::OffsetDateTime::now_utc();
    let handled = handler
        .handle(
            &execution,
            WorkflowMessage::Command(command),
            MessageMetadata::caused_by(&execution, &message_id, message_id.clone(), now),
            Now::at(now),
        )
        .await?;

    state.notify_execution_worker();

    tracing::info!(
        execution_id = %execution,
        state = ?handled.projection.state,
        requested_by = %who,
        duplicate = handled.duplicate,
        "applied a command to a managed execution"
    );

    // The actions come back with the run, so a caller that has just paused
    // knows Resume is the one that applies now without asking again.
    Ok(Json(RunView {
        allowed: allowed_run_actions(handled.projection.state.state_type),
        execution: handled.projection,
    }))
}

/// Stop a run, and everything it has not already dispatched.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/commands/cancel",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    request_body = CancelBody,
    responses(
        (status = 200, body = RunView),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The run is in no state to accept this"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn cancel_execution(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
    body: Option<Json<CancelBody>>,
) -> ApiResult<Json<RunView>> {
    let reason = body.map(|Json(body)| body.reason).unwrap_or_default();

    apply(&state, &execution_id, &caller, |_| {
        WorkflowCommand::CancelExecution { reason }
    })
    .await
}

/// Schedule nothing further until this run is resumed.
///
/// What is already dispatched keeps running: an attempt is somebody else's
/// process, and a pause that could reach into it would be a cancel wearing the
/// wrong name.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/commands/pause",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    responses(
        (status = 200, body = RunView),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The run is in no state to accept this"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn pause_execution(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<RunView>> {
    apply(&state, &execution_id, &caller, |_| {
        WorkflowCommand::PauseExecution
    })
    .await
}

/// Let a paused run schedule again.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/commands/resume",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    responses(
        (status = 200, body = RunView),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The run is in no state to accept this"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn resume_execution(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<RunView>> {
    apply(&state, &execution_id, &caller, |_| {
        WorkflowCommand::ResumeExecution
    })
    .await
}

/// Take one step again, from the inputs and the code its plan pinned.
///
/// A new attempt rather than a rewrite: the failed one stays in the stream with
/// its error, which is what a waterfall draws and what a person reads to find
/// out why this needed a second go.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/steps/{step_id}/commands/retry",
    params(
        ("execution_id" = String, Path, description = "The id a start returned"),
        ("step_id" = String, Path, description = "A step of that run's pinned plan"),
    ),
    responses(
        (status = 200, body = RunView),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The step is in no state to be retried"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn retry_step(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id)): Path<(String, String)>,
) -> ApiResult<Json<RunView>> {
    apply(&state, &execution_id, &caller, |_| {
        WorkflowCommand::RetryStep { step_id }
    })
    .await
}

/// Answer the question a `HumanInput` step is waiting on.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/steps/{step_id}/input",
    params(
        ("execution_id" = String, Path, description = "The id a start returned"),
        ("step_id" = String, Path, description = "The step that asked"),
    ),
    request_body = ProvideInputBody,
    responses(
        (status = 200, body = RunView),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "That step is not waiting for this answer"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn provide_input(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id)): Path<(String, String)>,
    Json(body): Json<ProvideInputBody>,
) -> ApiResult<Json<RunView>> {
    apply(&state, &execution_id, &caller, |who| {
        // Who answered comes from the session, never from the body. A field a
        // caller could set would make the one record of a human decision say
        // whatever the caller preferred it to say.
        WorkflowCommand::ProvideInput {
            step_id,
            attempt: body.attempt,
            answered_by: who.to_owned(),
            response: body.response,
        }
    })
    .await
}
