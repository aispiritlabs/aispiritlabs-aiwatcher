//! Additive IAM metadata API. Project dataset routes reuse its verified context.
//! Identity always comes from the outer authentication layer; instance roles
//! authorize only organization creation, never another organization's access.

use crate::{ApiError, AppState, Caller, error::ApiResult};
use aiwatcher_auth::Role;
use aiwatcher_iam::{
    AuditEntry, Change, Command, Grant, IamStore, Invitation, InvitationId, InvitationOffer,
    IssuedInvitation, Organization, OrganizationId, Principal, ProjectAccess, ProjectId,
    ProjectScope, Redeemed, Roster,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use serde::Deserialize;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    organizations,
    create_organization,
    apply,
    projects,
    access,
    roster,
    project_grants,
    invite,
    invitations,
    revoke_invitation,
    redeem,
    audit
))]
struct Api;
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/iam/organizations",
            get(organizations).post(create_organization),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/commands",
            post(apply),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/projects",
            get(projects),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/projects/{project}/access",
            get(access),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/roster",
            get(roster),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/projects/{project}/grants",
            get(project_grants),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/projects/{project}/invitations",
            post(invite),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/invitations",
            get(invitations),
        )
        .route(
            "/api/v1/iam/organizations/{organization}/invitations/{invitation}",
            delete(revoke_invitation),
        )
        // Outside every organization on purpose: whoever redeems one holds a
        // token and knows no id to put in a path.
        .route("/api/v1/iam/invitations/redeem", post(redeem))
        .route("/api/v1/iam/organizations/{organization}/audit", get(audit))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
}

pub(crate) fn context<'a>(
    state: &'a AppState,
    caller: &Caller,
) -> ApiResult<(&'a dyn IamStore, Principal)> {
    let store = state.iam.as_deref().ok_or(ApiError::IamDisabled)?;
    let principal = state
        .auth
        .as_ref()
        .ok_or(ApiError::Unauthenticated)?
        .iam_principal(caller.identity())
        .map_err(|_| ApiError::Unauthenticated)?;
    Ok((store, principal))
}

/// Non-simple header prevents cookie-authenticated cross-origin form writes.
/// Browser fetches from another origin must pass the deployment's CORS policy.
pub(crate) fn mutation(headers: &HeaderMap) -> ApiResult<()> {
    if headers.get("x-aiwatcher-iam").and_then(|h| h.to_str().ok()) != Some("1") {
        return Err(ApiError::IamForbidden);
    }
    Ok(())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganization {
    pub name: String,
}

#[utoipa::path(get, path = "/api/v1/iam/organizations", tag = "iam",
    responses((status = 200, body = Vec<Organization>), (status = 401), (status = 501)))]
async fn organizations(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<Vec<Organization>>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(store.organizations(&actor).await?))
}

/// Bootstrap policy: an OIDC instance admin creates an organization owned by
/// themselves. A client cannot substitute an owner in the request body.
#[utoipa::path(post, path = "/api/v1/iam/organizations", tag = "iam", request_body = CreateOrganization,
    params(("X-AIWatcher-IAM" = String, Header, description = "Required value: 1")),
    responses((status = 201, body = Organization), (status = 400), (status = 401), (status = 403), (status = 501), (status = 503)))]
async fn create_organization(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<CreateOrganization>,
) -> ApiResult<(StatusCode, Json<Organization>)> {
    let (store, actor) = context(&state, &caller)?;
    mutation(&headers)?;
    caller.require(Role::Admin)?;
    Ok((
        StatusCode::CREATED,
        Json(store.create_organization(&actor, &body.name).await?),
    ))
}

#[utoipa::path(post, path = "/api/v1/iam/organizations/{organization}/commands", tag = "iam", request_body = Command,
    params(("organization" = OrganizationId, Path), ("X-AIWatcher-IAM" = String, Header, description = "Required value: 1")),
    responses((status = 200, body = Change), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 501), (status = 503)))]
async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Path(organization): Path<OrganizationId>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> ApiResult<Json<Change>> {
    let (store, actor) = context(&state, &caller)?;
    mutation(&headers)?;
    Ok(Json(store.apply(organization, &actor, command).await?))
}

#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/projects", tag = "iam",
    params(("organization" = OrganizationId, Path)),
    responses((status = 200, body = Vec<ProjectAccess>), (status = 401), (status = 404), (status = 501), (status = 503)))]
async fn projects(
    State(state): State<AppState>,
    caller: Caller,
    Path(organization): Path<OrganizationId>,
) -> ApiResult<Json<Vec<ProjectAccess>>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(store.projects(organization, &actor).await?))
}

#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/projects/{project}/access", tag = "iam",
    params(("organization" = OrganizationId, Path), ("project" = ProjectId, Path)),
    responses((status = 200, body = ProjectAccess), (status = 401), (status = 404), (status = 501), (status = 503)))]
async fn access(
    State(state): State<AppState>,
    caller: Caller,
    Path((organization, project)): Path<(OrganizationId, ProjectId)>,
) -> ApiResult<Json<ProjectAccess>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(
        store
            .access(
                ProjectScope {
                    organization,
                    project,
                },
                &actor,
            )
            .await?,
    ))
}

/// Who is in this organization, for an owner or admin of it.
///
/// The one read here that is not about the caller. `projects` and `access`
/// answer "what may I reach", which is the right shape for a permission check
/// and no use to somebody administering one: an organization admin may grant on
/// a project they hold no grant on, and without this could not name it.
#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/roster", tag = "iam",
    params(("organization" = OrganizationId, Path)),
    responses((status = 200, body = Roster), (status = 401), (status = 403), (status = 404), (status = 501), (status = 503)))]
