//! Stream naming and positions.
//!
//! Emmett names a stream `<streamType>:<streamId>` and records two positions
//! per event: `streamPosition` (1-based, within the stream) and
//! `globalPosition` (across the whole store). We keep both, because they answer
//! different questions: stream position orders one run's events, global
//! position is what a consumer resumes from.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// 1-based index of an event inside its own stream.
pub type StreamPosition = u64;

/// Monotonic index of an event across the whole log.
pub type GlobalPosition = u64;

/// The stream a run's events live on.
///
/// One stream per **run**, not per conversation: a conversation can fan out
/// into several agent runs at once, and partitioning the log by conversation
/// would serialise runs that have no reason to wait for each other.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(try_from = "String", into = "String")]
pub struct StreamName {
    stream_type: String,
    stream_id: String,
}

impl StreamName {
    /// The stream type used for agent runs. Also the Laser topic name.
    pub const RUN: &'static str = "run";

    pub fn new(stream_type: impl Into<String>, stream_id: impl Into<String>) -> Self {
        Self {
            stream_type: stream_type.into(),
            stream_id: stream_id.into(),
        }
    }

    /// The canonical stream for one agent run.
    pub fn for_run(run_id: impl Into<String>) -> Self {
        Self::new(Self::RUN, run_id)
    }

    #[must_use]
    pub fn stream_type(&self) -> &str {
        &self.stream_type
    }

    #[must_use]
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The key a Laser producer partitions by.
    ///
    /// Partitioning by the whole stream name keeps every event of one run in
    /// order on one partition while letting unrelated runs proceed in parallel.
    #[must_use]
    pub fn partition_key(&self) -> String {
        self.to_string()
    }
}

impl FromStr for StreamName {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (stream_type, stream_id) =
            value
                .split_once(':')
                .ok_or_else(|| CoreError::InvalidStreamName {
                    value: value.to_owned(),
                })?;
        if stream_type.is_empty() || stream_id.is_empty() {
            return Err(CoreError::InvalidStreamName {
                value: value.to_owned(),
            });
        }
        Ok(Self::new(stream_type, stream_id))
    }
}

impl TryFrom<String> for StreamName {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<StreamName> for String {
    fn from(value: StreamName) -> Self {
        value.to_string()
    }
}

impl fmt::Display for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.stream_type, self.stream_id)
    }
}

impl fmt::Debug for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StreamName({}:{})", self.stream_type, self.stream_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type_and_id() {
        let name: StreamName = "run:01J8".parse().expect("valid");
        assert_eq!(name.stream_type(), "run");
        assert_eq!(name.stream_id(), "01J8");
        assert_eq!(name.to_string(), "run:01J8");
    }

    #[test]
    fn an_id_may_itself_contain_colons() {
        let name: StreamName = "run:tenant:a:01J8".parse().expect("valid");
        assert_eq!(name.stream_type(), "run");
        assert_eq!(name.stream_id(), "tenant:a:01J8");
    }

    #[test]
    fn rejects_names_without_a_separator_or_with_empty_halves() {
        assert!("run".parse::<StreamName>().is_err());
        assert!(":01J8".parse::<StreamName>().is_err());
        assert!("run:".parse::<StreamName>().is_err());
    }

    #[test]
    fn serialises_as_a_flat_string() {
        let name = StreamName::for_run("01J8");
        assert_eq!(
            serde_json::to_string(&name).expect("serializes"),
            "\"run:01J8\""
        );
    }
}
