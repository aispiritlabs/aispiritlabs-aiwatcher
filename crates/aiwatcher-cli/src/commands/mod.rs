//! What a command line means.
//!
//! One `match`, in one place, so that "what does `aiwatcher X` do" has exactly
//! one answer and finding it is one read. The order of the arms is the only
//! subtle thing here and it is deliberate: the words that run a *server* are
//! checked before the words that talk to one, because `serve` and `work` are
//! the container's contract and must never become ambiguous with a noun
//! somebody adds later.

pub mod api;
pub mod read;
pub mod profile;
pub mod serve;
pub mod stack;
pub mod token;
pub mod write;

#[cfg(feature = "duckdb")]
pub mod sql;

use std::time::Duration;

use aiwatcher_server::config::{Config as ServerConfig, ProcessRole};

use crate::client::Client;
use crate::config::Config;
use crate::paths::Paths;
use crate::{Args, CliError};

/// How long a command waits for an instance before giving up.
///
/// Generous, because one of these routes folds a log and another asks an object
/// store; short enough that a wrong address in a profile is a mistake somebody
/// notices rather than a terminal that hangs.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Do what the line says.
///
/// # Errors
///
/// Whatever the chosen command could not do.
pub async fn dispatch(mut args: Args) -> Result<(), CliError> {
    let paths = Paths::from_env();

    // ── Running a server ─────────────────────────────────────────────────────
    //
    // Including the bare invocation, which is the container image's entry
    // point and therefore may not become anything else. See the crate docs.
    match args.word(0) {
        None => return serve(None).await,
        // Not `api` as a third spelling of this role, however well it reads:
        // `aiwatcher api get path=…` is the passthrough, and one word cannot
        // mean both "run the API" and "call it". The role's own variable still
        // accepts `api`, because there it is unambiguous.
        Some("serve" | "server") => return serve(Some(ProcessRole::Serve)).await,
        Some("work" | "worker") => return serve(Some(ProcessRole::Work)).await,
        Some("both" | "all") => return serve(Some(ProcessRole::Both)).await,
        _ => {}
    }

    // ── Everything else ──────────────────────────────────────────────────────
    match args.word(0) {
        Some("help" | "h") => {
            crate::help::print(args.word(1));
            return Ok(());
        }
        Some("version") => {
            println!("aiwatcher {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("up" | "down" | "status") => return stack::run(&args, &paths).await,
        Some("token") => return token::run(&args, &paths),
        Some("profile" | "context") => return profile::run(&args, &paths),
        #[cfg(feature = "duckdb")]
        Some("sql") => return sql::run(&args, &paths),
        #[cfg(not(feature = "duckdb"))]
        Some("sql") => {
            return Err(CliError::Environment(
                "this build has no local database; rebuild with `--features duckdb`".into(),
            ));
        }
        _ => {}
    }

    // Everything below here talks to an instance.
    let client = connect(&args, &paths)?;

    if args.word(0) == Some("api") {
        return api::run(&args, &client).await;
    }

    // A verb is looked up by its words, longest first, and the two tables are
    // asked in the order that makes a write impossible to reach by accident: a
    // word that reads is a word that reads.
    if let Some(spec) = read::find(args.words()) {
        let taken = spec.words.len();
        args.take_words(taken);
        return read::run(spec, &args, &client).await;
    }
    if let Some(spec) = write::find(args.words()) {
        let taken = spec.words.len();
        args.take_words(taken);
        return write::command(spec, &args, &client).await;
    }

    match (args.word(0), args.word(1)) {
        (Some("prompts"), Some("publish")) => write::publish_prompt(&args, &client).await,
        (Some("prompts"), Some("label")) => write::label_prompt(&args, &client).await,
        (Some("events"), Some("publish")) => write::publish_events(&args, &client).await,
        (Some("executions"), Some("start")) => write::start_execution(&args, &client).await,
        (Some(word), _) => Err(CliError::Usage(format!(
            "{word:?} is not an aiwatcher command; `aiwatcher help` lists what is"
        ))),
        (None, _) => {
            crate::help::print(None);
            Ok(())
        }
    }
}

/// Run the server in the role the line named.
async fn serve(role: Option<ProcessRole>) -> Result<(), CliError> {
    let mut config = ServerConfig::from_env().map_err(|error| CliError::Other(error.into()))?;
    // The argument wins over the variable: a container sets `AIWATCHER_ROLE`
    // and a person types `aiwatcher work`, and the one typed last is the one
    // that was meant.
    if let Some(role) = role {
        config.role = role;
    }
    // Read again, because the argument can have made a valid configuration
    // invalid — splitting the binary in two on a store that holds one process
    // is the case, and it is better said here than by a lock file.
    config
        .validate()
        .map_err(|error| CliError::Other(error.into()))?;
    serve::run(config).await.map_err(CliError::Other)
}

/// A client for whichever instance this line is about.
fn connect(args: &Args, paths: &Paths) -> Result<Client, CliError> {
    let config = Config::load(&paths.config_file())?;
    let resolved = config.resolve(
        args.value("profile"),
        stack::DEFAULT_URL,
        paths.token_file(),
    );
    // `url=` on the line outranks the profile, for the one case a profile is
    // the wrong tool: reaching an instance once, without storing anything.
    let resolved = match args.value("url") {
        None => resolved,
        Some(url) => crate::config::Resolved {
            name: format!("{} (url=)", resolved.name),
            profile: crate::config::Profile {
                url: url.to_owned(),
                ..resolved.profile
            },
        },
    };
    Client::new(&resolved, TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_that_names_nothing_is_refused_with_the_command_that_helps() {
        let args = Args::parse(["frobnicate"]).expect("parses");
        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(dispatch(args))
            .expect_err("refuses");
        assert!(error.to_string().contains("help"), "{error}");
        assert!(error.to_string().contains("frobnicate"), "{error}");
    }

    #[test]
    fn no_verb_is_also_the_name_of_a_server_role() {
        // `api` was both, briefly: a third spelling of the `serve` role and
        // the passthrough. `aiwatcher api get path=…` started a server.
        for word in ["api", "up", "down", "status", "token", "profile", "sql", "help"] {
            assert!(
                !matches!(word, "serve" | "server" | "work" | "worker" | "both" | "all"),
                "{word} is a command and a role"
            );
        }
    }

    #[test]
    fn the_reading_verbs_and_the_writing_verbs_do_not_collide() {
        // A word in both tables would resolve by whichever is asked first,
        // which is a coin toss between reading a run and cancelling it.
        for read in read::READS {
            assert!(
                write::find(&read.words.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
                    .is_none(),
                "{:?} is both a read and a write",
                read.words
            );
        }
    }
}