async fn roster(
    State(state): State<AppState>,
    caller: Caller,
    Path(organization): Path<OrganizationId>,
) -> ApiResult<Json<Roster>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(store.roster(organization, &actor).await?))
}

/// Every grant on one project, for whoever may issue one there.
///
/// The windows are as issued and nothing is filtered by the clock — what a
/// grant does *now* is the access route's answer, per person, and a list that
/// dropped a lapsed row would hide the thing somebody opened it to see.
#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/projects/{project}/grants", tag = "iam",
    params(("organization" = OrganizationId, Path), ("project" = ProjectId, Path)),
    responses((status = 200, body = Vec<Grant>), (status = 401), (status = 403), (status = 404), (status = 501), (status = 503)))]
async fn project_grants(
    State(state): State<AppState>,
    caller: Caller,
    Path((organization, project)): Path<(OrganizationId, ProjectId)>,
) -> ApiResult<Json<Vec<Grant>>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(
        store
            .project_grants(
                ProjectScope {
                    organization,
                    project,
                },
                &actor,
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Redeem {
    pub token: String,
}

/// Offer a grant to somebody who may never have signed in here.
///
/// A grant names a `(provider, subject)` pair, and nobody knows a stranger's
/// subject until their provider has minted one — so the offer is made to a
/// secret instead, and the pair is learned when it is redeemed. The token comes
/// back once, in this response, and nothing can show it again.
#[utoipa::path(post, path = "/api/v1/iam/organizations/{organization}/projects/{project}/invitations", tag = "iam",
    request_body = InvitationOffer,
    params(("organization" = OrganizationId, Path), ("project" = ProjectId, Path), ("X-AIWatcher-IAM" = String, Header, description = "Required value: 1")),
    responses((status = 201, body = IssuedInvitation), (status = 400), (status = 401), (status = 403), (status = 404), (status = 501), (status = 503)))]
async fn invite(
    State(state): State<AppState>,
    caller: Caller,
    Path((organization, project)): Path<(OrganizationId, ProjectId)>,
    headers: HeaderMap,
    Json(offer): Json<InvitationOffer>,
) -> ApiResult<(StatusCode, Json<IssuedInvitation>)> {
    let (store, actor) = context(&state, &caller)?;
    mutation(&headers)?;
    Ok((
        StatusCode::CREATED,
        Json(
            store
                .invite(
                    ProjectScope {
                        organization,
                        project,
                    },
                    &actor,
                    offer,
                )
                .await?,
        ),
    ))
}

/// The offers for projects this caller may administer, newest first.
#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/invitations", tag = "iam",
    params(("organization" = OrganizationId, Path)),
    responses((status = 200, body = Vec<Invitation>), (status = 401), (status = 403), (status = 404), (status = 501), (status = 503)))]
async fn invitations(
    State(state): State<AppState>,
    caller: Caller,
    Path(organization): Path<OrganizationId>,
) -> ApiResult<Json<Vec<Invitation>>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(store.invitations(organization, &actor).await?))
}

/// Withdraw an offer nobody has taken up.
///
/// A redeemed one answers 409 rather than disappearing: what it produced is a
/// grant, and taking that back is the revoke-grant command's job.
#[utoipa::path(delete, path = "/api/v1/iam/organizations/{organization}/invitations/{invitation}", tag = "iam",
    params(("organization" = OrganizationId, Path), ("invitation" = InvitationId, Path), ("X-AIWatcher-IAM" = String, Header, description = "Required value: 1")),
    responses((status = 204), (status = 401), (status = 403), (status = 404), (status = 409), (status = 501), (status = 503)))]
async fn revoke_invitation(
    State(state): State<AppState>,
    caller: Caller,
    Path((organization, invitation)): Path<(OrganizationId, InvitationId)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let (store, actor) = context(&state, &caller)?;
    mutation(&headers)?;
    store
        .revoke_invitation(organization, &actor, invitation)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Turn a token into membership and a grant, once.
///
/// Any signed-in principal may call this — the token is the authority, and the
/// caller's verified pair is what the grant is written to. It names no
/// organization because whoever holds one does not know which organization it
/// belongs to, and a route that made them say would leak that they had guessed
/// right.
#[utoipa::path(post, path = "/api/v1/iam/invitations/redeem", tag = "iam", request_body = Redeem,
    params(("X-AIWatcher-IAM" = String, Header, description = "Required value: 1")),
    responses((status = 200, body = Redeemed), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 410), (status = 501), (status = 503)))]
async fn redeem(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Json(body): Json<Redeem>,
) -> ApiResult<Json<Redeemed>> {
    let (store, actor) = context(&state, &caller)?;
    mutation(&headers)?;
    Ok(Json(store.redeem(&body.token, &actor).await?))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AuditQuery {
    #[serde(default)]
    pub after: i64,
    #[serde(default = "audit_default_limit")]
    pub limit: usize,
}
fn audit_default_limit() -> usize {
    50
}

#[utoipa::path(get, path = "/api/v1/iam/organizations/{organization}/audit", tag = "iam",
    params(("organization" = OrganizationId, Path), AuditQuery),
    responses((status = 200, body = Vec<AuditEntry>), (status = 400), (status = 401), (status = 403), (status = 404), (status = 501), (status = 503)))]
async fn audit(
    State(state): State<AppState>,
    caller: Caller,
    Path(organization): Path<OrganizationId>,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let (store, actor) = context(&state, &caller)?;
    Ok(Json(
        store
            .audit(organization, &actor, query.after, query.limit)
            .await?,
    ))
}
