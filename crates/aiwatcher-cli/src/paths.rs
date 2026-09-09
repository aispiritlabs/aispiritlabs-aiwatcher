//! Where this CLI keeps things, and how it writes a secret.
//!
//! A value that is read from the environment **once**, at the top of the
//! process, and passed down — rather than a set of functions that each read it
//! again. Two reasons, and the second is the one that decided it. Reading the
//! environment repeatedly means a command's behaviour depends on when it looked;
//! and `std::env::set_var` is `unsafe` in edition 2024, which this workspace
//! forbids outright, so a test that wanted a scratch directory could not have
//! one. Passing the paths in makes the tests possible and the production path
//! simpler at the same time.

use std::path::{Path, PathBuf};

use crate::CliError;

/// Every path this CLI uses.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `~/.config/aiwatcher`, holding the profiles and the local token.
    pub config_dir: PathBuf,
    /// Where a locally-run stack keeps its state: the log, the object store,
    /// the workflow store, and the record of what `up` started.
    pub data_dir: PathBuf,
    /// The local token, when something other than the config directory names
    /// it.
    token_file: Option<PathBuf>,
}

impl Paths {
    /// Read them from the environment.
    ///
    /// XDG rather than a dot-directory in `$HOME`, and the same on macOS as on
    /// Linux: a CLI that hides its configuration somewhere platform-specific is
    /// one whose users cannot find it to edit, and this file is meant to be
    /// edited.
    #[must_use]
    pub fn from_env() -> Self {
        let config_dir = std::env::var("AIWATCHER_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map_or_else(
                        || home().join(".config").join("aiwatcher"),
                        |xdg| PathBuf::from(xdg).join("aiwatcher"),
                    )
            });
        Self {
            config_dir,
            data_dir: std::env::var("AIWATCHER_DATA_DIR")
                .map_or_else(|_| PathBuf::from("./.data"), PathBuf::from),
            token_file: std::env::var("AIWATCHER_AUTH_LOCAL_TOKEN_FILE")
                .ok()
                .map(PathBuf::from),
        }
    }

    /// Paths rooted at one directory. What a test uses.
    #[must_use]
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            token_file: None,
        }
    }

    /// The profiles file.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// The local instance's token.
    ///
    /// One path, read by both halves — the CLI presents what is in it and the
    /// server accepts what is in it — so rotating the credential is a single
    /// write and there is no second copy to go stale.
    #[must_use]
    pub fn token_file(&self) -> PathBuf {
        self.token_file
            .clone()
            .unwrap_or_else(|| self.config_dir.join("token"))
    }

    /// What `aiwatcher up` started, so `down` and `status` can find it again.
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.data_dir.join("stack")
    }

    /// The local database, when this build has one.
    #[must_use]
    pub fn database(&self) -> PathBuf {
        self.data_dir.join("aiwatcher.duckdb")
    }
}

fn home() -> PathBuf {
    std::env::var("HOME").map_or_else(|_| PathBuf::from("."), PathBuf::from)
}

/// Write a file only its owner can read, creating the directory.
///
/// The mode is set **before** the bytes land, by creating the file with it,
/// rather than written and then chmod-ed: between those two calls a
/// world-readable token exists on disk, and that window is exactly what
/// somebody watching the directory is waiting for.
///
/// # Errors
///
/// [`CliError::Io`] naming the path.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CliError::Io(format!("creating {}: {error}", parent.display())))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| CliError::Io(format!("writing {}: {error}", path.display())))?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|error| CliError::Io(format!("writing {}: {error}", path.display())))?;
    // An existing file keeps the mode it was created with, so a rotation onto
    // one somebody had loosened would stay loose. Said again on every write.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| CliError::Io(format!("securing {}: {error}", path.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("aiwatcher-paths-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).expect("creates");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_written_secret_is_readable_only_by_its_owner() {
        let scratch = Scratch::new("secret");
        let path = scratch.0.join("token");
        write_private(&path, b"shhh").expect("writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("stats")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the token file is group or world readable"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "shhh");
    }

    #[test]
    fn a_rewritten_secret_is_tightened_again() {
        let scratch = Scratch::new("loose");
        let path = scratch.0.join("token");
        std::fs::write(&path, b"old").expect("writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("opens");
            write_private(&path, b"new").expect("writes");
            let mode = std::fs::metadata(&path)
                .expect("stats")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_secret_lands_in_a_directory_that_did_not_exist() {
        let scratch = Scratch::new("nested");
        let path = scratch.0.join("a").join("b").join("token");
        write_private(&path, b"shhh").expect("writes");
        assert!(path.exists());
    }
}
