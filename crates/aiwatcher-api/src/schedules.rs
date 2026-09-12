//! When a definition runs unattended, and starting one now.
//!
//! ```text
//! GET | PUT | DELETE  /api/v1/curation-pipelines/{name}/schedule
//! GET | PUT | DELETE  /api/v1/workflow-definitions/{name}/schedule
//! ```
//!
//! The [`DefinitionKind`] comes from the **path**, never from a body: a body
//! carrying its own kind would let a curation-pipeline URL write a workflow
//! schedule that nothing on that page would ever show.
//!
//! `run_now` is a flag rather than a fourth route. "Run it once, and from
//! tomorrow every day at nine" is one intention, and two requests to express it
//! is one more way for the second to fail after the first succeeded. It is not
//! a special case either: a run now is the slot at this instant, named by
//! [`ScheduledDefinition::execution_id_for`] exactly as a nine o'clock one is,
//! so a double-clicked button is a redelivery rather than a second run.
//!
//! A schedule is stored beside its definition, never in it — changing nine to
//! ten must not mint a revision. See
//! [`aiwatcher_execution::schedule::store`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::OpenApi;

use aiwatcher_execution::plan::DefinitionKind;
use aiwatcher_execution::{
    Cadence, OverlapPolicy, Schedule, ScheduleStore, ScheduledDefinition, SlotAdmissionRequest,
    SlotKey, SlotRecord, SlotSettlement, WorkflowStore as _,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    get_schedule,
    set_schedule,
    clear_schedule,
    get_workflow_schedule,
    set_workflow_schedule,
    clear_workflow_schedule
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/curation-pipelines/{name}/schedule",
            get(get_schedule).put(set_schedule).delete(clear_schedule),
        )
        .route(
            "/api/v1/workflow-definitions/{name}/schedule",
            get(get_workflow_schedule)
                .put(set_workflow_schedule)
                .delete(clear_workflow_schedule),
        )
}

/// What a caller sets.
///
/// `deny_unknown_fields`, like every other command body here: a `cron` somebody
/// hoped would work is a 400 naming it rather than a field that is ignored and
/// reads as accepted.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetScheduleBody {
    pub cadence: Cadence,
    /// An IANA name — `Europe/Warsaw`. Never an offset; see [`Schedule`].
    pub timezone: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub overlap: OverlapPolicy,
    /// Start it once, now, as well as saving it.
    #[serde(default)]
    pub run_now: bool,
    /// What makes two `run_now` requests the same request.
    ///
    /// The execution id is derived from it, so a retry — a proxy repeating the
    /// PUT, a click on a button whose first response was slow — lands on the
    /// run it already started instead of beside it. Without it the id comes
    /// from the second the request arrived in, which made a retry one second
    /// later a second run.
    ///
    /// Any stable string the caller minds: the panel sends one per user
    /// action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

const fn yes() -> bool {
    true
}

/// How many firings the panel is given. One card shows the last; the rest are
/// what makes "refused every morning for a week" visible as a pattern rather
/// than as one line that keeps changing.
const RECENT_SLOTS: usize = 20;

/// A schedule, when it next fires, and what setting it started.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScheduleView {
    pub schedule: ScheduledDefinition,
    /// What the tick did, newest first.
    ///
    /// Read from the **workflow store**, not from the schedule object. They
    /// used to be one thing, which is what let a tick's write-back undo an
    /// edit or resurrect a deleted schedule; configuration and slot
    /// outcomes now have different writers and live in different places.
    ///
    /// Empty when this deployment wired no execution store — there is then no
    /// tick either, so nothing has fired and an empty list is the true answer
    /// rather than a missing one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub firings: Vec<SlotRecord>,
    /// When the tick would next start it. `None` while it is off.
    ///
    /// Answered here rather than left to the caller, and that is the same rule
    /// as `RunView::allowed`: the panel computing it would be a second
    /// implementation of `slots_between` — in another language, with its own
    /// idea of when the clocks change — free to show an hour the run does not
    /// happen at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_run: Option<OffsetDateTime>,
    /// The run `run_now` started, when it was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
}

impl ScheduleView {
    async fn of(state: &AppState, schedule: ScheduledDefinition, started: Option<String>) -> Self {
        let firings = firings_of(state, &schedule).await;
        Self {
            next_run: schedule.schedule.next_after(OffsetDateTime::now_utc()),
            schedule,
            firings,
            started,
        }
    }
}

