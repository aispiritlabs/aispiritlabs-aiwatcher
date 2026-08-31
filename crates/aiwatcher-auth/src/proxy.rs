//! Identity asserted by an authenticating reverse proxy.
//!
//! This is the mode that costs nothing to adopt where authentik is already in
//! front of aiwatcher. planner's ingress does exactly that today: two Traefik
//! middlewares, the second of which is a forward-auth to authentik's embedded
//! outpost, so nobody reaches the panel without a session — and aiwatcher has
//! no idea who they are. The outpost is already setting `X-authentik-username`
//! and friends on every request it lets through; `proxy` mode is reading them.
//!
//! ## What makes this safe, and what makes it not
//!
//! A header is a claim by whoever sent the request. Trusting it is only sound
//! when nothing can reach this port except the proxy — which is a *network*
//! property, not one this crate can check. So the mode is explicit, never a
//! fallback, and the chart refuses to render `auth.mode: proxy` without a
//! NetworkPolicy fencing the server (see `deploy/helm/aiwatcher`). A missing
//! header is a 401 rather than an anonymous request, for the same reason: in
//! this mode, absence means the request did not come through the proxy.

use serde::Deserialize;

use crate::error::{AuthError, AuthResult};
use crate::identity::{Credential, Identity, RoleMapping};

/// Which headers carry the identity.
///
/// Defaults are authentik's. Configurable because oauth2-proxy, Pomerium and
/// Cloudflare Access each spell the same four things differently, and a
/// deployment already running one of those should not have to add authentik to
/// use this mode.
#[derive(Clone, Debug, Deserialize)]
pub struct ProxyHeaders {
    pub subject: String,
    pub username: String,
    pub email: String,
    pub name: String,
    pub groups: String,
}

impl Default for ProxyHeaders {
    fn default() -> Self {
        Self {
            // authentik's outpost sends a stable uid *and* a username. The uid
            // is the subject because usernames are renamed and a rename must
            // not read as a different person in an audit log.
            subject: "x-authentik-uid".to_owned(),
            username: "x-authentik-username".to_owned(),
            email: "x-authentik-email".to_owned(),
            name: "x-authentik-name".to_owned(),
            groups: "x-authentik-groups".to_owned(),
        }
    }
}

/// Build an identity from whatever the proxy set.
///
/// `header` is a lookup rather than a map so the caller can hand over axum's
/// `HeaderMap` without this crate depending on http.
pub fn identity_from<'a>(
    headers: &ProxyHeaders,
    roles: &RoleMapping,
    header: impl Fn(&str) -> Option<&'a str>,
) -> AuthResult<Identity> {
    let username = header(&headers.username)
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let subject = header(&headers.subject)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(username)
        .ok_or_else(|| AuthError::ProxyIdentityMissing {
            header: headers.username.clone(),
        })?;

    let groups = split_groups(header(&headers.groups).unwrap_or_default());
    let roles = roles.resolve(subject, &groups)?;

    Ok(Identity {
        subject: subject.to_owned(),
        username: username.map(ToOwned::to_owned),
        name: header(&headers.name)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned),
        email: header(&headers.email)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned),
        groups,
        roles,
        // No expiry of our own. The proxy decides when the session ends, and
        // the next request simply does not carry the headers.
        expires_at: None,
        credential: Credential::Proxy,
    })
}

/// authentik joins groups with `|`; oauth2-proxy uses `,`. Both, plus the
/// no-groups case, which is a header set to the empty string rather than an
/// absent one.
fn split_groups(raw: &str) -> Vec<String> {
    raw.split(['|', ','])
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Role;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> + 'a {
        move |wanted| {
            pairs
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, value)| *value)
        }
    }

    #[test]
    fn an_authentik_outposts_headers_become_an_identity() {
        let identity = identity_from(
            &ProxyHeaders::default(),
            &RoleMapping::default(),
            lookup(&[
                ("x-authentik-uid", "0f2e"),
                ("x-authentik-username", "alice"),
                ("x-authentik-email", "alice@example.test"),
                ("x-authentik-name", "Alice Example"),
                ("x-authentik-groups", "everyone|aiwatcher-admins"),
            ]),
        )
        .expect("an identity");

        assert_eq!(identity.subject, "0f2e", "the uid, not the username");
        assert_eq!(identity.username.as_deref(), Some("alice"));
        assert_eq!(identity.groups.len(), 2);
        assert!(identity.can(Role::Admin));
        assert_eq!(identity.credential, Credential::Proxy);
    }

    #[test]
    fn no_identity_header_is_a_refusal_rather_than_an_anonymous_caller() {
        // In this mode the absence of the header means the request did not
        // come through the proxy, which is the one case that must never be
        // read as "nobody is signed in, carry on".
        let error = identity_from(
            &ProxyHeaders::default(),
            &RoleMapping::default(),
            lookup(&[("x-authentik-groups", "everyone")]),
        )
        .expect_err("refused");
        assert!(matches!(error, AuthError::ProxyIdentityMissing { .. }));
    }

    #[test]
    fn a_proxy_that_sends_only_a_username_still_authenticates() {
        let identity = identity_from(
            &ProxyHeaders::default(),
            &RoleMapping::default(),
            lookup(&[("x-authentik-username", "alice")]),
        )
        .expect("an identity");
        assert_eq!(identity.subject, "alice");
        assert_eq!(identity.role(), Role::Viewer);
    }

    #[test]
    fn groups_split_on_either_separator_in_use() {
        assert_eq!(split_groups("a|b").len(), 2);
        assert_eq!(split_groups("a, b").len(), 2);
        assert!(split_groups("").is_empty());
    }
}
