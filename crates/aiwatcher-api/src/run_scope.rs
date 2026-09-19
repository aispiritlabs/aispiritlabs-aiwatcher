//! Which side of the project boundary a read of the log's folds answers on.
//!
//! The authored registries resolve a *store* per project — one prefix per
//! scope, `Registry::for_project` (ADR_0033). There is no second store to
//! resolve here: the log is one log and the fold is one fold with the project
//! in the row (IAM-02 E2). So what a route resolves is the **side** it answers
//! on, and it resolves it the same way and in the same place every scoped
//! route does, through [`crate::project_scope::resolve`]:
//!
//! * no `/orgs/{organization}/projects/{project}` in the path is
//!   [`ReadScope::Global`], under instance authorization, exactly as before;
//! * one in the path is that project's side, after a grant asked of IAM **on
//!   this request**. A principal with no live grant is told the project is not
//!   there — `aiwatcher_iam::Error::NotFound`, rendered 404 — which is the
//!   answer `StoreError::OutOfScope` already gives on the other half of the
//!   data plane, and for the same reason: a run somebody may not reach is a run
//!   they do not have.
//!
//! **A stream re-asks.** A list is one request and one decision, and
//! `ProjectAccess` is a snapshot with `evaluated_at` rather than a capability,
//! so the next list asks again. A stream is one request that lives for hours,
//! and until this the session cookie's TTL *was* its revocation window
//! (ADR_0013). [`StillHeld`] is what an open stream asks every
//! [`RECHECK_EVERY`], and what closes it when the answer has changed.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_iam::{IamStore, Principal, ProjectScope};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiResult;
use crate::state::AppState;
use crate::{ApiError, Caller};

/// How often an open stream asks again whether the access that opened it still
/// holds.
///
/// Thirty seconds is one grant lookup per stream per half a minute — an
/// in-process read on the memory store and one indexed row on PostgreSQL —
/// against a revocation that takes effect while the person who revoked it is
/// still looking at the screen. It is not a measurement against a real number
/// of streams, and that is written down in `docs/iam-02-data-plane.md` as the
/// risk it is rather than left to be discovered.
pub(crate) const RECHECK_EVERY: Duration = Duration::from_secs(30);

/// A read of the runs fold, and what it may see.
pub(crate) struct RunRead {
    pub(crate) scope: aiwatcher_projector::ReadScope,
    /// What to ask again while a stream this opened is still running. Unused
    /// by the routes that answer once and close.
    pub(crate) held: StillHeld,
}

/// The access one open stream is running under, in the form it can be re-asked
/// in.
#[derive(Clone)]
pub(crate) enum StillHeld {
    /// Instance authorization, which has nothing per-stream to re-ask: what
    /// bounds it is the session cookie's own TTL, and that is ADR_0013's
    /// decision rather than this one's. Named rather than represented as an
    /// absent grant, so a reader can see that the global side was considered.
    Instance,
    /// One project's grant, plus when the session that opened the stream stops
    /// being valid. Both, because a stream that outlives its own session is
    /// the same complaint as a stream that outlives its own grant.
    Grant {
        store: Arc<dyn IamStore>,
        principal: Box<Principal>,
        scope: ProjectScope,
        expires_at: Option<i64>,
    },
}

/// What asking again answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Still {
    Held,
    /// The grant is gone, or the session that opened the stream has expired.
    Gone,
    /// IAM could not be asked. **Not** an answer, so it does not close the
    /// stream: a store that is briefly unreachable is `Transient`, which is
    /// what `ProjectDispatcher` already treats an IAM failure as. The cost is
    /// named rather than hidden — while IAM is down a grant revoked mid-stream
    /// keeps that one connection alive, and no *new* stream opens, because the
    /// extractor below fails closed on the same error.
    Unknown,
}

impl StillHeld {
    pub(crate) async fn ask(&self, now: i64) -> Still {
        let Self::Grant {
            store,
            principal,
            scope,
            expires_at,
        } = self
        else {
            return Still::Held;
        };
        if expires_at.is_some_and(|expiry| expiry <= now) {
            return Still::Gone;
        }
        match store.access(*scope, principal).await {
            Ok(_) => Still::Held,
            Err(aiwatcher_iam::Error::NotFound | aiwatcher_iam::Error::Forbidden) => Still::Gone,
            Err(error) => {
                tracing::warn!(%error, "a live stream's grant could not be re-read; keeping it open");
                Still::Unknown
            }
        }
    }
}

