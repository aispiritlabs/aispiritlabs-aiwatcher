//! Resolve storage only after current project (or legacy instance) admission.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_execution::definition::DefinitionRegistry as Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct DefinitionRead(pub Arc<Registry>);
pub(crate) struct DefinitionWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
impl DefinitionWrite {
    /// A slow JSON upload cannot extend an expired or revoked write grant.
    pub(crate) async fn authorize(self) -> ApiResult<Arc<Registry>> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.registry)
    }
}
async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<DefinitionWrite> {
    let authorization = project_scope::resolve(parts, state, write).await?;
    let root = state
        .workflow_definitions
        .as_ref()
        .ok_or(ApiError::WorkflowDefinitionsDisabled)?;
    let registry = match &authorization {
        Some(authorization) => Arc::new(root.for_project(authorization.scope)?),
        None => root.clone(),
    };
    Ok(DefinitionWrite {
        registry,
        authorization,
    })
}
impl FromRequestParts<AppState> for DefinitionRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for DefinitionWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
