//! The reading verbs, as a table rather than as thirty near-identical
//! functions.
//!
//! Every one of these is the same shape — substitute the path parameters,
//! forward the named values as query parameters, print what came back — so the
//! part that differs between them is data and is written as data. What that
//! buys is not brevity: it is that adding a verb cannot accidentally introduce
//! a *different* way of handling a window, a cursor or a format, which is
//! exactly the drift that makes a CLI's own subcommands disagree with each
//! other.
//!
//! Two translations happen here and nowhere else. `window=15m` becomes
//! `window_seconds=900`, because a person thinks in minutes and the API is
//! specified in seconds; and a CLI-side name is mapped to the API's own where
//! they differ (`agent=` for `agent_id=`, `run=` for `run_id=`), because the
//! prefix reads as noise on a command line and as precision in a contract.

use crate::client::{Client, emit};
use crate::{Args, CliError, Format};

/// One read verb.
#[derive(Debug)]
pub struct Read {
    /// What the person types, in order.
    pub words: &'static [&'static str],
    /// The route, with `{name}` for anything taken from an argument.
    pub path: &'static str,
    /// Values forwarded as query parameters. Each is `(what the person types,
    /// what the API calls it)` — the same string twice where they agree.
    pub query: &'static [(&'static str, &'static str)],
    /// One line for `aiwatcher help`.
    pub summary: &'static str,
}

/// Every read the CLI has a verb for.
///
/// Not every read the API has: the passthrough reaches the rest, and a verb
/// exists where the command is one somebody types often enough to want the
/// short spelling.
pub const READS: &[Read] = &[
    Read {
        words: &["runs", "list"],
        path: "/api/v1/runs",
        query: &[
            ("agent", "agent_id"),
            ("conversation", "conversation_id"),
            ("model", "model"),
            ("runtime", "runtime"),
            ("status", "status"),
            ("tool", "tool"),
            ("trace", "trace_id"),
            ("workflow", "workflow"),
            ("before", "before"),
            ("limit", "limit"),
        ],
        summary: "runs active in the window, newest first",
    },
    Read {
        words: &["runs", "show"],
        path: "/api/v1/runs/{id}",
        query: &[],
        summary: "one run, with its spans",
    },
    Read {
        words: &["runs", "events"],
        path: "/api/v1/runs/{id}/events",
        query: &[
            ("after", "after"),
            ("limit", "limit"),
            ("search", "q"),
            ("span", "span_id"),
            ("type", "event_type"),
        ],
        summary: "one run's events, a page at a time",
    },
    Read {
        words: &["spans", "list"],
        path: "/api/v1/spans",
        query: &[
            ("run", "run_id"),
            ("trace", "trace_id"),
            ("agent", "agent_id"),
            ("model", "model"),
            ("tool", "tool"),
            ("operation", "operation"),
            ("status", "status"),
            ("search", "search"),
            ("slower-than", "min_duration_ms"),
            ("after", "after"),
            ("limit", "limit"),
        ],
        summary: "finished spans, filtered",
    },
    Read {
        words: &["dimensions"],
        path: "/api/v1/dimensions/{kind}",
        query: &[
            ("agent", "agent_id"),
            ("search", "search"),
            ("after", "after"),
            ("limit", "limit"),
        ],
        summary: "one pivot over the runs: kind=session|agent|runtime|workflow|trace|model|tool",
    },
    Read {
        words: &["metrics"],
        path: "/api/v1/metrics",
        query: &[
            ("agent", "agent_id"),
            ("model", "model"),
            ("conversation", "conversation_id"),
            ("buckets", "buckets"),
        ],
        summary: "the timeline: runs, tokens and latency per bucket",
    },
    Read {
        words: &["live"],
        path: "/api/v1/live",
        query: &[("run", "run_id"), ("from", "from")],
        summary: "what the live channel holds right now",
    },
    Read {
        words: &["evaluations", "list"],
        path: "/api/v1/evaluations",
        query: &[
            ("suite", "suite"),
            ("dataset", "dataset"),
            ("variant", "variant"),
            ("status", "status"),
            ("search", "search"),
            ("after", "after"),
            ("limit", "limit"),
        ],
        summary: "evaluation reports",
    },
    Read {
        words: &["evaluations", "show"],
        path: "/api/v1/evaluations/{id}",
        query: &[],
        summary: "one report, with its cases",
    },
    Read {
        words: &["prompts", "list"],
        path: "/api/v1/prompts",
        query: &[
            ("search", "search"),
            ("tag", "tag"),
            ("after", "after"),
            ("limit", "limit"),
        ],
        summary: "the prompt registry",
    },
    Read {
        words: &["prompts", "show"],
        path: "/api/v1/prompts/{name}",
        query: &[],
        summary: "one prompt: its versions and its labels",
    },
    Read {
        words: &["workflows", "list"],
        path: "/api/v1/workflows",
        query: &[("search", "search"), ("after", "after"), ("limit", "limit")],
        summary: "workflow graphs folded from the log",
    },
    Read {
        words: &["workflows", "show"],
        path: "/api/v1/workflows/{id}",
        query: &[],
        summary: "one declared graph",
    },
    Read {
        words: &["executions", "list"],
        path: "/api/v1/workflow-executions",
        query: &[
            ("workflow", "workflow_id"),
            ("status", "status"),
            ("search", "search"),
            ("after", "after"),
            ("limit", "limit"),
        ],
        summary: "executions of a graph, managed or observed",
    },
    Read {
        words: &["executions", "show"],
        path: "/api/v1/executions/{id}",
        query: &[],
        summary: "one managed run: its state and what may be done to it",
    },
    Read {
        words: &["executions", "history"],
        path: "/api/v1/executions/{id}/history",
        query: &[("after", "after"), ("limit", "limit")],
        summary: "one managed run's decisions, in order",
    },
    // The registry routes take no cursor: these three lists are bounded by
    // what somebody authored rather than by retention, so there is no page to
    // ask for. Declaring one here would send a parameter the route does not
    // accept, and the API refuses unknown query parameters — see the contract
    // test, which is what found this.
    Read {
        words: &["models", "list"],
        path: "/api/v1/models",
        query: &[],
        summary: "the model registry",
    },
    Read {
        words: &["training", "list"],
        path: "/api/v1/training-runs",
        query: &[
            ("model", "model"),
            ("dataset", "dataset"),
            ("status", "status"),
            ("limit", "limit"),
        ],
        summary: "training runs and their curves",
    },
    Read {
        words: &["datasets", "list"],
        path: "/api/v1/datasets",
        query: &[],
        summary: "dataset versions",
    },
    Read {
        words: &["schedules", "show"],
        path: "/api/v1/curation-pipelines/{name}/schedule",
        query: &[],
        summary: "when a curation pipeline next runs",
    },
];

