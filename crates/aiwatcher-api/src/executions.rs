//! Managed execution: asking this system to run something, and reading how far
//! it got.
//!
//! `POST` accepts a **command** — compile a definition, write one transaction,
//! answer 202; nothing has run. `GET` reads the store's inline projection for
//! one run: its own page, and the state the next command is accepted against.
//!
//! **No list here.** That is `/api/v1/workflow-executions`, folded from
//! `workflow.declared` and `step.*` on the event log. Two lists would be two
//! pictures of one run, free to disagree with nothing able to say which is
//! right.
//!
//! **Not `/api/v1/engine/launches`.** That asks somebody else's orchestrator to
//! start something it holds; this runs a plan *here*, compiled from a
//! definition this system stores, against reactors this deployment configured.
//! They differ in who owns the retries.
//!
//! **`editor`, not `admin`.** A launch and a rerun ask another system to work
//! inside the cluster; this is aiwatcher doing its own work and producing the
//! same artifact as `POST /api/v1/datasets`. So a leaked ingest token can build
//! a dataset it could already publish, and cannot start anything in anybody's
//! cluster.
//!
//! ADR_0025, ADR_0026.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::OpenApi;

use aiwatcher_execution::hosted::{DeciderLease, LeaseOutcome, Timer, TimerWrite};
use aiwatcher_execution::message::RunProjection;
use aiwatcher_execution::start::{ExecutionTarget, RunIdentity, StartRequest, StartRun, Window};
use aiwatcher_execution::{
    Decider, ExecutionHandler, ExecutionId, MessageMetadata, Now, RunAction, WorkflowCommand,
    WorkflowMessage, WorkflowStore, allowed_run_actions, derive_uuid,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The header a caller repeats to reach the same execution twice.
///
/// Every mutating command endpoint takes one. Here it does its
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
    execution_timers,
    seal_payload,
    read_payload,
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
        // The hosted decider's append side. There is no `GET`
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
            "/api/v1/executions/{execution_id}/timers",
            get(execution_timers),
        )
        .route(
            "/api/v1/executions/{execution_id}/payloads",
            post(seal_payload),
        )
        .route(
            "/api/v1/executions/{execution_id}/payloads/{digest}",
            get(read_payload),
        )
        .route(
            "/api/v1/executions/{execution_id}/decider-lease",
            get(read_decider_lease).post(take_decider_lease),
        )
        .route(
            "/api/v1/executions/{execution_id}/decider-lease/release",
            post(release_decider_lease),
        )
        // The command routes. Grouped under `commands/` for the run and under
        // the step for the two that name one, which reads correctly: pausing is
        // done to a run, and retrying is done to a step of one.
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

/// What a caller may ask this system to run.
///
/// Note what is not here, which is the same absence as `LaunchBody`'s and
/// `RerunBody`'s: no endpoint, no script, no host. A plan names a binding and
/// its parameters, and every executor's address is configuration.
/// `deny_unknown_fields` so an attempt to supply one — or a `backend`, a `mode`
/// or a `publish` flag that is not implemented — is a 400 naming it rather than
/// a field silently ignored that reads as accepted.
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
    /// Where this run's words live. Left out, the deployment's default, which
    /// is `external` unless somebody set otherwise: the content stays with the
    /// worker and this instance holds a reference, a digest and a size.
    ///
    /// `sealed` sends the content here to be encrypted under the conversation
    /// archive's keys, and is refused — naming both variables — when this
    /// instance has no archive. It is never quietly downgraded: a run that
    /// asked for its words to be sealed and got them kept somewhere else
    /// instead is the failure the whole policy exists to prevent.
    #[serde(default)]
    pub payloads: Option<aiwatcher_execution::message::PayloadPolicy>,
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
    // What the route decides, and all of it: who is asking, and which id. The
    // rest — which registry, which compiler, which payload policy, which
    // message id — is the use case's, shared with the tick that starts a
    // scheduled slot.
    let requester = caller
        .require(aiwatcher_auth::Role::Editor)?
        .log_subject()
        .to_owned();
    let started = state
        .executions()
        .compile_and_start(StartRequest {
            target: body.target,
            window: Window {
                seconds: body.window_seconds,
                as_of: body.as_of,
            },
            run: StartRun {
                identity: idempotently(&headers),
                parameters: body.parameters,
                requested_by: requester.clone(),
                decided_by: body.decided_by,
                payloads: body.payloads,
            },
        })
        .await?;
    let handled = started.handled;

    tracing::info!(
        execution_id = %started.execution_id,
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
    /// Deferred appends this decision sets or withdraws.
    ///
    /// In the same request rather than a route of its own: a decision that
    /// schedules a timeout and the record of having scheduled it are one
    /// decision, and split in two a crash between them leaves either a timer
    /// nobody decided on or a decision whose timer never happened.
    #[serde(default)]
    pub timers: Vec<TimerBody>,
}

