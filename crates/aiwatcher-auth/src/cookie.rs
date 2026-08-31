//! Cookies, as values rather than as strings.
//!
//! The session travels in a cookie rather than in a header for one reason that
//! decides the whole design: `EventSource` and `WebSocket` cannot set request
//! headers. The live stream is not an optional extra here — it is what the
//! panel is for — so a bearer token in JavaScript would leave the two most
//! important routes needing the token in a query string, which is the one
//! place a credential ends up in an access log. A cookie the browser attaches
//! on its own works identically for `fetch`, for SSE and for the WebSocket,
//! and never reaches JavaScript at all.

use std::fmt;

/// A cookie this server sets.
#[derive(Clone, Debug)]
pub struct CookieSpec {
    pub name: String,
    pub value: String,
    /// `None` is a session cookie the browser drops when it closes; `Some(0)`
    /// deletes one.
    pub max_age: Option<i64>,
    pub path: String,
    pub domain: Option<String>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: SameSite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    /// What both cookies here use. `Strict` would drop the session on the
    /// redirect back from the identity provider — a top-level navigation from
    /// another site is exactly what a login *is* — so the sign-in would loop
    /// forever with no error anywhere.
    Lax,
    Strict,
    /// Only ever with `Secure`. Not used here; present so a deployment that
    /// embeds the panel in an iframe has a name to reach for.
    None,
}

impl fmt::Display for SameSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lax => "Lax",
            Self::Strict => "Strict",
            Self::None => "None",
        })
    }
}

impl CookieSpec {
    /// A cookie carrying a value, with this crate's defaults.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            max_age: None,
            path: "/".to_owned(),
            domain: None,
            http_only: true,
            secure: true,
            same_site: SameSite::Lax,
        }
    }

    /// The same cookie, emptied and expired.
    ///
    /// Every attribute that scopes a cookie has to match the one that set it,
    /// or the browser deletes nothing and keeps the original — a sign-out that
    /// silently does not.
    #[must_use]
    pub fn removal(&self) -> Self {
        Self {
            value: String::new(),
            max_age: Some(0),
            ..self.clone()
        }
    }

    #[must_use]
    pub fn with_max_age(mut self, seconds: i64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    #[must_use]
    pub fn with_domain(mut self, domain: Option<String>) -> Self {
        self.domain = domain;
        self
    }
}

impl fmt::Display for CookieSpec {
    /// Renders the `Set-Cookie` header value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}; Path={}", self.name, self.value, self.path)?;
        if let Some(domain) = &self.domain {
            write!(f, "; Domain={domain}")?;
        }
        if let Some(max_age) = self.max_age {
            write!(f, "; Max-Age={max_age}")?;
        }
        if self.http_only {
            f.write_str("; HttpOnly")?;
        }
        if self.secure {
            f.write_str("; Secure")?;
        }
        write!(f, "; SameSite={}", self.same_site)
    }
}

/// Find one cookie in a `Cookie:` header value.
///
/// Hand-rolled rather than a crate: this reads one name out of a
/// semicolon-separated list, and the parsing that a cookie library exists for
/// — attributes, quoting, encoding — is on the *response* side, which is
/// [`CookieSpec`] above and is ours to write.
#[must_use]
pub fn read<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_cookie_carries_every_attribute_that_protects_it() {
        let rendered = CookieSpec::new("aiwatcher_session", "abc")
            .with_max_age(3600)
            .to_string();
        assert!(rendered.contains("aiwatcher_session=abc"));
        assert!(rendered.contains("HttpOnly"), "must not reach JavaScript");
        assert!(rendered.contains("Secure"));
        // Lax, not Strict: the redirect back from the provider is a top-level
        // navigation from another site, and Strict would drop the cookie on it.
        assert!(rendered.contains("SameSite=Lax"));
        assert!(rendered.contains("Max-Age=3600"));
    }

    #[test]
    fn a_removal_keeps_the_scope_of_what_it_removes() {
        let cookie = CookieSpec::new("aiwatcher_session", "abc")
            .with_domain(Some("example.test".to_owned()));
        let removal = cookie.removal();
        assert_eq!(removal.max_age, Some(0));
        assert_eq!(removal.path, cookie.path);
        assert_eq!(removal.domain, cookie.domain);
        assert!(removal.value.is_empty());
    }

    #[test]
    fn one_cookie_is_read_out_of_a_header_holding_several() {
        let header = "theme=dark; aiwatcher_session=abc.def; other=1";
        assert_eq!(read(header, "aiwatcher_session"), Some("abc.def"));
        assert_eq!(read(header, "missing"), None);
    }

    #[test]
    fn a_cookie_whose_name_is_a_prefix_of_another_is_not_confused_for_it() {
        let header = "aiwatcher_session_old=stale; aiwatcher_session=fresh";
        assert_eq!(read(header, "aiwatcher_session"), Some("fresh"));
    }
}
