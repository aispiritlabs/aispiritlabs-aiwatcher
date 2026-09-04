//! What this adapter refuses, and what it turns into the port's own errors.
//!
//! The interesting conversion is the one that is *not* an error. A unique
//! violation on `(execution_id, stream_version)` is two deciders racing to
//! write the same version, which is
//! [`StoreError::VersionConflict`](aiwatcher_execution::StoreError::VersionConflict)
//! — the handler re-reads and decides again. Reporting it as a database failure
//! would turn ordinary contention into a 500.

use crate::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PostgresError {
    #[error("connecting to PostgreSQL: {0}")]
    Connect(#[source] sqlx::Error),

    #[error("applying execution schema version {version}")]
    Migration {
        version: i64,
        #[source]
        source: sqlx::Error,
    },

    #[error(transparent)]
    Sql(#[from] sqlx::Error),

    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

/// PostgreSQL's own name for "somebody already wrote that row".
const UNIQUE_VIOLATION: &str = "23505";

/// Whether this failure is two writers racing rather than a broken database.
#[must_use]
pub fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database) if database.code().as_deref() == Some(UNIQUE_VIOLATION)
    )
}

impl From<PostgresError> for StoreError {
    fn from(error: PostgresError) -> Self {
        match error {
            PostgresError::Encoding(error) => Self::Encoding(error),
            other => Self::Backend(other.to_string()),
        }
    }
}

/// A `sqlx` failure as the port reports it, with contention told apart from
/// everything else.
#[must_use]
pub fn store_error(error: sqlx::Error, expected: u64, actual: u64) -> StoreError {
    if is_unique_violation(&error) {
        StoreError::VersionConflict { expected, actual }
    } else {
        StoreError::Backend(error.to_string())
    }
}
