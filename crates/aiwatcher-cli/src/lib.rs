//! The aiwatcher command line.
//!
//! One binary with two jobs, which is the whole design. It **runs** an
//! instance — the local stack, or one half of a deployment — and it **speaks
//! to** one, local or remote, over the same REST API the panel uses. Nothing
//! here is a second implementation of anything: a read is an HTTP call against
//! a route that already exists, so a command cannot disagree with the panel
//! about what a run is.
//!
//! ## The shape
//!
//! `aiwatcher <words…> key=value… flag…`, after Obsidian's own CLI. A value is
//! always `key=value`, wherever on the line it sits, so there is nothing to
//! learn about short options, long options or which side of the subcommand an
//! argument goes on. `format=json` turns any read into something a script can
//! consume.
//!
//! ## Why a bare `aiwatcher` still starts a server
//!
//! Every other CLI would print help. This one runs the server, because the
//! container image's `ENTRYPOINT` is this binary with no arguments and a
//! deployment that has been running for months must not start printing usage
//! text on the next upgrade. `aiwatcher help` is the help; the cost of that
//! trade is one sentence of documentation, and the alternative was a silent
//! change to every chart in the field.

pub mod args;
pub mod client;
pub mod commands;
pub mod config;
pub mod help;
pub mod output;
pub mod paths;

use std::fmt;

pub use args::Args;

/// What went wrong, in the terms the person at the keyboard is thinking in.
///
/// Typed rather than `anyhow` for the one reason the workspace's convention
/// gives: the *category* decides the exit status, and a caller that had to
/// match on a message string to tell "you typed it wrong" from "the server said
/// no" would be reading prose.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The command line does not say something this program can do.
    #[error("{0}")]
    Usage(String),
    /// A file could not be read or written.
    #[error("{0}")]
    Io(String),
    /// The instance could not be reached at all.
    #[error("cannot reach {url}: {source}")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The instance answered, and the answer was a refusal.
    #[error("{status} from {path}{}", detail(.body))]
    Api {
        status: u16,
        path: String,
        body: String,
    },
    /// Something a command needed from the machine — Docker, a port, a process.
    #[error("{0}")]
    Environment(String),
    /// Anything else, from a library that reports in its own terms.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // The API answers refusals as a JSON problem document; showing the field
    // people care about beats showing the envelope it arrived in.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(message) = value
            .get("detail")
            .or_else(|| value.get("message"))
            .or_else(|| value.get("error"))
            .and_then(serde_json::Value::as_str)
    {
        return format!(" — {message}");
    }
    format!(" — {trimmed}")
}

impl CliError {
    /// What the process exits with.
    ///
    /// Distinguished rather than a flat `1`, because a wrapper script wants to
    /// retry a `75` and never wants to retry a `64`.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            // `EX_USAGE`, from sysexits.h — what every shell convention means
            // by "the command line was wrong".
            Self::Usage(_) => 64,
            // `EX_TEMPFAIL`: the instance may well be there in a moment.
            Self::Unreachable { .. } => 75,
            Self::Api { .. } | Self::Io(_) | Self::Environment(_) | Self::Other(_) => 1,
        }
    }
}

/// How a command wants its answer shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// Aligned columns. What a person reads.
    #[default]
    Table,
    /// The API's own JSON, unchanged. What a script reads, and what makes this
    /// CLI a usable client for the routes it has no verb for.
    Json,
}

impl Format {
    /// Read `format=…`, defaulting to a table.
    ///
    /// # Errors
    ///
    /// [`CliError::Usage`] naming what arrived, rather than quietly choosing
    /// one — a script that asked for `json` and got a table would produce
    /// output nothing downstream could parse.
    pub fn from_args(args: &Args) -> Result<Self, CliError> {
        if args.flag("json") {
            return Ok(Self::Json);
        }
        match args.value("format") {
            None | Some("table" | "text") => Ok(Self::Table),
            Some("json") => Ok(Self::Json),
            Some(other) => Err(CliError::Usage(format!(
                "format={other:?}; expected table or json"
            ))),
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Table => "table",
            Self::Json => "json",
        })
    }
}

/// The entry point: read `argv`, do what it says, report what happened.
///
/// # Errors
///
/// Whatever the chosen command could not do.
pub async fn run<I, S>(raw: I) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = Args::parse(raw)?;
    commands::dispatch(args).await
}
