//! `aiwatcher profile …` — which instance a command talks to.
//!
//! The second half of the answer to "the CLI should also reach a server". A
//! local install needs none of this: with no configuration at all, commands go
//! to the instance on this machine and present the token in the local token
//! file. A profile is what you add when there is somewhere *else* — a shared
//! deployment, a colleague's staging namespace — and it holds the address and
//! the credential for it, in a file only its owner can read.

use crate::config::{Config, Profile};
use crate::paths::Paths;
use crate::{Args, CliError, Format};

/// Dispatch `profile list | use | set | remove | show`.
///
/// # Errors
///
/// [`CliError::Usage`] for an unknown subcommand or a missing argument, and
/// [`CliError::Io`] for a configuration file that cannot be read or written.
pub fn run(args: &Args, paths: &Paths) -> Result<(), CliError> {
    match args.word(1) {
        Some("list") | Some("ls") | None => list(args, paths),
        Some("use") | Some("switch") => use_one(args, paths),
        Some("set") | Some("add") => set(args, paths),
        Some("remove") | Some("delete") | Some("rm") => remove(args, paths),
        Some("show") => show(args, paths),
        Some(other) => Err(CliError::Usage(format!(
            "profile {other:?}; expected list, use, set, remove or show"
        ))),
    }
}

