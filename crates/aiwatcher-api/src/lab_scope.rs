//! Resolve lab storage only after current project (or legacy instance)
//! admission, exactly as `prompt_scope` does for prompts.
//!
//! It resolves **two** registries, bound to the same scope: a lab's tests are
//! a scorecard version and a derived cohort, and those are checked where they
//! live rather than taken on the lab's word. One admission decides both,
//! because a caller who may write a project's labs is by definition reading
//! that project's cards.
use crate::{
    ApiError, AppState,
    error::ApiResult,
    project_scope::{self, ProjectAuthorization},
};
use aiwatcher_labs::Registry;
use axum::{extract::FromRequestParts, http::request::Parts};
use std::sync::Arc;

/// The registries a lab route reads: its own, and the evaluation registry its
/// pins are resolved in — `None` when this deployment wired none, which is a
/// lab that can be read and one whose measurement cannot be named.
pub(crate) struct Labs {
    pub(crate) registry: Arc<Registry>,
    pub(crate) evaluations: Option<Arc<aiwatcher_evaluation::Registry>>,
}

pub(crate) struct LabRead(pub(crate) Labs);
pub(crate) struct LabWrite {
    labs: Labs,
    authorization: Option<ProjectAuthorization>,
}

impl LabWrite {
    /// A slow upload of a long brief cannot extend an expired or revoked
    /// write grant.
    pub(crate) async fn authorize(self) -> ApiResult<Labs> {
        if let Some(authorization) = self.authorization {
            authorization.authorize_write().await?;
        }
        Ok(self.labs)
    }
}

async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<LabWrite> {
    let authorization = project_scope::resolve(parts, state, write).await?;
    let root = state.labs.as_ref().ok_or(ApiError::LabRegistryDisabled)?;
    let (registry, evaluations) = match &authorization {
        Some(authorization) => (
            Arc::new(root.for_project(authorization.scope)?),
            match state.evaluations.as_ref() {
                Some(registry) => Some(Arc::new(
                    registry.for_project_authored(authorization.scope)?,
                )),
                None => None,
            },
        ),
        None => (root.clone(), state.evaluations.clone()),
    };
    Ok(LabWrite {
        labs: Labs {
            registry,
            evaluations,
        },
        authorization,
    })
}

impl FromRequestParts<AppState> for LabRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.labs))
    }
}

impl FromRequestParts<AppState> for LabWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
