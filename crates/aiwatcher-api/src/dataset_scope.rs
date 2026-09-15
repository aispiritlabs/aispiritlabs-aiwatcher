//! Select the registry before a handler can read or write anything. A scoped
//! route always requires IAM; it never falls back to instance permissions.
use crate::{ApiError, AppState, Caller, error::ApiResult};
use aiwatcher_datasets::Registry;
use aiwatcher_iam::{IamStore, Principal, ProjectRole, ProjectScope};
use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct ScopedRoute;
pub(crate) struct DatasetRead(pub Arc<Registry>);
pub(crate) struct DatasetWrite {
    registry: Arc<Registry>,
    authorization: Option<(Arc<dyn IamStore>, Principal, ProjectScope)>,
}
impl DatasetWrite {
    /// JSON extraction can wait on a slow upload. Recheck after receiving the
    /// body so an expired/revoked grant cannot authorize that delayed write.
    pub(crate) async fn authorize(self) -> ApiResult<Arc<Registry>> {
        if let Some((store, principal, scope)) = self.authorization
            && store.access(scope, &principal).await?.role < ProjectRole::Editor
        {
            return Err(ApiError::IamForbidden);
        }
        Ok(self.registry)
    }
}

async fn resolve(parts: &mut Parts, state: &AppState, write: bool) -> ApiResult<DatasetWrite> {
    let caller = Caller::from_request_parts(parts, state).await?;
    if parts.extensions.get::<ScopedRoute>().is_some() {
        let (store, principal) = crate::iam::context(state, &caller)?;
        let Path(scope) = Path::<ProjectScope>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::BadRequest("invalid organization/project scope".into()))?;
        if write {
            crate::iam::mutation(&parts.headers)?;
        }
        let access = store.access(scope, &principal).await?;
        if write && access.role < ProjectRole::Editor {
            return Err(ApiError::IamForbidden);
        }
        let root = state
            .datasets
            .as_ref()
            .ok_or(ApiError::DatasetRegistryDisabled)?;
        return Ok(DatasetWrite {
            registry: Arc::new(root.for_project(scope)?),
            authorization: Some((
                state.iam.clone().ok_or(ApiError::IamDisabled)?,
                principal,
                scope,
            )),
        });
    }
    if write {
        caller.require(aiwatcher_auth::Role::Editor)?;
    }
    Ok(DatasetWrite {
        registry: state
            .datasets
            .clone()
            .ok_or(ApiError::DatasetRegistryDisabled)?,
        authorization: None,
    })
}

impl FromRequestParts<AppState> for DatasetRead {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, false)
            .await
            .map(|resolved| Self(resolved.registry))
    }
}
impl FromRequestParts<AppState> for DatasetWrite {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        resolve(parts, state, true).await
    }
}