/// Write down a `run_now` as a firing of the slot it happened at.
///
/// Through the same admission the tick uses, so a double-clicked button is one
/// slot and one run: the second call finds the slot settled and starts nothing.
/// `OverlapPolicy::Allow` deliberately, whatever the schedule says — somebody
/// pressing the button has asked for this run, and `skip` is a policy about
/// what the *clock* should do unattended.
async fn record_manual_firing(
    state: &AppState,
    scheduled: &ScheduledDefinition,
    slot: OffsetDateTime,
    execution_id: &aiwatcher_execution::ExecutionId,
) {
    let Some(executions) = state.executions.as_ref() else {
        return;
    };
    let key = SlotKey::new(
        scheduled.definition_kind,
        scheduled.definition_name.clone(),
        slot,
    );
    let owner = format!("api/{}", std::process::id());
    let store = executions.store();
    let admitted = store
        .admit_slot(&SlotAdmissionRequest {
            key: key.clone(),
            owner: owner.clone(),
            overlap: OverlapPolicy::Allow,
            now: slot,
        })
        .await;
    if !matches!(admitted, Ok(aiwatcher_execution::SlotAdmission::Admitted)) {
        return;
    }
    if let Err(error) = store
        .settle_slot(
            &key,
            &owner,
            SlotSettlement::Started {
                execution_id: execution_id.to_string(),
            },
            slot,
        )
        .await
    {
        tracing::warn!(%error, "a manual run was started and could not be written down");
    }
}

/// One definition's firings, or none when there is no store to hold them.
///
/// A read failure is an empty list and a log line rather than a 500: the
/// schedule itself was read successfully, and refusing to show it because its
/// history could not be fetched would take the settings away over the
/// decoration.
async fn firings_of(state: &AppState, schedule: &ScheduledDefinition) -> Vec<SlotRecord> {
    let Some(executions) = state.executions.as_ref() else {
        return Vec::new();
    };
    match executions
        .store()
        .recent_slots(
            schedule.definition_kind,
            &schedule.definition_name,
            RECENT_SLOTS,
        )
        .await
    {
        Ok(firings) => firings,
        Err(error) => {
            tracing::warn!(
                definition = %schedule.definition_name,
                %error,
                "a schedule's firings could not be read"
            );
            Vec::new()
        }
    }
}

fn schedules(state: &AppState) -> ApiResult<&ScheduleStore> {
    state
        .schedules
        .as_deref()
        .ok_or(ApiError::DatasetRegistryDisabled)
}