/// The verb whose words this line starts with, longest match first.
///
/// Longest first because `runs list` and `runs` would otherwise be ambiguous in
/// the order they happen to be written in — and the day somebody adds a bare
/// `runs`, the two-word verbs must not stop working.
#[must_use]
pub fn find(words: &[String]) -> Option<&'static Read> {
    READS
        .iter()
        .filter(|read| {
            read.words.len() <= words.len()
                && read
                    .words
                    .iter()
                    .zip(words)
                    .all(|(expected, given)| expected == given)
        })
        .max_by_key(|read| read.words.len())
}

/// Run one.
///
/// # Errors
///
/// [`CliError::Usage`] for a missing path parameter, and whatever the instance
/// answered.
pub async fn run(read: &Read, args: &Args, client: &Client) -> Result<(), CliError> {
    let format = Format::from_args(args)?;
    let path = substitute_path(read.path, args)?;

    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(seconds) = window_seconds(args)? {
        query.push(("window_seconds", seconds.to_string()));
    }
    if let Some(as_of) = args.value("as-of") {
        query.push(("as_of", as_of.to_owned()));
    }
    for (typed, sent) in read.query {
        if let Some(value) = args.value(typed) {
            query.push((sent, value.to_owned()));
        }
    }

    let borrowed: Vec<(&str, &str)> = query
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let answer = client.get(&path, &borrowed).await?;
    emit(&answer, format)
}