impl FromRequestParts<AppState> for RunRead {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let expires_at = Caller::from_request_parts(parts, state)
            .await?
            .identity()
            .expires_at;
        let Some(authorization) = crate::project_scope::resolve(parts, state, false).await? else {
            return Ok(Self {
                scope: aiwatcher_projector::ReadScope::Global,
                held: StillHeld::Instance,
            });
        };
        let scope = authorization.scope;
        let store = state.iam.clone().ok_or(ApiError::IamDisabled)?;
        let caller = Caller::from_request_parts(parts, state).await?;
        let (_, principal) = crate::iam::context(state, &caller)?;
        Ok(Self {
            scope: aiwatcher_projector::ReadScope::Project(crate::project_scope::on_the_log(scope)),
            held: StillHeld::Grant {
                store,
                principal: Box::new(principal),
                scope,
                expires_at,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::sync::Arc;

    use aiwatcher_iam::memory::MemoryIamStore;
    use aiwatcher_iam::{Change, Command, GrantWindow, Grantee, IamStore, Principal, ProjectRole};

    use super::{Still, StillHeld};

    const NOW: i64 = 1_700_000_000;

    /// One organization, one project, and a live grant on it for `member`.
    async fn granted() -> (Arc<MemoryIamStore>, Principal, aiwatcher_iam::ProjectScope) {
        let store = Arc::new(MemoryIamStore::default());
        let owner = Principal::new("https://issuer.test", "owner").expect("a principal");
        let member = Principal::new("https://issuer.test", "member").expect("a principal");
        let organization = store
            .create_organization(&owner, "Workshop")
            .await
            .expect("an organization");
        let Change::ProjectCreated(project) = store
            .apply(
                organization.id,
                &owner,
                Command::CreateProject {
                    name: "Lesson".to_owned(),
                },
            )
            .await
            .expect("a project")
        else {
            panic!("expected a project");
        };
        store
            .apply(
                organization.id,
                &owner,
                Command::SetMember {
                    principal: member.clone(),
                    role: aiwatcher_iam::OrganizationRole::Member,
                },
            )
            .await
            .expect("a member");
        store
            .apply(
                organization.id,
                &owner,
                Command::Grant {
                    project: project.scope.project,
                    grantee: Grantee::User(member.clone()),
                    role: ProjectRole::Viewer,
                    window: GrantWindow::permanent(0),
                },
            )
            .await
            .expect("a grant");
        (store, member, project.scope)
    }

    fn held(
        store: Arc<MemoryIamStore>,
        principal: Principal,
        scope: aiwatcher_iam::ProjectScope,
        expires_at: Option<i64>,
    ) -> StillHeld {
        StillHeld::Grant {
            store,
            principal: Box::new(principal),
            scope,
            expires_at,
        }
    }

    #[tokio::test]
    async fn a_revoked_grant_is_the_answer_that_closes_a_stream() {
        let (store, member, scope) = granted().await;
        let guard = held(store.clone(), member.clone(), scope, None);
        assert_eq!(guard.ask(NOW).await, Still::Held);

        let owner = Principal::new("https://issuer.test", "owner").expect("a principal");
        let grants = store
            .project_grants(scope, &owner)
            .await
            .expect("the grants");
        for grant in grants {
            store
                .apply(
                    scope.organization,
                    &owner,
                    Command::RevokeGrant {
                        project: scope.project,
                        grant: grant.id,
                    },
                )
                .await
                .expect("revokes");
        }

        // No sign-out, no expiry, no reconnect: the next tick is the whole
        // revocation window, which before this was the session cookie's TTL.
        assert_eq!(guard.ask(NOW).await, Still::Gone);
    }

    #[tokio::test]
    async fn a_session_that_has_run_out_closes_its_own_stream_with_the_grant_intact() {
        // A stream that outlives its own session is the same complaint as one
        // that outlives its own grant, and the cheaper half to answer: it is
        // read off the identity the stream opened with, with nothing asked of
        // IAM at all.
        let (store, member, scope) = granted().await;
        let guard = held(store, member, scope, Some(NOW - 1));
        assert_eq!(guard.ask(NOW).await, Still::Gone);
    }

    #[tokio::test]
    async fn the_instance_side_has_nothing_per_stream_to_re_ask() {
        // Named rather than represented as an absent grant: what bounds a
        // global stream is the cookie's own TTL, and that is ADR_0013's
        // decision rather than this one's.
        assert_eq!(StillHeld::Instance.ask(NOW).await, Still::Held);
    }
}