/// One definition's schedule, or 404 when it has none.
#[utoipa::path(
    get,
    path = "/api/v1/curation-pipelines/{name}/schedule",
    params(("name" = String, Path, description = "The saved pipeline")),
    responses(
        (status = 200, body = ScheduleView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn get_schedule(
    state: State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ScheduleView>> {
    read(state, DefinitionKind::CurationPipeline, name).await
}

/// One workflow's schedule, or 404 when it has none.
#[utoipa::path(
    get,
    path = "/api/v1/workflow-definitions/{name}/schedule",
    params(("name" = String, Path, description = "The registered workflow")),
    responses(
        (status = 200, body = ScheduleView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn get_workflow_schedule(
    state: State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ScheduleView>> {
    read(state, DefinitionKind::Workflow, name).await
}

async fn read(
    State(state): State<AppState>,
    kind: DefinitionKind,
    name: String,
) -> ApiResult<Json<ScheduleView>> {
    let schedule = schedules(&state)?
        .get(kind, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .ok_or_else(|| ApiError::NotFound(format!("a schedule for {} {name}", noun(kind))))?;
    Ok(Json(ScheduleView::of(&state, schedule, None).await))
}

/// What a definition of this kind is called in a sentence somebody reads.
///
/// `DefinitionKind::as_str` is the wire word — `curation_pipeline` — and a 404
/// is prose. Kept next to the routes because it is presentation, and the same
/// reason `stage_hint` may decide nothing else.
const fn noun(kind: DefinitionKind) -> &'static str {
    match kind {
        DefinitionKind::CurationPipeline => "pipeline",
        DefinitionKind::Workflow => "workflow",
    }
}

/// Set or replace a schedule, and optionally start it once.
///
/// The definition is read first and a name nobody saved is a 404: a schedule
/// for a pipeline that does not exist is a run that fails every morning at nine
/// with nobody watching.
#[utoipa::path(
    put,
    path = "/api/v1/curation-pipelines/{name}/schedule",
    params(("name" = String, Path, description = "The saved pipeline")),
    request_body = SetScheduleBody,
    responses(
        (status = 200, body = ScheduleView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The schedule cannot mean anything"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn set_schedule(
    state: State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    body: Json<SetScheduleBody>,
) -> ApiResult<Json<ScheduleView>> {
    write(state, caller, DefinitionKind::CurationPipeline, name, body).await
}

/// Set or replace a workflow's schedule, and optionally start it once.
///
/// A workflow that does not compile is refused here rather than at nine
/// tomorrow, exactly as a pipeline is — the compiler differs and nothing else
/// does, which is why one function serves both.
#[utoipa::path(
    put,
    path = "/api/v1/workflow-definitions/{name}/schedule",
    params(("name" = String, Path, description = "The registered workflow")),
    request_body = SetScheduleBody,
    responses(
        (status = 200, body = ScheduleView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The schedule cannot mean anything"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn set_workflow_schedule(
    state: State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    body: Json<SetScheduleBody>,
) -> ApiResult<Json<ScheduleView>> {
    write(state, caller, DefinitionKind::Workflow, name, body).await
}

async fn write(
    State(state): State<AppState>,
    caller: Caller,
    kind: DefinitionKind,
    name: String,
    Json(body): Json<SetScheduleBody>,
) -> ApiResult<Json<ScheduleView>> {
    let who = caller
        .require(aiwatcher_auth::Role::Editor)?
        .log_subject()
        .to_owned();

    let schedule = Schedule {
        cadence: body.cadence,
        timezone: body.timezone,
        enabled: body.enabled,
        overlap: body.overlap,
    };
    schedule
        .check()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    // A definition that does not compile is refused here rather than at nine
    // tomorrow. It is also what turns a typo in the name into a 404 now — and
    // it is the API's own compiler, the same one the tick will reach for when
    // the slot comes due.
    let plan = state.executions().compile_head(kind, &name).await?;

    // What the tick last did survives an edit. It is a fact about the
    // *definition* — a run was started for it at that slot — and changing the
    // hour does not make it untrue. Clearing it would make every edit look
    // like a schedule that has never fired.
    let stored = schedules(&state)?
        .get(kind, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;

    let now = OffsetDateTime::now_utc();
    let mut scheduled = ScheduledDefinition {
        definition_kind: kind,
        definition_name: name,
        schedule,
        set_by: who.clone(),
        updated_at: now,
        // Carried forward when this edit does not change *when* it fires, so
        // an already-due slot survives a change to `overlap`; reset when it
        // does, so a new rule does not reach into the past.
        effective_from: None,
    };
    scheduled.effective_from = Some(scheduled.activation_after(stored.as_ref(), now));

    let started = if body.run_now {
        let execution_id = body.request_id.as_deref().map_or_else(
            || scheduled.execution_id_for(now),
            |request| scheduled.execution_id_for_request(request),
        );
        let handled = state
            .executions()
            .start(
                plan,
                aiwatcher_execution::StartRun {
                    identity: aiwatcher_execution::RunIdentity::Named(execution_id.clone()),
                    parameters: std::collections::BTreeMap::new(),
                    requested_by: format!("schedule:{who}"),
                    // A schedule runs a definition this system compiled, so
                    // this system decides it. A hosted run's decider is a
                    // worker that has to be there to receive it, which is not
                    // something a tick can arrange.
                    decided_by: aiwatcher_execution::Decider::Local,
                    // The deployment's own, and refused here for the same
                    // reason the route refuses it: a schedule that could not
                    // store a turn at nine tomorrow should say so when it is
                    // saved.
                    payloads: None,
                },
            )
            .await?
            .handled;
        // Recorded like any other firing, and in the same place the tick
        // records one: a run *was* started for a slot, and a card reading
        // "never fired" straight after somebody watched one start would be a
        // card nobody believes again. Best effort — the run has already
        // started, and failing the request now would invite a retry that
        // starts a second one.
        // Only for the request that actually started it. A retry lands on the
        // same execution, and writing a second firing for it would show two
        // lines in the panel for one run.
        if !handled.duplicate {
            record_manual_firing(&state, &scheduled, now, &execution_id).await;
        }
        Some(execution_id.to_string())
    } else {
        None
    };

    // After the run, so a stored `started` always has one behind it. The other
    // order costs a note the next tick would rewrite; this one cannot claim a
    // run that did not happen.
    schedules(&state)?
        .set(&scheduled)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;

    tracing::info!(
        definition = %scheduled.definition_name,
        timezone = %scheduled.schedule.timezone,
        enabled = scheduled.schedule.enabled,
        set_by = %who,
        started = started.as_deref().unwrap_or("-"),
        "set a schedule"
    );

    Ok(Json(ScheduleView::of(&state, scheduled, started).await))
}

/// Forget a schedule.
///
/// Deleting and `enabled: false` are different things and both are offered:
/// one says "not any more", the other "not for now, and these are still the
/// settings".
#[utoipa::path(
    delete,
    path = "/api/v1/curation-pipelines/{name}/schedule",
    params(("name" = String, Path, description = "The saved pipeline")),
    responses(
        (status = 204),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn clear_schedule(
    state: State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    forget(state, caller, DefinitionKind::CurationPipeline, name).await
}

/// Forget a workflow's schedule.
#[utoipa::path(
    delete,
    path = "/api/v1/workflow-definitions/{name}/schedule",
    params(("name" = String, Path, description = "The registered workflow")),
    responses(
        (status = 204),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn clear_workflow_schedule(
    state: State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    forget(state, caller, DefinitionKind::Workflow, name).await
}

async fn forget(
    State(state): State<AppState>,
    caller: Caller,
    kind: DefinitionKind,
    name: String,
) -> ApiResult<StatusCode> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    schedules(&state)?
        .clear(kind, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    // 204 whether or not there was one: the caller asked for there to be no
    // schedule, and there is none.
    Ok(StatusCode::NO_CONTENT)
}