fn list(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let path = paths.config_file();
    let config = Config::load(&path)?;
    let current = config.current.as_deref().unwrap_or(crate::config::LOCAL);

    if Format::from_args(args)? == Format::Json {
        let rows: Vec<_> = config
            .profiles
            .iter()
            .map(|(name, profile)| {
                serde_json::json!({
                    "name": name,
                    "url": profile.url,
                    "current": name == current,
                    "has_token": profile.token.is_some() || profile.token_file.is_some(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "current": current, "items": rows })
        );
        return Ok(());
    }

    if config.profiles.is_empty() {
        println!("no profiles configured; commands go to the instance on this machine");
        println!("add one with: aiwatcher profile set name=prod url=https://… token=…");
        return Ok(());
    }
    for (name, profile) in &config.profiles {
        let marker = if name == current { "*" } else { " " };
        println!("{marker} {name}  {}", profile.url);
    }
    Ok(())
}

fn show(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let config = Config::load(&paths.config_file())?;
    let resolved = config.resolve(
        args.value("name"),
        crate::commands::stack::DEFAULT_URL,
        paths.token_file(),
    );
    // Never the secret itself: this is the command somebody runs while
    // screen-sharing to find out where their commands are going.
    let has = resolved.profile.secret()?.is_some();
    match Format::from_args(args)? {
        Format::Json => println!(
            "{}",
            serde_json::json!({
                "name": resolved.name,
                "url": resolved.profile.url,
                "has_token": has,
            })
        ),
        Format::Table => {
            println!("name   {}", resolved.name);
            println!("url    {}", resolved.profile.url);
            println!("token  {}", if has { "yes" } else { "none" });
        }
    }
    Ok(())
}

fn use_one(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let name = args
        .value("name")
        .or_else(|| args.word(2))
        .ok_or_else(|| CliError::Usage("this command needs name=…".into()))?
        .to_owned();
    let path = paths.config_file();
    let mut config = Config::load(&path)?;
    // `local` is legitimate with nothing stored under it — that is the profile
    // a machine with no configuration already resolves to, and refusing it
    // would make "go back to my own instance" the one switch that fails.
    if name != crate::config::LOCAL && !config.profiles.contains_key(&name) {
        return Err(CliError::Usage(format!(
            "no profile called {name:?}; `aiwatcher profile list` shows what there is"
        )));
    }
    config.current = Some(name.clone());
    config.save(&path)?;
    println!("commands now go to {name}");
    Ok(())
}

fn set(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let name = args.require("name")?.to_owned();
    let url = args.require("url")?.trim_end_matches('/').to_owned();
    let path = paths.config_file();
    let mut config = Config::load(&path)?;

    let profile = Profile {
        url,
        token: args.value("token").map(str::to_owned),
        token_file: args.value("token-file").map(std::path::PathBuf::from),
    };
    let replacing = config.profiles.insert(name.clone(), profile).is_some();
    if args.flag("use") || config.current.is_none() {
        config.current = Some(name.clone());
    }
    config.save(&path)?;
    println!(
        "{} profile {name} in {}",
        if replacing { "replaced" } else { "added" },
        path.display()
    );
    Ok(())
}

fn remove(args: &Args, paths: &Paths) -> Result<(), CliError> {
    let name = args
        .value("name")
        .or_else(|| args.word(2))
        .ok_or_else(|| CliError::Usage("this command needs name=…".into()))?
        .to_owned();
    let path = paths.config_file();
    let mut config = Config::load(&path)?;
    if config.profiles.remove(&name).is_none() {
        return Err(CliError::Usage(format!("no profile called {name:?}")));
    }
    // A current profile that no longer exists would make every later command
    // fail with a message about a name nobody typed.
    if config.current.as_deref() == Some(name.as_str()) {
        config.current = None;
    }
    config.save(&path)?;
    println!("removed profile {name}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("aiwatcher-profile-{name}-{}", std::process::id()));
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

    fn args(line: &[&str]) -> Args {
        Args::parse(line).expect("parses")
    }

    #[test]
    fn removing_the_current_profile_does_not_leave_a_dangling_pointer() {
        let scratch = Scratch::new("dangling");
        let paths = scratch.paths();
        set(
            &args(&["profile", "set", "name=prod", "url=https://p.example"]),
            &paths,
        )
        .expect("sets");
        remove(&args(&["profile", "remove", "name=prod"]), &paths).expect("removes");
        let config = Config::load(&paths.config_file()).expect("loads");
        assert_eq!(config.current, None, "a removed profile stayed current");
    }

    #[test]
    fn the_first_profile_added_becomes_the_current_one() {
        let scratch = Scratch::new("first");
        let paths = scratch.paths();
        set(
            &args(&["profile", "set", "name=prod", "url=https://p.example"]),
            &paths,
        )
        .expect("sets");
        let config = Config::load(&paths.config_file()).expect("loads");
        assert_eq!(config.current.as_deref(), Some("prod"));
    }

    #[test]
    fn switching_back_to_local_works_with_nothing_stored_under_it() {
        let scratch = Scratch::new("local");
        let paths = scratch.paths();
        set(
            &args(&["profile", "set", "name=prod", "url=https://p.example"]),
            &paths,
        )
        .expect("sets");
        use_one(&args(&["profile", "use", "name=local"]), &paths).expect("switches");
        let config = Config::load(&paths.config_file()).expect("loads");
        assert_eq!(config.current.as_deref(), Some("local"));
    }

    #[test]
    fn switching_to_a_profile_nobody_added_is_refused_by_name() {
        let scratch = Scratch::new("missing");
        let error = use_one(&args(&["profile", "use", "name=nope"]), &scratch.paths())
            .expect_err("refuses");
        assert!(error.to_string().contains("nope"), "{error}");
    }

    #[test]
    fn a_stored_profile_keeps_its_token_across_a_round_trip() {
        let scratch = Scratch::new("roundtrip");
        let paths = scratch.paths();
        set(
            &args(&[
                "profile",
                "set",
                "name=prod",
                "url=https://p.example",
                "token=abcdefghijklmnopqrstuvwx",
            ]),
            &paths,
        )
        .expect("sets");
        let config = Config::load(&paths.config_file()).expect("loads");
        let profile = config.profiles.get("prod").expect("stored");
        assert_eq!(
            profile.secret().expect("reads").as_deref(),
            Some("abcdefghijklmnopqrstuvwx")
        );
    }
}
