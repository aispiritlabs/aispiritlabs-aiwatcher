//! Alert rules, the history of what was sent, and one button that proves the
//! channel works.
//!
//! Three shapes are worth noticing, and each is a decision rather than a
//! convention:
//!
//! * **There is no route that names a destination.** A rule says what is worth
//!   telling somebody about; where it goes is one deployment-wide channel from
//!   configuration. An endpoint that arrived in a request body would let
//!   anything able to publish a rule aim this process's outbound POSTs wherever
//!   it liked from inside the cluster.
//! * **Publishing is a `POST` to the collection**, as it is for a prompt and a
//!   lab: the caller does not choose the version id, because it is a digest
//!   over the document.
//! * **The test is synchronous and lands in the history.** A channel test that
//!   only queued something would answer "accepted" for a webhook nobody is
//!   listening on, which is the one answer a test must not give.
//!
//! `admin` guards everything that writes. Publishing a rule decides who gets
//! woken at four in the morning, and testing the channel makes this process
//! post to a third party — both are operator work, and neither is something an
//! editor's ingest token has any business doing.

use axum::extract::{Path, Query};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;

use aiwatcher_alerts::{
    AlertChannel, AlertError, AlertPayload, ChannelDescription, Delivery, DeliveryFilter,
    DeliveryPage, PublishRule, Registry, RuleFilter, RuleHead, RuleName, RulePage, RulePublished,
    RuleVersion, TriggerKind,
};
use aiwatcher_auth::Role;
use aiwatcher_jobs::JobState;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    list_alert_rules,
    publish_alert_rule,
    get_alert_rule,
    get_alert_rule_version,
    set_alert_rule_enabled,
    list_alert_deliveries,
    get_alert_delivery,
    retry_alert_delivery,
    get_alert_channel,
    test_alert_channel,
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
            "/api/v1/alert-rules",
            get(list_alert_rules).post(publish_alert_rule),
        )
        .route("/api/v1/alert-rules/{name}", get(get_alert_rule))
        .route(
            "/api/v1/alert-rules/{name}/versions/{version_id}",
            get(get_alert_rule_version),
        )
        .route(
            "/api/v1/alert-rules/{name}/enabled",
            put(set_alert_rule_enabled),
        )
        .route("/api/v1/alert-deliveries", get(list_alert_deliveries))
        .route("/api/v1/alert-deliveries/{key}", get(get_alert_delivery))
        .route(
            "/api/v1/alert-deliveries/{key}/retry",
            post(retry_alert_delivery),
        )
        .route("/api/v1/alert-channel", get(get_alert_channel))
        .route("/api/v1/alert-channel/test", post(test_alert_channel))
}

fn registry(state: &AppState) -> ApiResult<&Registry> {
    state
        .alerts
        .as_deref()
        .ok_or(ApiError::AlertRegistry(AlertError::Disabled))
}

fn channel(state: &AppState) -> ApiResult<&dyn AlertChannel> {
    state
        .alert_channel
        .as_deref()
        .ok_or(ApiError::AlertRegistry(AlertError::NoChannel))
}

fn name_of(raw: &str) -> ApiResult<RuleName> {
    RuleName::parse(raw).map_err(|error| ApiError::BadRequest(error.to_string()))
}

fn now() -> i64 {
    aiwatcher_alerts::now()
}

#[derive(Deserialize)]
struct RulePath {
    name: String,
}

#[derive(Deserialize)]
struct VersionPath {
    name: String,
    version_id: String,
}

