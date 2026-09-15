//! Resolve storage only after current project (or legacy instance) admission.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_training::Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct TrainingRead(pub Arc<Registry>);
pub(crate) struct TrainingWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
impl TrainingWrite {
    /// A slow JSON upload cannot extend an expired or revoked write grant.
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
    required_role: aiwatcher_iam::ProjectRole,
) -> ApiResult<TrainingWrite> {
    let authorization = project_scope::resolve_required(parts, state, required_role).await?;
    let root = state
        .training
        .as_ref()
        .ok_or(ApiError::TrainingRegistryDisabled)?;
    let registry = match &authorization {
        Some(authorization) => Arc::new(root.for_project(authorization.scope)?),
        None => root.clone(),
    };
    Ok(TrainingWrite {
        registry,
        authorization,
    })
}
impl FromRequestParts<AppState> for TrainingRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, aiwatcher_iam::ProjectRole::Viewer)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for TrainingWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, aiwatcher_iam::ProjectRole::Editor).await
    }
}

/// Model labels require project admin, or instance admin on a legacy route.
pub(crate) struct TrainingPromote(pub TrainingWrite);
impl FromRequestParts<AppState> for TrainingPromote {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, aiwatcher_iam::ProjectRole::Admin)
            .await
            .map(Self)
    }
}
