//! When a definition runs unattended, and starting one now.
//!
//! Three routes over one object, plus the flag that makes setting a schedule
//! and running it once the same click:
//!
//! ```text
//! GET    /api/v1/curation-pipelines/{name}/schedule
//! PUT    /api/v1/curation-pipelines/{name}/schedule
//! DELETE /api/v1/curation-pipelines/{name}/schedule
//! ```
//!
//! ## Why `run_now` is not a fourth route
//!
//! "Run it once, and from tomorrow every day at nine" is one intention, and
//! two requests to express it is two ways for the second to fail after the
//! first succeeded. It is also not a special case in this design: a run *now*
//! is the slot at this instant, named by
//! [`ScheduledDefinition::execution_id_for`] exactly as a nine o'clock one is —
//! so a double-clicked button within one second is a redelivery and not a
//! second run, with no idempotency key to pass.
//!
//! The existing **Run on the server** button is a different thing and stays:
//! it starts an ad-hoc run of whatever is on the canvas, with no schedule
//! involved.
//!
//! ## Setting a schedule does not touch the definition
//!
//! It is stored beside it, not in it. Changing nine to ten must not mint a
//! pipeline revision — see [`aiwatcher_execution::schedule::store`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::OpenApi;

use aiwatcher_execution::plan::DefinitionKind;
use aiwatcher_execution::{
    Cadence, FiringOutcome, LastFiring, OverlapPolicy, Schedule, ScheduleStore, ScheduledDefinition,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(get_schedule, set_schedule, clear_schedule))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/curation-pipelines/{name}/schedule",
        get(get_schedule).put(set_schedule).delete(clear_schedule),
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
    ///
    /// The slot is this instant, so it goes through the same derived id as
    /// every other slot — which is what makes a double-clicked button a
    /// redelivery rather than two runs.
    #[serde(default)]
    pub run_now: bool,
}

const fn yes() -> bool {
    true
}

/// A schedule, when it next fires, and what setting it started.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScheduleView {
    pub schedule: ScheduledDefinition,
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
    fn of(schedule: ScheduledDefinition, started: Option<String>) -> Self {
        Self {
            next_run: schedule.schedule.next_after(OffsetDateTime::now_utc()),
            schedule,
            started,
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
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ScheduleView>> {
    schedules(&state)?
        .get(DefinitionKind::CurationPipeline, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .map(|schedule| Json(ScheduleView::of(schedule, None)))
        .ok_or_else(|| ApiError::NotFound(format!("a schedule for pipeline {name}")))
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
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
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

    // A pipeline that does not compile is refused here rather than at nine
    // tomorrow. It is also what turns a typo in the name into a 404 now.
    let plan = crate::executions::compile_curation_named(&state, &name, None, None).await?;

    // What the tick last did survives an edit. It is a fact about the
    // *definition* — a run was started for it at that slot — and changing the
    // hour does not make it untrue. Clearing it would make every edit look
    // like a schedule that has never fired.
    let previous = schedules(&state)?
        .get(DefinitionKind::CurationPipeline, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .and_then(|existing| existing.last);

    let mut scheduled = ScheduledDefinition {
        definition_kind: DefinitionKind::CurationPipeline,
        definition_name: name,
        schedule,
        set_by: who.clone(),
        updated_at: OffsetDateTime::now_utc(),
        last: previous,
    };

    let started = if body.run_now {
        let now = OffsetDateTime::now_utc();
        let execution_id = scheduled.execution_id_for(now);
        crate::executions::start(
            &state,
            &execution_id,
            plan,
            std::collections::BTreeMap::new(),
            &format!("schedule:{who}"),
        )
        .await?;
        // Recorded like any other firing: a run *was* started for a slot, and
        // a card reading "never fired" straight after somebody watched one
        // start would be a card nobody believes again.
        scheduled.last = Some(LastFiring {
            slot: now,
            outcome: FiringOutcome::Started,
            execution_id: Some(execution_id.to_string()),
            detail: None,
        });
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

    Ok(Json(ScheduleView::of(scheduled, started)))
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
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    schedules(&state)?
        .clear(DefinitionKind::CurationPipeline, &name)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    // 204 whether or not there was one: the caller asked for there to be no
    // schedule, and there is none.
    Ok(StatusCode::NO_CONTENT)
}
