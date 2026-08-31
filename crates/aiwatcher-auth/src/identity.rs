//! Who is asking, and what that lets them do.
//!
//! An [`Identity`] is what every authenticated request carries. It is
//! deliberately small — a subject, a few display fields, the groups the
//! provider asserted and the [`Role`]s those groups resolved to — because it
//! travels in a cookie and is read on every request.
//!
//! The provider decides *who*; this crate decides *what they may do*. Keeping
//! that split means an operator adds somebody to a group in authentik and
//! nothing here changes, and it means a provider that starts asserting a new
//! group grants nothing until the mapping names it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// What a caller is allowed to do.
///
/// Three, not one per route. The distinction that matters is not which
/// endpoint but which *kind* of thing an endpoint does: read what happened,
/// author something that outlives the log, or ask another system to run work.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Everything that reads: runs, spans, dimensions, metrics, evaluations,
    /// the live stream, and the prompt registry's `GET` routes.
    #[default]
    Viewer,
    /// Writes that stay inside aiwatcher: publishing a prompt version, moving
    /// a label, recording an optimisation, and HTTP ingest.
    Editor,
    /// The one thing that reaches out of aiwatcher — dispatching a rerun to
    /// the configured orchestrator.
    Admin,
}

impl Role {
    /// Whether holding `self` satisfies a requirement for `needed`.
    ///
    /// Ordered rather than a set of independent capabilities: an admin who
    /// could not read the run they were about to rerun would be a permission
    /// model nobody could configure correctly.
    #[must_use]
    pub fn satisfies(self, needed: Self) -> bool {
        self >= needed
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = UnknownRole;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "viewer" | "reader" | "read" => Ok(Self::Viewer),
            "editor" | "writer" | "write" => Ok(Self::Editor),
            "admin" | "administrator" | "owner" => Ok(Self::Admin),
            other => Err(UnknownRole(other.to_owned())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a role; expected one of viewer, editor, admin")]
pub struct UnknownRole(pub String);

/// How the caller proved who they are.
///
/// Kept on the identity because the three are not interchangeable when
/// something goes wrong: a session is a browser, a bearer is a machine, and a
/// proxy header is a statement by whatever sits in front of this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Credential {
    /// The session cookie this server issued after a login.
    Session,
    /// An `Authorization: Bearer` token, verified against the provider's JWKS.
    /// What a machine client uses — an authentik service account, or anything
    /// holding a client-credentials token for this audience.
    Bearer,
    /// A shared secret from `AIWATCHER_AUTH_INGEST_TOKENS`. What a producer
    /// uses: an agent posting to `/api/v1/events` reaches the Service
    /// directly, so it never passes whatever authenticates a browser, and it
    /// cannot complete an interactive sign-in either.
    Token,
    /// Headers set by an authenticating reverse proxy. Only trusted in
    /// `proxy` mode, and only because the deployment guarantees nothing else
    /// can reach this port.
    Proxy,
    /// No provider is configured. Every caller is this, and every role check
    /// passes — which is what `AIWATCHER_AUTH_MODE=none` means.
    Anonymous,
}

/// An authenticated caller.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Identity {
    /// The provider's stable identifier — `sub` on an OIDC token. Never the
    /// email, which people change.
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// What the provider asserted, verbatim. Kept beside the resolved roles so
    /// an operator debugging "why is this person only a viewer" can see the
    /// input to the mapping rather than guessing at it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// The roles the groups above resolved to, highest last.
    pub roles: Vec<Role>,
    /// When this identity stops being valid, as a Unix timestamp. `None` for
    /// the anonymous identity, which never expires because it was never
    /// issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub credential: Credential,
}

impl Identity {
    /// The caller when no provider is configured.
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            subject: "anonymous".to_owned(),
            username: None,
            name: None,
            email: None,
            groups: Vec::new(),
            roles: vec![Role::Admin],
            expires_at: None,
            credential: Credential::Anonymous,
        }
    }

    /// The highest role this identity holds.
    #[must_use]
    pub fn role(&self) -> Role {
        self.roles.iter().copied().max().unwrap_or(Role::Viewer)
    }

    #[must_use]
    pub fn can(&self, needed: Role) -> bool {
        self.role().satisfies(needed)
    }

    /// What to put in a log line. Never the email or the group list: an access
    /// log is read by more people than the identity provider's user list is.
    #[must_use]
    pub fn log_subject(&self) -> &str {
        self.username.as_deref().unwrap_or(&self.subject)
    }
}

/// Which authentik groups mean which role.
///
/// Every field is a list because a provider can name the same idea more than
/// once — an `aiwatcher-admins` group in one environment and a
/// `platform-owners` group in another, both meaning admin.
#[derive(Clone, Debug)]
pub struct RoleMapping {
    pub admin_groups: Vec<String>,
    pub editor_groups: Vec<String>,
    pub viewer_groups: Vec<String>,
    /// When non-empty, a caller in none of these groups is refused outright
    /// rather than given `default_role`. This is the "who may sign in at all"
    /// switch, separate from "what may they do once they have".
    pub required_groups: Vec<String>,
    /// What an authenticated caller in none of the mapped groups gets.
    /// `Some(Viewer)` by default: an identity provider that let somebody
    /// through has already made the "may this person see aiwatcher" decision,
    /// and a login that succeeds and then shows nothing reads as a broken
    /// deployment. `None` refuses them instead.
    pub default_role: Option<Role>,
}

