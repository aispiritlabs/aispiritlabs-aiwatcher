//! Resolve storage only after current project (or legacy instance) admission.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_prompts::Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct PromptRead(pub Arc<Registry>);
pub(crate) struct PromptWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
impl PromptWrite {
    /// A slow JSON upload cannot extend an expired or revoked write grant.
    pub(crate) async fn authorize(self) -> ApiResult<Arc<Registry>> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.registry)
    }
}
async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<PromptWrite> {
    let authorization = project_scope::resolve(parts, state, write).await?;
    let root = state.prompts.as_ref().ok_or(ApiError::RegistryDisabled)?;
    let registry = match &authorization {
        Some(authorization) => Arc::new(root.for_project(authorization.scope)?),
        None => root.clone(),
    };
    Ok(PromptWrite {
        registry,
        authorization,
    })
}
impl FromRequestParts<AppState> for PromptRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for PromptWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
