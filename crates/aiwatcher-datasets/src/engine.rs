//! Which query engine a piece of authored text was written for.
//!
//! A deployment runs one query engine — Flow PHP, DataFusion or DuckDB — chosen
//! by `AIWATCHER_QUERY_ENGINE` (AW-3). What a person *writes* for one of them is
//! text in that engine's language: Flow's DSL, or Python against the engine's
//! own API. A transform and a recipe are text, so the text names its engine,
//! and a deployment runs what was written for it and refuses the rest by name
//! rather than guessing a language from what the text looks like.
//!
//! **Absent is `flow`, and stays absent when saved.** Everything stored before
//! this field existed was Flow, and a revision is the digest of what was saved:
//! a field written out as `"engine": "flow"` would move the digest of every
//! stored Flow revision, and the `plan_id` of every plan compiled from one.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// One of the query engines a deployment may run.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    ToSchema,
)]
pub enum QueryEngine {
    /// Flow PHP and its DSL. The default, and everything saved before AW-3.
    #[default]
    #[serde(rename = "flow")]
    Flow,
    /// Apache DataFusion, through its Python DataFrame API.
    #[serde(rename = "datafusion")]
    DataFusion,
    /// DuckDB, through its relational Python API.
    #[serde(rename = "duckdb")]
    DuckDb,
}

impl QueryEngine {
    /// Every engine, in the order a refusal lists them.
    pub const ALL: [Self; 3] = [Self::Flow, Self::DataFusion, Self::DuckDb];

    /// The word a deployment chooses it by, and the word stored beside text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::DataFusion => "datafusion",
            Self::DuckDb => "duckdb",
        }
    }

    /// Its name in a sentence somebody reads.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Flow => "Flow PHP",
            Self::DataFusion => "DataFusion",
            Self::DuckDb => "DuckDB",
        }
    }

    /// Whether this is the engine absence means.
    ///
    /// On a reference because that is what `skip_serializing_if` calls.
    #[must_use]
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub const fn is_flow(&self) -> bool {
        matches!(self, Self::Flow)
    }

    /// The words an engine is chosen by, together, for a refusal to name.
    #[must_use]
    pub fn offered() -> String {
        Self::ALL.map(Self::as_str).join(", ")
    }
}

impl fmt::Display for QueryEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A word that names no query engine.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("'{0}' is not a query engine; the engines are {offered}", offered = QueryEngine::offered())]
pub struct UnknownEngine(pub String);

impl FromStr for QueryEngine {
    type Err = UnknownEngine;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|engine| engine.as_str() == value.trim())
            .ok_or_else(|| UnknownEngine(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_is_chosen_by_the_word_it_is_stored_as() {
        for engine in QueryEngine::ALL {
            assert_eq!(engine.as_str().parse::<QueryEngine>(), Ok(engine));
            assert_eq!(
                serde_json::to_string(&engine).expect("an engine serialises"),
                format!("\"{engine}\"")
            );
        }
    }

    #[test]
    fn an_engine_that_is_not_offered_is_refused_naming_the_ones_that_are() {
        // Polars was measured and not taken (AW-3): as fast as DataFusion, with a
        // peak twenty times its size. The refusal says what may be chosen.
        let refused = "polars".parse::<QueryEngine>().expect_err("not an engine");
        assert_eq!(
            refused.to_string(),
            "'polars' is not a query engine; the engines are flow, datafusion, duckdb"
        );
    }
}
