//! Current grant admission for producer evidence and its approvals.
use crate::{
    ApiError, AppState, Caller,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_evaluation::Registry;
use aiwatcher_iam::ProjectRole;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct EvidenceRead(pub Arc<Registry>);
pub(crate) struct EvidenceWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
pub(crate) struct EvidenceAdmin(pub EvidenceWrite);
impl EvidenceWrite {
    pub(crate) async fn authorize(self) -> ApiResult<Arc<Registry>> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.registry)
    }
}
async fn resolve(
    parts: &mut Parts,
    state: &AppState,
    role: ProjectRole,
) -> ApiResult<EvidenceWrite> {
    let authorization = project_scope::resolve_required(parts, state, role).await?;
    let caller = Caller::from_request_parts(parts, state).await?;
    let root = state
        .evaluations
        .as_ref()
        .ok_or(ApiError::EvaluationDisabled)?;
    let registry = match &authorization {
        Some(authorization) => root.for_project_evidence(authorization.scope)?,
        None => {
            caller.require(aiwatcher_auth::Role::Viewer)?;
            root.as_ref()
                .clone()
                .with_content_access(caller.require(aiwatcher_auth::Role::Admin).is_ok())
        }
    };
    Ok(EvidenceWrite {
        registry: Arc::new(registry),
        authorization,
    })
}
impl FromRequestParts<AppState> for EvidenceRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, ProjectRole::Viewer)
            .await
            .map(|value| Self(value.registry))
    }
}
impl FromRequestParts<AppState> for EvidenceWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, ProjectRole::Editor).await
    }
}
impl FromRequestParts<AppState> for EvidenceAdmin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, ProjectRole::Admin).await.map(Self)
    }
}