#[derive(Deserialize)]
struct DeliveryPath {
    key: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct ListRulesQuery {
    /// Page by name: the last name on the previous page.
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct ListDeliveriesQuery {
    /// Only this rule's deliveries.
    rule: Option<String>,
    /// Only deliveries in this state.
    state: Option<JobState>,
    /// Page by key: the last key on the previous page.
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AlertRuleEnabledRequest)]
struct EnabledRequest {
    enabled: bool,
}

/// One rule, with the version it is at.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRuleDetail)]
pub struct RuleDetail {
    pub head: RuleHead,
    /// The version occurrences are matched against, carried with the rule so
    /// the page paints without a second round trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<RuleVersion>,
}

/// What this deployment can send a notification through.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertChannelView)]
pub struct ChannelView {
    /// Absent when no channel is configured, which is the default and is why
    /// this is a 200 carrying `None` rather than a 501: whether a channel
    /// exists is exactly the question being asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<ChannelDescription>,
}

/// What a channel test did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertChannelTest)]
pub struct ChannelTest {
    pub delivered: bool,
    /// The receiver's refusal, in the adapter's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The record this test left in the history, so a reader can find it.
    pub delivery: Delivery,
}

// ── Rules ────────────────────────────────────────────────────────────────────

/// Every alert rule, by name.
#[utoipa::path(
    get, path = "/api/v1/alert-rules", params(ListRulesQuery),
    responses(
        (status = 200, body = aiwatcher_alerts::RulePage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn list_alert_rules(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Query(query): Query<ListRulesQuery>,
) -> ApiResult<Json<RulePage>> {
    caller.require(Role::Viewer)?;
    let filter = RuleFilter {
        after: query.after,
        limit: query.limit,
    };
    Ok(Json(registry(&state)?.list(&filter).await?))
}

/// Publish a version of an alert rule.
///
/// Idempotent on the document: `created` is `false` when this exact rule was
/// already stored, and nothing that already fired under it fires again.
#[utoipa::path(
    post, path = "/api/v1/alert-rules", request_body = PublishRule,
    responses(
        (status = 201, body = RulePublished, description = "A new version was stored"),
        (status = 200, body = RulePublished, description = "This rule was already stored"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn publish_alert_rule(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Json(request): Json<PublishRule>,
) -> ApiResult<(axum::http::StatusCode, Json<RulePublished>)> {
    caller.require(Role::Admin)?;
    let author = Some(caller.identity().log_subject().to_owned());
    let published = registry(&state)?.publish(request, author, now()).await?;
    let status = if published.created {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(published)))
}

/// One rule: its versions, and the one it is at now.
#[utoipa::path(
    get, path = "/api/v1/alert-rules/{name}",
    params(("name" = String, Path, description = "The rule to fetch")),
    responses(
        (status = 200, body = RuleDetail),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn get_alert_rule(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Path(RulePath { name }): Path<RulePath>,
) -> ApiResult<Json<RuleDetail>> {
    caller.require(Role::Viewer)?;
    let name = name_of(&name)?;
    let registry = registry(&state)?;
    let head = registry
        .head(&name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("alert rule {name}")))?;
    let current = match head.current.as_deref() {
        Some(version) => registry.version(&name, version).await?,
        None => None,
    };
    Ok(Json(RuleDetail { head, current }))
}

/// One version of a rule.
#[utoipa::path(
    get, path = "/api/v1/alert-rules/{name}/versions/{version_id}",
    params(
        ("name" = String, Path, description = "The rule"),
        ("version_id" = String, Path, description = "The version's digest"),
    ),
    responses(
        (status = 200, body = RuleVersion),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn get_alert_rule_version(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Path(VersionPath { name, version_id }): Path<VersionPath>,
) -> ApiResult<Json<RuleVersion>> {
    caller.require(Role::Viewer)?;
    let name = name_of(&name)?;
    registry(&state)?
        .version(&name, &version_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("alert rule {name} version {version_id}")))
}

/// Switch a rule on or off.
///
/// Not an edit: no version is published, the dedup key does not move, and
/// switching a rule back on does not re-send what it already sent while it was
/// on. Silencing a rule during an incident is what this is for.
#[utoipa::path(
    put, path = "/api/v1/alert-rules/{name}/enabled",
    params(("name" = String, Path, description = "The rule")),
    request_body = EnabledRequest,
    responses(
        (status = 200, body = RuleHead),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn set_alert_rule_enabled(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Path(RulePath { name }): Path<RulePath>,
    Json(request): Json<EnabledRequest>,
) -> ApiResult<Json<RuleHead>> {
    caller.require(Role::Admin)?;
    let name = name_of(&name)?;
    Ok(Json(
        registry(&state)?
            .set_enabled(&name, request.enabled, now())
            .await?,
    ))
}

// ── Deliveries ───────────────────────────────────────────────────────────────

/// What was sent, newest first.
#[utoipa::path(
    get, path = "/api/v1/alert-deliveries", params(ListDeliveriesQuery),
    responses(
        (status = 200, body = DeliveryPage),
        (status = 400, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn list_alert_deliveries(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Query(query): Query<ListDeliveriesQuery>,
) -> ApiResult<Json<DeliveryPage>> {
    caller.require(Role::Viewer)?;
    let rule = query.rule.as_deref().map(name_of).transpose()?;
    let filter = DeliveryFilter {
        rule,
        state: query.state,
        after: query.after,
        limit: query.limit,
    };
    Ok(Json(registry(&state)?.deliveries(&filter).await?))
}

/// One delivery, by its dedup key.
#[utoipa::path(
    get, path = "/api/v1/alert-deliveries/{key}",
    params(("key" = String, Path, description = "The dedup key")),
    responses(
        (status = 200, body = Delivery),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn get_alert_delivery(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Path(DeliveryPath { key }): Path<DeliveryPath>,
) -> ApiResult<Json<Delivery>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .delivery(&key)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("alert delivery {key}")))
}

/// Send a failed delivery again.
///
/// The budget starts over, which is the point: a channel that was down for an
/// hour outlives three attempts, and somebody deciding this one is still worth
/// sending is a different fact from a receiver that refused the body. A
/// delivery that did not fail answers 409 — re-sending a delivered one would
/// be this system creating the duplicate it exists to avoid.
#[utoipa::path(
    post, path = "/api/v1/alert-deliveries/{key}/retry",
    params(("key" = String, Path, description = "The dedup key")),
    responses(
        (status = 200, body = Delivery),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "It did not fail"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "alerts",
)]
async fn retry_alert_delivery(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
    Path(DeliveryPath { key }): Path<DeliveryPath>,
) -> ApiResult<Json<Delivery>> {
    caller.require(Role::Admin)?;
    Ok(Json(registry(&state)?.retry(&key, now()).await?))
}

// ── The channel ──────────────────────────────────────────────────────────────

/// What this deployment sends through, and never where.
#[utoipa::path(
    get, path = "/api/v1/alert-channel",
    responses((status = 200, body = ChannelView), (status = 403, body = crate::error::ErrorBody)),
    tag = "alerts",
)]
async fn get_alert_channel(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ChannelView>> {
    caller.require(Role::Admin)?;
    Ok(Json(ChannelView {
        channel: state
            .alert_channel
            .as_deref()
            .map(aiwatcher_alerts::AlertChannel::describe),
    }))
}

/// Send one notification through the configured channel, now.
///
/// Synchronous on purpose. A test that queued something would answer
/// "accepted" for a webhook nobody is listening on, which is the one answer a
/// test must not give. It leaves a record in the history like any other
/// delivery, so what a receiver saw can be found afterwards.
#[utoipa::path(
    post, path = "/api/v1/alert-channel/test",
    responses(
        (status = 200, body = ChannelTest),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody, description = "No channel is configured"),
    ),
    tag = "alerts",
)]
async fn test_alert_channel(
    axum::extract::State(state): axum::extract::State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ChannelTest>> {
    caller.require(Role::Admin)?;
    let registry = registry(&state)?;
    let channel = channel(&state)?;
    let at = now();
    let payload = test_payload(caller.identity().log_subject(), at)?;
    let mut delivery = Delivery::new(payload, at);

    let error = match channel.deliver(&delivery.payload).await {
        Ok(()) => {
            delivery.sent(at);
            None
        }
        Err(error) => {
            let message = error.to_string();
            // Never requeued, whatever went wrong: the point of a test is the
            // answer, and a queued retry behind it would send a second copy of
            // something nobody asked to receive twice.
            delivery.attempt_failed(&message, false, at);
            Some(message)
        }
    };
    registry.record(&delivery).await?;
    Ok(Json(ChannelTest {
        delivered: error.is_none(),
        error,
        delivery,
    }))
}

/// The rule name a test is recorded under.
///
/// Not a [`RuleName`] anybody published: a test belongs in the history beside
/// the real deliveries, and inventing a rule for it would put a row in the
/// rules list that nothing raises.
const TEST_RULE: &str = "channel-test";

fn test_payload(by: &str, at: i64) -> ApiResult<AlertPayload> {
    // Every test gets its own key. A test is the one delivery that *should*
    // repeat: somebody asking twice whether the channel works is asking twice.
    let occurrence = aiwatcher_alerts::AlertSignal {
        trigger: TriggerKind::ExecutionFailed,
        subject: format!("{TEST_RULE}:{by}:{at}"),
        within: None,
        occurred_at: at,
        title: String::new(),
        facts: Vec::new(),
        links: Vec::new(),
    };
    Ok(AlertPayload {
        dedup_key: aiwatcher_alerts::dedup_key(TEST_RULE, &occurrence),
        rule: name_of(TEST_RULE)?,
        rule_version: TEST_RULE.to_owned(),
        trigger: TriggerKind::ExecutionFailed,
        subject: by.to_owned(),
        title: "aiwatcher alert channel test".to_owned(),
        description: "Somebody asked this deployment to prove it can reach you.".to_owned(),
        occurred_at: at,
        facts: vec![aiwatcher_alerts::AlertFact::new("asked by", by)],
        links: vec![aiwatcher_alerts::AlertLink::new("alerts", "/system/alerts")],
    })
}
