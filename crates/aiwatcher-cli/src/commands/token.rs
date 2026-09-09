//! `aiwatcher token …` — the credential a local instance accepts.
//!
//! One secret, one file, read by both halves: the CLI presents what is in it,
//! and a server running `AIWATCHER_AUTH_MODE=local` accepts what is in it. That
//! there is exactly one copy is the point — rotating the credential is a single
//! write, and there is no second place for it to go stale or to be found.
//!
//! Creating a token is deliberately not something `up` does silently and never
//! mentions. It says the path, and `token show` prints the secret, because the
//! next thing anybody does with a local instance is point something else at it.

use aiwatcher_auth::identity::Role;
use aiwatcher_auth::local::LocalAuth;

use crate::paths::{Paths, write_private};
use crate::{Args, CliError, Format};

/// Dispatch `token create | show | path`.
///
/// # Errors
///
/// [`CliError::Usage`] for an unknown subcommand, [`CliError::Io`] for a file
/// that could not be written or read.
pub fn run(args: &Args, paths: &Paths) -> Result<(), CliError> {
    match args.word(1) {
        Some("create") | Some("new") | None => create(args, paths),
        Some("show") | Some("print") => show(args, paths),
        Some("path") | Some("where") => {
            println!("{}", paths.token_file().display());
            Ok(())
        }
        Some(other) => Err(CliError::Usage(format!(
            "token {other:?}; expected create, show or path"
        ))),
    }
}

/// Write a fresh secret, unless one is already there and `force` was not given.
///
/// Refusing to overwrite is the important half. A second `aiwatcher up` in a
/// terminal somebody forgot about must not silently invalidate the token every
/// agent on the machine is already configured with — and when that *is* what
/// was wanted, `force` says so.
///
/// # Errors
///
/// [`CliError::Io`] naming the path, [`CliError::Other`] when the system random
/// number generator fails.
pub fn create(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let path = paths.token_file();
    if path.exists() && !args.flag("force") {
        return Err(CliError::Usage(format!(
            "a token already exists in {}; `aiwatcher token show` prints it, \
             and `aiwatcher token create force` replaces it",
            path.display()
        )));
    }
    let secret = ensure(paths, true)?;
    match Format::from_args(args)? {
        Format::Json => println!(
            "{}",
            serde_json::json!({ "token": secret, "path": path.display().to_string(), "role": "admin" })
        ),
        Format::Table => {
            println!("token  {secret}");
            println!("file   {}", path.display());
            println!("role   admin");
            println!();
            println!(
                "The server accepts it with AIWATCHER_AUTH_MODE=local, which `aiwatcher up` sets."
            );
        }
    }
    Ok(())
}

/// Print the secret that is there.
///
/// # Errors
///
/// [`CliError::Usage`] when there is none — with the command that makes one,
/// because "no such file" is not what somebody wants to read here.
pub fn show(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let path = paths.token_file();
    let secret = read(paths)?.ok_or_else(|| {
        CliError::Usage(format!(
            "no token in {}; run `aiwatcher token create`",
            path.display()
        ))
    })?;
    match Format::from_args(args)? {
        Format::Json => println!("{}", serde_json::json!({ "token": secret })),
        Format::Table => println!("{secret}"),
    }
    Ok(())
}

/// The secret on this machine, if there is one.
///
/// # Errors
///
/// [`CliError::Io`] when the file exists and cannot be read — which is a
/// different thing from there being none, and is not flattened into it.
pub fn read(paths: &Paths) -> Result<Option<String>, CliError> {
    let path = paths.token_file();
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| CliError::Io(format!("reading {}: {error}", path.display())))?;
    let trimmed = raw.trim().to_owned();
    Ok((!trimmed.is_empty()).then_some(trimmed))
}

/// The secret on this machine, making one if there is none.
///
/// What `up` calls: a local instance authenticates, and the credential it
/// authenticates with is generated here rather than asked for. `replace` is
/// what `token create force` passes.
///
/// # Errors
///
/// [`CliError::Io`] naming the path, [`CliError::Other`] when the generator
/// fails — which is not a condition to paper over with a weaker secret.
pub fn ensure(paths: &Paths, replace: bool) -> Result<String, CliError> {
    if !replace && let Some(existing) = read(paths)? {
        return Ok(existing);
    }
    let secret = LocalAuth::generate().map_err(|error| CliError::Other(error.into()))?;
    // Generated and checked on the same side, so a secret this writes is one
    // the server will accept — a generator that drifted below the length bar
    // would be a start-up failure nobody could explain.
    LocalAuth::new(secret.clone(), Role::Admin).map_err(|error| CliError::Other(error.into()))?;
    write_private(&paths.token_file(), format!("{secret}\n").as_bytes())?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own directory, because these read and write a real
    /// file and the whole point of the file is that there is one of it.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("aiwatcher-token-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).expect("creates");
            Self(dir)
        }

        fn paths(&self) -> Paths {
            Paths::under(&self.0)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_generated_token_is_one_the_server_would_accept() {
        let scratch = Scratch::new("accepts");
        let paths = scratch.paths();
        let secret = ensure(&paths, false).expect("generates");
        LocalAuth::new(secret.clone(), Role::Admin).expect("the server's own bar");
        assert_eq!(read(&paths).expect("reads"), Some(secret));
    }

    #[test]
    fn ensure_is_idempotent_so_a_second_up_keeps_the_first_ones_credential() {
        let scratch = Scratch::new("idempotent");
        let paths = scratch.paths();
        let first = ensure(&paths, false).expect("generates");
        let second = ensure(&paths, false).expect("reads");
        assert_eq!(first, second, "a second `up` invalidated the token");
    }

    #[test]
    fn replacing_is_something_you_have_to_ask_for() {
        let scratch = Scratch::new("replace");
        let paths = scratch.paths();
        let first = ensure(&paths, false).expect("generates");
        let second = ensure(&paths, true).expect("replaces");
        assert_ne!(first, second);
    }

    #[test]
    fn create_refuses_to_overwrite_without_being_told_to() {
        let scratch = Scratch::new("guard");
        let paths = scratch.paths();
        ensure(&paths, false).expect("generates");
        let args = Args::parse(["token", "create"]).expect("parses");
        let error = create(&args, &paths).expect_err("refuses");
        assert!(error.to_string().contains("force"), "{error}");
    }

    #[test]
    fn a_machine_with_no_token_says_so_rather_than_reporting_an_empty_secret() {
        let scratch = Scratch::new("absent");
        assert_eq!(read(&scratch.paths()).expect("reads"), None);
    }

    #[test]
    fn the_token_file_is_the_one_the_server_is_pointed_at() {
        // The CLI writes it and the server reads it; a test that let those two
        // resolve differently would prove nothing about the pair.
        let scratch = Scratch::new("shared");
        let paths = scratch.paths();
        ensure(&paths, false).expect("generates");
        assert!(paths.token_file().exists());
    }
}