/// One thing to do to this execution's timers.
///
/// A tagged pair rather than a struct with a `cancel` flag, because the two
/// carry different fields and a body that could name a `due_at` while
/// cancelling would be a shape with a meaningless half.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TimerBody {
    /// Hold this message until `due_at`. Scheduling one id twice is one timer,
    /// which is what makes a decider's retry safe.
    Schedule(ScheduleTimerBody),
    /// Withdraw it. A no-op when there is none, because a saga that cancels a
    /// timeout it already handled is doing the ordinary thing.
    Cancel(CancelTimerBody),
}

/// A deferred append, as a caller asks for one.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTimerBody {
    /// The worker's own id, unique inside this execution.
    pub timer_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub due_at: time::OffsetDateTime,
    /// What to append when it comes due. Composed by the caller, because an
    /// engine that assembled one would be deciding what a timeout means.
    pub message: aiwatcher_execution::message::HostedMessage,
}

/// The timer to withdraw.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelTimerBody {
    pub timer_id: String,
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

/// Append a worker's messages to a hosted execution's history.
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
        timers: body
            .timers
            .into_iter()
            .map(|timer| match timer {
                TimerBody::Schedule(set) => TimerWrite::Schedule(Timer {
                    execution: execution.clone(),
                    timer_id: set.timer_id,
                    due_at: set.due_at,
                    message: set.message,
                }),
                TimerBody::Cancel(drop) => TimerWrite::Cancel(drop.timer_id),
            })
            .collect(),
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

/// What one execution is still waiting on.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExecutionTimers {
    /// Soonest first. A fired or cancelled timer is not a row, so everything
    /// here is still going to happen.
    pub timers: Vec<Timer>,
}

