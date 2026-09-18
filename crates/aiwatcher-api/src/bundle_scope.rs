//! Current IAM admission for project approval bundle storage.
use crate::{
    ApiError, AppState, Caller,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_auth::Role;
use aiwatcher_evaluation::ApprovalBundles;
use aiwatcher_iam::ProjectRole;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

pub(crate) struct BundleRead(pub Arc<dyn ApprovalBundles>);
pub(crate) struct BundleWrite {
    bundles: Arc<dyn ApprovalBundles>,
    authorization: Option<ProjectAuthorization>,
}
impl BundleWrite {
    pub(crate) async fn authorize(self) -> ApiResult<Arc<dyn ApprovalBundles>> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.bundles)
    }
}
async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<BundleWrite> {
    let authorization = project_scope::resolve_required(
        parts,
        state,
        if write {
            ProjectRole::Admin
        } else {
            ProjectRole::Viewer
        },
    )
    .await?;
    if authorization.is_none() {
        Caller::from_request_parts(parts, state)
            .await?
            .require(Role::Viewer)?;
    }
    let root = state
        .evaluation_bundles
        .as_ref()
        .ok_or(ApiError::EvaluationDisabled)?;
    let bundles = match &authorization {
        Some(authorization) => root.for_project(authorization.scope)?,
        None => root.clone(),
    };
    Ok(BundleWrite {
        bundles,
        authorization,
    })
}
impl FromRequestParts<AppState> for BundleRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.bundles))
    }
}
impl FromRequestParts<AppState> for BundleWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
