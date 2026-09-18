//! Which project an event belongs to, on the wire and in the record.
//!
//! Two uuids and nothing else. The *control plane's* scope —
//! `aiwatcher_iam::ProjectScope`, which knows what an organization is, who is
//! in it and what a grant means — cannot be this type, because
//! [`aiwatcher-core`](crate) takes no dependency on a store and
//! `aiwatcher-iam` is one. So the log carries the pair of ids and nothing
//! about them, and the one place the two meet is the ingest route, which sees
//! both crates ([`crate::envelope::EventEnvelope::project`]).
//!
//! **Absence is the global side.** Every event this build has ever written has
//! no scope, so nothing existing moves, and a reader that finds none is
//! reading the unscoped log it has always read (ADR_0033 pt. 3).
//!
//! The scope is **never** part of a derived id. `TraceId::derive` and
//! `SpanId::derive` stay pure functions of `run_id` and the span key, so a
//! historical trace keeps addressing the same span and a redelivery still
//! lands on it (ADR_0001, ADR_0033 pt. 5 — which folds the scope into an
//! *execution* id, a different identifier with a different job).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One project of one organization, as the log carries it.
///
/// `Copy`, because it travels beside every event and a clone per row would be
/// two allocations for thirty-two bytes.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
pub struct ProjectScope {
    // `value_type` and no `format`: the `Uuid` format needs utoipa's `uuid`
    // feature, which this crate would then carry for two strings in a contract
    // that already describes a hundred ids as plain strings.
    #[schema(value_type = String)]
    pub organization: Uuid,
    #[schema(value_type = String)]
    pub project: Uuid,
}

impl ProjectScope {
    #[must_use]
    pub const fn new(organization: Uuid, project: Uuid) -> Self {
        Self {
            organization,
            project,
        }
    }

    /// What a fold keys by, a filter compares and a log line prints:
    /// `<organization>/<project>`.
    ///
    /// Two uuids, which is what makes it safe to build by concatenation —
    /// neither half can hold the separator, so no pair of scopes produces one
    /// key. The same string `aiwatcher_execution::ExecutionScope::key` writes,
    /// deliberately: one scope, one spelling, wherever it is read.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.organization, self.project)
    }

    /// The key of a scope that may be absent — the empty string for the
    /// global side.
    ///
    /// A free function rather than a method on `Option`, so a fold keying
    /// rows by scope has one call and no `map_or_else` at every site.
    #[must_use]
    pub fn key_of(scope: Option<Self>) -> String {
        scope.as_ref().map(Self::key).unwrap_or_default()
    }

    /// The sentence a refusal or a log line names this scope with.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "project {} of organization {}",
            self.project, self.organization
        )
    }

    /// Read `<organization-uuid>/<project-uuid>`.
    ///
    /// The one text form, shared by the ingest token's configuration and by
    /// anything that has to print a scope and read it back.
    pub fn parse(text: &str) -> Result<Self, ScopeError> {
        let (organization, project) = text.trim().split_once('/').ok_or_else(|| ScopeError {
            text: text.to_owned(),
        })?;
        Ok(Self {
            organization: Uuid::parse_str(organization.trim()).map_err(|_| ScopeError {
                text: text.to_owned(),
            })?,
            project: Uuid::parse_str(project.trim()).map_err(|_| ScopeError {
                text: text.to_owned(),
            })?,
        })
    }
}

impl std::fmt::Display for ProjectScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key())
    }
}

impl std::str::FromStr for ProjectScope {
    type Err = ScopeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// Text that is not `<organization-uuid>/<project-uuid>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{text:?} is not a project scope; write it as <organization-uuid>/<project-uuid>")]
pub struct ScopeError {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ProjectScope {
        ProjectScope::new(
            Uuid::parse_str("0198c0de-0000-7000-8000-000000000001").expect("uuid"),
            Uuid::parse_str("0198c0de-0000-7000-8000-000000000002").expect("uuid"),
        )
    }

    #[test]
    fn a_scope_reads_back_from_what_it_prints() {
        assert_eq!(ProjectScope::parse(&scope().key()), Ok(scope()));
        assert_eq!(scope().to_string(), scope().key());
    }

    #[test]
    fn the_global_side_has_the_empty_key() {
        assert_eq!(ProjectScope::key_of(None), "");
        assert_eq!(ProjectScope::key_of(Some(scope())), scope().key());
    }

    #[test]
    fn two_projects_of_one_organization_never_share_a_key() {
        let other = ProjectScope::new(scope().organization, Uuid::now_v7());
        assert_ne!(scope().key(), other.key());
        // …and neither does a project whose id another organization reuses,
        // which is the pair a key built by concatenation has to keep apart.
        let elsewhere = ProjectScope::new(Uuid::now_v7(), scope().project);
        assert_ne!(scope().key(), elsewhere.key());
    }

    #[test]
    fn text_that_is_not_two_uuids_is_refused_by_name() {
        for broken in [
            "",
            "not-a-uuid/also-not",
            "0198c0de-0000-7000-8000-000000000001",
            "/",
        ] {
            let error = ProjectScope::parse(broken).expect_err(broken);
            assert!(
                error
                    .to_string()
                    .contains("<organization-uuid>/<project-uuid>"),
                "{broken}: {error}"
            );
        }
    }

    #[test]
    fn a_scope_survives_json_as_the_bus_carries_it() {
        let json = serde_json::to_string(&scope()).expect("serialises");
        assert_eq!(
            serde_json::from_str::<ProjectScope>(&json).expect("reads back"),
            scope()
        );
    }
}