/// The deferred appends this run is holding.
///
/// A read, so a decider that restarted can see what it scheduled before it went
/// away — which is the question the whole of step 4 exists to make answerable.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/timers",
    params(("execution_id" = String, Path)),
    responses(
        (status = 200, body = ExecutionTimers),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn execution_timers(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<ExecutionTimers>> {
    let timers = handler(&state)?
        .store()
        .timers_of(&ExecutionId::new(execution_id))
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    Ok(Json(ExecutionTimers { timers }))
}

/// Where sealed content went.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PayloadSealed {
    /// What the workflow stream records, resolvable only through this instance.
    pub reference: String,
    /// `sha256` of the plaintext, recomputed here rather than believed.
    pub digest: String,
    pub size: usize,
}

/// Seal one hosted execution's payload under this instance's keys.
///
/// The `sealed` half of a run's payload policy. `external` runs never call
/// this: their words stay wherever the worker keeps them, and this instance
/// holds a reference it cannot resolve — which is the free default and the one
/// a deployment gets by doing nothing.
///
/// The body is the plaintext, as bytes. It is not JSON on purpose: what is
/// being stored is somebody's words, and re-encoding them on the way in would
/// mean the digest recorded in the stream is a digest of this route's idea of
/// them rather than of what was sent.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/payloads",
    params(("execution_id" = String, Path)),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = PayloadSealed),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody, description = "No such execution"),
        (status = 501, body = crate::error::ErrorBody, description = "This instance has no conversation archive"),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn seal_payload(
    State(state): State<AppState>,
    caller: Caller,
    Path(execution_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<PayloadSealed>> {
    // An editor, as an append is: this is a worker storing its own turn, and
    // the cap on an ingest token is what keeps that from being more.
    caller.require(aiwatcher_auth::Role::Editor)?;
    let archive = state
        .conversations
        .as_ref()
        .ok_or(ApiError::ConversationArchiveDisabled)?;
    // A payload's lifetime is its run's, and the sweep that keeps that promise
    // reads "no projection" as "this run was forgotten". Sealing for a run that
    // never existed would put content in the archive that nothing accounts for
    // and the next sweep takes away — a 404 now says so at the moment it can be
    // acted on.
    if handler(&state)?
        .store()
        .projection(&ExecutionId::new(execution_id.clone()))
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .is_none()
    {
        return Err(ApiError::NotFound(format!("execution {execution_id}")));
    }
    let sealed = archive
        .seal_payload(&execution_id, &body)
        .await
        .map_err(ApiError::ConversationArchive)?;
    Ok(Json(PayloadSealed {
        reference: sealed.reference,
        digest: sealed.digest,
        size: sealed.size,
    }))
}

/// Read one back.
///
/// An `admin`, as reading a turn's content is, and for the same reason: these
/// are somebody's words, and the only thing that changed by sealing them here
/// rather than leaving them with the worker is that this deployment took
/// responsibility for them.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/payloads/{digest}",
    params(
        ("execution_id" = String, Path),
        ("digest" = String, Path, description = "The plaintext `sha256` the stream recorded"),
    ),
    responses(
        (status = 200, description = "The plaintext", content_type = "application/octet-stream"),
        (status = 403, body = crate::error::ErrorBody, description = "Reading content needs the admin role"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn read_payload(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, digest)): Path<(String, String)>,
) -> ApiResult<axum::response::Response> {
    caller.require(aiwatcher_auth::Role::Admin)?;
    let archive = state
        .conversations
        .as_ref()
        .ok_or(ApiError::ConversationArchiveDisabled)?;
    let plaintext = archive
        .open_payload(&execution_id, &digest)
        .await
        .map_err(ApiError::ConversationArchive)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        plaintext,
    )
        .into_response())
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

/// Take, or renew, the right to decide one hosted run.
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

/// Which id this run gets, from the header the caller may have sent.
///
/// The header is the only part of identity a route decides; what each answer
/// *means* is [`RunIdentity`]'s, because a scheduled slot names its own id and
/// a browser does not.
fn idempotently(headers: &HeaderMap) -> RunIdentity {
    headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map_or(RunIdentity::Fresh, |key| RunIdentity::Key(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_key(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_KEY, key.parse().expect("a header value"));
        headers
    }

    #[test]
    fn a_key_asks_for_the_run_it_already_started() {
        assert!(matches!(
            idempotently(&with_key("nightly-2026-09-05")),
            RunIdentity::Key(key) if key == "nightly-2026-09-05"
        ));
    }

    #[test]
    fn a_blank_key_is_no_key_rather_than_one_every_caller_shares() {
        // A header somebody sent empty would otherwise be an idempotency key
        // of `""`, which every other caller who did the same lands on: one
        // stream for unrelated runs, each answered as a duplicate of the
        // first.
        assert!(matches!(idempotently(&with_key("  ")), RunIdentity::Fresh));
        assert!(matches!(
            idempotently(&HeaderMap::new()),
            RunIdentity::Fresh
        ));
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
/// A message id derived from less than what it identifies collides with a
/// different intention, and the second is swallowed as a redelivery of the
/// first. A retry of `read` and a retry of `publish` are two commands.
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
    F: FnOnce(&Caller, &str, &RunProjection) -> ApiResult<WorkflowCommand>,
{
    // The floor, which every command shares. A route whose rule is stricter
    // than this applies it in `build`, where the run has already been read —
    // `provide_input` is the one, because which role may answer is a fact
    // about the step's own question and not about the route.
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

    let command = build(caller, &who, &current)?;
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

    apply(&state, &execution_id, &caller, |_, _, _| {
        Ok(WorkflowCommand::CancelExecution { reason })
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
    apply(&state, &execution_id, &caller, |_, _, _| {
        Ok(WorkflowCommand::PauseExecution)
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
    apply(&state, &execution_id, &caller, |_, _, _| {
        Ok(WorkflowCommand::ResumeExecution)
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
    apply(&state, &execution_id, &caller, |_, _, _| {
        Ok(WorkflowCommand::RetryStep { step_id })
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
    apply(&state, &execution_id, &caller, |caller, who, run| {
        // The role the *question* named, checked here because this is the one
        // place that knows it: it comes from the pinned plan, through the
        // step's own `awaiting`, and a route cannot know it in advance. The
        // editor floor is already held above; a gate only ever raises it, so a
        // step nobody is waiting on and a step asking for an editor both fall
        // through unchanged. A gate that asked for nothing readable is a gate
        // an editor answers.
        if let Some(asked) = run
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .and_then(|step| step.awaiting.as_ref())
            && let Ok(needed) = asked.role.parse::<aiwatcher_auth::Role>()
        {
            caller.require(needed)?;
        }
        // What this deployment allows an answer to be. Checked here because
        // this is the one door every answer comes through and the only place
        // that holds both the configuration and the step's own history —
        // `decide` reads no configuration and must not, or a replay would
        // reach a different decision on an instance configured differently.
        //
        // A step's answers accumulate: a parked attempt is resumed by one that
        // re-runs the work and reads all of them, so nothing takes any away.
        // Unbounded, the only backstop is the store refusing a message that has
        // grown too large — which arrives late, breaks the run, and names the
        // wrong thing.
        let given = run
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map_or(0, |step| step.answers.len());
        let bytes = serde_json::to_vec(&body.response).map_or(0, |json| json.len());
        state
            .answer_limits
            .admit(given, bytes)
            .map_err(ApiError::BadRequest)?;
        // Who answered comes from the session, never from the body. A field a
        // caller could set would make the one record of a human decision say
        // whatever the caller preferred it to say.
        Ok(WorkflowCommand::ProvideInput {
            step_id,
            attempt: body.attempt,
            answered_by: who.to_owned(),
            response: body.response,
        })
    })
    .await
}
