//! The argument shape: `aiwatcher <words…> key=value… flag…`.
//!
//! Obsidian's own CLI rather than a flag parser, and the reason is that this
//! surface has two audiences at once. A person types `aiwatcher runs list
//! window=15m`, and what they typed reads back as what it means. A script types
//! the same thing with `format=json` on the end. Neither has to learn whether
//! an option is short or long, whether it takes `=` or a space, or which side
//! of the subcommand it goes on — a value is `key=value`, wherever it sits.
//!
//! Hand-rolled rather than `clap` for the same reason `role_argument` was: what
//! is parsed here is a word list and a map, and every line of it is in this
//! file. What that costs is the help text, which is written out in
//! [`crate::help`] instead of derived — and that is the half worth writing by
//! hand anyway.

use std::collections::BTreeMap;

use crate::CliError;

/// One parsed command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// The bare words, in order: `["runs", "list"]`.
    words: Vec<String>,
    /// Everything that arrived as `key=value`, first occurrence winning — so a
    /// wrapper script may put a default in front and let a person override it
    /// by typing the same key again.
    values: BTreeMap<String, String>,
}

impl Args {
    /// Parse what the process was given, minus the program name.
    ///
    /// # Errors
    ///
    /// [`CliError::Usage`] for a `=value` with no key, which is the one shape
    /// that cannot mean anything.
    pub fn parse<I, S>(raw: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut args = Self::default();
        for token in raw {
            let token = token.as_ref();
            match token.split_once('=') {
                Some((key, _)) if key.trim().is_empty() => {
                    return Err(CliError::Usage(format!(
                        "{token:?} has no name on the left of the `=`"
                    )));
                }
                Some((key, value)) => {
                    args.values
                        .entry(key.trim().to_owned())
                        .or_insert_with(|| value.to_owned());
                }
                // A leading dash is how every other CLI spells a flag, and
                // somebody will type it out of habit. Accepted as the bare
                // word it would have been, rather than refused: `--json` and
                // `json` mean the same thing and there is nothing to be gained
                // by being strict about which.
                None => args.words.push(token.trim_start_matches('-').to_owned()),
            }
        }
        Ok(args)
    }

    /// The command words, in order.
    #[must_use]
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// The word at `index`, if there is one.
    #[must_use]
    pub fn word(&self, index: usize) -> Option<&str> {
        self.words.get(index).map(String::as_str)
    }

    /// Consume the leading words a command's name is made of, leaving the rest
    /// as flags.
    pub fn take_words(&mut self, count: usize) {
        self.words.drain(..count.min(self.words.len()));
    }

    /// A named value, if it was given.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// A named value, or the default.
    #[must_use]
    pub fn value_or<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.value(key).unwrap_or(fallback)
    }

    /// A named value that has to be there.
    ///
    /// # Errors
    ///
    /// [`CliError::Usage`] naming the key, because a missing one is a typo
    /// somebody is about to look for.
    pub fn require(&self, key: &str) -> Result<&str, CliError> {
        self.value(key)
            .ok_or_else(|| CliError::Usage(format!("this command needs {key}=…")))
    }

    /// A named value parsed as a number.
    ///
    /// # Errors
    ///
    /// [`CliError::Usage`] naming the key and what arrived.
    pub fn number(&self, key: &str) -> Result<Option<usize>, CliError> {
        let Some(raw) = self.value(key) else {
            return Ok(None);
        };
        raw.parse()
            .map(Some)
            .map_err(|_| CliError::Usage(format!("{key}={raw:?} is not a whole number")))
    }

    /// Whether a bare word was given — `detach`, `follow`, `force`.
    #[must_use]
    pub fn flag(&self, name: &str) -> bool {
        self.words.iter().any(|word| word == name)
    }

    /// Every `key=value`, for the commands that pass them straight through.
    #[must_use]
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// The values that are not one of `known`, in order.
    ///
    /// What a passthrough sends as query parameters, and what a typo check
    /// reports. Its own method because both callers want the same answer and
    /// one of them getting it wrong is a silently ignored argument.
    #[must_use]
    pub fn other_values<'a>(&'a self, known: &[&str]) -> Vec<(&'a str, &'a str)> {
        self.values
            .iter()
            .filter(|(key, _)| !known.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_may_sit_anywhere_on_the_line() {
        let before = Args::parse(["format=json", "runs", "list"]).expect("parses");
        let after = Args::parse(["runs", "list", "format=json"]).expect("parses");
        assert_eq!(before, after);
        assert_eq!(before.words(), ["runs", "list"]);
        assert_eq!(before.value("format"), Some("json"));
    }

    #[test]
    fn a_value_containing_an_equals_sign_keeps_it() {
        // Base64 is the case: a secret ends in padding, and truncating it at
        // the second `=` would produce a token that authenticates as nothing.
        let args = Args::parse(["token=abc=="]).expect("parses");
        assert_eq!(args.value("token"), Some("abc=="));
    }

    #[test]
    fn the_first_occurrence_of_a_key_wins_so_a_wrapper_can_be_overridden() {
        let args = Args::parse(["window=5m", "window=1h"]).expect("parses");
        assert_eq!(args.value("window"), Some("5m"));
    }

    #[test]
    fn a_dashed_flag_means_the_bare_word_it_looks_like() {
        let args = Args::parse(["runs", "list", "--json"]).expect("parses");
        assert!(args.flag("json"));
    }

    #[test]
    fn a_value_with_no_name_is_refused_rather_than_ignored() {
        let error = Args::parse(["=json"]).expect_err("refuses");
        assert!(matches!(error, CliError::Usage(_)));
    }

    #[test]
    fn other_values_reports_what_a_command_did_not_name() {
        let args = Args::parse(["format=json", "agent=planner"]).expect("parses");
        assert_eq!(args.other_values(&["format"]), [("agent", "planner")]);
    }
}
