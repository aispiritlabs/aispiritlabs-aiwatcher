//! Which side of the project boundary a read or a command about one execution
//! answers on.
//!
//! The authored registries resolve a store per project and `run_scope` resolves
//! a *side* of one fold; this resolves a **bound handler** — the same workflow
//! store, narrowed to one project (ADR_0033). A project's run is invisible to
//! the unscoped handler by construction, so the instance's own routes answer
//! 404 for it and this is what a project's own routes answer from.
//!
//! The split between what is served twice and what is not is the same one
//! `RuntimeKind::outside_a_project` makes: a person's reads and the four
//! commands they may send have a project form; the hosted decider's lease, its
//! stream append, its timers and its payloads do not, because those belong to a
//! worker and a worker's credential names queues rather than a project.

use std::sync::Arc;

use aiwatcher_execution::{ExecutionHandler, WorkflowStore};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiResult;
use crate::project_scope::ProjectAuthorization;
use crate::state::AppState;
use crate::{ApiError, Caller};

/// One execution's store, on the side the caller was admitted to.
pub(crate) struct RunHandle {
    handler: Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>,
    authorization: Option<ProjectAuthorization>,
}

impl RunHandle {
    pub(crate) fn handler(&self) -> &ExecutionHandler<Arc<dyn WorkflowStore>> {
        &self.handler
    }

    /// The role this caller needs for a command, taken on the right side.
    ///
    /// On the instance's own routes it is the instance's `editor`, as it has
    /// always been. On a project's it is the grant, asked **again** here —
    /// after the run has been read and before anything is written — which is
    /// the second of ADR_0033's two asks.
    ///
    /// # Errors
    ///
    /// Whatever the role check or IAM refused.
    pub(crate) async fn authorize_command(&self, caller: &Caller) -> ApiResult<String> {
        match &self.authorization {
            Some(authorization) => {
                authorization
                    .clone()
                    .needing(aiwatcher_iam::ProjectRole::Editor)
                    .authorize_write()
                    .await?;
                Ok(caller.identity().log_subject().to_owned())
            }
            None => Ok(caller
                .require(aiwatcher_auth::Role::Editor)?
                .log_subject()
                .to_owned()),
        }
    }
}

impl FromRequestParts<AppState> for RunHandle {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        // `Viewer`, because a read of one run is a read: a command re-asks for
        // `Editor` through `authorize_command`, once it knows the run exists.
        let authorization = crate::project_scope::resolve_required(
            parts,
            state,
            aiwatcher_iam::ProjectRole::Viewer,
        )
        .await?;
        let root = state
            .executions
            .as_ref()
            .ok_or(ApiError::ExecutionsDisabled)?;
        let Some(authorization) = authorization else {
            return Ok(Self {
                handler: Arc::clone(root),
                authorization: None,
            });
        };
        // A refusal here is the boundary rather than a bad moment — the store
        // is bound elsewhere and will be on the next request too — so it reads
        // as the 404 every other scope refusal does.
        let store = root
            .store()
            .for_project(authorization.scope)
            .map_err(aiwatcher_execution::HandleError::Store)?;
        Ok(Self {
            handler: Arc::new(ExecutionHandler::new(store)),
            authorization: Some(authorization),
        })
    }
}
