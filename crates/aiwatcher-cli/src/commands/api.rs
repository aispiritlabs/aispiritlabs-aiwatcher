//! `aiwatcher api …` — the escape hatch.
//!
//! The reading verbs cover what people type often; this covers everything else,
//! which today is a hundred-odd routes and tomorrow is however many there are.
//! It exists so that "the CLI cannot do that" is never an answer: a route that
//! is in the contract is reachable from the command line the moment it ships,
//! with the same credential and against the same profile as every other
//! command.
//!
//! A body may be given inline or read from a file with `body=@path`, and `-` is
//! standard input — which is what makes `aiwatcher api post … body=@-` a link in
//! a pipeline rather than a shell-quoting exercise.

use std::io::Read as _;

use serde_json::Value;

use crate::client::{Client, emit};
use crate::{Args, CliError, Format};

/// Run a raw request.
///
/// # Errors
///
/// [`CliError::Usage`] for a method or a body this cannot read, and whatever
/// the instance answered.
pub async fn run(args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let method = method_of(args)?;
    let path = args.require("path")?;
    let body = body_of(args)?;

    // Everything the command did not name for itself is a query parameter.
    // That is what makes an unfamiliar route usable without this file learning
    // about it — and the alternative, a `query=` string somebody has to
    // percent-encode by hand, is the thing people get wrong.
    let known = ["path", "method", "body", "format"];
    let extra = args.other_values(&known);

    let answer = client.request(method, path, &extra, body).await?;
    emit(&answer, format)
}

/// The verb, from the word after `api` or from `method=`.
fn method_of(args: &Args) -> Result<reqwest::Method, CliError> {
    let raw = args
        .word(1)
        .or_else(|| args.value("method"))
        .unwrap_or("get");
    match raw.to_ascii_lowercase().as_str() {
        "get" => Ok(reqwest::Method::GET),
        "post" => Ok(reqwest::Method::POST),
        "put" => Ok(reqwest::Method::PUT),
        "patch" => Ok(reqwest::Method::PATCH),
        "delete" => Ok(reqwest::Method::DELETE),
        other => Err(CliError::Usage(format!(
            "api {other:?}; expected get, post, put, patch or delete"
        ))),
    }
}

/// The request body: absent, inline JSON, `@file`, or `@-` for stdin.
fn body_of(args: &Args) -> Result<Option<Value>, CliError> {
    let Some(raw) = args.value("body") else {
        return Ok(None);
    };
    let text = match raw.strip_prefix('@') {
        None => raw.to_owned(),
        Some("-") => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|error| CliError::Io(format!("reading standard input: {error}")))?;
            buffer
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| CliError::Io(format!("reading {path}: {error}")))?,
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| CliError::Usage(format!("the body is not JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_method_may_be_a_word_or_a_value() {
        let word = Args::parse(["api", "post", "path=/x"]).expect("parses");
        assert_eq!(method_of(&word).expect("reads"), reqwest::Method::POST);
        let value = Args::parse(["api", "method=delete", "path=/x"]).expect("parses");
        assert_eq!(method_of(&value).expect("reads"), reqwest::Method::DELETE);
    }

    #[test]
    fn a_get_is_what_you_get_for_saying_nothing() {
        let args = Args::parse(["api", "path=/api/v1/runs"]).expect("parses");
        assert_eq!(method_of(&args).expect("reads"), reqwest::Method::GET);
    }

    #[test]
    fn an_unknown_method_is_refused_by_name() {
        let args = Args::parse(["api", "yeet", "path=/x"]).expect("parses");
        let error = method_of(&args).expect_err("refuses");
        assert!(error.to_string().contains("yeet"), "{error}");
    }

    #[test]
    fn a_body_may_be_inline_json() {
        let args = Args::parse([r#"body={"a":1}"#]).expect("parses");
        assert_eq!(
            body_of(&args).expect("reads"),
            Some(serde_json::json!({ "a": 1 }))
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_refused_before_it_is_sent() {
        // Sending it would produce a 400 from the server describing a document
        // the person never wrote; the refusal here names the actual problem.
        let args = Args::parse(["body=not json"]).expect("parses");
        assert!(matches!(body_of(&args), Err(CliError::Usage(_))));
    }

    #[test]
    fn everything_the_command_did_not_name_becomes_a_query_parameter() {
        let args = Args::parse(["path=/api/v1/runs", "format=json", "status=running"])
            .expect("parses");
        let extra = args.other_values(&["path", "method", "body", "format"]);
        assert_eq!(extra, [("status", "running")]);
    }
}
