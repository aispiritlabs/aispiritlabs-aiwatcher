//! Resolve storage only after current project (or legacy instance) admission.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_annotations::Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct AnnotationRead(pub Arc<Registry>);
pub(crate) struct AnnotationWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
impl AnnotationWrite {
    /// A slow JSON upload cannot extend an expired or revoked write grant.
    pub(crate) async fn authorize(self) -> ApiResult<Arc<Registry>> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.registry)
    }
}
async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<AnnotationWrite> {
    let authorization = project_scope::resolve(parts, state, write).await?;
    let root = state
        .annotations
        .as_ref()
        .ok_or(ApiError::AnnotationRegistryDisabled)?;
    let registry = match &authorization {
        Some(authorization) => Arc::new(root.for_project(authorization.scope)?),
        None => root.clone(),
    };
    Ok(AnnotationWrite {
        registry,
        authorization,
    })
}
impl FromRequestParts<AppState> for AnnotationRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for AnnotationWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
