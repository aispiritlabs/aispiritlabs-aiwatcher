//! Where a command goes, and what it presents when it gets there.
//!
//! One file — `~/.config/aiwatcher/config.toml` — holding named profiles and
//! which of them is current. Obsidian's `set-default` for a vault; kubectl's
//! contexts for a cluster; the same idea either way, which is that the address
//! and the credential are a property of *where you are working*, not something
//! to retype on every line.
//!
//! Two things are deliberately not here. A profile never holds a password, and
//! the **local** profile never holds its token: it names the file the token
//! lives in, which is the same file the server reads, so there is exactly one
//! copy of that secret on the machine and rotating it is one write. A remote
//! profile does hold its token, because there is nothing else on this side that
//! could — and the file is written `0600` for that reason.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::CliError;

/// The name a fresh install works under.
pub const LOCAL: &str = "local";

/// The whole file.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    /// Which profile a command uses when it is not told. Absent means
    /// [`LOCAL`], which is what a machine that has only ever run `aiwatcher up`
    /// has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
}

/// One instance this CLI can talk to.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Profile {
    /// Where the API is.
    pub url: String,
    /// The bearer token, for a remote instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The file holding it, for the local one. Read at use rather than at
    /// load, so `aiwatcher token create` takes effect on the next command
    /// without rewriting this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
}

impl Profile {
    /// A profile for an instance on this machine.
    #[must_use]
    pub fn local(url: String, token_file: PathBuf) -> Self {
        Self {
            url,
            token: None,
            token_file: Some(token_file),
        }
    }

    /// The secret to present, from wherever this profile keeps it.
    ///
    /// `None` is an ordinary answer: an instance running `AIWATCHER_AUTH_MODE=none`
    /// wants no credential, and presenting one there would be neither better
    /// nor worse.
    ///
    /// # Errors
    ///
    /// [`CliError::Io`] when the named token file cannot be read — which is
    /// different from there being no token, and is reported rather than
    /// swallowed into an anonymous request that 401s later.
    pub fn secret(&self) -> Result<Option<String>, CliError> {
        if let Some(token) = &self.token {
            return Ok(Some(token.clone()));
        }
        let Some(path) = &self.token_file else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|error| CliError::Io(format!("reading {}: {error}", path.display())))?;
        let trimmed = raw.trim();
        Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
    }
}

impl Config {
    /// Read the file, or start from nothing.
    ///
    /// A missing file is not an error: the first command anybody runs is on a
    /// machine that has none.
    ///
    /// # Errors
    ///
    /// [`CliError::Io`] when it exists and cannot be read or parsed — a
    /// corrupt file is worth saying out loud rather than silently replacing
    /// with defaults, because the replacement would drop somebody's profiles.
    pub fn load(path: &Path) -> Result<Self, CliError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|error| CliError::Io(format!("reading {}: {error}", path.display())))?;
        toml::from_str(&raw)
            .map_err(|error| CliError::Io(format!("reading {}: {error}", path.display())))
    }

    /// Write it back, creating the directory and keeping the file private.
    ///
    /// # Errors
    ///
    /// [`CliError::Io`] naming the path.
    pub fn save(&self, path: &Path) -> Result<(), CliError> {
        let body = toml::to_string_pretty(self)
            .map_err(|error| CliError::Io(format!("writing {}: {error}", path.display())))?;
        crate::paths::write_private(path, body.as_bytes())
    }

    /// The profile a command should use: the one named, else the current one,
    /// else [`LOCAL`] invented on the spot.
    ///
    /// Inventing it is what makes a fresh install work with no setup — the
    /// local server is at a known address, and its token is in a known file.
    #[must_use]
    pub fn resolve(&self, named: Option<&str>, default_url: &str, token_file: PathBuf) -> Resolved {
        let name = named
            .or(self.current.as_deref())
            .unwrap_or(LOCAL)
            .to_owned();
        let profile = self.profiles.get(&name).cloned().unwrap_or_else(|| {
            Profile::local(
                std::env::var("AIWATCHER_URL").unwrap_or_else(|_| default_url.to_owned()),
                token_file,
            )
        });
        Resolved { name, profile }
    }
}

/// A profile and the name it was found under.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub name: String,
    pub profile: Profile,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_with_no_config_still_resolves_the_local_profile() {
        let config = Config::default();
        let resolved = config.resolve(None, "http://127.0.0.1:8080", PathBuf::from("/tmp/token"));
        assert_eq!(resolved.name, LOCAL);
        assert_eq!(resolved.profile.url, "http://127.0.0.1:8080");
    }

    #[test]
    fn a_named_profile_outranks_the_current_one() {
        let mut config = Config {
            current: Some("staging".into()),
            ..Config::default()
        };
        config.profiles.insert(
            "prod".into(),
            Profile {
                url: "https://prod.example".into(),
                token: Some("secret".into()),
                token_file: None,
            },
        );
        let resolved = config.resolve(Some("prod"), "http://127.0.0.1:8080", PathBuf::new());
        assert_eq!(resolved.profile.url, "https://prod.example");
    }

    #[test]
    fn a_profile_with_no_credential_is_an_answer_rather_than_a_failure() {
        // An instance running AIWATCHER_AUTH_MODE=none wants none, which is
        // the default this repository ships.
        let profile = Profile {
            url: "http://127.0.0.1:8080".into(),
            token: None,
            token_file: None,
        };
        assert_eq!(profile.secret().expect("reads"), None);
    }

    #[test]
    fn a_local_profile_reads_its_token_at_use_rather_than_at_load() {
        let dir = std::env::temp_dir().join(format!("aiwatcher-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates");
        let path = dir.join("token");
        let profile = Profile::local("http://127.0.0.1:8080".into(), path.clone());
        assert_eq!(profile.secret().expect("reads"), None);
        std::fs::write(&path, "shhh\n").expect("writes");
        assert_eq!(profile.secret().expect("reads"), Some("shhh".to_owned()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