/// Fill `{name}` from the arguments.
///
/// Shared with the write verbs, which take the same path shapes: two copies of
/// this would be two answers to "what does `{id}` mean".
///
/// # Errors
///
/// [`CliError::Usage`] naming the parameter that was not given.
pub fn substitute_path(template: &str, args: &Args) -> Result<String, CliError> {
    let mut path = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|index| open + index) else {
            break;
        };
        let name = &rest[open + 1..close];
        // `id` is the one that gets a synonym, because every noun has one and
        // typing `runs show id=…` is what a person reaches for before
        // `run_id=…`. The rest are named for what they are.
        let value = args
            .value(name)
            .or_else(|| (name == "id").then(|| args.value("run")).flatten())
            .ok_or_else(|| CliError::Usage(format!("this command needs {name}=…")))?;
        path.push_str(&rest[..open]);
        path.push_str(value);
        rest = &rest[close + 1..];
    }
    path.push_str(rest);
    Ok(path)
}

/// `window=15m` as seconds.
///
/// The suffixes are the ones the panel's own control offers, and a bare number
/// is seconds — so a script that already has one does not have to append a
/// unit it would then have to strip.
///
/// # Errors
///
/// [`CliError::Usage`] naming what arrived.
pub fn window_seconds(args: &Args) -> Result<Option<u64>, CliError> {
    let Some(raw) = args.value("window") else {
        return Ok(None);
    };
    parse_window(raw).map(Some)
}

/// The parse, separately, because it is the part worth testing.
///
/// # Errors
///
/// [`CliError::Usage`] naming what arrived.
pub fn parse_window(raw: &str) -> Result<u64, CliError> {
    let raw = raw.trim();
    let refuse = || CliError::Usage(format!("window={raw:?}; expected something like 15m, 6h or 7d"));
    let (digits, multiplier) = match raw.chars().last() {
        Some('s') => (&raw[..raw.len() - 1], 1),
        Some('m') => (&raw[..raw.len() - 1], 60),
        Some('h') => (&raw[..raw.len() - 1], 60 * 60),
        Some('d') => (&raw[..raw.len() - 1], 24 * 60 * 60),
        Some(character) if character.is_ascii_digit() => (raw, 1),
        _ => return Err(refuse()),
    };
    let count: u64 = digits.parse().map_err(|_| refuse())?;
    count.checked_mul(multiplier).ok_or_else(refuse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_read_in_the_units_a_person_thinks_in() {
        assert_eq!(parse_window("15m").expect("parses"), 900);
        assert_eq!(parse_window("6h").expect("parses"), 21_600);
        assert_eq!(parse_window("7d").expect("parses"), 604_800);
        assert_eq!(parse_window("90").expect("parses"), 90, "a bare number is seconds");
    }

    #[test]
    fn an_unreadable_window_is_refused_rather_than_treated_as_zero() {
        // Zero would be a window containing nothing, which reads as "there is
        // no data" — the single most misleading thing this could do.
        assert!(parse_window("fifteen").is_err());
        assert!(parse_window("15x").is_err());
        assert!(parse_window("").is_err());
    }

    #[test]
    fn the_longest_matching_verb_wins() {
        let words = vec!["runs".to_owned(), "list".to_owned()];
        let found = find(&words).expect("matches");
        assert_eq!(found.words, ["runs", "list"]);
    }

    #[test]
    fn a_path_parameter_comes_from_the_argument_that_names_it() {
        let args = Args::parse(["id=run-7"]).expect("parses");
        assert_eq!(
            substitute_path("/api/v1/runs/{id}/events", &args).expect("substitutes"),
            "/api/v1/runs/run-7/events"
        );
    }

    #[test]
    fn a_missing_path_parameter_names_itself() {
        let args = Args::parse(["format=json"]).expect("parses");
        let error = substitute_path("/api/v1/runs/{id}", &args).expect_err("refuses");
        assert!(error.to_string().contains("id="), "{error}");
    }

    #[test]
    fn every_verb_declares_a_route_that_starts_at_the_api_root() {
        for read in READS {
            assert!(read.path.starts_with("/api/v1/"), "{}", read.path);
            assert!(!read.words.is_empty());
            assert!(!read.summary.is_empty(), "{}", read.path);
        }
    }
}
