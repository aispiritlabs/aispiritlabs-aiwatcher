//! Admission for authored evaluation resources and native cohort derivation.
//! Evidence and execution routes need their own source and runtime boundary.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_evaluation::Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct EvaluationRead(pub Arc<Registry>);
pub(crate) struct CohortWrite(pub EvaluationWrite);
pub(crate) struct EvaluationWrite {
    registry: Arc<Registry>,
    authorization: Option<ProjectAuthorization>,
}
impl EvaluationWrite {
    /// Content-sensitive operations use the current project admin role after
    /// a slow body upload. An instance role never supplies that capability.
    pub(crate) async fn authorize_with_admin(
        self,
        legacy_admin: bool,
    ) -> ApiResult<(Arc<Registry>, bool)> {
        let admin = match self.authorization {
            Some(authorization) => {
                authorization.authorize_write_role().await? == aiwatcher_iam::ProjectRole::Admin
            }
            None => legacy_admin,
        };
        Ok((self.registry, admin))
    }

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
    write: bool,
    cohorts: bool,
) -> ApiResult<EvaluationWrite> {
    let authorization = project_scope::resolve(parts, state, write).await?;
    if authorization.is_none() {
        use crate::Caller;
        Caller::from_request_parts(parts, state)
            .await?
            .require(aiwatcher_auth::Role::Viewer)?;
    }
    let root = state
        .evaluations
        .as_ref()
        .ok_or(ApiError::EvaluationDisabled)?;
    let registry = match &authorization {
        Some(authorization) if cohorts => Arc::new(root.for_project_cohorts(authorization.scope)?),
        Some(authorization) => Arc::new(root.for_project_authored(authorization.scope)?),
        None => root.clone(),
    };
    Ok(EvaluationWrite {
        registry,
        authorization,
    })
}
impl FromRequestParts<AppState> for EvaluationRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false, false)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for EvaluationWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true, false).await
    }
}

impl FromRequestParts<AppState> for CohortWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true, true).await.map(Self)
    }
}