impl Default for RoleMapping {
    fn default() -> Self {
        Self {
            admin_groups: vec!["aiwatcher-admins".to_owned()],
            editor_groups: vec!["aiwatcher-editors".to_owned()],
            viewer_groups: vec!["aiwatcher-viewers".to_owned()],
            required_groups: Vec::new(),
            default_role: Some(Role::Viewer),
        }
    }
}

/// A caller the mapping refuses.
#[derive(Debug, thiserror::Error)]
#[error("{subject} is in no group this deployment grants a role to")]
pub struct NotEntitled {
    pub subject: String,
}

impl RoleMapping {
    /// Resolve asserted groups into roles.
    ///
    /// Group names are compared case-insensitively. Authentik's own names are
    /// case-sensitive, but the failure mode of an exact match is silent — a
    /// user who should be an admin quietly becomes a viewer — while the
    /// failure mode of a loose match needs two groups differing only by case,
    /// which nobody creates on purpose.
    pub fn resolve(&self, subject: &str, groups: &[String]) -> Result<Vec<Role>, NotEntitled> {
        if !self.required_groups.is_empty() && !any_match(&self.required_groups, groups) {
            return Err(NotEntitled {
                subject: subject.to_owned(),
            });
        }

        let mut roles = BTreeSet::new();
        if any_match(&self.viewer_groups, groups) {
            roles.insert(Role::Viewer);
        }
        if any_match(&self.editor_groups, groups) {
            roles.insert(Role::Editor);
        }
        if any_match(&self.admin_groups, groups) {
            roles.insert(Role::Admin);
        }

        if roles.is_empty() {
            match self.default_role {
                Some(role) => {
                    roles.insert(role);
                }
                None => {
                    return Err(NotEntitled {
                        subject: subject.to_owned(),
                    });
                }
            }
        }

        Ok(roles.into_iter().collect())
    }
}

fn any_match(configured: &[String], asserted: &[String]) -> bool {
    configured.iter().any(|wanted| {
        asserted
            .iter()
            .any(|held| held.trim().eq_ignore_ascii_case(wanted.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_admin_may_do_everything_an_editor_and_a_viewer_may() {
        assert!(Role::Admin.satisfies(Role::Viewer));
        assert!(Role::Admin.satisfies(Role::Editor));
        assert!(Role::Editor.satisfies(Role::Viewer));
        assert!(!Role::Viewer.satisfies(Role::Editor));
        assert!(!Role::Editor.satisfies(Role::Admin));
    }

    #[test]
    fn a_signed_in_user_in_no_mapped_group_can_still_read() {
        // The provider already decided this person may reach aiwatcher. A
        // login that succeeds and then shows nothing reads as broken software.
        let mapping = RoleMapping::default();
        let roles = mapping
            .resolve("alice", &groups(&["everyone"]))
            .expect("ok");
        assert_eq!(roles, vec![Role::Viewer]);
    }

    #[test]
    fn group_names_match_regardless_of_case() {
        let mapping = RoleMapping::default();
        let roles = mapping
            .resolve("alice", &groups(&["AIWatcher-Admins"]))
            .expect("ok");
        assert!(roles.contains(&Role::Admin));
    }

    #[test]
    fn a_required_group_refuses_everyone_outside_it() {
        let mapping = RoleMapping {
            required_groups: groups(&["planner-staff"]),
            ..RoleMapping::default()
        };
        assert!(mapping.resolve("mallory", &groups(&["everyone"])).is_err());
        assert!(
            mapping
                .resolve("alice", &groups(&["planner-staff"]))
                .is_ok()
        );
    }

    #[test]
    fn refusing_the_default_role_refuses_an_unmapped_user() {
        let mapping = RoleMapping {
            default_role: None,
            ..RoleMapping::default()
        };
        assert!(mapping.resolve("alice", &groups(&["everyone"])).is_err());
    }

    #[test]
    fn the_highest_role_is_the_one_that_answers_a_check() {
        let identity = Identity {
            roles: vec![Role::Viewer, Role::Admin],
            ..Identity::anonymous()
        };
        assert_eq!(identity.role(), Role::Admin);
        assert!(identity.can(Role::Editor));
    }

    #[test]
    fn an_anonymous_caller_passes_every_check() {
        // What AIWATCHER_AUTH_MODE=none means: there is no identity to check,
        // so refusing on one would be inventing a rule nobody configured.
        let identity = Identity::anonymous();
        assert!(identity.can(Role::Admin));
        assert_eq!(identity.credential, Credential::Anonymous);
    }
}
