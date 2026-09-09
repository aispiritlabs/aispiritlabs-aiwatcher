//! The verbs that change something.
//!
//! Fewer than the reads, and each one deliberate. A CLI that could do
//! everything the API can would be a generated one, and the escape hatch
//! already reaches every route; what earns a verb here is a thing somebody
//! types while something is going wrong — pause this run, retry that step,
//! promote that prompt — where looking up a path and hand-writing a JSON body
//! is the wrong thing to be doing.
//!
//! The commands that carry no body share a table for the same reason the reads
//! do: they differ only in a path, and writing them out five times is five
//! chances to handle a confirmation differently.

use serde_json::{Value, json};

use crate::client::{Client, emit};
use crate::{Args, CliError, Format};

/// A write whose body is empty or trivial.
#[derive(Debug)]
pub struct Command {
    pub words: &'static [&'static str],
    pub path: &'static str,
    pub summary: &'static str,
}

/// The command routes, which are all POSTs against one run.
pub const COMMANDS: &[Command] = &[
    Command {
        words: &["executions", "cancel"],
        path: "/api/v1/executions/{id}/commands/cancel",
        summary: "stop a managed run",
    },
    Command {
        words: &["executions", "pause"],
        path: "/api/v1/executions/{id}/commands/pause",
        summary: "hold a managed run where it is",
    },
    Command {
        words: &["executions", "resume"],
        path: "/api/v1/executions/{id}/commands/resume",
        summary: "let a paused run continue",
    },
    Command {
        words: &["executions", "retry"],
        path: "/api/v1/executions/{id}/steps/{step}/commands/retry",
        summary: "try one failed step again",
    },
    Command {
        words: &["workflows", "rerun"],
        path: "/api/v1/workflows/{id}/rerun",
        summary: "ask the configured runner to run a workflow again",
    },
    Command {
        words: &["prompts", "rebuild"],
        path: "/api/v1/prompts/{name}/rebuild",
        summary: "rebuild one prompt's index from the objects behind it",
    },
];

/// The command whose words this line starts with.
#[must_use]
pub fn find(words: &[String]) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .filter(|command| {
            command.words.len() <= words.len()
                && command
                    .words
                    .iter()
                    .zip(words)
                    .all(|(expected, given)| expected == given)
        })
        .max_by_key(|command| command.words.len())
}

/// Run one.
///
/// # Errors
///
/// [`CliError::Usage`] for a missing path parameter, and whatever the instance
/// answered — including the 409 that says the run is not in a state this
/// command applies to, which is an answer rather than a bug.
pub async fn command(spec: &Command, args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let path = super::read::substitute_path(spec.path, args)?;
    let body = args
        .value("reason")
        .map_or_else(|| json!({}), |reason| json!({ "reason": reason }));
    let answer = client.post(&path, &body).await?;
    if matches!(answer, Value::Null) {
        println!("done");
        return Ok(());
    }
    emit(&answer, format)
}

/// `aiwatcher prompts publish name=… text=@file`.
///
/// Publishing is idempotent by construction — a version id is the digest of the
/// text — so this needs no confirmation and repeating it is free.
///
/// # Errors
///
/// [`CliError::Usage`] for a missing argument or a text file that cannot be
/// read, and whatever the instance answered.
pub async fn publish_prompt(args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let name = args.require("name")?;
    let text = text_of(args, "text")?;

    let mut body = json!({ "name": name, "text": text });
    if let Some(label) = args.value("label")
        && let Some(map) = body.as_object_mut()
    {
        map.insert("label".into(), json!(label));
    }
    if let Some(description) = args.value("description")
        && let Some(map) = body.as_object_mut()
    {
        map.insert("description".into(), json!(description));
    }
    let answer = client.post("/api/v1/prompts", &body).await?;
    emit(&answer, format)
}

/// `aiwatcher prompts label name=… label=production version=…`.
///
/// # Errors
///
/// As above.
pub async fn label_prompt(args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let name = args.require("name")?;
    let label = args.require("label")?;
    let version = args.require("version")?;
    let path = format!("/api/v1/prompts/{name}/labels/{label}");
    let answer = client.put(&path, &json!({ "version_id": version })).await?;
    if matches!(answer, Value::Null) {
        println!("{label} now points at {version}");
        return Ok(());
    }
    emit(&answer, format)
}

/// `aiwatcher events publish body=@run.json` — the ingest route.
///
/// What an agent's SDK does, available from a shell so that a local install can
/// be given something to show without writing a program.
///
/// # Errors
///
/// [`CliError::Usage`] for a body that is not JSON, and whatever the instance
/// answered — including the 501 that says this instance does not accept a
/// second write path.
pub async fn publish_events(args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let body: Value = serde_json::from_str(&text_of(args, "body")?)
        .map_err(|error| CliError::Usage(format!("the body is not JSON: {error}")))?;
    let answer = client.post("/api/v1/events", &body).await?;
    emit(&answer, format)
}

/// `aiwatcher executions start …` — start a managed run.
///
/// # Errors
///
/// As above.
pub async fn start_execution(args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let body: Value = match args.value("body") {
        Some(_) => serde_json::from_str(&text_of(args, "body")?)
            .map_err(|error| CliError::Usage(format!("the body is not JSON: {error}")))?,
        None => {
            let kind = args.value_or("kind", "pipeline");
            let name = args.require("name")?;
            json!({ "kind": kind, "name": name })
        }
    };
    let answer = client.post("/api/v1/executions", &body).await?;
    emit(&answer, format)
}

/// A named value that may be inline, `@file`, or `@-` for standard input.
fn text_of(args: &Args, key: &str) -> Result<String, CliError> {
    let raw = args.require(key)?;
    match raw.strip_prefix('@') {
        None => Ok(raw.to_owned()),
        Some("-") => {
            use std::io::Read as _;
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|error| CliError::Io(format!("reading standard input: {error}")))?;
            Ok(buffer)
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| CliError::Io(format!("reading {path}: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_matching_command_wins() {
        let words = vec!["executions".to_owned(), "retry".to_owned()];
        assert_eq!(
            find(&words).expect("matches").words,
            ["executions", "retry"]
        );
    }

    #[test]
    fn a_line_that_names_no_command_matches_none() {
        let words = vec!["executions".to_owned(), "explode".to_owned()];
        assert!(find(&words).is_none());
    }

    #[test]
    fn text_may_be_given_inline() {
        let args = Args::parse(["text=You are a helpful assistant."]).expect("parses");
        assert_eq!(
            text_of(&args, "text").expect("reads"),
            "You are a helpful assistant."
        );
    }

    #[test]
    fn text_may_be_read_from_a_file() {
        let path = std::env::temp_dir().join(format!("aiwatcher-prompt-{}", std::process::id()));
        std::fs::write(&path, "from a file").expect("writes");
        let args = Args::parse([format!("text=@{}", path.display())]).expect("parses");
        assert_eq!(text_of(&args, "text").expect("reads"), "from a file");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_published_as_its_own_name() {
        // The failure this prevents: `text=@prompt.txt` with a typo publishing
        // the literal string "@prompt.txt" as somebody's system prompt.
        let args = Args::parse(["text=@/nonexistent/prompt.txt"]).expect("parses");
        assert!(matches!(text_of(&args, "text"), Err(CliError::Io(_))));
    }

    #[test]
    fn every_command_declares_a_route_and_a_summary() {
        for command in COMMANDS {
            assert!(command.path.starts_with("/api/v1/"), "{}", command.path);
            assert!(!command.summary.is_empty(), "{}", command.path);
        }
    }
}
