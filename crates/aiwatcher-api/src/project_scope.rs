//! Shared admission and contract for project resource routes.
use crate::{ApiError, AppState, Caller, error::ApiResult};
use aiwatcher_iam::{IamStore, Principal, ProjectRole, ProjectScope};
use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct ScopedRoute;

/// The control plane's scope, as the event log carries it.
///
/// Two types for one idea, and the crate boundary is the whole reason:
/// `aiwatcher-core` takes no dependency on a store and `aiwatcher-iam` is one
/// (`scripts/rust-boundaries.json`), so the log's form holds two uuids and
/// knows nothing about organizations, teams or grants. The conversion lives
/// with the control plane's type, because the outbox publisher needs it too
/// and a second copy is a second chance to spell it the other way round; what
/// stays here is the name this crate's routes read and
/// `the_two_scopes_spell_one_project_the_same_way` below, which holds all
/// three spellings to one key.
pub(crate) fn on_the_log(scope: ProjectScope) -> aiwatcher_core::ProjectScope {
    scope.on_the_log()
}

pub(crate) struct ProjectAuthorization {
    store: Arc<dyn IamStore>,
    principal: Principal,
    required_role: ProjectRole,
    pub(crate) scope: ProjectScope,
}
impl ProjectAuthorization {
    pub(crate) async fn authorize_write(&self) -> ApiResult<()> {
        self.authorize_write_role().await.map(|_| ())
    }

    pub(crate) async fn authorize_write_role(&self) -> ApiResult<ProjectRole> {
        let role = self.store.access(self.scope, &self.principal).await?.role;
        if role < self.required_role {
            return Err(ApiError::IamForbidden);
        }
        Ok(role)
    }
}

/// None means a legacy route with instance authorization, never a fallback
/// from a scoped request. Repeat write authorization after receiving its body.
pub(crate) async fn resolve(
    parts: &mut Parts,
    state: &AppState,
    write: bool,
) -> ApiResult<Option<ProjectAuthorization>> {
    resolve_required(
        parts,
        state,
        if write {
            ProjectRole::Editor
        } else {
            ProjectRole::Viewer
        },
    )
    .await
}

/// Select the operation's role once, then retain it for the post-upload check.
pub(crate) async fn resolve_required(
    parts: &mut Parts,
    state: &AppState,
    required_role: ProjectRole,
) -> ApiResult<Option<ProjectAuthorization>> {
    let write = required_role > ProjectRole::Viewer;
    let caller = Caller::from_request_parts(parts, state).await?;
    if parts.extensions.get::<ScopedRoute>().is_none() {
        if write {
            caller.require(match required_role {
                ProjectRole::Admin => aiwatcher_auth::Role::Admin,
                _ => aiwatcher_auth::Role::Editor,
            })?;
        }
        return Ok(None);
    }
    let (store, principal) = crate::iam::context(state, &caller)?;
    let Path(scope) = Path::<ProjectScope>::from_request_parts(parts, state)
        .await
        .map_err(|_| ApiError::BadRequest("invalid organization/project scope".into()))?;
    if write {
        crate::iam::mutation(&parts.headers)?;
    }
    let access = store.access(scope, &principal).await?;
    if access.role < required_role {
        return Err(ApiError::IamForbidden);
    }
    Ok(Some(ProjectAuthorization {
        store: state.iam.clone().ok_or(ApiError::IamDisabled)?,
        principal,
        required_role,
        scope,
    }))
}

pub(crate) fn openapi(mut api: utoipa::openapi::OpenApi) -> utoipa::openapi::OpenApi {
    // Both route families execute these same handlers/extractors. Derive the
    // scoped contract from the legacy operations so their bodies cannot drift.
    for (path, mut item) in api.paths.paths.clone() {
        for (operation, write) in [
            (&mut item.get, false),
            (&mut item.post, true),
            (&mut item.put, true),
            (&mut item.delete, true),
        ] {
            let Some(operation) = operation else { continue };
            operation.operation_id = operation
                .operation_id
                .take()
                .map(|id| format!("project_{id}"));
            let parameters = operation.parameters.get_or_insert_with(Vec::new);
            for name in ["organization", "project"] {
                parameters.push(
                    utoipa::openapi::path::ParameterBuilder::new()
                        .name(name)
                        .parameter_in(utoipa::openapi::path::ParameterIn::Path)
                        .required(utoipa::openapi::Required::True)
                        .schema(Some(
                            utoipa::openapi::ObjectBuilder::new()
                                .schema_type(utoipa::openapi::Type::String)
                                .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                                    utoipa::openapi::KnownFormat::Uuid,
                                ))),
                        ))
                        .build(),
                );
            }
            if write {
                parameters.push(
                    utoipa::openapi::path::ParameterBuilder::new()
                        .name("X-AIWatcher-IAM")
                        .parameter_in(utoipa::openapi::path::ParameterIn::Header)
                        .required(utoipa::openapi::Required::True)
                        .description(Some("Required value: 1"))
                        .schema(Some(
                            utoipa::openapi::ObjectBuilder::new()
                                .schema_type(utoipa::openapi::Type::String),
                        ))
                        .build(),
                );
            }
            for status in ["400", "401", "403", "404", "503"] {
                operation
                    .responses
                    .responses
                    .entry(status.into())
                    .or_insert_with(|| {
                        utoipa::openapi::ResponseBuilder::new()
                            .description("Current project authorization failed or is unavailable")
                            .build()
                            .into()
                    });
            }
        }
        api.paths.paths.insert(
            path.replacen(
                "/api/v1",
                "/api/v1/orgs/{organization}/projects/{project}",
                1,
            ),
            item,
        );
    }
    api
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_scopes_spell_one_project_the_same_way() {
        let scope = ProjectScope {
            organization: aiwatcher_iam::OrganizationId(uuid::Uuid::now_v7()),
            project: aiwatcher_iam::ProjectId(uuid::Uuid::now_v7()),
        };

        // Two types the crate boundary keeps apart, one key. A fold that groups
        // rows by the log's spelling and a grant check that looks a project up
        // by the control plane's must be asking about the same project, and
        // this is the only place that can say so.
        assert_eq!(on_the_log(scope).key(), scope.key());
        assert_eq!(
            aiwatcher_core::ProjectScope::parse(&scope.key()),
            Ok(on_the_log(scope))
        );
        assert_eq!(
            ProjectScope::parse(&on_the_log(scope).key()).expect("reads back"),
            scope
        );
    }
}
