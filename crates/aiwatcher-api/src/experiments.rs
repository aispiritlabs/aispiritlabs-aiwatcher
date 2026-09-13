//! Experiments: the variants measured in one context, side by side.
//!
//! Quality, sample size, failures and what the answers took come from the
//! published evidence, which outlives the log; how long each whole run took
//! comes from the log's own fold, for as long as it keeps the run. The two are
//! kept apart on purpose — a run's duration is every step and every retry, and
//! a case's latency is one answer — and nothing here averages one result's
//! percentiles with another's.
//!
//! What each variant was observed doing is a third clock: the runs that named
//! it on the log, served to somebody using the application rather than made
//! for a measurement, over the window asked for — from the projector's period
//! fold, which holds every period whole, wherever there is a store to write
//! periods to; and without a window, or without that store, from the read
//! model.

use aiwatcher_auth::Role;
use aiwatcher_evaluation::{Experiment, ExperimentIndex};
use aiwatcher_projector::{ExecutionSummary, VariantObservations};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(list_experiments, get_experiment))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/experiments", get(list_experiments))
        .route("/api/v1/experiments/{context_id}", get(get_experiment))
}

fn registry(state: &AppState) -> ApiResult<aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .cloned()
        .ok_or(ApiError::EvaluationDisabled)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The contexts results were published in, newest first — each an experiment:
/// what it measures, how many results and how many variants.
#[utoipa::path(get, path = "/api/v1/experiments",
    responses((status = 200, body = ExperimentIndex), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_experiments(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ExperimentIndex>> {
    caller.require(Role::Viewer)?;
    Ok(Json(
        registry(&state)?
            .with_content_access(caller.require(Role::Admin).is_ok())
            .experiments(&caller.identity().subject, now())
            .await?,
    ))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct ExperimentQuery {
    /// A result in this context every other row is compared with.
    baseline: Option<String>,
    /// How far back the observed runs reach, in seconds; absent or zero is
    /// everything the read model still holds. A window counts every period it
    /// reaches into, whole, and says where counting began.
    window_seconds: Option<i64>,
}

/// One experiment, and the runs its results were measured by.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExperimentView {
    pub experiment: Experiment,
    /// The managed runs the rows name, as the log folded them. A run the log
    /// no longer holds is absent, and its row keeps everything the evidence
    /// says.
    pub executions: Vec<ExecutionSummary>,
    /// What each variant the rows measured was observed doing: the runs on the
    /// log that name it and that no measurement made, one row per variant
    /// whether or not any did. A different sample from the evidence, on the
    /// log's clock, and never folded into it.
    pub observed: Vec<VariantObservations>,
}

/// Every result published in one context, newest first, compared with the
/// baseline when one is named — and the whole-run timing of each managed run
/// behind them.
#[utoipa::path(get, path = "/api/v1/experiments/{context_id}",
    params(("context_id" = String, Path), ExperimentQuery),
    responses((status = 200, body = ExperimentView), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_experiment(
    State(state): State<AppState>,
    caller: Caller,
    Path(context_id): Path<String>,
    Query(query): Query<ExperimentQuery>,
) -> ApiResult<Json<ExperimentView>> {
    caller.require(Role::Viewer)?;
    let experiment = registry(&state)?
        .with_content_access(caller.require(Role::Admin).is_ok())
        .experiment(
            &context_id,
            query.baseline.as_deref(),
            &caller.identity().subject,
            now(),
        )
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(match &query.baseline {
                Some(baseline) => format!("experiment {context_id} with baseline {baseline}"),
                None => format!("experiment {context_id}"),
            })
        })?;
    let experiment = match state.model_prices.as_deref() {
        Some(prices) => experiment.priced(prices),
        None => experiment,
    };
    let mut executions = Vec::new();
    let named: std::collections::BTreeSet<&str> = experiment
        .rows
        .iter()
        .filter_map(|row| row.origin.as_ref()?.execution_id.as_deref())
        .collect();
    for id in named {
        if let Some(detail) = state.read_model.workflow_execution(id).await {
            executions.push(detail.summary);
        }
    }
    let mut variants: Vec<&str> = Vec::new();
    for row in &experiment.rows {
        if !variants.contains(&row.variant_id.as_str()) {
            variants.push(&row.variant_id);
        }
    }
    // A window is the period fold's alone: without one, the question is what
    // the read model holds, and every period ever written is not that.
    let observed = match (&state.observations, query.window_seconds) {
        (Some(fold), Some(window)) if window > 0 => fold
            .observe(&variants, now() - window, state.model_prices.as_deref())
            .await
            .map_err(aiwatcher_evaluation::EvaluationError::Storage)?,
        _ => {
            state
                .read_model
                .variant_observations(
                    &variants,
                    query.window_seconds,
                    state.model_prices.as_deref(),
                )
                .await
        }
    };
    Ok(Json(ExperimentView {
        experiment,
        executions,
        observed,
    }))
}
